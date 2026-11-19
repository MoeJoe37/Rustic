//! Wrapper identity and joint collection across one agent's documents.

use std::collections::{HashMap, TryReserveError};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WrapperKind {
    Node,
    Implementation,
}

#[derive(Clone, Copy, Debug)]
struct Wrapper {
    realm: RealmId,
    kind: WrapperKind,
    object: ObjectId,
    data: u64,
}

#[derive(Debug, Default)]
struct NodeBindings {
    wrappers: Vec<Wrapper>,
    edges: Vec<Value>,
}

impl NodeBindings {
    fn wrapper(&self, realm: RealmId, kind: WrapperKind) -> Option<&Wrapper> {
        self.wrappers
            .iter()
            .find(|wrapper| wrapper.realm == realm && wrapper.kind == kind)
    }
}

/// Wrapper identity and node-held values for the documents of one agent.
///
/// Use one coordinator for the agent, route its collections through this
/// heap while it owns DOM values, and supply only values from that agent
/// when registering node-held edges.
#[derive(Debug)]
pub struct DomHeap {
    documents: HashMap<DocumentId, bool>,
    agent: Option<AgentId>,
    bindings: HashMap<NodeId, NodeBindings>,
    // Embedder data keys are never reused, even across coordinators.
    nodes: HashMap<u64, (NodeId, ObjectId, WrapperKind)>,
}

impl DomHeap {
    /// Start with one document registered.
    pub fn new(document: &Document) -> Self {
        Self {
            documents: HashMap::from([(document.id(), true)]),
            agent: None,
            bindings: HashMap::new(),
            nodes: HashMap::new(),
        }
    }

    /// Register a document in the agent's collection domain. Returns whether
    /// the document was newly registered.
    pub fn attach_document(&mut self, document: &Document) -> Result<bool, DomGcError> {
        self.register_document(document, true)
    }

    pub(crate) fn register_document(
        &mut self,
        document: &Document,
        rooted: bool,
    ) -> Result<bool, DomGcError> {
        if self.documents.contains_key(&document.id()) {
            return Ok(false);
        }
        self.documents.try_reserve(1)?;
        self.documents.insert(document.id(), rooted);
        Ok(true)
    }

    pub(crate) fn has_document(&self, id: DocumentId) -> bool {
        self.documents.contains_key(&id)
    }

    pub(crate) fn discard_document(&mut self, id: DocumentId) {
        debug_assert_eq!(self.documents.get(&id), Some(&false));
        self.documents.remove(&id);
        self.bindings.retain(|node, bindings| {
            if node.document() != id {
                return true;
            }
            for wrapper in &bindings.wrappers {
                self.nodes.remove(&wrapper.data);
            }
            false
        });
    }

