//! UI event interfaces and their native input payloads.

use rustic_js::runtime::{Agent, JsResult, ObjectId, Value};

use crate::webidl::{IntegerConversion, coerce_string, from_dom_string, to_long, to_unsigned_long};

use super::super::{install_interface, required_arg, with_page};
use super::input::{KeyInput, Modifiers};
use super::state::{self, Event};
use super::{PrivateArray, option, with_root};

const VIEW: u32 = 0;
const CONTROL: u32 = 1;
const ALT: u32 = 2;
const SHIFT: u32 = 3;
const META: u32 = 4;
const CLIENT_X: u32 = 5;
const CLIENT_Y: u32 = 6;
const BUTTON: u32 = 7;
const BUTTONS: u32 = 8;
const POINTER_ID: u32 = 9;
const POINTER_TYPE: u32 = 10;
const PRIMARY: u32 = 11;
const KEY: u32 = 5;
const CODE: u32 = 6;
const REPEAT: u32 = 7;
const LOCATION: u32 = 8;
const COMPOSING: u32 = 9;
const UI: u16 = Event::UI;
const MOUSE: u16 = UI | Event::MOUSE;
const POINTER: u16 = MOUSE | Event::POINTER;
const KEYBOARD: u16 = UI | Event::KEYBOARD;

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        "UIEvent",
        Some("Event"),
        construct::<UI>,
        1,
        &[
            ("view", "get view", field::<VIEW, { Event::UI }>),
            ("detail", "get detail", detail),
        ],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        "MouseEvent",
        Some("UIEvent"),
        construct::<MOUSE>,
        1,
        &[
            (
                "clientX",
                "get clientX",
                field::<CLIENT_X, { Event::MOUSE }>,
            ),
            (
                "clientY",
                "get clientY",
                field::<CLIENT_Y, { Event::MOUSE }>,
            ),
            ("button", "get button", field::<BUTTON, { Event::MOUSE }>),
            ("buttons", "get buttons", field::<BUTTONS, { Event::MOUSE }>),
            ("ctrlKey", "get ctrlKey", field::<CONTROL, { Event::MOUSE }>),
            ("altKey", "get altKey", field::<ALT, { Event::MOUSE }>),
            ("shiftKey", "get shiftKey", field::<SHIFT, { Event::MOUSE }>),
            ("metaKey", "get metaKey", field::<META, { Event::MOUSE }>),
        ],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        "PointerEvent",
        Some("MouseEvent"),
        construct::<POINTER>,
        1,
        &[
            (
                "pointerId",
                "get pointerId",
                field::<POINTER_ID, { Event::POINTER }>,
            ),
            (
                "pointerType",
                "get pointerType",
                field::<POINTER_TYPE, { Event::POINTER }>,
            ),
            (
                "isPrimary",
                "get isPrimary",
                field::<PRIMARY, { Event::POINTER }>,
            ),
        ],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        "KeyboardEvent",
        Some("UIEvent"),
        construct::<KEYBOARD>,
        1,
        &[
            ("key", "get key", field::<KEY, { Event::KEYBOARD }>),
            ("code", "get code", field::<CODE, { Event::KEYBOARD }>),
            ("repeat", "get repeat", field::<REPEAT, { Event::KEYBOARD }>),
            (
                "location",
                "get location",
                field::<LOCATION, { Event::KEYBOARD }>,
            ),
            (
                "isComposing",
                "get isComposing",
                field::<COMPOSING, { Event::KEYBOARD }>,
            ),
            (
                "ctrlKey",
                "get ctrlKey",
                field::<CONTROL, { Event::KEYBOARD }>,
            ),
            ("altKey", "get altKey", field::<ALT, { Event::KEYBOARD }>),
            (
                "shiftKey",
                "get shiftKey",
                field::<SHIFT, { Event::KEYBOARD }>,
            ),
            ("metaKey", "get metaKey", field::<META, { Event::KEYBOARD }>),
        ],
        |_, _| Ok(()),
    )
}

fn detail(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    ui_receiver::<UI>(agent, receiver)?.detail(agent)
}

fn field<const INDEX: u32, const BRAND: u16>(
    agent: &mut Agent,
    receiver: Value,
    _: &[Value],
) -> JsResult<Value> {
    ui_receiver::<BRAND>(agent, receiver)?
        .extra(agent)?
        .get(agent, INDEX)
}

