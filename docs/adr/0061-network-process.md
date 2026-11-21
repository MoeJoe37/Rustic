# ADR 0061 — Network process

Date: 2026-11-21
Status: Accepted for the CLI prototype (verified on Linux x86-64 in the sandbox gate; OS-confined on Linux only, TD-176)

## Context

After ISO-02 the renderer holds no authority, but the broker still opens
sockets, runs TLS, parses HTTP/1.1 framing and undoes `gzip`, `deflate`
and `br` content codings for every request: the code that reads hostile
bytes from servers runs in the process that holds the user's files,
cookies and profile. ISO-07 asks that network crashes be survivable, and
the M7 status listed "a network process" as open. Chromium's network
service and Gecko's socket process make the same split.

## Decision

1. **Decisions stay in the broker, exchanges move out.** The broker keeps
   origin grants, CSP, mixed content, CORS, cookies, the HTTP cache,
   ORB and COEP. `rustic_net::Transport::with_connector` hands each
   request *after* the broker's `NetworkPolicy` has granted it, together
   with the granted address and the exchange limits, to a `Connector`.
   In a split run the connector is a `rustic __network` child
   (`rustic-cli/src/netproc.rs`) that runs the socket, TLS, framing and
   content decoding and streams the result back. `--single-process`
   keeps exchanges in process.
2. **Protocol.** A separate role (`Role::Network`, handshake byte 3) and
   message set (`rustic_ipc::network`): `Configure` (an optional
   replacement trust store) first, then `Open`, `Cancel` and `Credit`;
   replies are `Head`, `Body`, `Finished` and `Failed` with numeric error
   codes. Frames are capped at 16 MiB and decoded as strictly as ADR 0056.
3. **The network process is a hostile peer.** Heads are revalidated
   (`ResponseHead::from_parts`: final status 200 to 999, reason without
   control bytes; every field through `Headers::append`; at most the
   default header count). Body bytes follow credit: the child may have at
   most one window unacknowledged per exchange, the broker returns credit
   as the consumer reads, and `RemoteFeed` refuses bytes beyond the window,
   events out of order, events for exchanges never opened and unknown
   error codes. Any violation kills the process. The child re-checks the
   grant (loopback only, TD-157) as defence in depth.
4. **Crash recovery.** A network process that exits, hangs up or breaks
   the protocol fails only its in-flight exchanges with
   `TransportError::NetworkProcess`; the next request starts a fresh
   process, at most four per run, so a process that dies on every request
   cannot loop. The child holds no state worth recovering: cookies and
   cache live in the broker. Requests are not retried (a retried `POST`
   could repeat a side effect).
5. **Diagnostics.** The child's standard error is relayed with control
   characters escaped, as for renderers.
6. **Confinement (Linux).** When renderers are sandboxed (the Linux
   default), the network process runs under the same bubblewrap launcher
   with `--share-net` added back, the trust-store directories bound
   read-only, and `seccomp::network_filter`: the renderer allowlist plus
   `connect`, `send*`/`recv*`, `{get,set}sockopt`, `getsockname`,
   `getpeername` and `shutdown`, with `socket` admitted only for
   `AF_INET` and `AF_INET6`. Unix-domain sockets are refused because a
   shared network namespace exposes abstract sockets (X11, D-Bus,
   container runtimes). Setup failure is an error, as for renderers.
   Windows and macOS start it unconfined for now (TD-176).

## Consequences

The broker no longer links its decisions to code that parses server
bytes, and a parser crash costs the requests in flight rather than the
browser. Each run pays one more process start and one copy of every body
through a pipe (TD-170). On Linux a compromised child can reach only
loopback TCP origins (which the broker could reach anyway) and nothing on
disk but system libraries and trust roots; on Windows and macOS it still
runs with the user's rights (TD-176), so there the split buys crash
isolation and a narrower broker, not containment.

## Revisit

When the network process gets its OS sandbox (Linux: a bubblewrap
profile that keeps the network namespace and the trust store, Windows:
an AppContainer with network client capabilities), when DNS arrives
(TD-157, which belongs in this process), when the M8 shell needs one
long-lived network process shared by many renderers, and when shared
memory replaces pipe copies (TD-170).
