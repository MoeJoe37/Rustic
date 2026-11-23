//! Object storage (ADR 0015): a shape (or a private dictionary) for named
//! properties, a slot vector for their values, and dense or sparse indexed
//! elements. The internal methods that read and change this storage are in
//! `internal.rs`.

use std::collections::{BTreeMap, HashMap};

use super::agent::NativeFn;
use super::env::EnvId;
use super::shape::{Attributes, PropertyKey, ROOT_SHAPE, ShapeId};
use super::value::{ObjectId, StringId, Value};
use crate::vm::bytecode::CodeId;

/// What kind of object this is, for exotic behavior and brand checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// An ordinary object.
    Ordinary,
    /// An Array exotic object (§10.4.2).
    Array,
    /// A function object (native until the interpreter exists).
    Function,
    /// An Error instance (has `[[ErrorData]]`).
    Error,
    /// A Boolean object (`[[BooleanData]]` in the primitive slot).
    Boolean,
    /// A Number object.
    Number,
    /// A String exotic object (§10.4.3).
    String,
    /// A Symbol object.
    Symbol,
    /// A BigInt object.
    BigInt,
    /// A host object: its meaning and outgoing edges belong to the
    /// embedder (a DOM wrapper, for example).
    Host,
    /// A WeakRef (§26.1).
    WeakRef,
    /// A WeakMap (§24.3).
    WeakMap,
    /// A WeakSet (§24.4).
    WeakSet,
    /// A FinalizationRegistry (§26.2).
    FinalizationRegistry,
    /// A Map (§24.1).
    Map,
    /// A Set (§24.2).
    Set,
    /// A Date (`[[DateValue]]` in the primitive slot, §21.4).
    Date,
    /// An arguments object (§10.4.4), mapped or not.
    Arguments,
    /// An ArrayBuffer (§25.1).
    ArrayBuffer,
    /// A typed array (§23.2).
    TypedArray,
    /// A DataView (§25.3).
    DataView,
    /// A module namespace exotic object (§10.4.6).
    ModuleNamespace,
}

/// A complete or partial property descriptor (§6.2.6).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PropertyDescriptor {
    /// `[[Value]]`.
    pub value: Option<Value>,
    /// `[[Writable]]`.
    pub writable: Option<bool>,
    /// `[[Get]]` (undefined or a callable object).
    pub get: Option<Value>,
    /// `[[Set]]` (undefined or a callable object).
    pub set: Option<Value>,
    /// `[[Enumerable]]`.
    pub enumerable: Option<bool>,
    /// `[[Configurable]]`.
    pub configurable: Option<bool>,
}

impl PropertyDescriptor {
    /// A writable, enumerable, configurable data property.
    pub fn data(value: Value) -> Self {
        Self::data_with(value, true, true, true)
    }

    /// A data property with explicit attributes.
    pub fn data_with(value: Value, writable: bool, enumerable: bool, configurable: bool) -> Self {
        Self {
            value: Some(value),
            writable: Some(writable),
            get: None,
            set: None,
            enumerable: Some(enumerable),
            configurable: Some(configurable),
        }
    }

    /// An accessor property.
    pub fn accessor(get: Value, set: Value, enumerable: bool, configurable: bool) -> Self {
        Self {
            value: None,
            writable: None,
            get: Some(get),
            set: Some(set),
            enumerable: Some(enumerable),
            configurable: Some(configurable),
        }
    }

    /// `IsAccessorDescriptor`.
    pub fn is_accessor(&self) -> bool {
        self.get.is_some() || self.set.is_some()
    }

    /// `IsDataDescriptor`.
    pub fn is_data(&self) -> bool {
        self.value.is_some() || self.writable.is_some()
    }

    /// `IsGenericDescriptor`.
    pub fn is_generic(&self) -> bool {
        !self.is_accessor() && !self.is_data()
    }

