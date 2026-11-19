//! Event handler IDL attributes ([HTML §8.1.8.1]): `onclick`, `onerror` and
//! the rest of a subset of `GlobalEventHandlers`, plus Window's
//! `onunhandledrejection` and `onrejectionhandled`.
//!
//! Assigning an object activates a handler: one listener record is appended
//! to the target's list at that moment, so a handler runs in the order it
//! was first set relative to `addEventListener` calls. Assigning `null`
//! (or any non-object) deactivates it and removes that record. The record's
//! callback slot holds the handler entry, whose current value is read when
//! the event runs.
//!
//! [HTML §8.1.8.1]: https://html.spec.whatwg.org/multipage/webappapis.html#event-handler-attributes

use rustic_js::runtime::{Agent, JsResult, NativeFn, ObjectId, PropertyDescriptor, Value};

use super::super::with_page;
use super::state::Event;
use super::{Listener, ListenerList, PrivateArray, Target, errors, receiver, type_key, with_root};

/// Handler names, in the order of [`GETTERS`] and [`SETTERS`]. The last
/// [`WINDOW_ONLY`] names exist only on Window.
const NAMES: [&str; 32] = [
    "abort",
    "blur",
    "cancel",
    "change",
    "click",
    "close",
    "contextmenu",
    "dblclick",
    "error",
    "focus",
    "input",
    "invalid",
    "keydown",
    "keypress",
    "keyup",
    "load",
    "mousedown",
    "mouseenter",
    "mouseleave",
    "mousemove",
    "mouseout",
    "mouseover",
    "mouseup",
    "reset",
    "resize",
    "scroll",
    "select",
    "submit",
    "toggle",
    "wheel",
    "rejectionhandled",
    "unhandledrejection",
];
const WINDOW_ONLY: usize = 2;

const GETTERS: [NativeFn; 32] = [
    get::<0>, get::<1>, get::<2>, get::<3>, get::<4>, get::<5>, get::<6>, get::<7>, get::<8>,
    get::<9>, get::<10>, get::<11>, get::<12>, get::<13>, get::<14>, get::<15>, get::<16>,
    get::<17>, get::<18>, get::<19>, get::<20>, get::<21>, get::<22>, get::<23>, get::<24>,
    get::<25>, get::<26>, get::<27>, get::<28>, get::<29>, get::<30>, get::<31>,
];
const SETTERS: [NativeFn; 32] = [
    set::<0>, set::<1>, set::<2>, set::<3>, set::<4>, set::<5>, set::<6>, set::<7>, set::<8>,
    set::<9>, set::<10>, set::<11>, set::<12>, set::<13>, set::<14>, set::<15>, set::<16>,
    set::<17>, set::<18>, set::<19>, set::<20>, set::<21>, set::<22>, set::<23>, set::<24>,
    set::<25>, set::<26>, set::<27>, set::<28>, set::<29>, set::<30>, set::<31>,
];

/// Handler entry slots: `[current value or null, active listener or null]`.
const VALUE: u32 = 0;
const LISTENER: u32 = 1;

/// Install the attributes on an interface prototype. Window also gets the
/// Window-only handlers.
pub(in crate::page) fn install(
    agent: &mut Agent,
    prototype: ObjectId,
    window: bool,
) -> JsResult<()> {
    let count = if window {
        NAMES.len()
    } else {
        NAMES.len() - WINDOW_ONLY
    };
    for index in 0..count {
        let name = format!("on{}", NAMES[index]);
        let get =
            Value::object(agent.new_native_function(GETTERS[index], &format!("get {name}"), 0)?);
        let set =
            Value::object(agent.new_native_function(SETTERS[index], &format!("set {name}"), 1)?);
        let key = agent.heap.key(&name)?;
        agent.define_property_or_throw(
            prototype,
            key,
            PropertyDescriptor::accessor(get, set, true, true),
        )?;
    }
    Ok(())
}

fn table(agent: &mut Agent, target: Value, create: bool) -> JsResult<Option<ObjectId>> {
    with_page(agent, |agent, page| {
        let existing = agent.weak_map_get(page.events.handlers, target);
        if let Some(table) = existing.as_object() {
            return Ok(Some(table));
        }
        if !create {
            return Ok(None);
        }
        // No prototype and only data properties: reads never run script.
        let table = agent.new_object_with(None)?;
        agent.weak_map_set(page.events.handlers, target, Value::object(table))?;
        Ok(Some(table))
    })
}

