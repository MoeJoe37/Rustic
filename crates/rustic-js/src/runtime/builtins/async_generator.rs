//! Async iteration: `%AsyncIteratorPrototype%` (§27.1.3),
//! `%AsyncFromSyncIteratorPrototype%` (§27.1.6), async generator objects
//! (§27.6) and `%AsyncGeneratorFunction%` (§27.4).
//!
//! An async generator holds its suspended frame and its queue of
//! requests in a [`Generator`](crate::runtime::object::Generator); the
//! methods here queue a request and, when the generator is suspended,
//! hand over to the interpreter's driver ([`Agent::async_generator_run`]).

use super::promise::{Capability, captures, closure, dynamic_function, me, perform_then};
use super::{Method, arg, install_methods, iterator_result, method};
use crate::runtime::agent::{Abrupt, Agent, JsResult};
use crate::runtime::object::{
    AsyncRequest, Class, GeneratorState, Internal, Object, PropertyDescriptor,
};
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::{ObjectId, Value};
use crate::runtime::{
    Extra, FLAG_NATIVE_CONSTRUCTOR, RESUME_RETURN, RESUME_THROW, new_intrinsic_promise,
    reject_promise, resolve_promise,
};

const RESUME_NEXT: u8 = 0;

const GENERATOR_METHODS: &[Method] = &[
    method("next", async_generator_next, 1),
    method("return", async_generator_return, 1),
    method("throw", async_generator_throw, 1),
];