    pub(crate) fn from_stored(attributes: Attributes, first: Value, second: Value) -> Self {
        let enumerable = attributes.has(Attributes::ENUMERABLE);
        let configurable = attributes.has(Attributes::CONFIGURABLE);
        if attributes.is_accessor() {
            Self::accessor(first, second, enumerable, configurable)
        } else {
            Self::data_with(
                first,
                attributes.has(Attributes::WRITABLE),
                enumerable,
                configurable,
            )
        }
    }

    /// The stored form of a complete descriptor.
    pub(crate) fn to_stored(self) -> (Attributes, Value, Value) {
        let mut bits = 0;
        if self.enumerable == Some(true) {
            bits |= Attributes::ENUMERABLE;
        }
        if self.configurable == Some(true) {
            bits |= Attributes::CONFIGURABLE;
        }
        if self.is_accessor() {
            bits |= Attributes::ACCESSOR;
            (
                Attributes::new(bits),
                self.get.unwrap_or(Value::UNDEFINED),
                self.set.unwrap_or(Value::UNDEFINED),
            )
        } else {
            if self.writable == Some(true) {
                bits |= Attributes::WRITABLE;
            }
            (
                Attributes::new(bits),
                self.value.unwrap_or(Value::UNDEFINED),
                Value::UNDEFINED,
            )
        }
    }
}

/// A stored property outside the shape tree: dictionary entries and sparse
/// elements.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stored {
    pub attributes: Attributes,
    /// The value, or the getter.
    pub first: Value,
    /// The setter (accessors only).
    pub second: Value,
}

/// Named properties of an object that left the shared shape tree, kept in
/// insertion order. Deleted entries leave a hole until compaction.
#[derive(Clone, Debug, Default)]
pub(crate) struct Dictionary {
    pub entries: Vec<Option<(PropertyKey, Stored)>>,
    pub index: HashMap<PropertyKey, u32>,
    pub holes: u32,
}

impl Dictionary {
    pub fn get(&self, key: PropertyKey) -> Option<Stored> {
        let at = *self.index.get(&key)?;
        self.entries[at as usize].map(|(_, s)| s)
    }

    pub fn set(&mut self, key: PropertyKey, stored: Stored) {
        match self.index.get(&key) {
            Some(&at) => self.entries[at as usize] = Some((key, stored)),
            None => {
                // Entry counts are bounded by the heap budget, far below 2³².
                self.index.insert(key, self.entries.len() as u32);
                self.entries.push(Some((key, stored)));
            }
        }
    }

    pub fn remove(&mut self, key: PropertyKey) {
        if let Some(at) = self.index.remove(&key) {
            self.entries[at as usize] = None;
            self.holes += 1;
            if self.holes as usize * 2 > self.entries.len() {
                self.entries.retain(Option::is_some);
                self.holes = 0;
                self.index = self
                    .entries
                    .iter()
                    .enumerate()
                    .filter_map(|(i, e)| e.map(|(k, _)| (k, i as u32)))
                    .collect();
            }
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = PropertyKey> + '_ {
        self.entries.iter().filter_map(|e| e.map(|(k, _)| k))
    }
}

/// Rarely needed per-object state, boxed so ordinary objects stay small.
#[derive(Clone, Debug, Default)]
pub(crate) struct Extra {
    /// Named properties after leaving the shape tree.
    pub dictionary: Option<Dictionary>,
    /// Indexed properties once any is not a plain data property or the
    /// dense vector would be mostly holes.
    pub sparse: Option<BTreeMap<u32, Stored>>,
    /// `[[BooleanData]]`, `[[NumberData]]`, `[[StringData]]` and friends.
    pub primitive: Option<Value>,
    /// The behavior of a native function.
    pub native: Option<NativeFn>,
    /// Internal slots of host and weak objects.
    pub internal: Option<Internal>,
}

/// One registration in a FinalizationRegistry (§26.2).
#[derive(Clone, Copy, Debug)]
pub(crate) struct RegistryCell {
    /// `[[WeakRefTarget]]`, held weakly.
    pub target: Value,
    /// `[[HeldValue]]`, held strongly.
    pub held: Value,
    /// `[[UnregisterToken]]`, held weakly; `UNINITIALIZED` when absent or
    /// collected.
    pub token: Value,
}

/// A function compiled from source: its code and what it closed over.
/// Closures live in their own arena, 24 bytes each, and the function
/// object names its closure through the `length` field; boxing them into
/// `Extra` would cost every function several times that.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Closure {
    /// The compiled function.
    pub code: CodeId,
    /// `[[Environment]]`: the scope the function was created in.
    pub env: EnvId,
    /// An arrow function's `this`, captured where it was created;
    /// `UNINITIALIZED` for every other function.
    pub this: Value,
    /// `[[HomeObject]]`, or undefined.
    pub home: Value,
}

