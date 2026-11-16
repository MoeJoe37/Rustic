//! Window bindings for the active top-level document.

use rustic_dom::NodeKind;
use rustic_js::runtime::{Agent, JsResult, NativeFn, ObjectId, PropertyDescriptor, Value};

use super::{illegal_constructor, install_interface, prototype, require_receiver, with_page};

pub(super) fn install(agent: &mut Agent) -> JsResult<Value> {
    install_interface(
        agent,
        "Window",
        None,
        illegal_constructor,
        0,
        &[],
        |_, _| Ok(()),
    )?;
    let prototype = prototype(agent, "Window")?;
    let global = agent.intrinsics.global;
    for (name, get, set) in [
        ("window", view as NativeFn, None),
        ("document", document, None),
        ("top", view, None),
        ("self", view, Some(replace_self as NativeFn)),
        ("frames", view, Some(replace_frames)),
        ("parent", view, Some(replace_parent)),
        ("length", length, Some(replace_length)),
        ("frameElement", frame_element, None),
        ("closed", closed, None),
    ] {
        let get = Value::object(agent.new_native_function(get, &format!("get {name}"), 0)?);
        let set = match set {
            Some(set) => {
                Value::object(agent.new_native_function(set, &format!("set {name}"), 1)?)
            }
            None => Value::UNDEFINED,
        };
        let key = agent.heap.key(name)?;
        let configurable = !matches!(name, "window" | "top" | "document");
        agent.define_property_or_throw(
            global,
            key,
            PropertyDescriptor::accessor(get, set, true, configurable),
        )?;
    }
    agent.install_window_proxy(prototype).map(Value::object)
}

fn receiver(agent: &mut Agent, value: Value) -> JsResult<ObjectId> {
    let global = agent.intrinsics.global;
    if value.is_nullish() {
        return Ok(global);
    }
    match value.as_object() {
        Some(object) if object == global || object == agent.global_this() => Ok(object),
        _ => agent.type_error("expected a Window receiver"),
    }
}

fn view(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    receiver(agent, this)?;
    Ok(Value::object(agent.global_this()))
}

fn document(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    receiver(agent, this)?;
    with_page(agent, |_, page| Ok(page.root))
}

fn length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    receiver(agent, this)?;
    Ok(Value::number(0.0))
}

fn frame_element(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    receiver(agent, this)?;
    Ok(Value::NULL)
}

fn closed(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    receiver(agent, this)?;
    Ok(Value::FALSE)
}

fn replace(agent: &mut Agent, this: Value, args: &[Value], name: &str) -> JsResult<Value> {
    let target = receiver(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let key = agent.heap.key(name)?;
    agent.create_data_property_or_throw(target, key, value)?;
    Ok(Value::UNDEFINED)
}

fn replace_self(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    replace(agent, this, args, "self")
}

fn replace_frames(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    replace(agent, this, args, "frames")
}

fn replace_parent(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    replace(agent, this, args, "parent")
}

fn replace_length(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    replace(agent, this, args, "length")
}

pub(super) fn default_view(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let id = require_receiver(
        agent,
        this,
        |kind| matches!(kind, NodeKind::Document),
        "expected a Document receiver",
    )?;
    with_page(agent, |_, page| {
        Ok(if id.document() == page.main {
            page.window
        } else {
            Value::NULL
        })
    })
}
