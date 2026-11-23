# ADR 0077 — WebAssembly decoding and execution boundaries

Date: 2026-11-23
Status: Accepted for M9 WASM-01–03 foundations

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
syntax and validation passes. Module decoding produces typed declarations and
flat instruction sequences, with names and data payloads borrowed from the
input. Absolute byte offsets identify syntax and validation errors. Bound
metadata allocation and traversal of length-prefixed records.

Keep binary decoding and type validation as execution-independent stages. The
validator builds import-first index spaces and checks bodies with explicit
operand and control stacks. It budgets work across the module, including
multi-value branches and initializer expressions. The validated module type
can only be created by this pass. The numeric interpreter consumes that type,
resolves structured control targets in a bounded preparation pass, and runs
functions using explicit value, local, call and control stacks. Invocation work
and stack budgets apply to the start function and every export call. Trap
diagnostics retain byte offsets because the interpreter reads the validated
instructions directly.

Host imports will be supplied through an embedding adapter. Mutable instance
state will have its own allocation limits. A later compilation tier requires
its own executable memory and platform review. Every host retains the interpreter.

## Alternatives and consequences

Using a third-party runtime would obscure the project requirement for an
original decoder and interpreter. Eagerly decoding every section at the
framing boundary would also combine resource admission with feature and
instruction validation. The borrowed representation gives those passes
their input and exact source offsets. An embedder that retains a compiled
module will need an owned validated representation or retained input bytes.

## Validation and revisit trigger

The core profile follows the [WebAssembly core specification](https://webassembly.github.io/spec/core/)
for numeric and multi-value control flow, direct and indirect calls, abstract
function and external references, and 32-bit memories and tables. GC and
concrete reference types, SIMD, shared memory, 64-bit addressing, and exception
instructions need further coverage before the selected product profile is
complete.

Tests check framing and declaration encodings, initializer visibility, operand
types, branch targets, index spaces, cancellation and cumulative budgets.
Execution tests exercise numerical results and traps, recursive and tail calls,
loop and block branches, start traps, and work and stack limits. The
`wasm-binary` fuzz target exercises decoding, validation, and bounded numeric
execution on mutated modules. Revisit the representation when concrete
reference types need subtype and canonical identity information, or when
instantiation requires an owned input lifetime.
