# ADR 0084 — Shared numeric WebAssembly globals

Date: 2026-11-24
Status: Accepted for the M9 numeric Window profile

## Context

An instance can import a global that was constructed in JavaScript or
exported by another instance. A mutable binding carries its original
identity and can be written by any of its users. The importer may outlive
the provider's JavaScript variable.

## Decision

The interpreter promotes a numeric local slot to a shared cell when it is
exposed. Ordinary local globals keep their existing inline representation.
Imported globals occupy the front of the global index space. Their
declared type and mutability are checked before initialization or start.
Guest writes and host writes use the same cell.

The Window bridge snapshots the result of each module and field lookup in
declaration order and charges for names before reading their properties.
It reads imports synchronously for a compiled Module
and after compilation for byte and response inputs. Start stays in the
existing bounded instantiation job for Promise APIs. A branded Global
preserves its wrapper on re-export. Bare numeric values become immutable
cells with their own wrapper when exported.

## Alternatives considered

Copying globals between provider and consumer would hide guest writes.
Allocating a separate shared cell for every local would increase heap
use even for globals that stay internal. Promoting only exposed numeric
globals preserves sharing and bounds the extra allocations by the imports
and exports the instance admits.

## Consequences and revisit trigger

A consumer's private binding record traces the original Global wrapper.
When a provider owns the cell, the wrapper also retains that provider and
its memory charge. Standalone wrappers charge their own cells. Sharing is
confined to the current agent. Revisit tracing and allocation ownership
when adding reference values or cross-agent shared state.

## Evidence

The interpreter regression verifies the missing and wrong-type paths and
checks read and write propagation across instances. The GC-stressed Window
test covers mixed index spaces, re-export identity, primitive snapshots,
initializers, BigInt, type and mutability rejection and deferred start.
The lookup budget regression uses a long import name on a compiled Module.
The two pinned upstream constructor files pass 68 subtests. The 18
expected failures use the remaining memory or table APIs.

[WebAssembly JS API, reading imports](https://webassembly.github.io/spec/js-api/#read-the-imports)
