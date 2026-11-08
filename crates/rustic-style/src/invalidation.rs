//! Incremental restyling: which elements a DOM mutation can affect, and a
//! restyle that recomputes only those (CSS-06).
//!
//! [`StyledDocument`] keeps the styles of a full [`crate::style_tree`] walk. The
//! caller reports each mutation after making it ([`StyledDocument::note`]),
//! and [`StyledDocument::restyle`] then recomputes the elements the
//! mutations may have affected, reusing every other element's style. The
//! result always equals a fresh [`crate::style_tree`] walk; the tests and the
//! `css-invalidation` fuzz target check exactly that.
//!
//! What a mutation affects comes from an [`InvalidationMap`] built from the
//! cascade's selectors:
//!
//! * An attribute that no selector tests (and that no element state reads)
//!   affects nothing, except `style`, which affects its own element.
//! * An attribute tested only in subject compounds affects its element; one
//!   tested in an ancestor compound also affects the element's subtree, and
//!   with sibling combinators the following siblings' subtrees too.
//! * A change to an element's children affects the children themselves when
//!   any selector is structural (`:nth-*`, `:*-child`, `:*-of-type`,
//!   `:empty`, `+`, `~`) and the parent (for `:empty`); inserted or moved
//!   elements are always styled afresh with their subtrees.
//! * Selectors whose result depends on facts far away in the tree (`:has()`
//!   and the HTML element states such as `:checked`, `:default`, `:lang()`
//!   and `:dir()`) make the relevant mutations restyle the whole document.
//!   This is conservative but correct; narrower invalidation for them is
//!   TD-32.
//!
//! A computed-style change on an element restyles its children (they
//! inherit), and a change on the root restyles everything (`rem`, `rlh`).

use std::collections::HashMap;

use rustic_css::{
    Combinator, ComplexSelector, CssParser, PseudoClass, SelectorList, SimpleSelector,
};
use rustic_dom::{Document, DomError, NodeId, NodeKind};

use crate::dom::{Dirty, Reuse, style_walk};
use crate::{
    Cascade, ComputeContext, ContainerSize, DomElement, ElementStyle, MatchContext,
    style_tree_with_sizes,
};

/// Attributes that HTML element states (form states, `:lang()`, `:dir()`)
/// read, anywhere in the tree.
const STATE_ATTRIBUTES: &[&str] = &[
    "checked",
    "content",
    "contenteditable",
    "dir",
    "disabled",
    "form",
    "http-equiv",
    "id",
    "lang",
    "multiple",
    "name",
    "placeholder",
    "readonly",
    "required",
    "selected",
    "size",
    "type",
    "value",
];

/// What the cascade's selectors depend on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InvalidationMap {
    /// Attribute local names (ASCII lowercase) that selectors test, with
    /// whether any test sits outside a subject compound. `#id` counts as
    /// `id` and `.class` as `class`.
    attributes: HashMap<String, bool>,
    /// Any `:nth-*`, `:first-*`, `:last-*`, `:only-*` or `:empty`.
    structural: bool,
    /// Any `+` or `~` combinator.
    siblings: bool,
    /// Any `:has()`.
    has: bool,
    /// Any HTML element state that reads whole-tree facts.
    states: bool,
}

impl InvalidationMap {
    /// The dependencies of every selector in `cascade`.
    pub fn new(cascade: &Cascade) -> Self {
        let mut map = Self::default();
        for sheet in cascade.sheets() {
            for rule in &sheet.rules {
                map.list(&rule.selectors, true);
            }
        }
        map
    }

    fn list(&mut self, list: &SelectorList, subject: bool) {
        for selector in list.selectors() {
            self.complex(selector, subject);
        }
    }

    fn complex(&mut self, selector: &ComplexSelector, subject: bool) {
        if selector
            .combinators()
            .iter()
            .any(|c| matches!(c, Combinator::NextSibling | Combinator::SubsequentSibling))
        {
            self.siblings = true;
        }
        let last = selector.compounds().len().saturating_sub(1);
        for (i, compound) in selector.compounds().iter().enumerate() {
            let subject = subject && i == last;
            for simple in compound.simple_selectors() {
                self.simple(simple, subject);
            }
        }
    }

    fn attribute(&mut self, name: &str, subject: bool) {
        let entry = self
            .attributes
            .entry(name.to_ascii_lowercase())
            .or_insert(false);
        *entry |= !subject;
    }

    fn simple(&mut self, simple: &SimpleSelector, subject: bool) {
        match simple {
            SimpleSelector::Id(_) => self.attribute("id", subject),
            SimpleSelector::Class(_) => self.attribute("class", subject),
            SimpleSelector::Attribute(a) => self.attribute(&a.local, subject),
            SimpleSelector::PseudoClass(class) => self.pseudo_class(class, subject),
            SimpleSelector::Nesting(Some(list)) => self.list(list, subject),
            SimpleSelector::Type { .. }
            | SimpleSelector::Universal { .. }
            | SimpleSelector::Nesting(None)
            | SimpleSelector::RelativeAnchor => {}
        }
    }

