use rustic_base::NodeId;

use crate::{DomError, DomString, QualifiedName};

/// An element attribute in insertion order, keyed by namespace and local name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attribute {
    pub(crate) name: QualifiedName,
    pub(crate) value: DomString,
}

impl Attribute {
    /// Attribute namespace, prefix, and local name.
    pub fn name(&self) -> &QualifiedName {
        &self.name
    }

    /// Attribute value as UTF-16, without implicit normalization.
    pub fn value(&self) -> &DomString {
        &self.value
    }

    pub(crate) fn data_bytes(&self) -> Result<usize, DomError> {
        self.name
            .data_bytes()?
            .checked_add(self.value.data_bytes())
            .ok_or(DomError::DataLimit)
    }
}

/// Immutable element data; mutation goes through the owning document.
#[derive(Debug)]
pub struct Element {
    pub(crate) name: QualifiedName,
    pub(crate) attributes: Vec<Attribute>,
}

impl Element {
    /// Element name supplied by the parser.
    pub fn name(&self) -> &QualifiedName {
        &self.name
    }

    /// Stable insertion order; replacing an attribute preserves its position.
    pub fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }
}

/// A document type's parsed strings, without a decision about HTML quirks mode.
#[derive(Debug)]
pub struct DocumentType {
    /// Parsed name; malformed input may produce an empty name.
    pub name: DomString,
    /// Parsed public identifier.
    pub public_id: DomString,
    /// Parsed system identifier.
    pub system_id: DomString,
}

/// Node kinds implemented by the initial storage layer.
#[derive(Debug)]
pub enum NodeKind {
    /// The document's protected root.
    Document,
    /// An element with ordered attributes.
    Element(Element),
    /// Text preserving UTF-16 code units.
    Text(DomString),
    /// Comment data preserving UTF-16 code units.
    Comment(DomString),
    /// Document type declaration.
    DocumentType(DocumentType),
}

impl NodeKind {
    pub(crate) fn data_bytes(&self) -> Result<usize, DomError> {
        match self {
            Self::Document => Ok(0),
            Self::Element(element) => element.name.data_bytes(), // Created without attributes.
            Self::Text(value) | Self::Comment(value) => Ok(value.data_bytes()),
            Self::DocumentType(doctype) => doctype
                .name
                .data_bytes()
                .checked_add(doctype.public_id.data_bytes())
                .and_then(|bytes| bytes.checked_add(doctype.system_id.data_bytes()))
                .ok_or(DomError::DataLimit),
        }
    }
}

/// Read-only node data and links. Only [`crate::Document`] can change the tree.
#[derive(Debug)]
pub struct Node {
    pub(crate) kind: NodeKind,
    pub(crate) parent: Option<NodeId>,
    pub(crate) first_child: Option<NodeId>,
    pub(crate) last_child: Option<NodeId>,
    pub(crate) previous_sibling: Option<NodeId>,
    pub(crate) next_sibling: Option<NodeId>,
    pub(crate) data_bytes: usize,
}

impl Node {
    pub(crate) fn new(kind: NodeKind, data_bytes: usize) -> Self {
        Self {
            kind,
            parent: None,
            first_child: None,
            last_child: None,
            previous_sibling: None,
            next_sibling: None,
            data_bytes,
        }
    }

    /// Node kind and immutable payload.
    pub fn kind(&self) -> &NodeKind {
        &self.kind
    }
    /// Parent, or `None` for the document root and detached nodes.
    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }
    /// First child, if any.
    pub fn first_child(&self) -> Option<NodeId> {
        self.first_child
    }
    /// Last child, if any.
    pub fn last_child(&self) -> Option<NodeId> {
        self.last_child
    }
    /// Previous sibling, if attached and not first.
    pub fn previous_sibling(&self) -> Option<NodeId> {
        self.previous_sibling
    }
    /// Next sibling, if attached and not last.
    pub fn next_sibling(&self) -> Option<NodeId> {
        self.next_sibling
    }
}
