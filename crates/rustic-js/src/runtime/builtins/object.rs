//! `Object` (§20.1): the constructor, its static methods and the
//! methods of `Object.prototype` that `builtins.rs` does not already
//! define (`valueOf`, `toString`).

use super::{Method, arg, install_constructor, install_methods, method, prototype_from_new_target};
use crate::runtime::agent::{Agent, JsResult};
use crate::runtime::internal::IntegrityLevel;
use crate::runtime::object::PropertyDescriptor;
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::{ObjectId, Value};

const STATIC_METHODS: &[Method] = &[
    method("assign", assign, 2),
    method("create", create, 2),
    method("defineProperties", define_properties, 2),
    method("defineProperty", define_property, 3),
    method("entries", entries, 1),
    method("freeze", freeze, 1),
    method("fromEntries", from_entries, 1),
    method("getOwnPropertyDescriptor", get_own_property_descriptor, 2),
    method("getOwnPropertyDescriptors", get_own_property_descriptors, 1),
    method("getOwnPropertyNames", get_own_property_names, 1),
    method("getOwnPropertySymbols", get_own_property_symbols, 1),
    method("getPrototypeOf", get_prototype_of, 1),
    method("hasOwn", has_own, 2),
    method("is", is, 2),
    method("isExtensible", is_extensible, 1),
    method("isFrozen", is_frozen, 1),
    method("isSealed", is_sealed, 1),
    method("keys", keys, 1),
    method("preventExtensions", prevent_extensions, 1),
    method("seal", seal, 1),
    method("setPrototypeOf", set_prototype_of, 2),
    method("values", values, 1),
];

const PROTOTYPE_METHODS: &[Method] = &[
    method("hasOwnProperty", has_own_property, 1),
    method("isPrototypeOf", is_prototype_of, 1),
    method("propertyIsEnumerable", property_is_enumerable, 1),
    method("toLocaleString", to_locale_string, 0),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let prototype = agent.intrinsics.object_prototype;
    let constructor = install_constructor(agent, "Object", object_constructor, 1, prototype)?;
    install_methods(agent, constructor, STATIC_METHODS)?;
    install_methods(agent, prototype, PROTOTYPE_METHODS)?;
    // `Object.prototype.__proto__` (§20.1.3.8, Annex B).
    let getter = agent.new_native_function(proto_getter, "get __proto__", 0)?;
    let setter = agent.new_native_function(proto_setter, "set __proto__", 1)?;
    let key = agent.heap.key("__proto__")?;
    agent.define_property_or_throw(
        prototype,
        key,
        PropertyDescriptor::accessor(Value::object(getter), Value::object(setter), false, true),
    )
}

/// `Object(value)` (§20.1.1.1).
fn object_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let object_prototype = agent.intrinsics.object_prototype;
    if let Some(target) = agent.new_target() {
        if agent.active_function() != Some(target) {
            let prototype = prototype_from_new_target(agent, object_prototype)?;
            return Ok(Value::object(agent.new_object_with(Some(prototype))?));
        }
    }
    let value = arg(args, 0);
    if value == Value::UNDEFINED || value == Value::NULL {
        return Ok(Value::object(agent.new_object()?));
    }
    Ok(Value::object(agent.to_object(value)?))
}

fn require_object(agent: &mut Agent, value: Value, what: &str) -> JsResult<ObjectId> {
    match value.as_object() {
        Some(o) => Ok(o),
        None => agent.type_error(&format!("{what} called on a non-object")),
    }
}

/// `FromPropertyDescriptor(desc)` (§6.2.6.4).
pub(crate) fn from_property_descriptor(
    agent: &mut Agent,
    desc: Option<PropertyDescriptor>,
) -> JsResult<Value> {
    let Some(desc) = desc else {
        return Ok(Value::UNDEFINED);
    };
    let o = agent.new_object()?;
    let fields: [(&str, Option<Value>); 6] = [
        ("value", desc.value),
        ("writable", desc.writable.map(Value::boolean)),
        ("get", desc.get),
        ("set", desc.set),
        ("enumerable", desc.enumerable.map(Value::boolean)),
        ("configurable", desc.configurable.map(Value::boolean)),
    ];
    for (name, value) in fields {
        if let Some(value) = value {
            let key = agent.heap.key(name)?;
            agent.create_data_property_or_throw(o, key, value)?;
        }
    }
    Ok(Value::object(o))
}

