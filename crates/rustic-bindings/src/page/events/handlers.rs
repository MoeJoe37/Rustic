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
//! Content attributes (`onclick="…"`) on HTML elements, and the
//! Window-reflecting ones on the body element, set the same handlers: an
//! attribute is compiled into a function of `event` (Window's `onerror`:
//! `event, source, lineno, colno, error`) when its handler is next looked
//! at: read through the IDL attribute, before `addEventListener` appends to
//! the element, before an event's listeners run on it, and when an
//! attribute API changes it. The host's policy (CSP `script-src-attr`)
//! decides whether it compiles; a refused or unparsable attribute leaves a
//! null handler. Bodies compile in the global scope, without the element,
//! form owner and document scopes (TD-153).
//!
//! [HTML §8.1.8.1]: https://html.spec.whatwg.org/multipage/webappapis.html#event-handler-attributes

use std::rc::Rc;

use rustic_dom::{Document, HTML_NAMESPACE, NodeId, NodeKind};
use rustic_js::runtime::{Abrupt, Agent, JsResult, NativeFn, ObjectId, PropertyDescriptor, Value};

use super::super::with_page;
use super::state::Event;
use super::{Listener, ListenerList, PrivateArray, Target, errors, receiver, type_key, with_root};

/// Handler names, in the order of [`GETTERS`] and [`SETTERS`]. The last
/// [`WINDOW_ONLY`] names exist only on Window.
const NAMES: [&str; 42] = [
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
    "animationcancel",
    "animationend",
    "animationiteration",
    "animationstart",
    "transitioncancel",
    "transitionend",
    "transitionrun",
    "transitionstart",
    "rejectionhandled",
    "unhandledrejection",
    "finish",
    "remove",
];
/// Names before the Window-only ones (GlobalEventHandlers).
const GLOBAL: usize = 38;
const WINDOW_ONLY: usize = 2;
/// Handlers only Animation has (`onfinish`, `onremove`), after the
/// Window-only ones; `oncancel` is shared with the global set.
const ANIMATION: [usize; 3] = [2, GLOBAL + WINDOW_ONLY, GLOBAL + WINDOW_ONLY + 1];

const GETTERS: [NativeFn; 42] = [
    get::<0>, get::<1>, get::<2>, get::<3>, get::<4>, get::<5>, get::<6>, get::<7>, get::<8>,
    get::<9>, get::<10>, get::<11>, get::<12>, get::<13>, get::<14>, get::<15>, get::<16>,
    get::<17>, get::<18>, get::<19>, get::<20>, get::<21>, get::<22>, get::<23>, get::<24>,
    get::<25>, get::<26>, get::<27>, get::<28>, get::<29>, get::<30>, get::<31>, get::<32>,
    get::<33>, get::<34>, get::<35>, get::<36>, get::<37>, get::<38>, get::<39>, get::<40>,
    get::<41>,
];
const SETTERS: [NativeFn; 42] = [
    set::<0>, set::<1>, set::<2>, set::<3>, set::<4>, set::<5>, set::<6>, set::<7>, set::<8>,
    set::<9>, set::<10>, set::<11>, set::<12>, set::<13>, set::<14>, set::<15>, set::<16>,
    set::<17>, set::<18>, set::<19>, set::<20>, set::<21>, set::<22>, set::<23>, set::<24>,
    set::<25>, set::<26>, set::<27>, set::<28>, set::<29>, set::<30>, set::<31>, set::<32>,
    set::<33>, set::<34>, set::<35>, set::<36>, set::<37>, set::<38>, set::<39>, set::<40>,
    set::<41>,
];

/// Handler entry slots: `[current value or null, active listener or null,
/// the content attribute's value when last looked at (string) or null]`.
const VALUE: u32 = 0;
const LISTENER: u32 = 1;
const SEEN: u32 = 2;

/// Body content attributes that set Window's handlers (HTML "the
/// Window-reflecting body element event handler set"), among [`NAMES`].
const BODY_FORWARDED: [&str; 6] = ["blur", "error", "focus", "load", "resize", "scroll"];

