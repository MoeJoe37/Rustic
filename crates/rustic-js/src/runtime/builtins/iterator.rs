//! `Iterator` (§27.1.3) and the iterator helpers of `Iterator.prototype`
//! (§27.1.4): `map`, `filter`, `take`, `drop`, `flatMap`, `reduce`,
//! `toArray`, `forEach`, `some`, `every`, `find`, with `Iterator.from` and
//! `Iterator.concat`.
//!
//! The specification writes each lazy helper as an abstract closure run
//! as a generator. Here a helper object keeps that closure's variables in
//! a [`Helper`] and each `next` runs one step of it, so a helper costs
//! one object and one small box, with no frame or stack of its own.

use super::{Method, arg, install_methods, iterator_result, method, prototype_from_new_target};
use crate::runtime::ErrorKind;
use crate::runtime::agent::{Abrupt, Agent, JsResult};
use crate::runtime::number::to_integer_or_infinity;
use crate::runtime::object::{
    Class, Helper, HelperKind, HelperState, Internal, Object, PropertyDescriptor,
};
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::{ObjectId, Value};

const STATIC_METHODS: &[Method] = &[
    method("concat", iterator_concat, 0),
    method("from", iterator_from, 1),
];

const PROTOTYPE_METHODS: &[Method] = &[
    method("drop", drop, 1),
    method("every", every, 1),
    method("filter", filter, 1),
    method("find", find, 1),
    method("flatMap", flat_map, 1),
    method("forEach", for_each, 1),
    method("map", map, 1),
    method("reduce", reduce, 1),
    method("some", some, 1),
    method("take", take, 1),
    method("toArray", to_array, 0),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let iterator_prototype = agent
        .get_prototype_of(agent.intrinsics.array_iterator_prototype)?
        .expect("%ArrayIteratorPrototype% inherits from %IteratorPrototype%");
    let iterator = super::install_constructor(
        agent,
        "Iterator",
        iterator_constructor,
        0,
        iterator_prototype,
    )?;
    agent.intrinsics.iterator = iterator;
    install_methods(agent, iterator, STATIC_METHODS)?;
    install_methods(agent, iterator_prototype, PROTOTYPE_METHODS)?;

    // `constructor` and `@@toStringTag` are accessors whose setters write
    // an own property on the receiver (§27.1.4.1, §27.1.4.14), so that
    // assigning them on an iterator does not hit the prototype.
    let constructor_key = PropertyKey::String(agent.names.constructor);
    let tag_key = PropertyKey::Symbol(agent.symbols.to_string_tag);
    for (key, getter, setter, name) in [
        (
            constructor_key,
            get_constructor as crate::runtime::NativeFn,
            set_constructor as crate::runtime::NativeFn,
            "constructor",
        ),
        (
            tag_key,
            get_to_string_tag,
            set_to_string_tag,
            "[Symbol.toStringTag]",
        ),
    ] {
        let get = agent.new_native_function(getter, &format!("get {name}"), 0)?;
        let set = agent.new_native_function(setter, &format!("set {name}"), 1)?;
        agent.define_property_or_throw(
            iterator_prototype,
            key,
            PropertyDescriptor::accessor(Value::object(get), Value::object(set), false, true),
        )?;
    }

    let helper_prototype = agent.new_object_with(Some(iterator_prototype))?;
    install_methods(
        agent,
        helper_prototype,
        &[
            method("next", helper_next, 0),
            method("return", helper_return, 0),
        ],
    )?;
    let tag = agent.string("Iterator Helper")?;
    agent.define_property_or_throw(
        helper_prototype,
        tag_key,
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    let wrap_prototype = agent.new_object_with(Some(iterator_prototype))?;
    install_methods(
        agent,
        wrap_prototype,
        &[
            method("next", wrap_next, 0),
            method("return", wrap_return, 0),
        ],
    )?;
    agent.intrinsics.iterator_helper_prototypes = [helper_prototype, wrap_prototype];
    Ok(())
}

// ---- iterator records ------------------------------------------------------

/// `GetIteratorDirect(obj)` (§7.4.4): the iterator and its `next`.
fn direct(agent: &mut Agent, iterator: ObjectId) -> JsResult<(Value, Value)> {
    let next = agent.get(iterator, PropertyKey::String(agent.names.next))?;
    Ok((Value::object(iterator), next))
}

/// `IteratorStepValue` (§7.4.10): the next value, or `None` when done.
fn step_value(agent: &mut Agent, iterator: Value, next: Value) -> JsResult<Option<Value>> {
    let result = agent.call(next, iterator, &[])?;
    let Some(result) = result.as_object() else {
        return agent.type_error("iterator result is not an object");
    };
    let done = agent.get(result, PropertyKey::String(agent.names.done))?;
    if agent.to_boolean(done) {
        return Ok(None);
    }
    agent
        .get(result, PropertyKey::String(agent.names.value))
        .map(Some)
}

/// `IteratorClose(iterator, completion)` for a throw completion: calls
/// `return` and keeps the original error whatever it does.
fn close_with(agent: &mut Agent, iterator: Value, error: Abrupt) -> Abrupt {
    if matches!(error, Abrupt::Throw(_)) {
        let key = PropertyKey::String(agent.names.return_);
        if let Ok(Some(close)) = agent.get_method(iterator, key) {
            let _ = agent.call(close, iterator, &[]);
        }
    }
    error
}

/// `IteratorClose(iterator, completion)` for a normal or return
/// completion.
fn close(agent: &mut Agent, iterator: Value) -> JsResult<()> {
    let key = PropertyKey::String(agent.names.return_);
    let Some(method) = agent.get_method(iterator, key)? else {
        return Ok(());
    };
    let result = agent.call(method, iterator, &[])?;
    if result.as_object().is_none() {
        return agent.type_error("iterator result is not an object");
    }
    Ok(())
}

/// `IfAbruptCloseIterator(value, iterator)`.
fn or_close<T>(agent: &mut Agent, iterator: Value, value: JsResult<T>) -> JsResult<T> {
    value.map_err(|e| close_with(agent, iterator, e))
}

/// `GetIteratorFlattenable(obj, primitiveHandling)` (§7.4.13).
fn flattenable(agent: &mut Agent, value: Value, strings: bool) -> JsResult<(Value, Value)> {
    if value.as_object().is_none() && !(strings && value.as_string().is_some()) {
        return agent.type_error("value is not an iterator or iterable");
    }
    let key = PropertyKey::Symbol(agent.symbols.iterator);
    let iterator = match agent.get_method(value, key)? {
        None => value,
        Some(method) => agent.call(method, value, &[])?,
    };
    let Some(iterator) = iterator.as_object() else {
        return agent.type_error("iterator is not an object");
    };
    direct(agent, iterator)
}

/// The receiver of an `Iterator.prototype` method as an iterator record,
/// after checking `function` is callable (closing the receiver if not).
fn receiver(
    agent: &mut Agent,
    this: Value,
    function: Value,
    name: &str,
) -> JsResult<(Value, Value)> {
    let Some(o) = this.as_object() else {
        return agent.type_error(&format!("Iterator.prototype.{name} called on a non-object"));
    };
    if !agent.is_callable(function) {
        let error = agent.throw(
            ErrorKind::TypeError,
            &format!("Iterator.prototype.{name}: the argument is not a function"),
        );
        return Err(close_with(agent, this, error));
    }
    direct(agent, o)
}

// ---- the constructor and statics -----------------------------------------

/// `Iterator()` (§27.1.3.1): abstract, so only subclasses construct it.
fn iterator_constructor(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let active = agent.active_function();
    match agent.new_target() {
        None => agent.type_error("Constructor Iterator requires 'new'"),
        Some(t) if Some(t) == active => agent.type_error("Iterator is an abstract class"),
        Some(_) => {
            let default = agent
                .get_prototype_of(agent.intrinsics.array_iterator_prototype)?
                .expect("%IteratorPrototype% exists");
            let prototype = prototype_from_new_target(agent, default)?;
            Ok(Value::object(agent.new_object_with(Some(prototype))?))
        }
    }
}

/// `Iterator.from(O)` (§27.1.3.2.1).
fn iterator_from(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let (iterator, next) = flattenable(agent, arg(args, 0), true)?;
    let constructor = Value::object(agent.intrinsics.iterator);
    if agent.ordinary_has_instance(constructor, iterator)? {
        return Ok(iterator);
    }
    let prototype = agent.intrinsics.iterator_helper_prototypes[1];
    let mut object = Object::new(Class::Ordinary, Some(prototype));
    object.extra_mut().internal = Some(Internal::Wrapped { iterator, next });
    agent
        .heap
        .charge(std::mem::size_of::<crate::runtime::object::Extra>())?;
    Ok(Value::object(agent.heap.alloc_object(object)?))
}

fn wrapped(agent: &mut Agent, this: Value) -> JsResult<(Value, Value)> {
    if let Some(o) = this.as_object() {
        if let Some(Internal::Wrapped { iterator, next }) = agent.heap.object(o).internal() {
            return Ok((*iterator, *next));
        }
    }
    agent.type_error("not an iterator from Iterator.from")
}

/// `%WrapForValidIteratorPrototype%.next()` (§27.1.3.2.1.1.1).
fn wrap_next(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (iterator, next) = wrapped(agent, this)?;
    agent.call(next, iterator, &[])
}

/// `%WrapForValidIteratorPrototype%.return()` (§27.1.3.2.1.1.2).
fn wrap_return(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (iterator, _) = wrapped(agent, this)?;
    let key = PropertyKey::String(agent.names.return_);
    match agent.get_method(iterator, key)? {
        None => iterator_result(agent, Value::UNDEFINED, true),
        Some(method) => agent.call(method, iterator, &[]),
    }
}

/// `Iterator.concat(...items)` (iterator sequencing, ES2026).
fn iterator_concat(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let mut items = Vec::with_capacity(args.len() * 2);
    for &item in args {
        if item.as_object().is_none() {
            return agent.type_error("Iterator.concat: an item is not an object");
        }
        let key = PropertyKey::Symbol(agent.symbols.iterator);
        let Some(method) = agent.get_method(item, key)? else {
            return agent.type_error("Iterator.concat: an item is not iterable");
        };
        items.push(item);
        items.push(method);
    }
    agent.heap.charge(8 * items.len())?;
    new_helper(
        agent,
        HelperKind::Concat,
        (Value::UNDEFINED, Value::UNDEFINED),
        Value::UNDEFINED,
        0.0,
        items.into_boxed_slice(),
    )
}

fn get_constructor(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::object(agent.intrinsics.iterator))
}

