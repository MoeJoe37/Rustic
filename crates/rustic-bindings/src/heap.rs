//! One collection over a document and an agent.

use std::collections::HashMap;
use std::fmt;

use rustic_base::{DocumentId, NodeId};
use rustic_dom::{Document, DomError, TreeMarks};
use rustic_js::runtime::{Agent, GcStats, HostTracer, ObjectId, Roots, Value};

/// A collection failed before anything was freed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomGcError {
    /// The document is not the one this heap binds.
    WrongDocument,
    /// The DOM refused (allocation of the mark table, or a corrupted tree).
    Dom(DomError),
}

impl fmt::Display for DomGcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongDocument => f.write_str("document is not bound to this heap"),
            Self::Dom(e) => write!(f, "DOM error during collection: {e}"),
        }
    }
}

impl std::error::Error for DomGcError {}

impl From<DomError> for DomGcError {
    fn from(e: DomError) -> Self {
        Self::Dom(e)
    }
}

/// What one collection did, on both sides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DomGcStats {
    /// The JavaScript heap's report.
    pub js: GcStats,
    /// Nodes destroyed with their unreachable trees.
    pub nodes_destroyed: usize,
    /// Wrappers whose node went (each wrapper object is in `js` too).
    pub wrappers_freed: usize,
    /// Node-held values dropped because their node was destroyed by the
    /// engine (outside a collection) or collected.
    pub edges_dropped: usize,
}

/// Wrapper identity and node-held values for one document, and the
/// collection that spans it and an agent.
///
/// The heap is bound to one document: the bindings for a realm with
/// several documents, and retargeting through adoption, belong to WEB-02
/// (TD-17).
#[derive(Debug)]
pub struct DomHeap {
    document: DocumentId,
    /// Node → its wrapper.
    wrappers: HashMap<NodeId, ObjectId>,
    /// Host data → node. Data values are never reused.
    nodes: HashMap<u64, NodeId>,
    next_data: u64,
    /// Values a node keeps alive: event listeners, expandos and the like.
    edges: HashMap<NodeId, Vec<Value>>,
}

impl DomHeap {
    /// Bindings for `document`.
    pub fn new(document: &Document) -> Self {
        Self {
            document: document.id(),
            wrappers: HashMap::new(),
            nodes: HashMap::new(),
            next_data: 0,
            edges: HashMap::new(),
        }
    }

    fn check(&self, document: &Document, node: NodeId) -> Result<(), DomGcError> {
        if document.id() != self.document {
            return Err(DomGcError::WrongDocument);
        }
        document.node(node)?;
        Ok(())
    }

    /// The wrapper of `node`, created on first use and the same object
    /// every time after (wrapper identity). The wrapper's prototype is set
    /// by WEB-02's generated bindings; it starts as `Object.prototype`.
    pub fn wrap(
        &mut self,
        agent: &mut Agent,
        document: &Document,
        node: NodeId,
    ) -> Result<ObjectId, DomGcError> {
        self.check(document, node)?;
        if let Some(&wrapper) = self.wrappers.get(&node) {
            return Ok(wrapper);
        }
        let data = self.next_data;
        let proto = agent.intrinsics.object_prototype;
        let wrapper = agent
            .new_host_object(Some(proto), data)
            .map_err(|_| DomGcError::Dom(DomError::AllocationFailed))?;
        self.next_data += 1;
        self.nodes.insert(data, node);
        self.wrappers.insert(node, wrapper);
        Ok(wrapper)
    }

    /// The node a wrapper stands for, if it is one of this heap's wrappers
    /// and the node still exists.
    pub fn node_of(&self, agent: &Agent, document: &Document, wrapper: ObjectId) -> Option<NodeId> {
        let node = *self.nodes.get(&agent.host_data(wrapper)?)?;
        (self.wrappers.get(&node) == Some(&wrapper) && document.node(node).is_ok()).then_some(node)
    }

    /// The wrapper `node` already has, without creating one.
    pub fn existing_wrapper(&self, node: NodeId) -> Option<ObjectId> {
        self.wrappers.get(&node).copied()
    }

    /// Make `node` keep `value` alive (an event listener, for example).
    pub fn add_edge(
        &mut self,
        document: &Document,
        node: NodeId,
        value: Value,
    ) -> Result<(), DomGcError> {
        self.check(document, node)?;
        self.edges.entry(node).or_default().push(value);
        Ok(())
    }

