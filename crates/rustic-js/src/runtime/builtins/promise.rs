//! `Promise` (§27.2) and the agent's job queue (§9.5).
//!
//! A promise is an ordinary object with a [`PromiseRecord`]. Reactions
//! queue [`Job`]s on the agent; the host drains them with
//! [`Agent::run_jobs`] (a microtask checkpoint).
//!
//! The capability of a promise made from `%Promise%` itself has no
//! resolving function objects: nothing outside the engine can reach them,
//! so a reaction settles the derived promise directly. Resolving functions
//! are only created where script can see them (the executor of
//! `new Promise`, a thenable's `then`, `Promise.withResolvers`) or where a
//! subclass constructor demands them.

use super::{Method, arg, install_methods, iterate, method, prototype_from_new_target};
use crate::runtime::Extra;
use crate::runtime::agent::{Abrupt, Agent, JsResult, NativeFn};
use crate::runtime::object::{
    Class, Internal, Job, Object, PromiseRecord, PromiseState, PropertyDescriptor, Reaction,
};
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::{ObjectId, Value};

const STATIC_METHODS: &[Method] = &[
    method("all", promise_all, 1),
    method("allSettled", promise_all_settled, 1),
    method("any", promise_any, 1),
    method("race", promise_race, 1),
    method("reject", promise_reject, 1),
    method("resolve", promise_resolve_static, 1),
    method("try", promise_try, 1),
    method("withResolvers", promise_with_resolvers, 0),
];

const PROTOTYPE_METHODS: &[Method] = &[
    method("catch", promise_catch, 1),
    method("finally", promise_finally, 1),
    method("then", promise_then, 2),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let prototype = agent.new_object()?;
    let constructor =
        super::install_constructor(agent, "Promise", promise_constructor, 1, prototype)?;
    install_methods(agent, constructor, STATIC_METHODS)?;
    install_methods(agent, prototype, PROTOTYPE_METHODS)?;
    let getter = agent.new_native_function(species_getter, "get [Symbol.species]", 0)?;
    let species = PropertyKey::Symbol(agent.symbols.species);
    agent.define_property_or_throw(
        constructor,
        species,
        PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, false, true),
    )?;
    let tag_key = PropertyKey::Symbol(agent.symbols.to_string_tag);
    let tag = agent.string("Promise")?;
    agent.define_property_or_throw(
        prototype,
        tag_key,
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    agent.intrinsics.promise = constructor;
    agent.intrinsics.promise_prototype = prototype;

    // %AsyncFunction% and its prototype (§27.7): not globals.
    let function_prototype = agent.intrinsics.function_prototype;
    let constructor_key = PropertyKey::String(agent.names.constructor);
    let prototype_key = PropertyKey::String(agent.names.prototype);
    let async_prototype = agent.new_object_with(Some(function_prototype))?;
    let tag = agent.string("AsyncFunction")?;
    agent.define_property_or_throw(
        async_prototype,
        tag_key,
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    let async_constructor = agent.new_native_function(async_function, "AsyncFunction", 1)?;
    agent
        .heap
        .object_mut(async_constructor)
        .set_flag(crate::runtime::FLAG_NATIVE_CONSTRUCTOR, true);
    let function = agent.get(function_prototype, constructor_key)?;
    if let Some(function) = function.as_object() {
        agent.set_prototype_of(async_constructor, Some(function))?;
    }
    agent.define_property_or_throw(
        async_constructor,
        prototype_key,
        PropertyDescriptor::data_with(Value::object(async_prototype), false, false, false),
    )?;
    agent.define_property_or_throw(
        async_prototype,
        constructor_key,
        PropertyDescriptor::data_with(Value::object(async_constructor), false, false, true),
    )?;
    agent.intrinsics.async_function_prototype = async_prototype;
    Ok(())
}

/// `AsyncFunction(...params, body)` (§27.7.1.1).
fn async_function(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let default = agent.intrinsics.async_function_prototype;
    dynamic_function(agent, args, "async function", default)
}

/// CreateDynamicFunction (§20.2.1.1.1) for a constructor of the given
/// kind: `keyword` starts the function expression, and `default` is the
/// prototype when `new.target` is the constructor itself.
pub(super) fn dynamic_function(
    agent: &mut Agent,
    args: &[Value],
    keyword: &str,
    default: ObjectId,
) -> JsResult<Value> {
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
    let f = agent.create_dynamic_function(keyword, &parameter_text.join(","), &body_text)?;
    if let (Some(target), Some(o)) = (agent.new_target(), f.as_object()) {
        if agent.active_function() != Some(target) {
            let prototype = prototype_from_new_target(agent, default)?;
            agent.set_prototype_of(o, Some(prototype))?;
        }
    }
    Ok(f)
}

fn species_getter(_: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(this)
}

// ---- promise records ---------------------------------------------------------

/// A new pending promise with prototype `prototype`.
fn new_promise(agent: &mut Agent, prototype: ObjectId) -> JsResult<ObjectId> {
    agent
        .heap
        .charge(std::mem::size_of::<Extra>() + std::mem::size_of::<PromiseRecord>())?;
    let mut object = Object::new(Class::Ordinary, Some(prototype));
    object.extra_mut().internal = Some(Internal::Promise(Box::new(PromiseRecord {
        state: PromiseState::Pending,
        result: Value::UNDEFINED,
        fulfill_reactions: Vec::new(),
        reject_reactions: Vec::new(),
        handled: false,
    })));
    Ok(agent.heap.alloc_object(object)?)
}

fn record(agent: &mut Agent, promise: ObjectId) -> Option<&mut PromiseRecord> {
    match agent.heap.object_mut(promise).internal_mut() {
        Some(Internal::Promise(p)) => Some(p),
        _ => None,
    }
}

/// `IsPromise(x)`.
fn is_promise(agent: &Agent, x: Value) -> bool {
    x.as_object()
        .is_some_and(|o| matches!(agent.heap.object(o).internal(), Some(Internal::Promise(_))))
}

/// `FulfillPromise` / `RejectPromise` (§27.2.1.4, §27.2.1.7) with
/// `TriggerPromiseReactions`.
fn settle(agent: &mut Agent, promise: ObjectId, value: Value, fulfilled: bool) {
    let Some(p) = record(agent, promise) else {
        return;
    };
    if p.state != PromiseState::Pending {
        return;
    }
    p.state = if fulfilled {
        PromiseState::Fulfilled
    } else {
        PromiseState::Rejected
    };
    p.result = value;
    let unhandled = !fulfilled && !p.handled;
    let fulfill = std::mem::take(&mut p.fulfill_reactions);
    let reject = std::mem::take(&mut p.reject_reactions);
    let reactions = if fulfilled { fulfill } else { reject };
    for reaction in reactions {
        agent.vm.jobs.push_back(Job::Reaction {
            reaction,
            fulfilled,
            argument: value,
        });
    }
    if unhandled {
        // `HostPromiseRejectionTracker(promise, "reject")` (§27.2.1.9).
        agent.vm.rejections.push(RejectionEvent {
            operation: RejectionOperation::Reject,
            promise,
        });
    }
}

/// What happened to a promise, for the host (§27.2.1.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectionOperation {
    /// The promise was rejected while it had no handler.
    Reject,
    /// A handler was added to a promise the host was already told about.
    Handle,
}