/// What a built-in iterator walks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IteratorKind {
    /// The elements of an array-like, by index.
    Array,
    /// The indices of an array-like.
    ArrayKeys,
    /// `[index, element]` pairs of an array-like.
    ArrayEntries,
    /// The code points of a string.
    String,
    /// The keys of a Map or the values of a Set.
    TableKeys,
    /// The values of a Map or Set.
    TableValues,
    /// `[key, value]` pairs of a Map or `[value, value]` of a Set.
    TableEntries,
}

impl IteratorKind {
    /// Whether the iterator walks a Map or Set table.
    pub(crate) fn walks_table(self) -> bool {
        matches!(
            self,
            IteratorKind::TableKeys | IteratorKind::TableValues | IteratorKind::TableEntries
        )
    }
}

/// Internal slots the collector treats specially.
#[derive(Clone, Debug)]
pub(crate) enum Internal {
    /// The brand of a frozen JSON.rawJSON wrapper.
    RawJson,
    /// Canonical Unicode locale identifier of an Intl.Locale.
    Locale(StringId),
    /// Resolved collation settings and the cached compare callback.
    Collator(Box<super::builtins::intl::collator::Collator>),
    /// Locale data and digit options of Intl.PluralRules.
    PluralRules(Box<super::builtins::intl::plural_rules::PluralRules>),
    /// An ECMA-402 service object with settings that never change
    /// after construction (NumberFormat and the services built on it),
    /// and the bound function its accessor hands out, undefined until
    /// first read.
    Service {
        state: std::sync::Arc<dyn std::any::Any + Send + Sync>,
        bound: Value,
        /// What the settings charge the heap.
        bytes: u32,
    },
    /// The resolved locale and template set for Intl.ListFormat.
    ListFormat {
        locale: &'static str,
        group: usize,
        kind: usize,
        style: usize,
    },
    /// A built-in iterator: what it walks and the next position.
    /// `target` is undefined once the iterator is exhausted.
    Iterator {
        target: Value,
        index: u32,
        kind: IteratorKind,
    },
    /// A host object's embedder data.
    Host(u64),
    /// A read-only Web IDL indexed property getter and its source.
    HostIndexed(super::host_indexed::IndexedHost),
    /// A Web IDL named getter/setter/deleter and its source.
    HostNamed(super::host_named::NamedHost),
    /// The ordinary Window of a local browsing realm.
    WindowProxy(ObjectId),
    /// Private string state of a branded host exception.
    HostError {
        interface: &'static str,
        name: StringId,
        message: StringId,
    },
    /// A bound function exotic object (§10.4.1): `[[BoundTargetFunction]]`,
    /// `[[BoundThis]]` and `[[BoundArguments]]`.
    Bound {
        target: ObjectId,
        this: Value,
        args: Box<[Value]>,
    },
    /// `[[WeakRefTarget]]`; `UNINITIALIZED` once the target is collected.
    WeakRef(Value),
    /// `[[MapData]]` or `[[SetData]]` (a Set stores each value as its own
    /// key and value).
    Table(Box<super::table::OrderedTable>),
    /// `[[WeakMapData]]` or `[[WeakSetData]]` keyed by the key's bits. An
    /// entry is an ephemeron: its value is live only while its key is.
    WeakMap(HashMap<u64, Value>),
    /// `[[Cells]]`, and held values of collected targets waiting for the
    /// host to run the cleanup callback.
    Registry {
        cells: Vec<RegistryCell>,
        pending: Vec<Value>,
    },
    /// An iterator helper (§27.1.2.1) or an `Iterator.concat` iterator.
    Helper(Box<Helper>),
    /// `[[Iterated]]` of a `%WrapForValidIteratorPrototype%` object from
    /// `Iterator.from`: the iterator and its `next` method.
    Wrapped { iterator: Value, next: Value },
    /// `%AsyncFromSyncIteratorPrototype%` objects: the sync iterator and
    /// its `next` method (§27.1.6).
    AsyncFromSync { iterator: Value, next: Value },
    /// A RegExp object's `[[RegExpMatcher]]`, `[[OriginalSource]]` and
    /// `[[OriginalFlags]]` (strings).
    RegExp {
        matcher: std::rc::Rc<crate::regexp::Regex>,
        source: Value,
        flags: Value,
    },
    /// A `%RegExpStringIteratorPrototype%` object (§22.2.9).
    RegExpStringIterator {
        regexp: Value,
        string: Value,
        global: bool,
        unicode: bool,
        done: bool,
    },
    /// A generator object's suspended frame (§27.5), or an async
    /// function's while it awaits.
    Generator(Box<Generator>),
    /// A promise's internal slots (§27.2.6).
    Promise(Box<PromiseRecord>),
    /// Suspended evaluation state for a graph of source text modules.
    ModuleEvaluation(Box<crate::module::AsyncModuleGraph>),
    /// The specifier and attributes of a queued dynamic import.
    ImportRequest(Box<crate::module::ImportJob>),
    /// A Proxy exotic object (§10.5): `[[ProxyTarget]]` and
    /// `[[ProxyHandler]]`, both `None` once revoked.
    Proxy {
        target: Option<ObjectId>,
        handler: Option<ObjectId>,
    },
    /// The `[[ParameterMap]]` of a mapped arguments object (§10.4.4.7):
    /// for each index still mapped, the slot of the parameter in the
    /// function's environment.
    Arguments {
        env: EnvId,
        slots: Box<[Option<u16>]>,
    },
    /// Export keys in UTF-16 order and their live bindings.
    ModuleNamespace(Box<[(PropertyKey, NamespaceBinding)]>, EnvId),
    /// An ArrayBuffer's bytes (§25.1).
    ArrayBuffer(Box<super::builtins::buffer::BufferData>),
    /// A typed array's `[[ViewedArrayBuffer]]`, element kind,
    /// `[[ByteOffset]]` and `[[ArrayLength]]` (`None` tracks the buffer).
    TypedArray {
        buffer: ObjectId,
        kind: super::builtins::buffer::ElementKind,
        offset: usize,
        length: Option<usize>,
    },
    /// A DataView's buffer, `[[ByteOffset]]` and `[[ByteLength]]`.
    DataView {
        buffer: ObjectId,
        offset: usize,
        length: Option<usize>,
    },
    /// The values a built-in closure captures (the resolving functions of
    /// a promise, the element functions of `Promise.all`, ...); the
    /// behavior knows what each position means.
    Captures(Box<[Value]>),
}

