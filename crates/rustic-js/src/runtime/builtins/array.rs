//! `Array` (§23.1): the constructor, `Array.from`/`of`/`isArray` and the
//! methods of `Array.prototype`.
//!
//! The algorithms follow the specification step by step through the
//! object internal methods, so they work on any array-like `this` and on
//! arrays with holes, accessors or a changed prototype. Indices above
//! 2³² − 2 become string keys, as for any object.

use std::cmp::Ordering;

use super::{
    Method, arg, install_constructor, install_methods, iterate, method, new_iterator,
    prototype_from_new_target,
};
use crate::runtime::agent::{Agent, JsResult};
use crate::runtime::number::{number_to_string, to_integer_or_infinity};
use crate::runtime::object::{Class, IteratorKind, Object, PropertyDescriptor};
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::{ObjectId, StringId, Value};
use crate::runtime::{ErrorKind, JsStr};

/// 2⁵³ − 1, the largest length an array-like may have.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
/// 2³² − 1, the largest length of an Array exotic object.
const MAX_ARRAY_LENGTH: f64 = 4_294_967_295.0;

const STATIC_METHODS: &[Method] = &[
    method("from", from, 1),
    method("isArray", is_array_method, 1),
    method("of", of, 0),
];

const PROTOTYPE_METHODS: &[Method] = &[
    method("at", at, 1),
    method("concat", concat, 1),
    method("copyWithin", copy_within, 2),
    method("entries", entries, 0),
    method("every", every, 1),
    method("fill", fill, 1),
    method("filter", filter, 1),
    method("find", find, 1),
    method("findIndex", find_index, 1),
    method("findLast", find_last, 1),
    method("findLastIndex", find_last_index, 1),
    method("flat", flat, 0),
    method("flatMap", flat_map, 1),
    method("forEach", for_each, 1),
    method("includes", includes, 1),
    method("indexOf", index_of, 1),
    method("join", join, 1),
    method("keys", keys, 0),
    method("lastIndexOf", last_index_of, 1),
    method("map", map, 1),
    method("pop", pop, 0),
    method("push", push, 1),
    method("reduce", reduce, 1),
    method("reduceRight", reduce_right, 1),
    method("reverse", reverse, 0),
    method("shift", shift, 0),
    method("slice", slice, 2),
    method("some", some, 1),
    method("sort", sort, 1),
    method("splice", splice, 2),
    method("toLocaleString", to_locale_string, 0),
    method("toReversed", to_reversed, 0),
    method("toSorted", to_sorted, 1),
    method("toSpliced", to_spliced, 2),
    method("toString", to_string, 0),
    method("unshift", unshift, 1),
    method("with", with, 2),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let prototype = agent.intrinsics.array_prototype;
    let constructor = install_constructor(agent, "Array", array_constructor, 1, prototype)?;
    install_methods(agent, constructor, STATIC_METHODS)?;
    install_methods(agent, prototype, PROTOTYPE_METHODS)?;
    // `get Array[@@species]` (§23.1.2.5).
    let getter = agent.new_native_function(species_getter, "get [Symbol.species]", 0)?;
    let species = PropertyKey::Symbol(agent.symbols.species);
    agent.define_property_or_throw(
        constructor,
        species,
        PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, false, true),
    )?;
    // `Array.prototype[@@unscopables]` (§23.1.3.41): a null-prototype
    // object naming the methods newer than `with` statements.
    let unscopables = agent.new_object_with(None)?;
    for name in [
        "at",
        "copyWithin",
        "entries",
        "fill",
        "find",
        "findIndex",
        "findLast",
        "findLastIndex",
        "flat",
        "flatMap",
        "includes",
        "keys",
        "toReversed",
        "toSorted",
        "toSpliced",
        "values",
    ] {
        let key = agent.heap.key(name)?;
        agent.create_data_property_or_throw(unscopables, key, Value::TRUE)?;
    }
    let key = PropertyKey::Symbol(agent.symbols.unscopables);
    agent.define_property_or_throw(
        prototype,
        key,
        PropertyDescriptor::data_with(Value::object(unscopables), false, false, true),
    )
}

// ---- helpers ----------------------------------------------------------------

/// The property key of index `k`, an integer in [0, 2⁵³).
fn index_key(agent: &mut Agent, k: f64) -> JsResult<PropertyKey> {
    if k < MAX_ARRAY_LENGTH {
        // `k` is a non-negative integer below 2³² − 1.
        Ok(PropertyKey::Index(k as u32))
    } else {
        Ok(agent.heap.key(&number_to_string(k))?)
    }
}

fn length_key(agent: &Agent) -> PropertyKey {
    PropertyKey::String(agent.names.length)
}

fn get_index(agent: &mut Agent, o: ObjectId, k: f64) -> JsResult<Value> {
    let key = index_key(agent, k)?;
    agent.get(o, key)
}

fn has_index(agent: &mut Agent, o: ObjectId, k: f64) -> JsResult<bool> {
    let key = index_key(agent, k)?;
    agent.has_property(o, key)
}

fn set_index(agent: &mut Agent, o: ObjectId, k: f64, value: Value) -> JsResult<()> {
    let key = index_key(agent, k)?;
    agent.set(o, key, value, true)
}

