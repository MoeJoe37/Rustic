//! `Array.fromAsync` (§23.1.2.2), driven by the intrinsic promise queue.

use super::super::async_generator::create_async_from_sync_iterator;
use super::super::promise::{closure, me, perform_then, promise_resolve, set_capture};
use super::{MAX_SAFE_INTEGER, arg, construct_or_array, create_index, get_index, set_length};
use crate::runtime::object::{Class, Internal, Object};
use crate::runtime::{
    Abrupt, Agent, ErrorKind, Extra, JsResult, ObjectId, PropertyKey, Value, new_intrinsic_promise,
    reject_promise, resolve_promise,
};

#[derive(Clone, Copy, Debug)]
enum Source {
    Iterator { iterator: Value, next: Value },
    ArrayLike { object: ObjectId, length: f64 },
}

#[derive(Clone, Copy, Debug)]
enum Phase {
    Next,
    ArrayLikeValue,
    Mapped,
    Close(Value),
}

/// The output and one outstanding await. Reactions reuse the same two native
/// callbacks for the operation's lifetime.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FromAsyncState {
    source: Source,
    array: ObjectId,
    promise: ObjectId,
    mapper: Value,
    this_arg: Value,
    fulfilled: ObjectId,
    rejected: ObjectId,
    index: f64,
    phase: Phase,
}

impl FromAsyncState {
    pub(crate) fn values(&self) -> [Value; 9] {
        let (source, next) = match self.source {
            Source::Iterator { iterator, next } => (iterator, next),
            Source::ArrayLike { object, .. } => (Value::object(object), Value::UNDEFINED),
        };
        let thrown = match self.phase {
            Phase::Close(error) => error,
            _ => Value::UNDEFINED,
        };
        [
            source,
            next,
            Value::object(self.array),
            Value::object(self.promise),
            self.mapper,
            self.this_arg,
            Value::object(self.fulfilled),
            Value::object(self.rejected),
            thrown,
        ]
    }
}

pub(super) fn from_async(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let promise = new_intrinsic_promise(agent)?;
    let result = start(
        agent,
        promise,
        this,
        arg(args, 0),
        arg(args, 1),
        arg(args, 2),
    );
    reject_on_throw(agent, promise, result)?;
    Ok(Value::object(promise))
}

fn reject_on_throw(agent: &mut Agent, promise: ObjectId, result: JsResult<()>) -> JsResult<()> {
    match result {
        Err(Abrupt::Throw(error)) => {
            reject_promise(agent, promise, error);
            Ok(())
        }
        other => other,
    }
}

fn iterator_record(agent: &mut Agent, items: Value, method: Value) -> JsResult<(Value, Value)> {
    let iterator = agent.call(method, items, &[])?;
    let Some(object) = iterator.as_object() else {
        return agent.type_error("Array.fromAsync: iterator is not an object");
    };
    let next = agent.get(object, PropertyKey::String(agent.names.next))?;
    Ok((iterator, next))
}

fn source(agent: &mut Agent, items: Value) -> JsResult<Source> {
    if let Some(method) =
        agent.get_method(items, PropertyKey::Symbol(agent.symbols.async_iterator))?
    {
        let (iterator, next) = iterator_record(agent, items, method)?;
        return Ok(Source::Iterator { iterator, next });
    }
    if let Some(method) = agent.get_method(items, PropertyKey::Symbol(agent.symbols.iterator))? {
        let (iterator, next) = iterator_record(agent, items, method)?;
        let iterator = create_async_from_sync_iterator(agent, iterator, next)?;
        let object = iterator.as_object().expect("an async-from-sync iterator");
        let next = agent.get(object, PropertyKey::String(agent.names.next))?;
        return Ok(Source::Iterator { iterator, next });
    }
    let object = agent.to_object(items)?;
    let length = agent.length_of_array_like(object)?;
    Ok(Source::ArrayLike { object, length })
}

fn start(
    agent: &mut Agent,
    promise: ObjectId,
    this: Value,
    items: Value,
    mapper: Value,
    this_arg: Value,
) -> JsResult<()> {
    if mapper != Value::UNDEFINED && !agent.is_callable(mapper) {
        return agent.type_error("Array.fromAsync: mapper is not a function");
    }
    let source = source(agent, items)?;
    let array = match source {
        Source::Iterator { .. } => construct_or_array(agent, this, &[])?,
        Source::ArrayLike { length, .. } => {
            construct_or_array(agent, this, &[Value::number(length)])?
        }
    };
    let fulfilled = closure(agent, resume::<true>, "", 1, 1)?;
    let rejected = closure(agent, resume::<false>, "", 1, 1)?;
    let state = FromAsyncState {
        source,
        array,
        promise,
        mapper,
        this_arg,
        fulfilled,
        rejected,
        index: 0.0,
        phase: Phase::Next,
    };
    agent
        .heap
        .charge(std::mem::size_of::<Extra>() + std::mem::size_of::<FromAsyncState>())?;
    let mut object = Object::new(Class::Ordinary, None);
    object.extra_mut().internal = Some(Internal::ArrayFromAsync(Box::new(state)));
    let id = agent.heap.alloc_object(object)?;
    set_capture(agent, fulfilled, 0, Value::object(id));
    set_capture(agent, rejected, 0, Value::object(id));
    advance(agent, id, state)
}

