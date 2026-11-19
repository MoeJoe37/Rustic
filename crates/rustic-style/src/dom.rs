//! [`ElementView`] for `rustic-dom` documents.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use rustic_base::CancellationToken;

use rustic_css::{CssParser, SelectorList};
use rustic_dom::{Document, DomError, Element, Node, NodeId, NodeKind, Walk};

use crate::matching::{MatchCache, matches_complex_cached};
use crate::{
    AttributeRef, Cascade, ChildRef, ComputeContext, ComputedStyle, ContainerInfo, ContainerSize,
    ElementView, MatchContext, SpecifiedStyle, StyleDiagnostic, StyleInputs, TreeContext,
    TreeScope, parse_style_attribute, resolve_declaration_urls,
};

/// An element of a [`Document`], borrowed for matching.
#[derive(Clone, Copy, Debug)]
pub struct DomElement<'a> {
    document: &'a Document,
    id: NodeId,
    node: &'a Node,
    element: Option<&'a Element>,
    work: Option<&'a SelectorWork<'a>>,
}

impl PartialEq for DomElement<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && std::ptr::eq(self.document, other.document)
    }
}

impl Eq for DomElement<'_> {}

impl std::hash::Hash for DomElement<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        std::ptr::from_ref(self.document).hash(state);
    }
}

impl<'a> DomElement<'a> {
    /// The element `id`, or `None` if `id` is not an element of `document`.
    pub fn new(document: &'a Document, id: NodeId) -> Option<Self> {
        Self::with_work(document, id, None)
    }

    fn with_work(
        document: &'a Document,
        id: NodeId,
        work: Option<&'a SelectorWork<'a>>,
    ) -> Option<Self> {
        if work.is_some_and(|work| !work.spend(1)) {
            return None;
        }
        let node = document.node(id).ok()?;
        let element = match node.kind() {
            NodeKind::Element(element) => Some(element),
            NodeKind::DocumentFragment if work.is_some() => None,
            _ => return None,
        };
        Some(Self {
            document,
            id,
            node,
            element,
            work,
        })
    }

    /// The element's node ID.
    pub fn id(&self) -> NodeId {
        self.id
    }

    pub(crate) fn document(&self) -> &'a Document {
        self.document
    }

    fn spend(&self, amount: u64) -> bool {
        self.work.is_none_or(|work| work.spend(amount))
    }

    fn element_at(
        &self,
        mut id: Option<NodeId>,
        next: fn(&Node) -> Option<NodeId>,
    ) -> Option<Self> {
        while let Some(current) = id {
            if !self.spend(1) {
                return None;
            }
            let node = self.document.node(current).ok()?;
            if let NodeKind::Element(element) = node.kind() {
                return Some(Self {
                    id: current,
                    node,
                    element: Some(element),
                    ..*self
                });
            }
            id = next(node);
        }
        None
    }
}

