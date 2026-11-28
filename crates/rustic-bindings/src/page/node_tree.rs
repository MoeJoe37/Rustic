//! `Node` tree members beyond the first slice ([DOM §4.4]): names and
//! values, more relatives, connectedness, containment, position comparison,
//! `replaceChild` and `normalize`.
//!
//! [DOM §4.4]: https://dom.spec.whatwg.org/#interface-node

use rustic_dom::{Document, DomError, DomString, Node, NodeId, NodeKind};
use rustic_js::runtime::{
    Agent, JsResult, NativeFn, ObjectId, OutOfMemory, PropertyDescriptor, Value,
};

use crate::dom_exception;
use crate::heap::WrapperKind;
use crate::webidl::{DomStringConversion, from_dom_string, to_dom_string};

use super::{append_error, identity, read_node, related, required_arg, with_mut_tree, wrap};

pub(super) fn install(agent: &mut Agent, prototype: ObjectId) -> JsResult<()> {
    for (name, getter, setter) in [
        ("nodeName", node_name as NativeFn, None),
        ("nodeValue", node_value, Some(set_node_value as NativeFn)),
        ("parentElement", parent_element, None),
        ("lastChild", last_child, None),
        ("previousSibling", previous_sibling, None),
        ("isConnected", is_connected, None),
    ] {
        let get = Value::object(agent.new_native_function(getter, &format!("get {name}"), 0)?);
        let set = match setter {
            Some(setter) => {
                Value::object(agent.new_native_function(setter, &format!("set {name}"), 1)?)
            }
            None => Value::UNDEFINED,
        };
        let key = agent.heap.key(name)?;
        agent.define_property_or_throw(
            prototype,
            key,
            PropertyDescriptor::accessor(get, set, true, true),
        )?;
    }
    for (name, method, length) in [
        ("hasChildNodes", has_child_nodes as NativeFn, 0),
        ("getRootNode", get_root_node, 0),
        ("contains", contains, 1),
        ("isSameNode", is_same_node, 1),
        ("compareDocumentPosition", compare_document_position, 1),
        ("replaceChild", replace_child, 2),
        ("normalize", normalize, 0),
    ] {
        let key = agent.heap.key(name)?;
        agent.define_method(prototype, key, method, name, length)?;
    }
    Ok(())
}

fn node_name(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let kind = read_node(agent, receiver, |agent, _, _, _, node| {
        let fixed = match node.kind() {
            NodeKind::Element(_) => return Ok(None),
            NodeKind::Text(_) => "#text",
            NodeKind::CdataSection(_) => "#cdata-section",
            NodeKind::Comment(_) => "#comment",
            NodeKind::Document => "#document",
            NodeKind::DocumentFragment => "#document-fragment",
            NodeKind::ProcessingInstruction(instruction) => {
                return from_dom_string(agent, instruction.target()).map(Some);
            }
            NodeKind::DocumentType(doctype) => {
                return from_dom_string(agent, &doctype.name).map(Some);
            }
        };
        agent.string(fixed).map(Some)
    })?;
    match kind {
        Some(name) => Ok(name),
        // An element's nodeName is its HTML-uppercased qualified name.
        None => super::creation::tag_name(agent, receiver, args),
    }
}

fn node_value(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, document, id, node| {
        match node.kind() {
            NodeKind::Text(_)
            | NodeKind::Comment(_)
            | NodeKind::CdataSection(_)
            | NodeKind::ProcessingInstruction(_) => {
                let data = document
                    .character_data(id)
                    .map_err(|error| dom_exception::from_dom_error(agent, error))?;
                from_dom_string(agent, data)
            }
            _ => Ok(Value::NULL),
        }
    })
}

fn set_node_value(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    // `DOMString?`: null becomes the empty string for character data.
    let value = if value == Value::NULL {
        DomString::default()
    } else {
        to_dom_string(agent, value, DomStringConversion::Default)?
    };
    with_mut_tree(agent, receiver, |agent, document, _, id| {
        let character = matches!(
            document.node(id).map(Node::kind),
            Ok(NodeKind::Text(_)
                | NodeKind::Comment(_)
                | NodeKind::CdataSection(_)
                | NodeKind::ProcessingInstruction(_))
        );
        if character {
            document
                .set_character_data(id, value)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        }
        Ok(Value::UNDEFINED)
    })
}

fn parent_element(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    related(agent, receiver, |document, node| {
        node.parent().filter(|&parent| {
            matches!(
                document.node(parent).map(Node::kind),
                Ok(NodeKind::Element(_))
            )
        })
    })
}

fn last_child(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    related(agent, receiver, |_, node| node.last_child())
}

