//! HTML §6.4.3 user activation: `navigator.userActivation` and the host's
//! view of it.
//!
//! A trusted activation-triggering input event (a primary click, or a
//! `keydown` other than Escape) gives the window *transient activation*
//! for [`TRANSIENT_ACTIVATION`] and sets *sticky activation*. Script cannot
//! create it: events it dispatches are untrusted and never reach
//! [`note_input`]. APIs that need a gesture ask [`has_transient`] and
//! spend it with [`consume`].

use std::time::Duration;

use rustic_js::runtime::{Agent, JsResult, PropertyDescriptor, Value};

use super::events::InputEvent;
use super::{illegal_constructor, install_interface, prototype, with_page};

/// HTML's "transient activation duration" (Chromium and Firefox: 5 s).
pub const TRANSIENT_ACTIVATION: Duration = Duration::from_secs(5);

const INTERFACE: &str = "UserActivation";

/// Install `Navigator`, `UserActivation` and `window.navigator`; returns
/// the page's (navigator, userActivation) objects, which the page keeps.
pub(super) fn install(agent: &mut Agent) -> JsResult<(Value, Value)> {
    install_interface(
        agent,
        INTERFACE,
        None,
        illegal_constructor,
        0,
        &[
            ("hasBeenActive", "get hasBeenActive", has_been_active),
            ("isActive", "get isActive", is_active),
        ],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        "Navigator",
        None,
        illegal_constructor,
        0,
        &[("userActivation", "get userActivation", user_activation)],
        |_, _| Ok(()),
    )?;
    let proto = prototype(agent, "Navigator")?;
    let navigator = Value::object(agent.new_object_with(Some(proto))?);
    agent.root(navigator)?;
    let proto = prototype(agent, INTERFACE)?;
    let activation = Value::object(agent.new_object_with(Some(proto))?);
    agent.root(activation)?;
    let get = Value::object(agent.new_native_function(navigator_getter, "get navigator", 0)?);
    let global = agent.intrinsics.global;
    let name = agent.heap.key("navigator")?;
    agent.define_property_or_throw(
        global,
        name,
        PropertyDescriptor::accessor(get, Value::UNDEFINED, true, true),
    )?;
    Ok((navigator, activation))
}

fn navigator_getter(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    with_page(agent, |_, page| Ok(page.navigator.0))
}

fn user_activation(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        if this == page.navigator.0 {
            Ok(page.navigator.1)
        } else {
            agent.type_error("expected a Navigator receiver")
        }
    })
}

fn check(agent: &mut Agent, this: Value) -> JsResult<()> {
    with_page(agent, |agent, page| {
        if this == page.navigator.1 {
            Ok(())
        } else {
            agent.type_error("expected a UserActivation receiver")
        }
    })
}

fn has_been_active(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    check(agent, this)?;
    with_page(agent, |_, page| Ok(Value::boolean(page.activation.get().1)))
}

fn is_active(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    check(agent, this)?;
    with_page(agent, |_, page| Ok(Value::boolean(transient(page))))
}

fn transient(page: &super::Page) -> bool {
    let (last, _) = page.activation.get();
    let now = page.now.get();
    last.is_some_and(|last| now >= last && now - last < TRANSIENT_ACTIVATION)
}

/// HTML "activation triggering input event": a primary click, or a
/// `keydown` that is not Escape.
pub(super) fn triggers_activation(input: &InputEvent) -> bool {
    match input {
        InputEvent::Click { .. } => true,
        InputEvent::KeyDown(key) => {
            key.key.as_units() != "Escape".encode_utf16().collect::<Vec<_>>()
        }
        // An assistive technology's default action stands for the user's
        // own click, as in other engines.
        InputEvent::Assistive(super::AssistiveAction::Click) => true,
        InputEvent::KeyUp(_) | InputEvent::Composition(_) | InputEvent::Assistive(_) => false,
    }
}

/// Record a trusted input event at the page's current time.
pub(super) fn note_input(page: &super::Page, input: &InputEvent) {
    if triggers_activation(input) {
        page.activation.set((Some(page.now.get()), true));
    }
}

/// Whether the window has transient activation now.
pub fn has_transient(agent: &Agent) -> bool {
    let host = agent.execution_host();
    host.as_ref()
        .and_then(|host| host.as_any().downcast_ref::<super::Page>())
        .is_some_and(transient)
}

/// HTML "consume user activation": end transient activation (sticky
/// activation stays). Returns whether there was any to consume.
pub fn consume(agent: &Agent) -> bool {
    let host = agent.execution_host();
    let Some(page) = host
        .as_ref()
        .and_then(|host| host.as_any().downcast_ref::<super::Page>())
    else {
        return false;
    };
    let active = transient(page);
    if active {
        page.activation.set((None, true));
    }
    active
}
