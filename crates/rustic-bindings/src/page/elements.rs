//! HTML interactive elements: `details`/`summary` (§4.11.1–2), `dialog`
//! (§4.11.4) and the popover API (§6.12).
//!
//! The `open` content attribute is the source of truth for details and
//! dialogs. Modal dialogs and showing popovers are in their document's top
//! layer ([`rustic_dom::Document::top_layer_add`]), which `:modal` and
//! `:popover-open` read. Content outside the topmost modal dialog, and
//! under an `inert` attribute, is inert ([`is_inert`]): it is not focusable
//! and clicks on it land on the dialog. Opening runs the dialog and popover
//! focusing steps and closing restores the previously focused element.
//! Escape is a close request ([`close_request`]); a press outside auto
//! popovers, or on the backdrop of a `closedby=any` dialog, light-dismisses
//! them ([`light_dismiss`]). Setting or removing `open` and `popover` with
//! the attribute APIs runs their attribute change steps
//! ([`attribute_changed`]). `toggle` events coalesce per element through a
//! toggle task tracker. `toggle` and `close` are queued as microtasks
//! rather than DOM manipulation tasks, and `::backdrop` is not generated
//! (TD-216).

use rustic_dom::{DomString, Element, NodeId, NodeKind, QualifiedName, TopLayer};
use rustic_js::runtime::{Abrupt, Agent, JsResult, NativeFn, ObjectId, PropertyDescriptor, Value};

use super::events::{PrivateArray, define_method, fire_at, fire_plain, toggle_event, with_root};
use super::traversal::{node_of, read_doc, wrap_node};
use super::{
    creation, dom_exception, illegal_constructor, install_interface, prototype, with_mut_tree,
    with_page,
};

const DETAILS: &str = "HTMLDetailsElement";
const DIALOG: &str = "HTMLDialogElement";

/// Page-lifetime state: dialog return values, toggle task trackers,
/// previously focused elements and the queued-event callback.
#[derive(Debug)]
pub(super) struct Elements {
    /// Dialog → its `returnValue` string.
    return_values: ObjectId,
    /// Element → its pending toggle job (HTML "toggle task tracker").
    toggle_tasks: ObjectId,
    /// Dialog or popover → the element focused before it opened.
    previous_focus: ObjectId,
    /// The native job that fires queued events.
    fire_job: Value,
}

impl Elements {
    pub(super) fn new(agent: &mut Agent) -> JsResult<Self> {
        let return_values = agent.new_weak_map()?;
        agent.root(Value::object(return_values))?;
        let toggle_tasks = agent.new_weak_map()?;
        agent.root(Value::object(toggle_tasks))?;
        let previous_focus = agent.new_weak_map()?;
        agent.root(Value::object(previous_focus))?;
        let fire_job = Value::object(agent.new_native_function(fire_queued, "fire queued", 1)?);
        agent.root(fire_job)?;
        Ok(Self {
            return_values,
            toggle_tasks,
            previous_focus,
            fire_job,
        })
    }
}

fn exception(agent: &mut Agent, name: &str, message: &str) -> Abrupt {
    creation::exception(agent, name, message)
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        DETAILS,
        Some("HTMLElement"),
        illegal_constructor,
        0,
        &[],
        |agent, proto| {
            accessor(agent, proto, "open", details_open, Some(set_details_open))?;
            accessor(agent, proto, "name", name, Some(set_name))
        },
    )?;
    install_interface(
        agent,
        DIALOG,
        Some("HTMLElement"),
        illegal_constructor,
        0,
        &[],
        |agent, proto| {
            accessor(agent, proto, "open", dialog_open, Some(set_dialog_open))?;
            accessor(
                agent,
                proto,
                "returnValue",
                return_value,
                Some(set_return_value),
            )?;
            define_method(agent, proto, "show", show, 0)?;
            define_method(agent, proto, "showModal", show_modal, 0)?;
            define_method(agent, proto, "close", close, 0)?;
            define_method(agent, proto, "requestClose", request_close, 0)
        },
    )?;
    let html = prototype(agent, "HTMLElement")?;
    accessor(agent, html, "popover", popover, Some(set_popover))?;
    accessor(agent, html, "inert", reflect_bool::<0>, Some(set_bool::<0>))?;
    accessor(
        agent,
        html,
        "autofocus",
        reflect_bool::<1>,
        Some(set_bool::<1>),
    )?;
    define_method(agent, html, "showPopover", show_popover, 0)?;
    define_method(agent, html, "hidePopover", hide_popover, 0)?;
    define_method(agent, html, "togglePopover", toggle_popover, 0)
}

fn accessor(
    agent: &mut Agent,
    proto: ObjectId,
    name: &str,
    get: NativeFn,
    set: Option<NativeFn>,
) -> JsResult<()> {
    let getter = Value::object(agent.new_native_function(get, &format!("get {name}"), 0)?);
    let setter = match set {
        Some(set) => Value::object(agent.new_native_function(set, &format!("set {name}"), 1)?),
        None => Value::UNDEFINED,
    };
    let key = agent.heap.key(name)?;
    agent.define_property_or_throw(
        proto,
        key,
        PropertyDescriptor::accessor(getter, setter, true, true),
    )
}

