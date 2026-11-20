# ADR 0052 — Network I/O on exchange threads with a bounded event window

Date: 2026-11-20
Status: Accepted for NET-01 (`network.transport`)

## Context

The M6 entry checklist asks whether network I/O uses a reviewed async
runtime or Rustic's own poll loop behind the existing task sources, with
the rule that the scheduler of ADRs 0021–0023 stays the only place where
page-visible ordering is decided. NET-01 needs cancellation, connect, idle
and total timeouts, streaming and backpressure.

The standard library has blocking sockets with read and write timeouts
but no portable readiness API (`poll`, `epoll`, `kqueue`, IOCP). An async
runtime (tokio, smol) would supply one, at the cost of a large dependency,
its own threads and executor semantics, and a second scheduler next to
Rustic's. A hand-written readiness layer would need `unsafe` FFI on five
OS families.

## Decision

Each `Exchange` runs on its **own thread with blocking sockets**. Read and
write timeouts are set to `poll_interval` (10 ms by default), so a
blocked call returns at least that often and the thread checks
cancellation, the total deadline and the idle timer between calls.
Connecting uses `TcpStream::connect_timeout` bounded by `connect_timeout`
and by the remaining total time.

Results flow through a **bounded event window** (`pipe.rs`): body bytes
count against `buffer_window`, and the network thread waits while the
window is full, without reading the socket, so TCP flow control slows the
peer. One body piece is always accepted into an empty window, so a piece
never deadlocks. Waiting for the consumer is not idleness of the peer and
does not count toward `idle_timeout`; it does count toward the total
deadline and stops on cancellation.

The consumer side (`Exchange::next_event`) never blocks unless asked to.
NET-02 drains exchanges from a networking task source inside the existing
document scheduler, which keeps ordering decisions there. Dropping an
`Exchange` cancels it and closes its connection.

## Alternatives considered

- **tokio or another async runtime.** Mature and scalable, but a large
  dependency graph, a second scheduler, and an executor model that would
  leak into `rustic-engine`. Rejected for the development prototype.
- **Own readiness poll loop over OS APIs.** Scales like a runtime without
  the dependency, but needs `unsafe` FFI per OS. Rejected until M12 shows
  that thread-per-exchange is a bottleneck.
- **Message-count bounded channel** (`sync_channel`). Bounds events, not
  bytes, so memory would depend on read sizes. Rejected for the
  byte-counted window.

## Consequences

- One OS thread per in-flight exchange. A page with hundreds of parallel
  subresources costs hundreds of threads; connection reuse and a bounded
  pool are TD-156.
- Cancellation latency is at most one `poll_interval` after connecting.
  A connect in progress cannot be interrupted and is bounded by
  `connect_timeout` (TD-156).
- No dependency, no `unsafe`, and the same code on every tier, including
  the emulated 32-bit and big-endian targets.

## Validation

`crates/rustic-net/tests/transport.rs`:
`a_slow_consumer_bounds_buffered_bytes` sends 4 MiB through a 64 KiB
window and checks the byte-exact body and that the peak buffered bytes
stay within one read of the window; `idle_timeout_while_waiting_for_the_head_and_the_body`,
`a_dripping_peer_is_not_idle_but_hits_the_total_timeout` and
`cancellation_and_drop_stop_the_exchange` check the time limits and
cancellation. `pipe.rs` unit tests check the window rules directly. The
suite was run 15 times in a row and 6 times with all cores loaded without
a failure.

## Revisit when

- PERF work (M12) or a page corpus shows thread count or wake-up latency
  as a cost, or
- HTTP/2 multiplexing needs one connection for many exchanges, or
- the M7 broker moves network I/O into another process.
