//! Geometry Interfaces rectangles backed by GC-traced private state.

use rustic_js::runtime::{Agent, JsResult, NativeFn, ObjectId, PropertyDescriptor, Value};

use super::events::{PrivateArray, define_method};
use super::{install_interface, prototype, with_page};

const READONLY: &str = "DOMRectReadOnly";
const MUTABLE: &str = "DOMRect";
const MEMBERS: [&str; 8] = [
    "x", "y", "width", "height", "top", "right", "bottom", "left",
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        READONLY,
        None,
        construct::<false>,
        0,
        &[
            ("x", "get x", get::<0>),
            ("y", "get y", get::<1>),
            ("width", "get width", get::<2>),
            ("height", "get height", get::<3>),
            ("top", "get top", get::<4>),
            ("right", "get right", get::<5>),
            ("bottom", "get bottom", get::<6>),
            ("left", "get left", get::<7>),
        ],
        |agent, proto| define_method(agent, proto, "toJSON", to_json, 0),
    )?;
    install_interface(
        agent,
        MUTABLE,
        Some(READONLY),
        construct::<true>,
        0,
        &[],
        |agent, proto| {
            for (name, getter, getter_name, setter, setter_name) in [
                (
                    "x",
                    get_mutable::<0> as NativeFn,
                    "get x",
                    set::<0> as NativeFn,
                    "set x",
                ),
                ("y", get_mutable::<1>, "get y", set::<1>, "set y"),
                (
                    "width",
                    get_mutable::<2>,
                    "get width",
                    set::<2>,
                    "set width",
                ),
                (
                    "height",
                    get_mutable::<3>,
                    "get height",
                    set::<3>,
                    "set height",
                ),
            ] {
                let get = Value::object(agent.new_native_function(getter, getter_name, 0)?);
                let set = Value::object(agent.new_native_function(setter, setter_name, 1)?);
                let key = agent.heap.key(name)?;
                agent.define_property_or_throw(
                    proto,
                    key,
                    PropertyDescriptor::accessor(get, set, true, true),
                )?;
            }
            Ok(())
        },
    )?;
    for (interface, method) in [
        (READONLY, from_rect::<false> as NativeFn),
        (MUTABLE, from_rect::<true>),
    ] {
        let proto = prototype(agent, interface)?;
        let key = agent.heap.key("constructor")?;
        let constructor = agent
            .get(proto, key)?
            .as_object()
            .expect("installed constructor");
        define_method(agent, constructor, "fromRect", method, 0)?;
    }
    Ok(())
}

pub(super) fn new_mutable(agent: &mut Agent, coords: [f64; 4]) -> JsResult<Value> {
    let proto = prototype(agent, MUTABLE)?;
    create(agent, proto, true, coords.map(Value::number))
}

fn construct<const WRITABLE: bool>(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("DOMRect construction requires new");
    };
    let mut coords = [Value::number(0.0); 4];
    for (value, &arg) in coords.iter_mut().zip(args) {
        *value = number(agent, arg)?;
    }
    let interface = if WRITABLE { MUTABLE } else { READONLY };
    let proto = agent.host_prototype_from_new_target(interface, target)?;
    create(agent, proto, WRITABLE, coords)
}

fn from_rect<const WRITABLE: bool>(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let source = args.first().copied().unwrap_or(Value::UNDEFINED);
    let mut coords = [Value::number(0.0); 4];
    if source != Value::UNDEFINED && source != Value::NULL {
        let Some(object) = source.as_object() else {
            return agent.type_error("DOMRectInit requires an object");
        };
        for index in [3, 2, 0, 1] {
            let key = agent.heap.key(MEMBERS[index])?;
            let value = agent.get(object, key)?;
            coords[index] = number(agent, value)?;
        }
    }
    let proto = prototype(agent, if WRITABLE { MUTABLE } else { READONLY })?;
    create(agent, proto, WRITABLE, coords)
}

fn number(agent: &mut Agent, arg: Value) -> JsResult<Value> {
    Ok(Value::number(if arg == Value::UNDEFINED {
        0.0
    } else {
        agent.to_number(arg)?
    }))
}

fn create(
    agent: &mut Agent,
    proto: ObjectId,
    writable: bool,
    coords: [Value; 4],
) -> JsResult<Value> {
    let [x, y, w, h] = coords;
    let rect = Value::object(agent.new_object_with(Some(proto))?);
    with_page(agent, |agent, page| {
        let record = PrivateArray::new(agent, &[x, y, w, h, Value::boolean(writable)])?;
        agent.weak_map_set(page.rects, rect, record.value())
    })?;
    Ok(rect)
}

fn state(agent: &mut Agent, rect: Value) -> JsResult<PrivateArray> {
    with_page(agent, |agent, page| {
        let value = agent.weak_map_get(page.rects, rect);
        match PrivateArray::from_value(value) {
            Some(record) => Ok(record),
            None => agent.type_error("expected DOMRectReadOnly"),
        }
    })
}

fn get<const MEMBER: u32>(agent: &mut Agent, rect: Value, _: &[Value]) -> JsResult<Value> {
    let record = state(agent, rect)?;
    if MEMBER < 4 {
        return record.get(agent, MEMBER);
    }
    let (origin, size) = if MEMBER % 2 == 0 { (1, 3) } else { (0, 2) };
    let origin = record
        .get(agent, origin)?
        .as_number()
        .expect("rectangle coordinate");
    let size = record
        .get(agent, size)?
        .as_number()
        .expect("rectangle dimension");
    let end = origin + size;
    let value = if origin.is_nan() || end.is_nan() {
        f64::NAN
    } else if matches!(MEMBER, 4 | 7) {
        if origin == end {
            f64::from_bits(origin.to_bits() | end.to_bits())
        } else {
            origin.min(end)
        }
    } else if origin == end {
        f64::from_bits(origin.to_bits() & end.to_bits())
    } else {
        origin.max(end)
    };
    Ok(Value::number(value))
}

fn writable_state(agent: &mut Agent, rect: Value) -> JsResult<PrivateArray> {
    let record = state(agent, rect)?;
    if record.get(agent, 4)? != Value::TRUE {
        return agent.type_error("expected DOMRect");
    }
    Ok(record)
}

fn get_mutable<const MEMBER: u32>(agent: &mut Agent, rect: Value, _: &[Value]) -> JsResult<Value> {
    writable_state(agent, rect)?.get(agent, MEMBER)
}

fn set<const MEMBER: u32>(agent: &mut Agent, rect: Value, args: &[Value]) -> JsResult<Value> {
    let record = writable_state(agent, rect)?;
    let value = agent.to_number(args.first().copied().unwrap_or(Value::UNDEFINED))?;
    record.set(agent, MEMBER, Value::number(value))?;
    Ok(Value::UNDEFINED)
}

fn to_json(agent: &mut Agent, rect: Value, _: &[Value]) -> JsResult<Value> {
    let getters: [NativeFn; 8] = [
        get::<0>, get::<1>, get::<2>, get::<3>, get::<4>, get::<5>, get::<6>, get::<7>,
    ];
    let object = agent.new_object()?;
    for (name, getter) in MEMBERS.into_iter().zip(getters) {
        let key = agent.heap.key(name)?;
        let value = getter(agent, rect, &[])?;
        agent.create_data_property_or_throw(object, key, value)?;
    }
    Ok(Value::object(object))
}
