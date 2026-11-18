//! EventTarget and synchronous dispatch through the page's live DOM.

use std::cell::Cell;
use std::time::Instant;

use rustic_dom::{HTML_NAMESPACE, NodeId, NodeKind};
use rustic_js::runtime::{
    Abrupt, Agent, JsResult, NativeFn, ObjectId, OutOfMemory, PropertyDescriptor, PropertyKey,
    Value,
};

use crate::heap::WrapperKind;
use crate::webidl::coerce_string;

use super::{identity, install_interface, prototype, read_node, required_arg, with_page, wrap};

mod input;
mod state;
mod ui;
pub use input::{InputEvent, KeyInput, Modifiers};
pub(super) use input::{connected, deliver, install_controls, reaches_root};

use state::{Event, Phase};

pub(super) const TARGET: &str = "EventTarget";

/// Weak maps are rooted for the page lifetime. Their private values trace
/// listeners and event state only while the public keys are alive.
#[derive(Debug)]
pub(super) struct Events {
    targets: ObjectId,
    states: ObjectId,
    is_trusted: Value,
    started: Instant,
    listener_errors: Cell<u64>,
    controls: ObjectId,
    pressed_space: Cell<Option<rustic_dom::NodeId>>,
}

impl Events {
    pub(super) fn new(agent: &mut Agent) -> JsResult<Self> {
        let targets = agent.new_weak_map()?;
        let states = agent.new_weak_map()?;
        let is_trusted = state::trusted_getter(agent)?;
        let controls = agent.new_weak_map()?;
        let storage = PrivateArray::new(
            agent,
            &[
                Value::object(targets),
                Value::object(states),
                is_trusted,
                Value::object(controls),
            ],
        )?;
        agent.root(storage.value())?;
        Ok(Self {
            targets,
            states,
            is_trusted,
            started: Instant::now(),
            listener_errors: Cell::new(0),
            controls,
            pressed_space: Cell::new(None),
        })
    }

    pub(super) fn take_listener_error_count(&self) -> u64 {
        self.listener_errors.take()
    }

    fn report_listener_error(&self) {
        self.listener_errors
            .set(self.listener_errors.get().saturating_add(1));
    }
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        TARGET,
        None,
        construct_target,
        0,
        &[],
        |agent, prototype| {
            for (name, behavior, length) in [
                ("addEventListener", add_listener as NativeFn, 2),
                ("removeEventListener", remove_listener, 2),
                ("dispatchEvent", dispatch_event, 1),
            ] {
                define_method(agent, prototype, name, behavior, length)?;
            }
            Ok(())
        },
    )?;
    state::install(agent)?;
    ui::install(agent)
}

pub(super) fn define_method(
    agent: &mut Agent,
    prototype: ObjectId,
    name: &str,
    behavior: NativeFn,
    length: u32,
) -> JsResult<()> {
    let key = agent.heap.key(name)?;
    let function = agent.new_native_function(behavior, name, length)?;
    agent.define_property_or_throw(
        prototype,
        key,
        PropertyDescriptor::data_with(Value::object(function), true, true, true),
    )
}

/// Arrays used as native records have no prototype, so reads and writes
/// cannot invoke page code. The records themselves are never exposed.
#[derive(Clone, Copy)]
pub(super) struct PrivateArray(ObjectId);

impl PrivateArray {
    pub(super) fn new(agent: &mut Agent, values: &[Value]) -> JsResult<Self> {
        let object = agent.create_array_from_list(values)?;
        let success = agent.set_prototype_of(object, None)?;
        debug_assert!(success);
        Ok(Self(object))
    }

    pub(super) fn value(self) -> Value {
        Value::object(self.0)
    }

    pub(super) fn get(self, agent: &mut Agent, index: u32) -> JsResult<Value> {
        agent.get(self.0, PropertyKey::Index(index))
    }

    pub(super) fn set(self, agent: &mut Agent, index: u32, value: Value) -> JsResult<()> {
        agent.set(self.0, PropertyKey::Index(index), value, true)
    }

    fn len(self, agent: &mut Agent) -> JsResult<u32> {
        let key = PropertyKey::String(agent.names.length);
        let len = agent.get(self.0, key)?;
        // A private array's non-configurable length is always a uint32.
        Ok(len.as_number().expect("array length") as u32)
    }

    pub(super) fn from_value(value: Value) -> Option<Self> {
        value.as_object().map(Self)
    }
}