impl ElementView for DomElement<'_> {
    fn is_virtual_scope(&self) -> bool {
        self.element.is_none()
    }

    fn parent_element(&self) -> Option<Self> {
        Self::with_work(self.document, self.node.parent()?, self.work)
    }

    fn previous_sibling_element(&self) -> Option<Self> {
        self.element_at(self.node.previous_sibling(), Node::previous_sibling)
    }

    fn next_sibling_element(&self) -> Option<Self> {
        self.element_at(self.node.next_sibling(), Node::next_sibling)
    }

    fn parser_form(&self) -> Option<Self> {
        let form = self.document.parser_form(self.id)?;
        Self::with_work(self.document, form, self.work)
    }

    fn parent_shadow_host(&self) -> Option<Self> {
        let parent = self.document.node(self.node.parent()?).ok()?;
        Self::with_work(self.document, parent.shadow_host()?, self.work)
    }

    fn is_root(&self) -> bool {
        self.spend(1)
            && self
                .node
                .parent()
                .and_then(|p| self.document.node(p).ok())
                .is_some_and(|p| matches!(p.kind(), NodeKind::Document))
    }

    fn local_name(&self) -> &str {
        if self.spend(1) {
            self.element.map_or("", |element| element.name().local())
        } else {
            ""
        }
    }

    fn namespace(&self) -> Option<&str> {
        if self.spend(1) {
            self.element.and_then(|element| element.name().namespace())
        } else {
            None
        }
    }

    fn attributes(&self) -> impl Iterator<Item = AttributeRef<'_>> {
        self.element
            .into_iter()
            .flat_map(Element::attributes)
            .take_while(|a| self.spend(1 + a.value().as_units().len() as u64 / 64))
            .map(|a| AttributeRef {
                namespace: a.name().namespace(),
                local: a.name().local(),
                value: a.value().as_units(),
            })
    }

    fn children(&self) -> impl Iterator<Item = ChildRef<'_, Self>> {
        let document = self.document;
        document
            .children(self.id)
            .into_iter()
            .flatten()
            .take_while(|_| self.spend(1))
            .filter_map(move |child| match document.node(child).ok()?.kind() {
                NodeKind::Element(_) => {
                    Self::with_work(document, child, self.work).map(ChildRef::Element)
                }
                NodeKind::Text(data) | NodeKind::CdataSection(data) => {
                    Some(ChildRef::Text(data.as_units()))
                }
                _ => None,
            })
    }

    fn first_child_element(&self) -> Option<Self> {
        let first = self.document.node(self.id).ok()?.first_child();
        self.element_at(first, Node::next_sibling)
    }

    fn is_empty(&self) -> bool {
        let Ok(mut children) = self.document.children(self.id) else {
            return true;
        };
        !children
            .by_ref()
            .take_while(|_| self.spend(1))
            .any(|child| match self.document.node(child).map(Node::kind) {
                Ok(NodeKind::Element(_)) => true,
                Ok(NodeKind::Text(data) | NodeKind::CdataSection(data)) => {
                    !data.as_units().is_empty()
                }
                _ => false,
            })
    }
}

/// Every element in the subtree rooted at `root` (including `root`) that
/// matches `list`, in tree order. Template contents are not searched.
///
/// Match results (nested lists and `:nth-*` positions) are shared across the
/// walk, so cost is proportional to elements × selector work.
pub fn query_selector_all(
    document: &Document,
    root: NodeId,
    list: &SelectorList,
    cx: &MatchContext<DomElement<'_>>,
) -> Result<Vec<NodeId>, DomError> {
    let mut found = Vec::new();
    let cache = MatchCache::default();
    for (id, _) in document.walk(root)? {
        if let Some(element) = DomElement::new(document, id) {
            let matched = list.selectors().iter().any(|s| {
                s.pseudo_element().is_none() && matches_complex_cached(s, element, cx, &cache)
            });
            if matched {
                found.push(id);
            }
        }
    }
    Ok(found)
}

/// Exhaustion or failure in a script-visible selector query.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectorQueryError {
    /// An invalid tree handle or an allocation failure.
    Dom(DomError),
    /// A host cancellation token was set.
    Cancelled,
    /// The query exhausted its work allowance.
    WorkLimit,
}

/// Per-query work allowance shared by the tree walk and selector traversal.
#[derive(Debug)]
pub struct SelectorWork<'a> {
    remaining: Cell<u64>,
    failure: Cell<Option<SelectorQueryError>>,
    cancellation: Option<&'a CancellationToken>,
}

impl<'a> SelectorWork<'a> {
    /// Set a deterministic allowance and optional cancellation token.
    pub fn new(allowance: u64, cancellation: Option<&'a CancellationToken>) -> Self {
        Self {
            remaining: Cell::new(allowance),
            failure: Cell::new(None),
            cancellation,
        }
    }

    /// Work left for the caller's surrounding task.
    pub fn remaining(&self) -> u64 {
        self.remaining.get()
    }

    /// Spend work, stopping further traversal after cancellation or exhaustion.
    pub fn spend(&self, amount: u64) -> bool {
        if self.failure.get().is_some() {
            return false;
        }
        if self
            .cancellation
            .is_some_and(CancellationToken::is_cancelled)
        {
            self.failure.set(Some(SelectorQueryError::Cancelled));
            return false;
        }
        let Some(left) = self.remaining.get().checked_sub(amount) else {
            self.remaining.set(0);
            self.failure.set(Some(SelectorQueryError::WorkLimit));
            return false;
        };
        self.remaining.set(left);
        true
    }

    /// Report the stop reason before publishing a query result.
    pub fn check(&self) -> Result<(), SelectorQueryError> {
        self.failure.get().map_or(Ok(()), Err)
    }
}

