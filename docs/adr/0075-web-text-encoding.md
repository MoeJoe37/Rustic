# ADR 0075 — Window text encoding and native binding state

Date: 2026-11-23
Status: Accepted for M9 Window `TextEncoder` and `TextDecoder`

## Context

The encoding crate already implements the WHATWG decoder algorithms. The
Web IDL interfaces add per-instance stream and BOM state, buffer views,
argument conversion that can run script, and realm-specific return values.

## Decision

Install the interfaces with the other Window interfaces in `rustic-bindings`.
Allocate a budgeted native payload with the JavaScript wrapper. The payload
holds Rust state without JavaScript references or host authority. The
collector reclaims it with the wrapper. Convert arguments and read
dictionary properties before accessing the current decoder state.

Use the engine's buffer slots to validate inputs and respect typed-array
and DataView ranges. Read bytes in bounded chunks without calling script.
Shared buffers currently live within one agent. A view detached by an
options getter contributes zero bytes. The UTF-8 encoder counts complete
scalars before writing to a destination, and charges temporary output against
the agent's heap budget.

## Alternatives and consequences

A page-owned decoder table would need a separate mapping and collection
hook to discard native entries when their wrappers die. GC-owned payloads
give private state the same lifetime as the corresponding wrapper. The IDL
surface stays in the bindings crate, and the encoding crate stays free of
JavaScript and browser dependencies.

## Validation and revisit trigger

Bindings tests cover prototype branding, coercion order, reentry, buffer
detachment, shared buffers, streaming boundaries and BOM handling. The
pinned Window WPT selection includes 12 Encoding API files. Two expectations
in `textdecoder-mistakes.any.js` preserve decoder state after a streaming
fatal error. The current Encoding Standard specifies a fresh decoder after
that error, and the runner records the difference against this WPT pin.
Revisit buffer reads before sharing backing stores across concurrent agents.
