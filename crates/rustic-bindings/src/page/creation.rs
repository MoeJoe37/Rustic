//! Node factories and qualified-name conversion.

use rustic_dom::{Document, DomError, DomString, HTML_NAMESPACE, NodeId, NodeKind, QualifiedName};
use rustic_js::runtime::{Abrupt, Agent, JsResult, NativeFn, ObjectId, Value};

use crate::dom_exception;
use crate::webidl::{DomStringConversion, from_dom_string, to_dom_string, to_nullable_dom_string};

use super::{create_node, read_node, require_receiver, required_arg};

pub(super) fn install_factories(agent: &mut Agent, prototype: ObjectId) -> JsResult<()> {
    for (name, operation, length) in [
        ("createElement", create_element as NativeFn, 1),
        ("createElementNS", create_element_ns, 2),
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

pub(super) fn document_receiver(agent: &mut Agent, receiver: Value) -> JsResult<NodeId> {
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

fn custom_element_options(
    agent: &mut Agent,
    document: Value,
    value: Value,
) -> JsResult<(Option<DomString>, Option<Value>)> {
    let Some(object) = value.as_object() else {
        return Ok((None, None));
    };
    let key = agent.heap.key("customElementRegistry")?;
    let registry = agent.get(object, key)?;
    let registry = super::custom_elements::convert_registry(agent, registry)?;
    let key = agent.heap.key("is")?;
    let value = agent.get(object, key)?;
    let custom_is = (value != Value::UNDEFINED)
        .then(|| string(agent, value))
        .transpose()?;
    if let Some(registry) = registry {
        if custom_is.is_some() {
            return Err(unsupported(
                agent,
                "is and customElementRegistry are mutually exclusive",
            ));
        }
        super::custom_elements::check_global(agent, document, registry)?;
    }
    Ok((custom_is, registry))
}

fn create_element(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    let name = required_arg(agent, args, 0)?;
    let name = string(agent, name)?;
    let (custom_is, registry) = custom_element_options(
        agent,
        receiver,
        args.get(1).copied().unwrap_or(Value::UNDEFINED),
    )?;
    let mut name = scalar_name(&name)?
        .filter(|name| valid_element_name(name))
        .ok_or_else(|| invalid_name(agent))?;
    let node = create_node(agent, Some(receiver), None, |document| {
        if document.is_html() {
            name.make_ascii_lowercase();
        }
        let namespace = (document.is_html()
            || document.format() == rustic_dom::DocumentFormat::XhtmlDocument)
            .then_some(HTML_NAMESPACE);
        element_with_is(
            document,
            QualifiedName::from_parsed(namespace, None, &name)?,
            custom_is,
        )
    })?;
    super::custom_elements::created(agent, node, registry)
}

fn create_element_ns(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    let namespace = required_arg(agent, args, 0)?;
    let qualified_name = required_arg(agent, args, 1)?;
    let namespace = to_nullable_dom_string(agent, namespace)?;
    let name = string(agent, qualified_name)?;
    let (custom_is, registry) = custom_element_options(
        agent,
        receiver,
        args.get(2).copied().unwrap_or(Value::UNDEFINED),
    )?;
    let name = qualified(agent, namespace.as_ref(), &name, false)?;
    let node = create_node(agent, Some(receiver), None, |document| {
        element_with_is(document, name, custom_is)
    })?;
    super::custom_elements::created(agent, node, registry)
}

pub(super) fn qualified(
    agent: &mut Agent,
    namespace: Option<&DomString>,
    qualified_name: &DomString,
    attribute: bool,
) -> JsResult<QualifiedName> {
    const XML: &str = "http://www.w3.org/XML/1998/namespace";
    const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
    let qualified_name = scalar_name(qualified_name)?.ok_or_else(|| invalid_name(agent))?;
    let (prefix, local) = qualified_name
        .split_once(':')
        .map_or((None, qualified_name.as_str()), |(prefix, local)| {
            (Some(prefix), local)
        });
    let valid_local = if attribute {
        !local.is_empty() && !local.chars().any(|c| c == '=' || excluded_name_char(c))
    } else {
        valid_element_name(local)
    };
    if !valid_local
        || prefix.is_some_and(|prefix| prefix.is_empty() || prefix.chars().any(excluded_name_char))
    {
        return Err(invalid_name(agent));
    }
    let namespace = namespace
        .map(|value| scalar_name(value)?.ok_or_else(|| invalid_name(agent)))
        .transpose()?;
    let namespace = namespace
        .as_deref()
        .filter(|namespace| !namespace.is_empty());
    let is_xmlns = qualified_name == "xmlns" || prefix == Some("xmlns");
    if (prefix.is_some() && namespace.is_none())
        || (prefix == Some("xml") && namespace != Some(XML))
        || (is_xmlns != (namespace == Some(XMLNS)))
    {
        return Err(exception(
            agent,
            "NamespaceError",
            "invalid namespace for the qualified name",
        ));
    }
    QualifiedName::from_parsed(namespace, prefix, local)
        .map_err(|error| dom_exception::from_dom_error(agent, error))
}

pub(super) fn element(document: &mut Document, name: QualifiedName) -> Result<NodeId, DomError> {
    element_with_is(document, name, None)
}

pub(super) fn element_with_is(
    document: &mut Document,
    name: QualifiedName,
    custom_is: Option<DomString>,
) -> Result<NodeId, DomError> {
    let is_template = name.namespace() == Some(HTML_NAMESPACE) && name.local() == "template";
    let id = document.create_element_with_is(name, custom_is)?;
    if is_template {
        if let Err(error) = document.create_template_contents(id) {
            document.destroy_subtree(id).expect("new detached template");
            return Err(error);
        }
    }
    Ok(id)
}

pub(super) fn namespace_uri(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, _, _, node| {
        let NodeKind::Element(element) = node.kind() else {
            return agent.type_error("expected an Element receiver");
        };
        match element.name().namespace() {
            Some(namespace) => agent.string(namespace),
            None => Ok(Value::NULL),
        }
    })
}

pub(super) fn prefix(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, _, _, node| {
        let NodeKind::Element(element) = node.kind() else {
            return agent.type_error("expected an Element receiver");
        };
        match element.name().prefix() {
            Some(prefix) => agent.string(prefix),
            None => Ok(Value::NULL),
        }
    })
}

pub(super) fn tag_name(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, document, _, node| {
        let NodeKind::Element(element) = node.kind() else {
            return agent.type_error("expected an Element receiver");
        };
        let qualified = element.name();
        let local = qualified.local();
        let prefix = qualified.prefix();
        let mut tag = String::new();
        let length = prefix
            .map_or(0, |prefix| prefix.len().saturating_add(1))
            .checked_add(local.len())
            .ok_or(Abrupt::OutOfMemory)?;
        tag.try_reserve_exact(length)
            .map_err(|_| Abrupt::OutOfMemory)?;
        if let Some(prefix) = prefix {
            tag.push_str(prefix);
            tag.push(':');
        }
        tag.push_str(local);
        if document.is_html() && qualified.namespace() == Some(HTML_NAMESPACE) {
            tag.make_ascii_uppercase();
        }
        agent.string(&tag)
    })
}