fn with_root<T>(
    agent: &mut Agent,
    value: Value,
    operation: impl FnOnce(&mut Agent) -> JsResult<T>,
) -> JsResult<T> {
    let root = agent.root(value)?;
    let result = operation(agent);
    agent.unroot(root);
    result
}

fn construct_target(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("EventTarget requires new");
    };
    let proto = agent.host_prototype_from_new_target(TARGET, new_target)?;
    let target = Value::object(agent.new_object_with(Some(proto))?);
    with_page(agent, |agent, page| {
        let listeners = ListenerList::new(agent)?;
        agent.weak_map_set(page.events.targets, target, listeners.0.value())?;
        Ok(target)
    })
}

#[derive(Clone, Copy)]
struct Target {
    value: Value,
    node: bool,
}

fn receiver(agent: &mut Agent, value: Value) -> JsResult<Target> {
    with_page(agent, |agent, page| {
        if value == page.window
            || value.is_nullish()
            || value == Value::object(agent.intrinsics.global)
        {
            return Ok(Target {
                value: page.window,
                node: false,
            });
        }
        let Some(object) = value.as_object() else {
            return agent.type_error("expected an EventTarget receiver");
        };
        if agent.host_data(object).is_none() && agent.weak_map_has(page.events.targets, value) {
            return Ok(Target { value, node: false });
        }
        let Ok(wrappers) = page.wrappers.try_borrow() else {
            return agent.type_error("DOM bindings are busy");
        };
        let id = identity(agent, &wrappers, value, WrapperKind::Node)?;
        page.read(agent, id.document(), |agent, document| {
            if document.node(id).is_err() {
                return agent.type_error("expected an EventTarget receiver");
            }
            Ok(Target { value, node: true })
        })
    })
}

fn listener_list(agent: &mut Agent, target: Value) -> JsResult<Option<ListenerList>> {
    with_page(agent, |agent, page| {
        Ok(
            PrivateArray::from_value(agent.weak_map_get(page.events.targets, target))
                .map(ListenerList),
        )
    })
}

fn listener_callback(agent: &mut Agent, value: Value) -> JsResult<Option<Value>> {
    if value.is_nullish() {
        Ok(None)
    } else if value.as_object().is_some() {
        Ok(Some(value))
    } else {
        agent.type_error("event listener must be an object or null")
    }
}

fn option(agent: &mut Agent, object: ObjectId, name: &str) -> JsResult<Value> {
    let key = agent.heap.key(name)?;
    agent.get(object, key)
}

fn capture_option(agent: &mut Agent, value: Value) -> JsResult<bool> {
    let value = match value.as_object() {
        Some(object) => option(agent, object, "capture")?,
        None => value,
    };
    Ok(agent.to_boolean(value))
}

#[derive(Clone, Copy, Default)]
struct Options {
    capture: bool,
    once: bool,
    passive: Option<bool>,
}

fn add_options(agent: &mut Agent, value: Value) -> JsResult<Options> {
    let capture = capture_option(agent, value)?;
    let Some(object) = value.as_object() else {
        return Ok(Options {
            capture,
            ..Options::default()
        });
    };
    let once = option(agent, object, "once")?;
    let once = agent.to_boolean(once);
    let passive = option(agent, object, "passive")?;
    let passive = (passive != Value::UNDEFINED).then(|| agent.to_boolean(passive));
    if option(agent, object, "signal")? != Value::UNDEFINED {
        return agent.type_error("signal must be an AbortSignal");
    }
    Ok(Options {
        capture,
        once,
        passive,
    })
}