fn set_constructor(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let key = PropertyKey::String(agent.names.constructor);
    ignore_prototype_setter(agent, this, key, arg(args, 0))
}

fn get_to_string_tag(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    agent.string("Iterator")
}

fn set_to_string_tag(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let key = PropertyKey::Symbol(agent.symbols.to_string_tag);
    ignore_prototype_setter(agent, this, key, arg(args, 0))
}

/// `SetterThatIgnoresPrototypeProperties(this, %Iterator.prototype%, p, v)`
/// (§27.1.4.14.1).
fn ignore_prototype_setter(
    agent: &mut Agent,
    this: Value,
    key: PropertyKey,
    value: Value,
) -> JsResult<Value> {
    let Some(o) = this.as_object() else {
        return agent.type_error("setter called on a non-object");
    };
    let home = agent
        .get_prototype_of(agent.intrinsics.array_iterator_prototype)?
        .expect("%IteratorPrototype% exists");
    if o == home {
        return agent.type_error("cannot set this property on Iterator.prototype");
    }
    if agent.get_own_property(o, key)?.is_none() {
        agent.create_data_property_or_throw(o, key, value)?;
    } else {
        agent.set(o, key, value, true)?;
    }
    Ok(Value::UNDEFINED)
}

// ---- lazy helpers ----------------------------------------------------------

