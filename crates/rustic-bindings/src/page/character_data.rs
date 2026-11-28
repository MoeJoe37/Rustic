//! CharacterData, Text and Comment over UTF-16 DOM payloads.

use rustic_dom::{Document, DomError, DomString, NodeId, NodeKind};
use rustic_js::runtime::{Abrupt, Agent, JsResult, NativeFn, ObjectId, PropertyDescriptor, Value};

use crate::metadata::{self, Argument, ConvertedValue, Member};
use crate::webidl::{from_dom_units, to_nullable_dom_string};
use crate::{DomGcError, dom_exception};

use super::{
    create_node, illegal_constructor, install_interface, prototype, read_node, require_receiver,
    with_mut_tree,
};

const CHARACTER_DATA: &str = "CharacterData";
const TEXT: &str = "Text";
const COMMENT: &str = "Comment";

pub(super) fn install_text_content(agent: &mut Agent, prototype: ObjectId) -> JsResult<()> {
    let get = Value::object(agent.new_native_function(text_content, "get textContent", 0)?);
    let set = Value::object(agent.new_native_function(set_text_content, "set textContent", 1)?);
    let key = agent.heap.key("textContent")?;
    agent.define_property_or_throw(
        prototype,
        key,
        PropertyDescriptor::accessor(get, set, true, true),
    )
}

fn text_content(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, document, id, node| {
        match node.kind() {
            NodeKind::Element(_) | NodeKind::DocumentFragment => {
                let mut content = Vec::new();
                let descendants = document
                    .walk(id)
                    .map_err(|error| dom_exception::from_dom_error(agent, error))?;
                for (child, _) in descendants.skip(1) {
                    agent.check_native_work()?;
                    let node = document
                        .node(child)
                        .map_err(|error| dom_exception::from_dom_error(agent, error))?;
                    if let NodeKind::Text(data) | NodeKind::CdataSection(data) = node.kind() {
                        content
                            .try_reserve(data.as_units().len())
                            .map_err(|_| Abrupt::OutOfMemory)?;
                        content.extend_from_slice(data.as_units());
                    }
                }
                from_dom_units(agent, &content)
            }
            NodeKind::Document | NodeKind::DocumentType(_) => Ok(Value::NULL),
            _ => {
                let units = payload(agent, document, id)?;
                from_dom_units(agent, units)
            }
        }
    })
}

fn set_text_content(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    require_receiver(agent, receiver, |_| true, "expected a Node receiver")?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let value = to_nullable_dom_string(agent, value)?.unwrap_or_default();
    mutate(agent, receiver, |document, id| {
        match document.node(id)?.kind() {
            NodeKind::Element(_) | NodeKind::DocumentFragment => {
                document.replace_children_with_text(id, value)
            }
            NodeKind::Document | NodeKind::DocumentType(_) => Ok(()),
            _ => document.set_character_data(id, value).map(drop),
        }
    })
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        CHARACTER_DATA,
        parent(CHARACTER_DATA),
        illegal_constructor,
        0,
        &[("length", "get length", length)],
        |agent, prototype| {
            let get = Value::object(agent.new_native_function(data, "get data", 0)?);
            let set = Value::object(agent.new_native_function(set_data, "set data", 1)?);
            let key = agent.heap.key("data")?;
            agent.define_property_or_throw(
                prototype,
                key,
                PropertyDescriptor::accessor(get, set, true, true),
            )?;
            for (name, native) in [
                ("substringData", substring_data as NativeFn),
                ("appendData", append_data),
                ("insertData", insert_data),
                ("deleteData", delete_data),
                ("replaceData", replace_data),
            ] {
                install_operation(agent, prototype, CHARACTER_DATA, name, native)?;
            }
            Ok(())
        },
    )?;
    install_interface(
        agent,
        TEXT,
        parent(TEXT),
        construct_text,
        0,
        &[
            ("wholeText", "get wholeText", whole_text),
            (
                "assignedSlot",
                "get assignedSlot",
                super::slots::assigned_slot,
            ),
        ],
        |agent, prototype| install_operation(agent, prototype, TEXT, "splitText", split_text),
    )?;
    install_interface(
        agent,
        "CDATASection",
        parent("CDATASection"),
        illegal_constructor,
        0,
        &[],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        COMMENT,
        parent(COMMENT),
        construct_comment,
        0,
        &[],
        |_, _| Ok(()),
    )
}

