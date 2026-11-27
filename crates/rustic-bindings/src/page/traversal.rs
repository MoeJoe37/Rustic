//! DOM §6 traversal: `NodeIterator`, `TreeWalker` and `NodeFilter`.
//!
//! Both traversers keep their root, `whatToShow` and filter in private
//! slots. A `TreeWalker`'s current node is a slot holding the node's
//! wrapper; a `NodeIterator`'s reference lives in its document
//! ([`rustic_dom::Document::create_node_iterator`]) so that removals move it
//! as the DOM's pre-removing steps say. Filters run with no tree borrowed.

use rustic_dom::{Document, LiveId, Node, NodeId, NodeKind};
use rustic_js::runtime::{Agent, JsResult, PropertyDescriptor, Value};

use super::events::{PrivateArray, define_method, with_root};
use super::{
    WrapperKind, creation, identity, illegal_constructor, install_interface, prototype, with_page,
    wrap,
};

const NODE_ITERATOR: &str = "NodeIterator";
const TREE_WALKER: &str = "TreeWalker";

const FILTER_ACCEPT: u16 = 1;
const FILTER_REJECT: u16 = 2;
const FILTER_SKIP: u16 = 3;

/// Shared slots: the root's wrapper, `whatToShow`, the filter, the active flag.
const S_ROOT: u32 = 0;
const S_SHOW: u32 = 1;
const S_FILTER: u32 = 2;
const S_ACTIVE: u32 = 3;
/// `TreeWalker`: the current node's wrapper. `NodeIterator`: the document's
/// iterator id.
const S_CURRENT: u32 = 4;

const NODE_FILTER_CONSTANTS: [(&str, f64); 16] = [
    ("FILTER_ACCEPT", 1.0),
    ("FILTER_REJECT", 2.0),
    ("FILTER_SKIP", 3.0),
    ("SHOW_ALL", 4_294_967_295.0),
    ("SHOW_ELEMENT", 1.0),
    ("SHOW_ATTRIBUTE", 2.0),
    ("SHOW_TEXT", 4.0),
    ("SHOW_CDATA_SECTION", 8.0),
    ("SHOW_ENTITY_REFERENCE", 16.0),
    ("SHOW_ENTITY", 32.0),
    ("SHOW_PROCESSING_INSTRUCTION", 64.0),
    ("SHOW_COMMENT", 128.0),
    ("SHOW_DOCUMENT", 256.0),
    ("SHOW_DOCUMENT_TYPE", 512.0),
    ("SHOW_DOCUMENT_FRAGMENT", 1024.0),
    ("SHOW_NOTATION", 2048.0),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let shared = [
        (
            "root",
            "get root",
            slot::<S_ROOT> as rustic_js::runtime::NativeFn,
        ),
        ("whatToShow", "get whatToShow", slot::<S_SHOW>),
        ("filter", "get filter", slot::<S_FILTER>),
    ];
    let mut iterator = shared.to_vec();
    iterator.push(("referenceNode", "get referenceNode", reference_node));
    iterator.push((
        "pointerBeforeReferenceNode",
        "get pointerBeforeReferenceNode",
        pointer_before,
    ));
    install_interface(
        agent,
        NODE_ITERATOR,
        None,
        illegal_constructor,
        0,
        &iterator,
        |agent, proto| {
            define_method(agent, proto, "nextNode", iterator_next, 0)?;
            define_method(agent, proto, "previousNode", iterator_previous, 0)?;
            define_method(agent, proto, "detach", detach, 0)
        },
    )?;
    install_interface(
        agent,
        TREE_WALKER,
        None,
        illegal_constructor,
        0,
        &shared,
        |agent, proto| {
            let get =
                Value::object(agent.new_native_function(current_node, "get currentNode", 0)?);
            let set =
                Value::object(agent.new_native_function(set_current_node, "set currentNode", 1)?);
            let key = agent.heap.key("currentNode")?;
            agent.define_property_or_throw(
                proto,
                key,
                PropertyDescriptor::accessor(get, set, true, true),
            )?;
            for (name, method) in [
                ("parentNode", walker_parent as rustic_js::runtime::NativeFn),
                ("firstChild", walker_first_child),
                ("lastChild", walker_last_child),
                ("previousSibling", walker_previous_sibling),
                ("nextSibling", walker_next_sibling),
                ("previousNode", walker_previous_node),
                ("nextNode", walker_next_node),
            ] {
                define_method(agent, proto, name, method, 0)?;
            }
            Ok(())
        },
    )?;
    // `NodeFilter` is a callback interface with constants: a plain object.
    let filter = agent.new_object()?;
    for (name, value) in NODE_FILTER_CONSTANTS {
        let key = agent.heap.key(name)?;
        agent.define_property_or_throw(
            filter,
            key,
            PropertyDescriptor::data_with(Value::number(value), false, true, false),
        )?;
    }
    let global = agent.global_this();
    let key = agent.heap.key("NodeFilter")?;
    agent.define_property_or_throw(
        global,
        key,
        PropertyDescriptor::data_with(Value::object(filter), true, false, true),
    )?;
    let document = prototype(agent, "Document")?;
    define_method(agent, document, "createNodeIterator", create_iterator, 1)?;
    define_method(agent, document, "createTreeWalker", create_walker, 1)
}

