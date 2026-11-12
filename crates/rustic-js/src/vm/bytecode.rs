//! Register bytecode (JS-04).
//!
//! A function compiles to a [`FunctionCode`]: a flat vector of [`Op`]s over
//! a frame of registers, a constant pool, the templates of nested
//! functions and a table of exception handlers. Registers hold [`Value`]s;
//! the frame lives on the agent's value stack, not the Rust stack.
//! Operands are `u16` register numbers and `u32` pool or code indices, so
//! an op is 12 bytes on every target.

use std::cell::Cell;
use std::rc::Rc;

use crate::runtime::{StringId, Value};

/// A compiled function in a heap. Code is collected like any other heap
/// record: a closure or a running frame keeps its code alive, and code
/// keeps its constants and nested functions alive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CodeId(pub(crate) u32);

/// A register in the current frame.
pub type Reg = u16;

/// Operators that take two values and produce one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Rem,
    /// `**`
    Exp,
    /// `<<`
    Shl,
    /// `>>`
    Sar,
    /// `>>>`
    Shr,
    /// `&`
    BitAnd,
    /// `|`
    BitOr,
    /// `^`
    BitXor,
    /// `==`
    Eq,
    /// `!=`
    NotEq,
    /// `===`
    StrictEq,
    /// `!==`
    StrictNotEq,
    /// `<`
    Lt,
    /// `<=`
    LtEq,
    /// `>`
    Gt,
    /// `>=`
    GtEq,
    /// `in`
    In,
    /// `instanceof`
    Instanceof,
}

/// Operators that take one value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    /// `-`
    Neg,
    /// `+` (ToNumber)
    Plus,
    /// `!`
    Not,
    /// `~`
    BitNot,
    /// `typeof` of a value
    Typeof,
    /// ToNumeric, for postfix updates
    ToNumeric,
    /// `x + 1` on a numeric
    Inc,
    /// `x - 1` on a numeric
    Dec,
    /// ToPropertyKey, for computed keys
    ToPropertyKey,
    /// ToString, for template substitutions
    ToString,
    /// ToObject (throws on null and undefined)
    ToObject,
}

