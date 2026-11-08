//! Prototype handle/root/tracing contract between DOM nodes and host objects.
//!
//! [`HostHeap`] stands in for the future JavaScript heap (JS-03) so the DOM side
//! of the contract can be tested before a runtime exists. It is a nonmoving,
//! stop-the-world mark/sweep model bound to one [`Document`]:
//!
//! - A node is reachable when its tree is reachable. Holding any node keeps its
//!   whole tree alive (parent, sibling and child links are script-visible), and a
//!   template and its contents keep each other alive.
//! - Roots are the document's own tree, counted node roots (host handles) and
//!   counted object roots (host stack/persistent handles).
//! - Nodes can hold host objects (wrappers, expandos, listeners) and host objects
//!   can hold nodes or other objects. These edges are registered explicitly.
//! - Collection destroys unreachable detached trees through the same
//!   allocation-free destruction the engine uses, so their IDs become stale, and
//!   frees unreachable objects with generation-checked identities.
//!
//! See ADR 0012 for scope and the planned hand-off to the real heap.

use std::fmt;
use std::num::NonZeroU64;

use rustic_base::{Budget, DocumentId, NodeId};

use crate::{Document, DomError, NodeKind};

/// Resource limits for a [`HostHeap`].
#[derive(Clone, Copy, Debug)]
pub struct HostHeapLimits {
    /// Maximum live host objects.
    pub max_objects: usize,
    /// Maximum registered edges plus distinct rooted nodes.
    pub max_edges: usize,
}

impl Default for HostHeapLimits {
    fn default() -> Self {
        Self {
            max_objects: 100_000,
            max_edges: 400_000,
        }
    }
}

/// A host object's heap, slot and slot generation.
///
/// Like [`NodeId`], an identity does not keep its object alive; only roots and
/// edges do. A freed object's identity never becomes valid again.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HostObjectId {
    document: DocumentId,
    slot: u32,
    generation: NonZeroU64,
}

/// One end of a traced edge or a root.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Traced {
    /// A node of the heap's document.
    Node(NodeId),
    /// A live host object of the same heap.
    Object(HostObjectId),
}

/// A rejected heap operation. Errors leave the heap and document unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostHeapError {
    /// A node lookup failed (stale, forged or another document's node).
    Node(DomError),
    /// The heap belongs to another document, or the object to another heap.
    WrongDocument,
    /// The object identity is stale or forged.
    InvalidObject,
    /// Node-to-node edges are owned by the DOM tree, not registered here.
    NodeToNodeEdge,
    /// The edge to remove is not registered.
    MissingEdge,
    /// The target is not rooted.
    NotRooted,
    /// The live object limit would be exceeded.
    ObjectLimit,
    /// The edge/root limit would be exceeded.
    EdgeLimit,
    /// The object slot space or a root counter is exhausted.
    IdentityExhausted,
    /// Storage reservation failed.
    AllocationFailed,
}

impl fmt::Display for HostHeapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Node(error) => error.fmt(formatter),
            Self::WrongDocument => formatter.write_str("host heap belongs to another document"),
            Self::InvalidObject => formatter.write_str("host object identity is stale or invalid"),
            Self::NodeToNodeEdge => formatter.write_str("node-to-node edges belong to the tree"),
            Self::MissingEdge => formatter.write_str("edge is not registered"),
            Self::NotRooted => formatter.write_str("target is not rooted"),
            Self::ObjectLimit => formatter.write_str("host object limit exceeded"),
            Self::EdgeLimit => formatter.write_str("host edge limit exceeded"),
            Self::IdentityExhausted => formatter.write_str("host identity space exhausted"),
            Self::AllocationFailed => formatter.write_str("host heap reservation failed"),
        }
    }
}

impl std::error::Error for HostHeapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Node(error) => Some(error),
            _ => None,
        }
    }
}

impl From<DomError> for HostHeapError {
    fn from(error: DomError) -> Self {
        Self::Node(error)
    }
}

/// What one [`HostHeap::collect`] did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CollectionStats {
    /// Unreachable nodes destroyed, including template contents.
    pub nodes_destroyed: usize,
    /// Unreachable host objects freed.
    pub objects_freed: usize,
    /// Edges and roots dropped because their node was destroyed by the engine
    /// or by this collection.
    pub entries_pruned: usize,
}

#[derive(Debug)]
struct HostObject {
    edges: Vec<Traced>,
    roots: usize,
}

#[derive(Debug)]
struct ObjectSlot {
    generation: NonZeroU64,
    object: Option<HostObject>,
    next_free: Option<u32>,
}

