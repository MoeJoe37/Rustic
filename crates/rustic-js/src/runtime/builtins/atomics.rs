//! The Atomics object (§25.4).
//!
//! An agent runs on one thread and nothing else can reach its buffers
//! yet, so every operation here is trivially atomic: it reads and writes
//! the buffer's bytes in place. What the functions do enforce is the
//! specification's contract: which typed arrays are accepted, the order
//! of conversions and revalidation, wrapping arithmetic on the raw
//! element, and the waiter lists of `wait`, `waitAsync` and `notify`.
//!
//! `Atomics.wait` cannot be woken: no other agent shares the memory. An
//! agent that may not block (the default, as on a browser's main thread)
//! throws a TypeError; one that may ([`Agent::set_can_block`]) reports
//! `"timed-out"` for a finite timeout, moving the fixed clock forward if
//! one is set, and is cancelled on an infinite one. `Atomics.waitAsync`
//! waiters are woken by `Atomics.notify` in the same agent, or expire
//! when the host calls [`Agent::expire_waiters`].

use super::buffer::ElementKind;
use super::typed_array::{View, length_of, view_of};
use super::{arg, install_methods, method};
use crate::runtime::agent::{Abrupt, Agent, ErrorKind, JsResult};
use crate::runtime::number::to_integer_or_infinity;
use crate::runtime::object::PropertyDescriptor;
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::{ObjectId, Value};
use crate::runtime::{new_intrinsic_promise, resolve_promise};

/// A promise of `Atomics.waitAsync` that has not settled.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Waiter {
    /// The shared buffer waited on.
    pub buffer: ObjectId,
    /// The byte position in the buffer.
    pub byte_index: usize,
    /// The promise to resolve with `"ok"` or `"timed-out"`.
    pub promise: ObjectId,
    /// When the wait times out, in milliseconds on the agent's clock;
    /// infinite for none.
    pub deadline: f64,
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let atomics = agent.new_object()?;
    install_methods(
        agent,
        atomics,
        &[
            method("add", add, 3),
            method("and", and, 3),
            method("compareExchange", compare_exchange, 4),
            method("exchange", exchange, 3),
            method("isLockFree", is_lock_free, 1),
            method("load", load, 2),
            method("notify", notify, 3),
            method("or", or, 3),
            method("pause", pause, 0),
            method("store", store, 3),
            method("sub", sub, 3),
            method("wait", wait, 4),
            method("waitAsync", wait_async, 4),
            method("xor", xor, 3),
        ],
    )?;
    super::buffer::to_string_tag(agent, atomics, "Atomics")?;
    let global = agent.intrinsics.global;
    let key = agent.heap.key("Atomics")?;
    agent.define_property_or_throw(
        global,
        key,
        PropertyDescriptor::data_with(Value::object(atomics), true, false, true),
    )
}

/// `ValidateIntegerTypedArray(typedArray, waitable)` (§25.4.3.1).
fn validate(agent: &mut Agent, value: Value, waitable: bool) -> JsResult<View> {
    let Some(view) = value.as_object().and_then(|o| view_of(agent, o)) else {
        return agent.type_error("Atomics operations need an integer typed array");
    };
    if length_of(agent, &view).is_none() {
        return agent.type_error("the typed array is detached or out of the bounds of its buffer");
    }
    let accepted = if waitable {
        matches!(view.kind, ElementKind::Int32 | ElementKind::BigInt64)
    } else {
        !matches!(
            view.kind,
            ElementKind::Uint8Clamped
                | ElementKind::Float16
                | ElementKind::Float32
                | ElementKind::Float64
        )
    };
    if !accepted {
        return agent.type_error(if waitable {
            "Atomics.wait and notify need an Int32Array or a BigInt64Array"
        } else {
            "Atomics operations need an integer typed array"
        });
    }
    Ok(view)
}

/// `ValidateAtomicAccess` (§25.4.3.2): the byte position of element
/// `index` in the buffer.
fn access(agent: &mut Agent, view: &View, index: Value) -> JsResult<usize> {
    let length = length_of(agent, view).expect("validated above");
    let index = agent.to_index(index)?;
    match usize::try_from(index).ok().filter(|i| *i < length) {
        Some(index) => Ok(view.offset + index * view.kind.size()),
        None => Err(agent.throw(ErrorKind::RangeError, "the index is out of range")),
    }
}

/// `RevalidateAtomicAccess` (§25.4.3.4): conversions ran script, which
/// may have shrunk or detached the buffer.
fn revalidate(agent: &mut Agent, view: &View, byte_index: usize) -> JsResult<()> {
    let Some(length) = length_of(agent, view) else {
        return agent.type_error("the typed array is detached or out of the bounds of its buffer");
    };
    if byte_index >= view.offset + length * view.kind.size() {
        return Err(agent.throw(ErrorKind::RangeError, "the index is out of range"));
    }
    Ok(())
}