/// The node behind a wrapper value.
pub(super) fn node_of(agent: &mut Agent, value: Value) -> JsResult<NodeId> {
    with_page(agent, |agent, page| {
        let Ok(wrappers) = page.wrappers.try_borrow() else {
            return agent.type_error("DOM bindings are busy");
        };
        identity(agent, &wrappers, value, WrapperKind::Node)
    })
}

/// The wrapper of a node.
pub(super) fn wrap_node(agent: &mut Agent, id: NodeId) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return agent.type_error("DOM bindings are busy");
        };
        page.read(agent, id.document(), |agent, document| {
            wrap(&mut wrappers, agent, document, id)
        })
    })
}

/// Read the document holding `id`.
pub(super) fn read_doc<T>(
    agent: &mut Agent,
    id: NodeId,
    read: impl FnOnce(&Document) -> T,
) -> JsResult<T> {
    with_page(agent, |agent, page| {
        page.read(agent, id.document(), |_, document| Ok(read(document)))
    })
}

fn relation(
    agent: &mut Agent,
    id: NodeId,
    step: impl FnOnce(&Node) -> Option<NodeId>,
) -> JsResult<Option<NodeId>> {
    read_doc(agent, id, |document| document.node(id).ok().and_then(step))
}

fn node_type(kind: &NodeKind) -> u32 {
    match kind {
        NodeKind::Element(_) => 1,
        NodeKind::Text(_) => 3,
        NodeKind::CdataSection(_) => 4,
        NodeKind::ProcessingInstruction(_) => 7,
        NodeKind::Comment(_) => 8,
        NodeKind::Document => 9,
        NodeKind::DocumentType(_) => 10,
        NodeKind::DocumentFragment => 11,
    }
}

fn exception(agent: &mut Agent, name: &str, message: &str) -> rustic_js::runtime::Abrupt {
    creation::exception(agent, name, message)
}

fn slots_of(agent: &mut Agent, this: Value, interface: &'static str) -> JsResult<PrivateArray> {
    let slots = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.traversers, this))
    })?;
    let ok = match PrivateArray::from_value(slots) {
        Some(slots) => {
            let current = slots.get(agent, S_CURRENT)?;
            // Iterators keep a number (their id), walkers a node wrapper.
            (current.as_number().is_some()) == (interface == NODE_ITERATOR)
        }
        None => false,
    };
    match PrivateArray::from_value(slots) {
        Some(slots) if ok => Ok(slots),
        _ => agent.type_error(if interface == NODE_ITERATOR {
            "expected a NodeIterator receiver"
        } else {
            "expected a TreeWalker receiver"
        }),
    }
}

fn slot<const SLOT: u32>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let slots = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.traversers, this))
    })?;
    match PrivateArray::from_value(slots) {
        Some(slots) => slots.get(agent, SLOT),
        None => agent.type_error("expected a NodeIterator or TreeWalker receiver"),
    }
}

