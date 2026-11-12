//! Proxy (§10.5, §28.2): the constructor, `Proxy.revocable`, and the
//! internal methods of Proxy exotic objects with their invariant checks.
//!
//! A proxy is an object flagged exotic whose internal slot holds the
//! target and the handler (both `None` once revoked). A proxy for a
//! callable target is a function object whose native behavior is
//! [`proxy_call`], so calls and `new` reach it like any built-in.

use super::function::create_list_from_array_like;
use super::object::{from_property_descriptor, to_property_descriptor};
use super::{arg, install_methods, method};
use crate::runtime::agent::{Agent, JsResult};
use crate::runtime::object::{
    Class, FLAG_EXOTIC, FLAG_NATIVE_CONSTRUCTOR, Internal, Object, PropertyDescriptor,
};
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::{Kind, ObjectId, Value};

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    // `Proxy` has no `prototype` property (§28.2.2).
    let constructor = agent.new_native_function(proxy_constructor, "Proxy", 2)?;
    agent
        .heap
        .object_mut(constructor)
        .set_flag(FLAG_NATIVE_CONSTRUCTOR, true);
    install_methods(
        agent,
        constructor,
        &[method("revocable", proxy_revocable, 2)],
    )?;
    let global = agent.intrinsics.global;
    let key = agent.heap.key("Proxy")?;
    agent.define_property_or_throw(
        global,
        key,
        PropertyDescriptor::data_with(Value::object(constructor), true, false, true),
    )
}

/// `ProxyCreate(target, handler)` (§10.5.14).
fn proxy_create(agent: &mut Agent, target: Value, handler: Value) -> JsResult<ObjectId> {
    let (Some(target), Some(handler)) = (target.as_object(), handler.as_object()) else {
        return agent.type_error("Cannot create proxy with a non-object as target or handler");
    };
    let callable = agent.is_callable(Value::object(target));
    let constructor = agent.is_constructor_value(Value::object(target));
    let class = if callable {
        Class::Function
    } else {
        Class::Ordinary
    };
    let mut object = Object::new(class, None);
    object.set_flag(FLAG_EXOTIC, true);
    if callable {
        object.extra_mut().native = Some(proxy_call);
        object.set_flag(FLAG_NATIVE_CONSTRUCTOR, constructor);
    }
    object.extra_mut().internal = Some(Internal::Proxy {
        target: Some(target),
        handler: Some(handler),
    });
    agent
        .heap
        .charge(std::mem::size_of::<crate::runtime::Extra>())?;
    Ok(agent.heap.alloc_object(object)?)
}

/// `new Proxy(target, handler)` (§28.2.1.1).
fn proxy_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if agent.new_target().is_none() {
        return agent.type_error("Constructor Proxy requires 'new'");
    }
    proxy_create(agent, arg(args, 0), arg(args, 1)).map(Value::object)
}

/// `Proxy.revocable(target, handler)` (§28.2.2.1).
fn proxy_revocable(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let proxy = proxy_create(agent, arg(args, 0), arg(args, 1))?;
    let revoke = agent.new_native_function(proxy_revoke, "", 0)?;
    agent
        .heap
        .charge(std::mem::size_of::<crate::runtime::Extra>() + 8)?;
    agent.heap.object_mut(revoke).extra_mut().internal =
        Some(Internal::Captures(Box::new([Value::object(proxy)])));
    let result = agent.new_object()?;
    let proxy_key = agent.heap.key("proxy")?;
    let revoke_key = agent.heap.key("revoke")?;
    agent.create_data_property_or_throw(result, proxy_key, Value::object(proxy))?;
    agent.create_data_property_or_throw(result, revoke_key, Value::object(revoke))?;
    Ok(Value::object(result))
}

/// The revoker function: clears the proxy's slots once.
fn proxy_revoke(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let f = agent.active_function().expect("a revoker is running");
    let proxy = match agent.heap.object_mut(f).internal_mut() {
        Some(Internal::Captures(values)) => std::mem::replace(&mut values[0], Value::NULL),
        _ => Value::NULL,
    };
    if let Some(proxy) = proxy.as_object() {
        if let Some(Internal::Proxy { target, handler }) =
            agent.heap.object_mut(proxy).internal_mut()
        {
            *target = None;
            *handler = None;
        }
    }
    Ok(Value::UNDEFINED)
}

/// The target and handler of `proxy`, or a TypeError once revoked.
fn parts(agent: &mut Agent, proxy: ObjectId) -> JsResult<(ObjectId, ObjectId)> {
    match agent.heap.object(proxy).internal() {
        Some(Internal::Proxy {
            target: Some(target),
            handler: Some(handler),
        }) => Ok((*target, *handler)),
        _ => agent.type_error("Cannot perform an operation on a revoked proxy"),
    }
}