/// A scoped query over a fixed HTML tree and one parsed selector list.
pub struct ScopedSelector<'a> {
    document: &'a Document,
    list: &'a SelectorList,
    scope: NodeId,
    context: MatchContext<DomElement<'a>>,
    work: &'a SelectorWork<'a>,
    cache: MatchCache<DomElement<'a>>,
}

impl<'a> ScopedSelector<'a> {
    /// Prepare a query. An element scope participates in selector matching
    /// even when the query only returns its descendants.
    pub fn new(
        document: &'a Document,
        scope: NodeId,
        list: &'a SelectorList,
        quirks: bool,
        work: &'a SelectorWork<'a>,
    ) -> Self {
        let scope_element = DomElement::with_work(document, scope, Some(work));
        Self {
            document,
            list,
            scope,
            work,
            context: MatchContext {
                scope: scope_element,
                quirks,
                ..MatchContext::html()
            },
            cache: MatchCache::default(),
        }
    }

    fn matches_element(&self, element: DomElement<'a>) -> bool {
        self.list.selectors().iter().any(|selector| {
            self.work.spend(1)
                && selector.pseudo_element().is_none()
                && matches_complex_cached(selector, element, &self.context, &self.cache)
        })
    }

    /// Test the scoping element itself.
    pub fn matches(&self) -> Result<bool, SelectorQueryError> {
        let result = self
            .context
            .scope
            .is_some_and(|e| !e.is_virtual_scope() && self.matches_element(e));
        self.work.check()?;
        Ok(result)
    }

    /// Find the nearest matching inclusive element ancestor.
    pub fn closest(&self) -> Result<Option<NodeId>, SelectorQueryError> {
        let answer = std::iter::successors(self.context.scope, ElementView::parent_element)
            .find(|&element| !element.is_virtual_scope() && self.matches_element(element))
            .map(|element| element.id());
        self.work.check()?;
        Ok(answer)
    }

    fn descendant_matches(&self) -> Result<impl Iterator<Item = NodeId> + '_, SelectorQueryError> {
        Ok(self
            .document
            .walk(self.scope)
            .map_err(SelectorQueryError::Dom)?
            .skip(1)
            .take_while(|_| self.work.spend(1))
            .filter_map(|(id, _)| DomElement::with_work(self.document, id, Some(self.work)))
            .filter(|&element| self.matches_element(element))
            .map(|element| element.id()))
    }

    /// Find the first matching descendant in tree order.
    pub fn first(&self) -> Result<Option<NodeId>, SelectorQueryError> {
        let result = self.descendant_matches()?.next();
        self.work.check()?;
        Ok(result)
    }

    /// Collect all matching descendants once, in tree order.
    pub fn all(&self) -> Result<Vec<NodeId>, SelectorQueryError> {
        let mut result = Vec::new();
        for id in self.descendant_matches()? {
            result
                .try_reserve(1)
                .map_err(|_| SelectorQueryError::Dom(DomError::AllocationFailed))?;
            result.push(id);
        }
        self.work.check()?;
        Ok(result)
    }
}

/// One element's style from [`style_tree`].
#[derive(Clone, Debug, PartialEq)]
pub struct ElementStyle {
    /// The element.
    pub node: NodeId,
    /// Its specified style (the cascade's output).
    pub specified: SpecifiedStyle,
    /// Its computed style.
    pub computed: ComputedStyle,
    /// Diagnostics from its `style` attribute.
    pub inline_diagnostics: Vec<StyleDiagnostic>,
}

/// The [`TreeContext`] of the shadow tree rooted at `shadow_root`.
///
/// Derived from the node's slot, so it is stable while the shadow root
/// lives and never equals [`TreeContext::DOCUMENT`].
pub fn shadow_tree_context(shadow_root: NodeId) -> TreeContext {
    TreeContext(u64::from(shadow_root.slot()) + 1)
}

/// One tree being walked by [`style_tree`].
struct TreeWalk<'a> {
    walk: Walk<'a>,
    /// Depth added to the walk's depths; a shadow tree's walk starts at
    /// its host's depth, and its root fragment is skipped.
    base: usize,
    shadow: bool,
    scope: TreeScope<DomElement<'a>>,
}