/// A module namespace property points to a declaration slot or a namespace.
#[derive(Clone, Copy, Debug)]
pub(crate) enum NamespaceBinding {
    Local(EnvId, u16),
    Namespace(ObjectId),
}

impl NamespaceBinding {
    pub(crate) fn trace(self, tracer: &mut impl super::gc::Tracer) {
        match self {
            Self::Local(env, _) => tracer.env(env),
            Self::Namespace(module) => tracer.value(Value::object(module)),
        }
    }
}

/// `[[PromiseState]]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PromiseState {
    Pending,
    Fulfilled,
    Rejected,
}

/// A PromiseReaction Record (§27.2.1.2) with its capability.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Reaction {
    /// The handler: a function, undefined (pass the value on), or for an
    /// `await` the suspended async function's state object.
    pub handler: Value,
    pub kind: ReactionKind,
    /// The capability's promise, or undefined for none. When `resolve`
    /// is undefined the promise is an intrinsic promise that nothing else
    /// can settle, and the job settles it directly.
    pub promise: Value,
    pub resolve: Value,
    pub reject: Value,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ReactionKind {
    Call,
    Await,
    Module(u32),
    Import,
}

impl Reaction {
    pub(crate) fn values(&self) -> [Value; 4] {
        [self.handler, self.promise, self.resolve, self.reject]
    }
}