fn create_index(agent: &mut Agent, o: ObjectId, k: f64, value: Value) -> JsResult<()> {
    let key = index_key(agent, k)?;
    agent.create_data_property_or_throw(o, key, value)
}

fn delete_index(agent: &mut Agent, o: ObjectId, k: f64) -> JsResult<()> {
    let key = index_key(agent, k)?;
    agent.delete_property_or_throw(o, key)
}

fn set_length(agent: &mut Agent, o: ObjectId, length: f64) -> JsResult<()> {
    let key = length_key(agent);
    agent.set(o, key, Value::number(length), true)
}

/// `ToObject(this)` and `LengthOfArrayLike` of it.
fn this_array_like(agent: &mut Agent, this: Value) -> JsResult<(ObjectId, f64)> {
    let o = agent.to_object(this)?;
    let length = agent.length_of_array_like(o)?;
    Ok((o, length))
}

fn to_integer(agent: &mut Agent, value: Value) -> JsResult<f64> {
    let n = agent.to_number(value)?;
    Ok(to_integer_or_infinity(n))
}

/// A relative index argument resolved against `length` (negative counts
/// from the end), clamped to [0, length]; `default` when undefined.
fn relative_index(agent: &mut Agent, value: Value, length: f64, default: f64) -> JsResult<f64> {
    if value == Value::UNDEFINED {
        return Ok(default);
    }
    let relative = to_integer(agent, value)?;
    Ok(if relative < 0.0 {
        (length + relative).max(0.0)
    } else {
        relative.min(length)
    })
}

/// The callback argument, or a TypeError naming the method.
fn callback(agent: &mut Agent, args: &[Value], name: &str) -> JsResult<Value> {
    let f = arg(args, 0);
    if !agent.is_callable(f) {
        return agent.type_error(&format!(
            "Array.prototype.{name}: callback is not a function"
        ));
    }
    Ok(f)
}

fn range_error<T>(agent: &mut Agent, message: &str) -> JsResult<T> {
    Err(agent.throw(ErrorKind::RangeError, message))
}

/// `IsArray(argument)` (§7.2.2). There are no proxies yet.
pub(crate) fn is_array(agent: &Agent, value: Value) -> bool {
    value
        .as_object()
        .is_some_and(|o| agent.class(o) == Class::Array)
}

/// `ArrayCreate(length, proto)` (§10.4.2.2).
fn array_create(agent: &mut Agent, length: f64, prototype: Option<ObjectId>) -> JsResult<ObjectId> {
    if length > MAX_ARRAY_LENGTH {
        return range_error(agent, "invalid array length");
    }
    let prototype = prototype.unwrap_or(agent.intrinsics.array_prototype);
    let a = agent
        .heap
        .alloc_object(Object::new(Class::Array, Some(prototype)))?;
    if length > 0.0 {
        // `length` is an integer below 2³².
        agent.heap.object_mut(a).length = length as u32;
    }
    Ok(a)
}

/// `ArraySpeciesCreate(originalArray, length)` (§10.4.2.3).
fn array_species_create(agent: &mut Agent, original: ObjectId, length: f64) -> JsResult<ObjectId> {
    if !is_array(agent, Value::object(original)) {
        return array_create(agent, length, None);
    }
    let key = agent.heap.key("constructor")?;
    let mut c = agent.get(original, key)?;
    if let Some(co) = c.as_object() {
        let species = PropertyKey::Symbol(agent.symbols.species);
        c = agent.get(co, species)?;
        if c == Value::NULL {
            c = Value::UNDEFINED;
        }
    }
    if c == Value::UNDEFINED {
        return array_create(agent, length, None);
    }
    if !agent.is_constructor_value(c) {
        return agent.type_error("species is not a constructor");
    }
    let made = agent.construct(c, &[Value::number(length)])?;
    match made.as_object() {
        Some(o) => Ok(o),
        None => agent.type_error("species constructor returned a non-object"),
    }
}

// ---- the constructor --------------------------------------------------------

/// `Array(...values)` (§23.1.1.1).
fn array_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let default = agent.intrinsics.array_prototype;
    let prototype = prototype_from_new_target(agent, default)?;
    match args {
        [] => Ok(Value::object(array_create(agent, 0.0, Some(prototype))?)),
        [length] => {
            let a = array_create(agent, 0.0, Some(prototype))?;
            let length = match length.as_number() {
                None => {
                    create_index(agent, a, 0.0, *length)?;
                    1.0
                }
                Some(n) => {
                    let int = f64::from(crate::runtime::number::to_uint32(n));
                    if int != n {
                        return range_error(agent, "invalid array length");
                    }
                    int
                }
            };
            set_length(agent, a, length)?;
            Ok(Value::object(a))
        }
        values => {
            let a = array_create(agent, 0.0, Some(prototype))?;
            agent.heap.charge(8 * values.len())?;
            let object = agent.heap.object_mut(a);
            object.elements = values.to_vec();
            // The heap budget keeps argument lists far below 2³².
            object.length = values.len() as u32;
            Ok(Value::object(a))
        }
    }
}

fn species_getter(_: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(this)
}

/// `Array.isArray(arg)` (§23.1.2.2).
fn is_array_method(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(is_array(agent, arg(args, 0))))
}