/// The HTML element behind `this` and its local name.
fn html_element(agent: &mut Agent, this: Value) -> JsResult<(NodeId, String)> {
    let id = node_of(agent, this)?;
    let name = read_doc(agent, id, |d| match d.node(id).map(|n| n.kind()) {
        Ok(NodeKind::Element(e)) if e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE) => {
            Some(e.name().local().to_owned())
        }
        _ => None,
    })?;
    match name {
        Some(name) => Ok((id, name)),
        None => agent.type_error("expected an HTML element receiver"),
    }
}

fn element_of(document: &rustic_dom::Document, id: NodeId) -> Option<&Element> {
    match document.node(id).ok()?.kind() {
        NodeKind::Element(e) => Some(e),
        _ => None,
    }
}

fn attribute(agent: &mut Agent, id: NodeId, name: &str) -> JsResult<Option<String>> {
    read_doc(agent, id, |d| {
        element_of(d, id).and_then(|e| {
            e.attributes()
                .iter()
                .find(|a| a.name().namespace().is_none() && a.name().local() == name)
                .map(|a| String::from_utf16_lossy(a.value().as_units()))
        })
    })
}

fn set_attribute(agent: &mut Agent, this: Value, name: &str, value: Option<&str>) -> JsResult<()> {
    with_mut_tree(agent, this, |agent, document, _, id| {
        let result = match value {
            Some(value) => {
                let qualified = QualifiedName::from_parsed(None, None, name)
                    .map_err(|e| dom_exception::from_dom_error(agent, e))?;
                document
                    .set_attribute(id, qualified, DomString::from(value))
                    .map(|_| ())
            }
            None => document.remove_attribute(id, None, name).map(|_| ()),
        };
        result.map_err(|e| dom_exception::from_dom_error(agent, e))
    })
}

fn text(agent: &mut Agent, value: &str) -> JsResult<Value> {
    Ok(Value::string(agent.heap.intern_str(value)?))
}

fn string_arg(agent: &mut Agent, value: Value) -> JsResult<String> {
    let units = creation::string(agent, value)?;
    Ok(String::from_utf16_lossy(units.as_units()))
}

/// Queue `event` to fire at `target` in a microtask.
fn queue(agent: &mut Agent, target: Value, event: Value) -> JsResult<()> {
    let job = PrivateArray::new(agent, &[target, event])?;
    let fire_job = with_page(agent, |_, page| Ok(page.elements.fire_job))?;
    agent
        .enqueue_microtask(fire_job, job.value())
        .map_err(|_| Abrupt::OutOfMemory)
}

/// Toggle jobs are `[target, event, canceled, oldState]`; others are
/// `[target, event]`.
const TOGGLE_JOB_LEN: u32 = 4;

fn fire_queued(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(job) = args.first().copied().and_then(PrivateArray::from_value) else {
        return Ok(Value::UNDEFINED);
    };
    let target = job.get(agent, 0)?;
    let event = job.get(agent, 1)?;
    if job.len(agent)? == TOGGLE_JOB_LEN {
        if job.get(agent, 2)? == Value::boolean(true) {
            return Ok(Value::UNDEFINED);
        }
        with_page(agent, |agent, page| {
            if agent.weak_map_get(page.elements.toggle_tasks, target) == job.value() {
                agent.weak_map_delete(page.elements.toggle_tasks, target);
            }
            Ok(())
        })?;
    }
    match fire_at(agent, target, event) {
        Ok(_) => Ok(Value::UNDEFINED),
        Err(Abrupt::Throw(error)) => {
            super::events::report_callback_error(agent, error)?;
            Ok(Value::UNDEFINED)
        }
        Err(error) => Err(error),
    }
}

/// Queue a `toggle` event, coalescing with a still-pending one: the
/// pending job is canceled and the new event keeps its `oldState`.
fn queue_toggle(agent: &mut Agent, target: Value, old: &str, new: &str) -> JsResult<()> {
    let tasks = with_page(agent, |_, page| Ok(page.elements.toggle_tasks))?;
    let mut old = old.to_owned();
    if let Some(pending) = PrivateArray::from_value(agent.weak_map_get(tasks, target)) {
        pending.set(agent, 2, Value::boolean(true))?;
        let previous = pending.get(agent, 3)?;
        old = string_arg(agent, previous)?;
    }
    let event = toggle_event(agent, "toggle", false, &old, new)?;
    with_root(agent, event, |agent| {
        let old = agent.string(&old)?;
        let job = PrivateArray::new(agent, &[target, event, Value::boolean(false), old])?;
        agent.weak_map_set(tasks, target, job.value())?;
        let fire_job = with_page(agent, |_, page| Ok(page.elements.fire_job))?;
        agent
            .enqueue_microtask(fire_job, job.value())
            .map_err(|_| Abrupt::OutOfMemory)
    })
}

// ---- reflection --------------------------------------------------------------

const BOOLEANS: [&str; 2] = ["inert", "autofocus"];

