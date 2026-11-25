# ADR 0087 — Resizable unshared Wasm memory

Date: 2026-11-25
Status: Accepted for the M9 unshared memory profile

## Context

An unshared Wasm memory can expose a fixed or resizable ArrayBuffer.
Growth of a resizable buffer preserves its JavaScript identity while
length-tracking views observe the updated bounds. Converting either
buffer mode to the other replaces the wrapper and detaches the old one.

## Decision

Each linear byte generation records whether growth can preserve its
views. Mode conversion prepares a replacement wrapper and cache entry
while the new generation is temporarily detached. On success the
store moves its existing vector to the new generation. A failed
preparation leaves the original view and bytes intact.

An external resizable ArrayBuffer carries its declared byte maximum and
a host callback. The ArrayBuffer builtin converts the requested length
and rechecks attachment before dispatch. The Wasm binding accepts
whole-page growth through the same path as `Memory.grow()`, with the
existing allocation and work limits. The guest's growth instruction
shares that allocation state. Window exposes the declared maximum as a
Number, including when the owning memory has a lower physical byte cap.

## Alternatives considered

Copying external bytes into an ordinary resizable ArrayBuffer would
require synchronization after each access. The generation retains the
same backing vector through growth so the guest and JavaScript views
can observe each other's writes directly.

## Consequences and revisit trigger

The buffer keeps tracing its allocation owner while attached. A detached
generation retains only its metadata. Both 32- and 64-bit memories keep
the existing 8 MiB browser cap per allocation owner. Revisit generation
and growth synchronization when adding shared Wasm memories.

## Validation

The native generation test verifies failure atomicity. Window tests cover
guest growth, host growth, ArrayBuffer resize, explicit and tracked view
lengths, argument coercion, mode changes, the memory64 maximum and GC
reachability. Pinned Web Platform Tests exercise the JavaScript API.

[WebAssembly JavaScript Interface, memories](https://www.w3.org/TR/2026/CRD-wasm-js-api-2-20261123/#memories)