/// The host's decision on compiling an event handler content attribute
/// (CSP `script-src-attr`), given its source text.
#[derive(Clone)]
pub struct HandlerPolicy(pub Rc<dyn Fn(&str) -> bool>);

impl std::fmt::Debug for HandlerPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HandlerPolicy")
    }
}

/// Install the attributes on an interface prototype. Window also gets the
/// Window-only handlers.
pub(in crate::page) fn install(
    agent: &mut Agent,
    prototype: ObjectId,
    window: bool,
) -> JsResult<()> {
    let count = if window { GLOBAL + WINDOW_ONLY } else { GLOBAL };
    define(agent, prototype, 0..count)
}

/// Install `onfinish`, `oncancel` and `onremove` on Animation's prototype.
pub(in crate::page) fn install_animation(agent: &mut Agent, prototype: ObjectId) -> JsResult<()> {
    define(agent, prototype, ANIMATION)
}

fn define(
    agent: &mut Agent,
    prototype: ObjectId,
    indices: impl IntoIterator<Item = usize>,
) -> JsResult<()> {
    for index in indices {
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

fn new_entry(agent: &mut Agent, table: ObjectId, index: usize) -> JsResult<PrivateArray> {
    let entry = PrivateArray::new(agent, &[Value::NULL, Value::NULL, Value::NULL])?;
    let key = agent.heap.key(NAMES[index])?;
    agent.create_data_property_or_throw(table, key, entry.value())?;
    Ok(entry)
}

fn get<const INDEX: usize>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let target = receiver(agent, this)?;
    sync(agent, target, INDEX)?;
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
    // An attribute change not yet seen happened before this assignment.
    sync(agent, target, INDEX)?;
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
            None => new_entry(agent, table, INDEX)?,
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

// ---- content attributes ------------------------------------------------------

/// The element whose `on…` content attribute sets `target`'s `name`
/// handler: the element itself (an HTML element other than body/frameset
/// for the forwarded names), or the body for Window.
fn attribute_owner(
    document: &Document,
    target: Option<NodeId>,
    window: bool,
    name: &str,
) -> Option<NodeId> {
    let html = |id: NodeId| match document.node(id).ok()?.kind() {
        NodeKind::Element(e) if e.name().namespace() == Some(HTML_NAMESPACE) => {
            Some(e.name().local().to_owned())
        }
        _ => None,
    };
    let forwarded = BODY_FORWARDED.contains(&name);
    if window {
        if !forwarded {
            return None;
        }
        let root = document.root();
        let element = document
            .children(root)
            .ok()?
            .find(|&c| html(c).as_deref() == Some("html"))?;
        return document
            .children(element)
            .ok()?
            .find(|&c| matches!(html(c).as_deref(), Some("body" | "frameset")));
    }
    let id = target?;
    match html(id)?.as_str() {
        "body" | "frameset" if forwarded => None,
        _ => Some(id),
    }
}

/// The `on{name}` attribute of the element that sets `target`'s handler.
fn content_source(agent: &mut Agent, target: Target, name: &str) -> JsResult<Option<String>> {
    let local = format!("on{name}");
    let id = if target.node {
        match super::super::traversal::node_of(agent, target.value) {
            Ok(id) => Some(id),
            Err(_) => return Ok(None),
        }
    } else {
        None
    };
    with_page(agent, |agent, page| {
        let window = target.value == page.window;
        if id.is_none() && !window {
            return Ok(None);
        }
        let document = id.map_or(page.main, NodeId::document);
        page.read(agent, document, |_, d| {
            let Some(owner) = attribute_owner(d, id, window, name) else {
                return Ok(None);
            };
            let Ok(NodeKind::Element(e)) = d.node(owner).map(|n| n.kind()) else {
                return Ok(None);
            };
            Ok(e.attributes()
                .iter()
                .find(|a| a.name().namespace().is_none() && a.name().local() == local)
                .map(|a| String::from_utf16_lossy(a.value().as_units())))
        })
    })
}

/// Bring `target`'s `NAMES[index]` handler up to date with its content
/// attribute, if that changed since it was last looked at.
pub(super) fn sync(agent: &mut Agent, target: Target, index: usize) -> JsResult<()> {
    if index >= GLOBAL {
        return Ok(());
    }
    let name = NAMES[index];
    let source = content_source(agent, target, name)?;
    let Some(table) = table(agent, target.value, source.is_some())? else {
        return Ok(());
    };
    let entry = match entry(agent, table, name)? {
        Some(entry) => entry,
        None if source.is_none() => return Ok(()),
        None => new_entry(agent, table, index)?,
    };
    let seen = entry.get(agent, SEEN)?;
    let seen = match seen.as_string() {
        Some(seen) => Some(agent.heap.string(seen).to_text()),
        None => None,
    };
    if seen == source {
        return Ok(());
    }
    let seen = match &source {
        Some(text) => agent.string(text)?,
        None => Value::NULL,
    };
    entry.set(agent, SEEN, seen)?;
    let value = match source {
        Some(text) => compile(agent, target, name, &text)?,
        None => Value::NULL,
    };
    with_root(agent, value, |agent| {
        entry.set(agent, VALUE, value)?;
        let listener = Listener::from_value(entry.get(agent, LISTENER)?);
        match (value == Value::NULL, listener) {
            (true, Some(listener)) => {
                if let Some(list) = super::listener_list(agent, target.value)? {
                    list.remove(agent, listener)?;
                }
                entry.set(agent, LISTENER, Value::NULL)
            }
            (false, None) => activate(agent, target, entry, index),
            _ => Ok(()),
        }
    })
}

/// Compile an attribute's body, or null when the policy refuses it or it
/// does not parse (the error is reported, HTML §8.1.8.1 step 3.9).
fn compile(agent: &mut Agent, target: Target, name: &str, body: &str) -> JsResult<Value> {
    let (window, policy) = with_page(agent, |_, page| {
        Ok((page.window, page.events.handler_policy.borrow().clone()))
    })?;
    if policy.is_some_and(|policy| !(policy.0)(body)) {
        return Ok(Value::NULL);
    }
    let params = if target.value == window && name == "error" {
        "event, source, lineno, colno, error"
    } else {
        "event"
    };
    match agent.compile_event_handler(params, body, &format!("on{name}")) {
        Ok(function) => Ok(function),
        Err(Abrupt::Throw(error)) => {
            super::report_callback_error(agent, error)?;
            Ok(Value::NULL)
        }
        Err(error) => Err(error),
    }
}

/// Sync the handler for `event_type` before `target`'s listeners run.
pub(super) fn sync_type(agent: &mut Agent, target: Value, event_type: Value) -> JsResult<()> {
    let Some(kind) = event_type.as_string() else {
        return Ok(());
    };
    let index = NAMES[..GLOBAL]
        .iter()
        .position(|name| agent.heap.string(kind).units().eq(name.encode_utf16()));
    let Some(index) = index else {
        return Ok(());
    };
    let target = receiver(agent, target)?;
    sync(agent, target, index)
}

/// Sync every handler whose content attribute `target` has (before a
/// listener is appended, so attribute handlers keep their place).
pub(super) fn sync_all(agent: &mut Agent, target: Target) -> JsResult<()> {
    for index in 0..GLOBAL {
        agent.check_native_work()?;
        sync(agent, target, index)?;
    }
    Ok(())
}

/// An attribute API changed `local` on `this`: sync its handler.
pub(in crate::page) fn attribute_changed(
    agent: &mut Agent,
    this: Value,
    local: &str,
) -> JsResult<()> {
    let Some(name) = local.strip_prefix("on") else {
        return Ok(());
    };
    let Some(index) = NAMES[..GLOBAL].iter().position(|n| *n == name) else {
        return Ok(());
    };
    let target = receiver(agent, this)?;
    sync(agent, target, index)?;
    // A body attribute sets Window's handler.
    if BODY_FORWARDED.contains(&name) {
        let window = with_page(agent, |_, page| Ok(page.window))?;
        let window = receiver(agent, window)?;
        sync(agent, window, index)?;
    }
    Ok(())
}