/// A reflected boolean content attribute of every HTML element.
fn reflect_bool<const N: usize>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (id, _) = html_element(agent, this)?;
    Ok(Value::boolean(attribute(agent, id, BOOLEANS[N])?.is_some()))
}

fn set_bool<const N: usize>(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    html_element(agent, this)?;
    let on = args.first().is_some_and(|&v| agent.to_boolean(v));
    set_attribute(agent, this, BOOLEANS[N], on.then_some(""))?;
    Ok(Value::UNDEFINED)
}

// ---- inertness and focus -----------------------------------------------------

/// Whether `id` is inert: under an HTML `inert` attribute, or blocked by
/// the topmost modal dialog (HTML §6.3, "blocked by a modal dialog").
pub(in crate::page) fn is_inert(document: &rustic_dom::Document, id: NodeId) -> bool {
    let mut at = Some(id);
    while let Some(current) = at {
        let Ok(node) = document.node(current) else {
            return false;
        };
        if let NodeKind::Element(e) = node.kind() {
            if e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE) && has(e, "inert") {
                return true;
            }
        }
        at = node.parent();
    }
    blocking_modal(document, id).is_some()
}

/// The modal dialog blocking `id`: the topmost one, unless `id` is inside
/// it or inside a top-layer element above it.
fn blocking_modal(document: &rustic_dom::Document, id: NodeId) -> Option<NodeId> {
    let layer = document.top_layer();
    let index = layer
        .iter()
        .rposition(|&(_, kind)| kind == TopLayer::Modal)?;
    let inside = layer[index..]
        .iter()
        .any(|&(top, _)| document.is_inclusive_descendant(id, top));
    (!inside).then_some(layer[index].0)
}

/// The focus delegate of a dialog or popover: its first focusable
/// descendant with `autofocus`, else (unless `autofocus_only`) its first
/// focusable descendant (HTML "focus delegate").
fn focus_delegate(agent: &mut Agent, id: NodeId, autofocus_only: bool) -> JsResult<Option<NodeId>> {
    let candidates = read_doc(agent, id, |d| {
        let mut out = Vec::new();
        if let Ok(walk) = d.walk(id) {
            for (other, _) in walk.skip(1) {
                if let Some(e) = element_of(d, other) {
                    if e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE) {
                        out.push((other, has(e, "autofocus")));
                    }
                }
            }
        }
        out
    })?;
    let mut first = None;
    for (candidate, autofocus) in candidates {
        agent.check_native_work()?;
        let focusable = with_page(agent, |agent, page| {
            page.read(agent, candidate.document(), |agent, d| {
                super::events::is_focusable(agent, d, candidate)
            })
        })?;
        if !focusable {
            continue;
        }
        if autofocus {
            return Ok(Some(candidate));
        }
        first = first.or(Some(candidate));
    }
    Ok(if autofocus_only { None } else { first })
}

fn focus_node(agent: &mut Agent, id: NodeId) -> JsResult<bool> {
    let wrapper = wrap_node(agent, id)?;
    super::events::focus_element(agent, wrapper)
}

/// Remember the focused element before `this` opens.
fn remember_focus(agent: &mut Agent, this: Value) -> JsResult<()> {
    let focused = super::events::focused_value(agent)?.unwrap_or(Value::NULL);
    with_page(agent, |agent, page| {
        agent.weak_map_set(page.elements.previous_focus, this, focused)
    })
}

/// Return focus to the element focused before `this` opened when focus is
/// inside it (or `always`, for a modal dialog).
fn restore_focus(agent: &mut Agent, this: Value, id: NodeId, always: bool) -> JsResult<()> {
    let previous = with_page(agent, |agent, page| {
        let previous = agent.weak_map_get(page.elements.previous_focus, this);
        agent.weak_map_delete(page.elements.previous_focus, this);
        Ok(previous)
    })?;
    if previous.as_object().is_none() {
        return Ok(());
    }
    let focused = with_page(agent, |_, page| Ok(page.events.forms.focused()))?;
    let inside = match focused {
        Some(focused) if focused.document() == id.document() => {
            read_doc(agent, id, |d| d.is_inclusive_descendant(focused, id))?
        }
        _ => false,
    };
    if always || inside {
        with_root(agent, previous, |agent| {
            super::events::focus_element(agent, previous).map(drop)
        })?;
    }
    Ok(())
}

// ---- details ---------------------------------------------------------------

fn details(agent: &mut Agent, this: Value) -> JsResult<NodeId> {
    match html_element(agent, this)? {
        (id, name) if name == "details" => Ok(id),
        _ => agent.type_error("expected an HTMLDetailsElement receiver"),
    }
}

fn details_open(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let id = details(agent, this)?;
    Ok(Value::boolean(attribute(agent, id, "open")?.is_some()))
}

fn set_details_open(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    details(agent, this)?;
    let open = args.first().is_some_and(|&v| agent.to_boolean(v));
    set_details_state(agent, this, open)?;
    Ok(Value::UNDEFINED)
}

