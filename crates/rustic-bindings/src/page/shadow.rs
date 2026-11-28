//! Shadow-root construction and the DOM's ShadowRoot interface.

use rustic_dom::{DomError, NodeKind, ShadowRootMode, ShadowRootOptions, SlotAssignment};
use rustic_js::runtime::{Agent, JsResult, ObjectId, Value};

use super::{
    creation, dom_exception, events, illegal_constructor, install_interface, read_node,
    required_arg, with_mut_tree, wrap,
};

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        "ShadowRoot",
        Some("DocumentFragment"),
        illegal_constructor,
        0,
        &[
            ("host", "get host", host),
            ("mode", "get mode", mode),
            (
                "customElementRegistry",
                "get customElementRegistry",
                super::custom_elements::registry_of,
            ),
            ("delegatesFocus", "get delegatesFocus", delegates_focus),
            ("clonable", "get clonable", clonable),
            ("serializable", "get serializable", serializable),
            ("slotAssignment", "get slotAssignment", slot_assignment),
        ],
        |_, _| Ok(()),
    )
}

pub(super) fn install_element(agent: &mut Agent, proto: ObjectId) -> JsResult<()> {
    events::define_method(agent, proto, "attachShadow", attach, 1)
}

pub(super) fn shadow_root(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, wrappers, doc, _, node| {
        let NodeKind::Element(element) = node.kind() else {
            return agent.type_error("expected Element");
        };
        match element
            .shadow_root()
            .filter(|_| element.shadow_root_mode() == Some(ShadowRootMode::Open))
        {
            Some(root) => wrap(wrappers, agent, doc, root),
            None => Ok(Value::NULL),
        }
    })
}

fn attach(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let init = required_arg(agent, args, 0)?;
    let Some(object) = init.as_object() else {
        return agent.type_error("ShadowRootInit requires a mode");
    };
    let key = agent.heap.key("clonable")?;
    let value = agent.get(object, key)?;
    let clonable = agent.to_boolean(value);
    let key = agent.heap.key("customElementRegistry")?;
    let scope = agent.get(object, key)?;
    let scope = super::custom_elements::convert_registry(agent, scope)?;
    let key = agent.heap.key("delegatesFocus")?;
    let value = agent.get(object, key)?;
    let delegates_focus = agent.to_boolean(value);
    let key = agent.heap.key("mode")?;
    let value = agent.get(object, key)?;
    let mode = match String::from_utf16_lossy(creation::string(agent, value)?.as_units()).as_str() {
        "open" => ShadowRootMode::Open,
        "closed" => ShadowRootMode::Closed,
        _ => return agent.type_error("invalid shadow root mode"),
    };
    let key = agent.heap.key("serializable")?;
    let value = agent.get(object, key)?;
    let serializable = agent.to_boolean(value);
    let key = agent.heap.key("slotAssignment")?;
    let value = agent.get(object, key)?;
    let slot_assignment = if value == Value::UNDEFINED {
        SlotAssignment::Named
    } else {
        match String::from_utf16_lossy(creation::string(agent, value)?.as_units()).as_str() {
            "named" => SlotAssignment::Named,
            "manual" => SlotAssignment::Manual,
            _ => return agent.type_error("invalid slot assignment mode"),
        }
    };
    let scope = if let Some(scope) = scope {
        let owner = super::custom_elements::owner_document(agent, receiver)?;
        super::custom_elements::check_global(agent, owner, scope)?;
        scope
    } else {
        let id = super::traversal::node_of(agent, receiver)?;
        super::custom_elements::owner_registry(agent, id)?
    };
    let options = ShadowRootOptions {
        keep_registry_null: scope == Value::NULL,
        clonable,
        delegates_focus,
        serializable,
        slot_assignment,
        ..ShadowRootOptions::new(mode)
    };
    with_mut_tree(agent, receiver, |agent, doc, wrappers, id| {
        if !matches!(
            doc.node(id).expect("live receiver").kind(),
            NodeKind::Element(_)
        ) {
            return agent.type_error("expected Element");
        }
        let root = doc
            .attach_shadow_with_options(id, options)
            .map_err(|error| match error {
                DomError::WrongNodeKind | DomError::InvalidHierarchy => creation::exception(
                    agent,
                    "NotSupportedError",
                    "element cannot attach this shadow root",
                ),
                _ => dom_exception::from_dom_error(agent, error),
            })?;
        let value = wrap(wrappers, agent, doc, root)?;
        super::custom_elements::set_registry(agent, value, scope)?;
        Ok(value)
    })
}

fn host(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, wrappers, doc, _, node| {
        let Some(host) = node.shadow_host() else {
            return agent.type_error("expected ShadowRoot");
        };
        wrap(wrappers, agent, doc, host)
    })
}

fn options(agent: &mut Agent, receiver: Value) -> JsResult<ShadowRootOptions> {
    read_node(agent, receiver, |agent, _, doc, _, node| {
        let Some(host) = node.shadow_host() else {
            return agent.type_error("expected ShadowRoot");
        };
        let NodeKind::Element(element) = doc.node(host).expect("live host").kind() else {
            unreachable!("a shadow host is an element");
        };
        Ok(element.shadow_root_options().expect("host owns a root"))
    })
}

fn mode(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    let value = match options(agent, receiver)?.mode {
        ShadowRootMode::Open => "open",
        ShadowRootMode::Closed => "closed",
    };
    agent.string(value)
}

fn delegates_focus(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(options(agent, receiver)?.delegates_focus))
}

fn clonable(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(options(agent, receiver)?.clonable))
}

fn serializable(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(options(agent, receiver)?.serializable))
}

fn slot_assignment(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    let value = match options(agent, receiver)?.slot_assignment {
        SlotAssignment::Named => "named",
        SlotAssignment::Manual => "manual",
    };
    agent.string(value)
}
