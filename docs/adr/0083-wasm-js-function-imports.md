# ADR 0083 — Reentrant WebAssembly function imports

Date: 2026-11-24
Status: Accepted for the M9 numeric Window profile

## Context

An imported JavaScript function may call another export while a guest
invocation is still active. Both invocations must see the same persistent
globals, memories and tables. The outer invocation keeps its control frames,
operands and budgets during the host call.

## Decision

The owned interpreter accepts numeric import dispatch by function index. The
dispatcher is borrowed for a start call or exported invocation. The runtime
stores signatures and keeps one thread-confined guest store per instance.
Each stateful opcode borrows the store locally. Host calls run after those
borrows have ended, with a separate stack for any nested invocation. Rust
embedders can continue to link their named callbacks.

The Window host admits callable bindings in module declaration order. A
private ephemeron table traces the imported functions from the instance. It
also records the owner of an exported function so the host can check linked
signatures and preserve the re-export's identity. Direct numeric calls between
instances keep float bit patterns in the interpreter. JavaScript exceptions
unwind through the host boundary and retain their original thrown value.

## Alternatives considered

A long-lived exclusive borrow across the guest invocation would block a
callback's access to the same state. Temporarily moving the state out would
also prevent a nested guest from seeing earlier writes. A resumable interpreter
could release the borrow when suspending and provides a future option for
asynchronous hosts. Dispatch is synchronous in the current Window profile.

## Consequences and revisit trigger

Ordinary guest control flow still uses the existing iterative interpreter.
The Window bridge separately limits nested host calls to 32 and callback
result iteration to one million items. The embedder's instruction and heap
limits also apply. Cross-instance calls each use their target's invocation
budget. Revisit these boundaries when adding shared state, reference-valued
calls, Wasm exception handling or asynchronous execution.

## Evidence

The GC-stressed Window regression checks ordered lookups, start callbacks,
same-instance reentry, depth exhaustion and recovery, exception identity,
streaming instantiation, mismatched signatures, re-export identity and
raw NaN payload preservation.

[WebAssembly JS API, exported functions and host calls](https://webassembly.github.io/spec/js-api/#exported-functions)