/// Open or close a details element through its `open` attribute.
fn set_details_state(agent: &mut Agent, this: Value, open: bool) -> JsResult<()> {
    let id = node_of(agent, this)?;
    let was_open = attribute(agent, id, "open")?.is_some();
    if was_open == open {
        return Ok(());
    }
    set_attribute(agent, this, "open", open.then_some(""))?;
    details_open_changed(agent, this, id, open)
}

/// The details `open` attribute change steps (HTML §4.11.1): an opened
/// element closes the others of its name group, then `toggle` is queued.
fn details_open_changed(agent: &mut Agent, this: Value, id: NodeId, open: bool) -> JsResult<()> {
    if open {
        // Exclusive accordions: other open details with the same name in
        // the same tree close.
        if let Some(group) = attribute(agent, id, "name")?.filter(|n| !n.is_empty()) {
            let others = read_doc(agent, id, |d| {
                let root = super::ranges::root_of(d, id);
                d.walk(root)
                    .map(|walk| {
                        walk.filter(|&(other, _)| {
                            other != id
                                && element_of(d, other).is_some_and(|e| {
                                    e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE)
                                        && e.name().local() == "details"
                                        && has(e, "open")
                                        && value_of(e, "name").as_deref() == Some(group.as_str())
                                })
                        })
                        .map(|(other, _)| other)
                        .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })?;
            for other in others {
                let wrapper = wrap_node(agent, other)?;
                set_details_state(agent, wrapper, false)?;
            }
        }
    }
    let (old, new) = if open {
        ("closed", "open")
    } else {
        ("open", "closed")
    };
    queue_toggle(agent, this, old, new)
}

// ---- attribute change steps ----------------------------------------------------

/// An attribute the change steps watch, with its value before the change,
/// or an event handler attribute (`on…`) by name.
pub(in crate::page) struct Watched(
    Option<(NodeId, &'static str, Option<String>)>,
    Option<String>,
);

/// Call before an attribute API sets or removes `local` (no namespace) on
/// `id`; pass the result to [`attribute_changed`] afterwards.
pub(in crate::page) fn watch_attribute(
    agent: &mut Agent,
    id: NodeId,
    namespaced: bool,
    local: &str,
) -> JsResult<Watched> {
    let lower = local.to_ascii_lowercase();
    let name = match lower.as_str() {
        _ if namespaced => return Ok(Watched(None, None)),
        "open" => "open",
        "popover" => "popover",
        handler if handler.starts_with("on") => return Ok(Watched(None, Some(lower))),
        _ => return Ok(Watched(None, None)),
    };
    Ok(Watched(Some((id, name, attribute(agent, id, name)?)), None))
}

/// Run the `open` (details, dialog) and `popover` attribute change steps
/// after an attribute API changed a watched attribute.
pub(in crate::page) fn attribute_changed(
    agent: &mut Agent,
    this: Value,
    watched: Watched,
) -> JsResult<()> {
    if let Some(handler) = watched.1 {
        return super::events::handler_attribute_changed(agent, this, &handler);
    }
    let Some((id, name, before)) = watched.0 else {
        return Ok(());
    };
    let local = read_doc(agent, id, |d| {
        element_of(d, id)
            .filter(|e| e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE))
            .map(|e| e.name().local().to_owned())
    })?;
    let Some(local) = local else {
        return Ok(());
    };
    let now = attribute(agent, id, name)?;
    match (local.as_str(), name) {
        ("details", "open") if before.is_some() != now.is_some() => {
            details_open_changed(agent, this, id, now.is_some())
        }
        ("dialog", "open") if before.is_some() && now.is_none() => {
            top_layer_remove(agent, id).map(drop)
        }
        (_, "popover")
            if popover_state(before.as_deref()) != popover_state(now.as_deref())
                && top_layer_state(agent, id)? == Some(TopLayer::Popover) =>
        {
            // Hide without events, returning focus (HTML §6.12.2).
            top_layer_remove(agent, id)?;
            restore_focus(agent, this, id, false)
        }
        _ => Ok(()),
    }
}

fn has(element: &Element, name: &str) -> bool {
    value_of(element, name).is_some()
}

fn value_of(element: &Element, name: &str) -> Option<String> {
    element
        .attributes()
        .iter()
        .find(|a| a.name().namespace().is_none() && a.name().local() == name)
        .map(|a| String::from_utf16_lossy(a.value().as_units()))
}

fn name(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let id = details(agent, this)?;
    let value = attribute(agent, id, "name")?.unwrap_or_default();
    text(agent, &value)
}

fn set_name(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    details(agent, this)?;
    let value = super::required_arg(agent, args, 0)?;
    let value = string_arg(agent, value)?;
    set_attribute(agent, this, "name", Some(&value))?;
    Ok(Value::UNDEFINED)
}

// ---- dialog ----------------------------------------------------------------

fn dialog(agent: &mut Agent, this: Value) -> JsResult<NodeId> {
    match html_element(agent, this)? {
        (id, name) if name == "dialog" => Ok(id),
        _ => agent.type_error("expected an HTMLDialogElement receiver"),
    }
}

fn dialog_open(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let id = dialog(agent, this)?;
    Ok(Value::boolean(attribute(agent, id, "open")?.is_some()))
}

fn set_dialog_open(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    dialog(agent, this)?;
    let open = args.first().is_some_and(|&v| agent.to_boolean(v));
    set_attribute(agent, this, "open", open.then_some(""))?;
    if !open {
        let id = node_of(agent, this)?;
        top_layer_remove(agent, id)?;
    }
    Ok(Value::UNDEFINED)
}

fn return_value(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    dialog(agent, this)?;
    let value = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.elements.return_values, this))
    })?;
    if value == Value::UNDEFINED {
        return text(agent, "");
    }
    Ok(value)
}