/// One call of `HostPromiseRejectionTracker`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RejectionEvent {
    /// `"reject"` or `"handle"`.
    pub operation: RejectionOperation,
    /// The promise.
    pub promise: ObjectId,
}

/// The steps of a promise resolve function (§27.2.1.3.2) after its
/// already-resolved check.
pub(crate) fn resolve_promise(
    agent: &mut Agent,
    promise: ObjectId,
    resolution: Value,
) -> JsResult<()> {
    if resolution == Value::object(promise) {
        let error = agent.throw(
            crate::runtime::ErrorKind::TypeError,
            "a promise cannot be resolved with itself",
        );
        let Abrupt::Throw(e) = error else {
            return Err(error);
        };
        settle(agent, promise, e, false);
        return Ok(());
    }
    let Some(object) = resolution.as_object() else {
        settle(agent, promise, resolution, true);
        return Ok(());
    };
    let then_key = agent.heap.key("then")?;
    let then = match agent.get(object, then_key) {
        Ok(then) => then,
        Err(Abrupt::Throw(e)) => {
            settle(agent, promise, e, false);
            return Ok(());
        }
        Err(other) => return Err(other),
    };
    if !agent.is_callable(then) {
        settle(agent, promise, resolution, true);
        return Ok(());
    }
    agent.vm.jobs.push_back(Job::ResolveThenable {
        promise: Value::object(promise),
        thenable: resolution,
        then,
    });
    Ok(())
}

/// `RejectPromise` for a promise only the engine can settle.
pub(crate) fn reject_promise(agent: &mut Agent, promise: ObjectId, reason: Value) {
    settle(agent, promise, reason, false);
}

/// `CreateResolvingFunctions(promise)` (§27.2.1.3): two built-in closures
/// that share the already-resolved state by clearing each other's
/// captured promise.
fn resolving_functions(agent: &mut Agent, promise: ObjectId) -> JsResult<(Value, Value)> {
    let resolve = closure(agent, resolve_function, "", 1, 2)?;
    let reject = closure(agent, reject_function, "", 1, 2)?;
    set_captures(
        agent,
        resolve,
        &[Value::object(promise), Value::object(reject)],
    );
    set_captures(
        agent,
        reject,
        &[Value::object(promise), Value::object(resolve)],
    );
    Ok((Value::object(resolve), Value::object(reject)))
}

/// A built-in function with `slots` captured values (all undefined).
pub(super) fn closure(
    agent: &mut Agent,
    behavior: NativeFn,
    name: &str,
    length: u32,
    slots: usize,
) -> JsResult<ObjectId> {
    let f = agent.new_native_function(behavior, name, length)?;
    agent
        .heap
        .charge(std::mem::size_of::<Extra>() + 8 * slots)?;
    agent.heap.object_mut(f).extra_mut().internal = Some(Internal::Captures(
        vec![Value::UNDEFINED; slots].into_boxed_slice(),
    ));
    Ok(f)
}

fn set_captures(agent: &mut Agent, f: ObjectId, values: &[Value]) {
    if let Some(Internal::Captures(c)) = agent.heap.object_mut(f).internal_mut() {
        c[..values.len()].copy_from_slice(values);
    }
}

pub(super) fn captures(agent: &Agent, f: ObjectId) -> Box<[Value]> {
    match agent.heap.object(f).internal() {
        Some(Internal::Captures(c)) => c.clone(),
        _ => Box::new([]),
    }
}

pub(super) fn set_capture(agent: &mut Agent, f: ObjectId, index: usize, value: Value) {
    if let Some(Internal::Captures(c)) = agent.heap.object_mut(f).internal_mut() {
        c[index] = value;
    }
}

/// The running built-in closure.
pub(super) fn me(agent: &Agent) -> ObjectId {
    agent
        .active_function()
        .expect("a built-in closure is running")
}

/// Take the promise out of a resolving function and its partner: `None`
/// when either has already been called.
fn take_resolving(agent: &mut Agent) -> Option<ObjectId> {
    let f = me(agent);
    let c = captures(agent, f);
    let promise = c.first()?.as_object()?;
    set_capture(agent, f, 0, Value::UNDEFINED);
    if let Some(partner) = c.get(1).and_then(|p| p.as_object()) {
        set_capture(agent, partner, 0, Value::UNDEFINED);
    }
    Some(promise)
}

