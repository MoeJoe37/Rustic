//! [`ElementView`] for `rustic-dom` documents.

use rustic_css::{CssParser, SelectorList};
use rustic_dom::{Document, DomError, Element, Node, NodeId, NodeKind};

use crate::matching::{MatchCache, matches_complex_cached};
use crate::{
    AttributeRef, Cascade, ComputeContext, ComputedStyle, ElementView, MatchContext,
    SpecifiedStyle, StyleDiagnostic, parse_style_attribute,
};

/// An element of a [`Document`], borrowed for matching.
#[derive(Clone, Copy, Debug)]
pub struct DomElement<'a> {
    document: &'a Document,
    id: NodeId,
    node: &'a Node,
    element: &'a Element,
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
        let node = document.node(id).ok()?;
        match node.kind() {
            NodeKind::Element(element) => Some(Self {
                document,
                id,
                node,
                element,
            }),
            _ => None,
        }
    }

    /// The element's node ID.
    pub fn id(&self) -> NodeId {
        self.id
    }

    fn element_at(
        &self,
        mut id: Option<NodeId>,
        next: fn(&Node) -> Option<NodeId>,
    ) -> Option<Self> {
        while let Some(current) = id {
            if let Some(element) = Self::new(self.document, current) {
                return Some(element);
            }
            id = next(self.document.node(current).ok()?);
        }
        None
    }
}

impl ElementView for DomElement<'_> {
    fn parent_element(&self) -> Option<Self> {
        Self::new(self.document, self.node.parent()?)
    }

    fn previous_sibling_element(&self) -> Option<Self> {
        self.element_at(self.node.previous_sibling(), Node::previous_sibling)
    }

    fn next_sibling_element(&self) -> Option<Self> {
        self.element_at(self.node.next_sibling(), Node::next_sibling)
    }

    fn is_root(&self) -> bool {
        self.node
            .parent()
            .and_then(|p| self.document.node(p).ok())
            .is_some_and(|p| matches!(p.kind(), NodeKind::Document))
    }

    fn local_name(&self) -> &str {
        self.element.name().local()
    }

    fn namespace(&self) -> Option<&str> {
        self.element.name().namespace()
    }

    fn attributes(&self) -> impl Iterator<Item = AttributeRef<'_>> {
        self.element.attributes().iter().map(|a| AttributeRef {
            namespace: a.name().namespace(),
            local: a.name().local(),
            value: a.value().as_units(),
        })
    }

    fn is_empty(&self) -> bool {
        let Ok(mut children) = self.document.children(self.id) else {
            return true;
        };
        !children.any(|child| match self.document.node(child).map(Node::kind) {
            Ok(NodeKind::Element(_)) => true,
            Ok(NodeKind::Text(data) | NodeKind::CdataSection(data)) => !data.as_units().is_empty(),
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

/// Style every element in the subtree rooted at `root`, in tree order: run
/// the cascade, then compute values, with each element inheriting its parent
/// element's computed values (template contents are not styled).
///
/// `style` attributes on elements are parsed with `parser`; if one exceeds
/// the parser's limits it is ignored (reported as an empty declaration list).
/// When `root` is not the document's root element, the subtree is styled as
/// if `root` had no parent.
pub fn style_tree(
    document: &Document,
    root: NodeId,
    cascade: &Cascade,
    parser: &CssParser,
    cx: &MatchContext<DomElement<'_>>,
    compute: &ComputeContext,
) -> Result<Vec<ElementStyle>, DomError> {
    let mut out: Vec<ElementStyle> = Vec::new();
    // (depth, index into `out`) of the styled ancestors on the current path.
    let mut path: Vec<(usize, usize)> = Vec::new();
    let mut root_style: Option<usize> = None;
    // The tree and the cascade stay borrowed and unchanged for the whole
    // restyle, so match results can be shared.
    let cache = MatchCache::default();
    for (id, depth) in document.walk(root)? {
        while path.last().is_some_and(|&(d, _)| d >= depth) {
            path.pop();
        }
        let Some(element) = DomElement::new(document, id) else {
            continue;
        };
        let style_attribute = element
            .attributes()
            .find(|a| a.namespace.is_none() && a.local == "style")
            .map(|a| String::from_utf16_lossy(a.value));
        let (inline, inline_diagnostics) = match style_attribute {
            Some(source) => parse_style_attribute(parser, &source).unwrap_or_default(),
            None => Default::default(),
        };
        // Only a direct parent element passes its style on.
        let parent = path
            .last()
            .filter(|&&(d, _)| d + 1 == depth)
            .map(|&(_, i)| &out[i].computed);
        let specified = cascade.specified_style_cached(element, cx, &inline, parent, &cache);
        let root_computed = root_style.map(|i| &out[i].computed);
        let computed = ComputedStyle::compute(
            &specified,
            parent,
            root_computed,
            element.is_root() || root_style.is_none(),
            compute,
        );
        if root_style.is_none() {
            root_style = Some(out.len());
        }
        path.push((depth, out.len()));
        out.push(ElementStyle {
            node: id,
            specified,
            computed,
            inline_diagnostics,
        });
    }
    Ok(out)
}
