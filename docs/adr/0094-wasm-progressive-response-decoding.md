# ADR 0094 — Progressive WebAssembly Fetch decoding

Date: 2026-11-26
Status: Accepted for Window in M9

## Context

Wasm compilation has access to response headers before the entire payload
arrives. Buffering it in the broker postpones decoding and prevents early
rejection when a well-framed section contains malformed instructions.

## Decision

Readable Wasm MIME responses can retain a native pull reader after Fetch
filters their headers. Both renderer modes pull bytes through the same
Window body-consumption path. The renderer checks response branding, status,
MIME and CSP before decoding. The decoder validates framing and decodes
completed sections, including code sections, while retaining a bounded byte
snapshot. At EOF it resolves borrowed declarations and validates the entire
module before publishing a Module.

The broker keeps origin authority and gives each renderer handles only for
its filtered responses. Protocol 3 carries a handle on the Fetch reply and
supports chunk pulls and explicit release. EOF, transfer failure and closing
a reader remove the handle. The document session caps outstanding readers at
16 across its frames. The broker also bounds its handle table and checks
every request. A response beyond the reader cap uses buffered delivery.

Response cloning buffers a deferred body for its independent consumers.
Regular body methods keep their existing complete-value API. The source
never calls back into JavaScript and aborted decoding releases its reader.

## Alternatives considered

Compiling in the broker would expose the privileged process to binary
decoding. Buffering and then splitting bytes in the renderer would preserve
the transport stall. Pulling admitted chunks keeps the decoder in the
renderer and reuses existing transport backpressure.

## Consequences and revisit trigger

Window can reject malformed input without waiting for all network bytes.
The renderer still pulls and compiles in one bounded promise reaction.
TD-200 tracks separate scheduling and completion via the networking task
source. TD-169 covers public ReadableStream bodies.

## Validation

The core tests compare every split of a module, including split LEB lengths,
to the complete decoder. They exercise early rejection, truncated input,
limits and cancellation. The Wasm fuzz target compares both decoders on
mutated inputs. The network regression runs ordinary and brokered
renderers, feeds a valid module in small pieces, and verifies early rejection
with the remaining bytes held open. It also checks clone reads on completed
and truncated transfers. The IPC tests round-trip the new message forms.

[WebAssembly Web API, streaming compilation and instantiation](https://webassembly.github.io/spec/web-api/#streaming-module-compilation-and-instantiation)