/// Prototype traced host heap for one document; see the module documentation.
#[derive(Debug)]
pub struct HostHeap {
    document: DocumentId,
    slots: Vec<ObjectSlot>,
    free: Option<u32>,
    objects: Budget,
    edges: Budget,
    /// Node-held objects, such as a wrapper or an event listener.
    node_edges: Vec<(NodeId, HostObjectId)>,
    /// Counted node roots; each distinct node is charged one edge.
    node_roots: Vec<(NodeId, usize)>,
}

impl HostHeap {
    /// Create an empty heap bound to `document`.
    pub fn new(document: &Document, limits: HostHeapLimits) -> Self {
        Self {
            document: document.id(),
            slots: Vec::new(),
            free: None,
            objects: Budget::new(limits.max_objects),
            edges: Budget::new(limits.max_edges),
            node_edges: Vec::new(),
            node_roots: Vec::new(),
        }
    }

    /// Live host objects.
    pub fn object_count(&self) -> usize {
        self.objects.used()
    }

    /// Registered edges plus distinct rooted nodes.
    pub fn edge_count(&self) -> usize {
        self.edges.used()
    }

    /// Whether the identity names a live object of this heap.
    pub fn contains(&self, object: HostObjectId) -> bool {
        self.object(object).is_ok()
    }

    /// Allocate an unrooted object with no edges.
    ///
    /// It is garbage at the next collection unless something roots or holds it.
    pub fn allocate(&mut self) -> Result<HostObjectId, HostHeapError> {
        let index = match self.free {
            Some(index) => index,
            None => {
                let index = u32::try_from(self.slots.len())
                    .map_err(|_| HostHeapError::IdentityExhausted)?;
                self.slots
                    .try_reserve(1)
                    .map_err(|_| HostHeapError::AllocationFailed)?;
                index
            }
        };
        self.objects
            .charge(1)
            .map_err(|_| HostHeapError::ObjectLimit)?;
        let object = Some(HostObject {
            edges: Vec::new(),
            roots: 0,
        });
        let generation = if self.free.is_some() {
            let slot = &mut self.slots[index as usize];
            self.free = slot.next_free.take();
            slot.object = object;
            slot.generation
        } else {
            self.slots.push(ObjectSlot {
                generation: NonZeroU64::MIN,
                object,
                next_free: None,
            });
            NonZeroU64::MIN
        };
        Ok(HostObjectId {
            document: self.document,
            slot: index,
            generation,
        })
    }

    /// Register an edge `from -> to`; duplicates are counted separately.
    pub fn add_edge(
        &mut self,
        document: &Document,
        from: Traced,
        to: Traced,
    ) -> Result<(), HostHeapError> {
        self.check(document, from)?;
        self.check(document, to)?;
        match (from, to) {
            (Traced::Node(_), Traced::Node(_)) => Err(HostHeapError::NodeToNodeEdge),
            (Traced::Node(node), Traced::Object(object)) => {
                self.node_edges
                    .try_reserve(1)
                    .map_err(|_| HostHeapError::AllocationFailed)?;
                self.edges.charge(1).map_err(|_| HostHeapError::EdgeLimit)?;
                self.node_edges.push((node, object));
                Ok(())
            }
            (Traced::Object(object), to) => {
                let slot = object.slot as usize;
                let edges = &mut self.slots[slot].object.as_mut().expect("checked").edges;
                edges
                    .try_reserve(1)
                    .map_err(|_| HostHeapError::AllocationFailed)?;
                self.edges.charge(1).map_err(|_| HostHeapError::EdgeLimit)?;
                edges.push(to);
                Ok(())
            }
        }
    }

    /// Remove one registered `from -> to` edge.
    pub fn remove_edge(
        &mut self,
        document: &Document,
        from: Traced,
        to: Traced,
    ) -> Result<(), HostHeapError> {
        self.check(document, from)?;
        self.check(document, to)?;
        let removed = match (from, to) {
            (Traced::Node(_), Traced::Node(_)) => return Err(HostHeapError::NodeToNodeEdge),
            (Traced::Node(node), Traced::Object(object)) => self
                .node_edges
                .iter()
                .position(|edge| *edge == (node, object))
                .map(|index| self.node_edges.swap_remove(index))
                .is_some(),
            (Traced::Object(object), to) => {
                let edges = &mut self.slots[object.slot as usize]
                    .object
                    .as_mut()
                    .expect("checked")
                    .edges;
                edges
                    .iter()
                    .position(|edge| *edge == to)
                    .map(|index| edges.swap_remove(index))
                    .is_some()
            }
        };
        if !removed {
            return Err(HostHeapError::MissingEdge);
        }
        self.edges.release(1).expect("release registered edge");
        Ok(())
    }