fn ui_receiver<const BRAND: u16>(agent: &mut Agent, receiver: Value) -> JsResult<Event> {
    let event = Event::receiver(agent, receiver)?;
    if !event.has(agent, BRAND)? {
        return agent.type_error("invalid UI event receiver");
    }
    Ok(event)
}

fn interface(kind: u16) -> &'static str {
    match kind {
        UI => "UIEvent",
        MOUSE => "MouseEvent",
        POINTER => "PointerEvent",
        KEYBOARD => "KeyboardEvent",
        _ => unreachable!("installed UI event constructor"),
    }
}

fn construct<const KIND: u16>(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("UI event constructor requires new");
    };
    let event_type = required_arg(agent, args, 0)?;
    let event_type = Value::string(coerce_string(agent, event_type)?);
    let input = args.get(1).copied().unwrap_or(Value::UNDEFINED);
    create::<KIND>(agent, event_type, input, Some(new_target))
}

pub(super) fn legacy_event(agent: &mut Agent, name: &str) -> Option<JsResult<Value>> {
    let construct: fn(&mut Agent, Value, Value, Option<ObjectId>) -> JsResult<Value> = match name {
        "uievent" | "uievents" => create::<UI>,
        "mouseevent" | "mouseevents" => create::<MOUSE>,
        "keyboardevent" => create::<KEYBOARD>,
        _ => return None,
    };
    Some(
        agent
            .string("")
            .and_then(|event_type| construct(agent, event_type, Value::UNDEFINED, None)),
    )
}

fn create<const KIND: u16>(
    agent: &mut Agent,
    event_type: Value,
    input: Value,
    new_target: Option<ObjectId>,
) -> JsResult<Value> {
    with_root(agent, event_type, |agent| {
        let (flags, _) = state::init_options::<false>(agent, input)?;
        let options = input.as_object();
        let detail = long(agent, options, "detail")?;
        let view = value(agent, options, "view")?;
        let view = if view.is_nullish() {
            Value::NULL
        } else {
            with_page(agent, |agent, page| {
                if view == page.window {
                    Ok(view)
                } else {
                    agent.type_error("view must be a Window")
                }
            })?
        };
        let modifiers = if KIND == UI {
            Modifiers::default()
        } else {
            Modifiers {
                alt: boolean(agent, options, "altKey")?,
                control: boolean(agent, options, "ctrlKey")?,
                meta: boolean(agent, options, "metaKey")?,
                shift: boolean(agent, options, "shiftKey")?,
            }
        };
        let base = fields(view, modifiers);
        let extra = match KIND {
            UI => PrivateArray::new(agent, &base)?,
            MOUSE | POINTER => {
                let button = unsigned_short(agent, options, "button")?;
                let button = i16::from_ne_bytes(button.to_ne_bytes());
                let buttons = unsigned_short(agent, options, "buttons")?;
                let x = long(agent, options, "clientX")?;
                let y = long(agent, options, "clientY")?;
                let mouse = [
                    base[0],
                    base[1],
                    base[2],
                    base[3],
                    base[4],
                    number(x),
                    number(y),
                    number(button),
                    number(buttons),
                ];
                if KIND == POINTER {
                    let primary = boolean(agent, options, "isPrimary")?;
                    let id = long(agent, options, "pointerId")?;
                    let kind = string(agent, options, "pointerType")?;
                    PrivateArray::new(
                        agent,
                        &[
                            mouse[0],
                            mouse[1],
                            mouse[2],
                            mouse[3],
                            mouse[4],
                            mouse[5],
                            mouse[6],
                            mouse[7],
                            mouse[8],
                            number(id),
                            kind,
                            Value::boolean(primary),
                        ],
                    )?
                } else {
                    PrivateArray::new(agent, &mouse)?
                }
            }
            KEYBOARD => {
                let code = string(agent, options, "code")?;
                with_root(agent, code, |agent| {
                    let composing = boolean(agent, options, "isComposing")?;
                    let key = string(agent, options, "key")?;
                    with_root(agent, key, |agent| {
                        let location = unsigned_long(agent, options, "location")?;
                        let repeat = boolean(agent, options, "repeat")?;
                        keyboard_fields(agent, base, key, code, repeat, location, composing)
                    })
                })?
            }
            _ => unreachable!("installed UI event constructor"),
        };
        with_root(agent, extra.value(), |agent| {
            let (prototype, initial) = match new_target {
                Some(target) => (
                    agent.host_prototype_from_new_target(interface(KIND), target)?,
                    0,
                ),
                None => (
                    super::prototype(agent, interface(KIND))?,
                    Event::UNINITIALIZED,
                ),
            };
            state::create(
                agent,
                prototype,
                event_type,
                flags | KIND | initial,
                number(detail),
                extra.value(),
            )
        })
    })
}

