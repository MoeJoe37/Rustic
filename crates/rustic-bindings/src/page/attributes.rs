//! Content attributes on the live Element.

use rustic_dom::{Attribute, Element, HTML_NAMESPACE, NodeId, NodeKind, QualifiedName};
use rustic_js::runtime::{Agent, JsResult, NativeFn, ObjectId, Value};

use crate::dom_exception;
use crate::webidl::from_dom_string;

use super::creation::{excluded_name_char, invalid_name, scalar_name, string};
use super::{read_node, require_receiver, required_arg, with_mut_tree};

pub(super) fn install(agent: &mut Agent, prototype: ObjectId) -> JsResult<()> {
    for (name, operation, length) in [
        ("getAttribute", get_attribute as NativeFn, 1),
        ("hasAttribute", has_attribute, 1),
        ("setAttribute", set_attribute, 2),
        ("removeAttribute", remove_attribute, 1),
    ] {
        let key = agent.heap.key(name)?;
        agent.define_method(prototype, key, operation, name, length)?;
    }
    Ok(())
}

fn element_receiver(agent: &mut Agent, receiver: Value) -> JsResult<NodeId> {
    require_receiver(
        agent,
        receiver,
        |kind| matches!(kind, NodeKind::Element(_)),
        "expected an Element receiver",
    )
}

fn normalize(element: &Element, name: &mut str) {
    if element.name().namespace() == Some(HTML_NAMESPACE) {
        name.make_ascii_lowercase();
    }
}

fn find_attribute<'a>(element: &'a Element, name: &str) -> Option<&'a Attribute> {
    element.attributes().iter().find(|attribute| {
        let qualified = attribute.name();
        match qualified.prefix() {
            Some(prefix) => {
                name.strip_prefix(prefix)
                    .and_then(|name| name.strip_prefix(':'))
                    == Some(qualified.local())
            }
            None => name == qualified.local(),
        }
    })
}

fn read_attribute(
    agent: &mut Agent,
    receiver: Value,
    args: &[Value],
    read: impl FnOnce(&mut Agent, Option<&Attribute>) -> JsResult<Value>,
) -> JsResult<Value> {
    element_receiver(agent, receiver)?;
    let value = required_arg(agent, args, 0)?;
    let value = string(agent, value)?;
    let Some(mut name) = scalar_name(&value)? else {
        return read(agent, None);
    };
    read_node(agent, receiver, |agent, _, _, _, node| {
        let NodeKind::Element(element) = node.kind() else {
            return agent.type_error("expected an Element receiver");
        };
        normalize(element, &mut name);
        read(agent, find_attribute(element, &name))
    })
}

fn get_attribute(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    read_attribute(agent, receiver, args, |agent, attribute| match attribute {
        Some(attribute) => from_dom_string(agent, attribute.value()),
        None => Ok(Value::NULL),
    })
}

fn has_attribute(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    read_attribute(agent, receiver, args, |_, attribute| {
        Ok(Value::boolean(attribute.is_some()))
    })
}

fn set_attribute(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let id = element_receiver(agent, receiver)?;
    let name = required_arg(agent, args, 0)?;
    let value = required_arg(agent, args, 1)?;
    let name = string(agent, name)?;
    let value = string(agent, value)?;
    let mut name = scalar_name(&name)?
        .filter(|name| !name.is_empty() && !name.chars().any(|c| c == '=' || excluded_name_char(c)))
        .ok_or_else(|| invalid_name(agent))?;

    with_mut_tree(agent, |agent, document, _| {
        let node = document
            .node(id)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        let NodeKind::Element(element) = node.kind() else {
            return agent.type_error("expected an Element receiver");
        };
        normalize(element, &mut name);
        let name = match find_attribute(element, &name) {
            Some(existing) => existing.name().clone(),
            None => QualifiedName::from_parsed(None, None, &name)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?,
        };
        document
            .set_attribute(id, name, value)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        Ok(Value::UNDEFINED)
    })
}

fn remove_attribute(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let id = element_receiver(agent, receiver)?;
    let name = required_arg(agent, args, 0)?;
    let name = string(agent, name)?;
    let Some(mut name) = scalar_name(&name)? else {
        return Ok(Value::UNDEFINED);
    };
    with_mut_tree(agent, |agent, document, _| {
        let node = document
            .node(id)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        let NodeKind::Element(element) = node.kind() else {
            return agent.type_error("expected an Element receiver");
        };
        normalize(element, &mut name);
        if let Some(attribute) = find_attribute(element, &name) {
            let qualified = attribute.name().clone();
            document
                .remove_attribute(id, qualified.namespace(), qualified.local())
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        }
        Ok(Value::UNDEFINED)
    })
}