/// Style every element in the subtree rooted at `root`, in shadow-including
/// tree order: run the cascade, then compute values, with each element
/// inheriting its parent element's computed values. A shadow tree is styled
/// right after its host, before the host's children, and its top-level
/// elements inherit from the host; its author sheets are those pushed with
/// [`shadow_tree_context`] (see [`crate::document_cascade`]). Template
/// contents are not styled. Slots are not assigned yet, so the host's light
/// children inherit from the host (TD-23).
///
/// `style` attributes on elements are parsed with `parser`; if one exceeds
/// the parser's limits it is ignored (reported as an empty declaration list).
/// When `root` is not the document's root element, the subtree is styled as
/// if `root` had no parent and were in the document tree.
pub fn style_tree<'a>(
    document: &'a Document,
    root: NodeId,
    cascade: &Cascade,
    parser: &CssParser,
    cx: &MatchContext<DomElement<'a>>,
    compute: &ComputeContext,
) -> Result<Vec<ElementStyle>, DomError> {
    style_walk(
        document,
        root,
        cascade,
        parser,
        cx,
        (compute, &HashMap::new()),
        None,
    )
    .map(|walk| walk.styles)
}

/// [`style_tree`] with the content-box sizes of query containers, as
/// layout would report them, for `@container` size features. Elements
/// without a size answer size features with unknown.
pub fn style_tree_with_sizes<'a>(
    document: &'a Document,
    root: NodeId,
    cascade: &Cascade,
    parser: &CssParser,
    cx: &MatchContext<DomElement<'a>>,
    compute: &ComputeContext,
    sizes: &HashMap<NodeId, ContainerSize>,
) -> Result<Vec<ElementStyle>, DomError> {
    style_walk(document, root, cascade, parser, cx, (compute, sizes), None).map(|walk| walk.styles)
}

/// Styles kept from an earlier walk and what has to be recomputed; see
/// [`crate::StyledDocument`].
pub(crate) struct Reuse<'r> {
    /// The earlier styles with the style parent (parent element or host)
    /// each had then.
    pub previous: HashMap<NodeId, (ElementStyle, Option<NodeId>)>,
    pub dirty: &'r Dirty,
}

/// Elements whose styles may be stale.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Dirty {
    /// Everything.
    pub all: bool,
    /// These elements themselves.
    pub elements: HashSet<NodeId>,
    /// These elements and everything below them (shadow trees included).
    pub subtrees: HashSet<NodeId>,
    /// Children were inserted or moved somewhere: walk the tree to find
    /// elements without a style or with a new style parent.
    pub children: bool,
}

impl Dirty {
    pub fn is_clean(&self) -> bool {
        !self.all && !self.children && self.elements.is_empty() && self.subtrees.is_empty()
    }
}

pub(crate) struct Walked {
    pub styles: Vec<ElementStyle>,
    /// Style parents in the same order as `styles`.
    pub parents: Vec<Option<NodeId>>,
    /// How many elements went through the cascade.
    pub recomputed: usize,
}

/// One styled ancestor on the current path.
#[derive(Clone, Copy)]
struct PathEntry {
    depth: usize,
    index: usize,
    /// Everything below must be recomputed (a dirty subtree).
    subtree: bool,
    /// Its computed style changed, so its children must be recomputed.
    changed: bool,
}