fn previous_sibling(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    related(agent, receiver, |_, node| node.previous_sibling())
}

fn has_child_nodes(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |_, _, _, _, node| {
        Ok(Value::boolean(node.first_child().is_some()))
    })
}

/// The root of the tree `id` is in, walking parents only (template contents
/// and shadow roots are roots of their own trees).
fn root_of(agent: &mut Agent, document: &Document, id: NodeId, composed: bool) -> JsResult<NodeId> {
    let mut current = id;
    loop {
        let node = document
            .node(current)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        let parent = node
            .parent()
            .or_else(|| composed.then(|| node.shadow_host()).flatten());
        let Some(parent) = parent else {
            return Ok(current);
        };
        agent.check_native_work()?;
        current = parent;
    }
}

fn is_connected(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, document, id, _| {
        Ok(Value::boolean(
            root_of(agent, document, id, true)? == document.root(),
        ))
    })
}

fn get_root_node(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let composed = match args.first().and_then(|value| value.as_object()) {
        Some(options) => {
            let key = agent.heap.key("composed")?;
            let value = agent.get(options, key)?;
            agent.to_boolean(value)
        }
        None => false,
    };
    read_node(agent, receiver, |agent, wrappers, document, id, _| {
        let root = root_of(agent, document, id, composed)?;
        wrap(wrappers, agent, document, root)
    })
}

/// The other node argument of a `Node?` operation, if any.
fn other_node(agent: &mut Agent, value: Value) -> JsResult<Option<NodeId>> {
    if value == Value::NULL {
        return Ok(None);
    }
    super::with_page(agent, |agent, page| {
        let Ok(wrappers) = page.wrappers.try_borrow() else {
            return agent.type_error("DOM bindings are busy");
        };
        identity(agent, &wrappers, value, WrapperKind::Node).map(Some)
    })
}

/// `node`'s inclusive ancestors, nearest first.
fn ancestors(agent: &mut Agent, document: &Document, node: NodeId) -> JsResult<Vec<NodeId>> {
    let mut chain = Vec::new();
    let mut current = Some(node);
    while let Some(id) = current {
        agent.check_native_work()?;
        chain.try_reserve(1).map_err(|_| OutOfMemory)?;
        chain.push(id);
        current = document
            .node(id)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?
            .parent();
    }
    Ok(chain)
}

fn contains(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let other = required_arg(agent, args, 0)?;
    let Some(other) = other_node(agent, other)? else {
        return Ok(Value::FALSE);
    };
    read_node(agent, receiver, |agent, _, document, id, _| {
        if other.document() != id.document() {
            return Ok(Value::FALSE);
        }
        Ok(Value::boolean(
            ancestors(agent, document, other)?.contains(&id),
        ))
    })
}

fn is_same_node(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let other = required_arg(agent, args, 0)?;
    Ok(Value::boolean(
        other == receiver && other.as_object().is_some(),
    ))
}

const DISCONNECTED: u16 = 0x01;
const PRECEDING: u16 = 0x02;
const FOLLOWING: u16 = 0x04;
const CONTAINS: u16 = 0x08;
const CONTAINED_BY: u16 = 0x10;
const IMPLEMENTATION_SPECIFIC: u16 = 0x20;

fn compare_document_position(
    agent: &mut Agent,
    receiver: Value,
    args: &[Value],
) -> JsResult<Value> {
    let other = required_arg(agent, args, 0)?;
    let Some(other) = other_node(agent, other)? else {
        return agent.type_error("compareDocumentPosition requires a Node");
    };
    let position = read_node(agent, receiver, |agent, _, document, id, _| {
        if id == other {
            return Ok(0);
        }
        // Nodes of different documents never share a root. The order
        // between them only needs to be consistent: compare identities.
        let disconnected = |first_is_earlier: bool| {
            DISCONNECTED
                | IMPLEMENTATION_SPECIFIC
                | if first_is_earlier {
                    FOLLOWING
                } else {
                    PRECEDING
                }
        };
        if other.document() != id.document() {
            return Ok(disconnected(id < other));
        }
        let mine = ancestors(agent, document, id)?;
        let theirs = ancestors(agent, document, other)?;
        if mine.last() != theirs.last() {
            return Ok(disconnected(mine.last() < theirs.last()));
        }
        if mine.contains(&other) {
            return Ok(CONTAINS | PRECEDING);
        }
        if theirs.contains(&id) {
            return Ok(CONTAINED_BY | FOLLOWING);
        }
        // Below the deepest common ancestor, compare the two children on
        // each path in sibling order.
        let (mut a, mut b) = (mine.len(), theirs.len());
        while a > 0 && b > 0 && mine[a - 1] == theirs[b - 1] {
            a -= 1;
            b -= 1;
        }
        let (mine_child, their_child) = (mine[a - 1], theirs[b - 1]);
        let mut sibling = document
            .node(mine_child)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?
            .next_sibling();
        while let Some(next) = sibling {
            agent.check_native_work()?;
            if next == their_child {
                return Ok(FOLLOWING);
            }
            sibling = document
                .node(next)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?
                .next_sibling();
        }
        Ok(PRECEDING)
    })?;
    Ok(Value::number(f64::from(position)))
}