/// A promise's state and pending reactions.
#[derive(Clone, Debug)]
pub(crate) struct PromiseRecord {
    pub state: PromiseState,
    /// `[[PromiseResult]]`: undefined while pending.
    pub result: Value,
    pub fulfill_reactions: Vec<Reaction>,
    pub reject_reactions: Vec<Reaction>,
    /// `[[PromiseIsHandled]]`.
    pub handled: bool,
}

impl PromiseRecord {
    pub(crate) fn values(&self) -> impl Iterator<Item = Value> + '_ {
        std::iter::once(self.result).chain(
            self.fulfill_reactions
                .iter()
                .chain(&self.reject_reactions)
                .flat_map(Reaction::values),
        )
    }
}

/// A job waiting in the agent's queue (§9.5).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Job {
    /// A validated import request awaiting the host's loader.
    Import {
        promise: Value,
        request: Value,
        referrer: Value,
    },
    /// `NewPromiseReactionJob`: run `reaction` with `argument`.
    Reaction {
        reaction: Reaction,
        fulfilled: bool,
        argument: Value,
    },
    /// `NewPromiseResolveThenableJob`: call `then` on `thenable` with
    /// fresh resolving functions of `promise`.
    ResolveThenable {
        promise: Value,
        thenable: Value,
        then: Value,
    },
}

impl Job {
    pub(crate) fn values(&self) -> [Value; 6] {
        match *self {
            Job::Import {
                promise,
                request,
                referrer,
            } => [
                promise,
                request,
                referrer,
                Value::UNDEFINED,
                Value::UNDEFINED,
                Value::UNDEFINED,
            ],
            Job::Reaction {
                reaction, argument, ..
            } => {
                let [a, b, c, d] = reaction.values();
                [a, b, c, d, argument, Value::UNDEFINED]
            }
            Job::ResolveThenable {
                promise,
                thenable,
                then,
            } => [
                promise,
                thenable,
                then,
                Value::UNDEFINED,
                Value::UNDEFINED,
                Value::UNDEFINED,
            ],
        }
    }
}

/// `[[GeneratorState]]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GeneratorState {
    SuspendedStart,
    SuspendedYield,
    Executing,
    Completed,
    /// An async function or generator suspended in `await`: still
    /// executing as far as the generator methods are concerned.
    Awaiting,
    /// An async generator settling its queue after it finished, waiting
    /// for the promise of a `return(value)` request (`draining-queue`).
    AwaitingReturn,
}

/// One `next`, `throw` or `return` call waiting on an async generator
/// (an AsyncGeneratorRequest Record, §27.6.3.1): how it resumes, the
/// value sent in and the intrinsic promise the call returned.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AsyncRequest {
    pub mode: u8,
    pub value: Value,
    pub promise: ObjectId,
}

