# ADR 0089 — Live WebAssembly function table exports

Date: 2026-11-25
Status: Accepted for module-defined nullable function tables in M9

## Context

Guest table instructions already keep instance-local function indexes. The
Window interface needs to expose current table contents and preserve the
JavaScript identity of functions obtained by different paths.

## Decision

An exported table wrapper holds its table index and address width. The
page's existing ephemeron map traces the wrapper to the owning instance.
The binding reads and writes the interpreter table directly. Host growth
preflights and charges work, then uses the same fallible growth path and
aggregate allocation limit as guest growth.

An instance traces a private cache of function wrappers by function index.
Named exports and table reads share it. An index already linked to a Wasm
function uses the provider's wrapper. A table write can use null or a
function that resolves to an index already in the instance's index space.
Coercion of the JS index or count finishes before the interpreter borrows
its table state.

## Alternatives considered

A second table in the JS heap would need to mirror active elements, bulk
operations and mutations before reentrant callbacks. Keeping one store
preserves the existing guest bounds and growth behavior at both entry points.

## Consequences and revisit trigger

Local function references require no native handles into the JS heap.
[ADR 0090](0090-live-wasm-external-tables.md) adds tracing for module-defined
external-reference table exports. [ADR 0092](0092-wasm-function-table-imports.md) extends this boundary to
function-table imports and independently defined function addresses.

## Validation

Tests cover table32/table64, host and guest mutation in both directions,
alias identity, indirect calls, the owning instance's budget and collection.
The pinned Window `Instance` and `instantiate` tests exercise table exports.

[WebAssembly JavaScript Interface, tables](https://webassembly.github.io/spec/js-api/#tables)
