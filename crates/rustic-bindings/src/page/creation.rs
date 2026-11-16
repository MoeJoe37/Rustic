//! Factories on the active HTML document.

use rustic_dom::{Document, DomString, NodeId, NodeKind, QualifiedName};
use rustic_js::runtime::{Abrupt, Agent, JsResult, NativeFn, ObjectId, Value};

use crate::dom_exception;
use crate::webidl::{DomStringConversion, from_dom_string, to_dom_string};

use super::{create_node, read_node, require_receiver, required_arg};

pub(super) fn install_factories(agent: &mut Agent, prototype: ObjectId) -> JsResult<()> {
    for (name, operation, length) in [
        ("createElement", create_element as NativeFn, 1),
        ("createTextNode", create_text, 1),
        ("createComment", create_comment, 1),
        ("createDocumentFragment", create_fragment, 0),
        ("createProcessingInstruction", create_instruction, 2),
        ("createCDATASection", create_cdata, 1),
    ] {
        let key = agent.heap.key(name)?;
        agent.define_method(prototype, key, operation, name, length)?;
    }
    Ok(())
}

fn document_receiver(agent: &mut Agent, receiver: Value) -> JsResult<NodeId> {
    require_receiver(
        agent,
        receiver,
        |kind| matches!(kind, NodeKind::Document),
        "expected a Document receiver",
    )
}

pub(super) fn string(agent: &mut Agent, input: Value) -> JsResult<DomString> {
    to_dom_string(agent, input, DomStringConversion::Default)
}

// Raw names in the DOM are UTF-8. Reject ill-formed names without rewriting
// them; payloads continue to use all UTF-16 units.
pub(super) fn scalar_name(value: &DomString) -> JsResult<Option<String>> {
    let mut name = String::new();
    for point in char::decode_utf16(value.as_units().iter().copied()) {
        let Ok(point) = point else {
            return Ok(None);
        };
        name.try_reserve(point.len_utf8())
            .map_err(|_| Abrupt::OutOfMemory)?;
        name.push(point);
    }
    Ok(Some(name))
}

pub(super) fn invalid_name(agent: &mut Agent) -> Abrupt {
    dom_exception::from_dom_error(agent, rustic_dom::DomError::InvalidName)
}

pub(super) fn excluded_name_char(c: char) -> bool {
    matches!(c, '\0' | '\t' | '\n' | '\u{000C}' | '\r' | ' ' | '/' | '>')
}

fn valid_element_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => chars.all(|c| !excluded_name_char(c)),
        Some(first) if matches!(first, ':' | '_') || !first.is_ascii() => chars.all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '_') || !c.is_ascii()
        }),
        _ => false,
    }
}

fn has_custom_element_options(agent: &mut Agent, value: Value) -> JsResult<bool> {
    if value.is_nullish() {
        return Ok(false);
    }
    if let Some(object) = value.as_object() {
        let key = agent.heap.key("customElementRegistry")?;
        let registry = agent.get(object, key)?;
        if !registry.is_nullish() {
            return agent
                .type_error("customElementRegistry must be a supported CustomElementRegistry");
        }
        let key = agent.heap.key("is")?;
        let is = agent.get(object, key)?;
        if is == Value::UNDEFINED {
            return Ok(false);
        }
        string(agent, is)?;
    } else {
        string(agent, value)?;
    }
    Ok(true)
}

fn create_element(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    let name = required_arg(agent, args, 0)?;
    let name = string(agent, name)?;
    let customized =
        has_custom_element_options(agent, args.get(1).copied().unwrap_or(Value::UNDEFINED))?;
    let mut name = scalar_name(&name)?
        .filter(|name| valid_element_name(name))
        .ok_or_else(|| invalid_name(agent))?;
    if customized {
        return Err(unsupported(
            agent,
            "custom element creation options are unavailable",
        ));
    }
    name.make_ascii_lowercase();
    let is_template = name == "template";
    let name =
        QualifiedName::html(&name).map_err(|error| dom_exception::from_dom_error(agent, error))?;
    create_node(agent, None, |document| {
        let id = document.create_element(name)?;
        if is_template {
            if let Err(error) = document.create_template_contents(id) {
                document.destroy_subtree(id).expect("new detached template");
                return Err(error);
            }
        }
        Ok(id)
    })
}