/// `ToPropertyDescriptor(Obj)` (§6.2.6.5).
pub(crate) fn to_property_descriptor(
    agent: &mut Agent,
    value: Value,
) -> JsResult<PropertyDescriptor> {
    let Some(o) = value.as_object() else {
        return agent.type_error("property descriptor is not an object");
    };
    let mut desc = PropertyDescriptor::default();
    let field = |agent: &mut Agent, name: &str| -> JsResult<Option<Value>> {
        let key = agent.heap.key(name)?;
        if agent.has_property(o, key)? {
            Ok(Some(agent.get(o, key)?))
        } else {
            Ok(None)
        }
    };
    if let Some(v) = field(agent, "enumerable")? {
        desc.enumerable = Some(agent.to_boolean(v));
    }
    if let Some(v) = field(agent, "configurable")? {
        desc.configurable = Some(agent.to_boolean(v));
    }
    if let Some(v) = field(agent, "value")? {
        desc.value = Some(v);
    }
    if let Some(v) = field(agent, "writable")? {
        desc.writable = Some(agent.to_boolean(v));
    }
    for (name, slot) in [("get", &mut desc.get), ("set", &mut desc.set)] {
        if let Some(v) = field(agent, name)? {
            if v != Value::UNDEFINED && !agent.is_callable(v) {
                return agent.type_error(&format!("property descriptor {name} is not callable"));
            }
            *slot = Some(v);
        }
    }
    if (desc.get.is_some() || desc.set.is_some())
        && (desc.value.is_some() || desc.writable.is_some())
    {
        return agent.type_error("a property descriptor cannot be both data and accessor");
    }
    Ok(desc)
}

/// `Object.assign(target, ...sources)` (§20.1.2.1).
fn assign(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let to = agent.to_object(arg(args, 0))?;
    for &source in args.iter().skip(1) {
        if source == Value::UNDEFINED || source == Value::NULL {
            continue;
        }
        let from = agent.to_object(source)?;
        for key in agent.own_property_keys(from) {
            let Some(desc) = agent.get_own_property(from, key)? else {
                continue;
            };
            if desc.enumerable == Some(true) {
                let value = agent.get(from, key)?;
                agent.set(to, key, value, true)?;
            }
        }
    }
    Ok(Value::object(to))
}

/// `Object.create(O, Properties)` (§20.1.2.2).
fn create(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let proto = arg(args, 0);
    let proto = match proto.as_object() {
        Some(p) => Some(p),
        None if proto == Value::NULL => None,
        None => return agent.type_error("Object prototype may only be an object or null"),
    };
    let o = agent.new_object_with(proto)?;
    let properties = arg(args, 1);
    if properties != Value::UNDEFINED {
        object_define_properties(agent, o, properties)?;
    }
    Ok(Value::object(o))
}

/// `ObjectDefineProperties(O, Properties)` (§20.1.2.3.1).
fn object_define_properties(agent: &mut Agent, o: ObjectId, properties: Value) -> JsResult<()> {
    let props = agent.to_object(properties)?;
    let mut descriptors = Vec::new();
    for key in agent.own_property_keys(props) {
        let Some(desc) = agent.get_own_property(props, key)? else {
            continue;
        };
        if desc.enumerable == Some(true) {
            let value = agent.get(props, key)?;
            descriptors.push((key, to_property_descriptor(agent, value)?));
        }
    }
    for (key, desc) in descriptors {
        agent.define_property_or_throw(o, key, desc)?;
    }
    Ok(())
}

fn define_properties(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = require_object(agent, arg(args, 0), "Object.defineProperties")?;
    object_define_properties(agent, o, arg(args, 1))?;
    Ok(Value::object(o))
}

