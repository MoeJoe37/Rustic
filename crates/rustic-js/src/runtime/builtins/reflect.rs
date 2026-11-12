//! The `Reflect` object (§28.1): the object internal methods as
//! functions.

use super::function::create_list_from_array_like;
use super::object::{from_property_descriptor, to_property_descriptor};
use super::{Method, arg, install_methods, method};
use crate::runtime::agent::{Agent, JsResult};
use crate::runtime::object::PropertyDescriptor;
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::{ObjectId, Value};

const FUNCTIONS: &[Method] = &[
    method("apply", apply, 3),
    method("construct", construct, 2),
    method("defineProperty", define_property, 3),
    method("deleteProperty", delete_property, 2),
    method("get", get, 2),
    method("getOwnPropertyDescriptor", get_own_property_descriptor, 2),
    method("getPrototypeOf", get_prototype_of, 1),
    method("has", has, 2),
    method("isExtensible", is_extensible, 1),
    method("ownKeys", own_keys, 1),
    method("preventExtensions", prevent_extensions, 1),
    method("set", set, 3),
    method("setPrototypeOf", set_prototype_of, 2),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let object_prototype = agent.intrinsics.object_prototype;
    let reflect = agent.new_object_with(Some(object_prototype))?;
    install_methods(agent, reflect, FUNCTIONS)?;
    let tag = agent.string("Reflect")?;
    agent.define_property_or_throw(
        reflect,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    let global = agent.intrinsics.global;
    let key = agent.heap.key("Reflect")?;
    agent.define_property_or_throw(
        global,
        key,
        PropertyDescriptor::data_with(Value::object(reflect), true, false, true),
    )
}

fn target(agent: &mut Agent, args: &[Value], name: &str) -> JsResult<ObjectId> {
    match arg(args, 0).as_object() {
        Some(o) => Ok(o),
        None => agent.type_error(&format!("Reflect.{name} called on a non-object")),
    }
}

fn key(agent: &mut Agent, args: &[Value]) -> JsResult<PropertyKey> {
    agent.to_property_key(arg(args, 1))
}

fn apply(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    if !agent.is_callable(f) {
        return agent.type_error("Reflect.apply target is not a function");
    }
    let list = create_list_from_array_like(agent, arg(args, 2))?;
    agent.call(f, arg(args, 1), &list)
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    if !agent.is_constructor_value(f) {
        return agent.type_error("Reflect.construct target is not a constructor");
    }
    let new_target = match args.get(2) {
        Some(&nt) => nt,
        None => f,
    };
    if !agent.is_constructor_value(new_target) {
        return agent.type_error("Reflect.construct newTarget is not a constructor");
    }
    let list = create_list_from_array_like(agent, arg(args, 1))?;
    let new_target = new_target.as_object().expect("constructors are objects");
    agent.construct_with(f, &list, new_target)
}

fn define_property(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = target(agent, args, "defineProperty")?;
    let key = key(agent, args)?;
    let desc = to_property_descriptor(agent, arg(args, 2))?;
    Ok(Value::boolean(agent.define_own_property(o, key, desc)?))
}

fn delete_property(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = target(agent, args, "deleteProperty")?;
    let key = key(agent, args)?;
    Ok(Value::boolean(agent.delete(o, key)?))
}

fn get(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = target(agent, args, "get")?;
    let key = key(agent, args)?;
    let receiver = match args.get(2) {
        Some(&r) => r,
        None => Value::object(o),
    };
    agent.get_with_receiver(o, key, receiver)
}

fn get_own_property_descriptor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = target(agent, args, "getOwnPropertyDescriptor")?;
    let key = key(agent, args)?;
    let desc = agent.get_own_property(o, key)?;
    from_property_descriptor(agent, desc)
}

fn get_prototype_of(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = target(agent, args, "getPrototypeOf")?;
    Ok(agent.get_prototype_of(o).map_or(Value::NULL, Value::object))
}

fn has(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = target(agent, args, "has")?;
    let key = key(agent, args)?;
    Ok(Value::boolean(agent.has_property(o, key)?))
}

fn is_extensible(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = target(agent, args, "isExtensible")?;
    Ok(Value::boolean(agent.is_extensible(o)))
}

fn own_keys(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = target(agent, args, "ownKeys")?;
    let keys = agent.own_property_keys(o);
    let mut values = Vec::with_capacity(keys.len());
    for key in keys {
        values.push(match key {
            PropertyKey::String(s) => Value::string(s),
            PropertyKey::Symbol(s) => Value::symbol(s),
            PropertyKey::Index(i) => agent.string(&i.to_string())?,
        });
    }
    Ok(Value::object(agent.create_array_from_list(&values)?))
}

fn prevent_extensions(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = target(agent, args, "preventExtensions")?;
    Ok(Value::boolean(agent.prevent_extensions(o)))
}

fn set(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = target(agent, args, "set")?;
    let key = key(agent, args)?;
    let receiver = match args.get(3) {
        Some(&r) => r,
        None => Value::object(o),
    };
    Ok(Value::boolean(agent.set_with_receiver(
        o,
        key,
        arg(args, 2),
        receiver,
    )?))
}

fn set_prototype_of(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = target(agent, args, "setPrototypeOf")?;
    let prototype = match arg(args, 1) {
        p if p == Value::NULL => None,
        p => match p.as_object() {
            Some(p) => Some(p),
            None => {
                return agent
                    .type_error("Reflect.setPrototypeOf: prototype must be an object or null");
            }
        },
    };
    Ok(Value::boolean(agent.set_prototype_of(o, prototype)))
}
