# ADR 0077 — WebAssembly decoding and execution boundaries

Date: 2026-11-23
Status: Accepted for M9 WASM-01 foundations

## Context

Rustic needs an original WebAssembly runtime that works with the JavaScript
heap, supports cancellation and resource limits, and can run without executable
memory. A binary may describe valid module sections while carrying invalid
instructions, indexes or types. These checks need distinct entry points and
evidence.

## Decision

Start `rustic-wasm` with a bounded reader over untrusted bytes. Borrow section
and function instruction bytes from the input. Decode framing, local
declarations and function type indexes before handing them to subsequent
syntax and validation passes. None of these operations instantiate anything.
Use absolute byte offsets for diagnostics and enforce resource limits before
allocating metadata or traversing length-prefixed records.

Keep binary decoding and type validation as execution-independent stages.
An interpreter will consume validated instructions under explicit stack,
memory and execution limits. Host imports will be supplied through an
embedding adapter. A later compilation tier requires its own executable
memory and platform review, with the interpreter available on every host.

## Alternatives and consequences

Using a third-party runtime would obscure the project requirement for an
original decoder and interpreter. Eagerly decoding every section at the
framing boundary would also combine resource admission with feature and
instruction validation. The borrowed representation gives those passes
their input and exact source offsets. An embedder that retains a compiled
module will need an owned validated representation or retained input bytes.

## Validation and revisit trigger

Tests check integer overflow and permitted padding, section ordering,
metadata budgets, cancellation, custom-section names, function-body
boundaries, and structured control delimiters. The separate instruction
decoder handles core numeric, control, reference, memory and bulk operations.
The `wasm-binary` fuzz target checks offsets and read boundaries on mutated
modules. Further syntax coverage and type validation precede instantiation.
Revisit the representation if validation or debug mappings need different
ownership or data layout.