/// A new object from `this` as a constructor, or an array.
fn construct_or_array(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<ObjectId> {
    if agent.is_constructor_value(this) {
        let made = agent.construct(this, args)?;
        match made.as_object() {
            Some(o) => Ok(o),
            None => agent.type_error("constructor returned a non-object"),
        }
    } else {
        let length = match args.first() {
            Some(n) => n.as_number().unwrap_or(0.0),
            None => 0.0,
        };
        array_create(agent, length, None)
    }
}

/// `Array.of(...items)` (§23.1.2.3).
fn of(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let length = args.len() as f64;
    let a = construct_or_array(agent, this, &[Value::number(length)])?;
    for (k, &value) in args.iter().enumerate() {
        create_index(agent, a, k as f64, value)?;
    }
    set_length(agent, a, length)?;
    Ok(Value::object(a))
}

/// `Array.from(items, mapfn, thisArg)` (§23.1.2.1).
fn from(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let items = arg(args, 0);
    let mapper = arg(args, 1);
    let this_arg = arg(args, 2);
    let mapping = mapper != Value::UNDEFINED;
    if mapping && !agent.is_callable(mapper) {
        return agent.type_error("Array.from: mapper is not a function");
    }
    let iterator_key = PropertyKey::Symbol(agent.symbols.iterator);
    if let Some(using) = agent.get_method(items, iterator_key)? {
        let a = construct_or_array(agent, this, &[])?;
        let mut k = 0.0;
        iterate(agent, items, using, |agent, value| {
            if k >= MAX_SAFE_INTEGER {
                return agent.type_error("Array.from: too many elements");
            }
            let value = if mapping {
                agent.call(mapper, this_arg, &[value, Value::number(k)])?
            } else {
                value
            };
            create_index(agent, a, k, value)?;
            k += 1.0;
            Ok(true)
        })?;
        set_length(agent, a, k)?;
        return Ok(Value::object(a));
    }
    let array_like = agent.to_object(items)?;
    let length = agent.length_of_array_like(array_like)?;
    let a = construct_or_array(agent, this, &[Value::number(length)])?;
    let mut k = 0.0;
    while k < length {
        let value = get_index(agent, array_like, k)?;
        let value = if mapping {
            agent.call(mapper, this_arg, &[value, Value::number(k)])?
        } else {
            value
        };
        create_index(agent, a, k, value)?;
        k += 1.0;
    }
    set_length(agent, a, length)?;
    Ok(Value::object(a))
}

// ---- iteration methods ------------------------------------------------------

fn keys(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = agent.to_object(this)?;
    new_iterator(agent, Value::object(o), IteratorKind::ArrayKeys)
}

fn entries(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = agent.to_object(this)?;
    new_iterator(agent, Value::object(o), IteratorKind::ArrayEntries)
}

/// The shared loop of `every`, `some`, `forEach`, `map` and `filter`:
/// calls the callback for each present element, in order, and passes
/// the result to `step`, which says whether to go on.
fn each_present(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
    name: &str,
    mut step: impl FnMut(&mut Agent, f64, Value, Value) -> JsResult<bool>,
) -> JsResult<(ObjectId, f64)> {
    let (o, length) = this_array_like(agent, this)?;
    let f = callback(agent, args, name)?;
    let this_arg = arg(args, 1);
    let mut k = 0.0;
    while k < length {
        if has_index(agent, o, k)? {
            let value = get_index(agent, o, k)?;
            let result = agent.call(f, this_arg, &[value, Value::number(k), Value::object(o)])?;
            if !step(agent, k, value, result)? {
                break;
            }
        }
        k += 1.0;
    }
    Ok((o, length))
}

fn every(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let mut all = true;
    each_present(agent, this, args, "every", |agent, _, _, result| {
        all = agent.to_boolean(result);
        Ok(all)
    })?;
    Ok(Value::boolean(all))
}

fn some(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let mut any = false;
    each_present(agent, this, args, "some", |agent, _, _, result| {
        any = agent.to_boolean(result);
        Ok(!any)
    })?;
    Ok(Value::boolean(any))
}

fn for_each(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    each_present(agent, this, args, "forEach", |_, _, _, _| Ok(true))?;
    Ok(Value::UNDEFINED)
}

/// `Array.prototype.map` (§23.1.3.21).
fn map(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    let f = callback(agent, args, "map")?;
    let this_arg = arg(args, 1);
    let a = array_species_create(agent, o, length)?;
    let mut k = 0.0;
    while k < length {
        if has_index(agent, o, k)? {
            let value = get_index(agent, o, k)?;
            let mapped = agent.call(f, this_arg, &[value, Value::number(k), Value::object(o)])?;
            create_index(agent, a, k, mapped)?;
        }
        k += 1.0;
    }
    Ok(Value::object(a))
}

/// `Array.prototype.filter` (§23.1.3.8).
fn filter(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, _) = this_array_like(agent, this)?;
    let a = array_species_create(agent, o, 0.0)?;
    let mut to = 0.0;
    each_present(agent, this, args, "filter", |agent, _, value, result| {
        if agent.to_boolean(result) {
            create_index(agent, a, to, value)?;
            to += 1.0;
        }
        Ok(true)
    })?;
    Ok(Value::object(a))
}