fn new_helper(
    agent: &mut Agent,
    kind: HelperKind,
    (iterator, next): (Value, Value),
    function: Value,
    count: f64,
    items: Box<[Value]>,
) -> JsResult<Value> {
    let prototype = agent.intrinsics.iterator_helper_prototypes[0];
    let mut object = Object::new(Class::Ordinary, Some(prototype));
    object.extra_mut().internal = Some(Internal::Helper(Box::new(Helper {
        kind,
        state: HelperState::Start,
        iterator,
        next,
        function,
        count,
        inner: Value::UNDEFINED,
        inner_next: Value::UNDEFINED,
        items,
    })));
    agent.heap.charge(
        std::mem::size_of::<crate::runtime::object::Extra>() + std::mem::size_of::<Helper>(),
    )?;
    Ok(Value::object(agent.heap.alloc_object(object)?))
}

fn map(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    let record = receiver(agent, this, f, "map")?;
    new_helper(agent, HelperKind::Map, record, f, 0.0, Box::new([]))
}

fn filter(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    let record = receiver(agent, this, f, "filter")?;
    new_helper(agent, HelperKind::Filter, record, f, 0.0, Box::new([]))
}

fn flat_map(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    let record = receiver(agent, this, f, "flatMap")?;
    new_helper(agent, HelperKind::FlatMap, record, f, 0.0, Box::new([]))
}