fn parent(interface: &str) -> Option<&'static str> {
    metadata::interface(interface)
        .expect("committed interface metadata")
        .parent()
}

fn members(interface: &str) -> &'static [Member] {
    metadata::interface(interface)
        .expect("committed interface metadata")
        .members()
}

fn operation(interface: &str, name: &str) -> &'static [Argument] {
    members(interface)
        .iter()
        .find_map(|member| match member {
            Member::Operation {
                name: key,
                arguments,
                ..
            } if *key == name => Some(*arguments),
            _ => None,
        })
        .expect("committed operation metadata")
}

fn install_operation(
    agent: &mut Agent,
    prototype: ObjectId,
    interface: &str,
    name: &str,
    native: NativeFn,
) -> JsResult<()> {
    let required = operation(interface, name)
        .iter()
        .take_while(|arg| !arg.is_optional())
        .count();
    let length = u32::try_from(required).expect("short generated signature");
    let key = agent.heap.key(name)?;
    agent.define_method(prototype, key, native, name, length)
}

fn string(value: ConvertedValue) -> DomString {
    let ConvertedValue::DomString(value) = value else {
        unreachable!("DOMString in committed interface metadata");
    };
    value
}

fn index(value: ConvertedValue) -> JsResult<usize> {
    let ConvertedValue::UnsignedLong(value) = value else {
        unreachable!("unsigned long in committed interface metadata");
    };
    usize::try_from(value).map_err(|_| Abrupt::OutOfMemory)
}

fn character_data(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Text(_)
            | NodeKind::CdataSection(_)
            | NodeKind::Comment(_)
            | NodeKind::ProcessingInstruction(_)
    )
}

fn is_text(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Text(_) | NodeKind::CdataSection(_))
}

fn character_receiver(agent: &mut Agent, receiver: Value) -> JsResult<NodeId> {
    require_receiver(
        agent,
        receiver,
        character_data,
        "expected a CharacterData receiver",
    )
}

fn payload<'a>(agent: &mut Agent, document: &'a Document, id: NodeId) -> JsResult<&'a [u16]> {
    match document.character_data(id) {
        Ok(data) => Ok(data.as_units()),
        Err(DomError::WrongNodeKind) => agent.type_error("expected a CharacterData receiver"),
        Err(error) => Err(dom_exception::from_dom_error(agent, error)),
    }
}

fn data(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, document, id, _| {
        let units = payload(agent, document, id)?;
        from_dom_units(agent, units)
    })
}

fn length(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, document, id, _| {
        let units = payload(agent, document, id)?;
        let len = u32::try_from(units.len()).map_err(|_| Abrupt::OutOfMemory)?;
        Ok(Value::number(f64::from(len)))
    })
}

fn set_data(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    character_receiver(agent, receiver)?;
    let value_type = members(CHARACTER_DATA)
        .iter()
        .find_map(|member| match member {
            Member::Attribute {
                name: "data",
                value_type,
                ..
            } => Some(*value_type),
            _ => None,
        })
        .expect("committed data attribute metadata");
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let value = string(value_type.convert(agent, value)?);
    mutate(agent, receiver, |document, id| {
        document.set_character_data(id, value).map(drop)
    })
}

fn range_arguments(agent: &mut Agent, name: &str, args: &[Value]) -> JsResult<(usize, usize)> {
    let signature = operation(CHARACTER_DATA, name);
    if args.len() < signature.len() {
        return agent.type_error("required Web IDL argument is missing");
    }
    let start = index(signature[0].convert(agent, args.first().copied())?)?;
    let count = index(signature[1].convert(agent, args.get(1).copied())?)?;
    Ok((start, count))
}

fn substring_data(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    character_receiver(agent, receiver)?;
    let (start, count) = range_arguments(agent, "substringData", args)?;
    read_node(agent, receiver, |agent, _, document, id, _| {
        let units = payload(agent, document, id)?;
        let suffix = units
            .get(start..)
            .ok_or_else(|| dom_exception::from_dom_error(agent, DomError::InvalidOffset))?;
        from_dom_units(agent, &suffix[..count.min(suffix.len())])
    })
}