fn default_passive(agent: &mut Agent, target: Target, event_type: Value) -> JsResult<bool> {
    let string = agent
        .heap
        .string(event_type.as_string().expect("event type"));
    if !["touchstart", "touchmove", "wheel", "mousewheel"]
        .into_iter()
        .any(|candidate| string.units().eq(candidate.encode_utf16()))
    {
        return Ok(false);
    }
    if !target.node {
        return with_page(agent, |_, page| Ok(target.value == page.window));
    }
    read_node(agent, target.value, |agent, _, document, id, node| {
        if matches!(node.kind(), NodeKind::Document) {
            return Ok(true);
        }
        for child in document
            .children(document.root())
            .map_err(|error| crate::dom_exception::from_dom_error(agent, error))?
        {
            agent.check_native_work()?;
            let Ok(NodeKind::Element(root)) = document.node(child).map(|child| child.kind()) else {
                continue;
            };
            if id == child {
                return Ok(true);
            }
            if node.parent() != Some(child)
                || root.name().namespace() != Some(HTML_NAMESPACE)
                || root.name().local() != "html"
            {
                return Ok(false);
            }
            // The body or frameset is the first such child of the document element.
            for sibling in document
                .children(child)
                .map_err(|error| crate::dom_exception::from_dom_error(agent, error))?
            {
                agent.check_native_work()?;
                if let Ok(NodeKind::Element(element)) =
                    document.node(sibling).map(|node| node.kind())
                {
                    if element.name().namespace() == Some(HTML_NAMESPACE)
                        && matches!(element.name().local(), "body" | "frameset")
                    {
                        return Ok(id == sibling);
                    }
                }
            }
            break;
        }
        Ok(false)
    })
}

fn type_key(agent: &mut Agent, event_type: Value) -> JsResult<Value> {
    let id = event_type.as_string().expect("event type");
    let length = agent.heap.string(id).len();
    for _ in (0..length).step_by(64) {
        agent.check_native_work()?;
    }
    // key_for_string interns non-index names. A canonical array index
    // spelling has one exact uint32 representation and its own Value tag.
    match agent.heap.key_for_string(id)? {
        PropertyKey::String(atom) => Ok(Value::string(atom)),
        PropertyKey::Index(index) => Ok(Value::number(f64::from(index))),
        PropertyKey::Symbol(_) => unreachable!("string key"),
    }
}

fn add_listener(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let target = receiver(agent, this)?;
    let event_type = required_arg(agent, args, 0)?;
    let callback = required_arg(agent, args, 1)?;
    let event_type = Value::string(coerce_string(agent, event_type)?);
    with_root(agent, event_type, |agent| {
        let callback = listener_callback(agent, callback)?;
        let options = add_options(agent, args.get(2).copied().unwrap_or(Value::UNDEFINED))?;
        let Some(callback) = callback else {
            return Ok(Value::UNDEFINED);
        };
        let passive = match options.passive {
            Some(passive) => passive,
            None => default_passive(agent, target, event_type)?,
        };
        let key = type_key(agent, event_type)?;
        let existing = listener_list(agent, target.value)?;
        if let Some(list) = existing {
            if list.find(agent, key, callback, options.capture)?.is_some() {
                return Ok(Value::UNDEFINED);
            }
        }
        let flags = (u8::from(options.capture) * Listener::CAPTURE)
            | (u8::from(options.once) * Listener::ONCE)
            | (u8::from(passive) * Listener::PASSIVE);
        let listener = Listener::new(agent, key, callback, flags)?;
        match existing {
            Some(list) => list.append(agent, listener)?,
            None => {
                let list = ListenerList::new(agent)?;
                list.append(agent, listener)?;
                with_page(agent, |agent, page| {
                    agent.weak_map_set(page.events.targets, target.value, list.0.value())
                })?;
            }
        }
        Ok(Value::UNDEFINED)
    })
}

fn remove_listener(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let target = receiver(agent, this)?.value;
    let event_type = required_arg(agent, args, 0)?;
    let callback = required_arg(agent, args, 1)?;
    let event_type = Value::string(coerce_string(agent, event_type)?);
    with_root(agent, event_type, |agent| {
        let callback = listener_callback(agent, callback)?;
        let capture = capture_option(agent, args.get(2).copied().unwrap_or(Value::UNDEFINED))?;
        if let (Some(callback), Some(list)) = (callback, listener_list(agent, target)?) {
            let key = type_key(agent, event_type)?;
            if let Some(found) = list.find(agent, key, callback, capture)? {
                list.remove(agent, found)?;
            }
        }
        Ok(Value::UNDEFINED)
    })
}

/// Active listeners form an insertion-ordered, doubly linked list. A
/// dispatch snapshots the matching record identities for each phase.
#[derive(Clone, Copy)]
struct ListenerList(PrivateArray);

impl ListenerList {
    const HEAD: u32 = 0;
    const TAIL: u32 = 1;

    fn new(agent: &mut Agent) -> JsResult<Self> {
        Ok(Self(PrivateArray::new(agent, &[Value::NULL, Value::NULL])?))
    }

