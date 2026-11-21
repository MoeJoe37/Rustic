# ADR 0056 — Broker/renderer IPC format

Date: 2026-11-20
Status: Accepted for the M7 prototype (revisit before shared memory or more process types)

## Context

ISO-02 needs a versioned, bounded channel between the broker (browser
process) and renderers that is safe when either side is compromised
([SECURITY.md](../SECURITY.md#isolation-rollout)). The threat is a
renderer running hostile content: it may send any bytes, any sizes, in
any order, or die at any moment. The broker must neither crash, allocate
what a peer only claims, nor act on a request it did not validate.

Options considered: a serialization library (serde formats, protobuf,
Cap'n Proto, Mojo-style IDL) or a small hand-written format. A library
is a reviewed dependency and often trades bounds checks for speed or
generality; Rustic's message set is small.

## Decision

1. **Own format, `rustic-ipc`, std only, no unsafe.** A connection opens
   with `RUSTICIP`, the protocol version (u16, exact match required: both
   ends are the same build) and the sender's role; a mismatch ends it.
2. **Frames** are a u32 little-endian length and a payload of at most
   `Limits::max_frame_bytes` (128 MiB, enough for the largest document,
   image and PNG budgets). Oversized announcements are refused before
   reading; payloads are read in 64 KiB steps so a stalled peer costs
   only what it sent.
3. **Payloads** start with a u16 message tag. Fields are little-endian
   integers and u32-length-prefixed byte strings; strings must be UTF-8,
   flags 0 or 1, UTF-16 even-length, counts below protocol limits and the
   bytes left. Decoding must consume the whole frame. The encoding is
   canonical (fuzz target `ipc-frame`).
4. **Transport** is the child's standard input and output, which exist on
   every desktop and mobile platform without descriptor passing. End of
   stream is `IpcError::Closed`: the peer exited or crashed, and the
   other side stops waiting.
5. **Protocol 1** (`rustic_ipc::protocol`): the broker sends one `Start`
   (command, non-authority arguments, and the granted document bytes and
   policies); the renderer asks for every subresource, `fetch()` and Web
   Storage operation with a numbered request and gets one reply; it ends
   with `Done` (status and output). Requests carry claims (URLs, nonces,
   kinds) that the broker re-validates against its own grant.
   **Protocol 2** (2026-11-21, [ADR 0062](0062-out-of-process-frames.md))
   adds `Frame` requests (an `iframe` to load in another renderer,
   answered with a handle) and the `Surface` a `render` sends before
   `Done`: premultiplied pixels and frame layers, which the broker
   validates, composites and encodes.
6. **No shared memory yet.** Bulk data is copied through the pipe
   (TD-170). Shared memory needs platform calls behind unsafe code or a
   reviewed crate, and validation of every mapped read.

## Consequences

Bytes cross twice for large images and PNGs. One outstanding request per
renderer keeps the protocol simple but serializes loads (as TD-165
already does). Adding a message is a protocol change: bump
`PROTOCOL_VERSION`.

## Revisit

When a GPU or network process is added, when shared memory is needed for
frames, or if profiles show IPC copies on a critical path.
