//! `TransitionEvent`, `AnimationEvent` and `AnimationPlaybackEvent`
//! ([CSS Transitions §6], [CSS Animations §4], [Web Animations §6.10]),
//! and firing them for the animation controller.
//!
//! The event-specific fields live in the event record's extra slot:
//! `[name, elapsedTime, pseudoElement]` for the CSS events and
//! `[currentTime, timelineTime]` for playback events.
//!
//! [CSS Transitions §6]: https://drafts.csswg.org/css-transitions/#interface-transitionevent
//! [CSS Animations §4]: https://drafts.csswg.org/css-animations/#interface-animationevent
//! [Web Animations §6.10]: https://drafts.csswg.org/web-animations-1/#the-animationplaybackevent-interface

use rustic_dom::NodeId;
use rustic_js::runtime::{Agent, JsResult, Value};

use crate::webidl::{DomStringConversion, coerce_string, from_dom_string, to_dom_string};

use super::super::{install_interface, required_arg, with_page, wrap};
use super::state::{self, Event};
use super::{ListenerList, PrivateArray, Target, fire_with_target, option, with_root};

/// The CSS event interfaces.
pub(in crate::page) const TRANSITION_EVENT: &str = "TransitionEvent";
pub(in crate::page) const ANIMATION_EVENT: &str = "AnimationEvent";
const PLAYBACK_EVENT: &str = "AnimationPlaybackEvent";

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        TRANSITION_EVENT,
        Some("Event"),
        construct::<0>,
        1,
        &[
            ("propertyName", "get propertyName", field::<0>),
            ("elapsedTime", "get elapsedTime", field::<1>),
            ("pseudoElement", "get pseudoElement", field::<2>),
        ],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        ANIMATION_EVENT,
        Some("Event"),
        construct::<1>,
        1,
        &[
            ("animationName", "get animationName", field::<0>),
            ("elapsedTime", "get elapsedTime", field::<1>),
            ("pseudoElement", "get pseudoElement", field::<2>),
        ],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        PLAYBACK_EVENT,
        Some("Event"),
        construct::<2>,
        1,
        &[
            ("currentTime", "get currentTime", field::<0>),
            ("timelineTime", "get timelineTime", field::<1>),
        ],
        |_, _| Ok(()),
    )
}

const INTERFACES: [&str; 3] = [TRANSITION_EVENT, ANIMATION_EVENT, PLAYBACK_EVENT];

fn construct<const KIND: usize>(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let interface = INTERFACES[KIND];
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("event constructor requires new");
    };
    let event_type = required_arg(agent, args, 0)?;
    let event_type = Value::string(coerce_string(agent, event_type)?);
    with_root(agent, event_type, |agent| {
        let options = args.get(1).copied().unwrap_or(Value::UNDEFINED);
        let (flags, _) = state::init_options::<false>(agent, options)?;
        let empty = agent.string("")?;
        let mut fields = if KIND == 2 {
            vec![Value::NULL, Value::NULL]
        } else {
            vec![empty, Value::number(0.0), empty]
        };
        if let Some(object) = options.as_object() {
            let names: &[&str] = match KIND {
                0 => &["propertyName", "elapsedTime", "pseudoElement"],
                1 => &["animationName", "elapsedTime", "pseudoElement"],
                _ => &["currentTime", "timelineTime"],
            };
            for (slot, name) in names.iter().enumerate() {
                let value = option(agent, object, name)?;
                if value == Value::UNDEFINED {
                    continue;
                }
                fields[slot] = if KIND == 2 {
                    if value == Value::NULL {
                        Value::NULL
                    } else {
                        Value::number(agent.to_number(value)?)
                    }
                } else if slot == 1 {
                    let number = agent.to_number(value)?;
                    if !number.is_finite() {
                        return agent.type_error("elapsedTime must be finite");
                    }
                    Value::number(number)
                } else {
                    let text = to_dom_string(agent, value, DomStringConversion::Default)?;
                    from_dom_string(agent, &text)?
                };
            }
        }
        let extra = PrivateArray::new(agent, &fields)?;
        with_root(agent, extra.value(), |agent| {
            let prototype = agent.host_prototype_from_new_target(interface, new_target)?;
            state::create(
                agent,
                prototype,
                event_type,
                flags,
                Value::NULL,
                extra.value(),
            )
        })
    })
}

fn field<const SLOT: u32>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let event = Event::receiver(agent, this)?;
    let extra = event.extra_value(agent)?;
    match PrivateArray::from_value(extra) {
        Some(fields) => fields.get(agent, SLOT),
        None => agent.type_error("expected an animation event receiver"),
    }
}

/// Fire a trusted CSS transition or animation event at `node`; a node
/// that went away is skipped. CSS transition and animation events bubble
/// and cannot be canceled.
pub(in crate::page) fn fire_css(
    agent: &mut Agent,
    interface: &'static str,
    node: NodeId,
    event_type: &str,
    name: &str,
    elapsed_seconds: f64,
) -> JsResult<()> {
    let target = with_page(agent, |agent, page| {
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return Ok(None);
        };
        page.read(agent, node.document(), |agent, document| {
            if document.node(node).is_err() {
                return Ok(None);
            }
            wrap(&mut wrappers, agent, document, node).map(Some)
        })
    })?;
    let Some(target) = target else {
        return Ok(());
    };
    with_root(agent, target, |agent| {
        let name = agent.string(name)?;
        let empty = agent.string("")?;
        let extra = PrivateArray::new(agent, &[name, Value::number(elapsed_seconds), empty])?;
        with_root(agent, extra.value(), |agent| {
            let event = state::trusted_event(
                agent,
                interface,
                event_type,
                Event::BUBBLES,
                Value::NULL,
                extra.value(),
            )?;
            with_root(agent, event, |agent| {
                let state = Event::receiver(agent, event)?;
                let target = Target {
                    value: target,
                    node: true,
                };
                fire_with_target(agent, target, event, state, target.value).map(drop)
            })
        })
    })
}

/// Fire a trusted `AnimationPlaybackEvent` at an Animation object.
pub(in crate::page) fn fire_playback(
    agent: &mut Agent,
    target: Value,
    event_type: &str,
    current_time: Option<f64>,
    timeline_time: Option<f64>,
) -> JsResult<()> {
    let number = |time: Option<f64>| time.map_or(Value::NULL, Value::number);
    let extra = PrivateArray::new(agent, &[number(current_time), number(timeline_time)])?;
    with_root(agent, extra.value(), |agent| {
        let event = state::trusted_event(
            agent,
            PLAYBACK_EVENT,
            event_type,
            0,
            Value::NULL,
            extra.value(),
        )?;
        with_root(agent, event, |agent| {
            let state = Event::receiver(agent, event)?;
            let target = Target {
                value: target,
                node: false,
            };
            fire_with_target(agent, target, event, state, target.value).map(drop)
        })
    })
}

/// Make a host object (an Animation) an event target.
pub(in crate::page) fn register_target(agent: &mut Agent, target: Value) -> JsResult<()> {
    with_page(agent, |agent, page| {
        let listeners = ListenerList::new(agent)?;
        agent.weak_map_set(page.events.targets, target, listeners.0.value())
    })
}
