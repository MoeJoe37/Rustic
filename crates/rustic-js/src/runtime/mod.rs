//! The runtime data model (JS-02): values, strings, numbers, objects,
//! environments and references, laid out for a small heap (ADR 0015),
//! and its garbage collector (JS-03, ADR 0016).
//! Nothing here evaluates syntax; the interpreter is JS-04.

mod agent;
mod bigint;
mod builtins;
mod completion;
mod convert;
mod env;
mod exotic;
mod gc;
mod heap;
mod internal;
mod number;
mod object;
mod shape;
mod string;
mod table;
mod value;
mod weak;

pub use agent::{
    Abrupt, Agent, ErrorKind, Intrinsics, JsResult, Names, NativeFn, RealmId, WellKnownSymbols,
};
pub use bigint::{BigInt, MAX_BITS as MAX_BIGINT_BITS};
pub(crate) use builtins::async_generator::create_async_from_sync_iterator;
pub(crate) use builtins::atomics::Waiter;
pub(crate) use builtins::bigint::{BigIntBinary, BigIntUnary, SameNumeric, same_numeric_types};
pub(crate) use builtins::generator::{RESUME_RETURN, RESUME_THROW};
pub(crate) use builtins::intrinsic_in_function_realm;
pub(crate) use builtins::iterator_result;
pub(crate) use builtins::json::parse_json_units;
pub use builtins::promise::{RejectionEvent, RejectionOperation};
pub(crate) use builtins::promise::{
    await_value, new_intrinsic_promise, reject_promise, resolve_promise, watch_import, watch_module,
};
pub(crate) use builtins::regexp::regexp_create_literal;
pub use completion::Completion;
pub use convert::{Numeric, PreferredType, string_to_bigint};
pub(crate) use env::{
    BINDING_IMPORT, BINDING_LEXICAL, BINDING_MUTABLE, BINDING_STRICT, ImportBinding,
};
pub use env::{EnvId, Environment, FunctionExtra, Reference, ReferenceBase, ThisStatus};
pub(crate) use gc::Tracer;
pub use gc::{GcStats, HostRoot, HostTracer, MIN_GC_THRESHOLD, Roots};
pub use heap::{DEFAULT_HEAP_BYTES, Heap, OutOfMemory, Symbol};
pub use internal::IntegrityLevel;
pub use number::{
    number_to_string, string_to_number, to_int32, to_integer_or_infinity, to_length, to_uint16,
    to_uint32,
};
pub use object::{Class, PropertyDescriptor};
pub(crate) use object::{
    Closure, Extra, FLAG_CLOSURE, FLAG_EXOTIC, FLAG_EXTENSIBLE, FLAG_NATIVE_CONSTRUCTOR,
    FLAG_PROTOTYPE, Generator, GeneratorState, Internal, Job, NamespaceBinding, Object,
};
pub(crate) use shape::ShapeId;
pub use shape::{Attributes, MAX_SHAPE_PROPERTIES, PropertyKey};
pub use string::JsStr;
pub use value::{BigIntId, Kind, ObjectId, StringId, SymbolId, Type, Value};