    fn head(self, agent: &mut Agent) -> JsResult<Option<Listener>> {
        Ok(Listener::from_value(self.0.get(agent, Self::HEAD)?))
    }

    fn find(
        self,
        agent: &mut Agent,
        event_type: Value,
        callback: Value,
        capture: bool,
    ) -> JsResult<Option<Listener>> {
        let mut current = self.head(agent)?;
        while let Some(listener) = current {
            agent.check_native_work()?;
            if listener.callback(agent)? == callback
                && listener.matches(agent, event_type, capture)?
            {
                return Ok(Some(listener));
            }
            current = listener.next(agent)?;
        }
        Ok(None)
    }

    fn append(self, agent: &mut Agent, listener: Listener) -> JsResult<()> {
        let last = self.0.get(agent, Self::TAIL)?;
        listener.0.set(agent, Listener::PREV, last)?;
        if let Some(last) = Listener::from_value(last) {
            last.0.set(agent, Listener::NEXT, listener.0.value())?;
        } else {
            self.0.set(agent, Self::HEAD, listener.0.value())?;
        }
        self.0.set(agent, Self::TAIL, listener.0.value())
    }

    fn remove(self, agent: &mut Agent, listener: Listener) -> JsResult<()> {
        let prev = listener.0.get(agent, Listener::PREV)?;
        let next = listener.0.get(agent, Listener::NEXT)?;
        if let Some(prev) = Listener::from_value(prev) {
            prev.0.set(agent, Listener::NEXT, next)?;
        } else {
            self.0.set(agent, Self::HEAD, next)?;
        }
        if let Some(next) = Listener::from_value(next) {
            next.0.set(agent, Listener::PREV, prev)?;
        } else {
            self.0.set(agent, Self::TAIL, prev)?;
        }
        listener
            .0
            .set(agent, Listener::CALLBACK, Value::UNDEFINED)?;
        listener.0.set(agent, Listener::PREV, Value::NULL)?;
        listener.0.set(agent, Listener::NEXT, Value::NULL)
    }

    fn snapshot(
        self,
        agent: &mut Agent,
        event_type: Value,
        capture: bool,
    ) -> JsResult<Option<PrivateArray>> {
        let mut matches = Vec::new();
        let mut current = self.head(agent)?;
        while let Some(listener) = current {
            agent.check_native_work()?;
            if listener.matches(agent, event_type, capture)? {
                matches.try_reserve(1).map_err(|_| OutOfMemory)?;
                matches.push(listener.0.value());
            }
            current = listener.next(agent)?;
        }
        if matches.is_empty() {
            return Ok(None);
        }
        PrivateArray::new(agent, &matches).map(Some)
    }
}

#[derive(Clone, Copy)]
struct Listener(PrivateArray);

impl Listener {
    const TYPE: u32 = 0;
    const CALLBACK: u32 = 1;
    const FLAGS: u32 = 2;
    const PREV: u32 = 3;
    const NEXT: u32 = 4;
    const CAPTURE: u8 = 1;
    const ONCE: u8 = 2;
    const PASSIVE: u8 = 4;

    fn new(agent: &mut Agent, event_type: Value, callback: Value, flags: u8) -> JsResult<Self> {
        Ok(Self(PrivateArray::new(
            agent,
            &[
                event_type,
                callback,
                Value::number(f64::from(flags)),
                Value::NULL,
                Value::NULL,
            ],
        )?))
    }

    fn from_value(value: Value) -> Option<Self> {
        PrivateArray::from_value(value).map(Self)
    }

    fn callback(self, agent: &mut Agent) -> JsResult<Value> {
        self.0.get(agent, Self::CALLBACK)
    }

    fn flags(self, agent: &mut Agent) -> JsResult<u8> {
        let flags = self.0.get(agent, Self::FLAGS)?;
        // This field only receives the flags defined above.
        Ok(flags.as_number().expect("listener flags") as u8)
    }

    fn matches(self, agent: &mut Agent, event_type: Value, capture: bool) -> JsResult<bool> {
        let listener_type = self.0.get(agent, Self::TYPE)?;
        Ok(listener_type == event_type && (self.flags(agent)? & Self::CAPTURE != 0) == capture)
    }

    fn next(self, agent: &mut Agent) -> JsResult<Option<Self>> {
        Ok(Self::from_value(self.0.get(agent, Self::NEXT)?))
    }
}