/// DOM "filter": `whatToShow`, then the filter callback.
fn filter(agent: &mut Agent, slots: PrivateArray, node: NodeId) -> JsResult<u16> {
    if slots.get(agent, S_ACTIVE)? == Value::boolean(true) {
        return Err(exception(
            agent,
            "InvalidStateError",
            "the filter is running",
        ));
    }
    let kind = read_doc(agent, node, |document| {
        document
            .node(node)
            .map(|n| node_type(n.kind()))
            .unwrap_or(1)
    })?;
    let show = slots.get(agent, S_SHOW)?.as_number().unwrap_or(0.0) as u32;
    if show & (1 << (kind - 1)) == 0 {
        return Ok(FILTER_SKIP);
    }
    let callback = slots.get(agent, S_FILTER)?;
    if callback == Value::NULL {
        return Ok(FILTER_ACCEPT);
    }
    let wrapper = wrap_node(agent, node)?;
    slots.set(agent, S_ACTIVE, Value::boolean(true))?;
    let result = (|| {
        let value = if agent.is_callable(callback) {
            agent.call(callback, Value::UNDEFINED, &[wrapper])?
        } else {
            let Some(object) = callback.as_object() else {
                return agent.type_error("NodeFilter must be an object");
            };
            let key = agent.heap.key("acceptNode")?;
            let method = agent.get(object, key)?;
            if !agent.is_callable(method) {
                return agent.type_error("NodeFilter has no acceptNode method");
            }
            agent.call(method, callback, &[wrapper])?
        };
        let number = agent.to_number(value)?;
        // Web IDL `unsigned short`.
        let integer = if number.is_finite() {
            number.trunc()
        } else {
            0.0
        };
        Ok(integer.rem_euclid(65_536.0) as u16)
    })();
    slots.set(agent, S_ACTIVE, Value::boolean(false))?;
    result
}

fn traverser(
    agent: &mut Agent,
    args: &[Value],
    interface: &'static str,
) -> JsResult<(Value, PrivateArray, NodeId)> {
    let root = super::required_arg(agent, args, 0)?;
    let root_id = node_of(agent, root)?;
    let show = match args.get(1).copied() {
        None | Some(Value::UNDEFINED) => 4_294_967_295.0,
        Some(value) => {
            // Web IDL `unsigned long`.
            let number = agent.to_number(value)?;
            if number.is_finite() {
                number.trunc().rem_euclid(4_294_967_296.0)
            } else {
                0.0
            }
        }
    };
    let filter = match args.get(2).copied() {
        None | Some(Value::UNDEFINED) | Some(Value::NULL) => Value::NULL,
        Some(value) if value.as_object().is_some() => value,
        Some(_) => return agent.type_error("NodeFilter must be an object"),
    };
    let proto = prototype(agent, interface)?;
    let object = Value::object(agent.new_object_with(Some(proto))?);
    let slots = PrivateArray::new(
        agent,
        &[
            root,
            Value::number(show),
            filter,
            Value::boolean(false),
            root,
        ],
    )?;
    with_page(agent, |agent, page| {
        agent.weak_map_set(page.traversers, object, slots.value())
    })?;
    Ok((object, slots, root_id))
}

fn create_walker(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    Ok(traverser(agent, args, TREE_WALKER)?.0)
}

fn create_iterator(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let (object, slots, root) = traverser(agent, args, NODE_ITERATOR)?;
    with_root(agent, object, |agent| {
        let id = with_page(agent, |agent, page| {
            page.write(agent, root.document(), |agent, document| {
                document
                    .create_node_iterator(root)
                    .map_err(|error| super::dom_exception::from_dom_error(agent, error))
            })
        })?;
        slots.set(agent, S_CURRENT, Value::number(id.get() as f64))?;
        super::ranges::track(agent, object, root.document(), id)?;
        Ok(object)
    })
}

// NodeIterator.

fn iterator_state(
    agent: &mut Agent,
    this: Value,
) -> JsResult<(PrivateArray, NodeId, LiveId, rustic_dom::IteratorState)> {
    let slots = slots_of(agent, this, NODE_ITERATOR)?;
    let root = slots.get(agent, S_ROOT)?;
    let root = node_of(agent, root)?;
    let raw = slots.get(agent, S_CURRENT)?.as_number().unwrap_or(0.0) as u64;
    let found = read_doc(agent, root, |document| {
        let id = LiveId::from_raw(raw);
        document.node_iterator(id).map(|state| (id, state))
    })?;
    match found {
        Some((id, state)) => Ok((slots, root, id, state)),
        None => agent.type_error("the NodeIterator's document is gone"),
    }
}