fn take(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    limited(agent, this, args, HelperKind::Take, "take")
}

fn drop(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    limited(agent, this, args, HelperKind::Drop, "drop")
}

/// `take(limit)` and `drop(limit)` (§27.1.4.12, §27.1.4.3).
fn limited(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
    kind: HelperKind,
    name: &str,
) -> JsResult<Value> {
    let Some(o) = this.as_object() else {
        return agent.type_error(&format!("Iterator.prototype.{name} called on a non-object"));
    };
    let limit = agent.to_number(arg(args, 0));
    let limit = or_close(agent, this, limit)?;
    let too_large = limit.is_finite() && limit > 9_007_199_254_740_991.0;
    if limit.is_nan() || too_large || to_integer_or_infinity(limit) < 0.0 {
        let error = agent.throw(
            ErrorKind::RangeError,
            &format!("Iterator.prototype.{name}: the limit must be a non-negative number"),
        );
        return Err(close_with(agent, this, error));
    }
    let record = direct(agent, o)?;
    let limit = to_integer_or_infinity(limit);
    new_helper(agent, kind, record, Value::UNDEFINED, limit, Box::new([]))
}

/// The helper of `this`, taken out of its object and marked running, so
/// a re-entrant `next` or `return` sees a running generator.
fn take_helper(agent: &mut Agent, this: Value) -> JsResult<(ObjectId, Helper)> {
    if let Some(o) = this.as_object() {
        if let Some(Internal::Helper(h)) = agent.heap.object_mut(o).internal_mut() {
            if h.state == HelperState::Running {
                return agent.type_error("the iterator helper is already running");
            }
            let running = Helper {
                kind: h.kind,
                state: HelperState::Running,
                iterator: Value::UNDEFINED,
                next: Value::UNDEFINED,
                function: Value::UNDEFINED,
                count: 0.0,
                inner: Value::UNDEFINED,
                inner_next: Value::UNDEFINED,
                items: Box::new([]),
            };
            return Ok((o, std::mem::replace(&mut **h, running)));
        }
    }
    agent.type_error("not an iterator helper")
}

fn put_helper(agent: &mut Agent, o: ObjectId, helper: Helper) {
    if let Some(Internal::Helper(h)) = agent.heap.object_mut(o).internal_mut() {
        **h = helper;
    }
}

/// `%IteratorHelperPrototype%.next()` (§27.1.2.1.1).
fn helper_next(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (o, mut h) = take_helper(agent, this)?;
    let step = if h.state == HelperState::Done {
        Ok(None)
    } else {
        step(agent, &mut h)
    };
    h.state = match step {
        Ok(Some(_)) => HelperState::Yielded,
        _ => HelperState::Done,
    };
    put_helper(agent, o, h);
    match step? {
        Some(value) => iterator_result(agent, value, false),
        None => iterator_result(agent, Value::UNDEFINED, true),
    }
}