/// The proxy's target while it is not revoked (`IsArray` looks through).
pub(crate) fn target_of(agent: &mut Agent, proxy: ObjectId) -> JsResult<ObjectId> {
    parts(agent, proxy).map(|(target, _)| target)
}

/// The target, the handler and the handler's trap `name`, if it has one.
fn trap(
    agent: &mut Agent,
    proxy: ObjectId,
    name: &str,
) -> JsResult<(ObjectId, ObjectId, Option<Value>)> {
    // Traps run script, which may recurse through further proxies.
    agent.check_native_stack()?;
    let (target, handler) = parts(agent, proxy)?;
    let key = agent.heap.key(name)?;
    let trap = agent.get_method(Value::object(handler), key)?;
    Ok((target, handler, trap))
}

fn object_or_null(value: Option<ObjectId>) -> Value {
    value.map_or(Value::NULL, Value::object)
}

/// `[[GetPrototypeOf]]` (§10.5.1).
pub(crate) fn get_prototype_of(agent: &mut Agent, proxy: ObjectId) -> JsResult<Option<ObjectId>> {
    let (target, handler, trap) = trap(agent, proxy, "getPrototypeOf")?;
    let Some(trap) = trap else {
        return agent.get_prototype_of(target);
    };
    let result = agent.call(trap, Value::object(handler), &[Value::object(target)])?;
    let prototype = match result.kind() {
        Kind::Object(o) => Some(o),
        Kind::Null => None,
        _ => {
            return agent
                .type_error("'getPrototypeOf' on proxy: trap returned neither object nor null");
        }
    };
    if agent.is_extensible(target)? {
        return Ok(prototype);
    }
    if agent.get_prototype_of(target)? != prototype {
        return agent.type_error(
            "'getPrototypeOf' on proxy: proxy target is non-extensible but the trap did not return its actual prototype",
        );
    }
    Ok(prototype)
}

/// `[[SetPrototypeOf]]` (§10.5.2).
pub(crate) fn set_prototype_of(
    agent: &mut Agent,
    proxy: ObjectId,
    prototype: Option<ObjectId>,
) -> JsResult<bool> {
    let (target, handler, trap) = trap(agent, proxy, "setPrototypeOf")?;
    let Some(trap) = trap else {
        return agent.set_prototype_of(target, prototype);
    };
    let result = agent.call(
        trap,
        Value::object(handler),
        &[Value::object(target), object_or_null(prototype)],
    )?;
    if !agent.to_boolean(result) {
        return Ok(false);
    }
    if agent.is_extensible(target)? {
        return Ok(true);
    }
    if agent.get_prototype_of(target)? != prototype {
        return agent.type_error(
            "'setPrototypeOf' on proxy: trap returned truish for setting a new prototype on the non-extensible proxy target",
        );
    }
    Ok(true)
}

/// `[[IsExtensible]]` (§10.5.3).
pub(crate) fn is_extensible(agent: &mut Agent, proxy: ObjectId) -> JsResult<bool> {
    let (target, handler, trap) = trap(agent, proxy, "isExtensible")?;
    let Some(trap) = trap else {
        return agent.is_extensible(target);
    };
    let result = agent.call(trap, Value::object(handler), &[Value::object(target)])?;
    let result = agent.to_boolean(result);
    if result != agent.is_extensible(target)? {
        return agent.type_error(
            "'isExtensible' on proxy: trap result does not reflect extensibility of proxy target",
        );
    }
    Ok(result)
}

/// `[[PreventExtensions]]` (§10.5.4).
pub(crate) fn prevent_extensions(agent: &mut Agent, proxy: ObjectId) -> JsResult<bool> {
    let (target, handler, trap) = trap(agent, proxy, "preventExtensions")?;
    let Some(trap) = trap else {
        return agent.prevent_extensions(target);
    };
    let result = agent.call(trap, Value::object(handler), &[Value::object(target)])?;
    let result = agent.to_boolean(result);
    if result && agent.is_extensible(target)? {
        return agent.type_error(
            "'preventExtensions' on proxy: trap returned truish but the proxy target is extensible",
        );
    }
    Ok(result)
}

