//! The few built-in functions JS-02's conversions rely on: the
//! `valueOf`/`toString` methods that `OrdinaryToPrimitive` calls, the
//! wrapper prototypes' brand checks, and the value properties of the
//! global object. The full library is JS-05.

mod array;
pub(crate) mod async_generator;
pub(crate) mod atomics;
pub(crate) mod bigint;
pub(crate) mod buffer;
mod collections;
mod date;
mod error;
mod function;
pub(crate) mod generator;
mod global;
mod iterator;
mod json;
mod math;
mod number_object;
mod object;
pub(crate) mod promise;
pub(crate) mod proxy;
mod reflect;
pub(crate) mod regexp;
mod string_object;
pub(crate) mod typed_array;

use super::FLAG_NATIVE_CONSTRUCTOR;
use super::agent::{Agent, JsResult, NativeFn};
use super::object::{Class, Internal, IteratorKind, PropertyDescriptor};
use super::shape::PropertyKey;
use super::string::JsStr;
use super::value::ObjectId;
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
    object::install(agent)?;
    function::install(agent)?;
    error::install(agent)?;
    global::install(agent)?;
    array::install(agent)?;
    bigint::install(agent)?;
    math::install(agent)?;
    number_object::install(agent)?;
    string_object::install(agent)?;
    json::install(agent)?;
    collections::install(agent)?;
    date::install(agent)?;
    iterator::install(agent)?;
    generator::install(agent)?;
    promise::install(agent)?;
    async_generator::install(agent)?;
    reflect::install(agent)?;
    proxy::install(agent)?;
    buffer::install(agent)?;
    typed_array::install(agent)?;
    atomics::install(agent)?;
    regexp::install(agent)?;

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
    // `%IteratorPrototype%` (§27.1.2): `[Symbol.iterator]() { return this }`.
    let object_prototype = agent.intrinsics.object_prototype;
    let iterator_prototype = agent.new_object_with(Some(object_prototype))?;
    let self_iterator = agent.new_native_function(return_this, "[Symbol.iterator]", 0)?;
    agent.define_property_or_throw(
        iterator_prototype,
        PropertyKey::Symbol(agent.symbols.iterator),
        PropertyDescriptor::data_with(Value::object(self_iterator), true, false, true),
    )?;
    let prototype = agent.new_object_with(Some(iterator_prototype))?;
    let tag = agent.string("Array Iterator")?;
    agent.define_property_or_throw(
        prototype,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
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

pub(crate) fn new_iterator(
    agent: &mut Agent,
    target: Value,
    kind: IteratorKind,
) -> JsResult<Value> {
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

fn return_this(_: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(this)
}

/// Runs `each` on every value that iterating `iterable` with `method`
/// produces, until it returns `false` (which closes the iterator) or
/// the iterator is done. When `each` throws, the iterator is closed and
/// the error wins over anything `return` does (§7.4.11).
pub(crate) fn iterate(
    agent: &mut Agent,
    iterable: Value,
    method: Value,
    mut each: impl FnMut(&mut Agent, Value) -> JsResult<bool>,
) -> JsResult<()> {
    let iterator = agent.call(method, iterable, &[])?;
    let Some(object) = iterator.as_object() else {
        return agent.type_error("iterator is not an object");
    };
    let next = agent.get(object, PropertyKey::String(agent.names.next))?;
    loop {
        let result = agent.call(next, iterator, &[])?;
        let Some(result) = result.as_object() else {
            return agent.type_error("iterator result is not an object");
        };
        let done = agent.get(result, PropertyKey::String(agent.names.done))?;
        if agent.to_boolean(done) {
            return Ok(());
        }
        let value = agent.get(result, PropertyKey::String(agent.names.value))?;
        match each(agent, value) {
            Ok(true) => {}
            Ok(false) => {
                let key = PropertyKey::String(agent.names.return_);
                if let Some(close) = agent.get_method(iterator, key)? {
                    let result = agent.call(close, iterator, &[])?;
                    if result.as_object().is_none() {
                        return agent.type_error("iterator result is not an object");
                    }
                }
                return Ok(());
            }
            Err(error) => {
                let key = PropertyKey::String(agent.names.return_);
                if let Ok(Some(close)) = agent.get_method(iterator, key) {
                    let _ = agent.call(close, iterator, &[]);
                }
                return Err(error);
            }
        }
    }
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

pub(crate) fn iterator_result(agent: &mut Agent, value: Value, done: bool) -> JsResult<Value> {
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
            }) if !kind.walks_table() => Some((o, *target, *index, *kind)),
            _ => None,
        });
    let Some((iterator, target, index, kind)) = state else {
        return agent.type_error("next called on an incompatible receiver");
    };
    let (value, advance) = match (kind, target.kind()) {
        (_, Kind::Undefined) => (None, 0),
        (IteratorKind::Array | IteratorKind::ArrayKeys | IteratorKind::ArrayEntries, _) => {
            let array = target.as_object().expect("array iterators walk objects");
            let length = match typed_array::view_of(agent, array) {
                // §23.1.5.1: a typed array is read by its own length.
                Some(view) => match typed_array::length_of(agent, &view) {
                    Some(length) => length as f64,
                    None => {
                        return agent.type_error(
                            "the typed array is detached or out of the bounds of its buffer",
                        );
                    }
                },
                None => agent.length_of_array_like(array)?,
            };
            if f64::from(index) < length {
                let key = Value::number(f64::from(index));
                let value = match kind {
                    IteratorKind::ArrayKeys => key,
                    IteratorKind::ArrayEntries => {
                        let element = agent.get(array, PropertyKey::Index(index))?;
                        Value::object(agent.create_array_from_list(&[key, element])?)
                    }
                    _ => agent.get(array, PropertyKey::Index(index))?,
                };
                (Some(value), 1)
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
        (_, _) => unreachable!("table iterators have their own next"),
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
pub(crate) fn object_to_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let builtin = match this.kind() {
        Kind::Undefined => return agent.string("[object Undefined]"),
        Kind::Null => return agent.string("[object Null]"),
        _ => {
            let o = agent.to_object(this)?;
            match agent.class(o) {
                _ if array::is_array(agent, Value::object(o))? => "Array",
                Class::Arguments => "Arguments",
                Class::Function => "Function",
                Class::Error => "Error",
                Class::Boolean => "Boolean",
                Class::Number => "Number",
                Class::String => "String",
                Class::Date => "Date",
                _ if regexp::is_regexp_object(agent, o) => "RegExp",
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

// ---- support for the library modules -------------------------------------

/// One method of a built-in object: its name, behavior and `length`.
#[derive(Clone, Copy)]
pub(crate) struct Method {
    name: &'static str,
    behavior: NativeFn,
    length: u32,
}

/// A [`Method`], for the tables of the library modules.
pub(crate) const fn method(name: &'static str, behavior: NativeFn, length: u32) -> Method {
    Method {
        name,
        behavior,
        length,
    }
}

/// Define each method of `table` on `target`: writable, not enumerable,
/// configurable (§18).
pub(crate) fn install_methods(
    agent: &mut Agent,
    target: ObjectId,
    table: &[Method],
) -> JsResult<()> {
    for m in table {
        let key = agent.heap.key(m.name)?;
        agent.define_method(target, key, m.behavior, m.name, m.length)?;
    }
    Ok(())
}

/// A built-in constructor named `name` whose `prototype` is `prototype`,
/// linked both ways and bound on the global object.
pub(crate) fn install_constructor(
    agent: &mut Agent,
    name: &'static str,
    behavior: NativeFn,
    length: u32,
    prototype: ObjectId,
) -> JsResult<ObjectId> {
    let f = agent.new_native_function(behavior, name, length)?;
    agent
        .heap
        .object_mut(f)
        .set_flag(FLAG_NATIVE_CONSTRUCTOR, true);
    let prototype_key = PropertyKey::String(agent.names.prototype);
    agent.define_property_or_throw(
        f,
        prototype_key,
        PropertyDescriptor::data_with(Value::object(prototype), false, false, false),
    )?;
    let constructor_key = agent.heap.key("constructor")?;
    agent.define_property_or_throw(
        prototype,
        constructor_key,
        PropertyDescriptor::data_with(Value::object(f), true, false, true),
    )?;
    let global = agent.intrinsics.global;
    let name_key = agent.heap.key(name)?;
    agent.define_property_or_throw(
        global,
        name_key,
        PropertyDescriptor::data_with(Value::object(f), true, false, true),
    )?;
    Ok(f)
}

/// The argument at `index`, or `undefined`.
pub(crate) fn arg(args: &[Value], index: usize) -> Value {
    args.get(index).copied().unwrap_or(Value::UNDEFINED)
}

/// `GetPrototypeFromConstructor(newTarget, default)` (§10.1.14) for the
/// running built-in: `default` when it was called rather than
/// constructed.
pub(crate) fn prototype_from_new_target(
    agent: &mut Agent,
    default: ObjectId,
) -> JsResult<ObjectId> {
    let Some(target) = agent.new_target() else {
        return Ok(default);
    };
    let key = PropertyKey::String(agent.names.prototype);
    let prototype = agent.get(target, key)?;
    Ok(prototype.as_object().unwrap_or(default))
}