fn event_path(agent: &mut Agent, target: Target, event_type: Value) -> JsResult<PrivateArray> {
    if !target.node {
        return PrivateArray::new(agent, &[target.value]);
    }
    let load = agent
        .heap
        .string(event_type.as_string().expect("event type"))
        .units()
        .eq("load".encode_utf16());
    with_page(agent, |agent, page| {
        read_node(
            agent,
            target.value,
            |agent, wrappers, document, id, node| {
                let mut path = Vec::new();
                path.try_reserve(1).map_err(|_| OutOfMemory)?;
                path.push(target.value);
                let mut current = node.parent();
                let mut reached_document = id == document.root();
                while let Some(id) = current {
                    agent.check_native_work()?;
                    path.try_reserve(1).map_err(|_| OutOfMemory)?;
                    path.push(wrap(wrappers, agent, document, id)?);
                    reached_document = id == document.root();
                    current = document
                        .node(id)
                        .map_err(|error| crate::dom_exception::from_dom_error(agent, error))?
                        .parent();
                }
                if reached_document && document.id() == page.main && !load {
                    path.try_reserve(1).map_err(|_| OutOfMemory)?;
                    path.push(page.window);
                }
                PrivateArray::new(agent, &path)
            },
        )
    })
}

fn dispatch_event(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let target = receiver(agent, this)?;
    let value = required_arg(agent, args, 0)?;
    let event = Event::receiver(agent, value)?;
    if event.is_dispatching(agent)? {
        return Err(super::creation::exception(
            agent,
            "InvalidStateError",
            "event is already dispatching",
        ));
    }
    if event.has(agent, Event::UNINITIALIZED)? {
        return Err(super::creation::exception(
            agent,
            "InvalidStateError",
            "event must be initialized before dispatch",
        ));
    }
    event.untrust(agent)?;
    fire(agent, target, value, event).map(Value::boolean)
}

pub(super) fn create_legacy_event(agent: &mut Agent, name: &str) -> JsResult<Value> {
    match name {
        "event" | "events" | "htmlevents" | "svgevents" => state::legacy_event(agent, false),
        "customevent" => state::legacy_event(agent, true),
        name => ui::legacy_event(agent, name).unwrap_or_else(|| {
            Err(super::creation::exception(
                agent,
                "NotSupportedError",
                "event interface is unavailable",
            ))
        }),
    }
}

fn fire(agent: &mut Agent, target: Target, value: Value, event: Event) -> JsResult<bool> {
    fire_with_target(agent, target, value, event, target.value)
}

fn fire_with_target(
    agent: &mut Agent,
    target: Target,
    value: Value,
    event: Event,
    event_target: Value,
) -> JsResult<bool> {
    let event_type = event.event_type(agent)?;
    let path = event_path(agent, target, event_type)?;
    with_root(agent, path.value(), |agent| {
        let key = type_key(agent, event_type)?;
        with_root(agent, key, |agent| {
            let click = target.node
                && event.has(agent, Event::MOUSE)?
                && agent
                    .heap
                    .string(event_type.as_string().expect("event type"))
                    .units()
                    .eq("click".encode_utf16());
            let checkbox = if click {
                let bubbles = event.bubbles(agent)?;
                input::activation_target(agent, path, bubbles)?
            } else {
                None
            };
            let previous = checkbox
                .map(|target| input::preactivate(agent, target))
                .transpose()?;
            event.begin(agent, event_target, path)?;
            let result = dispatch(agent, event, value, key, path);
            event.finish(agent)?;
            let canceled = event.canceled(agent)?;
            if let Some((activation, previous)) = checkbox.zip(previous) {
                input::finish_activation(agent, activation, previous, canceled || result.is_err())?;
            }
            result?;
            Ok(!canceled)
        })
    })
}

/// Trusted events produced by the local script host and parser.
#[derive(Clone, Copy, Debug)]
pub enum LifecycleEvent {
    /// The document's readiness changed.
    ReadyStateChange,
    /// Deferred execution reached the end of its ordered queue.
    DomContentLoaded,
    /// The document has finished loading.
    WindowLoad,
    /// An external script acquired executable source.
    ScriptLoad(NodeId),
    /// Script source or its dependency graph failed to load.
    ScriptError(NodeId),
}

