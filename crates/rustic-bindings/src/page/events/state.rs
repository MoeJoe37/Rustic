//! Event and CustomEvent slots and their script interfaces.

use rustic_js::runtime::{
    Agent, JsResult, NativeFn, ObjectId, OutOfMemory, PropertyDescriptor, Value,
};

use crate::webidl::coerce_string;

use super::super::{install_interface, required_arg, with_page};
use super::{PrivateArray, define_method, option, with_root};

#[derive(Clone, Copy)]
pub(super) enum Phase {
    None = 0,
    Capturing = 1,
    AtTarget = 2,
    Bubbling = 3,
}

impl Phase {
    fn value(self) -> Value {
        Value::number(f64::from(self as u8))
    }
}

#[derive(Clone, Copy)]
pub(super) struct Event(PrivateArray);

impl Event {
    const TYPE: u32 = 0;
    const FLAGS: u32 = 1;
    const TARGET: u32 = 2;
    const CURRENT_TARGET: u32 = 3;
    const PHASE: u32 = 4;
    const PATH: u32 = 5;
    const TIMESTAMP: u32 = 6;
    const DETAIL: u32 = 7;
    const EXTRA: u32 = 8;

    pub(super) const BUBBLES: u16 = 1 << 0;
    pub(super) const CANCELABLE: u16 = 1 << 1;
    pub(super) const COMPOSED: u16 = 1 << 2;
    const DISPATCHING: u16 = 1 << 3;
    const STOPPED: u16 = 1 << 4;
    const IMMEDIATE: u16 = 1 << 5;
    const CANCELED: u16 = 1 << 6;
    const PASSIVE: u16 = 1 << 7;
    const CUSTOM: u16 = 1 << 8;
    const TRUSTED: u16 = 1 << 9;
    pub(super) const UI: u16 = 1 << 10;
    pub(super) const MOUSE: u16 = 1 << 11;
    pub(super) const KEYBOARD: u16 = 1 << 12;
    pub(super) const POINTER: u16 = 1 << 13;

    pub(super) fn receiver(agent: &mut Agent, value: Value) -> JsResult<Self> {
        with_page(agent, |agent, page| {
            let record = agent.weak_map_get(page.events.states, value);
            match PrivateArray::from_value(record) {
                Some(record) => Ok(Self(record)),
                None => agent.type_error("expected an Event receiver"),
            }
        })
    }

    fn flags(self, agent: &mut Agent) -> JsResult<u16> {
        let value = self.0.get(agent, Self::FLAGS)?;
        // The private field is only set by set_flags and construction.
        Ok(value.as_number().expect("event flags") as u16)
    }

    fn set_flags(self, agent: &mut Agent, flags: u16) -> JsResult<()> {
        self.0
            .set(agent, Self::FLAGS, Value::number(f64::from(flags)))
    }

    pub(super) fn has(self, agent: &mut Agent, flag: u16) -> JsResult<bool> {
        Ok(self.flags(agent)? & flag != 0)
    }

    pub(super) fn event_type(self, agent: &mut Agent) -> JsResult<Value> {
        self.0.get(agent, Self::TYPE)
    }

    pub(super) fn is_dispatching(self, agent: &mut Agent) -> JsResult<bool> {
        self.has(agent, Self::DISPATCHING)
    }

    pub(super) fn untrust(self, agent: &mut Agent) -> JsResult<()> {
        let flags = self.flags(agent)?;
        self.set_flags(agent, flags & !Self::TRUSTED)
    }

    pub(super) fn detail(self, agent: &mut Agent) -> JsResult<Value> {
        self.0.get(agent, Self::DETAIL)
    }

    pub(super) fn extra(self, agent: &mut Agent) -> JsResult<PrivateArray> {
        let value = self.0.get(agent, Self::EXTRA)?;
        Ok(PrivateArray::from_value(value).expect("UI event fields"))
    }

    pub(super) fn bubbles(self, agent: &mut Agent) -> JsResult<bool> {
        self.has(agent, Self::BUBBLES)
    }

    pub(super) fn canceled(self, agent: &mut Agent) -> JsResult<bool> {
        self.has(agent, Self::CANCELED)
    }

    pub(super) fn stopped(self, agent: &mut Agent) -> JsResult<bool> {
        self.has(agent, Self::STOPPED)
    }