/// `CompletePropertyDescriptor` (§6.2.6.6).
fn complete(desc: PropertyDescriptor) -> PropertyDescriptor {
    if desc.is_accessor() {
        PropertyDescriptor::accessor(
            desc.get.unwrap_or(Value::UNDEFINED),
            desc.set.unwrap_or(Value::UNDEFINED),
            desc.enumerable.unwrap_or(false),
            desc.configurable.unwrap_or(false),
        )
    } else {
        PropertyDescriptor::data_with(
            desc.value.unwrap_or(Value::UNDEFINED),
            desc.writable.unwrap_or(false),
            desc.enumerable.unwrap_or(false),
            desc.configurable.unwrap_or(false),
        )
    }
}

/// `[[GetOwnProperty]]` (§10.5.5).
pub(crate) fn get_own_property(
    agent: &mut Agent,
    proxy: ObjectId,
    key: PropertyKey,
) -> JsResult<Option<PropertyDescriptor>> {
    let (target, handler, trap) = trap(agent, proxy, "getOwnPropertyDescriptor")?;
    let Some(trap) = trap else {
        return agent.get_own_property(target, key);
    };
    let key_value = agent.key_to_value(key)?;
    let result = agent.call(
        trap,
        Value::object(handler),
        &[Value::object(target), key_value],
    )?;
    if result.as_object().is_none() && result != Value::UNDEFINED {
        return agent.type_error(
            "'getOwnPropertyDescriptor' on proxy: trap returned neither object nor undefined",
        );
    }
    let target_desc = agent.get_own_property(target, key)?;
    if result == Value::UNDEFINED {
        let Some(target_desc) = target_desc else {
            return Ok(None);
        };
        if target_desc.configurable == Some(false) {
            return agent.type_error(
                "'getOwnPropertyDescriptor' on proxy: trap returned undefined for a property which is non-configurable in the proxy target",
            );
        }
        if !agent.is_extensible(target)? {
            return agent.type_error(
                "'getOwnPropertyDescriptor' on proxy: trap returned undefined for a property which exists in the non-extensible proxy target",
            );
        }
        return Ok(None);
    }
    let extensible = agent.is_extensible(target)?;
    let result_desc = complete(to_property_descriptor(agent, result)?);
    if !agent.validate_only(extensible, result_desc, target_desc)? {
        return agent.type_error(
            "'getOwnPropertyDescriptor' on proxy: trap returned a descriptor that is incompatible with the existing property in the proxy target",
        );
    }
    if result_desc.configurable == Some(false) {
        let Some(target_desc) = target_desc.filter(|d| d.configurable == Some(false)) else {
            return agent.type_error(
                "'getOwnPropertyDescriptor' on proxy: trap reported non-configurability for a property which is either non-existent or configurable in the proxy target",
            );
        };
        if result_desc.writable == Some(false) && target_desc.writable == Some(true) {
            return agent.type_error(
                "'getOwnPropertyDescriptor' on proxy: trap reported a non-configurable and non-writable property which is writable in the proxy target",
            );
        }
    }
    Ok(Some(result_desc))
}

/// `[[DefineOwnProperty]]` (§10.5.6).
pub(crate) fn define_own_property(
    agent: &mut Agent,
    proxy: ObjectId,
    key: PropertyKey,
    desc: PropertyDescriptor,
) -> JsResult<bool> {
    let (target, handler, trap) = trap(agent, proxy, "defineProperty")?;
    let Some(trap) = trap else {
        return agent.define_own_property(target, key, desc);
    };
    let desc_object = from_property_descriptor(agent, Some(desc))?;
    let key_value = agent.key_to_value(key)?;
    let result = agent.call(
        trap,
        Value::object(handler),
        &[Value::object(target), key_value, desc_object],
    )?;
    if !agent.to_boolean(result) {
        return Ok(false);
    }
    let target_desc = agent.get_own_property(target, key)?;
    let extensible = agent.is_extensible(target)?;
    let setting_non_configurable = desc.configurable == Some(false);
    match target_desc {
        None => {
            if !extensible {
                return agent.type_error(
                    "'defineProperty' on proxy: trap returned truish for adding a property to the non-extensible proxy target",
                );
            }
            if setting_non_configurable {
                return agent.type_error(
                    "'defineProperty' on proxy: trap returned truish for defining a non-configurable property which does not exist in the proxy target",
                );
            }
        }
        Some(target_desc) => {
            if !agent.validate_only(extensible, desc, Some(target_desc))? {
                return agent.type_error(
                    "'defineProperty' on proxy: trap returned truish for adding a property that is incompatible with the existing property in the proxy target",
                );
            }
            if setting_non_configurable && target_desc.configurable == Some(true) {
                return agent.type_error(
                    "'defineProperty' on proxy: trap returned truish for defining a non-configurable property which is configurable in the proxy target",
                );
            }
            if target_desc.is_data()
                && target_desc.configurable == Some(false)
                && target_desc.writable == Some(true)
                && desc.writable == Some(false)
            {
                return agent.type_error(
                    "'defineProperty' on proxy: trap returned truish for defining a non-configurable, non-writable property which is writable in the proxy target",
                );
            }
        }
    }
    Ok(true)
}