fn store_return_value(agent: &mut Agent, this: Value, value: Value) -> JsResult<()> {
    with_page(agent, |agent, page| {
        agent.weak_map_set(page.elements.return_values, this, value)
    })
}

fn set_return_value(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    dialog(agent, this)?;
    let value = super::required_arg(agent, args, 0)?;
    let value = Value::string(agent.to_string(value)?);
    store_return_value(agent, this, value)?;
    Ok(Value::UNDEFINED)
}

fn top_layer_state(agent: &mut Agent, id: NodeId) -> JsResult<Option<TopLayer>> {
    read_doc(agent, id, |d| d.top_layer_state(id))
}

fn top_layer_add(agent: &mut Agent, id: NodeId, kind: TopLayer) -> JsResult<()> {
    with_page(agent, |agent, page| {
        page.write(agent, id.document(), |agent, d| {
            d.top_layer_add(id, kind)
                .map_err(|e| dom_exception::from_dom_error(agent, e))
        })
    })
}

fn top_layer_remove(agent: &mut Agent, id: NodeId) -> JsResult<bool> {
    with_page(agent, |agent, page| {
        page.write(agent, id.document(), |_, d| Ok(d.top_layer_remove(id)))
    })
}

fn connected(agent: &mut Agent, id: NodeId) -> JsResult<bool> {
    read_doc(agent, id, |d| d.is_inclusive_descendant(id, d.root()))
}

/// `show()` and `showModal()`.
fn open_dialog(agent: &mut Agent, this: Value, modal: bool) -> JsResult<Value> {
    let id = dialog(agent, this)?;
    let open = attribute(agent, id, "open")?.is_some();
    let is_modal = top_layer_state(agent, id)? == Some(TopLayer::Modal);
    if open && is_modal == modal {
        return Ok(Value::UNDEFINED);
    }
    if open {
        return Err(exception(
            agent,
            "InvalidStateError",
            "the dialog is already open in the other mode",
        ));
    }
    if modal && !connected(agent, id)? {
        return Err(exception(
            agent,
            "InvalidStateError",
            "the dialog is not connected",
        ));
    }
    if top_layer_state(agent, id)? == Some(TopLayer::Popover) {
        return Err(exception(
            agent,
            "InvalidStateError",
            "the dialog is a showing popover",
        ));
    }
    let before = toggle_event(agent, "beforetoggle", true, "closed", "open")?;
    if !fire_at(agent, this, before)? {
        return Ok(Value::UNDEFINED);
    }
    if attribute(agent, id, "open")?.is_some() {
        return Ok(Value::UNDEFINED);
    }
    // Remember focus before the modal dialog makes the rest inert.
    remember_focus(agent, this)?;
    set_attribute(agent, this, "open", Some(""))?;
    if modal {
        top_layer_add(agent, id, TopLayer::Modal)?;
    }
    // The dialog focusing steps: the focus delegate, else (modal) the
    // viewport, since everything outside the dialog is now inert.
    match focus_delegate(agent, id, false)? {
        Some(control) => {
            focus_node(agent, control)?;
        }
        None if modal => super::events::blur_focus(agent)?,
        None => {}
    }
    queue_toggle(agent, this, "closed", "open")?;
    Ok(Value::UNDEFINED)
}

fn show(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    open_dialog(agent, this, false)
}

fn show_modal(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    open_dialog(agent, this, true)
}

/// "Close the dialog" with an optional result.
fn close_dialog(agent: &mut Agent, this: Value, result: Option<Value>) -> JsResult<()> {
    let id = dialog(agent, this)?;
    if attribute(agent, id, "open")?.is_none() {
        return Ok(());
    }
    let before = toggle_event(agent, "beforetoggle", false, "open", "closed")?;
    fire_at(agent, this, before)?;
    if attribute(agent, id, "open")?.is_none() {
        return Ok(());
    }
    let was_modal = top_layer_state(agent, id)? == Some(TopLayer::Modal);
    set_attribute(agent, this, "open", None)?;
    top_layer_remove(agent, id)?;
    if let Some(result) = result {
        store_return_value(agent, this, result)?;
    }
    restore_focus(agent, this, id, was_modal)?;
    queue_toggle(agent, this, "open", "closed")?;
    let close = super::events::state_event(agent, "close", false)?;
    with_root(agent, close, |agent| queue(agent, this, close))
}