/// `FindViaPredicate` (§23.1.3.12.1): every index, holes included, from
/// the start or the end. Returns the index and value found.
fn find_via_predicate(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
    name: &str,
    ascending: bool,
) -> JsResult<Option<(f64, Value)>> {
    let (o, length) = this_array_like(agent, this)?;
    let predicate = callback(agent, args, name)?;
    let this_arg = arg(args, 1);
    let mut i = 0.0;
    while i < length {
        let k = if ascending { i } else { length - 1.0 - i };
        let value = get_index(agent, o, k)?;
        let result = agent.call(
            predicate,
            this_arg,
            &[value, Value::number(k), Value::object(o)],
        )?;
        if agent.to_boolean(result) {
            return Ok(Some((k, value)));
        }
        i += 1.0;
    }
    Ok(None)
}

fn find(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let found = find_via_predicate(agent, this, args, "find", true)?;
    Ok(found.map_or(Value::UNDEFINED, |(_, v)| v))
}

fn find_index(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let found = find_via_predicate(agent, this, args, "findIndex", true)?;
    Ok(Value::number(found.map_or(-1.0, |(k, _)| k)))
}

fn find_last(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let found = find_via_predicate(agent, this, args, "findLast", false)?;
    Ok(found.map_or(Value::UNDEFINED, |(_, v)| v))
}

fn find_last_index(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let found = find_via_predicate(agent, this, args, "findLastIndex", false)?;
    Ok(Value::number(found.map_or(-1.0, |(k, _)| k)))
}

/// `reduce` and `reduceRight` (§23.1.3.24, §23.1.3.25).
fn reduce_impl(agent: &mut Agent, this: Value, args: &[Value], right: bool) -> JsResult<Value> {
    let name = if right { "reduceRight" } else { "reduce" };
    let (o, length) = this_array_like(agent, this)?;
    let f = callback(agent, args, name)?;
    let index = |i: f64| if right { length - 1.0 - i } else { i };
    let mut i = 0.0;
    let mut accumulator = match args.get(1) {
        Some(&initial) => initial,
        None => loop {
            if i >= length {
                return agent.type_error(&format!(
                    "Array.prototype.{name} of an empty array with no initial value"
                ));
            }
            let k = index(i);
            i += 1.0;
            if has_index(agent, o, k)? {
                break get_index(agent, o, k)?;
            }
        },
    };
    while i < length {
        let k = index(i);
        if has_index(agent, o, k)? {
            let value = get_index(agent, o, k)?;
            accumulator = agent.call(
                f,
                Value::UNDEFINED,
                &[accumulator, value, Value::number(k), Value::object(o)],
            )?;
        }
        i += 1.0;
    }
    Ok(accumulator)
}

fn reduce(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    reduce_impl(agent, this, args, false)
}

fn reduce_right(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    reduce_impl(agent, this, args, true)
}

// ---- searching ----------------------------------------------------------------

/// `Array.prototype.at` (§23.1.3.1).
fn at(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    let relative = to_integer(agent, arg(args, 0))?;
    let k = if relative >= 0.0 {
        relative
    } else {
        length + relative
    };
    if k < 0.0 || k >= length {
        return Ok(Value::UNDEFINED);
    }
    get_index(agent, o, k)
}

/// The start index of `indexOf`/`includes`: `fromIndex` resolved
/// against `length`, or `None` when the search is empty.
fn from_index(agent: &mut Agent, value: Value, length: f64) -> JsResult<Option<f64>> {
    let n = to_integer(agent, value)?;
    if n == f64::INFINITY || n >= length {
        return Ok(None);
    }
    Ok(Some(if n >= 0.0 { n } else { (length + n).max(0.0) }))
}

/// `Array.prototype.indexOf` (§23.1.3.17).
fn index_of(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    if length == 0.0 {
        return Ok(Value::number(-1.0));
    }
    let Some(mut k) = from_index(agent, arg(args, 1), length)? else {
        return Ok(Value::number(-1.0));
    };
    let target = arg(args, 0);
    while k < length {
        if has_index(agent, o, k)? {
            let value = get_index(agent, o, k)?;
            if agent.is_strictly_equal(value, target) {
                return Ok(Value::number(k));
            }
        }
        k += 1.0;
    }
    Ok(Value::number(-1.0))
}

/// `Array.prototype.includes` (§23.1.3.16): SameValueZero, holes read
/// as undefined.
fn includes(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    if length == 0.0 {
        return Ok(Value::FALSE);
    }
    let Some(mut k) = from_index(agent, arg(args, 1), length)? else {
        return Ok(Value::FALSE);
    };
    let target = arg(args, 0);
    while k < length {
        let value = get_index(agent, o, k)?;
        if agent.same_value_zero(value, target) {
            return Ok(Value::TRUE);
        }
        k += 1.0;
    }
    Ok(Value::FALSE)
}