/// The operand of a read-modify-write: the value `store` returns and its
/// raw element bits. Numbers go through `ToIntegerOrInfinity` first, so
/// `-0` and `NaN` become `+0`.
fn operand(agent: &mut Agent, kind: ElementKind, value: Value) -> JsResult<(Value, u64)> {
    if kind.is_bigint() {
        let big = agent.to_bigint(value)?;
        let bits = big.to_u64_wrapping();
        return Ok((agent.bigint_value(big)?, bits));
    }
    let n = agent.to_number(value)?;
    let integer = Value::number(to_integer_or_infinity(n));
    let bits = agent.element_bits(kind, integer)?;
    Ok((integer, bits))
}

/// `AtomicReadModifyWrite` (§25.4.3.14): replace the element with
/// `op(old, operand)` and return the old value.
fn read_modify_write(
    agent: &mut Agent,
    args: &[Value],
    op: fn(u64, u64) -> u64,
) -> JsResult<Value> {
    let view = validate(agent, arg(args, 0), false)?;
    let at = access(agent, &view, arg(args, 1))?;
    let (_, operand) = operand(agent, view.kind, arg(args, 2))?;
    revalidate(agent, &view, at)?;
    let size = view.kind.size();
    let old = agent.buffer_read(view.buffer, at, size);
    // `buffer_write` keeps the low `size` bytes, which is the wrap.
    agent.buffer_write(view.buffer, at, size, op(old, operand))?;
    agent.element_value(view.kind, old)
}

fn add(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    read_modify_write(agent, args, u64::wrapping_add)
}

fn sub(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    read_modify_write(agent, args, u64::wrapping_sub)
}

fn and(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    read_modify_write(agent, args, |a, b| a & b)
}

fn or(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    read_modify_write(agent, args, |a, b| a | b)
}

fn xor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    read_modify_write(agent, args, |a, b| a ^ b)
}

fn exchange(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    read_modify_write(agent, args, |_, b| b)
}

/// `Atomics.compareExchange` (§25.4.6): elements compare by their raw
/// bytes, so `expected` wraps the way a stored value would.
fn compare_exchange(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let view = validate(agent, arg(args, 0), false)?;
    agent.require_mutable_buffer(view.buffer)?;
    let at = access(agent, &view, arg(args, 1))?;
    let (_, expected) = operand(agent, view.kind, arg(args, 2))?;
    let (_, replacement) = operand(agent, view.kind, arg(args, 3))?;
    revalidate(agent, &view, at)?;
    let size = view.kind.size();
    let mask = if size == 8 {
        u64::MAX
    } else {
        (1 << (8 * size)) - 1
    };
    let old = agent.buffer_read(view.buffer, at, size);
    if old == expected & mask {
        agent.buffer_write(view.buffer, at, size, replacement)?;
    }
    agent.element_value(view.kind, old)
}

/// `Atomics.isLockFree(size)` (§25.4.8): every element size is, since
/// nothing here takes a lock.
fn is_lock_free(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let n = agent.to_number(arg(args, 0))?;
    let n = to_integer_or_infinity(n);
    Ok(Value::boolean(n == 1.0 || n == 2.0 || n == 4.0 || n == 8.0))
}

fn load(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let view = validate(agent, arg(args, 0), false)?;
    let at = access(agent, &view, arg(args, 1))?;
    revalidate(agent, &view, at)?;
    let bits = agent.buffer_read(view.buffer, at, view.kind.size());
    agent.element_value(view.kind, bits)
}

fn store(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let view = validate(agent, arg(args, 0), false)?;
    let at = access(agent, &view, arg(args, 1))?;
    let (value, bits) = operand(agent, view.kind, arg(args, 2))?;
    revalidate(agent, &view, at)?;
    agent.buffer_write(view.buffer, at, view.kind.size(), bits)?;
    Ok(value)
}

/// `Atomics.pause(N)`: a hint with no effect here.
fn pause(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let n = arg(args, 0);
    if n != Value::UNDEFINED && !n.as_number().is_some_and(|n| n.fract() == 0.0) {
        return agent.type_error("Atomics.pause: the iteration count must be an integer");
    }
    Ok(Value::UNDEFINED)
}

/// What `DoWait` (§25.4.3.14) found before it would suspend.
enum Wait {
    /// The element did not hold the expected value.
    NotEqual,
    /// The agent would wait at byte `at` of `buffer` for `timeout`
    /// milliseconds.
    Suspend {
        buffer: ObjectId,
        at: usize,
        timeout: f64,
    },
}