fn resolve_function(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if let Some(promise) = take_resolving(agent) {
        resolve_promise(agent, promise, arg(args, 0))?;
    }
    Ok(Value::UNDEFINED)
}

fn reject_function(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if let Some(promise) = take_resolving(agent) {
        settle(agent, promise, arg(args, 0), false);
    }
    Ok(Value::UNDEFINED)
}

// ---- capabilities ------------------------------------------------------------

/// A PromiseCapability Record; `resolve` undefined means the promise is
/// an intrinsic one settled directly.
#[derive(Clone, Copy)]
pub(super) struct Capability {
    pub(super) promise: Value,
    pub(super) resolve: Value,
    pub(super) reject: Value,
}

/// `NewPromiseCapability(C)` (§27.2.1.5).
fn new_capability(agent: &mut Agent, c: Value) -> JsResult<Capability> {
    if c == Value::object(agent.intrinsics.promise) {
        let prototype = agent.intrinsics.promise_prototype;
        let promise = new_promise(agent, prototype)?;
        return Ok(Capability {
            promise: Value::object(promise),
            resolve: Value::UNDEFINED,
            reject: Value::UNDEFINED,
        });
    }
    if !agent.is_constructor_value(c) {
        return agent.type_error("the promise constructor is not a constructor");
    }
    let executor = closure(agent, capability_executor, "", 2, 2)?;
    let promise = agent.construct(c, &[Value::object(executor)])?;
    let c = captures(agent, executor);
    let (resolve, reject) = (c[0], c[1]);
    if !agent.is_callable(resolve) || !agent.is_callable(reject) {
        return agent.type_error("the promise capability functions are not callable");
    }
    Ok(Capability {
        promise,
        resolve,
        reject,
    })
}

/// `GetCapabilitiesExecutor` functions (§27.2.1.5.1).
fn capability_executor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let f = me(agent);
    let c = captures(agent, f);
    if c[0] != Value::UNDEFINED || c[1] != Value::UNDEFINED {
        return agent.type_error("the promise executor was already called");
    }
    set_captures(agent, f, &[arg(args, 0), arg(args, 1)]);
    Ok(Value::UNDEFINED)
}

impl Capability {
    pub(super) fn resolve(self, agent: &mut Agent, value: Value) -> JsResult<()> {
        if self.resolve == Value::UNDEFINED {
            let promise = self.promise.as_object().expect("an intrinsic promise");
            resolve_promise(agent, promise, value)
        } else {
            agent
                .call(self.resolve, Value::UNDEFINED, &[value])
                .map(drop)
        }
    }

    pub(super) fn reject(self, agent: &mut Agent, reason: Value) -> JsResult<()> {
        if self.resolve == Value::UNDEFINED {
            let promise = self.promise.as_object().expect("an intrinsic promise");
            settle(agent, promise, reason, false);
            Ok(())
        } else {
            agent
                .call(self.reject, Value::UNDEFINED, &[reason])
                .map(drop)
        }
    }

    /// `IfAbruptRejectPromise`: turn a thrown error into a rejection.
    fn settle_abrupt(self, agent: &mut Agent, result: JsResult<Value>) -> JsResult<Value> {
        match result {
            Ok(v) => Ok(v),
            Err(Abrupt::Throw(e)) => {
                self.reject(agent, e)?;
                Ok(self.promise)
            }
            Err(other) => Err(other),
        }
    }

    /// The reject function as a value script can call (`Promise.all`
    /// passes it to `then`), creating resolving functions for an
    /// intrinsic promise on first need.
    fn reject_value(&mut self, agent: &mut Agent) -> JsResult<Value> {
        if self.resolve == Value::UNDEFINED {
            let promise = self.promise.as_object().expect("an intrinsic promise");
            let (resolve, reject) = resolving_functions(agent, promise)?;
            self.resolve = resolve;
            self.reject = reject;
        }
        Ok(self.reject)
    }

    fn resolve_value(&mut self, agent: &mut Agent) -> JsResult<Value> {
        self.reject_value(agent)?;
        Ok(self.resolve)
    }
}

/// `PerformPromiseThen(promise, onFulfilled, onRejected, capability)`
/// (§27.2.5.4.1).
pub(super) fn perform_then(
    agent: &mut Agent,
    promise: ObjectId,
    on_fulfilled: Value,
    on_rejected: Value,
    capability: Option<Capability>,
) -> JsResult<()> {
    let callable = |agent: &Agent, f: Value| {
        if agent.is_callable(f) {
            f
        } else {
            Value::UNDEFINED
        }
    };
    let (promise_value, resolve, reject) = match capability {
        Some(c) => (c.promise, c.resolve, c.reject),
        None => (Value::UNDEFINED, Value::UNDEFINED, Value::UNDEFINED),
    };
    let fulfill = Reaction {
        handler: callable(agent, on_fulfilled),
        kind: crate::runtime::object::ReactionKind::Call,
        promise: promise_value,
        resolve,
        reject,
    };
    let rejected = Reaction {
        handler: callable(agent, on_rejected),
        ..fulfill
    };
    add_reactions(agent, promise, fulfill, rejected)
}