/// `Array.prototype.lastIndexOf` (§23.1.3.20).
fn last_index_of(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    if length == 0.0 {
        return Ok(Value::number(-1.0));
    }
    let n = if args.len() > 1 {
        to_integer(agent, args[1])?
    } else {
        length - 1.0
    };
    if n == f64::NEG_INFINITY {
        return Ok(Value::number(-1.0));
    }
    let mut k = if n >= 0.0 {
        n.min(length - 1.0)
    } else {
        length + n
    };
    let target = arg(args, 0);
    while k >= 0.0 {
        if has_index(agent, o, k)? {
            let value = get_index(agent, o, k)?;
            if agent.is_strictly_equal(value, target) {
                return Ok(Value::number(k));
            }
        }
        k -= 1.0;
    }
    Ok(Value::number(-1.0))
}

// ---- mutation -------------------------------------------------------------------

/// `Array.prototype.push` (§23.1.3.23).
fn push(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    if length + args.len() as f64 > MAX_SAFE_INTEGER {
        return agent.type_error("Array.prototype.push: length exceeds 2^53 - 1");
    }
    let mut n = length;
    for &value in args {
        set_index(agent, o, n, value)?;
        n += 1.0;
    }
    set_length(agent, o, n)?;
    Ok(Value::number(n))
}

/// `Array.prototype.pop` (§23.1.3.22).
fn pop(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    if length == 0.0 {
        set_length(agent, o, 0.0)?;
        return Ok(Value::UNDEFINED);
    }
    let index = length - 1.0;
    let element = get_index(agent, o, index)?;
    delete_index(agent, o, index)?;
    set_length(agent, o, index)?;
    Ok(element)
}

/// `Array.prototype.shift` (§23.1.3.27).
fn shift(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    if length == 0.0 {
        set_length(agent, o, 0.0)?;
        return Ok(Value::UNDEFINED);
    }
    let first = get_index(agent, o, 0.0)?;
    move_elements(agent, o, 1.0, 0.0, length - 1.0)?;
    delete_index(agent, o, length - 1.0)?;
    set_length(agent, o, length - 1.0)?;
    Ok(first)
}

/// Moves `count` elements from `from` to `to` one at a time, holes
/// included, walking in the direction that does not overwrite unread
/// elements.
fn move_elements(agent: &mut Agent, o: ObjectId, from: f64, to: f64, count: f64) -> JsResult<()> {
    let mut i = 0.0;
    while i < count {
        let (source, target) = if from >= to {
            (from + i, to + i)
        } else {
            (from + count - 1.0 - i, to + count - 1.0 - i)
        };
        if has_index(agent, o, source)? {
            let value = get_index(agent, o, source)?;
            set_index(agent, o, target, value)?;
        } else {
            delete_index(agent, o, target)?;
        }
        i += 1.0;
    }
    Ok(())
}

/// `Array.prototype.unshift` (§23.1.3.38).
fn unshift(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    let count = args.len() as f64;
    if count > 0.0 {
        if length + count > MAX_SAFE_INTEGER {
            return agent.type_error("Array.prototype.unshift: length exceeds 2^53 - 1");
        }
        move_elements(agent, o, 0.0, count, length)?;
        for (j, &value) in args.iter().enumerate() {
            set_index(agent, o, j as f64, value)?;
        }
    }
    set_length(agent, o, length + count)?;
    Ok(Value::number(length + count))
}

/// `Array.prototype.splice` (§23.1.3.31).
fn splice(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    let start = relative_index(agent, arg(args, 0), length, 0.0)?;
    let items = args.get(2..).unwrap_or(&[]);
    let delete_count = match args.len() {
        0 => 0.0,
        1 => length - start,
        _ => to_integer(agent, args[1])?.clamp(0.0, length - start),
    };
    let item_count = items.len() as f64;
    if length + item_count - delete_count > MAX_SAFE_INTEGER {
        return agent.type_error("Array.prototype.splice: length exceeds 2^53 - 1");
    }
    let removed = array_species_create(agent, o, delete_count)?;
    let mut k = 0.0;
    while k < delete_count {
        if has_index(agent, o, start + k)? {
            let value = get_index(agent, o, start + k)?;
            create_index(agent, removed, k, value)?;
        }
        k += 1.0;
    }
    set_length(agent, removed, delete_count)?;
    let tail = length - start - delete_count;
    if item_count < delete_count {
        move_elements(agent, o, start + delete_count, start + item_count, tail)?;
        let mut k = length;
        while k > length - delete_count + item_count {
            delete_index(agent, o, k - 1.0)?;
            k -= 1.0;
        }
    } else if item_count > delete_count {
        move_elements(agent, o, start + delete_count, start + item_count, tail)?;
    }
    for (j, &value) in items.iter().enumerate() {
        set_index(agent, o, start + j as f64, value)?;
    }
    set_length(agent, o, length - delete_count + item_count)?;
    Ok(Value::object(removed))
}

/// `Array.prototype.reverse` (§23.1.3.26).
fn reverse(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    let middle = (length / 2.0).floor();
    let mut lower = 0.0;
    while lower != middle {
        let upper = length - lower - 1.0;
        let lower_exists = has_index(agent, o, lower)?;
        let lower_value = if lower_exists {
            get_index(agent, o, lower)?
        } else {
            Value::UNDEFINED
        };
        let upper_exists = has_index(agent, o, upper)?;
        let upper_value = if upper_exists {
            get_index(agent, o, upper)?
        } else {
            Value::UNDEFINED
        };
        match (lower_exists, upper_exists) {
            (true, true) => {
                set_index(agent, o, lower, upper_value)?;
                set_index(agent, o, upper, lower_value)?;
            }
            (false, true) => {
                set_index(agent, o, lower, upper_value)?;
                delete_index(agent, o, upper)?;
            }
            (true, false) => {
                delete_index(agent, o, lower)?;
                set_index(agent, o, upper, lower_value)?;
            }
            (false, false) => {}
        }
        lower += 1.0;
    }
    Ok(Value::object(o))
}

