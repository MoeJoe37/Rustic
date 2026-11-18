//! Window bindings for the active top-level document.

use std::fmt::Debug;
use std::time::Duration;

use rustic_dom::NodeKind;
use rustic_js::runtime::{
    Agent, JsResult, NativeFn, ObjectId, PropertyDescriptor, StringId, Value,
};

use super::{
    events, illegal_constructor, install_interface, prototype, require_receiver, required_arg,
    with_page,
};

use crate::webidl::{IntegerConversion, coerce_string, to_long, to_unsigned_long};

/// Timer code and arguments after Web IDL conversion.
pub enum TimerHandler<'a> {
    /// A callable and its supplied arguments.
    Function(Value, &'a [Value]),
    /// Script source in the current realm.
    Source(StringId),
}

/// Scheduling operations for a single Window realm.
///
/// Implementations retain queued values through the agent's roots and release
/// them on cancellation or after their final invocation.
pub trait WindowScheduler: Debug {
    /// Allocate an ID and queue a timer.
    fn set_timer(
        &self,
        agent: &mut Agent,
        handler: TimerHandler<'_>,
        delay: Duration,
        repeat: bool,
    ) -> JsResult<i32>;

    /// Cancel either kind of timer by its shared Window ID.
    fn clear_timer(&self, agent: &mut Agent, id: i32) -> JsResult<()>;

    /// Register a callback for the next offered rendering opportunity.
    fn request_frame(&self, agent: &mut Agent, callback: Value) -> JsResult<u32>;

    /// Cancel a frame callback by its Window ID.
    fn cancel_frame(&self, agent: &mut Agent, id: u32) -> JsResult<()>;
}

pub(super) fn install(agent: &mut Agent, scheduler: bool) -> JsResult<Value> {
    install_interface(
        agent,
        "Window",
        Some(events::TARGET),
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
    let computed_style = agent.new_native_function(
        super::computed_style::get_computed_style,
        "getComputedStyle",
        1,
    )?;
    let key = agent.heap.key("getComputedStyle")?;
    agent.define_property_or_throw(
        global,
        key,
        PropertyDescriptor::data_with(Value::object(computed_style), true, true, true),
    )?;
    if scheduler {
        for (name, function, length) in [
            ("setTimeout", set_timer::<false> as NativeFn, 1),
            ("setInterval", set_timer::<true>, 1),
            ("clearTimeout", clear_timer, 0),
            ("clearInterval", clear_timer, 0),
            ("requestAnimationFrame", request_frame, 1),
            ("cancelAnimationFrame", cancel_frame, 1),
        ] {
            let value = Value::object(agent.new_native_function(function, name, length)?);
            let key = agent.heap.key(name)?;
            agent.define_property_or_throw(
                global,
                key,
                PropertyDescriptor::data_with(value, true, true, true),
            )?;
        }
    }
    agent.install_window_proxy(prototype).map(Value::object)
}

pub(super) fn receiver(agent: &mut Agent, value: Value) -> JsResult<ObjectId> {
    let global = agent.intrinsics.global;
    if value.is_nullish() {
        return Ok(global);
    }
    match value.as_object() {
        Some(object) if object == global || object == agent.global_this() => Ok(object),
        _ => agent.type_error("expected a Window receiver"),
    }
}

fn with_scheduler<T>(
    agent: &mut Agent,
    operation: impl FnOnce(&mut Agent, &dyn WindowScheduler) -> JsResult<T>,
) -> JsResult<T> {
    with_page(agent, |agent, page| {
        let Some(scheduler) = &page.scheduler else {
            return agent.type_error("window scheduling is unavailable");
        };
        operation(agent, scheduler.as_ref())
    })
}

fn set_timer<const REPEAT: bool>(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
) -> JsResult<Value> {
    receiver(agent, this)?;
    let code = required_arg(agent, args, 0)?;
    let source = if agent.is_callable(code) {
        None
    } else {
        Some(coerce_string(agent, code)?)
    };
    // Coercing the delay can reenter script and collect the converted string.
    let root = source.map(|s| agent.root(Value::string(s))).transpose()?;
    let result = (|| {
        let delay = args.get(1).copied().unwrap_or(Value::UNDEFINED);
        let delay = to_long(agent, delay, IntegerConversion::Wrap)?.max(0);
        let delay = Duration::from_millis(u64::try_from(delay).expect("nonnegative delay"));
        let handler = match source {
            Some(source) => TimerHandler::Source(source),
            None => TimerHandler::Function(code, args.get(2..).unwrap_or_default()),
        };
        with_scheduler(agent, |agent, scheduler| {
            scheduler.set_timer(agent, handler, delay, REPEAT)
        })
        .map(|id| Value::number(f64::from(id)))
    })();
    if let Some(root) = root {
        agent.unroot(root);
    }
    result
}

fn clear_timer(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    receiver(agent, this)?;
    let id = to_long(
        agent,
        args.first().copied().unwrap_or(Value::UNDEFINED),
        IntegerConversion::Wrap,
    )?;
    with_scheduler(agent, |agent, scheduler| {
        scheduler.clear_timer(agent, id)?;
        Ok(Value::UNDEFINED)
    })
}

fn request_frame(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    receiver(agent, this)?;
    let callback = required_arg(agent, args, 0)?;
    if !agent.is_callable(callback) {
        return agent.type_error("animation frame callback must be callable");
    }
    with_scheduler(agent, |agent, scheduler| {
        scheduler.request_frame(agent, callback)
    })
    .map(|id| Value::number(f64::from(id)))
}

fn cancel_frame(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    receiver(agent, this)?;
    let id = required_arg(agent, args, 0)?;
    let id = to_unsigned_long(agent, id, IntegerConversion::Wrap)?;
    with_scheduler(agent, |agent, scheduler| {
        scheduler.cancel_frame(agent, id)?;
        Ok(Value::UNDEFINED)
    })
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
