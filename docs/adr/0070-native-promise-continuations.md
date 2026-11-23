# ADR 0070 — Native promise continuations and job-boundary collection

Date: 2026-11-23
Status: Accepted for M9 `Array.fromAsync`

## Context

`Array.fromAsync` may suspend between pulls, mapping, and iterator cleanup.
A native-only sequence may run thousands of promise jobs without executing a
bytecode safe point. Completed awaits then occupy the heap until the next
collection, even when the only growing live object is the result array.

## Decision

Store the current source, destination, index, mapper and pending completion
in a charged heap record. Two native callbacks capture it through ordinary
promise reactions. These callbacks run in their creating realm and follow the
promise queue's existing FIFO order. The collector traces the record through
the same queued jobs and pending promise reactions as other async operations.

Check the existing collection threshold between promise jobs, before popping
the next job and while the entire queue is reachable. The checkpoint continues
to use the existing host collector when an execution host is attached. Native
bodies continue to run without collection through their local handles.

## Alternatives and consequences

An interpreter frame could reuse the existing `await` machinery with a new
bytecode entry point. The native callbacks use the existing iterator adapter,
intrinsic Promise resolution and `CreateDataProperty` machinery directly. They
also preserve a single outstanding await without preparing a guest script or
accumulating future input. Larger results still consume the agent's heap budget.

## Validation and revisit trigger

`rustic-js/tests/gc.rs` checks the original rejection across a suspended
iterator close and processes 30,000 native awaits on a 2 MiB heap. Revisit the
job-boundary policy if checkpoint profiling shows collection overhead on
native promise workloads or a new native continuation needs another root.