fn result_arg(agent: &mut Agent, args: &[Value]) -> JsResult<Option<Value>> {
    match args.first().copied() {
        None | Some(Value::UNDEFINED) => Ok(None),
        Some(value) => Ok(Some(Value::string(agent.to_string(value)?))),
    }
}

fn close(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    dialog(agent, this)?;
    let result = result_arg(agent, args)?;
    close_dialog(agent, this, result)?;
    Ok(Value::UNDEFINED)
}

fn request_close(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = dialog(agent, this)?;
    let result = result_arg(agent, args)?;
    if attribute(agent, id, "open")?.is_none() {
        return Ok(Value::UNDEFINED);
    }
    if fire_plain(agent, this, "cancel", true)? {
        close_dialog(agent, this, result)?;
    }
    Ok(Value::UNDEFINED)
}

// ---- popover ---------------------------------------------------------------

/// The `popover` attribute's state: `auto`, `manual`, `hint`, or none.
fn popover_state(value: Option<&str>) -> Option<&'static str> {
    let value = value?;
    Some(match value.to_ascii_lowercase().as_str() {
        "" | "auto" => "auto",
        "hint" => "hint",
        _ => "manual",
    })
}

fn popover(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (id, _) = html_element(agent, this)?;
    match popover_state(attribute(agent, id, "popover")?.as_deref()) {
        Some(state) => text(agent, state),
        None => Ok(Value::NULL),
    }
}

fn set_popover(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    html_element(agent, this)?;
    let value = super::required_arg(agent, args, 0)?;
    if value == Value::NULL {
        set_attribute(agent, this, "popover", None)?;
    } else {
        let value = string_arg(agent, value)?;
        set_attribute(agent, this, "popover", Some(&value))?;
    }
    Ok(Value::UNDEFINED)
}

/// HTML "check popover validity": whether the element may change to the
/// expected state (`Ok(false)` when it already is in the other one).
fn check_popover(agent: &mut Agent, id: NodeId, expect_showing: bool) -> JsResult<bool> {
    if attribute(agent, id, "popover")?.is_none() {
        return Err(exception(
            agent,
            "NotSupportedError",
            "the element is not a popover",
        ));
    }
    let showing = top_layer_state(agent, id)? == Some(TopLayer::Popover);
    if showing != expect_showing {
        return Ok(false);
    }
    if !connected(agent, id)? {
        return Err(exception(
            agent,
            "InvalidStateError",
            "the popover is not connected",
        ));
    }
    let modal_dialog = top_layer_state(agent, id)? == Some(TopLayer::Modal);
    if modal_dialog {
        return Err(exception(
            agent,
            "InvalidStateError",
            "the popover is a modal dialog",
        ));
    }
    Ok(true)
}

/// Hide every showing `auto` popover that is not an inclusive ancestor of
/// `keep` (a simplified "hide all popovers until").
fn hide_other_auto(agent: &mut Agent, keep: NodeId) -> JsResult<()> {
    let others = read_doc(agent, keep, |d| {
        d.top_layer()
            .iter()
            .filter(|&&(id, kind)| {
                kind == TopLayer::Popover
                    && id != keep
                    && !d.is_inclusive_descendant(keep, id)
                    && element_of(d, id).is_some_and(|e| {
                        popover_state(value_of(e, "popover").as_deref()) == Some("auto")
                    })
            })
            .map(|&(id, _)| id)
            .collect::<Vec<_>>()
    })?;
    for other in others {
        let wrapper = wrap_node(agent, other)?;
        hide(agent, wrapper, other)?;
    }
    Ok(())
}

fn show_inner(agent: &mut Agent, this: Value, id: NodeId) -> JsResult<()> {
    if !check_popover(agent, id, false)? {
        return Ok(());
    }
    let before = toggle_event(agent, "beforetoggle", true, "closed", "open")?;
    if !fire_at(agent, this, before)? {
        return Ok(());
    }
    if !check_popover(agent, id, false)? {
        return Ok(());
    }
    let state = popover_state(attribute(agent, id, "popover")?.as_deref());
    if state == Some("auto") {
        hide_other_auto(agent, id)?;
    }
    if state != Some("manual") {
        remember_focus(agent, this)?;
    }
    top_layer_add(agent, id, TopLayer::Popover)?;
    // The popover focusing steps: only an `autofocus` delegate (or the
    // popover itself with `autofocus`) takes focus.
    let own = attribute(agent, id, "autofocus")?.is_some();
    let control = if own {
        Some(id)
    } else {
        focus_delegate(agent, id, true)?
    };
    if let Some(control) = control {
        focus_node(agent, control)?;
    }
    queue_toggle(agent, this, "closed", "open")
}

fn hide(agent: &mut Agent, this: Value, id: NodeId) -> JsResult<()> {
    if !check_popover(agent, id, true)? {
        return Ok(());
    }
    let before = toggle_event(agent, "beforetoggle", false, "open", "closed")?;
    fire_at(agent, this, before)?;
    if top_layer_remove(agent, id)? {
        restore_focus(agent, this, id, false)?;
        queue_toggle(agent, this, "open", "closed")?;
    }
    Ok(())
}

