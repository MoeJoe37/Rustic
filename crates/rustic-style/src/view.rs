//! The read-only element interface selector matching needs.

use rustic_css::ElementState;

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

/// A copyable, read-only handle to an element in some tree.
///
/// Equality must mean "the same element".
pub trait ElementView: Copy + PartialEq {
    /// The parent, if it is an element.
    fn parent_element(&self) -> Option<Self>;

    /// The closest preceding sibling that is an element.
    fn previous_sibling_element(&self) -> Option<Self>;

    /// The closest following sibling that is an element.
    fn next_sibling_element(&self) -> Option<Self>;

    /// Whether the parent is a document (the `:root` test).
    fn is_root(&self) -> bool;

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
}
