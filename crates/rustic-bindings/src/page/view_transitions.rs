//! Same-document view transitions (CSS View Transitions Level 1, editor's
//! draft reviewed 2026-11-25): `document.startViewTransition()` and the
//! `ViewTransition` interface with its `updateCallbackDone`, `ready` and
//! `finished` promises and `skipTransition()`.
//!
//! The transition runs through rendering opportunities: the first one
//! captures the old state (the elements' `view-transition-name`s, which
//! must be unique) and calls the update callback; once its promise
//! fulfills the new state is captured and `ready` resolves; `finished`
//! resolves when the default group animation (0.25 s) has run, or at once
//! with no named elements. Snapshots and the `::view-transition`
//! pseudo-element tree are not painted, so the old and new states cross
//! without a visual animation (TD-207).

use std::cell::{Cell, RefCell};

use rustic_dom::NodeKind;
use rustic_js::runtime::{Abrupt, Agent, HostRoot, JsResult, ObjectId, Value};

use super::events::{PrivateArray, define_method, with_root};
use super::{creation, illegal_constructor, install_interface, prototype, with_page};

const VIEW_TRANSITION: &str = "ViewTransition";

/// Transition slots.
const S_UPDATE_DONE: u32 = 0;
const S_READY: u32 = 1;
const S_FINISHED: u32 = 2;
const S_CALLBACK: u32 = 3;

/// The default `::view-transition-group` animation duration (ms).
const GROUP_DURATION_MS: f64 = 250.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Waiting for a rendering opportunity to capture the old state.
    PendingCapture,
    /// The update callback was called; waiting for it to finish.
    Updating,
    /// The new state was captured: `finished` resolves at this time.
    Animating(Option<f64>),
}

#[derive(Debug)]
struct Active {
    transition: Value,
    root: HostRoot,
    phase: Cell<Phase>,
}

/// Page-lifetime view transition state.
#[derive(Debug)]
pub(super) struct ViewTransitions {
    /// Transition object → slots.
    states: ObjectId,
    active: RefCell<Option<Active>>,
}

impl ViewTransitions {
    pub(super) fn new(agent: &mut Agent) -> JsResult<Self> {
        let states = agent.new_weak_map()?;
        agent.root(Value::object(states))?;
        Ok(Self {
            states,
            active: RefCell::new(None),
        })
    }

    /// Whether a transition waits for a rendering opportunity.
    pub(super) fn wants_frame(&self) -> bool {
        self.active.borrow().as_ref().is_some_and(|a| {
            // Updating polls for a rejected update callback.
            matches!(
                a.phase.get(),
                Phase::PendingCapture | Phase::Updating | Phase::Animating(_)
            )
        })
    }
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        VIEW_TRANSITION,
        None,
        illegal_constructor,
        0,
        &[
            (
                "updateCallbackDone",
                "get updateCallbackDone",
                slot::<S_UPDATE_DONE>,
            ),
            ("ready", "get ready", slot::<S_READY>),
            ("finished", "get finished", slot::<S_FINISHED>),
        ],
        |agent, proto| define_method(agent, proto, "skipTransition", skip_transition, 0),
    )?;
    let document = prototype(agent, "Document")?;
    define_method(agent, document, "startViewTransition", start, 0)
}

fn slots_of(agent: &mut Agent, this: Value) -> JsResult<PrivateArray> {
    let slots = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.view_transitions.states, this))
    })?;
    match PrivateArray::from_value(slots) {
        Some(slots) => Ok(slots),
        None => agent.type_error("expected a ViewTransition receiver"),
    }
}

fn slot<const SLOT: u32>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    slots_of(agent, this)?.get(agent, SLOT)
}

fn promise(agent: &mut Agent, slots: PrivateArray, slot: u32) -> JsResult<ObjectId> {
    match slots.get(agent, slot)?.as_object() {
        Some(promise) => Ok(promise),
        None => agent.type_error("view transition promise missing"),
    }
}

fn error(agent: &mut Agent, name: &str, message: &str) -> JsResult<Value> {
    match creation::exception(agent, name, message) {
        Abrupt::Throw(value) => Ok(value),
        other => Err(other),
    }
}

