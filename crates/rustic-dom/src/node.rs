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
    pub(crate) custom_is: Option<DomString>,
    pub(crate) keep_registry_null: bool,
    pub(crate) template_contents: Option<NodeId>,
    pub(crate) shadow_root: Option<(NodeId, ShadowRootOptions)>,
}

/// Whether script outside a shadow tree may reach it (`attachShadow`'s mode).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ShadowRootMode {
    /// Reachable through the host's `shadowRoot`.
    Open,
    /// Hidden from the host's `shadowRoot`.
    Closed,
}

/// Distribution mode of a shadow root.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SlotAssignment {
    /// Assign light children to the first slot with their name.
    #[default]
    Named,
    /// Assign light children explicitly.
    Manual,
}

/// Options stored on a shadow host with its root identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShadowRootOptions {
    /// Visibility of the root from the host.
    pub mode: ShadowRootMode,
    /// Whether host focus delegates into the shadow tree.
    pub delegates_focus: bool,
    /// Whether cloning the host also clones the shadow tree.
    pub clonable: bool,
    /// Whether HTML serialization may include this shadow tree.
    pub serializable: bool,
    /// How light children are distributed into slots.
    pub slot_assignment: SlotAssignment,
    /// Whether the parser attached the root declaratively.
    pub declarative: bool,
    /// Whether the root starts with a null custom element registry.
    pub keep_registry_null: bool,
}

impl ShadowRootOptions {
    /// The defaults of a shadow-root initialization dictionary.
    pub const fn new(mode: ShadowRootMode) -> Self {
        Self {
            mode,
            delegates_focus: false,
            clonable: false,
            serializable: false,
            slot_assignment: SlotAssignment::Named,
            declarative: false,
            keep_registry_null: false,
        }
    }
}

impl Element {
    /// Element name supplied by the parser.
    pub fn name(&self) -> &QualifiedName {
        &self.name
    }

    /// The customized built-in name captured at element creation.
    pub fn custom_is(&self) -> Option<&DomString> {
        self.custom_is.as_ref()
    }

    /// Whether the parser or creation options explicitly require a null registry.
    pub fn keeps_registry_null(&self) -> bool {
        self.keep_registry_null
    }

    /// Stable insertion order; replacing an attribute preserves its position.
    pub fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }

    /// Separate template contents fragment, which is never one of the children.
    pub fn template_contents(&self) -> Option<NodeId> {
        self.template_contents
    }

    /// The element's shadow root, which is never one of the children.
    pub fn shadow_root(&self) -> Option<NodeId> {
        self.shadow_root.map(|(id, _)| id)
    }

    /// The mode the shadow root was attached with.
    pub fn shadow_root_mode(&self) -> Option<ShadowRootMode> {
        self.shadow_root_options().map(|options| options.mode)
    }

    /// Options associated with the attached shadow root.
    pub fn shadow_root_options(&self) -> Option<ShadowRootOptions> {
        self.shadow_root.map(|(_, options)| options)
    }

    /// The separate trees this element owns: template contents and its
    /// shadow root.
    pub(crate) fn owned_roots(&self) -> impl Iterator<Item = NodeId> + use<> {
        self.template_contents.into_iter().chain(self.shadow_root())
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
/// A shadow root is a [`NodeKind::DocumentFragment`] with a
/// [`Node::shadow_host`].
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
                element
                    .name
                    .data_bytes()?
                    .checked_add(element.custom_is.as_ref().map_or(0, DomString::data_bytes))
                    .ok_or(DomError::DataLimit)?,
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
    pub(crate) shadow_host: Option<NodeId>,
    /// An upper bound on the height of this node's subtree (0 for a leaf),
    /// raised along the ancestors on every link and left high on removal;
    /// an insertion recomputes it exactly only when the bound would fail the
    /// depth limit (TD-03).
    pub(crate) height_bound: u32,
}

impl Node {
    /// Bytes of retained names and character data charged to the document.
    pub fn retained_data_bytes(&self) -> usize {
        self.data_bytes
    }

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
            shadow_host: None,
            height_bound: 0,
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
    /// The host element when this node is its shadow root.
    pub fn shadow_host(&self) -> Option<NodeId> {
        self.shadow_host
    }
    /// The element owning this separate tree root: a template for its
    /// contents, or a host for its shadow root.
    pub(crate) fn owner_host(&self) -> Option<NodeId> {
        self.template_host.or(self.shadow_host)
    }
    /// Next sibling, if attached and not last.
    pub fn next_sibling(&self) -> Option<NodeId> {
        self.next_sibling
    }
}
