//! The few built-in functions JS-02's conversions rely on: the
//! `valueOf`/`toString` methods that `OrdinaryToPrimitive` calls, the
//! wrapper prototypes' brand checks, and the value properties of the
//! global object. The full library is JS-05.

use super::agent::{Agent, JsResult};
use super::number::number_to_string;
use super::object::{Class, Internal, IteratorKind, PropertyDescriptor};
use super::shape::PropertyKey;
use super::string::JsStr;
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

    install_iterators(agent)?;

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

/// The iteration protocol of arrays and strings (§23.1.5, §22.1.5): what
/// `for-of`, spread and destructuring need before the rest of the
/// library exists (JS-05).
fn install_iterators(agent: &mut Agent) -> JsResult<()> {
    let object_prototype = agent.intrinsics.object_prototype;
    let prototype = agent.new_object_with(Some(object_prototype))?;
    let next = agent.new_native_function(iterator_next, "next", 0)?;
    agent.define_property_or_throw(
        prototype,
        PropertyKey::String(agent.names.next),
        PropertyDescriptor::data_with(Value::object(next), true, false, true),
    )?;
    let values = agent.new_native_function(array_values, "values", 0)?;
    let array_prototype = agent.intrinsics.array_prototype;
    let iterator = PropertyKey::Symbol(agent.symbols.iterator);
    let values_key = agent.heap.key("values")?;
    for key in [values_key, iterator] {
        agent.define_property_or_throw(
            array_prototype,
            key,
            PropertyDescriptor::data_with(Value::object(values), true, false, true),
        )?;
    }
    let string_iterator = agent.new_native_function(string_iterator, "[Symbol.iterator]", 0)?;
    let string_prototype = agent.intrinsics.string_prototype;
    agent.define_property_or_throw(
        string_prototype,
        iterator,
        PropertyDescriptor::data_with(Value::object(string_iterator), true, false, true),
    )?;
    agent.intrinsics.array_iterator_prototype = prototype;
    agent.intrinsics.array_values = values;
    agent.intrinsics.array_iterator_next = next;
    Ok(())
}

fn new_iterator(agent: &mut Agent, target: Value, kind: IteratorKind) -> JsResult<Value> {
    let prototype = agent.intrinsics.array_iterator_prototype;
    let object = agent.new_object_with(Some(prototype))?;
    agent
        .heap
        .charge(std::mem::size_of::<super::object::Extra>())?;
    agent.heap.object_mut(object).extra_mut().internal = Some(Internal::Iterator {
        target,
        index: 0,
        kind,
    });
    Ok(Value::object(object))
}

/// `Array.prototype.values` (§23.1.3.37).
fn array_values(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let object = agent.to_object(this)?;
    new_iterator(agent, Value::object(object), IteratorKind::Array)
}

/// `String.prototype[Symbol.iterator]` (§22.1.3.36).
fn string_iterator(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let this = agent.require_object_coercible(this)?;
    let string = agent.to_string(this)?;
    new_iterator(agent, Value::string(string), IteratorKind::String)
}

fn iterator_result(agent: &mut Agent, value: Value, done: bool) -> JsResult<Value> {
    let result = agent.new_object()?;
    let value_key = PropertyKey::String(agent.names.value);
    let done_key = PropertyKey::String(agent.names.done);
    agent.create_data_property_or_throw(result, value_key, value)?;
    agent.create_data_property_or_throw(result, done_key, Value::boolean(done))?;
    Ok(Value::object(result))
}

/// `%ArrayIteratorPrototype%.next` (§23.1.5.2.1), shared with string
/// iterators.
fn iterator_next(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let state = this
        .as_object()
        .and_then(|o| match agent.heap.object(o).internal() {
            Some(Internal::Iterator {
                target,
                index,
                kind,
            }) => Some((o, *target, *index, *kind)),
            _ => None,
        });
    let Some((iterator, target, index, kind)) = state else {
        return agent.type_error("next called on an incompatible receiver");
    };
    let (value, advance) = match (kind, target.kind()) {
        (_, Kind::Undefined) => (None, 0),
        (IteratorKind::Array, _) => {
            let array = target.as_object().expect("array iterators walk objects");
            let length = agent.length_of_array_like(array)?;
            if f64::from(index) < length {
                (Some(agent.get(array, PropertyKey::Index(index))?), 1)
            } else {
                (None, 0)
            }
        }
        (IteratorKind::String, Kind::String(s)) => {
            let text = agent.heap.string(s);
            match text.unit(index as usize) {
                None => (None, 0),
                Some(first) => {
                    // A surrogate pair is one code point (§22.1.5.1).
                    let second = text.unit(index as usize + 1);
                    let pair = (0xD800..0xDC00).contains(&first)
                        && second.is_some_and(|u| (0xDC00..0xE000).contains(&u));
                    let units: Vec<u16> = [Some(first), second.filter(|_| pair)]
                        .into_iter()
                        .flatten()
                        .collect();
                    let advance = units.len() as u32;
                    let piece = agent.string_from(JsStr::from_units(&units))?;
                    (Some(piece), advance)
                }
            }
        }
        (IteratorKind::String, _) => (None, 0),
    };
    if let Some(Internal::Iterator { target, index, .. }) =
        agent.heap.object_mut(iterator).internal_mut()
    {
        match value {
            Some(_) => *index += advance,
            None => *target = Value::UNDEFINED,
        }
    }
    iterator_result(agent, value.unwrap_or(Value::UNDEFINED), value.is_none())
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