    fn check_document(&self, document: &Document) -> Result<(), DomGcError> {
        if !self.has_document(document.id()) {
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
        let prototype = agent.intrinsics.object_prototype;
        self.wrap_with_prototype(agent, document, node, prototype)
    }

    /// As [`Self::wrap`], with the realm's installed interface prototype
    /// for a wrapper that has yet to be created.
    pub(crate) fn wrap_with_prototype(
        &mut self,
        agent: &mut Agent,
        document: &Document,
        node: NodeId,
        prototype: ObjectId,
    ) -> Result<ObjectId, DomGcError> {
        self.wrap_with_kind(agent, document, node, prototype, WrapperKind::Node)
    }

    pub(crate) fn wrap_with_kind(
        &mut self,
        agent: &mut Agent,
        document: &Document,
        node: NodeId,
        prototype: ObjectId,
        kind: WrapperKind,
    ) -> Result<ObjectId, DomGcError> {
        self.check_document(document)?;
        document.node(node)?;
        self.check_agent(agent)?;
        let realm = agent.current_realm();
        if let Some(wrapper) = self
            .bindings
            .get(&node)
            .and_then(|b| b.wrapper(realm, kind))
        {
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
        let old = bindings
            .wrappers
            .iter()
            .position(|w| w.realm == realm && w.kind == kind);
        if old.is_none() {
            bindings.wrappers.try_reserve(1)?;
        }
        let data = NEXT_WRAPPER_DATA
            .allocate()
            .map_err(|_| DomError::AllocationFailed)?
            .get();
        let object = agent
            .new_host_object(Some(prototype), data)
            .map_err(|_| DomError::AllocationFailed)?;
        let wrapper = Wrapper {
            realm,
            kind,
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
        self.nodes.insert(data, (node, object, kind));
        self.agent = Some(agent.id());
        Ok(object)
    }

    /// The node a live wrapper stands for in `document`.
    pub fn node_of(&self, agent: &Agent, document: &Document, wrapper: ObjectId) -> Option<NodeId> {
        let node = self.identity(agent, wrapper, WrapperKind::Node)?;
        document.node(node).is_ok().then_some(node)
    }

    pub(crate) fn identity(
        &self,
        agent: &Agent,
        wrapper: ObjectId,
        kind: WrapperKind,
    ) -> Option<NodeId> {
        if self.agent != Some(agent.id()) {
            return None;
        }
        let &(node, object, role) = self.nodes.get(&agent.host_data(wrapper)?)?;
        (object == wrapper && role == kind).then_some(node)
    }

    /// The cached wrapper in the initial realm, without creating one.
    pub fn existing_wrapper(&self, node: NodeId) -> Option<ObjectId> {
        self.existing_wrapper_in_realm(node, RealmId::MAIN)
    }

    /// The cached wrapper in `realm`, without creating one.
    pub fn existing_wrapper_in_realm(&self, node: NodeId, realm: RealmId) -> Option<ObjectId> {
        self.bindings
            .get(&node)?
            .wrapper(realm, WrapperKind::Node)
            .map(|w| w.object)
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
        self.transfer(target, source, node, None)
    }

    pub(crate) fn insert_adopted(
        &mut self,
        target: &mut Document,
        source: &mut Document,
        node: NodeId,
        parent: NodeId,
        before: Option<NodeId>,
    ) -> Result<Adopted, DomGcError> {
        self.transfer(target, source, node, Some((parent, before)))
    }

    fn transfer(
        &mut self,
        target: &mut Document,
        source: &mut Document,
        node: NodeId,
        insert: Option<(NodeId, Option<NodeId>)>,
    ) -> Result<Adopted, DomGcError> {
        self.check_document(source)?;
        self.check_document(target)?;
        let reserve = |count: usize| {
            self.bindings
                .try_reserve(count.min(self.bindings.len()))
                .map_err(|_| DomError::AllocationFailed)
        };
        let adopted = match insert {
            Some((parent, before)) => {
                target.insert_from_document(source, node, parent, before, reserve)
            }
            None => target.adopt_node_with_preflight(source, node, reserve),
        }?;
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

    /// The values `node` keeps alive. Removal may reorder them.
    pub(crate) fn edges(&self, node: NodeId) -> &[Value] {
        self.bindings
            .get(&node)
            .map_or(&[], |bindings| bindings.edges.as_slice())
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
        self.collect_documents_with_nodes(agent, documents, roots, &[])
    }

    /// Collect while also retaining document nodes referenced by native state.
    pub(crate) fn collect_documents_with_nodes(
        &mut self,
        agent: &mut Agent,
        documents: &mut [&mut Document],
        roots: Roots<'_>,
        native_nodes: &[NodeId],
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
        for &node in native_nodes {
            let entry = marked
                .get(&node.document())
                .ok_or(DomGcError::DocumentSet)?;
            entry.document.node(node)?;
        }

        // An earlier JavaScript-only collection may have recycled an object slot.
        self.prune_wrappers(agent);
        self.agent = Some(agent.id());
        let mut tracer = Tracer {
            documents: &mut marked,
            heap: self,
            native_nodes,
            error: None,
        };
        let js = agent.collect_garbage_with(roots, &mut tracer);
        if let Some(error) = tracer.error {
            return Err(error.into());
        }
        let wrappers_freed = self.prune_wrappers(agent);
        let mut nodes_destroyed = 0;
        for (&id, entry) in &mut marked {
            if entry.marks.is_marked(entry.document.root()) {
                nodes_destroyed += entry.document.sweep_unmarked_trees(&entry.marks)?;
            } else {
                self.documents.remove(&id);
                nodes_destroyed += entry.document.node_count();
            }
        }

        let mut edges_dropped = 0;
        self.bindings.retain(|node, bindings| {
            let live = self.documents.contains_key(&node.document())
                && marked[&node.document()].document.node(*node).is_ok();
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
    native_nodes: &'a [NodeId],
    error: Option<DomError>,
}

impl HostTracer for Tracer<'_, '_> {
    fn roots(&mut self, sink: &mut dyn FnMut(Value)) {
        for (&id, entry) in &mut *self.documents {
            if !self.heap.documents[&id] {
                continue;
            }
            let root = entry.document.root();
            if let Err(error) = mark(self.heap, entry, root, sink) {
                self.error.get_or_insert(error);
            }
        }
        for &node in self.native_nodes {
            let entry = self
                .documents
                .get_mut(&node.document())
                .expect("registered native root");
            let root = entry.document.root();
            if let Err(error) =
                mark(self.heap, entry, root, sink).and_then(|()| mark(self.heap, entry, node, sink))
            {
                self.error.get_or_insert(error);
            }
        }
    }

    fn trace(&mut self, _: u64, _: &mut dyn FnMut(Value)) {}

    fn trace_object(&mut self, object: ObjectId, data: u64, sink: &mut dyn FnMut(Value)) {
        if let Some(&(node, wrapper, _)) = self.heap.nodes.get(&data) {
            let entry = self
                .documents
                .get_mut(&node.document())
                .expect("registered document");
            if object == wrapper && entry.document.node(node).is_ok() {
                let root = entry.document.root();
                if let Err(error) = mark(self.heap, entry, root, sink)
                    .and_then(|()| mark(self.heap, entry, node, sink))
                {
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