    /// Add one root count for a node or object (a host handle).
    pub fn root(&mut self, document: &Document, target: Traced) -> Result<(), HostHeapError> {
        self.check(document, target)?;
        match target {
            Traced::Object(object) => {
                let roots = &mut self.slots[object.slot as usize]
                    .object
                    .as_mut()
                    .expect("checked")
                    .roots;
                *roots = roots
                    .checked_add(1)
                    .ok_or(HostHeapError::IdentityExhausted)?;
            }
            Traced::Node(node) => {
                if let Some((_, count)) = self.node_roots.iter_mut().find(|(id, _)| *id == node) {
                    *count = count
                        .checked_add(1)
                        .ok_or(HostHeapError::IdentityExhausted)?;
                } else {
                    self.node_roots
                        .try_reserve(1)
                        .map_err(|_| HostHeapError::AllocationFailed)?;
                    self.edges.charge(1).map_err(|_| HostHeapError::EdgeLimit)?;
                    self.node_roots.push((node, 1));
                }
            }
        }
        Ok(())
    }

    /// Remove one root count added by [`Self::root`].
    ///
    /// A node destroyed by the engine has a stale ID; its roots are pruned by the
    /// next collection rather than unrooted here.
    pub fn unroot(&mut self, document: &Document, target: Traced) -> Result<(), HostHeapError> {
        self.check(document, target)?;
        match target {
            Traced::Object(object) => {
                let roots = &mut self.slots[object.slot as usize]
                    .object
                    .as_mut()
                    .expect("checked")
                    .roots;
                *roots = roots.checked_sub(1).ok_or(HostHeapError::NotRooted)?;
            }
            Traced::Node(node) => {
                let index = self
                    .node_roots
                    .iter()
                    .position(|(id, _)| *id == node)
                    .ok_or(HostHeapError::NotRooted)?;
                self.node_roots[index].1 -= 1;
                if self.node_roots[index].1 == 0 {
                    self.node_roots.swap_remove(index);
                    self.edges.release(1).expect("release node root");
                }
            }
        }
        Ok(())
    }