/// A generator's frame while it is not running: where it stopped and its
/// registers. While it runs, its registers live on the value stack and
/// the copy here is stale until the next `yield` writes it back.
#[derive(Clone, Debug)]
pub(crate) struct Generator {
    pub state: GeneratorState,
    pub code: CodeId,
    /// The op to resume at.
    pub pc: u32,
    pub env: EnvId,
    /// Environments the frame had pushed.
    pub depth: u16,
    pub this: Value,
    pub function: Value,
    pub new_target: Value,
    /// Where a resumption writes the value sent in and how it was sent
    /// (the `dst` and `mode` of the `yield`); `None` before the start.
    pub resume: Option<(u16, u16)>,
    /// An async function's promise; undefined for a generator.
    pub promise: Value,
    pub registers: Box<[Value]>,
    /// Whether this is an async generator, and its queue of requests;
    /// the first one is the request being served.
    pub async_generator: bool,
    pub queue: std::collections::VecDeque<AsyncRequest>,
}

impl Generator {
    /// Every value the generator holds, for the collector.
    pub(crate) fn values(&self) -> impl Iterator<Item = Value> + '_ {
        [self.this, self.function, self.new_target, self.promise]
            .into_iter()
            .chain(self.registers.iter().copied())
            .chain(
                self.queue
                    .iter()
                    .flat_map(|r| [r.value, Value::object(r.promise)]),
            )
    }
}

/// What an iterator helper does on `next`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HelperKind {
    Map,
    Filter,
    Take,
    Drop,
    FlatMap,
    Concat,
}

/// `[[GeneratorState]]` of an iterator helper, whose body the
/// specification writes as an abstract closure run as a generator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HelperState {
    Start,
    Yielded,
    Running,
    Done,
}

/// The state of an iterator helper: its generator body unrolled into
/// fields, so a step is a plain function call.
#[derive(Clone, Debug)]
pub(crate) struct Helper {
    pub kind: HelperKind,
    pub state: HelperState,
    /// `[[UnderlyingIterator]]` and its `next` method (undefined for
    /// `Iterator.concat`).
    pub iterator: Value,
    pub next: Value,
    /// The mapper or predicate.
    pub function: Value,
    /// The counter passed to the function; the remaining limit of `take`
    /// and `drop`; the next item of `Iterator.concat`.
    pub count: f64,
    /// The inner iterator of `flatMap` and `Iterator.concat`, undefined
    /// between inner iterators, and its `next` method.
    pub inner: Value,
    pub inner_next: Value,
    /// `Iterator.concat`: each iterable and its `@@iterator` method.
    pub items: Box<[Value]>,
}

impl Helper {
    /// Every value the helper holds, for the collector.
    pub(crate) fn values(&self) -> impl Iterator<Item = Value> + '_ {
        [
            self.iterator,
            self.next,
            self.function,
            self.inner,
            self.inner_next,
        ]
        .into_iter()
        .chain(self.items.iter().copied())
    }
}

/// The `length` of an Array is writable (cleared by freezing).
pub(crate) const FLAG_EXTENSIBLE: u8 = 1;
pub(crate) const FLAG_ARRAY_LENGTH_WRITABLE: u8 = 2;
/// Some object has (or had) this one as its prototype. Changing its named
/// properties can change what other objects inherit, so it invalidates
/// the interpreter's inline caches.
pub(crate) const FLAG_PROTOTYPE: u8 = 8;
/// A native function that also has `[[Construct]]`: a built-in
/// constructor.
pub(crate) const FLAG_NATIVE_CONSTRUCTOR: u8 = 16;
/// A function compiled from source; `length` indexes the closure arena.
pub(crate) const FLAG_CLOSURE: u8 = 4;
/// An exotic object whose internal methods are overridden: a Proxy, a
/// mapped arguments object, a typed array or a module namespace. The
/// kind is in the object's [`Internal`] slot, and the ordinary internal
/// methods hand such an object to `exotic.rs`. Its dense `elements` stay
/// empty, so the interpreter's element fast paths never see one.
pub(crate) const FLAG_EXOTIC: u8 = 32;
/// Created while its realm's intrinsics were (see `Agent::create_realm`):
/// such objects have the same position in every realm's run of them.
pub(crate) const FLAG_INTRINSIC: u8 = 64;
/// The prototype of the underlying Window is fixed after host initialization.
pub(crate) const FLAG_IMMUTABLE_PROTO: u8 = 128;