/// `Object.defineProperty(O, P, Attributes)` (§20.1.2.4).
fn define_property(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = require_object(agent, arg(args, 0), "Object.defineProperty")?;
    let key = agent.to_property_key(arg(args, 1))?;
    let desc = to_property_descriptor(agent, arg(args, 2))?;
    agent.define_property_or_throw(o, key, desc)?;
    Ok(Value::object(o))
}

/// What `EnumerableOwnProperties` collects.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Collect {
    Keys,
    Values,
    Entries,
}

/// `EnumerableOwnProperties(O, kind)` (§7.3.23).
fn enumerable_own(agent: &mut Agent, value: Value, kind: Collect) -> JsResult<Value> {
    let o = agent.to_object(value)?;
    let mut list = Vec::new();
    for key in agent.own_property_keys(o) {
        if matches!(key, PropertyKey::Symbol(_)) {
            continue;
        }
        let Some(desc) = agent.get_own_property(o, key)? else {
            continue;
        };
        if desc.enumerable != Some(true) {
            continue;
        }
        let name = agent.key_to_value(key)?;
        let item = match kind {
            Collect::Keys => name,
            Collect::Values => agent.get(o, key)?,
            Collect::Entries => {
                let v = agent.get(o, key)?;
                Value::object(agent.create_array_from_list(&[name, v])?)
            }
        };
        list.push(item);
    }
    Ok(Value::object(agent.create_array_from_list(&list)?))
}

fn keys(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    enumerable_own(agent, arg(args, 0), Collect::Keys)
}

fn values(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    enumerable_own(agent, arg(args, 0), Collect::Values)
}

fn entries(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    enumerable_own(agent, arg(args, 0), Collect::Entries)
}

fn integrity(
    agent: &mut Agent,
    args: &[Value],
    level: IntegrityLevel,
    name: &str,
) -> JsResult<Value> {
    let value = arg(args, 0);
    let Some(o) = value.as_object() else {
        return Ok(value);
    };
    if !agent.set_integrity_level(o, level)? {
        return agent.type_error(&format!("{name}: the object cannot be changed"));
    }
    Ok(value)
}

fn freeze(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    integrity(agent, args, IntegrityLevel::Frozen, "Object.freeze")
}

fn seal(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    integrity(agent, args, IntegrityLevel::Sealed, "Object.seal")
}

fn is_frozen(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    match arg(args, 0).as_object() {
        Some(o) => Ok(Value::boolean(
            agent.test_integrity_level(o, IntegrityLevel::Frozen)?,
        )),
        None => Ok(Value::TRUE),
    }
}

fn is_sealed(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    match arg(args, 0).as_object() {
        Some(o) => Ok(Value::boolean(
            agent.test_integrity_level(o, IntegrityLevel::Sealed)?,
        )),
        None => Ok(Value::TRUE),
    }
}

fn is_extensible(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(
        arg(args, 0)
            .as_object()
            .is_some_and(|o| agent.is_extensible(o)),
    ))
}

fn prevent_extensions(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let value = arg(args, 0);
    if let Some(o) = value.as_object() {
        if !agent.prevent_extensions(o) {
            return agent.type_error("Object.preventExtensions: the object cannot be changed");
        }
    }
    Ok(value)
}

/// `Object.fromEntries(iterable)` (§20.1.2.7), over arrays and other
/// array-likes until iterators of every kind exist (JS-05).
fn from_entries(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let iterable = agent.require_object_coercible(arg(args, 0))?;
    let o = agent.new_object()?;
    let items = super::function::create_list_from_array_like(agent, iterable)?;
    for entry in items {
        let Some(entry) = entry.as_object() else {
            return agent.type_error("Object.fromEntries: an entry is not an object");
        };
        let k = agent.get(entry, PropertyKey::Index(0))?;
        let v = agent.get(entry, PropertyKey::Index(1))?;
        let key = agent.to_property_key(k)?;
        agent.create_data_property_or_throw(o, key, v)?;
    }
    Ok(Value::object(o))
}

fn get_own_property_descriptor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = agent.to_object(arg(args, 0))?;
    let key = agent.to_property_key(arg(args, 1))?;
    let desc = agent.get_own_property(o, key)?;
    from_property_descriptor(agent, desc)
}