fn reference_node(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, _, _, state) = iterator_state(agent, this)?;
    wrap_node(agent, state.reference)
}

fn pointer_before(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, _, _, state) = iterator_state(agent, this)?;
    Ok(Value::boolean(state.pointer_before_reference))
}

fn detach(_: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::UNDEFINED)
}

/// The node after `node` in tree order inside `root`.
fn following(document: &Document, root: NodeId, node: NodeId) -> Option<NodeId> {
    let current = document.node(node).ok()?;
    if let Some(child) = current.first_child() {
        return Some(child);
    }
    let mut at = node;
    loop {
        if at == root {
            return None;
        }
        let here = document.node(at).ok()?;
        if let Some(sibling) = here.next_sibling() {
            return Some(sibling);
        }
        at = here.parent()?;
    }
}

/// The node before `node` in tree order inside `root`.
fn preceding(document: &Document, root: NodeId, node: NodeId) -> Option<NodeId> {
    if node == root {
        return None;
    }
    let current = document.node(node).ok()?;
    match current.previous_sibling() {
        Some(mut last) => {
            while let Some(child) = document.node(last).ok()?.last_child() {
                last = child;
            }
            Some(last)
        }
        None => current.parent(),
    }
}

fn iterator_traverse(agent: &mut Agent, this: Value, next: bool) -> JsResult<Value> {
    let (slots, root, id, state) = iterator_state(agent, this)?;
    let mut node = state.reference;
    let mut before = state.pointer_before_reference;
    loop {
        if next {
            if before {
                before = false;
            } else {
                match read_doc(agent, root, |d| following(d, root, node))? {
                    Some(n) => node = n,
                    None => return Ok(Value::NULL),
                }
            }
        } else if !before {
            before = true;
        } else {
            match read_doc(agent, root, |d| preceding(d, root, node))? {
                Some(n) => node = n,
                None => return Ok(Value::NULL),
            }
        }
        if filter(agent, slots, node)? == FILTER_ACCEPT {
            break;
        }
    }
    with_page(agent, |agent, page| {
        page.write(agent, root.document(), |agent, document| {
            document
                .set_node_iterator(id, node, before)
                .map_err(|error| super::dom_exception::from_dom_error(agent, error))
        })
    })?;
    wrap_node(agent, node)
}

fn iterator_next(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    iterator_traverse(agent, this, true)
}

fn iterator_previous(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    iterator_traverse(agent, this, false)
}

// TreeWalker.

fn walker(agent: &mut Agent, this: Value) -> JsResult<(PrivateArray, NodeId, NodeId)> {
    let slots = slots_of(agent, this, TREE_WALKER)?;
    let root = slots.get(agent, S_ROOT)?;
    let root = node_of(agent, root)?;
    let current = slots.get(agent, S_CURRENT)?;
    let current = node_of(agent, current)?;
    Ok((slots, root, current))
}

fn accept(agent: &mut Agent, slots: PrivateArray, node: NodeId) -> JsResult<Value> {
    let wrapper = wrap_node(agent, node)?;
    slots.set(agent, S_CURRENT, wrapper)?;
    Ok(wrapper)
}

fn current_node(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    slots_of(agent, this, TREE_WALKER)?.get(agent, S_CURRENT)
}

fn set_current_node(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let slots = slots_of(agent, this, TREE_WALKER)?;
    let value = super::required_arg(agent, args, 0)?;
    node_of(agent, value)?;
    slots.set(agent, S_CURRENT, value)?;
    Ok(Value::UNDEFINED)
}

fn walker_parent(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (slots, root, mut node) = walker(agent, this)?;
    while node != root {
        let Some(parent) = relation(agent, node, Node::parent)? else {
            break;
        };
        node = parent;
        if filter(agent, slots, node)? == FILTER_ACCEPT {
            return accept(agent, slots, node);
        }
    }
    Ok(Value::NULL)
}

