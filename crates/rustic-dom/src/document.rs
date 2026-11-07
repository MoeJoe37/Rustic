use rustic_base::{DocumentId, NodeId};

use crate::storage::Arena;
use crate::{
    Attribute, DocumentType, DomError, DomString, Element, Node, NodeKind, QualifiedName, Walk,
};

/// Per-document limits, covering connected and detached nodes alike.
///
/// String accounting includes UTF-8 name bytes and UTF-16 text/attribute/doctype
/// bytes. It excludes allocator overhead/capacity and caller-owned input buffers.
#[derive(Clone, Copy, Debug)]
pub struct DocumentLimits {
    /// Maximum live nodes, including the protected document root.
    pub max_nodes: usize,
    /// Maximum parent-child edges from any tree root to a descendant.
    pub max_depth: usize,
    /// Maximum retained name/string payload bytes.
    pub max_data_bytes: usize,
}

impl Default for DocumentLimits {
    fn default() -> Self {
        Self {
            max_nodes: 100_000,
            max_depth: 256,
            max_data_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Sole owner of an arena and its tree links; it is deliberately not cloneable.
///
/// Checked moves operate within one document. Cross-document adoption, fragments,
/// mutation observers, bindings and garbage collection are not implemented yet.
#[derive(Debug)]
pub struct Document {
    arena: Arena,
    root: NodeId,
    limits: DocumentLimits,
}

impl Document {
    /// Create a document and charge its root against the node limit.
    pub fn new(limits: DocumentLimits) -> Result<Self, DomError> {
        let mut arena = Arena::new(limits.max_nodes, limits.max_data_bytes)?;
        let root = arena.insert(NodeKind::Document)?;
        Ok(Self {
            arena,
            root,
            limits,
        })
    }

    /// Process-local identity, also embedded in all node handles.
    pub fn id(&self) -> DocumentId {
        self.arena.id()
    }
    /// The document's permanent root.
    pub fn root(&self) -> NodeId {
        self.root
    }
    /// Current live node count, including root and detached nodes.
    pub fn node_count(&self) -> usize {
        self.arena.node_count()
    }
    /// Currently accounted string/name payload bytes.
    pub fn data_bytes(&self) -> usize {
        self.arena.data_bytes()
    }
    /// Immutable access after validating document, slot, generation and occupancy.
    pub fn node(&self, id: NodeId) -> Result<&Node, DomError> {
        self.arena.get(id)
    }

    /// Create an unattached element without normalizing the supplied parsed name.
    pub fn create_element(&mut self, name: QualifiedName) -> Result<NodeId, DomError> {
        self.arena.insert(NodeKind::Element(Element {
            name,
            attributes: Vec::new(),
        }))
    }

    /// Create unattached text, preserving UTF-16 code units.
    pub fn create_text(&mut self, value: impl Into<DomString>) -> Result<NodeId, DomError> {
        self.arena.insert(NodeKind::Text(value.into()))
    }

    /// Create an unattached comment.
    pub fn create_comment(&mut self, value: impl Into<DomString>) -> Result<NodeId, DomError> {
        self.arena.insert(NodeKind::Comment(value.into()))
    }

    /// Create an unattached document type declaration.
    pub fn create_doctype(&mut self, value: DocumentType) -> Result<NodeId, DomError> {
        self.arena.insert(NodeKind::DocumentType(value))
    }

    /// Visit direct children in order, without copying IDs or allocating a stack.
    pub fn children(&self, parent: NodeId) -> Result<impl Iterator<Item = NodeId> + '_, DomError> {
        Ok(std::iter::successors(
            self.node(parent)?.first_child(),
            move |id| self.node(*id).expect("validated child link").next_sibling(),
        ))
    }

    /// Walk an attached or detached subtree in preorder, including its root.
    pub fn walk(&self, root: NodeId) -> Result<Walk<'_>, DomError> {
        self.node(root)?;
        Ok(Walk::new(self, root))
    }

    /// Move a node to the end of a parent's children after full validation.
    pub fn append_child(&mut self, parent: NodeId, child: NodeId) -> Result<(), DomError> {
        self.insert_before(parent, child, None)
    }

    /// Move a node before a direct child, or append when `before` is absent.
    ///
    /// All hierarchy, cycle, reference, and depth checks happen before unlinking.
    /// This low-level operation has no browser callbacks or mutation observers.
    pub fn insert_before(
        &mut self,
        parent: NodeId,
        child: NodeId,
        before: Option<NodeId>,
    ) -> Result<(), DomError> {
        let parent_node = self.node(parent)?;
        let child_node = self.node(child)?;
        if child == self.root {
            return Err(DomError::ProtectedRoot);
        }
        if !matches!(
            parent_node.kind(),
            NodeKind::Document | NodeKind::Element(_)
        ) {
            return Err(DomError::InvalidHierarchy);
        }
        if let Some(reference) = before {
            if self.node(reference)?.parent() != Some(parent) {
                return Err(DomError::InvalidReference);
            }
        }
        if matches!(child_node.kind(), NodeKind::DocumentType(_)) && parent != self.root {
            return Err(DomError::InvalidHierarchy);
        }
        let mut depth = 0usize;
        let mut ancestor = Some(parent);
        while let Some(id) = ancestor {
            if id == child {
                return Err(DomError::Cycle);
            }
            ancestor = self.node(id)?.parent();
            if ancestor.is_some() {
                depth += 1;
            }
        }
        let height = self.walk(child)?.map(|(_, depth)| depth).max().unwrap_or(0);
        depth
            .checked_add(1)
            .and_then(|depth| depth.checked_add(height))
            .filter(|depth| *depth <= self.limits.max_depth)
            .ok_or(DomError::DepthLimit)?;
        let before = if before == Some(child) {
            child_node.next_sibling()
        } else {
            before
        };
        if parent == self.root {
            self.validate_document_order(child, before)?;
        }
        if child_node.parent() == Some(parent) && child_node.next_sibling() == before {
            return Ok(());
        }

        self.unlink(child);
        let previous = before
            .map(|id| self.live(id).previous_sibling())
            .unwrap_or_else(|| self.live(parent).last_child());
        if let Some(id) = previous {
            self.live_mut(id).next_sibling = Some(child);
        } else {
            self.live_mut(parent).first_child = Some(child);
        }
        if let Some(id) = before {
            self.live_mut(id).previous_sibling = Some(child);
        } else {
            self.live_mut(parent).last_child = Some(child);
        }
        let node = self.live_mut(child);
        node.parent = Some(parent);
        node.previous_sibling = previous;
        node.next_sibling = before;
        Ok(())
    }

    /// Disconnect a subtree while preserving all node IDs and accounted payload.
    pub fn detach(&mut self, node: NodeId) -> Result<(), DomError> {
        self.node(node)?;
        if node == self.root {
            return Err(DomError::ProtectedRoot);
        }
        self.unlink(node);
        Ok(())
    }

    /// Permanently destroy a subtree and invalidate every ID within it.
    ///
    /// This explicit engine operation must not be used as future JS-visible DOM
    /// removal: detached nodes can remain reachable from script. No recursion or
    /// depth-sized temporary stack is needed to destroy a deeply nested tree.
    pub fn destroy_subtree(&mut self, root: NodeId) -> Result<(), DomError> {
        self.node(root)?;
        if root == self.root {
            return Err(DomError::ProtectedRoot);
        }
        self.unlink(root);
        let mut current = root;
        loop {
            if let Some(child) = self.live(current).first_child() {
                current = child;
                continue;
            }
            let parent = self.live(current).parent();
            self.unlink(current);
            self.arena.remove_detached_leaf(current);
            if current == root {
                break;
            }
            current = parent.expect("non-root destruction node has parent");
        }
        Ok(())
    }

    /// Replace text/comment data atomically with respect to the payload budget.
    pub fn set_character_data(
        &mut self,
        id: NodeId,
        value: impl Into<DomString>,
    ) -> Result<DomString, DomError> {
        if !matches!(
            self.node(id)?.kind(),
            NodeKind::Text(_) | NodeKind::Comment(_)
        ) {
            return Err(DomError::WrongNodeKind);
        }
        let value = value.into();
        self.arena.resize_data(id, value.data_bytes())?;
        match &mut self.live_mut(id).kind {
            NodeKind::Text(old) | NodeKind::Comment(old) => Ok(std::mem::replace(old, value)),
            _ => unreachable!("kind validated before accounting"),
        }
    }

    /// Set an attribute by expanded name; replacement keeps its insertion order.
    pub fn set_attribute(
        &mut self,
        id: NodeId,
        name: QualifiedName,
        value: impl Into<DomString>,
    ) -> Result<Option<DomString>, DomError> {
        let element = self.element(id)?;
        let position = element
            .attributes
            .iter()
            .position(|attribute| attribute.name.same_expanded_name(&name));
        let old_bytes = position
            .map(|index| element.attributes[index].data_bytes())
            .transpose()?
            .unwrap_or(0);
        let attribute = Attribute {
            name,
            value: value.into(),
        };
        let attribute_bytes = attribute.data_bytes()?;
        let bytes = self
            .node(id)?
            .data_bytes
            .checked_sub(old_bytes)
            .and_then(|bytes| bytes.checked_add(attribute_bytes))
            .ok_or(DomError::DataLimit)?;
        if position.is_none() {
            self.element_mut(id)
                .attributes
                .try_reserve(1)
                .map_err(|_| DomError::AllocationFailed)?;
        }
        self.arena.resize_data(id, bytes)?;
        let attributes = &mut self.element_mut(id).attributes;
        Ok(if let Some(index) = position {
            Some(std::mem::replace(&mut attributes[index], attribute).value)
        } else {
            attributes.push(attribute);
            None
        })
    }

    /// Remove an attribute by namespace/local name, returning its previous value.
    pub fn remove_attribute(
        &mut self,
        id: NodeId,
        namespace: Option<&str>,
        local: &str,
    ) -> Result<Option<DomString>, DomError> {
        let namespace = namespace.filter(|value| !value.is_empty());
        let element = self.element(id)?;
        let Some(index) = element.attributes.iter().position(|attribute| {
            attribute.name.namespace() == namespace && attribute.name.local() == local
        }) else {
            return Ok(None);
        };
        let bytes = self.node(id)?.data_bytes - element.attributes[index].data_bytes()?;
        self.arena.resize_data(id, bytes)?;
        Ok(Some(self.element_mut(id).attributes.remove(index).value))
    }

    fn element(&self, id: NodeId) -> Result<&Element, DomError> {
        match self.node(id)?.kind() {
            NodeKind::Element(element) => Ok(element),
            _ => Err(DomError::WrongNodeKind),
        }
    }

    fn element_mut(&mut self, id: NodeId) -> &mut Element {
        match &mut self.live_mut(id).kind {
            NodeKind::Element(element) => element,
            _ => unreachable!("validated element"),
        }
    }

    fn live(&self, id: NodeId) -> &Node {
        self.node(id).expect("validated internal node link")
    }
    fn live_mut(&mut self, id: NodeId) -> &mut Node {
        self.arena
            .get_mut(id)
            .expect("validated internal node link")
    }

    fn unlink(&mut self, id: NodeId) {
        let node = self.live(id);
        let (Some(parent), previous, next) =
            (node.parent(), node.previous_sibling(), node.next_sibling())
        else {
            return;
        };
        if let Some(previous) = previous {
            self.live_mut(previous).next_sibling = next;
        } else {
            self.live_mut(parent).first_child = next;
        }
        if let Some(next) = next {
            self.live_mut(next).previous_sibling = previous;
        } else {
            self.live_mut(parent).last_child = previous;
        }
        let node = self.live_mut(id);
        node.parent = None;
        node.previous_sibling = None;
        node.next_sibling = None;
    }

    fn validate_document_order(
        &self,
        child: NodeId,
        before: Option<NodeId>,
    ) -> Result<(), DomError> {
        let mut element_seen = false;
        let mut doctype_seen = false;
        let mut check = |id| {
            match self.live(id).kind() {
                NodeKind::Comment(_) => {}
                NodeKind::Element(_) if !element_seen => {
                    element_seen = true;
                }
                NodeKind::DocumentType(_) if !doctype_seen && !element_seen => {
                    doctype_seen = true;
                }
                _ => return Err(DomError::InvalidHierarchy),
            }
            Ok(())
        };
        for existing in self.children(self.root)? {
            if existing == child {
                continue;
            }
            if Some(existing) == before {
                check(child)?;
            }
            check(existing)?;
        }
        if before.is_none() {
            check(child)?;
        }
        Ok(())
    }
}