/// `document.startViewTransition(updateCallback)` or
/// `startViewTransition({update})`.
fn start(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let argument = args.first().copied().unwrap_or(Value::UNDEFINED);
    let callback = if agent.is_callable(argument) {
        argument
    } else if let Some(object) = argument.as_object() {
        let key = agent.heap.key("update")?;
        let update = agent.get(object, key)?;
        if update == Value::UNDEFINED || agent.is_callable(update) {
            update
        } else {
            return agent.type_error("update must be a function");
        }
    } else if argument == Value::UNDEFINED || argument == Value::NULL {
        Value::UNDEFINED
    } else {
        return agent.type_error("startViewTransition expects a callback or options");
    };
    let proto = prototype(agent, VIEW_TRANSITION)?;
    let transition = Value::object(agent.new_object_with(Some(proto))?);
    with_root(agent, transition, |agent| {
        let update_done = Value::object(agent.new_host_promise()?);
        let ready = Value::object(agent.new_host_promise()?);
        let finished = Value::object(agent.new_host_promise()?);
        let slots = PrivateArray::new(agent, &[update_done, ready, finished, callback])?;
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.view_transitions.states, transition, slots.value())
        })?;
        // A new transition skips the active one.
        let previous = with_page(agent, |_, page| {
            Ok(page
                .view_transitions
                .active
                .borrow()
                .as_ref()
                .map(|a| a.transition))
        })?;
        if let Some(previous) = previous {
            skip(
                agent,
                previous,
                "AbortError",
                "a new view transition started",
            )?;
        }
        let root = agent.root(transition)?;
        with_page(agent, |_, page| {
            *page.view_transitions.active.borrow_mut() = Some(Active {
                transition,
                root,
                phase: Cell::new(Phase::PendingCapture),
            });
            Ok(())
        })?;
        Ok(transition)
    })
}

fn skip_transition(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    slots_of(agent, this)?;
    skip(agent, this, "AbortError", "the view transition was skipped")?;
    Ok(Value::UNDEFINED)
}

/// Call the update callback once; `updateCallbackDone` follows its result.
fn call_update(agent: &mut Agent, slots: PrivateArray) -> JsResult<()> {
    let callback = slots.get(agent, S_CALLBACK)?;
    if callback == Value::NULL {
        return Ok(());
    }
    slots.set(agent, S_CALLBACK, Value::NULL)?;
    let result = if callback == Value::UNDEFINED {
        Value::UNDEFINED
    } else {
        match agent.call(callback, Value::UNDEFINED, &[]) {
            Ok(value) => value,
            Err(Abrupt::Throw(reason)) => {
                let rejected = agent.new_host_promise()?;
                agent.reject_host_promise(rejected, reason);
                Value::object(rejected)
            }
            Err(other) => return Err(other),
        }
    };
    with_root(agent, result, |agent| {
        // Fulfilled with undefined, or rejected with the callback's reason.
        let settled = agent.react_to_host_value(result, return_undefined, Value::UNDEFINED)?;
        let done = promise(agent, slots, S_UPDATE_DONE)?;
        agent.resolve_host_promise(done, settled)
    })
}

fn return_undefined(_: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::UNDEFINED)
}

/// Skip `transition` (View Transitions §7.4): `ready` rejects with a
/// `name` DOMException, `finished` follows `updateCallbackDone`.
fn skip(agent: &mut Agent, transition: Value, name: &str, message: &str) -> JsResult<()> {
    let slots = slots_of(agent, transition)?;
    let ready = promise(agent, slots, S_READY)?;
    if agent.promise_rejected(ready).is_none() {
        let reason = error(agent, name, message)?;
        agent.mark_promise_handled(ready);
        agent.reject_host_promise(ready, reason);
    }
    // The update callback still runs.
    call_update(agent, slots)?;
    let done = slots.get(agent, S_UPDATE_DONE)?;
    let after = agent.react_to_host_value(done, return_undefined, Value::UNDEFINED)?;
    let finished = promise(agent, slots, S_FINISHED)?;
    agent.resolve_host_promise(finished, after)?;
    clear(agent, transition)
}

fn clear(agent: &mut Agent, transition: Value) -> JsResult<()> {
    let active = with_page(agent, |_, page| {
        let mut active = page.view_transitions.active.borrow_mut();
        Ok(match active.as_ref() {
            Some(a) if a.transition == transition => active.take(),
            _ => None,
        })
    })?;
    if let Some(active) = active {
        agent.unroot(active.root);
    }
    Ok(())
}