    pub(super) fn immediate(self, agent: &mut Agent) -> JsResult<bool> {
        self.has(agent, Self::IMMEDIATE)
    }

    pub(super) fn set_passive(self, agent: &mut Agent, passive: bool) -> JsResult<()> {
        let flags = self.flags(agent)? & !Self::PASSIVE;
        self.set_flags(agent, flags | (u16::from(passive) * Self::PASSIVE))
    }

    pub(super) fn begin(
        self,
        agent: &mut Agent,
        target: Value,
        path: PrivateArray,
    ) -> JsResult<()> {
        let flags = self.flags(agent)?;
        self.0.set(agent, Self::TARGET, target)?;
        self.0.set(agent, Self::PATH, path.value())?;
        self.set_flags(agent, flags | Self::DISPATCHING)
    }

    pub(super) fn current(self, agent: &mut Agent, target: Value, phase: Phase) -> JsResult<()> {
        self.0.set(agent, Self::CURRENT_TARGET, target)?;
        self.0.set(agent, Self::PHASE, phase.value())
    }

    pub(super) fn finish(self, agent: &mut Agent) -> JsResult<()> {
        let flags = self.flags(agent)?;
        self.set_flags(
            agent,
            flags & !(Self::DISPATCHING | Self::STOPPED | Self::IMMEDIATE | Self::PASSIVE),
        )?;
        self.0.set(agent, Self::PATH, Value::NULL)?;
        self.current(agent, Value::NULL, Phase::None)
    }

    fn prevent_default(self, agent: &mut Agent) -> JsResult<()> {
        let flags = self.flags(agent)?;
        if flags & Self::CANCELABLE != 0 && flags & Self::PASSIVE == 0 {
            self.set_flags(agent, flags | Self::CANCELED)?;
        }
        Ok(())
    }

    fn stop(self, agent: &mut Agent, flags: u16) -> JsResult<()> {
        let updated = self.flags(agent)? | Self::STOPPED | flags;
        self.set_flags(agent, updated)
    }

    fn init(
        self,
        agent: &mut Agent,
        event_type: Value,
        bubbles: bool,
        cancelable: bool,
    ) -> JsResult<()> {
        let flags = self.flags(agent)?
            & (Self::CUSTOM
                | Self::COMPOSED
                | Self::UI
                | Self::MOUSE
                | Self::POINTER
                | Self::KEYBOARD);
        let flags = flags
            | (u16::from(bubbles) * Self::BUBBLES)
            | (u16::from(cancelable) * Self::CANCELABLE);
        self.0.set(agent, Self::TYPE, event_type)?;
        self.0.set(agent, Self::TARGET, Value::NULL)?;
        self.set_flags(agent, flags)
    }
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        "Event",
        None,
        construct::<false>,
        1,
        &[
            ("type", "get type", get_field::<{ Event::TYPE }>),
            ("target", "get target", get_field::<{ Event::TARGET }>),
            (
                "srcElement",
                "get srcElement",
                get_field::<{ Event::TARGET }>,
            ),
            (
                "currentTarget",
                "get currentTarget",
                get_field::<{ Event::CURRENT_TARGET }>,
            ),
            (
                "eventPhase",
                "get eventPhase",
                get_field::<{ Event::PHASE }>,
            ),
            (
                "timeStamp",
                "get timeStamp",
                get_field::<{ Event::TIMESTAMP }>,
            ),
            ("bubbles", "get bubbles", get_flag::<{ Event::BUBBLES }>),
            (
                "cancelable",
                "get cancelable",
                get_flag::<{ Event::CANCELABLE }>,
            ),
            ("composed", "get composed", get_flag::<{ Event::COMPOSED }>),
            (
                "defaultPrevented",
                "get defaultPrevented",
                get_flag::<{ Event::CANCELED }>,
            ),
        ],
        |agent, prototype| {
            for (name, method, length) in [
                ("stopPropagation", stop_propagation as NativeFn, 0),
                ("stopImmediatePropagation", stop_immediate, 0),
                ("preventDefault", prevent_default, 0),
                ("composedPath", composed_path, 0),
                ("initEvent", init_event, 1),
            ] {
                define_method(agent, prototype, name, method, length)?;
            }
            for (name, getter, setter) in [
                (
                    "cancelBubble",
                    get_flag::<{ Event::STOPPED }> as NativeFn,
                    set_cancel_bubble as NativeFn,
                ),
                ("returnValue", return_value, set_return_value),
            ] {
                let get = agent.new_native_function(getter, &format!("get {name}"), 0)?;
                let set = agent.new_native_function(setter, &format!("set {name}"), 1)?;
                let key = agent.heap.key(name)?;
                agent.define_property_or_throw(
                    prototype,
                    key,
                    PropertyDescriptor::accessor(
                        Value::object(get),
                        Value::object(set),
                        true,
                        true,
                    ),
                )?;
            }
            Ok(())
        },
    )?;
    let prototype = super::prototype(agent, "Event")?;
    let constructor_key = agent.heap.key("Event")?;
    let constructor = agent
        .get(agent.intrinsics.global, constructor_key)?
        .as_object()
        .expect("Event constructor");
    for (name, phase) in [
        ("NONE", Phase::None),
        ("CAPTURING_PHASE", Phase::Capturing),
        ("AT_TARGET", Phase::AtTarget),
        ("BUBBLING_PHASE", Phase::Bubbling),
    ] {
        let key = agent.heap.key(name)?;
        let desc = PropertyDescriptor::data_with(phase.value(), false, true, false);
        agent.define_property_or_throw(prototype, key, desc)?;
        agent.define_property_or_throw(constructor, key, desc)?;
    }
    install_interface(
        agent,
        "CustomEvent",
        Some("Event"),
        construct::<true>,
        1,
        &[("detail", "get detail", detail)],
        |agent, prototype| define_method(agent, prototype, "initCustomEvent", init_custom_event, 1),
    )
}

