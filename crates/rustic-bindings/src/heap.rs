//! Wrapper identity and joint collection across one agent's documents.

use std::collections::{HashMap, HashSet, TryReserveError};
use std::fmt;

use rustic_base::{DocumentId, IdSequence, NodeId};
use rustic_dom::{Adopted, Document, DomError, TreeMarks};
use rustic_js::runtime::{Agent, AgentId, GcStats, HostTracer, ObjectId, RealmId, Roots, Value};

static NEXT_WRAPPER_DATA: IdSequence = IdSequence::new();

/// A binding or joint collection failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomGcError {
    /// A document is not registered with this heap.
    WrongDocument,
    /// Collection requires each registered document exactly once.
    DocumentSet,
    /// The JavaScript agent is not the one bound to this heap.
    WrongAgent,
    /// A DOM operation failed.
    Dom(DomError),
}

impl fmt::Display for DomGcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongDocument => f.write_str("document is not bound to this heap"),
            Self::DocumentSet => f.write_str("collection requires every bound document once"),
            Self::WrongAgent => f.write_str("agent is not bound to this heap"),
            Self::Dom(e) => write!(f, "DOM binding error: {e}"),
        }
    }
}

impl std::error::Error for DomGcError {}

impl From<DomError> for DomGcError {
    fn from(e: DomError) -> Self {
        Self::Dom(e)
    }
}

impl From<TryReserveError> for DomGcError {
    fn from(_: TryReserveError) -> Self {
        DomError::AllocationFailed.into()
    }
}

/// What one collection did, on both sides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DomGcStats {
    /// The JavaScript heap's report.
    pub js: GcStats,
    /// Nodes destroyed with their unreachable trees.
    pub nodes_destroyed: usize,
    /// Tracked wrappers whose JavaScript objects were collected.
    pub wrappers_freed: usize,
    /// Node-held values dropped because their node was destroyed.
    pub edges_dropped: usize,
}

#[derive(Clone, Copy, Debug)]
struct Wrapper {
    realm: RealmId,
    object: ObjectId,
    data: u64,
}

#[derive(Debug, Default)]
struct NodeBindings {
    wrappers: Vec<Wrapper>,
    edges: Vec<Value>,
}

impl NodeBindings {
    fn wrapper(&self, realm: RealmId) -> Option<&Wrapper> {
        self.wrappers.iter().find(|wrapper| wrapper.realm == realm)
    }
}

/// Wrapper identity and node-held values for the documents of one agent.
///
/// Use one coordinator for the agent, route its collections through this
/// heap while it owns DOM values, and supply only values from that agent
/// when registering node-held edges.
#[derive(Debug)]
pub struct DomHeap {
    documents: HashSet<DocumentId>,
    agent: Option<AgentId>,
    bindings: HashMap<NodeId, NodeBindings>,
    // Embedder data keys are never reused, even across coordinators.
    nodes: HashMap<u64, (NodeId, ObjectId)>,
}

impl DomHeap {
    /// Start with one document registered.
    pub fn new(document: &Document) -> Self {
        Self {
            documents: HashSet::from([document.id()]),
            agent: None,
            bindings: HashMap::new(),
            nodes: HashMap::new(),
        }
    }

    /// Register a document in the agent's collection domain. Returns whether
    /// the document was newly registered.
    pub fn attach_document(&mut self, document: &Document) -> Result<bool, DomGcError> {
        if self.documents.contains(&document.id()) {
            return Ok(false);
        }
        self.documents.try_reserve(1)?;
        Ok(self.documents.insert(document.id()))
    }

    fn check_document(&self, document: &Document) -> Result<(), DomGcError> {
        if !self.documents.contains(&document.id()) {
            return Err(DomGcError::WrongDocument);
        }
        Ok(())
    }

    fn check_agent(&self, agent: &Agent) -> Result<(), DomGcError> {
        if self.agent.is_some_and(|id| id != agent.id()) {
            return Err(DomGcError::WrongAgent);
        }
        Ok(())
    }