    /// Remove one `node` → `value` edge; `false` if there was none.
    pub fn remove_edge(&mut self, node: NodeId, value: Value) -> bool {
        let Some(values) = self.edges.get_mut(&node) else {
            return false;
        };
        let Some(at) = values.iter().position(|v| *v == value) else {
            return false;
        };
        values.swap_remove(at);
        if values.is_empty() {
            self.edges.remove(&node);
        }
        true
    }

    /// Live wrappers.
    pub fn wrapper_count(&self) -> usize {
        self.wrappers.len()
    }

    /// Registered node → value edges.
    pub fn edge_count(&self) -> usize {
        self.edges.values().map(Vec::len).sum()
    }

    /// Collect the agent and the document together. `roots` are the
    /// values and environments the caller holds (see
    /// [`Agent::collect_garbage`]). Fails before freeing anything if the
    /// document is foreign or its mark table cannot be allocated.
    pub fn collect(
        &mut self,
        agent: &mut Agent,
        document: &mut Document,
        roots: Roots<'_>,
    ) -> Result<DomGcStats, DomGcError> {
        if document.id() != self.document {
            return Err(DomGcError::WrongDocument);
        }
        // Edges of nodes the engine destroyed outside a collection.
        let mut edges_dropped = 0;
        self.edges.retain(|node, values| {
            let live = document.node(*node).is_ok();
            if !live {
                edges_dropped += values.len();
            }
            live
        });
        let marks = document.tree_marks()?;
        let mut tracer = Tracer {
            document,
            marks,
            heap: self,
            error: None,
            finalized: Vec::new(),
        };
        let js = agent.collect_garbage_with(roots, &mut tracer);
        let Tracer {
            marks,
            error,
            finalized,
            ..
        } = tracer;
        let wrappers_freed = finalized.len();
        for data in finalized {
            if let Some(node) = self.nodes.remove(&data) {
                self.wrappers.remove(&node);
            }
        }
        if let Some(e) = error {
            // Marking failed partway. The JS side has swept, but nothing on
            // the DOM side is destroyed, so no live node is lost.
            return Err(e.into());
        }
        let nodes_destroyed = document.sweep_unmarked_trees(&marks)?;
        self.edges.retain(|node, values| {
            let live = document.node(*node).is_ok();
            if !live {
                edges_dropped += values.len();
            }
            live
        });
        // Wrappers of nodes the engine destroyed: the wrapper stays a valid
        // object, but no longer names a node.
        let stale: Vec<NodeId> = self
            .wrappers
            .keys()
            .filter(|node| document.node(**node).is_err())
            .copied()
            .collect();
        for node in stale {
            self.wrappers.remove(&node);
        }
        self.nodes
            .retain(|_, node| self.wrappers.contains_key(node));
        Ok(DomGcStats {
            js,
            nodes_destroyed,
            wrappers_freed,
            edges_dropped,
        })
    }
}

struct Tracer<'a> {
    document: &'a Document,
    marks: TreeMarks,
    heap: &'a DomHeap,
    error: Option<DomError>,
    finalized: Vec<u64>,
}

impl Tracer<'_> {
    fn mark(&mut self, node: NodeId, sink: &mut dyn FnMut(Value)) {
        let heap = self.heap;
        let mut visit = |id: NodeId| {
            if let Some(&wrapper) = heap.wrappers.get(&id) {
                sink(Value::object(wrapper));
            }
            for &value in heap.edges.get(&id).into_iter().flatten() {
                sink(value);
            }
        };
        if let Err(e) = self.document.mark_tree(&mut self.marks, node, &mut visit) {
            self.error.get_or_insert(e);
        }
    }
}

impl HostTracer for Tracer<'_> {
    fn roots(&mut self, sink: &mut dyn FnMut(Value)) {
        let root = self.document.root();
        self.mark(root, sink);
    }

    fn trace(&mut self, data: u64, sink: &mut dyn FnMut(Value)) {
        // A wrapper whose node the engine destroyed holds nothing.
        if let Some(&node) = self.heap.nodes.get(&data) {
            if self.document.node(node).is_ok() {
                self.mark(node, sink);
            }
        }
    }

    fn finalize(&mut self, data: u64) {
        self.finalized.push(data);
    }
}
