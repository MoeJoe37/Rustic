# ADR 0091 — Shared WebAssembly external-reference table imports

Date: 2026-11-26
Status: Accepted for nullable external-reference tables in M9

## Context

A Wasm importer must observe the supplied table's current contents and
mutations. The source can be a standalone Window table or an export from
an existing instance. An imported value may become reachable from guest
state before the importing instance is published to JavaScript.

## Decision

The interpreter indexes shared table allocations. Each allocation retains its
owner's entry budget and declared maximum. An importer checks the element
type, address width, current length and declared maximum. Table copies test
allocation identity to handle overlapping copies through distinct indices.
Re-exports return their source table wrapper.

Standalone external tables initially use a private JS array. On first link,
the binding prepares a native allocation and replaces the wrapper's backing
after the entries have been copied and the bounded growth is reserved against
the JS heap. JavaScript methods resolve the backing after index coercion,
which may itself link the table. A module-defined table keeps its allocation
and its original instance budget when linked.

The instance's private import record traces table wrappers back to their
allocation owners. Each owner marks its values once. The importer traces
local state and suspends with roots before calling out during start, using the
same dispatch path as ordinary numeric exports. The instance wrapper is
rooted before start, and exports are published after start succeeds.

Active element and data writes are prepared before publication. A guest
bounds trap commits the completed segment prefix. Allocation and cancellation
checks finish before writing that prefix.

## Alternatives considered

A JS mirror of native storage would need to synchronize each bulk instruction
and each store before a reentrant call. Reserving native growth at initial
construction would change the costs of standalone tables that never link.

## Consequences and revisit trigger

The first link of a standalone table reserves up to its declared maximum or
65,536 entries, whichever is smaller. Additional imports reuse that allocation.
The reservation stays with the table wrapper until collection.
[ADR 0092](0092-wasm-function-table-imports.md) uses the same allocations
for function-table imports and traces each function's defining instance.

## Validation

Window regressions cover standalone and exported imports, both address widths,
aliasing, re-export identity, overlap, shared growth and declared limits.
They also exercise active element traps and collection during a start callback.
Core regression tests cover owner and importer budgets and cancellation.

[WebAssembly Core Specification, type matching](https://webassembly.github.io/spec/core/valid/matching.html)

[WebAssembly Core Specification, instantiation](https://webassembly.github.io/spec/core/exec/modules.html#exec-instantiation)
