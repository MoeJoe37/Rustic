# Implementation progress

This ledger describes completed code, not a promise of web compatibility. The
[roadmap](ROADMAP.md) remains the full plan. Each finished unit is committed and
pushed with focused tests and updated contracts.

## DOC-01 — Shared document-core primitives

Implemented in `rustic-base`, with no external dependencies and no unsafe code:

| Contract | Behavior | Regression evidence |
| --- | --- | --- |
| `DocumentId` | Process-local monotonic allocation; exhaustion never wraps or reuses an ID | Concurrent allocation and exhaustion tests |
| `NodeId` | Typed document/slot/generation identity; ownership and generation must be checked by storage | Storage checks land with the DOM arena |
| `Budget` | Explicit charge/release; failures preserve usage; arithmetic never wraps | Exact-limit, overflow, zero-capacity and release-underflow cases |
| `CancellationToken` | Shared one-way cooperative cancellation, including across threads | Clone sharing, repeat cancellation and independent-token cases |
| `SourceSpan` | Half-open byte offsets; checked bounds and UTF-8 slicing | Reversed ranges, extreme offsets and split multibyte characters |
| `Clock` | Monotonic elapsed time; real and agent-local manual implementations | Deterministic advancement and overflow preserving time |

IDs are labels, not permissions or strong references. A `NodeId` constructor
cannot create or authorize a node. Lookup must reject the wrong document,
generation, vacant slot and out-of-range slot. Cross-process identity and IPC
authorization remain future work.

Budget units belong to the consumer: node count, input bytes or another explicit
resource. This primitive is not a total memory limit and does not automatically
free resources. Cancellation is checked cooperatively; it does not preempt blocked
code. Source spans refer to the caller's matching buffer; they are byte offsets,
not JS UTF-16 indices or persistent source-revision handles.

Next: DOC-03 and the first DOC-04 slice implement bounded node storage, checked
tree mutation and deterministic inspection. URL parsing, HTML parsing, browser
bindings, tracing GC and page execution remain unimplemented.