/// `Array.prototype.fill` (§23.1.3.7).
fn fill(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    let value = arg(args, 0);
    let mut k = relative_index(agent, arg(args, 1), length, 0.0)?;
    let end = relative_index(agent, arg(args, 2), length, length)?;
    while k < end {
        set_index(agent, o, k, value)?;
        k += 1.0;
    }
    Ok(Value::object(o))
}

/// `Array.prototype.copyWithin` (§23.1.3.4).
fn copy_within(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    let to = relative_index(agent, arg(args, 0), length, 0.0)?;
    let from = relative_index(agent, arg(args, 1), length, 0.0)?;
    let end = relative_index(agent, arg(args, 2), length, length)?;
    let count = (end - from).min(length - to);
    if count > 0.0 {
        move_elements(agent, o, from, to, count)?;
    }
    Ok(Value::object(o))
}

// ---- copying ------------------------------------------------------------------

/// `Array.prototype.slice` (§23.1.3.28).
fn slice(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    let mut k = relative_index(agent, arg(args, 0), length, 0.0)?;
    let end = relative_index(agent, arg(args, 1), length, length)?;
    let count = (end - k).max(0.0);
    let a = array_species_create(agent, o, count)?;
    let mut n = 0.0;
    while k < end {
        if has_index(agent, o, k)? {
            let value = get_index(agent, o, k)?;
            create_index(agent, a, n, value)?;
        }
        k += 1.0;
        n += 1.0;
    }
    set_length(agent, a, n)?;
    Ok(Value::object(a))
}

/// `IsConcatSpreadable(O)` (§23.1.3.2.1).
fn is_concat_spreadable(agent: &mut Agent, value: Value) -> JsResult<bool> {
    let Some(o) = value.as_object() else {
        return Ok(false);
    };
    let key = PropertyKey::Symbol(agent.symbols.is_concat_spreadable);
    let spreadable = agent.get(o, key)?;
    if spreadable != Value::UNDEFINED {
        return Ok(agent.to_boolean(spreadable));
    }
    Ok(is_array(agent, value))
}

/// `Array.prototype.concat` (§23.1.3.2).
fn concat(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = agent.to_object(this)?;
    let a = array_species_create(agent, o, 0.0)?;
    let mut n = 0.0;
    for item in std::iter::once(Value::object(o)).chain(args.iter().copied()) {
        if is_concat_spreadable(agent, item)? {
            let e = item.as_object().expect("spreadable values are objects");
            let length = agent.length_of_array_like(e)?;
            if n + length > MAX_SAFE_INTEGER {
                return agent.type_error("Array.prototype.concat: length exceeds 2^53 - 1");
            }
            let mut k = 0.0;
            while k < length {
                if has_index(agent, e, k)? {
                    let value = get_index(agent, e, k)?;
                    create_index(agent, a, n, value)?;
                }
                n += 1.0;
                k += 1.0;
            }
        } else {
            if n >= MAX_SAFE_INTEGER {
                return agent.type_error("Array.prototype.concat: length exceeds 2^53 - 1");
            }
            create_index(agent, a, n, item)?;
            n += 1.0;
        }
    }
    set_length(agent, a, n)?;
    Ok(Value::object(a))
}

/// `FlattenIntoArray` (§23.1.3.13.1) with an explicit stack, so deeply
/// nested arrays cannot overflow the native stack. Returns the next
/// target index.
fn flatten_into_array(
    agent: &mut Agent,
    target: ObjectId,
    source: ObjectId,
    source_length: f64,
    depth: f64,
    mapper: Option<(Value, Value)>,
) -> JsResult<f64> {
    struct Level {
        source: ObjectId,
        length: f64,
        index: f64,
        depth: f64,
    }
    let mut n = 0.0;
    let mut stack = vec![Level {
        source,
        length: source_length,
        index: 0.0,
        depth,
    }];
    while let Some(level) = stack.last_mut() {
        if level.index >= level.length {
            stack.pop();
            continue;
        }
        let (source, k, depth) = (level.source, level.index, level.depth);
        level.index += 1.0;
        if !has_index(agent, source, k)? {
            continue;
        }
        let mut element = get_index(agent, source, k)?;
        // Only the top level is mapped (flatMap).
        if stack.len() == 1 {
            if let Some((f, this_arg)) = mapper {
                element = agent.call(
                    f,
                    this_arg,
                    &[element, Value::number(k), Value::object(source)],
                )?;
            }
        }
        if depth > 0.0 && is_array(agent, element) {
            let inner = element.as_object().expect("arrays are objects");
            let length = agent.length_of_array_like(inner)?;
            agent.heap.charge(std::mem::size_of::<Level>())?;
            stack.push(Level {
                source: inner,
                length,
                index: 0.0,
                depth: depth - 1.0,
            });
        } else {
            if n >= MAX_SAFE_INTEGER {
                return agent.type_error("flattened array length exceeds 2^53 - 1");
            }
            create_index(agent, target, n, element)?;
            n += 1.0;
        }
    }
    Ok(n)
}

