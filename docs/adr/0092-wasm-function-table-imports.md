# ADR 0092 — Shared WebAssembly function tables and function addresses

Date: 2026-11-26
Status: Accepted for nullable function tables and numeric cross-instance calls in M9

## Context

An imported function table can be read and written by several instances.
A function stored by one instance must keep its identity and execute its
defining instance's code. Active element initialization can publish functions
to a shared table before a later segment or the start function traps.

## Decision

A hosted function address carries a compact instance identity and an index
in that instance. The host associates the identity with a rooted instance
object before making staged initializer writes visible. The object holds the
prepared runtime and imports, so an already-published function remains
callable after a later initializer traps. The unhosted interpreter continues
to use local indices.

Function and external-reference tables share the same allocation, import
matching and limit checks. A standalone table adopts interpreter storage on
its first link. Re-exports use the original wrapper. Copying and growing
shared tables preserve the stored function addresses. The declaring
instance's wrapper cache also serves JavaScript table reads.

The collector marks the owning instance of a reachable function address.
It visits entries at the allocation owner, private guest globals and
retained segments, and suspended frames at call boundaries. An indirect
cross-instance call roots its selected owner while the callee executes.
Store borrows end before entering the host dispatcher.

The dispatcher compares the callee's numeric signature with the expected
signature before entering the callee. Indirect tail calls release the local
frame and use the same bounded dispatcher when crossing instances.

## Alternatives considered

Embedding a strong runtime pointer in every entry would create cycles
between instances and tables. Storing JavaScript function wrappers in
every guest entry would require eager wrapper allocation during element
initialization. An instance identity and index allow lazy wrapper creation
and let collection follow existing heap roots.

## Consequences and revisit trigger

TD-208's shared table ownership is implemented.
[ADR 0093](0093-wasm-reference-interop.md) adds a boundary for nullable
reference arguments and results.

## Validation

Window regression tests cover standalone and module table imports, both
address widths, overlapping aliases, numeric indirect and tail calls, type
mismatch, initializer traps and GC during cross-instance calls. Nine pinned
Window WPT subtests for function-table imports now pass.

[WebAssembly JavaScript Interface, exported functions](https://webassembly.github.io/spec/js-api/#exported-functions)

[WebAssembly Core Specification, instantiation](https://webassembly.github.io/spec/core/exec/modules.html#exec-instantiation)