    fn pseudo_class(&mut self, class: &PseudoClass, subject: bool) {
        match class {
            PseudoClass::Empty | PseudoClass::OnlyChild | PseudoClass::OnlyOfType => {
                self.structural = true
            }
            PseudoClass::Nth { nth, .. } => {
                self.structural = true;
                if let Some(of) = &nth.of {
                    self.list(of, subject);
                }
            }
            PseudoClass::Is(list) | PseudoClass::Where(list) | PseudoClass::Not(list) => {
                self.list(list, subject);
            }
            PseudoClass::Host(list) => {
                if let Some(list) = list {
                    // The host is outside the shadow tree whose elements
                    // the rule styles.
                    self.list(list, false);
                }
            }
            PseudoClass::Has(list) => {
                self.has = true;
                for relative in list.selectors() {
                    self.complex(relative.selector(), false);
                }
            }
            PseudoClass::AnyLink { .. } => self.attribute("href", subject),
            PseudoClass::Form(_) | PseudoClass::Lang(_) | PseudoClass::Dir(_) => {
                self.states = true;
            }
            PseudoClass::Root
            | PseudoClass::Scope
            | PseudoClass::Visited
            | PseudoClass::State(_) => {}
        }
    }
}

/// A DOM change, reported after it was made.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Mutation {
    /// An attribute of `element` was set, changed or removed.
    Attribute {
        /// The element.
        element: NodeId,
        /// The attribute's local name (compared ASCII case-insensitively).
        local: String,
    },
    /// Nodes were inserted into, removed from or moved within `parent`
    /// (an element, shadow root, document or fragment).
    Children {
        /// The parent.
        parent: NodeId,
    },
    /// The data of a text node (or another character-data node) changed.
    /// The text of a `<style>` element changes a stylesheet as well: rebuild
    /// the cascade and report [`StyledDocument::cascade_changed`].
    Text {
        /// The node.
        node: NodeId,
    },
    /// Anything else, such as a changed stylesheet or a newly attached
    /// shadow root: restyle everything.
    Everything,
}

/// Restyle statistics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RestyleStats {
    /// Elements that went through the cascade again.
    pub recomputed: usize,
    /// Elements whose earlier style was reused.
    pub reused: usize,
}

/// A document's styles, kept up to date incrementally.
#[derive(Clone, Debug)]
pub struct StyledDocument {
    styles: Vec<ElementStyle>,
    parents: Vec<Option<NodeId>>,
    index: HashMap<NodeId, usize>,
    map: InvalidationMap,
    dirty: Dirty,
    sizes: HashMap<NodeId, ContainerSize>,
}