const NO_PROTOTYPE: u32 = u32::MAX;

/// One object.
#[derive(Clone, Debug)]
pub struct Object {
    pub(crate) shape: ShapeId,
    proto: u32,
    /// Array `length` for arrays; the closure index for functions
    /// compiled from source (`FLAG_CLOSURE`).
    pub(crate) length: u32,
    pub(crate) class: Class,
    pub(crate) flags: u8,
    /// The realm the object was created in (§10.2.1 `[[Realm]]` for
    /// functions). It fits in what was padding.
    pub(crate) realm: u16,
    /// Named property values, in shape slot order (unused once a
    /// dictionary).
    pub(crate) slots: Vec<Value>,
    /// Dense indexed elements; holes are [`Value::UNINITIALIZED`]. Each is a
    /// writable, enumerable, configurable data property.
    pub(crate) elements: Vec<Value>,
    pub(crate) extra: Option<Box<Extra>>,
}

impl Object {
    /// A new extensible object of `class`.
    pub(crate) fn new(class: Class, proto: Option<ObjectId>) -> Self {
        Self {
            shape: ROOT_SHAPE,
            proto: proto.map_or(NO_PROTOTYPE, ObjectId::index),
            length: 0,
            class,
            flags: FLAG_EXTENSIBLE | FLAG_ARRAY_LENGTH_WRITABLE,
            realm: 0,
            slots: Vec::new(),
            elements: Vec::new(),
            extra: None,
        }
    }

    /// `[[Prototype]]`.
    /// `[[Prototype]]` as stored: the object's index, or a marker for
    /// none. Inline caches compare it without decoding.
    pub(crate) fn proto_raw(&self) -> u32 {
        self.proto
    }

    pub(crate) fn proto(&self) -> Option<ObjectId> {
        (self.proto != NO_PROTOTYPE).then_some(ObjectId(self.proto))
    }

    pub(crate) fn set_proto(&mut self, proto: Option<ObjectId>) {
        self.proto = proto.map_or(NO_PROTOTYPE, ObjectId::index);
    }

    pub(crate) fn has_flag(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    pub(crate) fn set_flag(&mut self, flag: u8, on: bool) {
        if on {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }

    pub(crate) fn extra_mut(&mut self) -> &mut Extra {
        self.extra.get_or_insert_with(Box::default)
    }

    pub(crate) fn dictionary(&self) -> Option<&Dictionary> {
        self.extra.as_ref()?.dictionary.as_ref()
    }

    pub(crate) fn sparse(&self) -> Option<&BTreeMap<u32, Stored>> {
        self.extra.as_ref()?.sparse.as_ref()
    }

    /// The primitive in a wrapper object's internal slot.
    pub(crate) fn primitive(&self) -> Option<Value> {
        self.extra.as_ref()?.primitive
    }

    pub(crate) fn native(&self) -> Option<NativeFn> {
        self.extra.as_ref()?.native
    }

    /// The index of this function's [`Closure`], when it was compiled
    /// from source.
    pub(crate) fn closure_index(&self) -> Option<u32> {
        self.has_flag(FLAG_CLOSURE).then_some(self.length)
    }

    pub(crate) fn internal(&self) -> Option<&Internal> {
        self.extra.as_ref()?.internal.as_ref()
    }

    pub(crate) fn internal_mut(&mut self) -> Option<&mut Internal> {
        self.extra.as_mut()?.internal.as_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_objects_are_small() {
        assert!(
            std::mem::size_of::<Object>() <= 72,
            "{}",
            std::mem::size_of::<Object>()
        );
        assert_eq!(std::mem::size_of::<PropertyKey>(), 8);
    }
}