    /// The wrapper of a node in the agent's current realm. Repeated calls
    /// in that realm return the same object. Until interface prototypes
    /// are installed, wrappers use the realm's `Object.prototype`.
    pub fn wrap(
        &mut self,
        agent: &mut Agent,
        document: &Document,
        node: NodeId,
    ) -> Result<ObjectId, DomGcError> {
        self.check_document(document)?;
        document.node(node)?;
        self.check_agent(agent)?;
        let realm = agent.current_realm();
        if let Some(wrapper) = self.bindings.get(&node).and_then(|b| b.wrapper(realm)) {
            if agent.host_data(wrapper.object) == Some(wrapper.data) {
                return Ok(wrapper.object);
            }
        }

        self.nodes.try_reserve(1)?;
        if !self.bindings.contains_key(&node) {
            self.bindings.try_reserve(1)?;
        }
        let mut fresh = NodeBindings::default();
        let bindings = self.bindings.get_mut(&node).unwrap_or(&mut fresh);
        let old = bindings.wrappers.iter().position(|w| w.realm == realm);
        if old.is_none() {
            bindings.wrappers.try_reserve(1)?;
        }
        let data = NEXT_WRAPPER_DATA
            .allocate()
            .map_err(|_| DomError::AllocationFailed)?
            .get();
        let object = agent
            .new_host_object(Some(agent.intrinsics.object_prototype), data)
            .map_err(|_| DomError::AllocationFailed)?;
        let wrapper = Wrapper {
            realm,
            object,
            data,
        };
        if let Some(at) = old {
            self.nodes.remove(&bindings.wrappers[at].data);
            bindings.wrappers[at] = wrapper;
        } else {
            bindings.wrappers.push(wrapper);
        }
        if !fresh.wrappers.is_empty() {
            self.bindings.insert(node, fresh);
        }
        self.nodes.insert(data, (node, object));
        self.agent = Some(agent.id());
        Ok(object)
    }

    /// The node a live wrapper stands for in `document`.
    pub fn node_of(&self, agent: &Agent, document: &Document, wrapper: ObjectId) -> Option<NodeId> {
        if self.agent != Some(agent.id()) {
            return None;
        }
        let &(node, object) = self.nodes.get(&agent.host_data(wrapper)?)?;
        (object == wrapper && document.node(node).is_ok()).then_some(node)
    }

    /// The cached wrapper in the initial realm, without creating one.
    pub fn existing_wrapper(&self, node: NodeId) -> Option<ObjectId> {
        self.existing_wrapper_in_realm(node, RealmId::MAIN)
    }

    /// The cached wrapper in `realm`, without creating one.
    pub fn existing_wrapper_in_realm(&self, node: NodeId, realm: RealmId) -> Option<ObjectId> {
        self.bindings.get(&node)?.wrapper(realm).map(|w| w.object)
    }

    /// Move a node between registered documents and preserve its subtree's
    /// wrappers and node-held values. Use this path for bound nodes so both
    /// sets of identities change together.
    pub fn adopt_node(
        &mut self,
        target: &mut Document,
        source: &mut Document,
        node: NodeId,
    ) -> Result<Adopted, DomGcError> {
        self.check_document(source)?;
        self.check_document(target)?;
        let adopted = target.adopt_node_with_preflight(source, node, |count| {
            self.bindings
                .try_reserve(count.min(self.bindings.len()))
                .map_err(|_| DomError::AllocationFailed)
        })?;
        for &(old, new) in &adopted.ids {
            if let Some(bindings) = self.bindings.remove(&old) {
                for wrapper in &bindings.wrappers {
                    self.nodes
                        .get_mut(&wrapper.data)
                        .expect("tracked wrapper")
                        .0 = new;
                }
                self.bindings.insert(new, bindings);
            }
        }
        Ok(adopted)
    }

    /// Make `node` keep an agent-local `value` alive.
    pub fn add_edge(
        &mut self,
        document: &Document,
        node: NodeId,
        value: Value,
    ) -> Result<(), DomGcError> {
        self.check_document(document)?;
        document.node(node)?;
        if let Some(bindings) = self.bindings.get_mut(&node) {
            bindings.edges.try_reserve(1)?;
            bindings.edges.push(value);
        } else {
            let mut bindings = NodeBindings::default();
            bindings.edges.try_reserve(1)?;
            bindings.edges.push(value);
            self.bindings.try_reserve(1)?;
            self.bindings.insert(node, bindings);
        }
        Ok(())
    }

    /// Remove one node-held edge. Returns whether it existed.
    pub fn remove_edge(&mut self, node: NodeId, value: Value) -> bool {
        let Some(bindings) = self.bindings.get_mut(&node) else {
            return false;
        };
        let Some(at) = bindings.edges.iter().position(|&v| v == value) else {
            return false;
        };
        bindings.edges.swap_remove(at);
        if bindings.edges.is_empty() && bindings.wrappers.is_empty() {
            self.bindings.remove(&node);
        }
        true
    }

    /// Tracked wrappers.
    pub fn wrapper_count(&self) -> usize {
        self.nodes.len()
    }

    /// Registered node-held edges.
    pub fn edge_count(&self) -> usize {
        self.bindings.values().map(|b| b.edges.len()).sum()
    }