/// `document.createTextNode(value)` for native callers.
pub(super) fn create_text_node(
    agent: &mut Agent,
    document: Value,
    value: Value,
) -> JsResult<Value> {
    create_text(agent, document, &[value])
}

/// `document.createDocumentFragment()` for native callers.
pub(super) fn create_fragment_node(agent: &mut Agent, document: Value) -> JsResult<Value> {
    create_fragment(agent, document, &[])
}

fn create_text(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    let value = required_arg(agent, args, 0)?;
    let value = string(agent, value)?;
    create_node(agent, Some(receiver), None, |document| {
        document.create_text(value)
    })
}

fn create_comment(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    let value = required_arg(agent, args, 0)?;
    let value = string(agent, value)?;
    create_node(agent, Some(receiver), None, |document| {
        document.create_comment(value)
    })
}

fn create_fragment(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    create_node(
        agent,
        Some(receiver),
        None,
        Document::create_document_fragment,
    )
}

pub(super) fn construct_fragment(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("DocumentFragment requires new");
    };
    let prototype = agent.host_prototype_from_new_target("DocumentFragment", target)?;
    create_node(
        agent,
        None,
        Some(prototype),
        Document::create_document_fragment,
    )
}

fn create_instruction(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    let target = required_arg(agent, args, 0)?;
    let data = required_arg(agent, args, 1)?;
    let target = string(agent, target)?;
    let data = string(agent, data)?;
    instruction(agent, Some(receiver), None, target, data)
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
    instruction(agent, None, Some(prototype), target, data)
}

fn instruction(
    agent: &mut Agent,
    receiver: Option<Value>,
    prototype: Option<ObjectId>,
    target: DomString,
    data: DomString,
) -> JsResult<Value> {
    let target = scalar_name(&target)?.ok_or_else(|| invalid_name(agent))?;
    create_node(agent, receiver, prototype, |document| {
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
    let data = string(agent, data)?;
    if read_node(agent, receiver, |_, _, document, _, _| {
        Ok(document.is_html())
    })? {
        return Err(unsupported(
            agent,
            "HTML documents cannot create CDATA sections",
        ));
    }
    create_node(agent, Some(receiver), None, |document| {
        document.create_cdata_section(data)
    })
}

pub(super) fn exception(agent: &mut Agent, name: &str, message: &str) -> Abrupt {
    dom_exception::new(agent, name, message).map_or_else(|error| error, Abrupt::Throw)
}

fn unsupported(agent: &mut Agent, message: &str) -> Abrupt {
    exception(agent, "NotSupportedError", message)
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
