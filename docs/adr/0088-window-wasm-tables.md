# ADR 0088 — Standalone Window Wasm tables

Date: 2026-11-25
Status: Accepted for standalone tables in M9

## Context

The Window `WebAssembly.Table` constructor creates a table that can store
external JavaScript values or Wasm function references. External values may
include objects, symbols and other values tracked by the JavaScript heap.
Keeping a Wasm function alive must also retain the instance it calls.

## Decision

Use a branded wrapper for the element kind, address width and admitted
capacity. The page Wasm link map associates the wrapper with a private dense
array. This ephemeron keeps the array and its entries alive for a reachable
table. The array has a null prototype and stays private to the binding.

Charge each stored entry to the JavaScript heap. A growth operation checks
the declared maximum, the 65,536-entry host cap and the work budget before
reserving storage. Update the entries and length together after a successful
reservation. Read the current length after converting a growth count, since
that conversion may reenter the same table.

## Alternatives considered

Native storage for JavaScript values would require a new tracing path for
each entry. The dense array uses the collector's existing value traversal and
the page's existing ephemeron map.

## Consequences and revisit trigger

Standalone tables share the normal heap budget. Table imports and exports
will need a shared representation that lets guest instructions and JavaScript
observe the same values and resolve functions to their owning instances.
Revisit the storage design when introducing those links.

## Validation

Focused tests cover collection stress, bounds, reentrant growth, function
identity, default values and the exact host cap. The pinned Window WPT
selection covers construction, descriptors, getters, setters and growth.

[WebAssembly JavaScript Interface, tables](https://webassembly.github.io/spec/js-api/#tables)
