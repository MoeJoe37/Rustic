# ADR 0002: Agent ownership and a traced host graph

Date: 2026-11-07. Status: accepted design; validate prototype before M5.

## Context

DOM nodes mutate, JS can retain detached subtrees, listener closures form cycles,
and host callbacks can reenter. Shared global mutable graphs and reference
counting alone make leaks, deadlocks and stale references difficult to control.

## Decision

One document agent owns observable DOM/JS mutation on its thread. Background I/O
posts completions rather than calling JS. Typed generation-checked handles avoid
raw-pointer APIs. Start DOM storage in a nonmoving arena; before bindings, extend
the ownership contract to explicit roots and a nonmoving traced JS/host graph.
The runtime traces registered host objects without a dependency on the DOM crate.
Use stop-the-world mark/sweep first. Layout/style/paint consume snapshots and
epochs. No lock or mutable DOM borrow is held across a page callback.

## Alternatives and consequences

An `Arc<Mutex<...>>` DOM encourages unsafe observable parallelism and callback
deadlocks. `Rc`/weak edges help trees but cannot collect arbitrary JS/DOM cycles.
A moving/generational collector might improve performance but adds barriers and
pointer-update complexity before semantics are correct. Nonmoving tracing has
pause/fragmentation costs and demands complete edge registration and rooting.
Arena IDs alone detect stale handles, not object liveness.

## Validation and revisit

Prototype detached-node retention, listener cycles, wrapper identity, repeated
collection and reentry before broad IDL generation. Compare memory after repeated
open/close; fuzz invalid/stale handles. Revisit collector strategy only with
measured heap/pause evidence and a new barrier/root correctness plan.