/// `parent.replaceChild(node, child)` for native callers.
pub(super) fn replace_child_value(
    agent: &mut Agent,
    parent: Value,
    node: Value,
    child: Value,
) -> JsResult<Value> {
    replace_child(agent, parent, &[node, child])
}

/// `replaceChild(node, child)`; [`rustic_dom::Document::replace_child`]
/// validates, replaces and records the DOM's mutation entries.
fn replace_child(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let node = required_arg(agent, args, 0)?;
    let child = required_arg(agent, args, 1)?;
    with_mut_tree(agent, receiver, |agent, document, wrappers, parent| {
        let replacement = identity(agent, wrappers, node, WrapperKind::Node)?;
        let old = identity(agent, wrappers, child, WrapperKind::Node)?;
        // Same-document replacement lets the DOM order its validity checks.
        let not_child = old.document() != parent.document()
            || document.node(old).ok().and_then(Node::parent) != Some(parent);
        if replacement.document() != parent.document() || old.document() != parent.document() {
            // Validity step 1 (the parent's kind) still precedes step 3.
            let container = matches!(
                document.node(parent).map(Node::kind),
                Ok(NodeKind::Document | NodeKind::Element(_) | NodeKind::DocumentFragment)
            );
            if !container {
                return Err(dom_exception::from_dom_error(
                    agent,
                    DomError::InvalidHierarchy,
                ));
            }
            if not_child {
                return Err(dom_exception::from_dom_error(
                    agent,
                    DomError::InvalidReference,
                ));
            }
            // Remove `child`, then adopt before its next sibling (as live
            // ranges observe); a rejected adoption puts `child` back. The
            // DOM's single record becomes two here (TD-153).
            let reference = document.node(old).ok().and_then(Node::next_sibling);
            document
                .detach(old)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
            if let Err(error) =
                super::insert_node(agent, document, wrappers, replacement, parent, reference)
            {
                document
                    .insert_before(parent, old, reference)
                    .map_err(|error| dom_exception::from_dom_error(agent, error))?;
                return Err(error);
            }
            return Ok(child);
        }
        document
            .replace_child(parent, replacement, old)
            .map_err(|error| append_error(agent, error))?;
        Ok(child)
    })
}

/// DOM `normalize()`: drop empty exclusive Text descendants and merge each
/// run of adjacent ones into its first node, which records one data change
/// and then one removal per merged node.
fn normalize(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    with_mut_tree(agent, receiver, |agent, document, _, root| {
        let mut texts = Vec::new();
        for (id, _) in document
            .walk(root)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?
        {
            agent.check_native_work()?;
            if id != root && matches!(document.node(id).map(Node::kind), Ok(NodeKind::Text(_))) {
                texts.try_reserve(1).map_err(|_| OutOfMemory)?;
                texts.push(id);
            }
        }
        for id in texts {
            agent.check_native_work()?;
            // An earlier merge may already have removed this node.
            let Ok(node) = document.node(id) else {
                continue;
            };
            if node.parent().is_none() {
                continue;
            }
            let NodeKind::Text(data) = node.kind() else {
                continue;
            };
            if data.as_units().is_empty() {
                document
                    .detach(id)
                    .map_err(|error| dom_exception::from_dom_error(agent, error))?;
                continue;
            }
            let mut run = Vec::new();
            let mut units = Vec::new();
            let mut next = node.next_sibling();
            while let Some(sibling) = next {
                agent.check_native_work()?;
                let sibling_node = document
                    .node(sibling)
                    .map_err(|error| dom_exception::from_dom_error(agent, error))?;
                let NodeKind::Text(more) = sibling_node.kind() else {
                    break;
                };
                units
                    .try_reserve(more.as_units().len())
                    .map_err(|_| OutOfMemory)?;
                units.extend_from_slice(more.as_units());
                run.try_reserve(1).map_err(|_| OutOfMemory)?;
                run.push(sibling);
                next = sibling_node.next_sibling();
            }
            if run.is_empty() {
                continue;
            }
            document
                .merge_text(id, &run, &units)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        }
        Ok(Value::UNDEFINED)
    })
}