/// The walk behind [`style_tree`]; with `reuse`, an element keeps its
/// earlier style unless it is dirty, has a new style parent, or its parent
/// (or the root) got a different computed style. With `@container` rules in
/// the cascade, a changed computed style restyles the whole subtree, since
/// any descendant may query it.
pub(crate) fn style_walk<'a>(
    document: &'a Document,
    root: NodeId,
    cascade: &Cascade,
    parser: &CssParser,
    cx: &MatchContext<DomElement<'a>>,
    (compute, sizes): (&ComputeContext, &HashMap<NodeId, ContainerSize>),
    mut reuse: Option<Reuse<'_>>,
) -> Result<Walked, DomError> {
    let container_queries = cascade.has_container_queries();
    let mut out: Vec<ElementStyle> = Vec::new();
    let mut parents: Vec<Option<NodeId>> = Vec::new();
    let mut recomputed = 0usize;
    let mut path: Vec<PathEntry> = Vec::new();
    let mut root_style: Option<usize> = None;
    // A changed root style changes `rem` and friends everywhere.
    let mut root_changed = false;
    // The tree and the cascade stay borrowed and unchanged for the whole
    // restyle, so match results can be shared.
    let cache = MatchCache::default();
    // An explicit stack of tree walks, bounded by live nodes, so nested
    // shadow trees never recurse.
    let mut walks = vec![TreeWalk {
        walk: document.walk(root)?,
        base: 0,
        shadow: false,
        scope: TreeScope::document(),
    }];
    while let Some(tree) = walks.last_mut() {
        let Some((id, depth)) = tree.walk.next() else {
            walks.pop();
            continue;
        };
        if tree.shadow && depth == 0 {
            continue; // The shadow root itself, standing in for its host.
        }
        let (scope, depth) = (tree.scope, tree.base + depth);
        while path.last().is_some_and(|e| e.depth >= depth) {
            path.pop();
        }
        let Some(element) = DomElement::new(document, id) else {
            continue;
        };
        // Only a direct parent element (or host) passes its style on.
        let parent_entry = path.last().copied().filter(|e| e.depth + 1 == depth);
        let parent_id = parent_entry.map(|e| out[e.index].node);
        let shadow_root = element.element.and_then(Element::shadow_root);

        let mut subtree = parent_entry.is_some_and(|e| e.subtree);
        let kept = match reuse.as_mut() {
            None => None,
            Some(reuse) => {
                subtree |= reuse.dirty.subtrees.contains(&id);
                let must = reuse.dirty.all
                    || subtree
                    || root_changed
                    || parent_entry.is_some_and(|e| e.changed)
                    || reuse.dirty.elements.contains(&id);
                match reuse.previous.remove(&id) {
                    Some((style, old_parent)) if !must && old_parent == parent_id => {
                        Some(Ok(style))
                    }
                    Some((style, old_parent)) => {
                        // A moved element is styled afresh, with its subtree.
                        subtree |= old_parent != parent_id;
                        Some(Err(Some(style)))
                    }
                    None => {
                        subtree = true;
                        Some(Err(None))
                    }
                }
            }
        };
        let (style, changed) = match kept {
            Some(Ok(style)) => (style, false),
            other => {
                let previous = match other {
                    Some(Err(previous)) => previous,
                    _ => None,
                };
                recomputed += 1;
                let style_attribute = element
                    .attributes()
                    .find(|a| a.namespace.is_none() && a.local == "style")
                    .map(|a| String::from_utf16_lossy(a.value));
                let (mut inline, inline_diagnostics) = match style_attribute {
                    Some(source) => parse_style_attribute(parser, &source).unwrap_or_default(),
                    None => Default::default(),
                };
                if let Some(base) = cascade.document_base_url() {
                    resolve_declaration_urls(&mut inline, base);
                }
                let containers: Vec<ContainerInfo<'_, DomElement<'a>>> = if container_queries {
                    path.iter()
                        .filter_map(|e| {
                            let ancestor = &out[e.index];
                            Some(ContainerInfo {
                                element: DomElement::new(document, ancestor.node)?,
                                style: &ancestor.computed,
                                size: sizes.get(&ancestor.node).copied(),
                            })
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let inputs = StyleInputs {
                    style_attribute: &inline,
                    scope,
                    shadow_root: shadow_root.map(shadow_tree_context),
                    containers: &containers,
                    ..StyleInputs::document()
                };
                let parent = parent_entry.map(|e| &out[e.index].computed);
                let specified =
                    cascade.specified_style_cached(element, cx, &inputs, parent, &cache);
                let root_computed = root_style.map(|i| &out[i].computed);
                let computed = ComputedStyle::compute(
                    &specified,
                    parent,
                    root_computed,
                    element.is_root() || root_style.is_none(),
                    compute,
                );
                let changed = previous.as_ref().is_none_or(|p| p.computed != computed);
                (
                    ElementStyle {
                        node: id,
                        specified,
                        computed,
                        inline_diagnostics,
                    },
                    changed,
                )
            }
        };
        if root_style.is_none() {
            root_style = Some(out.len());
            root_changed = changed && reuse.is_some();
        }
        path.push(PathEntry {
            depth,
            index: out.len(),
            subtree: subtree || (container_queries && changed),
            changed,
        });
        out.push(style);
        parents.push(parent_id);
        if let Some(shadow_root) = shadow_root {
            walks.push(TreeWalk {
                walk: document.walk(shadow_root)?,
                base: depth,
                shadow: true,
                scope: TreeScope {
                    context: shadow_tree_context(shadow_root),
                    host: Some(element),
                    depth: scope.depth.saturating_add(1),
                },
            });
        }
    }
    Ok(Walked {
        styles: out,
        parents,
        recomputed,
    })
}
