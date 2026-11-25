# ADR 0085 — Wasm fixed-length memory buffers

Date: 2026-11-24
Status: Accepted for the M9 unshared 32-bit Window profile

## Context

A guest memory, its JavaScript Memory object and its typed views must
observe the same writes. Successful growth replaces the fixed-length
buffer, even when the page count stays the same. The budget charge for
memory needs to survive as long as an attached view can use it.

## Decision

A linear store has one current generation. Wasm and JavaScript borrow
the bytes for synchronous operations. Successful growth reserves and
zeroes storage, then moves the vector into a new generation if a view
exists. The empty old generation reports as detached. An attached
ArrayBuffer traces the Memory wrapper, and an exported wrapper traces
the owning instance. An importer traces its original wrapper, including
when the import has no re-export. Ordinary ArrayBuffers keep their
owned byte storage.

Each standalone memory has a byte cap and each instance has an aggregate
cap for the memories it creates. An imported handle keeps its original
allocation pool. Bounds failures and reserve failures leave growth
unapplied. Instantiation prepares its data writes before copying. If
a data segment traps, the checked prefix remains visible to an importer.
Bulk copies compare backing identity so two aliases use overlap-safe copy.

## Alternatives considered

Copying between the JS and guest stores would obscure live mutations and
increase the cost of growth and callbacks. Retaining each old allocation
would keep memory alive through detached views. A detachable generation
makes the transition observable without a second persistent copy.

## Consequences and revisit trigger

Window supports unshared wasm32 memories and fixed-length views.
The browser caps standalone and aggregate instance-owned memory at 8 MiB.
[ADR 0086](0086-wasm-memory64.md) extends this model to 64-bit memories.
Shared memory and resizable Wasm views require additional ownership
and synchronization work. Revisit the byte limit when measuring larger
workloads or introducing a reserved virtual memory implementation.

## Evidence

The core and Window tests exercise shared identity, live writes in both
directions, checked import limits, data-segment traps, overlapping copies,
growth failures, zero-page growth and detachment. The GC-stressed Window
test retains an attached buffer after dropping its owner variable.
The pinned memory fixtures pass 49 of 51 subtests. Shared memory growth
and conversion to a resizable buffer account for the expected failures.
Three existing constructor subtests now pass with memory bindings.

[WebAssembly JS API, memories](https://webassembly.github.io/spec/js-api/#memories)