/// The `view-transition-name`s of rendered elements; `Err` names a
/// duplicate.
fn capture(agent: &mut Agent) -> JsResult<Result<Vec<String>, String>> {
    with_page(agent, |agent, page| {
        let nodes = page.read(agent, page.main, |_, document| {
            let mut out = Vec::new();
            let mut stack = vec![document.root()];
            while let Some(id) = stack.pop() {
                if let Ok(node) = document.node(id) {
                    if matches!(node.kind(), NodeKind::Element(_)) {
                        out.push(id);
                    }
                }
                if let Ok(children) = document.children(id) {
                    let mut children: Vec<_> = children.collect();
                    children.reverse();
                    stack.extend(children);
                }
            }
            Ok(out)
        })?;
        let mut names: Vec<String> = Vec::new();
        for node in nodes {
            // Without layout there are no computed styles, hence no names.
            let Ok(style) = page.tree.computed_style(node) else {
                break;
            };
            match style.get_longhand(rustic_css::PropertyId::ViewTransitionName) {
                None | Some("none") => {}
                Some(name) if names.iter().any(|n| n == name) => return Ok(Err(name.to_owned())),
                Some(name) => names.push(name.to_owned()),
            }
        }
        Ok(Ok(names))
    })
}

/// The view transition steps of a rendering opportunity at `now` ms.
pub(super) fn update(agent: &mut Agent, now: f64) -> JsResult<()> {
    let active = with_page(agent, |_, page| {
        Ok(page
            .view_transitions
            .active
            .borrow()
            .as_ref()
            .map(|a| (a.transition, a.phase.get())))
    })?;
    let Some((transition, phase)) = active else {
        return Ok(());
    };
    let set_phase = |agent: &mut Agent, phase: Phase| {
        with_page(agent, |_, page| {
            if let Some(a) = page.view_transitions.active.borrow().as_ref() {
                a.phase.set(phase);
            }
            Ok(())
        })
    };
    let slots = slots_of(agent, transition)?;
    match phase {
        Phase::PendingCapture => {
            if let Err(name) = capture(agent)? {
                return skip(
                    agent,
                    transition,
                    "InvalidStateError",
                    &format!("duplicate view-transition-name {name}"),
                );
            }
            set_phase(agent, Phase::Updating)?;
            call_update(agent, slots)?;
            let done = slots.get(agent, S_UPDATE_DONE)?;
            // The rejection is handled by the Updating phase.
            let derived = agent.react_to_host_value(done, update_finished, transition)?;
            if let Some(derived) = derived.as_object() {
                agent.mark_promise_handled(derived);
            }
        }
        Phase::Updating => {
            // A rejected update callback skips the transition: `ready`
            // and `finished` take its reason.
            let done = promise(agent, slots, S_UPDATE_DONE)?;
            if agent.promise_rejected(done) == Some(true) {
                let ready = promise(agent, slots, S_READY)?;
                agent.mark_promise_handled(ready);
                for slot in [S_READY, S_FINISHED] {
                    let p = promise(agent, slots, slot)?;
                    agent.resolve_host_promise(p, Value::object(done))?;
                }
                clear(agent, transition)?;
            }
        }
        Phase::Animating(None) => {
            set_phase(agent, Phase::Animating(Some(now + GROUP_DURATION_MS)))?;
        }
        Phase::Animating(Some(end)) => {
            if now >= end {
                let finished = promise(agent, slots, S_FINISHED)?;
                agent.resolve_host_promise(finished, Value::UNDEFINED)?;
                clear(agent, transition)?;
            }
        }
    }
    Ok(())
}

/// `updateCallbackDone` fulfilled: capture the new state and start the
/// animation phase.
fn update_finished(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let transition = agent.host_capture(0);
    let still_active = with_page(agent, |_, page| {
        Ok(page
            .view_transitions
            .active
            .borrow()
            .as_ref()
            .is_some_and(|a| a.transition == transition))
    })?;
    if !still_active {
        return Ok(Value::UNDEFINED);
    }
    let names = match capture(agent)? {
        Ok(names) => names,
        Err(name) => {
            skip(
                agent,
                transition,
                "InvalidStateError",
                &format!("duplicate view-transition-name {name}"),
            )?;
            return Ok(Value::UNDEFINED);
        }
    };
    let slots = slots_of(agent, transition)?;
    let ready = promise(agent, slots, S_READY)?;
    agent.resolve_host_promise(ready, Value::UNDEFINED)?;
    if names.is_empty() {
        let finished = promise(agent, slots, S_FINISHED)?;
        agent.resolve_host_promise(finished, Value::UNDEFINED)?;
        clear(agent, transition)?;
    } else {
        with_page(agent, |_, page| {
            if let Some(a) = page.view_transitions.active.borrow().as_ref() {
                a.phase.set(Phase::Animating(None));
            }
            Ok(())
        })?;
    }
    Ok(Value::UNDEFINED)
}
