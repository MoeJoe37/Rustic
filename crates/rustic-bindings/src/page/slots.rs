//! HTML slot APIs and the slot signals delivered with mutation observers.

use std::cell::RefCell;
use std::collections::HashMap;

use rustic_dom::{Document, DocumentId, DomError, NodeId, NodeKind, QualifiedName, ShadowRootMode};
use rustic_js::runtime::{Agent, JsResult, Value};

use super::events::{PrivateArray, define_method, with_root};
use super::traversal::node_of;
use super::{
    Page, creation, dom_exception, illegal_constructor, install_interface, read_node,
    with_mut_tree, wrap,
};

pub(super) const INTERFACE: &str = "HTMLSlotElement";

#[derive(Debug, Default, Eq, PartialEq)]
struct Contents {
    assigned: bool,
    nodes: Vec<NodeId>,
}

type SlotContents = (NodeId, Contents);
type Snapshot = (Option<u64>, Vec<SlotContents>);

#[derive(Debug, Default)]
pub(super) struct Signals(RefCell<HashMap<DocumentId, Snapshot>>);

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        INTERFACE,
        Some("HTMLElement"),
        illegal_constructor,
        0,
        &[],
        |agent, proto| {
            super::elements::accessor(agent, proto, "name", name, Some(set_name))?;
            define_method(agent, proto, "assignedNodes", assigned_nodes, 0)?;
            define_method(agent, proto, "assignedElements", assigned_elements, 0)?;
            define_method(agent, proto, "assign", assign, 0)
        },
    )
}

pub(super) fn assigned_slot(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, wrappers, doc, id, node| {
        if !matches!(
            node.kind(),
            NodeKind::Element(_) | NodeKind::Text(_) | NodeKind::CdataSection(_)
        ) {
            return agent.type_error("expected Element or Text");
        }
        let map = doc
            .slot_map()
            .map_err(|err| dom_exception::from_dom_error(agent, err))?;
        let Some(slot) = map.assigned_slot(id) else {
            return Ok(Value::NULL);
        };
        let host = super::ranges::root_of(doc, slot);
        let open = doc
            .node(host)
            .ok()
            .and_then(|root| root.shadow_host())
            .is_some_and(|host| {
                matches!(doc.node(host).unwrap().kind(), NodeKind::Element(element)
                if element.shadow_root_mode() == Some(ShadowRootMode::Open))
            });
        if open {
            wrap(wrappers, agent, doc, slot)
        } else {
            Ok(Value::NULL)
        }
    })
}

fn name(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, doc, id, node| {
        if !doc.is_slot(id) {
            return agent.type_error("expected HTMLSlotElement");
        }
        let NodeKind::Element(element) = node.kind() else {
            unreachable!()
        };
        match element
            .attributes()
            .iter()
            .find(|attr| attr.name().namespace().is_none() && attr.name().local() == "name")
        {
            Some(attr) => crate::webidl::from_dom_string(agent, attr.value()),
            None => agent.string(""),
        }
    })
}

fn set_name(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let name = creation::string(agent, args.first().copied().unwrap_or(Value::UNDEFINED))?;
    with_mut_tree(agent, receiver, |agent, doc, _, slot| {
        if !doc.is_slot(slot) {
            return agent.type_error("expected HTMLSlotElement");
        }
        doc.set_attribute(
            slot,
            QualifiedName::from_parsed(None, None, "name").expect("literal name"),
            name,
        )
        .map_err(|err| dom_exception::from_dom_error(agent, err))?;
        Ok(Value::UNDEFINED)
    })
}

fn assign(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let ids = args
        .iter()
        .map(|&arg| node_of(agent, arg))
        .collect::<JsResult<Vec<_>>>()?;
    with_mut_tree(agent, receiver, |agent, doc, _, slot| {
        doc.assign_slot(slot, &ids).map_err(|err| match err {
            DomError::WrongNodeKind => agent.throw(
                rustic_js::runtime::ErrorKind::TypeError,
                "expected a slot and Element or Text arguments",
            ),
            _ => dom_exception::from_dom_error(agent, err),
        })?;
        Ok(Value::UNDEFINED)
    })
}

fn assigned_nodes(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    assigned(agent, receiver, args, false)
}

fn assigned_elements(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    assigned(agent, receiver, args, true)
}