/// `%IteratorHelperPrototype%.return()` (§27.1.2.1.2).
fn helper_return(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (o, mut h) = take_helper(agent, this)?;
    let state = h.state;
    let (iterator, inner) = (h.iterator, h.inner);
    h.state = HelperState::Done;
    match state {
        HelperState::Start => {
            // The generator completes before the iterator is closed.
            put_helper(agent, o, h);
            if iterator != Value::UNDEFINED {
                close(agent, iterator)?;
            }
        }
        HelperState::Yielded => {
            // A return completion at the yield runs the closure's
            // IfAbruptCloseIterator steps: the inner iterator closes
            // first, then the underlying one (`Iterator.concat` has none).
            let closed = if inner == Value::UNDEFINED {
                close(agent, iterator)
            } else {
                match close(agent, inner) {
                    Err(e) if h.kind == HelperKind::FlatMap => Err(close_with(agent, iterator, e)),
                    Ok(()) if h.kind == HelperKind::FlatMap => close(agent, iterator),
                    other => other,
                }
            };
            put_helper(agent, o, h);
            closed?;
        }
        HelperState::Done | HelperState::Running => put_helper(agent, o, h),
    }
    iterator_result(agent, Value::UNDEFINED, true)
}

/// One step of a helper's closure: the value it yields next, or `None`
/// when it returns.
fn step(agent: &mut Agent, h: &mut Helper) -> JsResult<Option<Value>> {
    match h.kind {
        HelperKind::Map => {
            let Some(value) = step_value(agent, h.iterator, h.next)? else {
                return Ok(None);
            };
            let mapped = agent.call(
                h.function,
                Value::UNDEFINED,
                &[value, Value::number(h.count)],
            );
            let mapped = or_close(agent, h.iterator, mapped)?;
            h.count += 1.0;
            Ok(Some(mapped))
        }
        HelperKind::Filter => loop {
            let Some(value) = step_value(agent, h.iterator, h.next)? else {
                return Ok(None);
            };
            let selected = agent.call(
                h.function,
                Value::UNDEFINED,
                &[value, Value::number(h.count)],
            );
            let selected = or_close(agent, h.iterator, selected)?;
            h.count += 1.0;
            if agent.to_boolean(selected) {
                return Ok(Some(value));
            }
        },
        HelperKind::Take => {
            if h.count == 0.0 {
                close(agent, h.iterator)?;
                return Ok(None);
            }
            if h.count != f64::INFINITY {
                h.count -= 1.0;
            }
            step_value(agent, h.iterator, h.next)
        }
        HelperKind::Drop => {
            while h.count > 0.0 {
                if h.count != f64::INFINITY {
                    h.count -= 1.0;
                }
                if step_value(agent, h.iterator, h.next)?.is_none() {
                    return Ok(None);
                }
            }
            step_value(agent, h.iterator, h.next)
        }
        HelperKind::FlatMap => loop {
            if h.inner != Value::UNDEFINED {
                let inner = step_value(agent, h.inner, h.inner_next);
                match or_close(agent, h.iterator, inner)? {
                    Some(value) => return Ok(Some(value)),
                    None => {
                        h.inner = Value::UNDEFINED;
                        h.inner_next = Value::UNDEFINED;
                        h.count += 1.0;
                    }
                }
            }
            let Some(value) = step_value(agent, h.iterator, h.next)? else {
                return Ok(None);
            };
            let mapped = agent.call(
                h.function,
                Value::UNDEFINED,
                &[value, Value::number(h.count)],
            );
            let mapped = or_close(agent, h.iterator, mapped)?;
            let inner = flattenable(agent, mapped, false);
            let (inner, inner_next) = or_close(agent, h.iterator, inner)?;
            h.inner = inner;
            h.inner_next = inner_next;
        },
        HelperKind::Concat => loop {
            if h.inner != Value::UNDEFINED {
                match step_value(agent, h.inner, h.inner_next)? {
                    Some(value) => return Ok(Some(value)),
                    None => {
                        h.inner = Value::UNDEFINED;
                        h.inner_next = Value::UNDEFINED;
                    }
                }
            }
            // The item index is far below 2⁵³.
            let i = h.count as usize;
            if 2 * i >= h.items.len() {
                return Ok(None);
            }
            h.count += 1.0;
            let iterator = agent.call(h.items[2 * i + 1], h.items[2 * i], &[])?;
            let Some(iterator) = iterator.as_object() else {
                return agent.type_error("Iterator.concat: the iterator is not an object");
            };
            let (inner, inner_next) = direct(agent, iterator)?;
            h.inner = inner;
            h.inner_next = inner_next;
        },
    }
}