fn traverse_children(agent: &mut Agent, this: Value, first: bool) -> JsResult<Value> {
    let (slots, root, current) = walker(agent, this)?;
    let child = |node: &Node| {
        if first {
            node.first_child()
        } else {
            node.last_child()
        }
    };
    let sibling = |node: &Node| {
        if first {
            node.next_sibling()
        } else {
            node.previous_sibling()
        }
    };
    let mut next = relation(agent, current, child)?;
    while let Some(mut node) = next {
        let result = filter(agent, slots, node)?;
        if result == FILTER_ACCEPT {
            return accept(agent, slots, node);
        }
        if result == FILTER_SKIP {
            if let Some(inner) = relation(agent, node, child)? {
                next = Some(inner);
                continue;
            }
        }
        next = loop {
            if let Some(s) = relation(agent, node, sibling)? {
                break Some(s);
            }
            match relation(agent, node, Node::parent)? {
                Some(parent) if parent != root && parent != current => node = parent,
                _ => return Ok(Value::NULL),
            }
        };
    }
    Ok(Value::NULL)
}

fn walker_first_child(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    traverse_children(agent, this, true)
}

fn walker_last_child(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    traverse_children(agent, this, false)
}

fn traverse_siblings(agent: &mut Agent, this: Value, next: bool) -> JsResult<Value> {
    let (slots, root, mut node) = walker(agent, this)?;
    if node == root {
        return Ok(Value::NULL);
    }
    let sibling = |n: &Node| {
        if next {
            n.next_sibling()
        } else {
            n.previous_sibling()
        }
    };
    let child = |n: &Node| {
        if next {
            n.first_child()
        } else {
            n.last_child()
        }
    };
    loop {
        let mut candidate = relation(agent, node, sibling)?;
        while let Some(s) = candidate {
            node = s;
            let result = filter(agent, slots, node)?;
            if result == FILTER_ACCEPT {
                return accept(agent, slots, node);
            }
            candidate = relation(agent, node, child)?;
            if result == FILTER_REJECT || candidate.is_none() {
                candidate = relation(agent, node, sibling)?;
            }
        }
        match relation(agent, node, Node::parent)? {
            Some(parent) if parent != root => node = parent,
            _ => return Ok(Value::NULL),
        }
        if filter(agent, slots, node)? == FILTER_ACCEPT {
            return Ok(Value::NULL);
        }
    }
}

fn walker_next_sibling(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    traverse_siblings(agent, this, true)
}

fn walker_previous_sibling(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    traverse_siblings(agent, this, false)
}

fn walker_previous_node(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (slots, root, mut node) = walker(agent, this)?;
    while node != root {
        let mut sibling = relation(agent, node, Node::previous_sibling)?;
        while let Some(s) = sibling {
            node = s;
            let mut result = filter(agent, slots, node)?;
            while result != FILTER_REJECT {
                let Some(last) = relation(agent, node, Node::last_child)? else {
                    break;
                };
                node = last;
                result = filter(agent, slots, node)?;
            }
            if result == FILTER_ACCEPT {
                return accept(agent, slots, node);
            }
            sibling = relation(agent, node, Node::previous_sibling)?;
        }
        let parent = relation(agent, node, Node::parent)?;
        let Some(parent) = parent.filter(|_| node != root) else {
            return Ok(Value::NULL);
        };
        node = parent;
        if filter(agent, slots, node)? == FILTER_ACCEPT {
            return accept(agent, slots, node);
        }
    }
    Ok(Value::NULL)
}

fn walker_next_node(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (slots, root, mut node) = walker(agent, this)?;
    let mut result = FILTER_ACCEPT;
    loop {
        while result != FILTER_REJECT {
            let Some(child) = relation(agent, node, Node::first_child)? else {
                break;
            };
            node = child;
            result = filter(agent, slots, node)?;
            if result == FILTER_ACCEPT {
                return accept(agent, slots, node);
            }
        }
        let mut temporary = Some(node);
        let mut found = false;
        while let Some(t) = temporary {
            if t == root {
                return Ok(Value::NULL);
            }
            if let Some(sibling) = relation(agent, t, Node::next_sibling)? {
                node = sibling;
                found = true;
                break;
            }
            temporary = relation(agent, t, Node::parent)?;
        }
        if !found {
            return Ok(Value::NULL);
        }
        result = filter(agent, slots, node)?;
        if result == FILTER_ACCEPT {
            return accept(agent, slots, node);
        }
    }
}