    /// Collect an agent whose only registered document is `document`.
    pub fn collect(
        &mut self,
        agent: &mut Agent,
        document: &mut Document,
        roots: Roots<'_>,
    ) -> Result<DomGcStats, DomGcError> {
        self.collect_documents(agent, &mut [document], roots)
    }

    /// Collect the agent and all registered documents as one graph.
    /// `roots` are the values and environments held by the caller (see
    /// [`Agent::collect_garbage`]). Validation and mark-table allocation
    /// precede collection.
    pub fn collect_documents(
        &mut self,
        agent: &mut Agent,
        documents: &mut [&mut Document],
        roots: Roots<'_>,
    ) -> Result<DomGcStats, DomGcError> {
        self.check_agent(agent)?;
        for document in documents.iter() {
            self.check_document(document)?;
        }
        if documents.len() != self.documents.len() {
            return Err(DomGcError::DocumentSet);
        }
        let mut marked = HashMap::new();
        marked.try_reserve(documents.len())?;
        for document in documents {
            let marks = document.tree_marks()?;
            let id = document.id();
            if marked
                .insert(id, MarkedDocument { document, marks })
                .is_some()
            {
                return Err(DomGcError::DocumentSet);
            }
        }

        // An earlier JavaScript-only collection may have recycled an object slot.
        self.prune_wrappers(agent);
        self.agent = Some(agent.id());
        let mut tracer = Tracer {
            documents: &mut marked,
            heap: self,
            error: None,
        };
        let js = agent.collect_garbage_with(roots, &mut tracer);
        if let Some(error) = tracer.error {
            return Err(error.into());
        }
        let wrappers_freed = self.prune_wrappers(agent);
        let mut nodes_destroyed = 0;
        for entry in marked.values_mut() {
            nodes_destroyed += entry.document.sweep_unmarked_trees(&entry.marks)?;
        }

        let mut edges_dropped = 0;
        self.bindings.retain(|node, bindings| {
            let live = marked[&node.document()].document.node(*node).is_ok();
            if !live {
                edges_dropped += bindings.edges.len();
                for wrapper in &bindings.wrappers {
                    self.nodes.remove(&wrapper.data);
                }
            }
            live && (!bindings.edges.is_empty() || !bindings.wrappers.is_empty())
        });
        Ok(DomGcStats {
            js,
            nodes_destroyed,
            wrappers_freed,
            edges_dropped,
        })
    }

    fn prune_wrappers(&mut self, agent: &Agent) -> usize {
        let before = self.nodes.len();
        for bindings in self.bindings.values_mut() {
            bindings.wrappers.retain(|wrapper| {
                let live = agent.host_data(wrapper.object) == Some(wrapper.data);
                if !live {
                    self.nodes.remove(&wrapper.data);
                }
                live
            });
        }
        before - self.nodes.len()
    }
}

struct MarkedDocument<'a> {
    document: &'a mut Document,
    marks: TreeMarks,
}

struct Tracer<'a, 'doc> {
    documents: &'a mut HashMap<DocumentId, MarkedDocument<'doc>>,
    heap: &'a DomHeap,
    error: Option<DomError>,
}

impl HostTracer for Tracer<'_, '_> {
    fn roots(&mut self, sink: &mut dyn FnMut(Value)) {
        for entry in self.documents.values_mut() {
            let root = entry.document.root();
            if let Err(error) = mark(self.heap, entry, root, sink) {
                self.error.get_or_insert(error);
            }
        }
    }

    fn trace(&mut self, _: u64, _: &mut dyn FnMut(Value)) {}

    fn trace_object(&mut self, object: ObjectId, data: u64, sink: &mut dyn FnMut(Value)) {
        if let Some(&(node, wrapper)) = self.heap.nodes.get(&data) {
            let entry = self
                .documents
                .get_mut(&node.document())
                .expect("registered document");
            if object == wrapper && entry.document.node(node).is_ok() {
                if let Err(error) = mark(self.heap, entry, node, sink) {
                    self.error.get_or_insert(error);
                }
            }
        }
    }
}

fn mark(
    heap: &DomHeap,
    entry: &mut MarkedDocument<'_>,
    node: NodeId,
    sink: &mut dyn FnMut(Value),
) -> Result<(), DomError> {
    entry.document.mark_tree(&mut entry.marks, node, &mut |id| {
        if let Some(bindings) = heap.bindings.get(&id) {
            for wrapper in &bindings.wrappers {
                sink(Value::object(wrapper.object));
            }
            for &value in &bindings.edges {
                sink(value);
            }
        }
    })
}