/// `Array.prototype.flat` (§23.1.3.13).
fn flat(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    let depth = match arg(args, 0) {
        d if d == Value::UNDEFINED => 1.0,
        d => to_integer(agent, d)?.max(0.0),
    };
    let a = array_species_create(agent, o, 0.0)?;
    flatten_into_array(agent, a, o, length, depth, None)?;
    Ok(Value::object(a))
}

/// `Array.prototype.flatMap` (§23.1.3.14).
fn flat_map(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    let f = callback(agent, args, "flatMap")?;
    let a = array_species_create(agent, o, 0.0)?;
    flatten_into_array(agent, a, o, length, 1.0, Some((f, arg(args, 1))))?;
    Ok(Value::object(a))
}

/// `Array.prototype.toReversed` (§23.1.3.33).
fn to_reversed(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    let a = array_create(agent, length, None)?;
    let mut k = 0.0;
    while k < length {
        let value = get_index(agent, o, length - k - 1.0)?;
        create_index(agent, a, k, value)?;
        k += 1.0;
    }
    Ok(Value::object(a))
}

/// `Array.prototype.with` (§23.1.3.39).
fn with(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    let relative = to_integer(agent, arg(args, 0))?;
    let index = if relative >= 0.0 {
        relative
    } else {
        length + relative
    };
    if index >= length || index < 0.0 {
        return range_error(agent, "Array.prototype.with: index out of range");
    }
    let a = array_create(agent, length, None)?;
    let mut k = 0.0;
    while k < length {
        let value = if k == index {
            arg(args, 1)
        } else {
            get_index(agent, o, k)?
        };
        create_index(agent, a, k, value)?;
        k += 1.0;
    }
    Ok(Value::object(a))
}

/// `Array.prototype.toSpliced` (§23.1.3.35).
fn to_spliced(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    let start = relative_index(agent, arg(args, 0), length, 0.0)?;
    let items = args.get(2..).unwrap_or(&[]);
    let skip = match args.len() {
        0 => 0.0,
        1 => length - start,
        _ => to_integer(agent, args[1])?.clamp(0.0, length - start),
    };
    let new_length = length + items.len() as f64 - skip;
    if new_length > MAX_SAFE_INTEGER {
        return agent.type_error("Array.prototype.toSpliced: length exceeds 2^53 - 1");
    }
    let a = array_create(agent, new_length, None)?;
    let mut i = 0.0;
    while i < start {
        let value = get_index(agent, o, i)?;
        create_index(agent, a, i, value)?;
        i += 1.0;
    }
    for &value in items {
        create_index(agent, a, i, value)?;
        i += 1.0;
    }
    let mut r = start + skip;
    while i < new_length {
        let value = get_index(agent, o, r)?;
        create_index(agent, a, i, value)?;
        i += 1.0;
        r += 1.0;
    }
    Ok(Value::object(a))
}

// ---- sorting ------------------------------------------------------------------

/// The comparator argument of `sort`/`toSorted`: undefined or callable.
fn comparator(agent: &mut Agent, args: &[Value], name: &str) -> JsResult<Option<Value>> {
    match arg(args, 0) {
        f if f == Value::UNDEFINED => Ok(None),
        f if agent.is_callable(f) => Ok(Some(f)),
        _ => agent.type_error(&format!(
            "Array.prototype.{name}: comparator must be a function or undefined"
        )),
    }
}

/// `SortIndexedProperties` and `SortCompare` (§23.1.3.30.1, §23.1.3.30.2)
/// on the values read: undefined last, then by comparator or by string
/// order. The sort is stable.
fn sort_values(
    agent: &mut Agent,
    values: Vec<Value>,
    compare: Option<Value>,
) -> JsResult<Vec<Value>> {
    let (mut defined, undefined): (Vec<Value>, Vec<Value>) =
        values.into_iter().partition(|&v| v != Value::UNDEFINED);
    match compare {
        None => {
            // Each value's string is computed once.
            let mut keyed: Vec<(StringId, Value)> = Vec::with_capacity(defined.len());
            for value in defined {
                keyed.push((agent.to_string(value)?, value));
            }
            let heap = &agent.heap;
            keyed.sort_by(|a, b| heap.string(a.0).cmp(heap.string(b.0)));
            defined = keyed.into_iter().map(|(_, v)| v).collect();
        }
        Some(f) => {
            defined = merge_sort(agent, defined, &mut |agent, x, y| {
                let v = agent.call(f, Value::UNDEFINED, &[x, y])?;
                let n = agent.to_number(v)?;
                Ok(if n < 0.0 {
                    Ordering::Less
                } else if n > 0.0 {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                })
            })?;
        }
    }
    defined.extend(undefined);
    Ok(defined)
}

type Compare<'a> = dyn FnMut(&mut Agent, Value, Value) -> JsResult<Ordering> + 'a;

