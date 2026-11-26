# ADR 0093 — Reference-valued WebAssembly calls and globals

Date: 2026-11-26
Status: Accepted for scalar and nullable reference values in M9

## Context

The interpreter already carries nullable function and external references in
tables and frames. The Window host needs the same values at function and
global boundaries, including reentrant calls that may trigger collection.

## Decision

The tracing dispatcher exchanges numbers and typed references. Numeric Rust
entry points keep their existing value API. The interpreter validates
arguments and dispatcher results against the callee signature. The Window
binding converts external references losslessly and uses the existing
function-address resolver for branded Wasm functions. It roots suspended
frames during callbacks and keeps the JS iterable rooted while coercing
multi-value results. Multi-value exports include undefined entries.

A shared global cell holds the same typed value representation. Local slots
promote once when exported, and imports and re-exports share the cell.
An immutable host value initializes a separate cell. The collector follows
the current value through global wrappers and interpreter state, so
overwriting a cell releases the old GC edge.

## Alternatives considered

Passing JS wrapper values through the interpreter would bind execution to
Window and require eager function wrapping. Keeping only numeric globals
would need a separate live mirror for reference-valued globals. The shared
cell and tracing dispatcher reuse the representation used by tables.

## Consequences and revisit trigger

The supported reference types are nullable function and external references.
Wider reference and value types remain part of WASM-07. The numeric and
tracing Rust interfaces share the interpreter and cancellation checks.

## Validation

Window regressions cover reference identity, mixed multi-value results,
invalid function values, callback collection, reentrant indirect and tail
calls, mutable global aliasing and reclamation after overwrite.

[WebAssembly JavaScript Interface, globals](https://webassembly.github.io/spec/js-api/#globals)

[WebAssembly JavaScript Interface, exported functions](https://webassembly.github.io/spec/js-api/#exported-functions)