fn construct<const CUSTOM: bool>(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("Event constructor requires new");
    };
    let event_type = required_arg(agent, args, 0)?;
    let event_type = Value::string(coerce_string(agent, event_type)?);
    with_root(agent, event_type, |agent| {
        let options = args.get(1).copied().unwrap_or(Value::UNDEFINED);
        let (flags, detail) = init_options::<CUSTOM>(agent, options)?;
        with_root(agent, detail, |agent| {
            let interface = if CUSTOM { "CustomEvent" } else { "Event" };
            let prototype = agent.host_prototype_from_new_target(interface, new_target)?;
            create(agent, prototype, event_type, flags, detail, Value::NULL)
        })
    })
}

pub(super) fn create(
    agent: &mut Agent,
    prototype: ObjectId,
    event_type: Value,
    flags: u16,
    detail: Value,
    extra: Value,
) -> JsResult<Value> {
    let object = agent.new_object_with(Some(prototype))?;
    let value = Value::object(object);
    with_page(agent, |agent, page| {
        let key = agent.heap.key("isTrusted")?;
        agent.define_property_or_throw(
            object,
            key,
            PropertyDescriptor::accessor(page.events.is_trusted, Value::UNDEFINED, true, false),
        )?;
        let timestamp = Value::number(page.events.started.elapsed().as_secs_f64() * 1000.0);
        let record = PrivateArray::new(
            agent,
            &[
                event_type,
                Value::number(f64::from(flags)),
                Value::NULL,
                Value::NULL,
                Phase::None.value(),
                Value::NULL,
                timestamp,
                detail,
                extra,
            ],
        )?;
        agent.weak_map_set(page.events.states, value, record.value())?;
        Ok(value)
    })
}

pub(super) fn trusted_event(
    agent: &mut Agent,
    interface: &'static str,
    event_type: &str,
    flags: u16,
    detail: Value,
    extra: Value,
) -> JsResult<Value> {
    let prototype = super::prototype(agent, interface)?;
    let event_type = agent.string(event_type)?;
    create(
        agent,
        prototype,
        event_type,
        flags | Event::TRUSTED,
        detail,
        extra,
    )
}