const FROM_SYNC_METHODS: &[Method] = &[
    method("next", from_sync_next, 1),
    method("return", from_sync_return, 1),
    method("throw", from_sync_throw, 1),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let function_prototype = agent.intrinsics.function_prototype;
    let constructor_key = PropertyKey::String(agent.names.constructor);
    let prototype_key = PropertyKey::String(agent.names.prototype);
    let tag_key = PropertyKey::Symbol(agent.symbols.to_string_tag);

    // %AsyncIteratorPrototype% (§27.1.3).
    let async_iterator_prototype = agent.new_object()?;
    let f = agent.new_native_function(return_this, "[Symbol.asyncIterator]", 0)?;
    let key = PropertyKey::Symbol(agent.symbols.async_iterator);
    agent.define_property_or_throw(
        async_iterator_prototype,
        key,
        PropertyDescriptor::data_with(Value::object(f), true, false, true),
    )?;

    // %AsyncFromSyncIteratorPrototype% (§27.1.6.2).
    let from_sync = agent.new_object_with(Some(async_iterator_prototype))?;
    install_methods(agent, from_sync, FROM_SYNC_METHODS)?;

    // %AsyncGeneratorPrototype% (§27.6.1).
    let generator_prototype = agent.new_object_with(Some(async_iterator_prototype))?;
    install_methods(agent, generator_prototype, GENERATOR_METHODS)?;
    let tag = agent.string("AsyncGenerator")?;
    agent.define_property_or_throw(
        generator_prototype,
        tag_key,
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;

    // %AsyncGeneratorFunction.prototype% (§27.4.3).
    let function_kind_prototype = agent.new_object_with(Some(function_prototype))?;
    agent.define_property_or_throw(
        function_kind_prototype,
        prototype_key,
        PropertyDescriptor::data_with(Value::object(generator_prototype), false, false, true),
    )?;
    agent.define_property_or_throw(
        generator_prototype,
        constructor_key,
        PropertyDescriptor::data_with(Value::object(function_kind_prototype), false, false, true),
    )?;
    let tag = agent.string("AsyncGeneratorFunction")?;
    agent.define_property_or_throw(
        function_kind_prototype,
        tag_key,
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;

    // %AsyncGeneratorFunction% (§27.4.1): not a global.
    let constructor =
        agent.new_native_function(async_generator_function, "AsyncGeneratorFunction", 1)?;
    agent
        .heap
        .object_mut(constructor)
        .set_flag(FLAG_NATIVE_CONSTRUCTOR, true);
    let function = agent.get(function_prototype, constructor_key)?;
    if let Some(function) = function.as_object() {
        agent.set_prototype_of(constructor, Some(function))?;
    }
    agent.define_property_or_throw(
        constructor,
        prototype_key,
        PropertyDescriptor::data_with(Value::object(function_kind_prototype), false, false, false),
    )?;
    agent.define_property_or_throw(
        function_kind_prototype,
        constructor_key,
        PropertyDescriptor::data_with(Value::object(constructor), false, false, true),
    )?;

    agent.intrinsics.async_generator_prototype = generator_prototype;
    agent.intrinsics.async_generator_function_prototype = function_kind_prototype;
    agent.intrinsics.async_from_sync_iterator_prototype = from_sync;
    Ok(())
}

fn return_this(_: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(this)
}

/// `AsyncGeneratorFunction(...params, body)` (§27.4.1.1).
fn async_generator_function(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let default = agent.intrinsics.async_generator_function_prototype;
    dynamic_function(agent, args, "async function*", default)
}

/// The value of a TypeError, for a rejection rather than a throw.
fn type_error_value(agent: &mut Agent, message: &str) -> JsResult<Value> {
    match agent.type_error::<()>(message) {
        Err(Abrupt::Throw(error)) => Ok(error),
        Err(other) => Err(other),
        Ok(()) => unreachable!("type_error throws"),
    }
}

/// `AsyncGeneratorValidate` (§27.6.3.2): the generator and its state, or
/// `None` when `this` is not an async generator.
fn validate(agent: &Agent, this: Value) -> Option<(ObjectId, GeneratorState)> {
    let o = this.as_object()?;
    match agent.heap.object(o).internal() {
        Some(Internal::Generator(g)) if g.async_generator => Some((o, g.state)),
        _ => None,
    }
}

/// `AsyncGeneratorEnqueue` (§27.6.3.3).
fn enqueue(
    agent: &mut Agent,
    o: ObjectId,
    mode: u8,
    value: Value,
    promise: ObjectId,
) -> JsResult<()> {
    agent.heap.charge(std::mem::size_of::<AsyncRequest>())?;
    if let Some(Internal::Generator(g)) = agent.heap.object_mut(o).internal_mut() {
        g.queue.push_back(AsyncRequest {
            mode,
            value,
            promise,
        });
    }
    Ok(())
}

/// The shared steps of `next`, `return` and `throw` (§27.6.1.2-4): a new
/// promise, the request queued, and the body run if it is suspended.
fn request(agent: &mut Agent, this: Value, args: &[Value], mode: u8) -> JsResult<Value> {
    let collectable = agent.resume_is_collectable();
    let promise = new_intrinsic_promise(agent)?;
    let value = arg(args, 0);
    let Some((o, state)) = validate(agent, this) else {
        let error = type_error_value(agent, "not an async generator")?;
        reject_promise(agent, promise, error);
        return Ok(Value::object(promise));
    };
    // The promise is rooted while the body runs.
    agent.with_rooted(Value::object(promise), |agent| {
        match (mode, state) {
            (RESUME_NEXT, GeneratorState::Completed) => {
                let result = iterator_result(agent, Value::UNDEFINED, true)?;
                resolve_promise(agent, promise, result)?;
            }
            (RESUME_THROW, GeneratorState::SuspendedStart | GeneratorState::Completed) => {
                if state == GeneratorState::SuspendedStart {
                    agent.finish_generator(o);
                    if let Some(Internal::Generator(g)) = agent.heap.object_mut(o).internal_mut() {
                        g.state = GeneratorState::Completed;
                    }
                }
                reject_promise(agent, promise, value);
            }
            (RESUME_RETURN, GeneratorState::SuspendedStart | GeneratorState::Completed) => {
                enqueue(agent, o, mode, value, promise)?;
                if state == GeneratorState::SuspendedStart {
                    agent.finish_generator(o);
                }
                agent.async_generator_await_return(o)?;
            }
            (_, GeneratorState::SuspendedStart | GeneratorState::SuspendedYield) => {
                enqueue(agent, o, mode, value, promise)?;
                agent.async_generator_run(o, mode, value, collectable)?;
            }
            _ => enqueue(agent, o, mode, value, promise)?,
        }
        Ok(Value::object(promise))
    })
}

/// `%AsyncGeneratorPrototype%.next(value)` (§27.6.1.2).
fn async_generator_next(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    request(agent, this, args, RESUME_NEXT)
}

/// `%AsyncGeneratorPrototype%.return(value)` (§27.6.1.3).
fn async_generator_return(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    request(agent, this, args, RESUME_RETURN)
}

/// `%AsyncGeneratorPrototype%.throw(exception)` (§27.6.1.4).
fn async_generator_throw(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    request(agent, this, args, RESUME_THROW)
}

// ---- %AsyncFromSyncIteratorPrototype% ------------------------------------------

/// `CreateAsyncFromSyncIterator(syncIteratorRecord)` (§27.1.6.1): the
/// wrapper object (its `next` is read by the caller).
pub(crate) fn create_async_from_sync_iterator(
    agent: &mut Agent,
    iterator: Value,
    next: Value,
) -> JsResult<Value> {
    agent.heap.charge(std::mem::size_of::<Extra>())?;
    let prototype = agent.intrinsics.async_from_sync_iterator_prototype;
    let mut object = Object::new(Class::Ordinary, Some(prototype));
    object.extra_mut().internal = Some(Internal::AsyncFromSync { iterator, next });
    Ok(Value::object(agent.heap.alloc_object(object)?))
}

/// The sync iterator record of `this`, which only engine code reaches.
fn sync_record(agent: &Agent, this: Value) -> (Value, Value) {
    let o = this.as_object().expect("an async-from-sync iterator");
    match agent.heap.object(o).internal() {
        Some(Internal::AsyncFromSync { iterator, next }) => (*iterator, *next),
        _ => unreachable!("an async-from-sync iterator"),
    }
}

fn intrinsic(promise: ObjectId) -> Capability {
    Capability {
        promise: Value::object(promise),
        resolve: Value::UNDEFINED,
        reject: Value::UNDEFINED,
    }
}

/// Settle `promise` from a step that may have thrown; `Ok(None)` when it
/// was rejected.
fn or_reject<T>(agent: &mut Agent, promise: ObjectId, result: JsResult<T>) -> JsResult<Option<T>> {
    match result {
        Ok(v) => Ok(Some(v)),
        Err(Abrupt::Throw(error)) => {
            reject_promise(agent, promise, error);
            Ok(None)
        }
        Err(other) => Err(other),
    }
}

/// Call `method` on the sync iterator with the argument if one was
/// passed, and require an object back.
fn call_sync(agent: &mut Agent, method: Value, iterator: Value, args: &[Value]) -> JsResult<Value> {
    let args = if args.is_empty() { &[][..] } else { &args[..1] };
    let result = agent.call(method, iterator, args)?;
    if result.as_object().is_none() {
        return agent.type_error("iterator result is not an object");
    }
    Ok(result)
}

/// `%AsyncFromSyncIteratorPrototype%.next(value)` (§27.1.6.2.1).
fn from_sync_next(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let promise = new_intrinsic_promise(agent)?;
    let (iterator, next) = sync_record(agent, this);
    let result = call_sync(agent, next, iterator, args);
    if let Some(result) = or_reject(agent, promise, result)? {
        continuation(agent, result, promise, iterator, true)?;
    }
    Ok(Value::object(promise))
}

/// `%AsyncFromSyncIteratorPrototype%.return(value)` (§27.1.6.2.2).
fn from_sync_return(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let promise = new_intrinsic_promise(agent)?;
    let (iterator, _) = sync_record(agent, this);
    let key = PropertyKey::String(agent.names.return_);
    let method = agent.get_method(iterator, key);
    let Some(method) = or_reject(agent, promise, method)? else {
        return Ok(Value::object(promise));
    };
    let Some(method) = method else {
        let result = iterator_result(agent, arg(args, 0), true)?;
        resolve_promise(agent, promise, result)?;
        return Ok(Value::object(promise));
    };
    let result = call_sync(agent, method, iterator, args);
    if let Some(result) = or_reject(agent, promise, result)? {
        continuation(agent, result, promise, iterator, false)?;
    }
    Ok(Value::object(promise))
}

/// `%AsyncFromSyncIteratorPrototype%.throw(value)` (§27.1.6.2.3).
fn from_sync_throw(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let promise = new_intrinsic_promise(agent)?;
    let (iterator, _) = sync_record(agent, this);
    let key = agent.heap.key("throw")?;
    let method = agent.get_method(iterator, key);
    let Some(method) = or_reject(agent, promise, method)? else {
        return Ok(Value::object(promise));
    };
    let Some(method) = method else {
        // The protocol is broken: close the sync iterator, then reject.
        let closed = close_normally(agent, iterator);
        if or_reject(agent, promise, closed)?.is_some() {
            let error = type_error_value(agent, "the iterator has no throw method")?;
            reject_promise(agent, promise, error);
        }
        return Ok(Value::object(promise));
    };
    let result = call_sync(agent, method, iterator, args);
    if let Some(result) = or_reject(agent, promise, result)? {
        continuation(agent, result, promise, iterator, true)?;
    }
    Ok(Value::object(promise))
}

/// `IteratorClose` with a normal completion.
fn close_normally(agent: &mut Agent, iterator: Value) -> JsResult<()> {
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

/// `IteratorClose` with a throw completion: whatever `return` does, the
/// error stands.
fn close_after_throw(agent: &mut Agent, iterator: Value, error: Value) -> JsResult<Value> {
    let key = PropertyKey::String(agent.names.return_);
    match agent.get_method(iterator, key) {
        Ok(Some(method)) => match agent.call(method, iterator, &[]) {
            Ok(_) | Err(Abrupt::Throw(_)) => {}
            Err(other) => return Err(other),
        },
        Ok(None) | Err(Abrupt::Throw(_)) => {}
        Err(other) => return Err(other),
    }
    Err(Abrupt::Throw(error))
}

/// `AsyncFromSyncIteratorContinuation(result, promiseCapability,
/// syncIteratorRecord, closeOnRejection)` (§27.1.6.4).
fn continuation(
    agent: &mut Agent,
    result: Value,
    promise: ObjectId,
    iterator: Value,
    close_on_rejection: bool,
) -> JsResult<()> {
    let o = result.as_object().expect("checked by the caller");
    let done = agent.get(o, PropertyKey::String(agent.names.done));
    let Some(done) = or_reject(agent, promise, done)? else {
        return Ok(());
    };
    let done = agent.to_boolean(done);
    let value = agent.get(o, PropertyKey::String(agent.names.value));
    let Some(value) = or_reject(agent, promise, value)? else {
        return Ok(());
    };
    let intrinsic_promise = Value::object(agent.intrinsics.promise);
    let wrapper = match super::promise::promise_resolve(agent, intrinsic_promise, value) {
        Err(Abrupt::Throw(error)) if !done && close_on_rejection => {
            close_after_throw(agent, iterator, error)
        }
        other => other,
    };
    let Some(wrapper) = or_reject(agent, promise, wrapper)? else {
        return Ok(());
    };
    let wrapper = wrapper
        .as_object()
        .expect("PromiseResolve returns a promise");
    let on_fulfilled = closure(agent, unwrap, "", 1, 1)?;
    set_capture(agent, on_fulfilled, Value::boolean(done));
    let on_rejected = if done || !close_on_rejection {
        Value::UNDEFINED
    } else {
        let f = closure(agent, close_iterator, "", 1, 1)?;
        set_capture(agent, f, iterator);
        Value::object(f)
    };
    perform_then(
        agent,
        wrapper,
        Value::object(on_fulfilled),
        on_rejected,
        Some(intrinsic(promise)),
    )
}

fn set_capture(agent: &mut Agent, f: ObjectId, value: Value) {
    if let Some(Internal::Captures(c)) = agent.heap.object_mut(f).internal_mut() {
        c[0] = value;
    }
}

/// The fulfilled handler of the continuation: `{value, done}`.
fn unwrap(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let f = me(agent);
    let done = captures(agent, f)[0] == Value::TRUE;
    iterator_result(agent, arg(args, 0), done)
}

/// The rejected handler of the continuation: close the sync iterator
/// and pass the rejection on.
fn close_iterator(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let f = me(agent);
    let iterator = captures(agent, f)[0];
    close_after_throw(agent, iterator, arg(args, 0))
}
