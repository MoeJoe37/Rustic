//! HTML interactive elements: `details`/`summary` (§4.11.1–2), `dialog`
//! (§4.11.4) and the popover API (§6.12).
//!
//! The `open` content attribute is the source of truth for details and
//! dialogs. Modal dialogs and showing popovers are in their document's top
//! layer ([`rustic_dom::Document::top_layer_add`]), which `:modal` and
//! `:popover-open` read. `toggle` and `close` events are queued as
//! microtasks rather than DOM manipulation tasks; focus, inertness, light
//! dismiss and `::backdrop` are not implemented (TD-216).

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

/// Page-lifetime state: dialog return values and the queued-event callback.
#[derive(Debug)]
pub(super) struct Elements {
    /// Dialog → its `returnValue` string.
    return_values: ObjectId,
    /// The native job that fires queued events.
    fire_job: Value,
}

impl Elements {
    pub(super) fn new(agent: &mut Agent) -> JsResult<Self> {
        let return_values = agent.new_weak_map()?;
        agent.root(Value::object(return_values))?;
        let fire_job = Value::object(agent.new_native_function(fire_queued, "fire queued", 1)?);
        agent.root(fire_job)?;
        Ok(Self {
            return_values,
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

fn fire_queued(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(job) = args.first().copied().and_then(PrivateArray::from_value) else {
        return Ok(Value::UNDEFINED);
    };
    let target = job.get(agent, 0)?;
    let event = job.get(agent, 1)?;
    match fire_at(agent, target, event) {
        Ok(_) => Ok(Value::UNDEFINED),
        Err(Abrupt::Throw(error)) => {
            super::events::report_callback_error(agent, error)?;
            Ok(Value::UNDEFINED)
        }
        Err(error) => Err(error),
    }
}

fn queue_toggle(agent: &mut Agent, target: Value, old: &str, new: &str) -> JsResult<()> {
    let event = toggle_event(agent, "toggle", false, old, new)?;
    with_root(agent, event, |agent| queue(agent, target, event))
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

/// Open or close a details element, closing the others of its name group
/// and queuing `toggle` (HTML §4.11.1 attribute change steps).
fn set_details_state(agent: &mut Agent, this: Value, open: bool) -> JsResult<()> {
    let id = node_of(agent, this)?;
    let was_open = attribute(agent, id, "open")?.is_some();
    if was_open == open {
        return Ok(());
    }
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
    set_attribute(agent, this, "open", open.then_some(""))?;
    let (old, new) = if open {
        ("closed", "open")
    } else {
        ("open", "closed")
    };
    queue_toggle(agent, this, old, new)
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
    set_attribute(agent, this, "open", Some(""))?;
    if modal {
        top_layer_add(agent, id, TopLayer::Modal)?;
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
    set_attribute(agent, this, "open", None)?;
    top_layer_remove(agent, id)?;
    if let Some(result) = result {
        store_return_value(agent, this, result)?;
    }
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
    if popover_state(attribute(agent, id, "popover")?.as_deref()) == Some("auto") {
        hide_other_auto(agent, id)?;
    }
    top_layer_add(agent, id, TopLayer::Popover)?;
    queue_toggle(agent, this, "closed", "open")
}

fn hide(agent: &mut Agent, this: Value, id: NodeId) -> JsResult<()> {
    if !check_popover(agent, id, true)? {
        return Ok(());
    }
    let before = toggle_event(agent, "beforetoggle", false, "open", "closed")?;
    fire_at(agent, this, before)?;
    if top_layer_remove(agent, id)? {
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
