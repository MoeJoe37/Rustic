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
    pub(crate) template_contents: Option<NodeId>,
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

    /// Separate template contents fragment, which is never one of the children.
    pub fn template_contents(&self) -> Option<NodeId> {
        self.template_contents
    }
}

/// A document type's parsed strings, without a decision about HTML quirks mode.
#[derive(Clone, Debug)]
pub struct DocumentType {
    /// Parsed name; malformed input may produce an empty name.
    pub name: DomString,
    /// Parsed public identifier.
    pub public_id: DomString,
    /// Parsed system identifier.
    pub system_id: DomString,
}

/// A processing instruction's target name and character data.
///
/// The target is validated as an XML `Name` at creation and never changes. The
/// data is character data: it can be edited like text, and the `?>` restriction
/// applies only to creation, as in the DOM Standard.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessingInstruction {
    pub(crate) target: DomString,
    pub(crate) data: DomString,
}

impl ProcessingInstruction {
    /// Target name, validated as an XML `Name` when created.
    pub fn target(&self) -> &DomString {
        &self.target
    }

    /// Character data as UTF-16 code units.
    pub fn data(&self) -> &DomString {
        &self.data
    }
}

/// Every DOM tree node kind. Attributes are element records, not tree nodes.
///
/// Shadow roots are not implemented; they arrive with shadow DOM (M5).
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
    /// A fragment tree root. Inserting a fragment moves its children instead.
    ///
    /// A fragment created by [`crate::Document::create_template_contents`] is
    /// owned by its template (see [`Node::template_host`]); any other fragment
    /// is free-standing and can be destroyed or adopted on its own.
    DocumentFragment,
    /// A processing instruction, such as one produced by a future XML parser.
    ProcessingInstruction(ProcessingInstruction),
    /// An XML CDATA section: text data that serializers keep in a CDATA block.
    CdataSection(DomString),
}

impl NodeKind {
    pub(crate) fn data_bytes(&self) -> Result<usize, DomError> {
        match self {
            Self::Document | Self::DocumentFragment => Ok(0),
            Self::Element(element) => element.attributes.iter().try_fold(
                element.name.data_bytes()?,
                |total, attribute| {
                    total
                        .checked_add(attribute.data_bytes()?)
                        .ok_or(DomError::DataLimit)
                },
            ),
            Self::Text(value) | Self::Comment(value) | Self::CdataSection(value) => {
                Ok(value.data_bytes())
            }
            Self::ProcessingInstruction(instruction) => instruction
                .target
                .data_bytes()
                .checked_add(instruction.data.data_bytes())
                .ok_or(DomError::DataLimit),
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
    pub(crate) template_host: Option<NodeId>,
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
            template_host: None,
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
    /// The owning `template` element when this node is its contents fragment.
    pub fn template_host(&self) -> Option<NodeId> {
        self.template_host
    }
    /// Next sibling, if attached and not last.
    pub fn next_sibling(&self) -> Option<NodeId> {
        self.next_sibling
    }
}