/// Attach a pair of reactions, or queue the matching one if `promise` is
/// settled.
fn add_reactions(
    agent: &mut Agent,
    promise: ObjectId,
    fulfill: Reaction,
    reject: Reaction,
) -> JsResult<()> {
    let state = record(agent, promise).map(|p| (p.state, p.result));
    let Some((state, result)) = state else {
        return Ok(());
    };
    match state {
        PromiseState::Pending => {
            agent.heap.charge(2 * std::mem::size_of::<Reaction>())?;
            let p = record(agent, promise).expect("checked above");
            p.fulfill_reactions.push(fulfill);
            p.reject_reactions.push(reject);
        }
        PromiseState::Fulfilled => agent.vm.jobs.push_back(Job::Reaction {
            reaction: fulfill,
            fulfilled: true,
            argument: result,
        }),
        PromiseState::Rejected => agent.vm.jobs.push_back(Job::Reaction {
            reaction: reject,
            fulfilled: false,
            argument: result,
        }),
    }
    let mut newly_handled = false;
    if let Some(p) = record(agent, promise) {
        newly_handled = p.state == PromiseState::Rejected && !p.handled;
        p.handled = true;
    }
    if newly_handled {
        // `HostPromiseRejectionTracker(promise, "handle")`. A rejection
        // the host has not taken yet is withdrawn instead, as HTML's
        // about-to-be-notified list does, so the list holds at most one
        // entry per promise that is unhandled or was reported.
        let pending = agent
            .vm
            .rejections
            .iter()
            .position(|e| e.promise == promise && e.operation == RejectionOperation::Reject);
        match pending {
            Some(at) => {
                agent.vm.rejections.remove(at);
            }
            None => agent.vm.rejections.push(RejectionEvent {
                operation: RejectionOperation::Handle,
                promise,
            }),
        }
    }
    Ok(())
}

/// `Await(value)` steps 2 to 9 (§27.7.5.3): resolve `value` to an
/// intrinsic promise and resume `state` (the suspended async function)
/// when it settles.
pub(crate) fn await_value(agent: &mut Agent, value: Value, state: ObjectId) -> JsResult<()> {
    let intrinsic = Value::object(agent.intrinsics.promise);
    let promise = promise_resolve(agent, intrinsic, value)?;
    let promise = promise
        .as_object()
        .expect("PromiseResolve returns a promise");
    let reaction = Reaction {
        handler: Value::object(state),
        kind: crate::runtime::object::ReactionKind::Await,
        promise: Value::UNDEFINED,
        resolve: Value::UNDEFINED,
        reject: Value::UNDEFINED,
    };
    add_reactions(agent, promise, reaction, reaction)
}

pub(crate) fn watch_module(
    agent: &mut Agent,
    promise: ObjectId,
    graph: ObjectId,
    index: u32,
) -> JsResult<()> {
    let reaction = Reaction {
        handler: Value::object(graph),
        kind: crate::runtime::object::ReactionKind::Module(index),
        promise: Value::UNDEFINED,
        resolve: Value::UNDEFINED,
        reject: Value::UNDEFINED,
    };
    add_reactions(agent, promise, reaction, reaction)
}

pub(crate) fn watch_import(
    agent: &mut Agent,
    completion: ObjectId,
    promise: ObjectId,
    namespace: ObjectId,
) -> JsResult<()> {
    let reaction = Reaction {
        handler: Value::object(namespace),
        kind: crate::runtime::object::ReactionKind::Import,
        promise: Value::object(promise),
        resolve: Value::UNDEFINED,
        reject: Value::UNDEFINED,
    };
    add_reactions(agent, completion, reaction, reaction)
}

/// A new intrinsic promise and its resolving operations, for an async
/// function.
pub(crate) fn new_intrinsic_promise(agent: &mut Agent) -> JsResult<ObjectId> {
    let prototype = agent.intrinsics.promise_prototype;
    new_promise(agent, prototype)
}

/// `PromiseResolve(C, x)` (§27.2.4.7.1).
pub(super) fn promise_resolve(agent: &mut Agent, c: Value, x: Value) -> JsResult<Value> {
    if is_promise(agent, x) {
        let o = x.as_object().expect("a promise is an object");
        let constructor = agent.get(o, PropertyKey::String(agent.names.constructor))?;
        if constructor == c {
            return Ok(x);
        }
    }
    let capability = new_capability(agent, c)?;
    capability.resolve(agent, x)?;
    Ok(capability.promise)
}

/// `SpeciesConstructor(O, %Promise%)` (§7.3.22).
fn species_constructor(agent: &mut Agent, o: ObjectId) -> JsResult<Value> {
    let default = Value::object(agent.intrinsics.promise);
    let c = agent.get(o, PropertyKey::String(agent.names.constructor))?;
    if c == Value::UNDEFINED {
        return Ok(default);
    }
    let Some(c) = c.as_object() else {
        return agent.type_error("the constructor property is not an object");
    };
    let s = agent.get(c, PropertyKey::Symbol(agent.symbols.species))?;
    if s.is_nullish() {
        return Ok(default);
    }
    if agent.is_constructor_value(s) {
        return Ok(s);
    }
    agent.type_error("the species is not a constructor")
}

// ---- jobs --------------------------------------------------------------------

impl Agent {
    /// React to a promise or thenable using an intrinsic promise and a native
    /// fulfillment handler. Its one captured value is traced by the JS heap.
    pub fn react_to_host_value(
        &mut self,
        source: Value,
        handler: NativeFn,
        context: Value,
    ) -> JsResult<Value> {
        let intrinsic = Value::object(self.intrinsics.promise);
        let result = new_capability(self, intrinsic)?;
        let function = closure(self, handler, "", 1, 1)?;
        set_capture(self, function, 0, context);
        let result_root = self.root(result.promise)?;
        let handler_value = Value::object(function);
        let handler_root = match self.root(handler_value) {
            Ok(root) => root,
            Err(error) => {
                self.unroot(result_root);
                return Err(error.into());
            }
        };
        let attached = (|| {
            let source = promise_resolve(self, intrinsic, source)?;
            let promise = source.as_object().expect("intrinsic promise");
            perform_then(self, promise, handler_value, Value::UNDEFINED, Some(result))?;
            Ok(result.promise)
        })();
        let outcome = result.settle_abrupt(self, attached);
        self.unroot(handler_root);
        self.unroot(result_root);
        outcome
    }

