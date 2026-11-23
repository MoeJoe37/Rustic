//! `Function` (§20.2): the constructor, which compiles source text, and
//! `Function.prototype` with `call`, `apply`, `bind`, `toString` and
//! `[Symbol.hasInstance]`.

use super::{Method, arg, install_constructor, install_methods, method};
use crate::runtime::FLAG_NATIVE_CONSTRUCTOR;
use crate::runtime::agent::{Agent, JsResult};
use crate::runtime::object::{Class, Internal, Object, PropertyDescriptor};
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::ObjectId;
use crate::runtime::value::Value;

const PROTOTYPE_METHODS: &[Method] = &[
    method("call", call, 1),
    method("apply", apply, 2),
    method("bind", bind, 1),
    method("toString", to_string, 0),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let prototype = agent.intrinsics.function_prototype;
    install_constructor(agent, "Function", function_constructor, 1, prototype)?;
    install_methods(agent, prototype, PROTOTYPE_METHODS)?;
    // `Function.prototype[@@hasInstance]` is not writable or configurable
    // (§20.2.3.6).
    let f = agent.new_native_function(has_instance, "[Symbol.hasInstance]", 1)?;
    let key = PropertyKey::Symbol(agent.symbols.has_instance);
    agent.define_property_or_throw(
        prototype,
        key,
        PropertyDescriptor::data_with(Value::object(f), false, false, false),
    )?;
    // `%ThrowTypeError%` (§10.2.4.1): one frozen function per realm.
    let thrower = agent.new_native_function(throw_type_error, "", 0)?;
    for name in [agent.names.length, agent.names.name] {
        agent.define_property_or_throw(
            thrower,
            PropertyKey::String(name),
            PropertyDescriptor {
                configurable: Some(false),
                ..Default::default()
            },
        )?;
    }
    agent.prevent_extensions(thrower)?;
    agent.intrinsics.throw_type_error = thrower;
    // `AddRestrictedFunctionProperties` (§10.2.4): `caller` and
    // `arguments` throw for every function that inherits them.
    for name in ["caller", "arguments"] {
        let key = agent.heap.key(name)?;
        let thrower = Value::object(thrower);
        agent.define_property_or_throw(
            prototype,
            key,
            PropertyDescriptor::accessor(thrower, thrower, false, true),
        )?;
    }
    Ok(())
}

/// `%ThrowTypeError%`. Reached through the inherited `caller` or
/// `arguments` of a sloppy function it answers `null` instead, as
/// browsers do for such functions (which §17.1 lets have their own
/// properties of those names); scripts on the web read them.
fn throw_type_error(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let sloppy = this
        .as_object()
        .and_then(|o| agent.heap.closure(o))
        .is_some_and(|closure| {
            let code = agent.heap.code(closure.code);
            !code.strict
                && code.kind == crate::vm::bytecode::FunctionKind::Normal
                && !code.generator
                && !code.is_async
        });
    if sloppy {
        return Ok(Value::NULL);
    }
    agent.type_error(
        "'caller', 'callee', and 'arguments' properties may not be accessed on strict mode functions or the arguments objects for calls to them",
    )
}

/// `Function(...params, body)` (§20.2.1.1).
fn function_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let (params, body) = match args.split_last() {
        None => (&[][..], None),
        Some((body, params)) => (params, Some(*body)),
    };
    let mut parameter_text = Vec::with_capacity(params.len());
    for &p in params {
        let s = agent.to_string(p)?;
        parameter_text.push(agent.heap.string(s).to_text());
    }
    let body_text = match body {
        Some(b) => {
            let s = agent.to_string(b)?;
            agent.heap.flatten(s)?;
            agent.heap.string(s).to_text()
        }
        None => String::new(),
    };
    let f = agent.create_dynamic_function("function", &parameter_text.join(","), &body_text)?;
    // A subclass's prototype (§20.2.1.1.1 step 23).
    if let (Some(target), Some(o)) = (agent.new_target(), f.as_object()) {
        if agent.active_function() != Some(target) {
            let default = agent.intrinsics.function_prototype;
            let prototype = super::prototype_from_new_target(agent, default)?;
            agent.set_prototype_of(o, Some(prototype))?;
        }
    }
    Ok(f)
}

/// `Function.prototype.call(thisArg, ...args)` (§20.2.3.3).
fn call(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    if !agent.is_callable(this) {
        return agent.type_error("Function.prototype.call called on a non-function");
    }
    let rest = args.get(1..).unwrap_or(&[]);
    agent.call(this, arg(args, 0), rest)
}

/// `Function.prototype.apply(thisArg, argArray)` (§20.2.3.1).
fn apply(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    if !agent.is_callable(this) {
        return agent.type_error("Function.prototype.apply called on a non-function");
    }
    let list = arg(args, 1);
    if list == Value::UNDEFINED || list == Value::NULL {
        return agent.call(this, arg(args, 0), &[]);
    }
    let list = create_list_from_array_like(agent, list)?;
    agent.call(this, arg(args, 0), &list)
}