pub(super) fn init_options<const CUSTOM: bool>(
    agent: &mut Agent,
    options: Value,
) -> JsResult<(u16, Value)> {
    let mut flags = if CUSTOM { Event::CUSTOM } else { 0 };
    let mut detail = Value::NULL;
    if !options.is_nullish() {
        let Some(object) = options.as_object() else {
            return agent.type_error("event init must be a dictionary");
        };
        for (name, bit) in [
            ("bubbles", Event::BUBBLES),
            ("cancelable", Event::CANCELABLE),
            ("composed", Event::COMPOSED),
        ] {
            let value = option(agent, object, name)?;
            if agent.to_boolean(value) {
                flags |= bit;
            }
        }
        if CUSTOM {
            let value = option(agent, object, "detail")?;
            if value != Value::UNDEFINED {
                detail = value;
            }
        }
    }
    Ok((flags, detail))
}

fn get_field<const INDEX: u32>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Event::receiver(agent, this)?.0.get(agent, INDEX)
}

fn get_flag<const FLAG: u16>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let event = Event::receiver(agent, this)?;
    Ok(Value::boolean(event.has(agent, FLAG)?))
}

pub(super) fn trusted_getter(agent: &mut Agent) -> JsResult<Value> {
    agent
        .new_native_function(is_trusted, "get isTrusted", 0)
        .map(Value::object)
}

fn is_trusted(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let event = Event::receiver(agent, this)?;
    Ok(Value::boolean(event.has(agent, Event::TRUSTED)?))
}

fn detail(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    custom_receiver(agent, this)?.0.get(agent, Event::DETAIL)
}

fn custom_receiver(agent: &mut Agent, this: Value) -> JsResult<Event> {
    let event = Event::receiver(agent, this)?;
    if !event.has(agent, Event::CUSTOM)? {
        return agent.type_error("expected a CustomEvent receiver");
    }
    Ok(event)
}

fn stop_propagation(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Event::receiver(agent, this)?.stop(agent, 0)?;
    Ok(Value::UNDEFINED)
}

fn stop_immediate(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Event::receiver(agent, this)?.stop(agent, Event::IMMEDIATE)?;
    Ok(Value::UNDEFINED)
}

fn prevent_default(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Event::receiver(agent, this)?.prevent_default(agent)?;
    Ok(Value::UNDEFINED)
}

fn set_cancel_bubble(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let event = Event::receiver(agent, this)?;
    if agent.to_boolean(args.first().copied().unwrap_or(Value::UNDEFINED)) {
        event.stop(agent, 0)?;
    }
    Ok(Value::UNDEFINED)
}

fn return_value(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let event = Event::receiver(agent, this)?;
    Ok(Value::boolean(!event.canceled(agent)?))
}

fn set_return_value(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let event = Event::receiver(agent, this)?;
    if !agent.to_boolean(args.first().copied().unwrap_or(Value::UNDEFINED)) {
        event.prevent_default(agent)?;
    }
    Ok(Value::UNDEFINED)
}

fn composed_path(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let event = Event::receiver(agent, this)?;
    let path = event.0.get(agent, Event::PATH)?;
    let mut values = Vec::new();
    if let Some(path) = PrivateArray::from_value(path) {
        let len = path.len(agent)?;
        values
            .try_reserve_exact(len as usize)
            .map_err(|_| OutOfMemory)?;
        for index in 0..len {
            agent.check_native_work()?;
            values.push(path.get(agent, index)?);
        }
    }
    agent.create_array_from_list(&values).map(Value::object)
}

fn init_event(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let event = Event::receiver(agent, this)?;
    reinitialize(agent, event, args, false)
}

fn init_custom_event(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let event = custom_receiver(agent, this)?;
    reinitialize(agent, event, args, true)
}

fn reinitialize(agent: &mut Agent, event: Event, args: &[Value], custom: bool) -> JsResult<Value> {
    let event_type = required_arg(agent, args, 0)?;
    let event_type = Value::string(coerce_string(agent, event_type)?);
    let bubbles = agent.to_boolean(args.get(1).copied().unwrap_or(Value::UNDEFINED));
    let cancelable = agent.to_boolean(args.get(2).copied().unwrap_or(Value::UNDEFINED));
    if !event.is_dispatching(agent)? {
        event.init(agent, event_type, bubbles, cancelable)?;
        if custom {
            let detail = args
                .get(3)
                .copied()
                .filter(|&value| value != Value::UNDEFINED)
                .unwrap_or(Value::NULL);
            event.0.set(agent, Event::DETAIL, detail)?;
        }
    }
    Ok(Value::UNDEFINED)
}
