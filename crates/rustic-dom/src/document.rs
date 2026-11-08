use rustic_base::{DocumentId, NodeId};

use crate::storage::Arena;
use crate::{
    Attribute, DocumentType, DomError, DomString, Element, Node, NodeKind, ProcessingInstruction,
    QualifiedName, Walk,
};

mod adoption;
mod character_data;
mod invariants;
pub(crate) mod tracing;

pub use invariants::InvariantViolation;

pub use adoption::Adopted;

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

/// Document readiness from the HTML Standard's document lifecycle.
///
/// Readiness only moves forward: `Loading` → `Interactive` → `Complete`.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ReadyState {
    /// A parser is still producing the document.
    Loading,
    /// Parsing finished; deferred work has not yet completed.
    Interactive,
    /// The document has fully loaded.
    Complete,
}

/// Sole owner of an arena and its tree links; it is deliberately not `Clone`.
///
/// Checked moves operate within one document; [`Self::clone_node`] copies within
/// a document and [`Self::adopt_node`] moves a subtree between documents. Mutation
/// observers, bindings and garbage collection are not implemented yet.
#[derive(Debug)]
pub struct Document {
    arena: Arena,
    root: NodeId,
    limits: DocumentLimits,
    ready_state: ReadyState,
}

impl Document {
    /// Create a script-style document, which is `Complete` from the start.
    ///
    /// The root is charged against the node limit.
    pub fn new(limits: DocumentLimits) -> Result<Self, DomError> {
        Self::with_ready_state(limits, ReadyState::Complete)
    }

    /// Create a document for a parser to fill, starting in `Loading`.
    pub fn new_for_parsing(limits: DocumentLimits) -> Result<Self, DomError> {
        Self::with_ready_state(limits, ReadyState::Loading)
    }

    fn with_ready_state(limits: DocumentLimits, ready_state: ReadyState) -> Result<Self, DomError> {
        let mut arena = Arena::new(limits.max_nodes, limits.max_data_bytes)?;
        let root = arena.insert(NodeKind::Document)?;
        Ok(Self {
            arena,
            root,
            limits,
            ready_state,
        })
    }

    /// Current readiness.
    pub fn ready_state(&self) -> ReadyState {
        self.ready_state
    }

    /// Move readiness forward. Staying put or going back fails with `InvalidState`.
    ///
    /// Skipping `Interactive` is allowed, as when a parser is aborted.
    pub fn advance_ready_state(&mut self, next: ReadyState) -> Result<(), DomError> {
        if next <= self.ready_state {
            return Err(DomError::InvalidState);
        }
        self.ready_state = next;
        Ok(())
    }

