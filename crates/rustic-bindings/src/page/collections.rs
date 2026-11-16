//! Static and live NodeList objects.

use rustic_dom::{Document, NodeId, NodeKind};
use rustic_js::runtime::{
    Agent, JsResult, LiveIndexAccess, ObjectId, OutOfMemory, PropertyDescriptor, PropertyKey, Value,
};

use crate::dom_exception;
use crate::webidl::{IntegerConversion, to_unsigned_long};

use super::{
    DomHeap, illegal_constructor, install_interface, prototype, read_node, required_arg, with_page,
    wrap,
};

pub(super) const NODE_LIST: &str = "NodeList";

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        NODE_LIST,
        None,
        illegal_constructor,
        0,
        &[("length", "get length", length)],
        |agent, prototype| {
            let key = agent.heap.key("item")?;
            agent.define_method(prototype, key, item, "item", 1)?;
            for name in ["entries", "keys", "values", "forEach"] {
                let key = agent.heap.key(name)?;
                let value = agent.get(agent.intrinsics.array_prototype, key)?;
                agent.define_property_or_throw(
                    prototype,
                    key,
                    PropertyDescriptor::data_with(value, true, false, true),
                )?;
            }
            agent.define_property_or_throw(
                prototype,
                PropertyKey::Symbol(agent.symbols.iterator),
                PropertyDescriptor::data_with(
                    Value::object(agent.intrinsics.array_values),
                    true,
                    false,
                    true,
                ),
            )
        },
    )
}

pub(super) fn receiver(
    agent: &mut Agent,
    value: Value,
    interface: &'static str,
) -> JsResult<ObjectId> {
    match value.as_object() {
        Some(object) if agent.has_indexed_brand(object, interface) => Ok(object),
        _ => agent.type_error("invalid DOM collection receiver"),
    }
}

fn length(agent: &mut Agent, value: Value, _: &[Value]) -> JsResult<Value> {
    let object = receiver(agent, value, NODE_LIST)?;
    Ok(Value::number(f64::from(agent.indexed_host_len(object)?)))
}

fn item(agent: &mut Agent, value: Value, args: &[Value]) -> JsResult<Value> {
    let object = receiver(agent, value, NODE_LIST)?;
    let index = required_arg(agent, args, 0)?;
    let index = to_unsigned_long(agent, index, IntegerConversion::Wrap)?;
    Ok(agent
        .indexed_host_item(object, index)?
        .unwrap_or(Value::NULL))
}

pub(super) fn snapshot(
    agent: &mut Agent,
    wrappers: &mut DomHeap,
    document: &Document,
    nodes: Vec<NodeId>,
) -> JsResult<Value> {
    let prototype = prototype(agent, NODE_LIST)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(nodes.len())
        .map_err(|_| OutOfMemory)?;
    for id in nodes {
        values.push(wrap(wrappers, agent, document, id)?);
    }
    agent
        .new_indexed_snapshot(NODE_LIST, prototype, values.into_boxed_slice())
        .map(Value::object)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum ChildKind {
    Nodes,
    Elements,
}

#[derive(Debug, Default)]
pub(super) struct ChildIndex {
    key: Option<(NodeId, u64)>,
    nodes: Vec<NodeId>,
    elements: Vec<NodeId>,
}

impl ChildIndex {
    fn rebuild(&mut self, agent: &mut Agent, document: &Document, root: NodeId) -> JsResult<()> {
        let key = document.child_revision().map(|revision| (root, revision));
        if key.is_some() && self.key == key {
            return Ok(());
        }
        self.key = None;
        self.nodes.clear();
        self.elements.clear();
        let children = document
            .children(root)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        for id in children {
            agent.check_native_work()?;
            self.nodes.try_reserve(1).map_err(|_| OutOfMemory)?;
            self.nodes.push(id);
            let node = document
                .node(id)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
            if matches!(node.kind(), NodeKind::Element(_)) {
                self.elements.try_reserve(1).map_err(|_| OutOfMemory)?;
                self.elements.push(id);
            }
        }
        self.key = key;
        Ok(())
    }

    pub(super) fn discard_collected(&mut self, document: &Document) {
        if self
            .key
            .is_some_and(|(root, _)| document.node(root).is_err())
        {
            *self = Self::default();
        }
    }
}

static CHILD_INDEX: LiveIndexAccess = LiveIndexAccess {
    length: child_count,
    item: child_item,
    named: None,
};

pub(super) fn child_nodes(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    cached_children(agent, receiver, ChildKind::Nodes, NODE_LIST, &CHILD_INDEX)
}

pub(super) fn cached_children(
    agent: &mut Agent,
    receiver: Value,
    kind: ChildKind,
    interface: &'static str,
    access: &'static LiveIndexAccess,
) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        read_node(agent, receiver, |agent, wrappers, document, id, _| {
            let key = (id, agent.current_realm(), kind);
            let Ok(mut cache) = page.child_lists.try_borrow_mut() else {
                return agent.type_error("DOM collections are busy");
            };
            if let Some(&object) = cache.get(&key) {
                return Ok(Value::object(object));
            }
            cache.try_reserve(1).map_err(|_| OutOfMemory)?;
            let proto = prototype(agent, interface)?;
            let object = agent.new_indexed_view(interface, proto, receiver, access)?;
            let result = Value::object(object);
            wrappers
                .add_edge(document, id, result)
                .map_err(|_| OutOfMemory)?;
            cache.insert(key, object);
            Ok(result)
        })
    })
}

pub(super) fn with_children<T>(
    agent: &mut Agent,
    source: Value,
    kind: ChildKind,
    visit: impl FnOnce(&mut Agent, &mut DomHeap, &Document, &[NodeId]) -> JsResult<T>,
) -> JsResult<T> {
    with_page(agent, |agent, page| {
        read_node(agent, source, |agent, wrappers, document, id, _| {
            let Ok(mut index) = page.child_index.try_borrow_mut() else {
                return agent.type_error("DOM child index is busy");
            };
            agent.check_native_work()?;
            index.rebuild(agent, document, id)?;
            let children = match kind {
                ChildKind::Nodes => &index.nodes,
                ChildKind::Elements => &index.elements,
            };
            visit(agent, wrappers, document, children)
        })
    })
}

pub(super) fn child_len(agent: &mut Agent, source: Value, kind: ChildKind) -> JsResult<u32> {
    with_children(agent, source, kind, |_, _, _, nodes| {
        u32::try_from(nodes.len()).map_err(|_| OutOfMemory.into())
    })
}

pub(super) fn child_at(
    agent: &mut Agent,
    source: Value,
    kind: ChildKind,
    index: u32,
) -> JsResult<Option<Value>> {
    with_children(agent, source, kind, |agent, wrappers, document, nodes| {
        nodes
            .get(index as usize)
            .map(|&id| wrap(wrappers, agent, document, id))
            .transpose()
    })
}

fn child_count(agent: &mut Agent, source: Value) -> JsResult<u32> {
    child_len(agent, source, ChildKind::Nodes)
}

fn child_item(agent: &mut Agent, source: Value, index: u32) -> JsResult<Option<Value>> {
    child_at(agent, source, ChildKind::Nodes, index)
}