fn mutate(
    agent: &mut Agent,
    receiver: Value,
    edit: impl FnOnce(&mut Document, NodeId) -> Result<(), DomError>,
) -> JsResult<Value> {
    with_mut_tree(agent, receiver, |agent, document, _, id| {
        edit(document, id).map_err(|error| dom_exception::from_dom_error(agent, error))?;
        Ok(Value::UNDEFINED)
    })
}

fn append_data(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    character_receiver(agent, receiver)?;
    let arg = operation(CHARACTER_DATA, "appendData")[0];
    let text = string(arg.convert(agent, args.first().copied())?);
    mutate(agent, receiver, |document, id| {
        document.append_character_data(id, text.as_units())
    })
}

fn insert_data(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    character_receiver(agent, receiver)?;
    let signature = operation(CHARACTER_DATA, "insertData");
    if args.len() < signature.len() {
        return agent.type_error("required Web IDL argument is missing");
    }
    let offset = index(signature[0].convert(agent, args.first().copied())?)?;
    let text = string(signature[1].convert(agent, args.get(1).copied())?);
    mutate(agent, receiver, |document, id| {
        document.replace_character_data(id, offset, 0, text.as_units())
    })
}

fn delete_data(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    character_receiver(agent, receiver)?;
    let (offset, count) = range_arguments(agent, "deleteData", args)?;
    mutate(agent, receiver, |document, id| {
        document.replace_character_data(id, offset, count, &[])
    })
}

fn replace_data(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    character_receiver(agent, receiver)?;
    let (offset, count) = range_arguments(agent, "replaceData", args)?;
    let arg = operation(CHARACTER_DATA, "replaceData")[2];
    let text = string(arg.convert(agent, args.get(2).copied())?);
    mutate(agent, receiver, |document, id| {
        document.replace_character_data(id, offset, count, text.as_units())
    })
}

fn construct(
    agent: &mut Agent,
    args: &[Value],
    interface: &'static str,
    create: impl FnOnce(&mut Document, DomString) -> Result<NodeId, DomError>,
) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("DOM constructor requires new");
    };
    let signature = members(interface)
        .iter()
        .find_map(|member| match member {
            Member::Constructor(arguments) => Some(*arguments),
            _ => None,
        })
        .expect("committed constructor metadata");
    let text = string(signature[0].convert(agent, args.first().copied())?);
    let prototype = agent.host_prototype_from_new_target(interface, target)?;
    create_node(agent, None, Some(prototype), |document| {
        create(document, text)
    })
}

fn construct_text(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    construct(agent, args, TEXT, |document, text| {
        document.create_text(text)
    })
}

fn construct_comment(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    construct(agent, args, COMMENT, |document, text| {
        document.create_comment(text)
    })
}

fn split_text(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    require_receiver(agent, receiver, is_text, "expected a Text receiver")?;
    let arg = operation(TEXT, "splitText")[0];
    let offset = index(arg.convert(agent, args.first().copied())?)?;
    let prototype = prototype(agent, TEXT)?;
    with_mut_tree(agent, receiver, |agent, document, wrappers, id| {
        document
            .split_text_with_preflight(id, offset, |document, new| {
                wrappers
                    .wrap_with_prototype(agent, document, new, prototype)
                    .map_err(|error| match error {
                        DomGcError::Dom(error) => error,
                        _ => DomError::InvalidNode,
                    })
            })
            .map(Value::object)
            .map_err(|error| dom_exception::from_dom_error(agent, error))
    })
}

fn whole_text(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, document, id, node| {
        if !is_text(node.kind()) {
            return agent.type_error("expected a Text receiver");
        }
        let mut start = id;
        let mut previous = node.previous_sibling();
        while let Some(prev) = previous {
            let sibling = document
                .node(prev)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
            if !is_text(sibling.kind()) {
                break;
            }
            start = prev;
            previous = sibling.previous_sibling();
        }
        let mut result = Vec::new();
        let mut current = Some(start);
        while let Some(id) = current {
            let sibling = document
                .node(id)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
            if !is_text(sibling.kind()) {
                break;
            }
            let units = payload(agent, document, id)?;
            result
                .try_reserve(units.len())
                .map_err(|_| Abrupt::OutOfMemory)?;
            result.extend_from_slice(units);
            current = sibling.next_sibling();
        }
        from_dom_units(agent, &result)
    })
}