/// One instruction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    /// `dst = constants[index]`
    Const { dst: Reg, index: u32 },
    /// `dst = undefined`
    Undefined { dst: Reg },
    /// `dst = <uninitialized>` (start of a TDZ)
    Uninit { dst: Reg },
    /// `dst = int` as a Number
    Int { dst: Reg, value: i32 },
    /// `dst = src`
    Move { dst: Reg, src: Reg },
    /// `dst = this` (throws in a derived constructor before `super()`)
    This { dst: Reg },
    /// `dst = new.target`
    NewTarget { dst: Reg },
    /// `dst = the running function`
    Callee { dst: Reg },
    /// Throw a ReferenceError naming `constants[name]` if `src` is
    /// uninitialized.
    CheckTdz { src: Reg, name: u32 },
    /// Throw a TypeError for assignment to the constant `constants[name]`.
    ConstAssign { name: u32 },

    /// `dst = scope slot`, `hops` scopes out, with a TDZ check.
    GetSlot { dst: Reg, hops: u16, slot: u16 },
    /// Store into a scope slot, checking the TDZ and mutability.
    SetSlot { src: Reg, hops: u16, slot: u16 },
    /// Initialize a scope slot (declarations; no checks).
    InitSlot { src: Reg, hops: u16, slot: u16 },
    /// Enter a block scope: a new environment with `scopes[scope]`'s
    /// bindings, all uninitialized.
    PushScope { scope: u32 },
    /// Leave the innermost block scope.
    PopScope,
    /// Replace the innermost scope with a copy (per-iteration bindings).
    CopyScope,

    /// `dst = name` resolved through the environment chain (globals,
    /// `with`); ReferenceError if unresolvable.
    GetName { dst: Reg, name: u32 },
    /// `typeof name`: like `GetName` but `undefined` when unresolvable.
    TypeofName { dst: Reg, name: u32 },
    /// Assign to `name` through the chain (`PutValue`).
    SetName { src: Reg, name: u32 },
    /// `dst = the global binding sites[site].name`: a global lexical
    /// binding or a property of the global object, cached in the site.
    /// Emitted for names no enclosing scope declares when no `with` is in
    /// the way.
    GetGlobal { dst: Reg, site: u16 },
    /// Assign to a global binding through `sites[site]`.
    SetGlobal { src: Reg, site: u16 },
    /// Initialize a global lexical binding or declare a global var.
    DeclareGlobal { name: u32, kind: u8 },
    /// Initialize a global lexical binding (`let`, `const` at the top of a
    /// script).
    InitName { src: Reg, name: u32 },
    /// `dst = delete name`
    DeleteName { dst: Reg, name: u32 },
    /// Enter a `with` body: an object environment for `object`.
    PushWith { object: Reg },
    /// `dst = this` for a name call through a `with` object, else
    /// undefined.
    WithBase { dst: Reg, name: u32 },

    /// `dst = object[key]`
    GetProp { dst: Reg, object: Reg, key: Reg },
    /// `dst = object.name` through `sites[site]`, which caches the shape
    /// and slot of the last own data property found.
    GetNamed { dst: Reg, object: Reg, site: u16 },
    /// `object[key] = value`
    SetProp { object: Reg, key: Reg, value: Reg },
    /// `object.name = value` through `sites[site]`.
    SetNamed { object: Reg, site: u16, value: Reg },
    /// `dst = delete object[key]`
    DeleteProp { dst: Reg, object: Reg, key: Reg },
    /// `CreateDataPropertyOrThrow(object, key, value)` (literals).
    DefineProp { object: Reg, key: Reg, value: Reg },
    /// Define a getter (`setter == false`) or setter: enumerable on a
    /// literal, not on a class.
    DefineAccessor {
        object: Reg,
        key: Reg,
        function: Reg,
        setter: bool,
        enumerable: bool,
    },
    /// Define a class method: a writable, configurable, non-enumerable
    /// data property.
    DefineMethod { object: Reg, key: Reg, value: Reg },
    /// Wire up a class: `proto` becomes a new prototype object, whose
    /// `constructor` is `ctor` and which `ctor.prototype` names. With
    /// `derived`, `heritage` (a constructor or null) supplies the
    /// prototypes of both; anything else throws a TypeError.
    ClassSetup {
        ctor: Reg,
        heritage: Reg,
        proto: Reg,
        derived: bool,
    },
    /// `dst = src.[[GetPrototypeOf]]()`, or null.
    GetPrototypeOf { dst: Reg, src: Reg },
    /// `dst = object[key]` with `this` as the receiver (`super.key`);
    /// `object` is the home object's prototype.
    GetSuper {
        dst: Reg,
        object: Reg,
        key: Reg,
        this: Reg,
    },
    /// `object[key] = value` with `this` as the receiver
    /// (`super.key = value`).
    SetSuper {
        object: Reg,
        key: Reg,
        value: Reg,
        this: Reg,
    },
    /// `dst = Construct(callee, args, new_target)` (`super(...)`).
    SuperCall {
        dst: Reg,
        callee: Reg,
        new_target: Reg,
        args: Reg,
        argc: u16,
    },
    /// `dst = Construct(callee, ...array, new_target)`.
    SuperCallSpread {
        dst: Reg,
        callee: Reg,
        new_target: Reg,
        array: Reg,
    },
    /// `dst = a fresh private name` described by `constants[name]`.
    NewPrivateName { dst: Reg, name: u32 },
    /// `dst = object.#key`, a private field.
    PrivateGet { dst: Reg, object: Reg, key: Reg },
    /// `object.#key = value`, a private field that must exist.
    PrivateSet { object: Reg, key: Reg, value: Reg },
    /// Add the private field or brand `key` to `object`.
    PrivateAdd { object: Reg, key: Reg, value: Reg },
    /// `dst = #key in object`. `key` is a private name the object must
    /// own, or (for static methods) the class the object must be.
    PrivateHas { dst: Reg, object: Reg, key: Reg },
    /// Throw a TypeError unless `object` has the brand, as in
    /// [`Op::PrivateHas`]: the check before a private method or accessor.
    PrivateBrand { object: Reg, brand: Reg },
    /// Throw a TypeError with the message `constants[message]`.
    TypeError { message: u32 },
    /// `dst = scope slot` without the TDZ check.
    GetSlotRaw { dst: Reg, hops: u16, slot: u16 },
    /// Throw a ReferenceError if `src` is initialized: `super()` ran
    /// twice.
    ThrowIfInit { src: Reg },
    /// Throw a TypeError unless `src` is an object or undefined: the
    /// result of a derived constructor.
    DerivedResult { src: Reg },
    /// `dst = {}`
    NewObject { dst: Reg },
    /// `dst = []`
    NewArray { dst: Reg },
    /// Append `value` (or a hole) to the array in `array`.
    ArrayPush { array: Reg, value: Reg },
    /// Append a hole.
    ArrayHole { array: Reg },
    /// Copy own enumerable properties of `source` into `target` (object
    /// spread and rest), skipping the keys in registers
    /// `excluded .. excluded + count`.
    CopyProps {
        target: Reg,
        source: Reg,
        excluded: Reg,
        count: u16,
    },
    /// `dst = a new closure` over `functions[index]`.
    Closure { dst: Reg, index: u32 },
    /// Set the closure's `[[HomeObject]]` for `super` lookups.
    SetHome { function: Reg, home: Reg },
    /// `dst = the template object of `templates[site]``: the frozen
    /// array of strings a tag function receives, made once per site.
    TemplateObject { dst: Reg, site: u32 },
    /// `name = src` through a reference resolved earlier: `base` is the
    /// binding object [`Op::WithBase`] found, or `undefined` to resolve
    /// the name now.
    SetNameIn { base: Reg, src: Reg, name: u32 },
    /// `dst = the `[[HomeObject]]` of the running function`.
    Home { dst: Reg },
    /// `dst = the arguments object`
    Arguments { dst: Reg },
    /// `dst = the rest parameters as an array`
    Rest { dst: Reg, from: u16 },

    /// `dst = op src`
    Unary { op: UnOp, dst: Reg, src: Reg },
    /// `dst = a op b`
    Binary { op: BinOp, dst: Reg, a: Reg, b: Reg },
    /// `dst = a op value`, for a small integer literal on the right.
    BinaryInt {
        op: BinOp,
        dst: Reg,
        a: Reg,
        value: i32,
    },

    /// Unconditional jump.
    Jump { target: u32 },
    /// Jump if ToBoolean(cond).
    JumpIfTrue { cond: Reg, target: u32 },
    /// Jump unless ToBoolean(cond).
    JumpIfFalse { cond: Reg, target: u32 },
    /// Jump if `src` is null or undefined.
    JumpIfNullish { src: Reg, target: u32 },
    /// Jump unless `src` is undefined.
    JumpIfNotUndefined { src: Reg, target: u32 },
    /// Jump unless `a op b` holds; `op` is a comparison. A test and its
    /// branch in one instruction.
    JumpUnless {
        op: BinOp,
        a: Reg,
        b: Reg,
        target: u32,
    },
    /// Jump unless `a op value` holds, for a small integer literal.
    JumpUnlessInt {
        op: BinOp,
        a: Reg,
        value: i32,
        target: u32,
    },
    /// Jump if `a === b` (switch cases).
    JumpIfStrictEq { a: Reg, b: Reg, target: u32 },
    /// Jump if `a` holds the Number `value` (finally dispatch).
    JumpIfInt { a: Reg, value: i32, target: u32 },

    /// `dst = callee(args)` with `this` = `this`; arguments in
    /// `args .. args + argc`.
    Call {
        dst: Reg,
        callee: Reg,
        this: Reg,
        args: Reg,
        argc: u16,
    },
    /// `dst = eval(args)` where `eval` is the bare identifier: a direct
    /// eval when `callee` is `%eval%` (§19.2.1.1), otherwise an ordinary
    /// call whose `this` is in register `callee - 1` (the binding object
    /// when `eval` was found in a `with`). `var_hops` counts environments
    /// to the variable environment, or is `u16::MAX` for the global one.
    DirectEval {
        dst: Reg,
        callee: Reg,
        args: Reg,
        argc: u16,
        var_hops: u16,
    },
    /// `dst = new callee(args)`
    New {
        dst: Reg,
        callee: Reg,
        args: Reg,
        argc: u16,
    },
    /// Return `src` from the function.
    Return { src: Reg },
    /// Throw `src`.
    Throw { src: Reg },
    /// The first op of an async function: create its promise and the
    /// state object its frame is suspended in at each `await`.
    AsyncStart,
    /// `dst = await src`: suspend until the promise for `src` settles,
    /// returning the function's promise to the caller. On resumption
    /// `dst` receives the value and `mode` is 0 (fulfilled) or 1
    /// (rejected: throw it).
    Await { src: Reg, dst: Reg, mode: Reg },
    /// The catch-all handler of an async function: reject its promise
    /// with `src` and return the promise.
    AsyncReject { src: Reg },
    /// The end of a generator function's parameter and declaration
    /// instantiation: create the generator object, suspend the frame in
    /// it and return the object to the caller.
    GeneratorStart,
    /// Suspend the generator, giving the caller an iterator result for
    /// `src` (or `src` itself when `raw`, for `yield*`). On resumption
    /// `dst` receives the value sent in and `mode` how it was sent: 0 for
    /// `next`, 1 for `throw`, 2 for `return`.
    Yield {
        src: Reg,
        dst: Reg,
        mode: Reg,
        raw: bool,
    },
    /// One step of `yield*` (§15.5.5): call the `next`, `throw` or
    /// `return` method (by `mode`) of `iterator` (whose `next` is in the
    /// register after it) with `value`, leaving the result object in
    /// `dst`. A missing `return` produces `{value, done: true}`; a missing
    /// `throw` closes the iterator and throws a TypeError.
    Delegate {
        dst: Reg,
        iterator: Reg,
        value: Reg,
        mode: Reg,
    },
    /// `iterator, iterator + 1 = GetIterator(src)` as a real iterator
    /// object and its `next` method (no array fast path).
    GetIteratorObject { iterator: Reg, src: Reg },
    /// `iterator, iterator + 1 = GetIterator(src, async)`: the
    /// `@@asyncIterator` method's result, or the sync iterator wrapped by
    /// CreateAsyncFromSyncIterator, and its `next` method.
    GetAsyncIterator { iterator: Reg, src: Reg },
    /// One step of `yield*` in an async generator: call `next`, `throw`
    /// or `return` (by `mode`) of `iterator` with `value` into `dst`, not
    /// awaited yet. A missing `return` jumps to `missing`; a missing
    /// `throw` calls `return` with no argument into `dst` and jumps to
    /// `missing` (or throws a TypeError if that is missing too).
    AsyncDelegate {
        dst: Reg,
        iterator: Reg,
        value: Reg,
        mode: Reg,
        missing: u32,
    },
    /// Throw a TypeError if `src` is not an object (an iterator result).
    RequireObject { src: Reg },
    /// The TypeError of a `yield*` whose async iterator has no `throw`.
    NoThrowMethod,
    /// The call half of AsyncIteratorClose: `dst = iterator.return()`,
    /// to be awaited, then forget the iterator. Jumps to `missing` when
    /// there is no iterator or no `return` method; when `silent` (the
    /// close after a throw) also when getting or calling it throws.
    AsyncIteratorReturn {
        dst: Reg,
        iterator: Reg,
        missing: u32,
        silent: bool,
    },

    /// Start a for-in over `src`: `state` holds the object (undefined
    /// for null and undefined), `state + 1` an array of its enumerable
    /// string keys, own first and then inherited, and `state + 2` the
    /// position.
    ForInStart { state: Reg, src: Reg },
    /// `dst = the next key that is still a property`, or jump to `done`
    /// when exhausted.
    ForInNext { dst: Reg, state: Reg, done: u32 },
    /// `iterator, next = GetIterator(src)` (sync). `next` is always the
    /// register after `iterator`. For an array whose iteration protocol
    /// is untouched, `iterator` is the array and `next` the position, and
    /// stepping allocates nothing.
    GetIterator { iterator: Reg, next: Reg, src: Reg },
    /// `dst = IteratorStepValue`, or jump to `done` when finished.
    IteratorStep {
        dst: Reg,
        iterator: Reg,
        next: Reg,
        done: u32,
    },
    /// `IteratorClose` for an early exit, then forget the iterator so a
    /// second close does nothing. `silent` is the close after a throw:
    /// errors from `return` are dropped in favor of the original one.
    IteratorClose { iterator: Reg, silent: bool },
    /// `dst = callee(...array)` with `this`.
    CallSpread {
        dst: Reg,
        callee: Reg,
        this: Reg,
        array: Reg,
    },
    /// `dst = new callee(...array)`
    NewSpread { dst: Reg, callee: Reg, array: Reg },
    /// `object.[[Prototype]] = value` if value is an object or null
    /// (`__proto__: value` in a literal).
    SetPrototype { object: Reg, value: Reg },
    /// A loop's back edge: poll for cancellation and collection, then
    /// jump to `target`.
    Loop { target: u32 },
    /// No operation (`debugger`).
    Nop,
}