fn show_popover(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (id, _) = html_element(agent, this)?;
    show_inner(agent, this, id)?;
    Ok(Value::UNDEFINED)
}

fn hide_popover(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (id, _) = html_element(agent, this)?;
    hide(agent, this, id)?;
    Ok(Value::UNDEFINED)
}

/// `togglePopover(force)` (a boolean or `{force}`): whether it now shows.
fn toggle_popover(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, _) = html_element(agent, this)?;
    let force = match args.first().copied() {
        None | Some(Value::UNDEFINED) => None,
        Some(value) => match value.as_object() {
            Some(options) if !agent.is_callable(value) => {
                let key = agent.heap.key("force")?;
                let force = agent.get(options, key)?;
                (force != Value::UNDEFINED).then(|| agent.to_boolean(force))
            }
            _ => Some(agent.to_boolean(value)),
        },
    };
    let showing = top_layer_state(agent, id)? == Some(TopLayer::Popover);
    if showing && force != Some(true) {
        hide(agent, this, id)?;
    } else if !showing && force != Some(false) {
        show_inner(agent, this, id)?;
    } else if attribute(agent, id, "popover")?.is_none() {
        return Err(exception(
            agent,
            "NotSupportedError",
            "the element is not a popover",
        ));
    }
    Ok(Value::boolean(
        top_layer_state(agent, id)? == Some(TopLayer::Popover),
    ))
}

// ---- close requests and light dismiss --------------------------------------

/// A dialog's `closedby` state: `any`, `closerequest` or `none`; missing
/// and invalid values are `closerequest` for modal dialogs, else `none`.
fn closed_by(agent: &mut Agent, id: NodeId) -> JsResult<&'static str> {
    let value = attribute(agent, id, "closedby")?.map(|v| v.to_ascii_lowercase());
    Ok(match value.as_deref() {
        Some("any") => "any",
        Some("closerequest") => "closerequest",
        Some("none") => "none",
        _ if top_layer_state(agent, id)? == Some(TopLayer::Modal) => "closerequest",
        _ => "none",
    })
}

/// Fire `cancel` at a dialog and close it unless a listener canceled it.
fn request_close_dialog(agent: &mut Agent, id: NodeId) -> JsResult<()> {
    let wrapper = wrap_node(agent, id)?;
    with_root(agent, wrapper, |agent| {
        if fire_plain(agent, wrapper, "cancel", true)? {
            close_dialog(agent, wrapper, None)?;
        }
        Ok(())
    })
}

fn main_document(agent: &mut Agent) -> JsResult<NodeId> {
    with_page(agent, |agent, page| {
        page.read(agent, page.main, |_, d| Ok(d.root()))
    })
}

/// Showing auto and hint popovers of `id`'s document, bottom first.
fn popover_stack(agent: &mut Agent, id: NodeId) -> JsResult<Vec<NodeId>> {
    read_doc(agent, id, |d| {
        d.top_layer()
            .iter()
            .filter(|&&(top, kind)| {
                kind == TopLayer::Popover
                    && element_of(d, top).is_some_and(|e| {
                        popover_state(value_of(e, "popover").as_deref()) != Some("manual")
                    })
            })
            .map(|&(top, _)| top)
            .collect()
    })
}