    /// The captured value of a currently running host promise reaction.
    pub fn host_reaction_context(&self) -> Value {
        self.active_function()
            .and_then(|function| match self.heap.object(function).internal() {
                Some(Internal::Captures(values)) => values.first().copied(),
                _ => None,
            })
            .unwrap_or(Value::UNDEFINED)
    }

    /// A new pending `%Promise%` that only the host settles, for host
    /// operations such as `fetch()` (Web IDL "a new promise").
    pub fn new_host_promise(&mut self) -> JsResult<ObjectId> {
        new_intrinsic_promise(self)
    }

    /// Web IDL "resolve" a host promise with `value` (thenables are
    /// adopted in a later job). A settled promise is left unchanged.
    pub fn resolve_host_promise(&mut self, promise: ObjectId, value: Value) -> JsResult<()> {
        if self.promise_rejected(promise).is_some() {
            return Ok(());
        }
        resolve_promise(self, promise, value)
    }

    /// Web IDL "reject" a host promise. A settled promise is unchanged.
    pub fn reject_host_promise(&mut self, promise: ObjectId, reason: Value) {
        if self.promise_rejected(promise).is_none() {
            reject_promise(self, promise, reason);
        }
    }

    /// `JSON.parse` of UTF-16 text without a reviver (Fetch's "parse JSON
    /// from bytes").
    pub fn parse_json(&mut self, text: &[u16]) -> JsResult<Value> {
        super::json::parse_json_units(self, text)
    }

    /// The rejection tracker calls since the last call of this function,
    /// oldest first: a promise rejected without a handler, or a handler
    /// added to a promise reported earlier. A rejection that got its
    /// handler before the host asked is not reported at all. Call it
    /// after [`Agent::run_jobs`], as a microtask checkpoint does; the
    /// promises stay alive until then.
    pub fn take_rejection_events(&mut self) -> Vec<RejectionEvent> {
        std::mem::take(&mut self.vm.rejections)
    }

    /// `[[PromiseResult]]` of a settled promise (the reason of a rejected
    /// one), or `None` if `promise` is pending or not a promise.
    pub fn promise_result(&self, promise: ObjectId) -> Option<Value> {
        match self.heap.object(promise).internal() {
            Some(Internal::Promise(p)) if p.state != PromiseState::Pending => Some(p.result),
            _ => None,
        }
    }

    /// Whether the settled promise rejected, or `None` while pending.
    pub fn promise_rejected(&self, promise: ObjectId) -> Option<bool> {
        match self.heap.object(promise).internal() {
            Some(Internal::Promise(p)) if p.state != PromiseState::Pending => {
                Some(p.state == PromiseState::Rejected)
            }
            _ => None,
        }
    }

    /// Queue `callback(argument)` behind the jobs already waiting, as HTML's
    /// "queue a microtask" does. A value thrown by the callback is discarded,
    /// like a reaction job without a derived promise; a host that must report
    /// errors passes a native callback that catches them itself.
    pub fn enqueue_microtask(
        &mut self,
        callback: Value,
        argument: Value,
    ) -> Result<(), crate::runtime::OutOfMemory> {
        self.vm
            .jobs
            .try_reserve(1)
            .map_err(|_| crate::runtime::OutOfMemory)?;
        self.vm.jobs.push_back(Job::Reaction {
            reaction: Reaction {
                handler: callback,
                kind: crate::runtime::object::ReactionKind::Call,
                promise: Value::UNDEFINED,
                resolve: Value::UNDEFINED,
                reject: Value::UNDEFINED,
            },
            fulfilled: true,
            argument,
        });
        Ok(())
    }

    /// `[[PromiseIsHandled]]` of a promise, or `None` if `promise` is not one.
    pub fn promise_is_handled(&self, promise: ObjectId) -> Option<bool> {
        match self.heap.object(promise).internal() {
            Some(Internal::Promise(p)) => Some(p.handled),
            _ => None,
        }
    }

    /// Whether promise jobs are waiting.
    pub fn has_pending_jobs(&self) -> bool {
        !self.vm.jobs.is_empty()
    }

    /// Run queued promise jobs until none is left, including the ones the
    /// jobs queue (a microtask checkpoint). Call it from the host, not
    /// from native code: jobs collect garbage at their safe points.
    /// Errors thrown inside a job settle promises and never end the run;
    /// only running out of memory or being cancelled does.
    pub fn run_jobs(&mut self) -> JsResult<()> {
        self.drain_jobs(None).map(|_| ())
    }

    /// Run up to `limit` jobs in FIFO order, counting jobs queued by other jobs.
    /// Returns `true` if the checkpoint drained the queue. A `false` result leaves
    /// the unrun jobs queued. As with [`Self::run_jobs`], call at a host safe point.
    pub fn run_jobs_limited(&mut self, limit: usize) -> JsResult<bool> {
        self.drain_jobs(Some(limit))
    }

    fn drain_jobs(&mut self, mut remaining: Option<usize>) -> JsResult<bool> {
        while !self.vm.jobs.is_empty() {
            if self
                .cancellation()
                .is_some_and(|token| token.is_cancelled())
            {
                return Err(Abrupt::Cancelled);
            }
            if let Some(left) = &mut remaining {
                if *left == 0 {
                    return Ok(false);
                }
                *left -= 1;
            }
            // Native-only promise chains may never enter bytecode. Collect
            // while the next job and all of its arguments are still queued.
            if self.should_collect() {
                self.collect_at_safe_point()?;
            }
            let job = self.vm.jobs.pop_front().expect("a queued job");
            self.vm.running_job = Some(job);
            let result = self.run_job(job);
            self.vm.running_job = None;
            match result {
                Ok(()) | Err(Abrupt::Throw(_)) => {}
                Err(other) => return Err(other),
            }
        }
        Ok(true)
    }