fn do_wait(agent: &mut Agent, args: &[Value], sync: bool) -> JsResult<Wait> {
    let view = validate(agent, arg(args, 0), true)?;
    if !agent.buffer_data(view.buffer).is_some_and(|d| d.shared) {
        return agent.type_error("Atomics.wait needs a typed array on a SharedArrayBuffer");
    }
    let at = access(agent, &view, arg(args, 1))?;
    let expected = if view.kind.is_bigint() {
        // The low eight bytes, as the element stores them.
        agent.to_bigint64(arg(args, 2))? as u64
    } else {
        let n = agent.to_number(arg(args, 2))?;
        agent.element_bits(view.kind, Value::number(n))?
    };
    let q = agent.to_number(arg(args, 3))?;
    let timeout = if q.is_nan() {
        f64::INFINITY
    } else {
        q.max(0.0)
    };
    if sync && !agent.vm.can_block {
        return agent.type_error("Atomics.wait cannot block this agent");
    }
    let size = view.kind.size();
    let mask = if size == 8 {
        u64::MAX
    } else {
        (1 << (8 * size)) - 1
    };
    if agent.buffer_read(view.buffer, at, size) != expected & mask {
        return Ok(Wait::NotEqual);
    }
    Ok(Wait::Suspend {
        buffer: view.buffer,
        at,
        timeout,
    })
}

/// `Atomics.wait(typedArray, index, value, timeout)` (§25.4.13).
fn wait(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    match do_wait(agent, args, true)? {
        Wait::NotEqual => agent.string("not-equal"),
        Wait::Suspend { timeout, .. } => {
            if timeout.is_infinite() {
                // Nothing can wake this agent: the wait would never end.
                return Err(Abrupt::Cancelled);
            }
            if let Some(now) = agent.vm.clock {
                agent.vm.clock = Some(now + timeout);
            }
            agent.string("timed-out")
        }
    }
}

/// `Atomics.waitAsync(typedArray, index, value, timeout)` (§25.4.14).
fn wait_async(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let outcome = do_wait(agent, args, false)?;
    let result = agent.new_object()?;
    let (is_async, value) = match outcome {
        Wait::NotEqual => (false, agent.string("not-equal")?),
        Wait::Suspend {
            buffer,
            at,
            timeout,
        } => 'wait: {
            if timeout == 0.0 {
                break 'wait (false, agent.string("timed-out")?);
            }
            let promise = new_intrinsic_promise(agent)?;
            agent.heap.charge(std::mem::size_of::<Waiter>())?;
            let deadline = super::date::now(agent) + timeout;
            agent.vm.waiters.push(Waiter {
                buffer,
                byte_index: at,
                promise,
                deadline,
            });
            (true, Value::object(promise))
        }
    };
    let key = agent.heap.key("async")?;
    agent.create_data_property_or_throw(result, key, Value::boolean(is_async))?;
    let key = PropertyKey::String(agent.names.value);
    agent.create_data_property_or_throw(result, key, value)?;
    Ok(Value::object(result))
}

/// `Atomics.notify(typedArray, index, count)` (§25.4.15): wake up to
/// `count` waiters on the element, oldest first.
fn notify(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let view = validate(agent, arg(args, 0), true)?;
    let at = access(agent, &view, arg(args, 1))?;
    let count = match arg(args, 2) {
        Value::UNDEFINED => f64::INFINITY,
        count => {
            let n = agent.to_number(count)?;
            to_integer_or_infinity(n).max(0.0)
        }
    };
    if !agent.buffer_data(view.buffer).is_some_and(|d| d.shared) {
        return Ok(Value::number(0.0));
    }
    let mut woken = Vec::new();
    agent.vm.waiters.retain(|waiter| {
        let matches = waiter.buffer == view.buffer && waiter.byte_index == at;
        if matches && (woken.len() as f64) < count {
            woken.push(waiter.promise);
            return false;
        }
        true
    });
    let ok = agent.string("ok")?;
    for &promise in &woken {
        resolve_promise(agent, promise, ok)?;
    }
    Ok(Value::number(woken.len() as f64))
}

impl Agent {
    /// Let `Atomics.wait` block this agent (`AgentCanSuspend`, §9.7).
    /// Off by default, as for a browser's main thread, where `wait`
    /// throws a TypeError. No other agent shares memory yet, so a wait
    /// that is allowed can only time out.
    pub fn set_can_block(&mut self, can_block: bool) {
        self.vm.can_block = can_block;
    }

    /// The earliest time an `Atomics.waitAsync` promise times out, in
    /// milliseconds on the agent's clock, if one is pending with a
    /// finite timeout.
    pub fn next_waiter_deadline(&self) -> Option<f64> {
        self.vm
            .waiters
            .iter()
            .map(|w| w.deadline)
            .filter(|d| d.is_finite())
            .min_by(f64::total_cmp)
    }

    /// Resolve with `"timed-out"` every `Atomics.waitAsync` promise whose
    /// deadline is at or before `now` (milliseconds on the agent's
    /// clock), and return how many there were. The host calls this from
    /// its timer task; the reactions run at the next
    /// [`Agent::run_jobs`].
    pub fn expire_waiters(&mut self, now: f64) -> JsResult<usize> {
        let mut expired = Vec::new();
        self.vm.waiters.retain(|waiter| {
            if waiter.deadline <= now {
                expired.push(waiter.promise);
                return false;
            }
            true
        });
        let timed_out = self.string("timed-out")?;
        for &promise in &expired {
            resolve_promise(self, promise, timed_out)?;
        }
        Ok(expired.len())
    }
}