pub(super) fn deliver_lifecycle(agent: &mut Agent, event: LifecycleEvent) -> JsResult<()> {
    with_page(agent, |agent, page| {
        let (target, kind, flags) = match event {
            LifecycleEvent::ReadyStateChange => (
                Target {
                    value: page.root,
                    node: true,
                },
                "readystatechange",
                0,
            ),
            LifecycleEvent::DomContentLoaded => (
                Target {
                    value: page.root,
                    node: true,
                },
                "DOMContentLoaded",
                Event::BUBBLES,
            ),
            LifecycleEvent::WindowLoad => (
                Target {
                    value: page.window,
                    node: false,
                },
                "load",
                0,
            ),
            LifecycleEvent::ScriptLoad(node) | LifecycleEvent::ScriptError(node) => {
                let mut wrappers = page.wrappers.borrow_mut();
                let target = page.read(agent, node.document(), |agent, document| {
                    wrap(&mut wrappers, agent, document, node)
                })?;
                let kind = if matches!(event, LifecycleEvent::ScriptLoad(_)) {
                    "load"
                } else {
                    "error"
                };
                (
                    Target {
                        value: target,
                        node: true,
                    },
                    kind,
                    0,
                )
            }
        };
        let exposed_target = if matches!(event, LifecycleEvent::WindowLoad) {
            page.root
        } else {
            target.value
        };
        with_root(agent, target.value, |agent| {
            let value =
                state::trusted_event(agent, "Event", kind, flags, Value::NULL, Value::NULL)?;
            with_root(agent, value, |agent| {
                let state = Event::receiver(agent, value)?;
                fire_with_target(agent, target, value, state, exposed_target).map(|_| ())
            })
        })
    })
}

fn dispatch(
    agent: &mut Agent,
    event: Event,
    value: Value,
    event_type: Value,
    path: PrivateArray,
) -> JsResult<()> {
    let len = path.len(agent)?;
    let bubbles = event.bubbles(agent)?;
    for index in (0..len).rev() {
        if event.stopped(agent)? {
            break;
        }
        let target = path.get(agent, index)?;
        let phase = if index == 0 {
            Phase::AtTarget
        } else {
            Phase::Capturing
        };
        invoke(agent, event, value, event_type, target, phase, true)?;
    }
    for index in 0..if bubbles { len } else { 1 } {
        if event.stopped(agent)? {
            break;
        }
        let target = path.get(agent, index)?;
        let phase = if index == 0 {
            Phase::AtTarget
        } else {
            Phase::Bubbling
        };
        invoke(agent, event, value, event_type, target, phase, false)?;
    }
    Ok(())
}

fn invoke(
    agent: &mut Agent,
    event: Event,
    value: Value,
    event_type: Value,
    target: Value,
    phase: Phase,
    capture: bool,
) -> JsResult<()> {
    agent.check_native_work()?;
    let Some(list) = listener_list(agent, target)? else {
        return Ok(());
    };
    let Some(listeners) = list.snapshot(agent, event_type, capture)? else {
        return Ok(());
    };
    event.current(agent, target, phase)?;
    with_root(agent, listeners.value(), |agent| {
        let len = listeners.len(agent)?;
        for index in 0..len {
            agent.check_native_work()?;
            let listener =
                Listener::from_value(listeners.get(agent, index)?).expect("listener snapshot");
            let callback = listener.callback(agent)?;
            if callback == Value::UNDEFINED {
                continue;
            }
            let flags = listener.flags(agent)?;
            let result = with_root(agent, callback, |agent| {
                if flags & Listener::ONCE != 0 {
                    list.remove(agent, listener)?;
                }
                event.set_passive(agent, flags & Listener::PASSIVE != 0)?;
                call_listener(agent, callback, target, value)
            });
            event.set_passive(agent, false)?;
            match result {
                Ok(_) => {}
                Err(Abrupt::Throw(_)) => with_page(agent, |_, page| {
                    page.events.report_listener_error();
                    Ok(())
                })?,
                Err(error) => return Err(error),
            }
            if event.immediate(agent)? {
                break;
            }
        }
        Ok(())
    })
}

fn call_listener(
    agent: &mut Agent,
    callback: Value,
    target: Value,
    event: Value,
) -> JsResult<Value> {
    if agent.is_callable(callback) {
        return agent.call(callback, target, &[event]);
    }
    let key = agent.heap.key("handleEvent")?;
    let Some(method) = agent.get_method(callback, key)? else {
        return agent.type_error("event listener has no handleEvent method");
    };
    agent.call(method, callback, &[event])
}