fn create_text(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    let value = required_arg(agent, args, 0)?;
    let value = string(agent, value)?;
    create_node(agent, None, |document| document.create_text(value))
}

fn create_comment(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    let value = required_arg(agent, args, 0)?;
    let value = string(agent, value)?;
    create_node(agent, None, |document| document.create_comment(value))
}

fn create_fragment(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    create_node(agent, None, Document::create_document_fragment)
}

pub(super) fn construct_fragment(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("DocumentFragment requires new");
    };
    let prototype = agent.host_prototype_from_new_target("DocumentFragment", target)?;
    create_node(agent, Some(prototype), Document::create_document_fragment)
}

fn create_instruction(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    let target = required_arg(agent, args, 0)?;
    let data = required_arg(agent, args, 1)?;
    let target = string(agent, target)?;
    let data = string(agent, data)?;
    instruction(agent, None, target, data)
}

pub(super) fn construct_instruction(
    agent: &mut Agent,
    _: Value,
    args: &[Value],
) -> JsResult<Value> {
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("ProcessingInstruction requires new");
    };
    let target = required_arg(agent, args, 0)?;
    let target = string(agent, target)?;
    let data = match args.get(1).copied() {
        None | Some(Value::UNDEFINED) => DomString::default(),
        Some(data) => string(agent, data)?,
    };
    let prototype = agent.host_prototype_from_new_target("ProcessingInstruction", new_target)?;
    instruction(agent, Some(prototype), target, data)
}

fn instruction(
    agent: &mut Agent,
    prototype: Option<ObjectId>,
    target: DomString,
    data: DomString,
) -> JsResult<Value> {
    let target = scalar_name(&target)?.ok_or_else(|| invalid_name(agent))?;
    create_node(agent, prototype, |document| {
        document.create_processing_instruction(&target, data)
    })
}

pub(super) fn instruction_target(
    agent: &mut Agent,
    receiver: Value,
    _: &[Value],
) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, _, _, node| match node.kind() {
        NodeKind::ProcessingInstruction(instruction) => {
            from_dom_string(agent, instruction.target())
        }
        _ => agent.type_error("expected a ProcessingInstruction receiver"),
    })
}

fn create_cdata(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    let data = required_arg(agent, args, 0)?;
    string(agent, data)?;
    Err(unsupported(
        agent,
        "HTML documents cannot create CDATA sections",
    ))
}

fn unsupported(agent: &mut Agent, message: &str) -> Abrupt {
    dom_exception::new(agent, "NotSupportedError", message)
        .map_or_else(|error| error, Abrupt::Throw)
}

fn doctype_field(
    agent: &mut Agent,
    receiver: Value,
    field: impl FnOnce(&rustic_dom::DocumentType) -> &DomString,
) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, _, _, node| match node.kind() {
        NodeKind::DocumentType(doctype) => from_dom_string(agent, field(doctype)),
        _ => agent.type_error("expected a DocumentType receiver"),
    })
}

pub(super) fn doctype_name(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    doctype_field(agent, receiver, |doctype| &doctype.name)
}

pub(super) fn doctype_public_id(
    agent: &mut Agent,
    receiver: Value,
    _: &[Value],
) -> JsResult<Value> {
    doctype_field(agent, receiver, |doctype| &doctype.public_id)
}

pub(super) fn doctype_system_id(
    agent: &mut Agent,
    receiver: Value,
    _: &[Value],
) -> JsResult<Value> {
    doctype_field(agent, receiver, |doctype| &doctype.system_id)
}