/// The `kind` operand of [`Op::DeclareGlobal`].
pub mod declare {
    /// `var` or a function declaration: a global object property.
    pub const VAR: u8 = 0;
    /// `let` or `class`: a mutable global lexical binding.
    pub const LET: u8 = 1;
    /// `const`: an immutable global lexical binding.
    pub const CONST: u8 = 2;
}

/// An exception handler: a throw from `start .. end` jumps to `target`
/// with the exception in `exception`, after dropping block scopes down to
/// `scope_depth`.
#[derive(Clone, Copy, Debug)]
pub struct Handler {
    /// First covered op.
    pub start: u32,
    /// One past the last covered op.
    pub end: u32,
    /// Where to continue.
    pub target: u32,
    /// Register receiving the exception.
    pub exception: Reg,
    /// Block-scope depth at the `try`.
    pub scope_depth: u16,
}

/// What kind of function a template makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunctionKind {
    /// A script's top level.
    Script,
    /// `function` declarations and expressions: callable and
    /// constructible, with their own `this`.
    Normal,
    /// Arrow functions: lexical `this`, not constructible.
    Arrow,
    /// Object-literal and class methods, getters and setters: not
    /// constructible.
    Method,
    /// The constructor of a class without `extends`: only `new` may
    /// call it.
    ClassConstructor,
    /// The constructor of a class with `extends`: `this` is bound by
    /// `super()`.
    DerivedConstructor,
}

