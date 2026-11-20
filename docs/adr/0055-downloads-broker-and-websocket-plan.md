# ADR 0055 — Downloads broker contract and WebSocket plan

Date: 2026-11-20
Status: Accepted (contract and plan; implementation lands with M7 and M9)

## Context

NET-07 asks for the contract a download must follow and a plan for
WebSocket, without implementing either before the M7 isolation gate.
`rustic_net::navigation` already ends a navigation in
`NavigationOutcome::Download` for `Content-Disposition: attachment` and
types Rustic cannot render, with a `Download` record (URL, MIME type,
filename, body). Today the CLI reports it as an error and writes nothing.
Both features give a page reach beyond its renderer: a download writes to
the user's file system, and a WebSocket holds an open network channel the
transport's request/response model does not cover.

## Decision

### Downloads

1. **Only the broker writes files.** A renderer (or the engine on its
   behalf) never opens a path. It hands the broker a `Download` together
   with the initiating document's origin, whether the navigation had user
   activation, and the sandbox flags of the browsing context. The broker
   is the browser process in M7 (ISO-02 capability).
2. **The user decides.** Without user activation, a download from a
   cross-origin frame, from a sandboxed context without
   `allow-downloads`, or from an opaque origin is refused by default
   (Chromium's and Firefox's behaviour). Every other download asks
   unless the user chose a default folder.
3. **Names are sanitized.** `Download::sanitized_filename` (already
   implemented and tested) replaces path separators, `:*?"<>|` and
   control characters with `_`, trims leading dots and trailing dots and
   spaces, caps the name at 255 bytes, and falls back to the URL's last
   segment or `download`. The broker additionally refuses Windows device
   names (`CON`, `NUL`, `COM1`…) and executable-looking renames. The broker adds a numeric suffix rather
   than overwrite, and writes to `<name>.part` until the body completes,
   then renames atomically, as `rustic-storage` writes profile files.
4. **Bodies stream under a budget.** The broker reads the remaining
   `Exchange` with backpressure; the download's size limit and the free
   disk check apply as bytes arrive; cancellation removes the `.part`
   file. Mixed content: an `http:` download from a secure context is
   blocked (Mixed Content §4.4), and `X-Content-Type-Options` does not
   make a download executable.
5. **No automatic opening.** Downloaded files are never opened or
   executed by Rustic; the platform's quarantine attribute (macOS
   `com.apple.quarantine`, Windows Mark of the Web) is set where the
   platform has one.

### WebSocket

1. **Transport.** RFC 6455 framing and the opening handshake are Rustic
   code in `rustic-net`, over the same allowlisted `Transport`
   connections and TLS adapter (`wss:` verifies certificates exactly like
   `https:`). The upgrade request is a Fetch request with mode
   `websocket`, so cookies, `Origin`, CSP `connect-src` and mixed
   content (an `ws:` connection from a secure context is blocked) apply
   unchanged. HTTP/2 and HTTP/3 WebSockets (RFC 8441/9220) wait for those
   transports (TD-156).
2. **Limits.** Message and frame size, fragmented-message reassembly,
   ping interval and the per-document connection count are bounded and
   charged like other network buffers; a server sending faster than the
   page reads is throttled through the existing byte-counted window.
3. **Events.** Incoming messages and state changes queue on the
   WebSocket task source of the owning document; closing the document
   closes the connection (`1001`).
4. **Bindings.** `WebSocket` (and later `WebSocketStream`) arrive with
   the M9 web API work, after Blob and ArrayBuffer bodies (TD-169).
   Compression (`permessage-deflate`) waits for a streaming inflater
   (TD-162).

## Consequences

The CLI keeps failing on downloads until the M7 broker exists, which is
the safe default. Nothing in M6 opens a socket outside a request.

## Revisit

When ISO-02's broker capabilities are designed (downloads), or when M9
starts the WebSocket bindings.
