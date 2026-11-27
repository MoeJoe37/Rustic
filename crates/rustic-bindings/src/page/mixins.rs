//! The DOM's `ParentNode` (`prepend`, `append`, `replaceChildren`) and
//! `ChildNode` (`before`, `after`, `replaceWith`, `remove`) mixins, and
//! `Node.isEqualNode` (DOM §4.2.6–4.2.8, §4.4).
//!
//! The mixins compose the same native insertion and removal steps as
//! `insertBefore` and `removeChild`; strings become Text nodes of the
//! receiver's node document ("convert nodes into a node").

use rustic_dom::{Document, Node, NodeId, NodeKind};
use rustic_js::runtime::{Agent, JsResult, NativeFn, ObjectId, Value};

use super::traversal::{node_of, read_doc, wrap_node};
use super::{WrapperKind, prototype, with_page};

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    for interface in ["Document", "DocumentFragment", "Element"] {
        let proto = prototype(agent, interface)?;
        for (name, method) in [
            ("prepend", prepend as NativeFn),
            ("append", append),
            ("replaceChildren", replace_children),
        ] {
            define(agent, proto, name, method, 0)?;
        }
    }
    for interface in ["Element", "CharacterData", "DocumentType"] {
        let proto = prototype(agent, interface)?;
        for (name, method) in [
            ("before", before as NativeFn),
            ("after", after),
            ("replaceWith", replace_with),
            ("remove", remove),
        ] {
            define(agent, proto, name, method, 0)?;
        }
    }
    let node = prototype(agent, "Node")?;
    define(agent, node, "isEqualNode", is_equal_node, 1)
}

fn define(
    agent: &mut Agent,
    proto: ObjectId,
    name: &str,
    method: NativeFn,
    length: u32,
) -> JsResult<()> {
    let key = agent.heap.key(name)?;
    agent.define_method(proto, key, method, name, length)?;
    Ok(())
}

fn relation(
    agent: &mut Agent,
    id: NodeId,
    step: impl FnOnce(&Node) -> Option<NodeId>,
) -> JsResult<Option<NodeId>> {
    read_doc(agent, id, |document| document.node(id).ok().and_then(step))
}

/// The receiver's node document wrapper.
fn node_document(agent: &mut Agent, this: NodeId) -> JsResult<Value> {
    let root = read_doc(agent, this, |document| document.root())?;
    wrap_node(agent, root)
}

/// Whether `value` is a node wrapper.
fn as_node(agent: &mut Agent, value: Value) -> JsResult<Option<NodeId>> {
    with_page(agent, |agent, page| {
        let Ok(wrappers) = page.wrappers.try_borrow() else {
            return agent.type_error("DOM bindings are busy");
        };
        Ok(value
            .as_object()
            .and_then(|object| wrappers.identity(agent, object, WrapperKind::Node)))
    })
}

/// DOM "convert nodes into a node".
fn convert(agent: &mut Agent, this: NodeId, args: &[Value]) -> JsResult<Value> {
    let document = node_document(agent, this)?;
    let mut nodes = Vec::with_capacity(args.len());
    for &arg in args {
        if as_node(agent, arg)?.is_some() {
            nodes.push(arg);
        } else {
            let text = agent.to_string(arg)?;
            nodes.push(super::creation::create_text_node(
                agent,
                document,
                Value::string(text),
            )?);
        }
    }
    if nodes.len() == 1 {
        return Ok(nodes[0]);
    }
    let fragment = super::creation::create_fragment_node(agent, document)?;
    for node in nodes {
        super::insert_child(agent, fragment, node, Value::NULL)?;
    }
    Ok(fragment)
}

fn receiver(agent: &mut Agent, this: Value) -> JsResult<NodeId> {
    node_of(agent, this)
}

fn prepend(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = receiver(agent, this)?;
    let node = convert(agent, id, args)?;
    let first = match relation(agent, id, Node::first_child)? {
        Some(first) => wrap_node(agent, first)?,
        None => Value::NULL,
    };
    super::insert_child(agent, this, node, first)?;
    Ok(Value::UNDEFINED)
}

fn append(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = receiver(agent, this)?;
    let node = convert(agent, id, args)?;
    super::insert_child(agent, this, node, Value::NULL)?;
    Ok(Value::UNDEFINED)
}

fn children(agent: &mut Agent, id: NodeId) -> JsResult<Vec<NodeId>> {
    read_doc(agent, id, |document| {
        document
            .children(id)
            .map(|c| c.collect())
            .unwrap_or_default()
    })
}

fn replace_children(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = receiver(agent, this)?;
    let node = convert(agent, id, args)?;
    let old = children(agent, id)?;
    // Insert first so a rejected node leaves the children in place.
    super::insert_child(agent, this, node, Value::NULL)?;
    for child in old {
        if relation(agent, child, Node::parent)? == Some(id) {
            let wrapper = wrap_node(agent, child)?;
            super::remove_child(agent, this, &[wrapper])?;
        }
    }
    Ok(Value::UNDEFINED)
}

/// Node arguments, for skipping viable siblings.
fn argument_nodes(agent: &mut Agent, args: &[Value]) -> JsResult<Vec<NodeId>> {
    let mut out = Vec::new();
    for &arg in args {
        if let Some(id) = as_node(agent, arg)? {
            out.push(id);
        }
    }
    Ok(out)
}

