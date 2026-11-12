//! The error constructors (§20.5): `Error` and the native errors.

use super::{arg, install_constructor, prototype_from_new_target};
use crate::runtime::agent::{Agent, JsResult, NativeFn};
use crate::runtime::object::{Class, Object, PropertyDescriptor};
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::ObjectId;
use crate::runtime::value::Value;

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let i = agent.intrinsics;
    let error = install_constructor(agent, "Error", error_constructor, 1, i.error_prototype)?;
    // The native errors inherit from `Error` (§20.5.6.2).
    let natives: [(&'static str, NativeFn, Option<ObjectId>); 6] = [
        ("TypeError", type_error, Some(i.type_error_prototype)),
        ("RangeError", range_error, Some(i.range_error_prototype)),
        (
            "ReferenceError",
            reference_error,
            Some(i.reference_error_prototype),
        ),
        ("SyntaxError", syntax_error, Some(i.syntax_error_prototype)),
        ("EvalError", eval_error, None),
        ("URIError", uri_error, None),
    ];
    for (name, behavior, prototype) in natives {
        let prototype = match prototype {
            Some(p) => p,
            None => {
                let p = agent.new_object_with(Some(i.error_prototype))?;
                let name_key = PropertyKey::String(agent.names.name);
                let value = Value::string(agent.heap.intern_str(name)?);
                agent.define_property_or_throw(
                    p,
                    name_key,
                    PropertyDescriptor::data_with(value, true, false, true),
                )?;
                let message = PropertyKey::String(agent.names.message);
                let empty = Value::string(agent.names.empty);
                agent.define_property_or_throw(
                    p,
                    message,
                    PropertyDescriptor::data_with(empty, true, false, true),
                )?;
                p
            }
        };
        let f = install_constructor(agent, name, behavior, 1, prototype)?;
        agent.set_prototype_of(f, Some(error));
    }

    // AggregateError (§20.5.7).
    let prototype = agent.new_object_with(Some(i.error_prototype))?;
    let name_key = PropertyKey::String(agent.names.name);
    let value = Value::string(agent.heap.intern_str("AggregateError")?);
    agent.define_property_or_throw(
        prototype,
        name_key,
        PropertyDescriptor::data_with(value, true, false, true),
    )?;
    let message = PropertyKey::String(agent.names.message);
    let empty = Value::string(agent.names.empty);
    agent.define_property_or_throw(
        prototype,
        message,
        PropertyDescriptor::data_with(empty, true, false, true),
    )?;
    let f = install_constructor(agent, "AggregateError", aggregate_error, 2, prototype)?;
    agent.set_prototype_of(f, Some(error));
    agent.intrinsics.aggregate_error_prototype = prototype;
    Ok(())
}

/// `AggregateError(errors, message, options)` (§20.5.7.1.1).
fn aggregate_error(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let default = agent.intrinsics.aggregate_error_prototype;
    let o = construct_error(agent, default, &[arg(args, 1), arg(args, 2)])?;
    let iterable = arg(args, 0);
    let key = PropertyKey::Symbol(agent.symbols.iterator);
    let Some(method) = agent.get_method(iterable, key)? else {
        return agent.type_error("AggregateError errors are not iterable");
    };
    let mut errors = Vec::new();
    super::iterate(agent, iterable, method, |_, v| {
        errors.push(v);
        Ok(true)
    })?;
    // The list is rooted by nothing while it grows, but nothing collects
    // inside a native call.
    let array = agent.create_array_from_list(&errors)?;
    let errors_key = agent.heap.key("errors")?;
    let o_id = o.as_object().expect("an error object");
    agent.define_property_or_throw(
        o_id,
        errors_key,
        PropertyDescriptor::data_with(Value::object(array), true, false, true),
    )?;
    Ok(o)
}

/// An AggregateError with `errors` (for `Promise.any`).
pub(crate) fn new_aggregate_error(agent: &mut Agent, errors: Value) -> JsResult<Value> {
    let prototype = agent.intrinsics.aggregate_error_prototype;
    let o = agent
        .heap
        .alloc_object(Object::new(Class::Error, Some(prototype)))?;
    let errors_key = agent.heap.key("errors")?;
    agent.define_property_or_throw(
        o,
        errors_key,
        PropertyDescriptor::data_with(errors, true, false, true),
    )?;
    Ok(Value::object(o))
}

/// The prototype a native error constructor uses: `name`'s, found
/// through the global constructor for the two errors that have no
/// intrinsic slot.
fn named_prototype(agent: &mut Agent, name: &str) -> JsResult<ObjectId> {
    let global = agent.intrinsics.global;
    let key = agent.heap.key(name)?;
    let constructor = agent.get(global, key)?;
    let prototype = match constructor.as_object() {
        Some(c) => {
            let key = PropertyKey::String(agent.names.prototype);
            agent.get(c, key)?.as_object()
        }
        None => None,
    };
    Ok(prototype.unwrap_or(agent.intrinsics.error_prototype))
}

/// `Error(message, options)` and the native errors (§20.5.1.1,
/// §20.5.6.1.1).
fn construct_error(agent: &mut Agent, default: ObjectId, args: &[Value]) -> JsResult<Value> {
    let prototype = prototype_from_new_target(agent, default)?;
    let o = agent
        .heap
        .alloc_object(Object::new(Class::Error, Some(prototype)))?;
    let message = arg(args, 0);
    if message != Value::UNDEFINED {
        let text = agent.to_string(message)?;
        let key = PropertyKey::String(agent.names.message);
        agent.define_property_or_throw(
            o,
            key,
            PropertyDescriptor::data_with(Value::string(text), true, false, true),
        )?;
    }
    // InstallErrorCause (§20.5.8.1).
    if let Some(options) = arg(args, 1).as_object() {
        let cause_key = agent.heap.key("cause")?;
        if agent.has_property(options, cause_key)? {
            let cause = agent.get(options, cause_key)?;
            agent.define_property_or_throw(
                o,
                cause_key,
                PropertyDescriptor::data_with(cause, true, false, true),
            )?;
        }
    }
    Ok(Value::object(o))
}

fn error_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let default = agent.intrinsics.error_prototype;
    construct_error(agent, default, args)
}

fn type_error(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let default = agent.intrinsics.type_error_prototype;
    construct_error(agent, default, args)
}

fn range_error(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let default = agent.intrinsics.range_error_prototype;
    construct_error(agent, default, args)
}

fn reference_error(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let default = agent.intrinsics.reference_error_prototype;
    construct_error(agent, default, args)
}

fn syntax_error(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let default = agent.intrinsics.syntax_error_prototype;
    construct_error(agent, default, args)
}

fn eval_error(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let default = named_prototype(agent, "EvalError")?;
    construct_error(agent, default, args)
}

fn uri_error(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let default = named_prototype(agent, "URIError")?;
    construct_error(agent, default, args)
}