fn entry(agent: &mut Agent, table: ObjectId, name: &str) -> JsResult<Option<PrivateArray>> {
    let key = agent.heap.key(name)?;
    let value = agent.get(table, key)?;
    Ok(PrivateArray::from_value(value))
}

fn get<const INDEX: usize>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let target = receiver(agent, this)?;
    let Some(table) = table(agent, target.value, false)? else {
        return Ok(Value::NULL);
    };
    match entry(agent, table, NAMES[INDEX])? {
        Some(entry) => entry.get(agent, VALUE),
        None => Ok(Value::NULL),
    }
}

fn set<const INDEX: usize>(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let target = receiver(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    // [LegacyTreatNonObjectAsNull]
    let value = if value.as_object().is_some() {
        value
    } else {
        Value::NULL
    };
    with_root(agent, value, |agent| {
        let Some(table) = table(agent, target.value, value != Value::NULL)? else {
            return Ok(Value::UNDEFINED);
        };
        let existing = entry(agent, table, NAMES[INDEX])?;
        let entry = match existing {
            Some(entry) => entry,
            None if value == Value::NULL => return Ok(Value::UNDEFINED),
            None => {
                let entry = PrivateArray::new(agent, &[Value::NULL, Value::NULL])?;
                let key = agent.heap.key(NAMES[INDEX])?;
                agent.create_data_property_or_throw(table, key, entry.value())?;
                entry
            }
        };
        entry.set(agent, VALUE, value)?;
        let listener = Listener::from_value(entry.get(agent, LISTENER)?);
        match (value == Value::NULL, listener) {
            (true, Some(listener)) => {
                if let Some(list) = super::listener_list(agent, target.value)? {
                    list.remove(agent, listener)?;
                }
                entry.set(agent, LISTENER, Value::NULL)?;
            }
            (false, None) => activate(agent, target, entry, INDEX)?,
            _ => {}
        }
        Ok(Value::UNDEFINED)
    })
}

/// Append the handler's listener record to the target's list.
fn activate(agent: &mut Agent, target: Target, entry: PrivateArray, index: usize) -> JsResult<()> {
    let name = agent.string(NAMES[index])?;
    let key = type_key(agent, name)?;
    let listener = Listener::new(agent, key, entry.value(), Listener::HANDLER)?;
    with_root(agent, listener.0.value(), |agent| {
        match super::listener_list(agent, target.value)? {
            Some(list) => list.append(agent, listener)?,
            None => {
                let list = ListenerList::new(agent)?;
                list.append(agent, listener)?;
                with_page(agent, |agent, page| {
                    agent.weak_map_set(page.events.targets, target.value, list.0.value())
                })?;
            }
        }
        entry.set(agent, LISTENER, listener.0.value())
    })
}

/// HTML "the event handler processing algorithm" for an active handler.
pub(super) fn call(
    agent: &mut Agent,
    entry: Value,
    target: Value,
    value: Value,
    event: Event,
) -> JsResult<Value> {
    let entry = PrivateArray::from_value(entry).expect("handler entry");
    let handler = entry.get(agent, VALUE)?;
    if handler == Value::NULL || !agent.is_callable(handler) {
        // A non-callable object handler converts to undefined (Web IDL).
        return Ok(Value::UNDEFINED);
    }
    let window = with_page(agent, |_, page| Ok(page.window))?;
    let event_type = event.event_type(agent)?.as_string().expect("event type");
    let error_type = agent
        .heap
        .string(event_type)
        .units()
        .eq("error".encode_utf16());
    if target == window && error_type {
        if let Some(fields) = errors::error_event_fields(agent, value)? {
            // OnErrorEventHandler: (message, source, lineno, colno, error);
            // returning true cancels the event.
            let result = agent.call(handler, target, &fields)?;
            if result == Value::TRUE {
                event.prevent_default(agent)?;
            }
            return Ok(result);
        }
    }
    let result = agent.call(handler, target, &[value])?;
    if result == Value::FALSE {
        event.prevent_default(agent)?;
    }
    Ok(result)
}