    fn run_job(&mut self, job: Job) -> JsResult<()> {
        match job {
            Job::Import {
                promise,
                request,
                referrer,
            } => self.run_import_job(
                promise.as_object().expect("import promise"),
                request.as_object().expect("import request"),
                referrer.as_string(),
            ),
            Job::Reaction {
                reaction,
                fulfilled,
                argument,
            } => {
                use crate::runtime::object::ReactionKind;
                match reaction.kind {
                    ReactionKind::Call => {}
                    ReactionKind::Await => {
                        let state = reaction
                            .handler
                            .as_object()
                            .expect("an async function state");
                        return self.async_resume(state, !fulfilled, argument);
                    }
                    ReactionKind::Module(index) => {
                        let graph = reaction.handler.as_object().expect("an async module graph");
                        return self.resume_module_evaluation(
                            graph,
                            index as usize,
                            fulfilled,
                            argument,
                        );
                    }
                    ReactionKind::Import => {
                        let promise = reaction.promise.as_object().expect("import promise");
                        if fulfilled {
                            return resolve_promise(self, promise, reaction.handler);
                        }
                        reject_promise(self, promise, argument);
                        return Ok(());
                    }
                }
                let result = if reaction.handler == Value::UNDEFINED {
                    if fulfilled {
                        Ok(argument)
                    } else {
                        Err(Abrupt::Throw(argument))
                    }
                } else {
                    self.call_from_host(reaction.handler, Value::UNDEFINED, &[argument])
                };
                if reaction.promise == Value::UNDEFINED {
                    return result.map(drop);
                }
                let capability = Capability {
                    promise: reaction.promise,
                    resolve: reaction.resolve,
                    reject: reaction.reject,
                };
                match result {
                    Ok(v) => capability.resolve(self, v),
                    Err(Abrupt::Throw(e)) => capability.reject(self, e),
                    Err(other) => Err(other),
                }
            }
            Job::ResolveThenable {
                promise,
                thenable,
                then,
            } => {
                let o = promise.as_object().expect("a promise");
                let (resolve, reject) = resolving_functions(self, o)?;
                self.vm.running_job = Some(Job::ResolveThenable {
                    promise: resolve,
                    thenable,
                    then: reject,
                });
                match self.call_from_host(then, thenable, &[resolve, reject]) {
                    Ok(_) => Ok(()),
                    Err(Abrupt::Throw(e)) => self.call(reject, Value::UNDEFINED, &[e]).map(drop),
                    Err(other) => Err(other),
                }
            }
        }
    }
}

// ---- constructor and prototype -----------------------------------------------

/// `Promise(executor)` (§27.2.3.1).
fn promise_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if agent.new_target().is_none() {
        return agent.type_error("Promise must be called with new");
    }
    let executor = arg(args, 0);
    if !agent.is_callable(executor) {
        return agent.type_error("the Promise executor is not callable");
    }
    let default = agent.intrinsics.promise_prototype;
    let prototype = prototype_from_new_target(agent, default)?;
    let promise = new_promise(agent, prototype)?;
    let (resolve, reject) = resolving_functions(agent, promise)?;
    if let Err(abrupt) = agent.call(executor, Value::UNDEFINED, &[resolve, reject]) {
        let Abrupt::Throw(e) = abrupt else {
            return Err(abrupt);
        };
        agent.call(reject, Value::UNDEFINED, &[e])?;
    }
    Ok(Value::object(promise))
}

/// `Promise.prototype.then(onFulfilled, onRejected)` (§27.2.5.4).
fn promise_then(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    if !is_promise(agent, this) {
        return agent.type_error("Promise.prototype.then called on a non-promise");
    }
    let promise = this.as_object().expect("a promise is an object");
    let c = species_constructor(agent, promise)?;
    let capability = new_capability(agent, c)?;
    perform_then(agent, promise, arg(args, 0), arg(args, 1), Some(capability))?;
    Ok(capability.promise)
}

/// `Promise.prototype.catch(onRejected)` (§27.2.5.1).
fn promise_catch(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let then = agent.heap.key("then")?;
    agent.invoke(this, then, &[Value::UNDEFINED, arg(args, 0)])
}

/// `Promise.prototype.finally(onFinally)` (§27.2.5.3).
fn promise_finally(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(promise) = this.as_object() else {
        return agent.type_error("Promise.prototype.finally called on a non-object");
    };
    let c = species_constructor(agent, promise)?;
    let on_finally = arg(args, 0);
    let (then_finally, catch_finally) = if agent.is_callable(on_finally) {
        let then_finally = closure(agent, then_finally, "", 1, 2)?;
        set_captures(agent, then_finally, &[on_finally, c]);
        let catch_finally = closure(agent, catch_finally, "", 1, 2)?;
        set_captures(agent, catch_finally, &[on_finally, c]);
        (Value::object(then_finally), Value::object(catch_finally))
    } else {
        (on_finally, on_finally)
    };
    let then = agent.heap.key("then")?;
    agent.invoke(this, then, &[then_finally, catch_finally])
}

fn finally_step(agent: &mut Agent, value: Value, thrower: bool) -> JsResult<Value> {
    let c = captures(agent, me(agent));
    let (on_finally, constructor) = (c[0], c[1]);
    let result = agent.call(on_finally, Value::UNDEFINED, &[])?;
    let promise = promise_resolve(agent, constructor, result)?;
    let thunk = closure(
        agent,
        if thrower { thrower_fn } else { value_thunk },
        "",
        0,
        1,
    )?;
    set_captures(agent, thunk, &[value]);
    let then = agent.heap.key("then")?;
    agent.invoke(promise, then, &[Value::object(thunk)])
}