/// `[[HasProperty]]` (§10.5.7).
pub(crate) fn has_property(agent: &mut Agent, proxy: ObjectId, key: PropertyKey) -> JsResult<bool> {
    let (target, handler, trap) = trap(agent, proxy, "has")?;
    let Some(trap) = trap else {
        return agent.has_property(target, key);
    };
    let key_value = agent.key_to_value(key)?;
    let result = agent.call(
        trap,
        Value::object(handler),
        &[Value::object(target), key_value],
    )?;
    let result = agent.to_boolean(result);
    if !result {
        if let Some(target_desc) = agent.get_own_property(target, key)? {
            if target_desc.configurable == Some(false) {
                return agent.type_error(
                    "'has' on proxy: trap returned falsish for a property which exists in the proxy target as non-configurable",
                );
            }
            if !agent.is_extensible(target)? {
                return agent.type_error(
                    "'has' on proxy: trap returned falsish for a property which exists in the non-extensible proxy target",
                );
            }
        }
    }
    Ok(result)
}

/// `[[Get]]` (§10.5.8).
pub(crate) fn get(
    agent: &mut Agent,
    proxy: ObjectId,
    key: PropertyKey,
    receiver: Value,
) -> JsResult<Value> {
    let (target, handler, trap) = trap(agent, proxy, "get")?;
    let Some(trap) = trap else {
        return agent.get_with_receiver(target, key, receiver);
    };
    let key_value = agent.key_to_value(key)?;
    let result = agent.call(
        trap,
        Value::object(handler),
        &[Value::object(target), key_value, receiver],
    )?;
    if let Some(target_desc) = agent.get_own_property(target, key)? {
        if target_desc.configurable == Some(false) {
            if target_desc.is_data()
                && target_desc.writable == Some(false)
                && !agent.same_value(result, target_desc.value.unwrap_or(Value::UNDEFINED))
            {
                return agent.type_error(
                    "'get' on proxy: property is a read-only and non-configurable data property on the proxy target but the proxy did not return its actual value",
                );
            }
            if target_desc.is_accessor()
                && target_desc.get.unwrap_or(Value::UNDEFINED) == Value::UNDEFINED
                && result != Value::UNDEFINED
            {
                return agent.type_error(
                    "'get' on proxy: property is a non-configurable accessor property on the proxy target and does not have a getter function, but the trap did not return undefined",
                );
            }
        }
    }
    Ok(result)
}

/// `[[Set]]` (§10.5.9).
pub(crate) fn set(
    agent: &mut Agent,
    proxy: ObjectId,
    key: PropertyKey,
    value: Value,
    receiver: Value,
) -> JsResult<bool> {
    let (target, handler, trap) = trap(agent, proxy, "set")?;
    let Some(trap) = trap else {
        return agent.set_with_receiver(target, key, value, receiver);
    };
    let key_value = agent.key_to_value(key)?;
    let result = agent.call(
        trap,
        Value::object(handler),
        &[Value::object(target), key_value, value, receiver],
    )?;
    if !agent.to_boolean(result) {
        return Ok(false);
    }
    if let Some(target_desc) = agent.get_own_property(target, key)? {
        if target_desc.configurable == Some(false) {
            if target_desc.is_data()
                && target_desc.writable == Some(false)
                && !agent.same_value(value, target_desc.value.unwrap_or(Value::UNDEFINED))
            {
                return agent.type_error(
                    "'set' on proxy: trap returned truish for property which exists in the proxy target as a non-configurable and non-writable data property with a different value",
                );
            }
            if target_desc.is_accessor()
                && target_desc.set.unwrap_or(Value::UNDEFINED) == Value::UNDEFINED
            {
                return agent.type_error(
                    "'set' on proxy: trap returned truish for property which exists in the proxy target as a non-configurable accessor property without a setter",
                );
            }
        }
    }
    Ok(true)
}

