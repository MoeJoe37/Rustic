# ADR 0082 — Live numeric WebAssembly globals

Date: 2026-11-24
Status: Accepted for the M9 numeric Window profile

## Context

A global wrapper exposes a storage location, so changes made by guest
instructions and host setters must be visible in both directions. A
script can keep a wrapper after dropping the instance and its exports record.

## Decision

Local interpreter slots retain their declaration type and mutability. Rust
host access checks the index, numeric type and mutability before a write.
Window constructs standalone numeric globals with their own storage. An
exported wrapper refers to an existing guest slot and holds its instance via
the same page-owned ephemeron table that links an instance to its exports
record. Each export index gets one wrapper per instance, even when multiple
names identify that index.

An exported value stays in a slot owned and charged by its instance. Copying
it into a separate JavaScript-side value would need to synchronize every
guest write, including writes made before a trap. Sharing an independently
allocated cell for every slot would support linked imports and incur that
allocation for ordinary local globals. The current representation keeps
local globals in one bounded vector.

## Consequences and revisit trigger

An exported wrapper keeps the underlying interpreter allocated and charged
while the wrapper is live. A setter performs JavaScript coercion before
borrowing the instance, allowing the coercion to call into the same Wasm
instance. Linking mutable globals across instances will need shareable
storage and a single accounting owner. Reference-typed globals additionally
need a traced reference representation.

## Evidence

The Rust regression verifies failure atomicity for invalid indices, types
and immutable writes. A GC-stressed Window test covers alias identity,
bidirectional mutation, isolated instances, reentrancy and wrapper lifetime.
Four files pinned to WPT revision `cb97c571c3b1947e08b8615eeb4028a2794b23a4`
exercise 132 numeric constructor, accessor, branding and string-tag cases.
[WebAssembly JS API, Globals](https://webassembly.github.io/spec/js-api/#globals)