impl FunctionKind {
    /// Whether `new` accepts a function of this kind.
    pub fn is_constructor(self) -> bool {
        matches!(
            self,
            FunctionKind::Normal
                | FunctionKind::ClassConstructor
                | FunctionKind::DerivedConstructor
        )
    }

    /// Whether this is a class constructor, which a plain call rejects.
    pub fn is_class_constructor(self) -> bool {
        matches!(
            self,
            FunctionKind::ClassConstructor | FunctionKind::DerivedConstructor
        )
    }
}

/// A compiled function.
#[derive(Debug)]
pub struct FunctionCode {
    /// Instructions.
    pub ops: Vec<Op>,
    /// Constant pool: numbers, strings and atoms (property names and
    /// binding names, as interned strings).
    pub constants: Vec<Value>,
    /// Templates of nested functions.
    pub functions: Vec<CodeId>,
    /// Block scopes: the names and flags of their bindings.
    pub scopes: Vec<Rc<Vec<(StringId, u8)>>>,
    /// Exception handlers, innermost first.
    pub handlers: Vec<Handler>,
    /// Registers per frame, parameters first.
    pub registers: u16,
    /// Declared parameters before any rest parameter (the `length`
    /// counts only those before the first default).
    pub params: u16,
    /// The function's `length`.
    pub length: u16,
    /// The function's `name`, as an atom.
    pub name: StringId,
    /// Strict mode code.
    pub strict: bool,
    /// What kind of function this is.
    pub kind: FunctionKind,
    /// A generator function: a call returns a generator object.
    pub generator: bool,
    /// An async function: a call returns a promise.
    pub is_async: bool,
    /// Named property access sites with their inline caches.
    pub sites: Box<[NamedSite]>,
    /// Tagged template sites.
    pub templates: Box<[TemplateSite]>,
}

