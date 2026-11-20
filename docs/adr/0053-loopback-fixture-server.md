# ADR 0053 — In-process loopback fixture server for network tests

Date: 2026-11-20
Status: Accepted for NET-01 and later M6 tests

## Context

The M6 entry checklist requires that tests never touch the public
internet, that the fixture server is in-process and bound to loopback
only, and that it works under the emulated tiers. Network tests also need
behaviour that real servers rarely show on demand: partial heads, stalls,
slow drips, malformed framing and early closes.

## Decision

`rustic_net::fixture::FixtureServer` binds `127.0.0.1` on an ephemeral
port, accepts on one thread and serves each connection on its own thread.
A route maps a request-target to a scripted `Reply`: a list of writes,
pauses and an optional "hold the connection open" step, after which the
connection closes. `Reply::response` builds an ordinary `Content-Length`
response; raw bytes script everything else. Unknown targets get a 404.
The server records each request it reads (method, target, header fields,
body) and counts connections, so tests can assert what Rustic sent and
that a blocked request opened no connection. Dropping the server stops it
and joins the accept thread.

The server is part of the `rustic-net` library rather than a test-only
module so that later crates (`rustic-engine`, the CLI's tests) can use the
same fixtures. It binds loopback only and has no configuration that could
make it listen elsewhere.

## Alternatives considered

- **An external server process** (Python, a WPT server). Needs a second
  runtime on every tier and leaks process management into tests. The WPT
  server may still be used later for imported WPT network tests.
- **An in-memory fake socket.** Fast and deterministic, but does not test
  the real socket timeouts, flow control or close behaviour that NET-01 is
  about. Kept as an option for parser-only tests, which already work on
  byte slices.

## Consequences

- Tests depend on loopback TCP, which every tier, including QEMU user-mode
  emulation, provides.
- Timing tests use generous upper bounds (5 s) and lower bounds tied to the
  configured limits, so a loaded machine slows them but does not fail them.

## Validation

All of `crates/rustic-net/tests/transport.rs` runs against this server.

## Revisit when

TLS fixtures are needed (with ADR 0051's TLS part), or HTTP/2, or WPT's
network tests are imported.