fn then_finally(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    finally_step(agent, arg(args, 0), false)
}

fn catch_finally(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    finally_step(agent, arg(args, 0), true)
}

fn value_thunk(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(captures(agent, me(agent))[0])
}

fn thrower_fn(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    Err(Abrupt::Throw(captures(agent, me(agent))[0]))
}

// ---- statics -----------------------------------------------------------------

/// `Promise.resolve(x)` (§27.2.4.7).
fn promise_resolve_static(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    if this.as_object().is_none() {
        return agent.type_error("Promise.resolve called on a non-object");
    }
    promise_resolve(agent, this, arg(args, 0))
}

/// `Promise.reject(r)` (§27.2.4.6).
fn promise_reject(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let capability = new_capability(agent, this)?;
    capability.reject(agent, arg(args, 0))?;
    Ok(capability.promise)
}

/// `Promise.withResolvers()` (§27.2.4.8).
fn promise_with_resolvers(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let mut capability = new_capability(agent, this)?;
    let resolve = capability.resolve_value(agent)?;
    let reject = capability.reject;
    let o = agent.new_object()?;
    for (name, value) in [
        ("promise", capability.promise),
        ("resolve", resolve),
        ("reject", reject),
    ] {
        let key = agent.heap.key(name)?;
        agent.create_data_property_or_throw(o, key, value)?;
    }
    Ok(Value::object(o))
}

/// `Promise.try(callback, ...args)` (§27.2.4.8, ES2025).
fn promise_try(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    if this.as_object().is_none() {
        return agent.type_error("Promise.try called on a non-object");
    }
    let capability = new_capability(agent, this)?;
    let rest = args.get(1..).unwrap_or(&[]);
    match agent.call(arg(args, 0), Value::UNDEFINED, rest) {
        // A promise of the same constructor is returned as is, not
        // wrapped in another.
        Ok(v) if is_promise(agent, v) => {
            let o = v.as_object().expect("a promise is an object");
            let constructor = agent.get(o, PropertyKey::String(agent.names.constructor))?;
            if constructor == this {
                return Ok(v);
            }
            capability.resolve(agent, v)?;
        }
        Ok(v) => capability.resolve(agent, v)?,
        Err(Abrupt::Throw(e)) => capability.reject(agent, e)?,
        Err(other) => return Err(other),
    }
    Ok(capability.promise)
}

/// Which combinator a loop runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Combinator {
    All,
    AllSettled,
    Any,
    Race,
}

fn promise_all(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    combinator(agent, this, arg(args, 0), Combinator::All)
}

fn promise_all_settled(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    combinator(agent, this, arg(args, 0), Combinator::AllSettled)
}

fn promise_any(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    combinator(agent, this, arg(args, 0), Combinator::Any)
}

fn promise_race(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    combinator(agent, this, arg(args, 0), Combinator::Race)
}

/// `Promise.all`, `allSettled`, `any` and `race` (§27.2.4.1-5): the
/// shared prologue, the loop over the iterable and the error handling.
fn combinator(agent: &mut Agent, c: Value, iterable: Value, kind: Combinator) -> JsResult<Value> {
    let mut capability = new_capability(agent, c)?;
    // GetPromiseResolve(C).
    let resolve_key = PropertyKey::String(agent.heap.intern_str("resolve")?);
    let promise_resolve = match c.as_object() {
        Some(o) => agent.get(o, resolve_key),
        None => agent.type_error("the promise constructor is not an object"),
    };
    let promise_resolve = match promise_resolve {
        Ok(f) if agent.is_callable(f) => f,
        Ok(_) => {
            let error = agent.type_error::<Value>("the resolve method is not callable");
            return capability.settle_abrupt(agent, error);
        }
        Err(e) => return capability.settle_abrupt(agent, Err(e)),
    };
    let key = PropertyKey::Symbol(agent.symbols.iterator);
    let method = match agent.get_method(iterable, key) {
        Ok(Some(m)) => m,
        Ok(None) => {
            let error = agent.type_error::<Value>("value is not iterable");
            return capability.settle_abrupt(agent, error);
        }
        Err(e) => return capability.settle_abrupt(agent, Err(e)),
    };
    // The shared state of the element functions: the values list (an
    // array never given to script) and the remaining-elements counter.
    let values = Value::object(agent.create_array_from_list(&[])?);
    let counter = closure(agent, value_thunk, "", 0, 1)?;
    set_captures(agent, counter, &[Value::number(1.0)]);
    let resolve = capability.resolve_value(agent)?;
    let reject = capability.reject;
    let mut index = 0u32;
    let then = agent.heap.key("then")?;
    let looped = iterate(agent, iterable, method, |agent, next| {
        let next_promise = agent.call(promise_resolve, c, &[next])?;
        let (on_fulfilled, on_rejected) = match kind {
            Combinator::Race => (resolve, reject),
            Combinator::All | Combinator::AllSettled | Combinator::Any => {
                let values_array = values.as_object().expect("the values array");
                agent.create_data_property_or_throw(
                    values_array,
                    PropertyKey::Index(index),
                    Value::UNDEFINED,
                )?;
                let remaining = captures(agent, counter)[0].as_number().unwrap_or(0.0);
                set_capture(agent, counter, 0, Value::number(remaining + 1.0));
                // Captures: index, values, capability resolve or reject,
                // counter, already-called flag holder.
                let already = closure(agent, value_thunk, "", 0, 1)?;
                set_captures(agent, already, &[Value::FALSE]);
                let element = |agent: &mut Agent, behavior: NativeFn, target: Value| {
                    let f = closure(agent, behavior, "", 1, 5)?;
                    set_captures(
                        agent,
                        f,
                        &[
                            Value::number(f64::from(index)),
                            values,
                            target,
                            Value::object(counter),
                            Value::object(already),
                        ],
                    );
                    Ok::<Value, Abrupt>(Value::object(f))
                };
                match kind {
                    Combinator::All => (element(agent, all_resolve_element, resolve)?, reject),
                    Combinator::AllSettled => (
                        element(agent, settled_fulfilled_element, resolve)?,
                        element(agent, settled_rejected_element, resolve)?,
                    ),
                    _ => (resolve, element(agent, any_reject_element, reject)?),
                }
            }
        };
        index = index.checked_add(1).ok_or_else(|| {
            agent.throw(crate::runtime::ErrorKind::RangeError, "too many promises")
        })?;
        agent.invoke(next_promise, then, &[on_fulfilled, on_rejected])?;
        Ok(true)
    });
    if let Err(e) = looped {
        return capability.settle_abrupt(agent, Err(e));
    }
    if kind != Combinator::Race {
        // The loop is done: drop the initial count.
        let remaining = captures(agent, counter)[0].as_number().unwrap_or(0.0) - 1.0;
        set_capture(agent, counter, 0, Value::number(remaining));
        if remaining == 0.0 {
            let list = list_of(agent, values)?;
            let array = Value::object(agent.create_array_from_list(&list)?);
            let done = if kind == Combinator::Any {
                let error = super::error::new_aggregate_error(agent, array)?;
                capability.reject(agent, error)
            } else {
                capability.resolve(agent, array)
            };
            if let Err(e) = done {
                return capability.settle_abrupt(agent, Err(e));
            }
        }
    }
    Ok(capability.promise)
}