fn assigned(agent: &mut Agent, receiver: Value, args: &[Value], elements: bool) -> JsResult<Value> {
    let flatten = if let Some(options) = args.first().and_then(|value| value.as_object()) {
        let key = agent.heap.key("flatten")?;
        let value = agent.get(options, key)?;
        agent.to_boolean(value)
    } else {
        false
    };
    read_node(agent, receiver, |agent, wrappers, doc, slot, _| {
        if !doc.is_slot(slot) {
            return agent.type_error("expected HTMLSlotElement");
        }
        let map = doc
            .slot_map()
            .map_err(|err| dom_exception::from_dom_error(agent, err))?;
        let flattened;
        let nodes = if flatten {
            flattened = map
                .flattened_nodes(slot)
                .map_err(|err| dom_exception::from_dom_error(agent, err))?;
            flattened.as_slice()
        } else {
            map.assigned_nodes(slot)
        };
        let holder = PrivateArray::new(agent, &[])?;
        with_root(agent, holder.value(), |agent| {
            let mut items = Vec::new();
            for &node in nodes {
                agent.check_native_work()?;
                if !elements
                    || matches!(
                        doc.node(node).expect("live assigned node").kind(),
                        NodeKind::Element(_)
                    )
                {
                    let value = wrap(wrappers, agent, doc, node)?;
                    let at = holder.len(agent)?;
                    holder.set(agent, at, value)?;
                    items.push(value);
                }
            }
            agent.create_array_from_list(&items).map(Value::object)
        })
    })
}

fn changed(
    doc: &Document,
    cache: &mut HashMap<DocumentId, Snapshot>,
) -> Result<Vec<NodeId>, DomError> {
    let previous = cache.get(&doc.id());
    if !doc.has_shadow_roots() && previous.is_none() {
        return Ok(Vec::new());
    }
    let revision = doc.mutation_revision();
    if revision.is_some() && previous.is_some_and(|(old, _)| *old == revision) {
        return Ok(Vec::new());
    }
    let map = doc.slot_map()?;
    let mut current = Vec::new();
    let mut signals = Vec::new();
    let mut before = previous
        .into_iter()
        .flat_map(|(_, slots)| slots.iter())
        .map(|(slot, nodes)| (*slot, nodes))
        .collect::<HashMap<_, _>>();
    for &slot in map.slots() {
        let assigned = map.assigned_nodes(slot);
        let contents = if assigned.is_empty() {
            Contents {
                assigned: false,
                nodes: doc.children(slot)?.collect(),
            }
        } else {
            Contents {
                assigned: true,
                nodes: assigned.to_vec(),
            }
        };
        let old = before.remove(&slot);
        if old.map_or(!contents.nodes.is_empty(), |old| old != &contents) {
            signals.push(slot);
        }
        current.push((slot, contents));
    }
    if let Some((_, slots)) = previous {
        signals.extend(slots.iter().filter_map(|(slot, contents)| {
            (contents.assigned && before.contains_key(slot) && doc.is_slot(*slot)).then_some(*slot)
        }));
    }
    if doc.has_shadow_roots() {
        cache.insert(doc.id(), (revision, current));
    } else {
        cache.remove(&doc.id());
    }
    Ok(signals)
}

pub(super) fn flush(agent: &mut Agent, page: &Page) -> JsResult<()> {
    let Some(main) = page.tree.read() else {
        return Ok(());
    };
    let Ok(additional) = page.additional.try_borrow() else {
        return Ok(());
    };
    let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
        return Ok(());
    };
    let mut cache = page.slot_signals.0.borrow_mut();
    cache.retain(|id, _| *id == main.id() || additional.contains_key(id));
    let mut process = |doc: &Document, agent: &mut Agent| -> JsResult<()> {
        for slot in
            changed(doc, &mut cache).map_err(|err| dom_exception::from_dom_error(agent, err))?
        {
            let value = wrap(&mut wrappers, agent, doc, slot)?;
            super::mutation_observer::signal_slot(agent, page, value)?;
        }
        Ok(())
    };
    process(&main, agent)?;
    for cell in additional.values() {
        if let Ok(doc) = cell.tree.try_borrow() {
            process(&doc, agent)?;
        }
    }
    Ok(())
}