/// A tagged template site: its strings and, once it has run, its
/// template object.
#[derive(Debug)]
pub struct TemplateSite {
    /// `constants[first .. first + count]` are the cooked strings
    /// (`undefined` for an invalid escape) and the next `count` the raw
    /// ones.
    pub first: u32,
    /// The number of strings.
    pub count: u32,
    /// The template object, or `undefined` before the first use.
    pub object: Cell<Value>,
}

/// What an access site remembers about its last lookup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CacheKind {
    /// Nothing yet.
    #[default]
    Empty,
    /// An own data property: the value is the receiver's `slot`.
    Own,
    /// An inherited data property: the value is slot `slot` of the
    /// object `other`, found through the receiver's prototype `proto`.
    Inherited,
    /// A write that adds the property: the receiver moves to shape
    /// `other` and the value becomes its slot `slot`. Nothing on the
    /// chain from `proto` intercepts the write.
    Add,
    /// A global lexical binding at position `slot`; never stale.
    GlobalLexical,
}

/// The one-entry inline cache of a [`NamedSite`].
///
/// An entry is trusted only while `epoch` is the agent's current cache
/// epoch, which moves on every collection (shape and object ids are
/// reused), on every change to a prototype object and on every new global
/// lexical binding, and only for a receiver with the same `shape` and,
/// where the chain matters, the same prototype.
#[derive(Clone, Copy, Debug, Default)]
pub struct SiteCache {
    /// What the entry says.
    pub kind: CacheKind,
    /// The cache epoch the entry was written in.
    pub epoch: u32,
    /// The receiver's shape.
    pub shape: u32,
    /// The slot to read or write.
    pub slot: u32,
    /// The holder object or the new shape (see [`CacheKind`]).
    pub other: u32,
    /// The receiver's prototype, as stored in the object.
    pub proto: u32,
}