/// `CreateListFromArrayLike(obj)` (§7.3.18).
pub(crate) fn create_list_from_array_like(agent: &mut Agent, value: Value) -> JsResult<Vec<Value>> {
    let Some(o) = value.as_object() else {
        return agent.type_error("argument list is not an object");
    };
    let length = agent.length_of_array_like(o)?;
    // The heap budget bounds what a list may hold; refuse more up front.
    if length > f64::from(u32::MAX) {
        return Err(agent.throw(crate::runtime::ErrorKind::RangeError, "too many arguments"));
    }
    let length = length as u32;
    agent.heap.charge(8 * length as usize)?;
    let mut list = Vec::with_capacity(length as usize);
    for index in 0..length {
        list.push(agent.get(o, PropertyKey::Index(index))?);
    }
    Ok(list)
}

/// `Function.prototype.bind(thisArg, ...args)` (§20.2.3.2).
fn bind(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = this.as_object().filter(|_| agent.is_callable(this)) else {
        return agent.type_error("Function.prototype.bind called on a non-function");
    };
    let bound_this = arg(args, 0);
    let bound_args = args.get(1..).unwrap_or(&[]).to_vec();
    let f = new_bound_function(agent, target, bound_this, bound_args)?;
    // `length`: the target's length minus the bound arguments, if it is
    // a number (§20.2.3.2 steps 4 to 6).
    let length_key = PropertyKey::String(agent.names.length);
    let mut length = 0.0;
    if agent.has_own_property(target, length_key)? {
        let target_length = agent.get(target, length_key)?;
        if let Some(n) = target_length.as_number() {
            length = if n == f64::INFINITY {
                f64::INFINITY
            } else if n == f64::NEG_INFINITY {
                0.0
            } else {
                let n = if n.is_nan() { 0.0 } else { n.trunc() };
                (n - (args.len().saturating_sub(1)) as f64).max(0.0)
            };
        }
    }
    agent.define_property_or_throw(
        f,
        length_key,
        PropertyDescriptor::data_with(Value::number(length), false, false, true),
    )?;
    let name_key = PropertyKey::String(agent.names.name);
    let target_name = agent.get(target, name_key)?;
    let name = match target_name.as_string() {
        Some(s) => {
            agent.heap.flatten(s)?;
            let mut text = agent.heap.string(s).to_units();
            let mut units: Vec<u16> = "bound ".encode_utf16().collect();
            units.append(&mut text);
            agent.string_from(crate::runtime::JsStr::from_units(&units))?
        }
        None => agent.string("bound ")?,
    };
    agent.define_property_or_throw(
        f,
        name_key,
        PropertyDescriptor::data_with(name, false, false, true),
    )?;
    Ok(Value::object(f))
}

/// `BoundFunctionCreate(target, boundThis, boundArgs)` (§10.4.1.3): a
/// native function whose behavior forwards to the target, and a
/// constructor exactly when the target is one.
fn new_bound_function(
    agent: &mut Agent,
    target: ObjectId,
    this: Value,
    args: Vec<Value>,
) -> JsResult<ObjectId> {
    let prototype = agent.get_prototype_of(target)?;
    let constructor = agent.is_constructor_value(Value::object(target));
    agent.heap.charge(8 * args.len())?;
    let mut object = Object::new(Class::Function, prototype);
    object.extra_mut().native = Some(bound_function_call);
    object.extra_mut().internal = Some(Internal::Bound {
        target,
        this,
        args: args.into_boxed_slice(),
    });
    object.set_flag(FLAG_NATIVE_CONSTRUCTOR, constructor);
    Ok(agent.heap.alloc_object(object)?)
}

/// `[[Call]]` and `[[Construct]]` of a bound function (§10.4.1.1,
/// §10.4.1.2).
fn bound_function_call(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let f = agent
        .active_function()
        .expect("a bound function is running");
    let Some(Internal::Bound {
        target,
        this,
        args: bound,
    }) = agent.heap.object(f).internal()
    else {
        unreachable!("bound functions carry their bindings")
    };
    let (target, this) = (*target, *this);
    let mut list = bound.to_vec();
    list.extend_from_slice(args);
    match agent.new_target() {
        None => agent.call(Value::object(target), this, &list),
        Some(new_target) => {
            let new_target = if new_target == f { target } else { new_target };
            agent.construct_with(Value::object(target), &list, new_target)
        }
    }
}

/// `Function.prototype.toString()` (§20.2.3.5). Source text is not kept,
/// so every function prints in the `NativeFunction` form, which the
/// specification allows for host-defined and built-in functions.
fn to_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    if !agent.is_callable(this) {
        return agent.type_error("Function.prototype.toString called on a non-function");
    }
    let o = this.as_object().expect("callables are objects");
    let name_key = PropertyKey::String(agent.names.name);
    let name = agent
        .get_own_property(o, name_key)?
        .and_then(|d| d.value)
        .and_then(Value::as_string)
        .map(|s| agent.heap.string(s).to_text())
        .filter(|n| is_identifier_like(n))
        .unwrap_or_default();
    agent.string(&format!("function {name}() {{ [native code] }}"))
}

fn is_identifier_like(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '$' || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '$' || c == '_')
}

/// `Function.prototype[@@hasInstance](V)` (§20.2.3.6).
fn has_instance(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(
        agent.ordinary_has_instance(this, arg(args, 0))?,
    ))
}