impl StyledDocument {
    /// Style the whole document (as [`crate::style_tree`] from its root).
    pub fn new<'a>(
        document: &'a Document,
        cascade: &Cascade,
        parser: &CssParser,
        cx: &MatchContext<DomElement<'a>>,
        compute: &ComputeContext,
    ) -> Result<Self, DomError> {
        let walked = style_walk(
            document,
            document.root(),
            cascade,
            parser,
            cx,
            (compute, &HashMap::new()),
            None,
        )?;
        let mut styled = Self {
            styles: Vec::new(),
            parents: Vec::new(),
            index: HashMap::new(),
            map: InvalidationMap::new(cascade),
            dirty: Dirty::default(),
            sizes: HashMap::new(),
        };
        styled.store(walked.styles, walked.parents);
        Ok(styled)
    }

    fn store(&mut self, styles: Vec<ElementStyle>, parents: Vec<Option<NodeId>>) {
        self.index = styles
            .iter()
            .enumerate()
            .map(|(i, s)| (s.node, i))
            .collect();
        self.styles = styles;
        self.parents = parents;
    }

    /// Every element's style, in the order [`crate::style_tree`] returns them.
    pub fn styles(&self) -> &[ElementStyle] {
        &self.styles
    }

    /// The style of `element`, if it was styled.
    pub fn style(&self, element: NodeId) -> Option<&ElementStyle> {
        self.index.get(&element).map(|&i| &self.styles[i])
    }

    /// Whether any mutation since the last restyle needs work.
    pub fn needs_restyle(&self) -> bool {
        !self.dirty.is_clean()
    }

    /// Record that the cascade changed (sheets added or removed, or a
    /// different media environment with [`Cascade::set_environment`]
    /// reporting a change): everything is restyled, and the selector
    /// dependencies are rebuilt from `cascade`.
    pub fn cascade_changed(&mut self, cascade: &Cascade) {
        self.map = InvalidationMap::new(cascade);
        self.dirty.all = true;
    }

    /// Mark everything at and after `element` among its siblings, with
    /// subtrees.
    fn following_siblings(&mut self, document: &Document, element: NodeId) {
        let mut current = document.node(element).ok().and_then(|n| n.next_sibling());
        while let Some(id) = current {
            if matches!(
                document.node(id).map(|n| n.kind()),
                Ok(NodeKind::Element(_))
            ) {
                self.dirty.subtrees.insert(id);
            }
            current = document.node(id).ok().and_then(|n| n.next_sibling());
        }
    }

    fn attribute_changed(&mut self, document: &Document, element: NodeId, local: &str) {
        let local = local.to_ascii_lowercase();
        if local == "style" {
            self.dirty.elements.insert(element);
        }
        if self.map.states && STATE_ATTRIBUTES.contains(&local.as_str()) {
            self.dirty.all = true;
            return;
        }
        let Some(&outside_subject) = self.map.attributes.get(&local) else {
            return;
        };
        if self.map.has {
            self.dirty.all = true;
            return;
        }
        if outside_subject {
            self.dirty.subtrees.insert(element);
            if self.map.siblings {
                self.following_siblings(document, element);
            }
        } else {
            self.dirty.elements.insert(element);
        }
    }

    /// Record a mutation already made to `document`.
    pub fn note(&mut self, document: &Document, mutation: Mutation) {
        match mutation {
            Mutation::Everything => self.dirty.all = true,
            Mutation::Attribute { element, local } => {
                self.attribute_changed(document, element, &local);
            }
            Mutation::Children { parent } => {
                self.dirty.children = true;
                if self.map.has || self.map.states {
                    self.dirty.all = true;
                    return;
                }
                // Inserted and moved elements are found by the walk, since
                // their style parent changed or they have no style yet.
                if self.map.structural || self.map.siblings {
                    if let Ok(children) = document.children(parent) {
                        for child in children {
                            if matches!(
                                document.node(child).map(|n| n.kind()),
                                Ok(NodeKind::Element(_))
                            ) {
                                if self.map.siblings {
                                    self.dirty.subtrees.insert(child);
                                } else {
                                    self.dirty.elements.insert(child);
                                }
                            }
                        }
                    }
                    self.dirty.elements.insert(parent);
                }
            }
            Mutation::Text { node } => {
                if self.map.has || self.map.states {
                    self.dirty.all = true;
                    return;
                }
                if self.map.structural {
                    if let Some(parent) = document.node(node).ok().and_then(|n| n.parent()) {
                        self.dirty.elements.insert(parent);
                    }
                }
            }
        }
    }

    /// Recompute what the noted mutations may have changed. `cascade` must
    /// be the cascade of the last [`Self::new`] or [`Self::cascade_changed`].
    pub fn restyle<'a>(
        &mut self,
        document: &'a Document,
        cascade: &Cascade,
        parser: &CssParser,
        cx: &MatchContext<DomElement<'a>>,
        compute: &ComputeContext,
    ) -> Result<RestyleStats, DomError> {
        if self.dirty.is_clean() {
            return Ok(RestyleStats {
                recomputed: 0,
                reused: self.styles.len(),
            });
        }
        let previous: HashMap<NodeId, (ElementStyle, Option<NodeId>)> =
            std::mem::take(&mut self.styles)
                .into_iter()
                .zip(std::mem::take(&mut self.parents))
                .map(|(style, parent)| (style.node, (style, parent)))
                .collect();
        let dirty = std::mem::take(&mut self.dirty);
        let reuse = Reuse {
            previous,
            dirty: &dirty,
        };
        let walked = style_walk(
            document,
            document.root(),
            cascade,
            parser,
            cx,
            (compute, &self.sizes),
            Some(reuse),
        )?;
        let stats = RestyleStats {
            recomputed: walked.recomputed,
            reused: walked.styles.len() - walked.recomputed,
        };
        self.store(walked.styles, walked.parents);
        Ok(stats)
    }

    /// Whether these styles equal a fresh [`crate::style_tree`] walk (for tests and
    /// fuzzing).
    pub fn matches_full_restyle<'a>(
        &self,
        document: &'a Document,
        cascade: &Cascade,
        parser: &CssParser,
        cx: &MatchContext<DomElement<'a>>,
        compute: &ComputeContext,
    ) -> Result<bool, DomError> {
        let full = style_tree_with_sizes(
            document,
            document.root(),
            cascade,
            parser,
            cx,
            compute,
            &self.sizes,
        )?;
        Ok(full == self.styles)
    }

    /// Record `element`'s content-box size from layout (or forget it with
    /// `None`) for `@container` size features. A change restyles the
    /// element's subtree on the next [`Self::restyle`].
    pub fn set_container_size(&mut self, element: NodeId, size: Option<ContainerSize>) {
        let old = match size {
            Some(size) => self.sizes.insert(element, size),
            None => self.sizes.remove(&element),
        };
        if old != size {
            self.dirty.subtrees.insert(element);
        }
    }

    /// The container sizes recorded with [`Self::set_container_size`].
    pub fn container_sizes(&self) -> &HashMap<NodeId, ContainerSize> {
        &self.sizes
    }
}
