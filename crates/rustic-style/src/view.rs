//! The read-only element interface selector matching needs.

use rustic_css::{ElementState, PseudoElement};

/// One attribute of an element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttributeRef<'a> {
    /// Attribute namespace URL; `None` for the usual unnamespaced attribute.
    pub namespace: Option<&'a str>,
    /// Attribute local name.
    pub local: &'a str,
    /// Attribute value as UTF-16 code units.
    pub value: &'a [u16],
}

/// A child node of an element, as selector matching sees it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildRef<'a, E> {
    /// An element child.
    Element(E),
    /// A text (or CDATA section) child's data as UTF-16 code units.
    Text(&'a [u16]),
}

/// A copyable, read-only handle to an element in some tree.
///
/// Equality must mean "the same element", and hashing must agree with it
/// (the matcher memoizes nested selector results per element).
pub trait ElementView: Copy + Eq + std::hash::Hash {
    /// The parent, if it is an element.
    fn parent_element(&self) -> Option<Self>;

    /// The closest preceding sibling that is an element.
    fn previous_sibling_element(&self) -> Option<Self>;

    /// The closest following sibling that is an element.
    fn next_sibling_element(&self) -> Option<Self>;

    /// The shadow host, if the parent is a shadow root. Selectors from a
    /// shadow tree's stylesheets reach the host through it; ordinary
    /// matching never crosses a shadow boundary.
    fn parent_shadow_host(&self) -> Option<Self> {
        None
    }

    /// Whether the parent is a document (the `:root` test).
    fn is_root(&self) -> bool;

    /// The element's children that are elements or text, in tree order
    /// (comments and processing instructions are skipped). Template contents
    /// are not children.
    fn children(&self) -> impl Iterator<Item = ChildRef<'_, Self>>;

    /// The first child that is an element.
    fn first_child_element(&self) -> Option<Self> {
        self.children().find_map(|child| match child {
            ChildRef::Element(e) => Some(e),
            ChildRef::Text(_) => None,
        })
    }

    /// The element's local name.
    fn local_name(&self) -> &str;

    /// The element's namespace URL.
    fn namespace(&self) -> Option<&str>;

    /// The element's attributes, in any order.
    fn attributes(&self) -> impl Iterator<Item = AttributeRef<'_>>;

    /// Whether the element has no element children and no text children
    /// with data (the `:empty` test; comments and processing instructions
    /// are ignored).
    fn is_empty(&self) -> bool;

    /// Whether the element is in a user-interaction state. Without an
    /// interaction model, nothing is.
    fn in_state(&self, state: ElementState) -> bool {
        let _ = state;
        false
    }

    /// Whether the element's `pseudo` pseudo-element is in a user-action
    /// state (`::before:hover`). Without an interaction model, none is.
    fn pseudo_element_in_state(&self, pseudo: PseudoElement, state: ElementState) -> bool {
        let _ = (pseudo, state);
        false
    }
}