fn value(agent: &mut Agent, options: Option<ObjectId>, name: &str) -> JsResult<Value> {
    match options {
        Some(object) => option(agent, object, name),
        None => Ok(Value::UNDEFINED),
    }
}

fn boolean(agent: &mut Agent, options: Option<ObjectId>, name: &str) -> JsResult<bool> {
    let value = value(agent, options, name)?;
    Ok(agent.to_boolean(value))
}

fn long(agent: &mut Agent, options: Option<ObjectId>, name: &str) -> JsResult<i32> {
    let value = value(agent, options, name)?;
    to_long(agent, value, IntegerConversion::Wrap)
}

fn unsigned_long(agent: &mut Agent, options: Option<ObjectId>, name: &str) -> JsResult<u32> {
    let value = value(agent, options, name)?;
    to_unsigned_long(agent, value, IntegerConversion::Wrap)
}

fn unsigned_short(agent: &mut Agent, options: Option<ObjectId>, name: &str) -> JsResult<u16> {
    // Both IDL short types wrap modulo 2^16.
    let units = unsigned_long(agent, options, name)? & u32::from(u16::MAX);
    Ok(u16::try_from(units).expect("16-bit mask"))
}

fn string(agent: &mut Agent, options: Option<ObjectId>, name: &str) -> JsResult<Value> {
    let value = value(agent, options, name)?;
    if value == Value::UNDEFINED {
        return agent.string("");
    }
    coerce_string(agent, value).map(Value::string)
}

fn fields(view: Value, modifiers: Modifiers) -> [Value; 5] {
    [
        view,
        Value::boolean(modifiers.control),
        Value::boolean(modifiers.alt),
        Value::boolean(modifiers.shift),
        Value::boolean(modifiers.meta),
    ]
}

fn number(value: impl Into<f64>) -> Value {
    Value::number(value.into())
}

fn keyboard_fields(
    agent: &mut Agent,
    base: [Value; 5],
    key: Value,
    code: Value,
    repeat: bool,
    location: u32,
    composing: bool,
) -> JsResult<PrivateArray> {
    PrivateArray::new(
        agent,
        &[
            base[0],
            base[1],
            base[2],
            base[3],
            base[4],
            key,
            code,
            Value::boolean(repeat),
            number(location),
            Value::boolean(composing),
        ],
    )
}

const INPUT_FLAGS: u16 = Event::BUBBLES | Event::CANCELABLE | Event::COMPOSED;

pub(super) fn key_event(agent: &mut Agent, kind: &str, input: &KeyInput) -> JsResult<Value> {
    let view = with_page(agent, |_, page| Ok(page.window))?;
    let key = from_dom_string(agent, &input.key)?;
    let code = from_dom_string(agent, &input.code)?;
    let record = keyboard_fields(
        agent,
        fields(view, input.modifiers),
        key,
        code,
        input.repeat,
        input.location,
        false,
    )?;
    state::trusted_event(
        agent,
        "KeyboardEvent",
        kind,
        INPUT_FLAGS | KEYBOARD,
        number(0),
        record.value(),
    )
}

pub(super) fn click_event(
    agent: &mut Agent,
    coordinates: Option<(i32, i32)>,
    modifiers: Modifiers,
) -> JsResult<Value> {
    let view = with_page(agent, |_, page| Ok(page.window))?;
    let base = fields(view, modifiers);
    let pointer = coordinates.is_some();
    let (x, y) = coordinates.unwrap_or_default();
    let pointer_type = agent.string(if pointer { "mouse" } else { "" })?;
    let record = PrivateArray::new(
        agent,
        &[
            base[0],
            base[1],
            base[2],
            base[3],
            base[4],
            number(x),
            number(y),
            number(0),
            number(0),
            number(if pointer { 1 } else { -1 }),
            pointer_type,
            Value::boolean(pointer),
        ],
    )?;
    state::trusted_event(
        agent,
        "PointerEvent",
        "click",
        INPUT_FLAGS | POINTER,
        number(i32::from(pointer)),
        record.value(),
    )
}