    /// Stop-the-world mark/sweep over the document and this heap.
    ///
    /// All temporary storage is reserved before anything changes, so an error
    /// leaves both untouched. Marking and destruction are iterative; content
    /// cannot drive recursion. Cost is linear in slots, edges and roots.
    pub fn collect(&mut self, document: &mut Document) -> Result<CollectionStats, HostHeapError> {
        if document.id() != self.document {
            return Err(HostHeapError::WrongDocument);
        }
        let live_nodes = document.node_count();

        // Template contents are parentless roots kept alive by, and keeping
        // alive, their template element (`Node::template_host`).
        let host_of = |fragment: NodeId| document.live(fragment).template_host();

        let mut node_marks = Vec::new();
        node_marks
            .try_reserve_exact(document.arena.slot_count())
            .map_err(|_| HostHeapError::AllocationFailed)?;
        node_marks.resize(document.arena.slot_count(), false);
        let mut object_marks = Vec::new();
        object_marks
            .try_reserve_exact(self.slots.len())
            .map_err(|_| HostHeapError::AllocationFailed)?;
        object_marks.resize(self.slots.len(), false);

        // Every push is bounded: each tree and object is expanded once, so the
        // worklist never holds more than roots + edges + 2 × nodes entries.
        let capacity = 1usize
            .checked_add(self.node_roots.len())
            .and_then(|value| value.checked_add(self.objects.used()))
            .and_then(|value| value.checked_add(self.edges.used()))
            .and_then(|value| value.checked_add(live_nodes.checked_mul(2)?))
            .ok_or(HostHeapError::AllocationFailed)?;
        let mut work = Vec::new();
        work.try_reserve_exact(capacity)
            .map_err(|_| HostHeapError::AllocationFailed)?;
        let mut victims = Vec::new();
        victims
            .try_reserve_exact(live_nodes)
            .map_err(|_| HostHeapError::AllocationFailed)?;

        // Edge lookup by node; order of registered edges is not observable.
        self.node_edges
            .sort_unstable_by_key(|(node, _)| node.slot());

        work.push(Traced::Node(document.root()));
        work.extend(self.node_roots.iter().map(|(node, _)| Traced::Node(*node)));
        for (index, slot) in self.slots.iter().enumerate() {
            if slot.object.as_ref().is_some_and(|object| object.roots > 0) {
                work.push(Traced::Object(HostObjectId {
                    document: self.document,
                    slot: u32::try_from(index).expect("slot count is checked at allocation"),
                    generation: slot.generation,
                }));
            }
        }

        while let Some(item) = work.pop() {
            match item {
                Traced::Node(node) => {
                    if document.node(node).is_err() {
                        continue; // Destroyed by the engine; pruned below.
                    }
                    let mut tree = node;
                    while let Some(parent) = document.live(tree).parent() {
                        tree = parent;
                    }
                    if node_marks[tree.slot() as usize] {
                        continue;
                    }
                    if let Some(host) = host_of(tree) {
                        work.push(Traced::Node(host));
                    }
                    for (id, _) in document.walk(tree)? {
                        node_marks[id.slot() as usize] = true;
                        if let NodeKind::Element(element) = document.live(id).kind() {
                            if let Some(contents) = element.template_contents() {
                                work.push(Traced::Node(contents));
                            }
                        }
                        let start = self
                            .node_edges
                            .partition_point(|(edge, _)| edge.slot() < id.slot());
                        work.extend(
                            self.node_edges[start..]
                                .iter()
                                .take_while(|(edge, _)| edge.slot() == id.slot())
                                .filter(|(edge, _)| *edge == id)
                                .map(|(_, object)| Traced::Object(*object)),
                        );
                    }
                }
                Traced::Object(object) => {
                    let mark = &mut object_marks[object.slot as usize];
                    if *mark {
                        continue;
                    }
                    *mark = true;
                    let object = self.object(object).expect("registered objects are live");
                    work.extend(object.edges.iter().copied());
                }
            }
        }

        // Sweep nodes: unmarked trees are whole trees. Template contents go with
        // their template, so only other parentless roots are destroyed directly.
        victims.extend(document.arena.live_ids().filter(|id| {
            !node_marks[id.slot() as usize]
                && document.live(*id).parent().is_none()
                && host_of(*id).is_none()
        }));
        for root in victims {
            document.destroy_subtree(root)?;
        }
        let mut stats = CollectionStats {
            nodes_destroyed: live_nodes - document.node_count(),
            ..CollectionStats::default()
        };

        // Sweep objects and drop entries that name destroyed nodes.
        for (index, marked) in object_marks.into_iter().enumerate() {
            let slot = &mut self.slots[index];
            if marked || slot.object.is_none() {
                continue;
            }
            let object = slot.object.take().expect("live object");
            self.edges
                .release(object.edges.len())
                .expect("release object edges");
            self.objects.release(1).expect("release object");
            stats.objects_freed += 1;
            if let Some(next) = slot
                .generation
                .get()
                .checked_add(1)
                .and_then(NonZeroU64::new)
            {
                slot.generation = next;
                slot.next_free = self.free;
                self.free =
                    Some(u32::try_from(index).expect("slot count is checked at allocation"));
            }
            // A generation at u64::MAX is retired permanently, never reused.
        }
        let live = |node: NodeId| document.node(node).is_ok();
        let before = self.node_edges.len() + self.node_roots.len();
        self.node_edges.retain(|(node, _)| live(*node));
        self.node_roots.retain(|(node, _)| live(*node));
        let mut pruned = before - self.node_edges.len() - self.node_roots.len();
        for slot in &mut self.slots {
            if let Some(object) = &mut slot.object {
                let before = object.edges.len();
                object.edges.retain(|edge| match edge {
                    Traced::Node(node) => live(*node),
                    Traced::Object(_) => true,
                });
                pruned += before - object.edges.len();
            }
        }
        self.edges.release(pruned).expect("release pruned entries");
        stats.entries_pruned = pruned;
        Ok(stats)
    }

    fn check(&self, document: &Document, target: Traced) -> Result<(), HostHeapError> {
        if document.id() != self.document {
            return Err(HostHeapError::WrongDocument);
        }
        match target {
            Traced::Node(node) => document.node(node).map(|_| ()).map_err(Into::into),
            Traced::Object(object) => self.object(object).map(|_| ()),
        }
    }

    fn object(&self, id: HostObjectId) -> Result<&HostObject, HostHeapError> {
        if id.document != self.document {
            return Err(HostHeapError::WrongDocument);
        }
        self.slots
            .get(id.slot as usize)
            .filter(|slot| slot.generation == id.generation)
            .and_then(|slot| slot.object.as_ref())
            .ok_or(HostHeapError::InvalidObject)
    }
}
