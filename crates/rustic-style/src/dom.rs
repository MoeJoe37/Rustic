//! [`ElementView`] for `rustic-dom` documents.

use rustic_css::SelectorList;
use rustic_dom::{Document, DomError, Element, Node, NodeId, NodeKind};

use crate::{AttributeRef, ElementView, MatchContext, matches};

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
/// Cost is proportional to elements × selector work; `:nth-*` pseudo-classes
/// count siblings on each test (no caching yet).
pub fn query_selector_all(
    document: &Document,
    root: NodeId,
    list: &SelectorList,
    cx: &MatchContext<DomElement<'_>>,
) -> Result<Vec<NodeId>, DomError> {
    let mut found = Vec::new();
    for (id, _) in document.walk(root)? {
        if let Some(element) = DomElement::new(document, id) {
            if matches(list, element, cx) {
                found.push(id);
            }
        }
    }
    Ok(found)
}