/// One `object.name` access site and its one-entry inline cache.
#[derive(Debug)]
pub struct NamedSite {
    /// The property name (an atom).
    pub name: StringId,
    /// The site's cache.
    pub cache: Cell<SiteCache>,
}

impl FunctionCode {
    /// Bytes this function's code, constants, tables and caches are
    /// charged for (nested functions are charged on their own).
    pub fn code_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.ops.len() * std::mem::size_of::<Op>()
            + self.constants.len() * 8
            + self.functions.len() * 4
            + self.scopes.iter().map(|s| 16 + s.len() * 8).sum::<usize>()
            + self.handlers.len() * std::mem::size_of::<Handler>()
            + self.sites.len() * std::mem::size_of::<NamedSite>()
            + self.templates.len() * std::mem::size_of::<TemplateSite>()
    }

    /// Report every heap reference the code holds. Shapes remembered by
    /// inline caches are not references: a cache entry is only trusted in
    /// the collection epoch that wrote it.
    pub(crate) fn trace(&self, tracer: &mut impl crate::runtime::Tracer) {
        for &v in &self.constants {
            tracer.value(v);
        }
        for &f in &self.functions {
            tracer.code(f);
        }
        for scope in &self.scopes {
            for &(name, _) in scope.iter() {
                tracer.string(name);
            }
        }
        tracer.string(self.name);
        for site in self.sites.iter() {
            tracer.string(site.name);
        }
        for site in self.templates.iter() {
            tracer.value(site.object.get());
        }
    }
}
