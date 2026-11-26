# ADR 0080 — Window WebAssembly module compilation

Date: 2026-11-24
Status: Accepted for M9 module compilation

## Context

Page JavaScript needs to validate and compile Wasm bytes using Rustic's core
decoder. The resulting object has realm-specific identity and prototypes.
Network responses enter through the existing Fetch binding and document CSP.

## Decision

Expose a Window `WebAssembly` namespace and realm-owned `Module` and error
constructors. A module object owns a byte snapshot and validated import,
export, and custom section metadata. The native record contains no JS
references. Reflection creates new descriptors and copies custom bytes
into new ArrayBuffers.

The Window adapter uses the core decoder and validator with byte, declaration,
instruction, scratch and work budgets. Promise jobs retain their inputs through
traced native captures. Buffer compilation snapshots the supplied range before
the job runs. WebAssembly validation remains available under CSP based on
the non-executing validation path.

Document policy checks `script-src` with `default-src` fallback. Each policy can
allow Wasm using `'wasm-unsafe-eval'` or `'unsafe-eval'`. The Fetch entry point
uses internal response state and requires a readable response with a successful
status and an exact, case-insensitive `application/wasm` Content-Type. It
consumes the body through the same used flag as the Fetch body mixin.

## Consequences and revisit trigger

The initial asynchronous path runs bounded compilation as an agent job, using
the Fetch binding's buffered body. [ADR 0094](0094-wasm-progressive-response-decoding.md)
adds incremental input. TD-200 tracks moving compilation and fulfillment to
the networking task source. A retained module's code can
share the byte snapshot when the ownership model for executable instances is
introduced. Instantiation and JavaScript import/export adapters will extend
the module boundary.

## Evidence

The Window binding tests check real prototypes, exact views, byte snapshot
timing, metadata copies, errors and cancellation budgets. A network fixture
exercises MIME acceptance and both enforced and report-only document policies.
The pinned WPT selection checks the original JS API fixtures for compilation,
validation and module reflection.

[WebAssembly JS API](https://webassembly.github.io/spec/js-api/),
[Web API](https://webassembly.github.io/spec/web-api/),
[CSP Level 3 §4.5](https://w3c.github.io/webappsec-csp/#wasm-integration)