// ---- eager methods ---------------------------------------------------------

/// Calls `each(value, counter)` for every value of the receiver until it
/// returns `Some`, then closes the receiver and returns that result.
fn consume(
    agent: &mut Agent,
    this: Value,
    function: Value,
    name: &str,
    mut each: impl FnMut(&mut Agent, Value, f64) -> JsResult<Option<Value>>,
) -> JsResult<Option<Value>> {
    let (iterator, next) = receiver(agent, this, function, name)?;
    let mut counter = 0.0;
    while let Some(value) = step_value(agent, iterator, next)? {
        let result = each(agent, value, counter);
        if let Some(result) = or_close(agent, iterator, result)? {
            close(agent, iterator)?;
            return Ok(Some(result));
        }
        counter += 1.0;
    }
    Ok(None)
}

fn for_each(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    consume(agent, this, f, "forEach", |agent, value, counter| {
        agent.call(f, Value::UNDEFINED, &[value, Value::number(counter)])?;
        Ok(None)
    })?;
    Ok(Value::UNDEFINED)
}

fn some(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    let found = consume(agent, this, f, "some", |agent, value, counter| {
        let r = agent.call(f, Value::UNDEFINED, &[value, Value::number(counter)])?;
        Ok(agent.to_boolean(r).then_some(Value::TRUE))
    })?;
    Ok(Value::boolean(found.is_some()))
}

fn every(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    let failed = consume(agent, this, f, "every", |agent, value, counter| {
        let r = agent.call(f, Value::UNDEFINED, &[value, Value::number(counter)])?;
        Ok((!agent.to_boolean(r)).then_some(Value::FALSE))
    })?;
    Ok(Value::boolean(failed.is_none()))
}

fn find(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    let found = consume(agent, this, f, "find", |agent, value, counter| {
        let r = agent.call(f, Value::UNDEFINED, &[value, Value::number(counter)])?;
        Ok(agent.to_boolean(r).then_some(value))
    })?;
    Ok(found.unwrap_or(Value::UNDEFINED))
}

fn reduce(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    let (iterator, next) = receiver(agent, this, f, "reduce")?;
    let (mut accumulator, mut counter) = if args.len() >= 2 {
        (args[1], 0.0)
    } else {
        match step_value(agent, iterator, next)? {
            Some(first) => (first, 1.0),
            None => return agent.type_error("Reduce of an empty iterator with no initial value"),
        }
    };
    while let Some(value) = step_value(agent, iterator, next)? {
        let result = agent.call(
            f,
            Value::UNDEFINED,
            &[accumulator, value, Value::number(counter)],
        );
        accumulator = or_close(agent, iterator, result)?;
        counter += 1.0;
    }
    Ok(accumulator)
}

fn to_array(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let Some(o) = this.as_object() else {
        return agent.type_error("Iterator.prototype.toArray called on a non-object");
    };
    let (iterator, next) = direct(agent, o)?;
    let mut values = Vec::new();
    // `create_array_from_list` charges the elements once they are known.
    while let Some(value) = step_value(agent, iterator, next)? {
        values.push(value);
    }
    Ok(Value::object(agent.create_array_from_list(&values)?))
}