/// A stable bottom-up merge sort whose comparison may throw. A comparator
/// that is not consistent leaves an implementation-defined order, as the
/// specification allows, but never loses or duplicates values.
fn merge_sort(
    agent: &mut Agent,
    mut items: Vec<Value>,
    compare: &mut Compare<'_>,
) -> JsResult<Vec<Value>> {
    let n = items.len();
    if n < 2 {
        return Ok(items);
    }
    agent.heap.charge(8 * n)?;
    let mut buffer = items.clone();
    let mut width = 1;
    while width < n {
        let mut start = 0;
        while start < n {
            let middle = (start + width).min(n);
            let end = (start + 2 * width).min(n);
            let (mut i, mut j, mut k) = (start, middle, start);
            while i < middle && j < end {
                if compare(agent, items[j], items[i])? == Ordering::Less {
                    buffer[k] = items[j];
                    j += 1;
                } else {
                    buffer[k] = items[i];
                    i += 1;
                }
                k += 1;
            }
            buffer[k..k + middle - i].copy_from_slice(&items[i..middle]);
            k += middle - i;
            buffer[k..k + end - j].copy_from_slice(&items[j..end]);
            start = end;
        }
        std::mem::swap(&mut items, &mut buffer);
        width *= 2;
    }
    Ok(items)
}

/// `Array.prototype.sort` (§23.1.3.30): present elements are sorted to
/// the front and holes stay at the end.
fn sort(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let compare = comparator(agent, args, "sort")?;
    let (o, length) = this_array_like(agent, this)?;
    let mut values = Vec::new();
    let mut k = 0.0;
    while k < length {
        if has_index(agent, o, k)? {
            agent.heap.charge(8)?;
            values.push(get_index(agent, o, k)?);
        }
        k += 1.0;
    }
    let sorted = sort_values(agent, values, compare)?;
    let count = sorted.len() as f64;
    for (j, value) in sorted.into_iter().enumerate() {
        set_index(agent, o, j as f64, value)?;
    }
    let mut j = count;
    while j < length {
        delete_index(agent, o, j)?;
        j += 1.0;
    }
    Ok(Value::object(o))
}

/// `Array.prototype.toSorted` (§23.1.3.34): holes read as undefined.
fn to_sorted(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let compare = comparator(agent, args, "toSorted")?;
    let (o, length) = this_array_like(agent, this)?;
    let a = array_create(agent, length, None)?;
    let mut values = Vec::new();
    let mut k = 0.0;
    while k < length {
        agent.heap.charge(8)?;
        values.push(get_index(agent, o, k)?);
        k += 1.0;
    }
    let sorted = sort_values(agent, values, compare)?;
    for (j, value) in sorted.into_iter().enumerate() {
        create_index(agent, a, j as f64, value)?;
    }
    Ok(Value::object(a))
}

// ---- strings ------------------------------------------------------------------

/// `Array.prototype.join` (§23.1.3.18).
fn join(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    let separator: Vec<u16> = match arg(args, 0) {
        s if s == Value::UNDEFINED => vec![u16::from(b',')],
        s => {
            let s = agent.to_string(s)?;
            agent.heap.string(s).to_units()
        }
    };
    join_with(agent, o, length, &separator, |agent, value| {
        let s = agent.to_string(value)?;
        Ok(agent.heap.string(s).to_units())
    })
}

/// The elements of `o` as strings between `separator`s, undefined and
/// null as empty strings. An array that is already being joined further
/// up the stack joins as the empty string, as browsers do, so a cycle
/// ends instead of recursing until the stack overflows.
fn join_with(
    agent: &mut Agent,
    o: ObjectId,
    length: f64,
    separator: &[u16],
    mut text: impl FnMut(&mut Agent, Value) -> JsResult<Vec<u16>>,
) -> JsResult<Value> {
    if agent.vm.join_stack.contains(&o) {
        return Ok(Value::string(agent.names.empty));
    }
    agent.vm.join_stack.push(o);
    let result = (|| {
        let mut units: Vec<u16> = Vec::new();
        let mut k = 0.0;
        while k < length {
            if k > 0.0 {
                agent.heap.charge(2 * separator.len())?;
                units.extend_from_slice(separator);
            }
            let element = get_index(agent, o, k)?;
            if !element.is_nullish() {
                let piece = text(agent, element)?;
                agent.heap.charge(2 * piece.len())?;
                units.extend_from_slice(&piece);
            }
            k += 1.0;
        }
        agent.string_from(JsStr::from_units(&units))
    })();
    agent.vm.join_stack.pop();
    result
}

/// `Array.prototype.toString` (§23.1.3.36).
fn to_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = agent.to_object(this)?;
    let key = agent.heap.key("join")?;
    let f = agent.get(o, key)?;
    if agent.is_callable(f) {
        return agent.call(f, Value::object(o), &[]);
    }
    super::object_to_string(agent, Value::object(o), &[])
}

/// `Array.prototype.toLocaleString` (§23.1.3.32), with no locale
/// support yet: each element's own `toLocaleString`, joined by commas.
fn to_locale_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (o, length) = this_array_like(agent, this)?;
    let key = agent.heap.key("toLocaleString")?;
    join_with(agent, o, length, &[u16::from(b',')], |agent, value| {
        let v = agent.invoke(value, key, &[])?;
        let s = agent.to_string(v)?;
        Ok(agent.heap.string(s).to_units())
    })
}