fn save(agent: &mut Agent, id: ObjectId, state: FromAsyncState) {
    let Some(Internal::ArrayFromAsync(slot)) = agent.heap.object_mut(id).internal_mut() else {
        unreachable!("an Array.fromAsync continuation");
    };
    **slot = state;
}

fn suspend(
    agent: &mut Agent,
    id: ObjectId,
    mut state: FromAsyncState,
    phase: Phase,
    value: Value,
) -> JsResult<()> {
    let intrinsic = Value::object(agent.intrinsics.promise);
    let awaited = promise_resolve(agent, intrinsic, value)?
        .as_object()
        .expect("an intrinsic promise");
    state.phase = phase;
    save(agent, id, state);
    perform_then(
        agent,
        awaited,
        Value::object(state.fulfilled),
        Value::object(state.rejected),
        None,
    )
}

fn finish(agent: &mut Agent, state: FromAsyncState) -> JsResult<()> {
    set_length(agent, state.array, state.index)?;
    resolve_promise(agent, state.promise, Value::object(state.array))
}

fn advance(agent: &mut Agent, id: ObjectId, state: FromAsyncState) -> JsResult<()> {
    agent.check_native_work()?;
    match state.source {
        Source::Iterator { iterator, next } => {
            if state.index >= MAX_SAFE_INTEGER {
                let error = agent.throw(ErrorKind::TypeError, "Array.fromAsync: too many elements");
                return close_after_error(agent, id, state, error);
            }
            let value = agent.call(next, iterator, &[])?;
            suspend(agent, id, state, Phase::Next, value)
        }
        Source::ArrayLike { object, length } => {
            if state.index == length {
                return finish(agent, state);
            }
            let value = get_index(agent, object, state.index)?;
            suspend(agent, id, state, Phase::ArrayLikeValue, value)
        }
    }
}

fn close_after_error(
    agent: &mut Agent,
    id: ObjectId,
    state: FromAsyncState,
    error: Abrupt,
) -> JsResult<()> {
    let (Source::Iterator { iterator, .. }, Abrupt::Throw(thrown)) = (state.source, error) else {
        return Err(error);
    };
    let close = (|| {
        let key = PropertyKey::String(agent.names.return_);
        let Some(method) = agent.get_method(iterator, key)? else {
            return Err(error);
        };
        let value = agent.call(method, iterator, &[])?;
        suspend(agent, id, state, Phase::Close(thrown), value)
    })();
    // AsyncIteratorClose preserves the original throw, even when return or
    // PromiseResolve throws. The host's execution limits still propagate.
    match close {
        Err(Abrupt::Throw(_)) => Err(error),
        other => other,
    }
}

fn accept_value(
    agent: &mut Agent,
    id: ObjectId,
    state: FromAsyncState,
    value: Value,
) -> JsResult<()> {
    if state.mapper == Value::UNDEFINED {
        return store_value(agent, id, state, value);
    }
    let mapped = agent
        .call(
            state.mapper,
            state.this_arg,
            &[value, Value::number(state.index)],
        )
        .and_then(|value| suspend(agent, id, state, Phase::Mapped, value));
    match mapped {
        Ok(()) => Ok(()),
        Err(error) => close_after_error(agent, id, state, error),
    }
}

fn store_value(
    agent: &mut Agent,
    id: ObjectId,
    mut state: FromAsyncState,
    value: Value,
) -> JsResult<()> {
    if let Err(error) = create_index(agent, state.array, state.index, value) {
        return close_after_error(agent, id, state, error);
    }
    state.index += 1.0;
    advance(agent, id, state)
}

fn resume<const FULFILLED: bool>(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(Internal::Captures(captures)) = agent.heap.object(me(agent)).internal() else {
        unreachable!("a native promise callback");
    };
    let id = captures[0].as_object().expect("the continuation object");
    let Some(Internal::ArrayFromAsync(state)) = agent.heap.object(id).internal() else {
        unreachable!("an Array.fromAsync continuation");
    };
    let state = **state;
    let value = arg(args, 0);
    let result = match state.phase {
        Phase::Close(original) => Err(Abrupt::Throw(original)),
        Phase::Mapped if !FULFILLED => close_after_error(agent, id, state, Abrupt::Throw(value)),
        _ if !FULFILLED => Err(Abrupt::Throw(value)),
        Phase::Mapped => store_value(agent, id, state, value),
        Phase::ArrayLikeValue => accept_value(agent, id, state, value),
        Phase::Next => accept_next(agent, id, state, value),
    };
    reject_on_throw(agent, state.promise, result)?;
    Ok(Value::UNDEFINED)
}

fn accept_next(
    agent: &mut Agent,
    id: ObjectId,
    state: FromAsyncState,
    value: Value,
) -> JsResult<()> {
    let Some(result) = value.as_object() else {
        return agent.type_error("Array.fromAsync: iterator result is not an object");
    };
    let done = agent.get(result, PropertyKey::String(agent.names.done))?;
    if agent.to_boolean(done) {
        return finish(agent, state);
    }
    let value = agent.get(result, PropertyKey::String(agent.names.value))?;
    accept_value(agent, id, state, value)
}