/// The first sibling of `id` in `next` direction that is not in `skip`.
fn viable(agent: &mut Agent, id: NodeId, skip: &[NodeId], next: bool) -> JsResult<Option<NodeId>> {
    let mut at = id;
    loop {
        let sibling = if next {
            relation(agent, at, Node::next_sibling)?
        } else {
            relation(agent, at, Node::previous_sibling)?
        };
        match sibling {
            Some(s) if skip.contains(&s) => at = s,
            other => return Ok(other),
        }
    }
}

fn wrap_or_null(agent: &mut Agent, id: Option<NodeId>) -> JsResult<Value> {
    match id {
        Some(id) => wrap_node(agent, id),
        None => Ok(Value::NULL),
    }
}

fn before(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = receiver(agent, this)?;
    let Some(parent) = relation(agent, id, Node::parent)? else {
        return Ok(Value::UNDEFINED);
    };
    let skip = argument_nodes(agent, args)?;
    let previous = viable(agent, id, &skip, false)?;
    let node = convert(agent, id, args)?;
    let reference = match previous {
        None => relation(agent, parent, Node::first_child)?,
        Some(previous) => relation(agent, previous, Node::next_sibling)?,
    };
    let parent = wrap_node(agent, parent)?;
    let reference = wrap_or_null(agent, reference)?;
    super::insert_child(agent, parent, node, reference)?;
    Ok(Value::UNDEFINED)
}

fn after(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = receiver(agent, this)?;
    let Some(parent) = relation(agent, id, Node::parent)? else {
        return Ok(Value::UNDEFINED);
    };
    let skip = argument_nodes(agent, args)?;
    let next = viable(agent, id, &skip, true)?;
    let node = convert(agent, id, args)?;
    let parent = wrap_node(agent, parent)?;
    let reference = wrap_or_null(agent, next)?;
    super::insert_child(agent, parent, node, reference)?;
    Ok(Value::UNDEFINED)
}

fn replace_with(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = receiver(agent, this)?;
    let Some(parent) = relation(agent, id, Node::parent)? else {
        return Ok(Value::UNDEFINED);
    };
    let skip = argument_nodes(agent, args)?;
    let next = viable(agent, id, &skip, true)?;
    let node = convert(agent, id, args)?;
    let parent_wrapper = wrap_node(agent, parent)?;
    if relation(agent, id, Node::parent)? == Some(parent) {
        super::node_tree::replace_child_value(agent, parent_wrapper, node, this)?;
    } else {
        let reference = wrap_or_null(agent, next)?;
        super::insert_child(agent, parent_wrapper, node, reference)?;
    }
    Ok(Value::UNDEFINED)
}

fn remove(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let id = receiver(agent, this)?;
    if let Some(parent) = relation(agent, id, Node::parent)? {
        let parent = wrap_node(agent, parent)?;
        super::remove_child(agent, parent, &[this])?;
    }
    Ok(Value::UNDEFINED)
}

/// DOM "equals" between `a` in `da` and `b` in `db`.
fn equals(da: &Document, a: NodeId, db: &Document, b: NodeId) -> bool {
    let (Ok(na), Ok(nb)) = (da.node(a), db.node(b)) else {
        return false;
    };
    let same = match (na.kind(), nb.kind()) {
        (NodeKind::Document, NodeKind::Document)
        | (NodeKind::DocumentFragment, NodeKind::DocumentFragment) => true,
        (NodeKind::DocumentType(x), NodeKind::DocumentType(y)) => {
            x.name == y.name && x.public_id == y.public_id && x.system_id == y.system_id
        }
        (NodeKind::Element(x), NodeKind::Element(y)) => {
            x.name() == y.name()
                && x.attributes().len() == y.attributes().len()
                && x.attributes().iter().all(|attribute| {
                    y.attributes().iter().any(|other| {
                        other.name().namespace() == attribute.name().namespace()
                            && other.name().local() == attribute.name().local()
                            && other.value() == attribute.value()
                    })
                })
        }
        (NodeKind::ProcessingInstruction(x), NodeKind::ProcessingInstruction(y)) => {
            x.target() == y.target() && x.data() == y.data()
        }
        (NodeKind::Text(x), NodeKind::Text(y))
        | (NodeKind::Comment(x), NodeKind::Comment(y))
        | (NodeKind::CdataSection(x), NodeKind::CdataSection(y)) => x == y,
        _ => false,
    };
    if !same {
        return false;
    }
    let (Ok(ca), Ok(cb)) = (da.children(a), db.children(b)) else {
        return false;
    };
    let (ca, cb): (Vec<NodeId>, Vec<NodeId>) = (ca.collect(), cb.collect());
    ca.len() == cb.len() && ca.iter().zip(&cb).all(|(&x, &y)| equals(da, x, db, y))
}

fn is_equal_node(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let a = receiver(agent, this)?;
    let other = args.first().copied().unwrap_or(Value::UNDEFINED);
    if other == Value::NULL || other == Value::UNDEFINED {
        return Ok(Value::boolean(false));
    }
    let Some(b) = as_node(agent, other)? else {
        return agent.type_error("isEqualNode expects a Node or null");
    };
    let equal = with_page(agent, |agent, page| {
        page.read(agent, a.document(), |agent, da| {
            if a.document() == b.document() {
                return Ok(equals(da, a, da, b));
            }
            page.read(agent, b.document(), |_, db| Ok(equals(da, a, db, b)))
        })
    })?;
    Ok(Value::boolean(equal))
}