/// `[[Delete]]` (§10.5.10).
pub(crate) fn delete(agent: &mut Agent, proxy: ObjectId, key: PropertyKey) -> JsResult<bool> {
    let (target, handler, trap) = trap(agent, proxy, "deleteProperty")?;
    let Some(trap) = trap else {
        return agent.delete(target, key);
    };
    let key_value = agent.key_to_value(key)?;
    let result = agent.call(
        trap,
        Value::object(handler),
        &[Value::object(target), key_value],
    )?;
    if !agent.to_boolean(result) {
        return Ok(false);
    }
    let Some(target_desc) = agent.get_own_property(target, key)? else {
        return Ok(true);
    };
    if target_desc.configurable == Some(false) {
        return agent.type_error(
            "'deleteProperty' on proxy: trap returned truish for property which is non-configurable in the proxy target",
        );
    }
    if !agent.is_extensible(target)? {
        return agent.type_error(
            "'deleteProperty' on proxy: trap returned truish for property but the proxy target is non-extensible",
        );
    }
    Ok(true)
}

/// `[[OwnPropertyKeys]]` (§10.5.11).
pub(crate) fn own_property_keys(agent: &mut Agent, proxy: ObjectId) -> JsResult<Vec<PropertyKey>> {
    let (target, handler, trap) = trap(agent, proxy, "ownKeys")?;
    let Some(trap) = trap else {
        return agent.own_property_keys(target);
    };
    let result = agent.call(trap, Value::object(handler), &[Value::object(target)])?;
    // `CreateListFromArrayLike(result, property-key)`.
    let values = create_list_from_array_like(agent, result)?;
    let mut keys: Vec<PropertyKey> = Vec::with_capacity(values.len());
    for value in values {
        if !matches!(value.kind(), Kind::String(_) | Kind::Symbol(_)) {
            return agent
                .type_error("'ownKeys' on proxy: trap result elements must be strings or symbols");
        }
        let key = agent.to_property_key(value)?;
        if keys.contains(&key) {
            return agent.type_error("'ownKeys' on proxy: trap returned duplicate entries");
        }
        keys.push(key);
    }
    let extensible = agent.is_extensible(target)?;
    let target_keys = agent.own_property_keys(target)?;
    let mut configurable = Vec::new();
    let mut non_configurable = Vec::new();
    for key in target_keys {
        match agent.get_own_property(target, key)? {
            Some(desc) if desc.configurable == Some(false) => non_configurable.push(key),
            _ => configurable.push(key),
        }
    }
    if extensible && non_configurable.is_empty() {
        return Ok(keys);
    }
    let mut unchecked = keys.clone();
    for key in non_configurable {
        match unchecked.iter().position(|k| *k == key) {
            Some(at) => {
                unchecked.swap_remove(at);
            }
            None => {
                return agent.type_error(
                    "'ownKeys' on proxy: trap result did not include a non-configurable key of the proxy target",
                );
            }
        }
    }
    if extensible {
        return Ok(keys);
    }
    for key in configurable {
        match unchecked.iter().position(|k| *k == key) {
            Some(at) => {
                unchecked.swap_remove(at);
            }
            None => {
                return agent.type_error(
                    "'ownKeys' on proxy: trap result did not include a key of the non-extensible proxy target",
                );
            }
        }
    }
    if !unchecked.is_empty() {
        return agent.type_error(
            "'ownKeys' on proxy: trap returned extra keys but the proxy target is non-extensible",
        );
    }
    Ok(keys)
}

/// `[[Call]]` (§10.5.12) and `[[Construct]]` (§10.5.13) of a proxy for a
/// callable target.
fn proxy_call(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let proxy = agent.active_function().expect("a proxy is running");
    let new_target = agent.new_target();
    agent.in_proxy(|agent| proxy_call_or_construct(agent, proxy, new_target, this, args))
}

fn proxy_call_or_construct(
    agent: &mut Agent,
    proxy: ObjectId,
    new_target: Option<ObjectId>,
    this: Value,
    args: &[Value],
) -> JsResult<Value> {
    match new_target {
        None => {
            let (target, handler, trap) = trap(agent, proxy, "apply")?;
            let Some(trap) = trap else {
                return agent.call(Value::object(target), this, args);
            };
            let list = agent.create_array_from_list(args)?;
            agent.call(
                trap,
                Value::object(handler),
                &[Value::object(target), this, Value::object(list)],
            )
        }
        Some(new_target) => {
            let (target, handler, trap) = trap(agent, proxy, "construct")?;
            let Some(trap) = trap else {
                return agent.construct_with(Value::object(target), args, new_target);
            };
            let list = agent.create_array_from_list(args)?;
            let result = agent.call(
                trap,
                Value::object(handler),
                &[
                    Value::object(target),
                    Value::object(list),
                    Value::object(new_target),
                ],
            )?;
            if result.as_object().is_none() {
                return agent.type_error("proxy [[Construct]] must return an object");
            }
            Ok(result)
        }
    }
}