/// The elements of the internal values array.
fn list_of(agent: &mut Agent, values: Value) -> JsResult<Vec<Value>> {
    let o = values.as_object().expect("the values array");
    let length = agent.heap.object(o).length;
    let mut list = Vec::with_capacity(length as usize);
    for i in 0..length {
        list.push(agent.get(o, PropertyKey::Index(i))?);
    }
    Ok(list)
}

/// The shared part of the element functions: check and set the
/// already-called flag, store `value` at the element's index, and when
/// it was the last one return the finished values array.
fn element_step(agent: &mut Agent, value: Value) -> JsResult<Option<(Value, Value)>> {
    let c = captures(agent, me(agent));
    let (index, values, target, counter, already) = (c[0], c[1], c[2], c[3], c[4]);
    let already = already.as_object().expect("the already-called flag");
    if captures(agent, already)[0] == Value::TRUE {
        return Ok(None);
    }
    set_capture(agent, already, 0, Value::TRUE);
    let values_array = values.as_object().expect("the values array");
    let index = index.as_number().unwrap_or(0.0) as u32;
    agent.create_data_property_or_throw(values_array, PropertyKey::Index(index), value)?;
    let counter = counter.as_object().expect("the counter");
    let remaining = captures(agent, counter)[0].as_number().unwrap_or(0.0) - 1.0;
    set_capture(agent, counter, 0, Value::number(remaining));
    if remaining != 0.0 {
        return Ok(None);
    }
    let list = list_of(agent, values)?;
    let array = agent.create_array_from_list(&list)?;
    Ok(Some((Value::object(array), target)))
}

/// `Promise.all` resolve element functions (§27.2.4.1.3).
fn all_resolve_element(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if let Some((array, resolve)) = element_step(agent, arg(args, 0))? {
        return agent.call(resolve, Value::UNDEFINED, &[array]);
    }
    Ok(Value::UNDEFINED)
}

fn settled_record(agent: &mut Agent, fulfilled: bool, value: Value) -> JsResult<Value> {
    let o = agent.new_object()?;
    let status = agent.string(if fulfilled { "fulfilled" } else { "rejected" })?;
    let status_key = agent.heap.key("status")?;
    agent.create_data_property_or_throw(o, status_key, status)?;
    let key = if fulfilled {
        PropertyKey::String(agent.names.value)
    } else {
        agent.heap.key("reason")?
    };
    agent.create_data_property_or_throw(o, key, value)?;
    Ok(Value::object(o))
}

/// `Promise.allSettled` resolve element functions (§27.2.4.2.2).
fn settled_fulfilled_element(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let c = captures(agent, me(agent));
    let already = c[4].as_object().expect("the already-called flag");
    if captures(agent, already)[0] == Value::TRUE {
        return Ok(Value::UNDEFINED);
    }
    let record = settled_record(agent, true, arg(args, 0))?;
    if let Some((array, resolve)) = element_step(agent, record)? {
        return agent.call(resolve, Value::UNDEFINED, &[array]);
    }
    Ok(Value::UNDEFINED)
}

/// `Promise.allSettled` reject element functions (§27.2.4.2.3).
fn settled_rejected_element(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let c = captures(agent, me(agent));
    let already = c[4].as_object().expect("the already-called flag");
    if captures(agent, already)[0] == Value::TRUE {
        return Ok(Value::UNDEFINED);
    }
    let record = settled_record(agent, false, arg(args, 0))?;
    if let Some((array, resolve)) = element_step(agent, record)? {
        return agent.call(resolve, Value::UNDEFINED, &[array]);
    }
    Ok(Value::UNDEFINED)
}

/// `Promise.any` reject element functions (§27.2.4.3.2).
fn any_reject_element(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if let Some((errors, reject)) = element_step(agent, arg(args, 0))? {
        let error = super::error::new_aggregate_error(agent, errors)?;
        return agent.call(reject, Value::UNDEFINED, &[error]);
    }
    Ok(Value::UNDEFINED)
}