/// A user close request (Escape): the topmost modal dialog gets `cancel`
/// and closes, or the topmost auto popover hides. Whether one handled it.
pub(in crate::page) fn close_request(agent: &mut Agent) -> JsResult<bool> {
    let root = main_document(agent)?;
    let stack = popover_stack(agent, root)?;
    let top = read_doc(agent, root, |d| {
        d.top_layer()
            .iter()
            .rev()
            .find(|&&(top, kind)| kind == TopLayer::Modal || stack.contains(&top))
            .copied()
    })?;
    match top {
        Some((id, TopLayer::Modal)) => {
            if closed_by(agent, id)? == "none" {
                return Ok(false);
            }
            request_close_dialog(agent, id)?;
            Ok(true)
        }
        Some((id, _)) => {
            let wrapper = wrap_node(agent, id)?;
            with_root(agent, wrapper, |agent| hide(agent, wrapper, id))?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Where a click on `id` lands: inert content is skipped by hit testing,
/// so a click blocked by a modal dialog goes to the dialog (`true`: on its
/// backdrop) and one under `inert` to the inert subtree's parent.
pub(in crate::page) fn click_target(agent: &mut Agent, id: NodeId) -> JsResult<(NodeId, bool)> {
    read_doc(agent, id, |d| {
        if let Some(modal) = blocking_modal(d, id) {
            return (modal, true);
        }
        let mut landing = id;
        let mut at = Some(id);
        while let Some(current) = at {
            let Ok(node) = d.node(current) else {
                break;
            };
            if element_of(d, current).is_some_and(|e| {
                e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE) && has(e, "inert")
            }) {
                landing = node.parent().unwrap_or(landing);
            }
            at = node.parent();
        }
        (landing, false)
    })
}

/// HTML "light dismiss": a press at `target` hides the auto popovers above
/// the topmost one containing it (or invoked from it), and a press on the
/// backdrop of a `closedby=any` modal dialog requests its close.
pub(in crate::page) fn light_dismiss(
    agent: &mut Agent,
    target: NodeId,
    outside: bool,
) -> JsResult<()> {
    if outside {
        let modal = read_doc(agent, target, |d| {
            d.top_layer()
                .iter()
                .rev()
                .find(|&&(_, kind)| kind == TopLayer::Modal)
                .map(|&(id, _)| id)
        })?;
        if let Some(modal) = modal {
            if closed_by(agent, modal)? == "any" {
                request_close_dialog(agent, modal)?;
            }
        }
    }
    let stack = popover_stack(agent, target)?;
    if stack.is_empty() {
        return Ok(());
    }
    let endpoint = read_doc(agent, target, |d| {
        // Popovers invoked by a button the press is on count as containing it.
        let mut invoked = Vec::new();
        let mut at = Some(target);
        while let Some(current) = at {
            if let Some(Activation::Popover(popover, _)) = activation_of(d, current) {
                invoked.push(popover);
            }
            at = d.node(current).ok().and_then(|n| n.parent());
        }
        stack.iter().rposition(|&popover| {
            d.is_inclusive_descendant(target, popover) || invoked.contains(&popover)
        })
    })?;
    let keep = endpoint.map_or(0, |index| index + 1);
    for &popover in stack[keep..].iter().rev() {
        let wrapper = wrap_node(agent, popover)?;
        with_root(agent, wrapper, |agent| hide(agent, wrapper, popover))?;
    }
    Ok(())
}

// ---- activation ------------------------------------------------------------

/// What a click on an element in the event path activates.
enum Activation {
    /// Toggle this details element (a click on its summary).
    Details(NodeId),
    /// Show, hide or toggle this popover (a button's `popovertarget`).
    Popover(NodeId, &'static str),
}

fn activation_of(document: &rustic_dom::Document, id: NodeId) -> Option<Activation> {
    let element = element_of(document, id)?;
    if element.name().namespace() != Some(rustic_dom::HTML_NAMESPACE) {
        return None;
    }
    match element.name().local() {
        "summary" => {
            // The details' summary for its parent: its first summary child.
            let parent = document.node(id).ok()?.parent()?;
            let details = element_of(document, parent)?;
            if details.name().local() != "details"
                || details.name().namespace() != Some(rustic_dom::HTML_NAMESPACE)
            {
                return None;
            }
            let first = document.children(parent).ok()?.find(|&c| {
                element_of(document, c).is_some_and(|e| {
                    e.name().local() == "summary"
                        && e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE)
                })
            })?;
            (first == id).then_some(Activation::Details(parent))
        }
        "button" | "input" => {
            if element.name().local() == "input"
                && !matches!(
                    value_of(element, "type")
                        .map(|t| t.to_ascii_lowercase())
                        .as_deref(),
                    Some("button" | "submit" | "reset" | "image")
                )
            {
                return None;
            }
            if has(element, "disabled") {
                return None;
            }
            let target = value_of(element, "popovertarget")?;
            let root = super::ranges::root_of(document, id);
            let found = document.walk(root).ok()?.find(|&(other, _)| {
                element_of(document, other).is_some_and(|e| {
                    value_of(e, "id").as_deref() == Some(target.as_str()) && has(e, "popover")
                })
            })?;
            let action = match value_of(element, "popovertargetaction")
                .map(|a| a.to_ascii_lowercase())
                .as_deref()
            {
                Some("show") => "show",
                Some("hide") => "hide",
                _ => "toggle",
            };
            Some(Activation::Popover(found.0, action))
        }
        _ => None,
    }
}

/// Run the activation behavior of the first element in `path` that has
/// one (summary toggling, popover invokers) after an uncanceled click.
pub(super) fn activate(agent: &mut Agent, path: PrivateArray, bubbles: bool) -> JsResult<()> {
    let window = with_page(agent, |_, page| Ok(page.window))?;
    let len = if bubbles { path.len(agent)? } else { 1 };
    for index in 0..len {
        let value = path.get(agent, index)?;
        if value == window {
            return Ok(());
        }
        let Ok(id) = node_of(agent, value) else {
            continue;
        };
        let found = read_doc(agent, id, |d| activation_of(d, id))?;
        match found {
            Some(Activation::Details(details)) => {
                let wrapper = wrap_node(agent, details)?;
                let open = attribute(agent, details, "open")?.is_some();
                return set_details_state(agent, wrapper, !open);
            }
            Some(Activation::Popover(target, action)) => {
                let wrapper = wrap_node(agent, target)?;
                let showing = top_layer_state(agent, target)? == Some(TopLayer::Popover);
                return match (action, showing) {
                    ("show", false) | ("toggle", false) => show_inner(agent, wrapper, target),
                    ("hide", true) | ("toggle", true) => hide(agent, wrapper, target),
                    _ => Ok(()),
                };
            }
            None => {}
        }
    }
    Ok(())
}