    /// The limits this document was created with.
    pub fn limits(&self) -> DocumentLimits {
        self.limits
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
            template_contents: None,
            shadow_root: None,
        }))
    }

    /// Attach an empty shadow root to `host`, as `attachShadow` does.
    ///
    /// The host must be an HTML element that may host a shadow root: a valid
    /// custom element name or one of `article`, `aside`, `blockquote`,
    /// `body`, `div`, `footer`, `h1`–`h6`, `header`, `main`, `nav`, `p`,
    /// `section` and `span` (`WrongNodeKind` otherwise). An element has at
    /// most one shadow root (`InvalidHierarchy`). Like template contents, the
    /// shadow root is a separate tree root: not a child, skipped by
    /// [`Self::walk`] of the host, never inserted, detached or destroyed on
    /// its own, and destroyed with its host. Declarative shadow roots and slot
    /// assignment are not implemented.
    pub fn attach_shadow(
        &mut self,
        host: NodeId,
        mode: crate::ShadowRootMode,
    ) -> Result<NodeId, DomError> {
        let element = self.element(host)?;
        if element.name.namespace() != Some(crate::HTML_NAMESPACE)
            || !can_host_shadow(element.name.local())
        {
            return Err(DomError::WrongNodeKind);
        }
        if element.shadow_root.is_some() {
            return Err(DomError::InvalidHierarchy);
        }
        let root = self.arena.insert(NodeKind::DocumentFragment)?;
        self.live_mut(root).shadow_host = Some(host);
        self.element_mut(host).shadow_root = Some((root, mode));
        Ok(root)
    }

    /// Create and associate the empty contents fragment of an HTML `template`.
    ///
    /// The fragment is a separate tree root: it is not a child, is skipped by
    /// [`Self::walk`] of the template, cannot be inserted, detached or destroyed
    /// on its own, and is destroyed together with its template element.
    pub fn create_template_contents(&mut self, template: NodeId) -> Result<NodeId, DomError> {
        let element = self.element(template)?;
        if element.name.namespace() != Some(crate::HTML_NAMESPACE)
            || element.name.local() != "template"
        {
            return Err(DomError::WrongNodeKind);
        }
        if element.template_contents.is_some() {
            return Err(DomError::InvalidHierarchy);
        }
        let fragment = self.arena.insert(NodeKind::DocumentFragment)?;
        self.live_mut(fragment).template_host = Some(template);
        self.element_mut(template).template_contents = Some(fragment);
        Ok(fragment)
    }

    /// Create an empty, free-standing document fragment.
    ///
    /// Unlike template contents it has no host: it can be destroyed or adopted
    /// on its own. Inserting it moves its children and leaves it empty.
    pub fn create_document_fragment(&mut self) -> Result<NodeId, DomError> {
        self.arena.insert(NodeKind::DocumentFragment)
    }

    /// Create an unattached processing instruction.
    ///
    /// The target must be an XML `Name` (`InvalidName`) and the data must not
    /// contain `?>` (`InvalidCharacterData`), as for `createProcessingInstruction`.
    pub fn create_processing_instruction(
        &mut self,
        target: &str,
        data: impl Into<DomString>,
    ) -> Result<NodeId, DomError> {
        if !crate::name::is_xml_name(target) {
            return Err(DomError::InvalidName);
        }
        let data = data.into();
        if contains_units(data.as_units(), "?>") {
            return Err(DomError::InvalidCharacterData);
        }
        self.arena
            .insert(NodeKind::ProcessingInstruction(ProcessingInstruction {
                target: target.into(),
                data,
            }))
    }

    /// Create an unattached CDATA section; `]]>` fails with `InvalidCharacterData`.
    pub fn create_cdata_section(
        &mut self,
        value: impl Into<DomString>,
    ) -> Result<NodeId, DomError> {
        let value = value.into();
        if contains_units(value.as_units(), "]]>") {
            return Err(DomError::InvalidCharacterData);
        }
        self.arena.insert(NodeKind::CdataSection(value))
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
    ///
    /// Template contents are separate roots and are not visited; use
    /// [`crate::Element::template_contents`] to walk them explicitly.
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
    /// When `child` is a document fragment (including template contents), its
    /// children move instead, in order, and the fragment stays behind empty.
    /// All hierarchy, cycle, reference, and depth checks happen before unlinking,
    /// so a rejected fragment insertion moves none of its children. This
    /// low-level operation has no browser callbacks or mutation observers.
    pub fn insert_before(
        &mut self,
        parent: NodeId,
        child: NodeId,
        before: Option<NodeId>,
    ) -> Result<(), DomError> {
        let parent_node = self.node(parent)?;
        let child_node = self.node(child)?;
        if !matches!(
            parent_node.kind(),
            NodeKind::Document | NodeKind::Element(_) | NodeKind::DocumentFragment
        ) {
            return Err(DomError::InvalidHierarchy);
        }
        // DOM "ensure pre-insert validity" order: host-including ancestors
        // (hopping from template contents to their template) before the reference.
        // A child with no children and no template contents can only be its own
        // ancestor, so hopping is skipped; this keeps the parser's insertion of a
        // fresh template into nested contents linear instead of quadratic.
        if child_node.shadow_host().is_some() {
            return Err(DomError::InvalidHierarchy);
        }
        let child_is_leaf = child_node.first_child().is_none()
            && !matches!(child_node.kind(), NodeKind::Element(element)
                if element.owned_roots().next().is_some());
        let mut depth = 0usize;
        let mut in_parent_tree = true;
        let mut ancestor = Some(parent);
        while let Some(id) = ancestor {
            if id == child {
                return Err(DomError::Cycle);
            }
            let node = self.node(id)?;
            ancestor = match node.parent() {
                Some(next) => {
                    depth += usize::from(in_parent_tree);
                    Some(next)
                }
                None if child_is_leaf => None,
                None => {
                    in_parent_tree = false;
                    node.owner_host()
                }
            };
        }
        if let Some(reference) = before {
            if self.node(reference)?.parent() != Some(parent) {
                return Err(DomError::InvalidReference);
            }
        }
        // Step 4: a document node is never insertable. A connected parent
        // already failed the ancestor check above with `Cycle`.
        if child == self.root {
            return Err(DomError::ProtectedRoot);
        }
        let fragment = matches!(child_node.kind(), NodeKind::DocumentFragment);
        let mut moved = Vec::new();
        if fragment {
            let count = self.children(child)?.count();
            moved
                .try_reserve_exact(count)
                .map_err(|_| DomError::AllocationFailed)?;
            moved.extend(self.children(child)?);
        } else {
            moved
                .try_reserve_exact(1)
                .map_err(|_| DomError::AllocationFailed)?;
            moved.push(child);
        }
        for &id in &moved {
            if matches!(self.live(id).kind(), NodeKind::DocumentType(_)) && parent != self.root {
                return Err(DomError::InvalidHierarchy);
            }
        }
        // A fragment's children land one level shallower than in the fragment.
        let height = self.walk(child)?.map(|(_, depth)| depth).max().unwrap_or(0);
        let height = if fragment {
            height.saturating_sub(1)
        } else {
            height
        };
        if !moved.is_empty() {
            depth
                .checked_add(1)
                .and_then(|depth| depth.checked_add(height))
                .filter(|depth| *depth <= self.limits.max_depth)
                .ok_or(DomError::DepthLimit)?;
        }
        let before = if before == Some(child) {
            child_node.next_sibling()
        } else {
            before
        };
        if parent == self.root {
            self.validate_document_order(&moved, before)?;
        }
        if !fragment && child_node.parent() == Some(parent) && child_node.next_sibling() == before {
            return Ok(());
        }

        for id in moved {
            self.unlink(id);
            self.link_before(parent, id, before);
        }
        Ok(())
    }

    /// Unchecked link of a detached node before `before` (or last).
    fn link_before(&mut self, parent: NodeId, child: NodeId, before: Option<NodeId>) {
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
    }

    /// Disconnect a subtree while preserving all node IDs and accounted payload.
    pub fn detach(&mut self, node: NodeId) -> Result<(), DomError> {
        if node == self.root {
            return Err(DomError::ProtectedRoot);
        }
        if self.node(node)?.owner_host().is_some() {
            return Err(DomError::InvalidHierarchy);
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
        if root == self.root {
            return Err(DomError::ProtectedRoot);
        }
        if self.node(root)?.owner_host().is_some() {
            return Err(DomError::InvalidHierarchy);
        }
        self.destroy_unchecked(root);
        Ok(())
    }

    /// Destroy any non-root subtree, including template contents, iteratively.
    fn destroy_unchecked(&mut self, root: NodeId) {
        self.unlink(root);
        let mut current = root;
        loop {
            // Splice template contents and shadow roots in as final children
            // so the same allocation-free walk destroys arbitrarily nested
            // owned trees.
            let owned = match &mut self.live_mut(current).kind {
                NodeKind::Element(element) => [
                    element.template_contents.take(),
                    element.shadow_root.take().map(|(id, _)| id),
                ],
                _ => [None, None],
            };
            for fragment in owned.into_iter().flatten() {
                self.link_last_child(current, fragment);
            }
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

    /// Unchecked append of a detached node, used only during destruction.
    fn link_last_child(&mut self, parent: NodeId, child: NodeId) {
        let previous = self.live(parent).last_child();
        if let Some(id) = previous {
            self.live_mut(id).next_sibling = Some(child);
        } else {
            self.live_mut(parent).first_child = Some(child);
        }
        self.live_mut(parent).last_child = Some(child);
        let node = self.live_mut(child);
        node.parent = Some(parent);
        node.previous_sibling = previous;
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

    /// Check the document's children after inserting `inserted` before `before`.
    fn validate_document_order(
        &self,
        inserted: &[NodeId],
        before: Option<NodeId>,
    ) -> Result<(), DomError> {
        // The DOM counts the node itself ("parent has an element/doctype
        // child"), so re-inserting the document element or doctype is rejected
        // rather than treated as a reorder.
        if inserted.iter().any(|id| {
            let node = self.live(*id);
            node.parent() == Some(self.root)
                && matches!(
                    node.kind(),
                    NodeKind::Element(_) | NodeKind::DocumentType(_)
                )
        }) {
            return Err(DomError::InvalidHierarchy);
        }
        let mut element_seen = false;
        let mut doctype_seen = false;
        let mut check = |id| {
            match self.live(id).kind() {
                NodeKind::Comment(_) | NodeKind::ProcessingInstruction(_) => {}
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
            if inserted.contains(&existing) {
                continue;
            }
            if Some(existing) == before {
                inserted.iter().try_for_each(|id| check(*id))?;
            }
            check(existing)?;
        }
        if before.is_none() {
            inserted.iter().try_for_each(|id| check(*id))?;
        }
        Ok(())
    }
}

fn contains_units(units: &[u16], needle: &str) -> bool {
    let needle: Vec<u16> = needle.encode_utf16().collect();
    units.windows(needle.len()).any(|window| window == needle)
}

/// Whether an HTML element named `local` may host a shadow root (DOM
/// `attachShadow` step 2 and HTML's valid custom element name).
fn can_host_shadow(local: &str) -> bool {
    const HOSTS: &[&str] = &[
        "article",
        "aside",
        "blockquote",
        "body",
        "div",
        "footer",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "header",
        "main",
        "nav",
        "p",
        "section",
        "span",
    ];
    const RESERVED: &[&str] = &[
        "annotation-xml",
        "color-profile",
        "font-face",
        "font-face-src",
        "font-face-uri",
        "font-face-format",
        "font-face-name",
        "missing-glyph",
    ];
    if HOSTS.contains(&local) {
        return true;
    }
    // A valid custom element name: a lowercase ASCII letter first, a hyphen
    // somewhere, no ASCII uppercase, and not a reserved name. The PCEN
    // character set is otherwise approximated by "not ASCII uppercase or
    // whitespace, and not a few ASCII punctuation characters".
    local.starts_with(|c: char| c.is_ascii_lowercase())
        && local.contains('-')
        && !local.chars().any(|c| {
            c.is_ascii_uppercase()
                || c.is_ascii_whitespace()
                || (c.is_ascii() && !c.is_ascii_alphanumeric() && !matches!(c, '-' | '.' | '_'))
        })
        && !RESERVED.contains(&local)
}
