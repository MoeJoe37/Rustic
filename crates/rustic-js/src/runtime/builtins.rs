//! The few built-in functions JS-02's conversions rely on: the
//! `valueOf`/`toString` methods that `OrdinaryToPrimitive` calls, the
//! wrapper prototypes' brand checks, and the value properties of the
//! global object. The full library is JS-05.

use super::agent::{Agent, JsResult};
use super::number::number_to_string;
use super::object::{Class, PropertyDescriptor};
use super::shape::PropertyKey;
use super::value::{Kind, Value};

pub(crate) fn install(agent: &mut Agent) -> JsResult<()> {
    let i = agent.intrinsics;
    let value_of = PropertyKey::String(agent.names.value_of);
    let to_string = PropertyKey::String(agent.names.to_string);
    agent.define_method(i.object_prototype, value_of, object_value_of, "valueOf", 0)?;
    agent.define_method(
        i.object_prototype,
        to_string,
        object_to_string,
        "toString",
        0,
    )?;
    agent.define_method(
        i.boolean_prototype,
        value_of,
        boolean_value_of,
        "valueOf",
        0,
    )?;
    agent.define_method(
        i.boolean_prototype,
        to_string,
        boolean_to_string,
        "toString",
        0,
    )?;
    agent.define_method(i.number_prototype, value_of, number_value_of, "valueOf", 0)?;
    agent.define_method(
        i.number_prototype,
        to_string,
        number_to_string_method,
        "toString",
        1,
    )?;
    agent.define_method(i.string_prototype, value_of, string_value_of, "valueOf", 0)?;
    agent.define_method(
        i.string_prototype,
        to_string,
        string_value_of,
        "toString",
        0,
    )?;
    agent.define_method(i.symbol_prototype, value_of, symbol_value_of, "valueOf", 0)?;
    agent.define_method(
        i.symbol_prototype,
        to_string,
        symbol_to_string,
        "toString",
        0,
    )?;
    let to_primitive = PropertyKey::Symbol(agent.symbols.to_primitive);
    let f = agent.new_native_function(symbol_value_of, "[Symbol.toPrimitive]", 1)?;
    agent.define_property_or_throw(
        i.symbol_prototype,
        to_primitive,
        PropertyDescriptor::data_with(Value::object(f), false, false, true),
    )?;
    agent.define_method(i.bigint_prototype, value_of, bigint_value_of, "valueOf", 0)?;
    agent.define_method(
        i.bigint_prototype,
        to_string,
        bigint_to_string,
        "toString",
        0,
    )?;
    agent.define_method(i.error_prototype, to_string, error_to_string, "toString", 0)?;

    // `String.prototype` is a String exotic object for "" (length 0).
    let length = PropertyKey::String(agent.names.length);
    agent.define_property_or_throw(
        i.string_prototype,
        length,
        PropertyDescriptor::data_with(Value::number(0.0), false, false, false),
    )?;
    // `Array.prototype` is an Array exotic object of length 0, and
    // `Function.prototype` a function named "".
    let name = PropertyKey::String(agent.names.name);
    let empty = Value::string(agent.names.empty);
    agent.define_property_or_throw(
        i.function_prototype,
        length,
        PropertyDescriptor::data_with(Value::number(0.0), false, false, true),
    )?;
    agent.define_property_or_throw(
        i.function_prototype,
        name,
        PropertyDescriptor::data_with(empty, false, false, true),
    )?;

    for (prototype, text) in [
        (i.error_prototype, "Error"),
        (i.type_error_prototype, "TypeError"),
        (i.range_error_prototype, "RangeError"),
        (i.reference_error_prototype, "ReferenceError"),
        (i.syntax_error_prototype, "SyntaxError"),
    ] {
        let value = agent.string(text)?;
        agent.define_property_or_throw(
            prototype,
            name,
            PropertyDescriptor::data_with(value, true, false, true),
        )?;
        let message = PropertyKey::String(agent.names.message);
        agent.define_property_or_throw(
            prototype,
            message,
            PropertyDescriptor::data_with(empty, true, false, true),
        )?;
    }

    // Value properties of the global object (§19.1).
    let global = i.global;
    let globals = [
        (agent.names.global_this, Value::object(global), true),
        (agent.names.nan, Value::NAN, false),
        (agent.names.infinity, Value::number(f64::INFINITY), false),
        (agent.names.undefined, Value::UNDEFINED, false),
    ];
    for (key, value, writable) in globals {
        agent.define_property_or_throw(
            global,
            PropertyKey::String(key),
            PropertyDescriptor::data_with(value, writable, false, writable),
        )?;
    }
    Ok(())
}