fn get_own_property_descriptors(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = agent.to_object(arg(args, 0))?;
    let result = agent.new_object()?;
    for key in agent.own_property_keys(o) {
        let desc = agent.get_own_property(o, key)?;
        let value = from_property_descriptor(agent, desc)?;
        if value != Value::UNDEFINED {
            agent.create_data_property_or_throw(result, key, value)?;
        }
    }
    Ok(Value::object(result))
}

/// `GetOwnPropertyKeys(O, type)` (§20.1.2.11.1).
fn own_keys(agent: &mut Agent, value: Value, symbols: bool) -> JsResult<Value> {
    let o = agent.to_object(value)?;
    let mut list = Vec::new();
    for key in agent.own_property_keys(o) {
        if matches!(key, PropertyKey::Symbol(_)) == symbols {
            list.push(agent.key_to_value(key)?);
        }
    }
    Ok(Value::object(agent.create_array_from_list(&list)?))
}

fn get_own_property_names(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    own_keys(agent, arg(args, 0), false)
}

fn get_own_property_symbols(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    own_keys(agent, arg(args, 0), true)
}

fn get_prototype_of(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = agent.to_object(arg(args, 0))?;
    Ok(agent.get_prototype_of(o).map_or(Value::NULL, Value::object))
}

fn set_prototype_of(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let value = agent.require_object_coercible(arg(args, 0))?;
    let proto = arg(args, 1);
    let proto = match proto.as_object() {
        Some(p) => Some(p),
        None if proto == Value::NULL => None,
        None => return agent.type_error("Object prototype may only be an object or null"),
    };
    if let Some(o) = value.as_object() {
        if !agent.set_prototype_of(o, proto) {
            return agent.type_error("Object.setPrototypeOf: the prototype cannot be set");
        }
    }
    Ok(value)
}

fn has_own(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let o = agent.to_object(arg(args, 0))?;
    let key = agent.to_property_key(arg(args, 1))?;
    Ok(Value::boolean(agent.has_own_property(o, key)?))
}

fn is(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(agent.same_value(arg(args, 0), arg(args, 1))))
}

/// `Object.prototype.hasOwnProperty(V)` (§20.1.3.2): the key first.
fn has_own_property(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let key = agent.to_property_key(arg(args, 0))?;
    let o = agent.to_object(this)?;
    Ok(Value::boolean(agent.has_own_property(o, key)?))
}

/// `Object.prototype.isPrototypeOf(V)` (§20.1.3.3).
fn is_prototype_of(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(mut v) = arg(args, 0).as_object() else {
        return Ok(Value::FALSE);
    };
    let o = agent.to_object(this)?;
    while let Some(parent) = agent.get_prototype_of(v) {
        if parent == o {
            return Ok(Value::TRUE);
        }
        v = parent;
    }
    Ok(Value::FALSE)
}

/// `Object.prototype.propertyIsEnumerable(V)` (§20.1.3.4).
fn property_is_enumerable(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let key = agent.to_property_key(arg(args, 0))?;
    let o = agent.to_object(this)?;
    let desc = agent.get_own_property(o, key)?;
    Ok(Value::boolean(
        desc.is_some_and(|d| d.enumerable == Some(true)),
    ))
}

/// `Object.prototype.toLocaleString()` (§20.1.3.5).
fn to_locale_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let key = PropertyKey::String(agent.names.to_string);
    agent.invoke(this, key, &[])
}

/// `get Object.prototype.__proto__` (§20.1.3.8.1).
fn proto_getter(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = agent.to_object(this)?;
    Ok(agent.get_prototype_of(o).map_or(Value::NULL, Value::object))
}

/// `set Object.prototype.__proto__` (§20.1.3.8.2).
fn proto_setter(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let this = agent.require_object_coercible(this)?;
    let proto = arg(args, 0);
    let proto = match proto.as_object() {
        Some(p) => Some(p),
        None if proto == Value::NULL => None,
        None => return Ok(Value::UNDEFINED),
    };
    if let Some(o) = this.as_object() {
        if !agent.set_prototype_of(o, proto) {
            return agent.type_error("Object.prototype.__proto__: the prototype cannot be set");
        }
    }
    Ok(Value::UNDEFINED)
}
