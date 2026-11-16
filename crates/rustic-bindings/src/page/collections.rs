//! Static and live NodeList objects.

use rustic_dom::{Document, NodeId};
use rustic_js::runtime::{
    Agent, JsResult, LiveIndexAccess, ObjectId, OutOfMemory, PropertyDescriptor, PropertyKey, Value,
};

use crate::dom_exception;
use crate::webidl::{IntegerConversion, to_unsigned_long};

use super::{
    DomHeap, illegal_constructor, install_interface, prototype, read_node, required_arg, with_page,
    wrap,
};

const NODE_LIST: &str = "NodeList";

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

fn receiver(agent: &mut Agent, value: Value) -> JsResult<ObjectId> {
    match value.as_object() {
        Some(object) if agent.has_indexed_brand(object, NODE_LIST) => Ok(object),
        _ => agent.type_error("expected a NodeList receiver"),
    }
}

fn length(agent: &mut Agent, value: Value, _: &[Value]) -> JsResult<Value> {
    let object = receiver(agent, value)?;
    Ok(Value::number(f64::from(agent.indexed_host_len(object)?)))
}

fn item(agent: &mut Agent, value: Value, args: &[Value]) -> JsResult<Value> {
    let object = receiver(agent, value)?;
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

static CHILD_INDEX: LiveIndexAccess = LiveIndexAccess {
    length: child_count,
    item: child_item,
};

pub(super) fn child_nodes(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        read_node(agent, receiver, |agent, wrappers, document, id, _| {
            let key = (id, agent.current_realm());
            let Ok(mut cache) = page.child_lists.try_borrow_mut() else {
                return agent.type_error("DOM collections are busy");
            };
            if let Some(&object) = cache.get(&key) {
                return Ok(Value::object(object));
            }
            cache.try_reserve(1).map_err(|_| OutOfMemory)?;
            let proto = prototype(agent, NODE_LIST)?;
            let object = agent.new_indexed_view(NODE_LIST, proto, receiver, &CHILD_INDEX)?;
            let result = Value::object(object);
            wrappers
                .add_edge(document, id, result)
                .map_err(|_| OutOfMemory)?;
            cache.insert(key, object);
            Ok(result)
        })
    })
}

fn child_count(agent: &mut Agent, source: Value) -> JsResult<u32> {
    read_node(agent, source, |agent, _, document, id, _| {
        let children = document
            .children(id)
            .map_err(|e| dom_exception::from_dom_error(agent, e))?;
        let mut count = 0_u32;
        for _ in children {
            agent.check_native_work()?;
            count = count.checked_add(1).ok_or(OutOfMemory)?;
        }
        Ok(count)
    })
}

fn child_item(agent: &mut Agent, source: Value, index: u32) -> JsResult<Option<Value>> {
    read_node(agent, source, |agent, wrappers, document, id, _| {
        let children = document
            .children(id)
            .map_err(|e| dom_exception::from_dom_error(agent, e))?;
        for (i, id) in children.enumerate() {
            agent.check_native_work()?;
            if i == index as usize {
                return wrap(wrappers, agent, document, id).map(Some);
            }
        }
        Ok(None)
    })
}