/// `thisBooleanValue` and friends: the primitive itself or a wrapper of
/// `class`, else a TypeError.
fn this_primitive(
    agent: &mut Agent,
    this: Value,
    class: Class,
    accept: fn(Value) -> bool,
) -> JsResult<Value> {
    if accept(this) {
        return Ok(this);
    }
    if let Some(o) = this.as_object() {
        if agent.class(o) == class {
            if let Some(p) = agent.primitive_of(o) {
                return Ok(p);
            }
        }
    }
    agent.type_error("method called on an incompatible receiver")
}

fn object_value_of(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    agent.to_object(this).map(Value::object)
}

/// `Object.prototype.toString` (§20.1.3.6).
fn object_to_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let builtin = match this.kind() {
        Kind::Undefined => return agent.string("[object Undefined]"),
        Kind::Null => return agent.string("[object Null]"),
        _ => {
            let o = agent.to_object(this)?;
            match agent.class(o) {
                Class::Array => "Array",
                Class::Function => "Function",
                Class::Error => "Error",
                Class::Boolean => "Boolean",
                Class::Number => "Number",
                Class::String => "String",
                _ => "Object",
            }
        }
    };
    let o = agent.to_object(this)?;
    let tag = agent.get(o, PropertyKey::Symbol(agent.symbols.to_string_tag))?;
    let tag = match tag.as_string() {
        Some(s) => agent.heap.string(s).to_text(),
        None => builtin.to_owned(),
    };
    agent.string(&format!("[object {tag}]"))
}

fn boolean_value_of(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    this_primitive(agent, this, Class::Boolean, |v| {
        matches!(v.kind(), Kind::Boolean(_))
    })
}

fn boolean_to_string(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let b = boolean_value_of(agent, this, args)?;
    agent.string(if b == Value::TRUE { "true" } else { "false" })
}

fn number_value_of(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    this_primitive(agent, this, Class::Number, |v| v.as_number().is_some())
}

fn number_to_string_method(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = number_value_of(agent, this, args)?
        .as_number()
        .unwrap_or(f64::NAN);
    // Radixes other than 10 arrive with the Number built-ins (JS-05).
    agent.string(&number_to_string(n))
}

fn string_value_of(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    this_primitive(agent, this, Class::String, |v| v.as_string().is_some())
}

fn symbol_value_of(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    this_primitive(agent, this, Class::Symbol, |v| {
        matches!(v.kind(), Kind::Symbol(_))
    })
}

fn symbol_to_string(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let symbol = symbol_value_of(agent, this, args)?;
    let text = agent.display(symbol);
    agent.string(&text)
}

fn bigint_value_of(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    this_primitive(agent, this, Class::BigInt, |v| {
        matches!(v.kind(), Kind::BigInt(_))
    })
}

fn bigint_to_string(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let b = bigint_value_of(agent, this, args)?;
    let s = agent.to_string(b)?;
    Ok(Value::string(s))
}

/// `Error.prototype.toString` (§20.5.3.4).
fn error_to_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let Some(o) = this.as_object() else {
        return agent.type_error("Error.prototype.toString called on a non-object");
    };
    let name = agent.get(o, PropertyKey::String(agent.names.name))?;
    let name = if name == Value::UNDEFINED {
        "Error".to_owned()
    } else {
        let s = agent.to_string(name)?;
        agent.heap.string(s).to_text()
    };
    let message = agent.get(o, PropertyKey::String(agent.names.message))?;
    let message = if message == Value::UNDEFINED {
        String::new()
    } else {
        let s = agent.to_string(message)?;
        agent.heap.string(s).to_text()
    };
    let text = match (name.is_empty(), message.is_empty()) {
        (true, _) => message,
        (_, true) => name,
        _ => format!("{name}: {message}"),
    };
    agent.string(&text)
}
