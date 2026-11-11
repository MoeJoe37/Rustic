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
mod gc;
mod heap;
mod internal;
mod number;
mod object;
mod shape;
mod string;
mod value;
mod weak;

pub use agent::{
    Abrupt, Agent, ErrorKind, Intrinsics, JsResult, Names, NativeFn, WellKnownSymbols,
};
pub use bigint::BigInt;
pub use completion::Completion;
pub use convert::{Numeric, PreferredType, string_to_bigint};
pub use env::{EnvId, Environment, Reference, ReferenceBase, ThisStatus};
pub use gc::{GcStats, HostRoot, HostTracer, MIN_GC_THRESHOLD, Roots};
pub use heap::{DEFAULT_HEAP_BYTES, Heap, OutOfMemory, Symbol};
pub use internal::IntegrityLevel;
pub use number::{
    number_to_string, string_to_number, to_int32, to_integer_or_infinity, to_length, to_uint16,
    to_uint32,
};
pub use object::{Class, PropertyDescriptor};
pub use shape::{Attributes, MAX_SHAPE_PROPERTIES, PropertyKey};
pub use string::JsStr;
pub use value::{BigIntId, Kind, ObjectId, StringId, SymbolId, Type, Value};
