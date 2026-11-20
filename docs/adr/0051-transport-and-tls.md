# ADR 0051 — Own HTTP/1.1, explicit network authority, rustls for TLS

Date: 2026-11-20
Status: Split. **Accepted** for HTTP/1.1 as Rustic code and the
origin-allowlist authority model (NET-01, `rustic-net`). **Proposed**, not
yet accepted, for the TLS library: no TLS dependency is added until the
maintainer accepts that part. Until then `https` fails closed.

## Context

M6 adds Rustic's first network code. The [M6 entry
checklist](../ROADMAP.md#m6-entry-checklist) asks which reviewed library
provides TLS and certificate verification, whether HTTP/1.1 is Rustic's own
code, and what the choice costs on every release platform (Windows,
macOS, Linux on x64 and ARM64; Android ARM64, 32-bit ARM and x86-64;
iOS/iPadOS) and on the 32-bit x86, RISC-V and big-endian compile tiers.
[DEPENDENCIES.md](../DEPENDENCIES.md) already allows a library for TLS and
HTTP framing and forbids implementing cryptographic primitives or
certificate verification ourselves.

Three separate questions:

1. **HTTP/1.1 framing.** A library such as hyper brings an async runtime
   contract (tokio traits or its own), many features Rustic would turn off,
   and its own idea of header and body limits. The framing that matters
   for security (Content-Length versus Transfer-Encoding, chunk sizes,
   header limits, interim responses) is small and is exactly where Rustic
   needs its own limits and tests. Fetch, redirects, CORS and caching sit
   above framing and are Rustic's in any case (ADR 0001).
2. **Authority.** Before the M7 isolation gate the engine must not browse
   the public internet ([ADR 0004](0004-isolation-rollout.md)), but the M6
   demo loads pages, styles, scripts and fetches from test servers.
3. **TLS.** Certificate verification must use each platform's trust store
   and never have a silent bypass (M6 gate).

## Decision

**HTTP/1.1 is Rustic code** in the new `rustic-net` crate (std only):
request serialization that refuses caller-supplied framing fields, an
incremental response parser whose result never depends on how input is
split, and body framing by `Content-Length`, `chunked` and close, each
bounded by `TransportLimits`. HTTP/2 and HTTP/3 are later work (TD-156).

**Authority is a `NetworkPolicy` allowlist.** It maps an origin (scheme,
host, effective port) to a socket address; nothing else can be contacted,
there is no DNS lookup, and before M7 every address must be loopback.
A request outside the list fails with `TransportError::Blocked` before any
socket is opened (TD-157).

**Proposed TLS stack:** `rustls` 0.23 with the `ring` provider, TLS 1.2
and 1.3, and `rustls-platform-verifier` 0.7 for certificates. The verifier
uses each platform's own verifier: Schannel/CryptoAPI on Windows,
Security.framework on macOS and iOS, the Android system verifier through
JNI, and on Linux the system roots through `rustls-native-certs` with
webpki path building. Rustic keeps HTTPS policy, certificate error
behaviour and the "no bypass" rule; the adapter never offers an
"accept invalid certificates" switch in non-test builds.

Until the TLS part is accepted, `https` requests fail with
`TransportError::TlsUnavailable`. They are never retried over `http`.

## Measured evidence (2026-11-20, x86-64 Linux)

A probe crate outside the repository with `rustls` 0.23.46
(`default-features = false`, features `ring`, `std`, `tls12`, `logging`)
and `rustls-platform-verifier` 0.7.1 built with Rust 1.99.1:

- Linux graph: 15 packages (`rustls`, `rustls-webpki`, `rustls-pki-types`,
  `ring`, `untrusted`, `subtle`, `zeroize`, `getrandom`, `libc`, `cfg-if`,
  `once_cell`, `log`, `rustls-native-certs`, `openssl-probe`,
  `rustls-platform-verifier`). All targets together: 53 packages, adding
  `security-framework` (Apple), `schannel` and `windows-sys` (Windows), and
  `jni` with `rustls-platform-verifier-android` (Android).
- Declared minimum Rust versions: `rustls` 1.71, `rustls-webpki` 1.71,
  `ring` 1.66, `rustls-platform-verifier` 1.85. All fit Rustic's MSRV of
  1.85.
- Licenses: MIT, Apache-2.0, ISC, BSD-3-Clause (`subtle`), Unlicense
  alternatives and, for `webpki-root-certs` (Android fallback roots),
  CDLA-Permissive-2.0. All are compatible with AGPL-3.0-only; the CDLA
  data license needs an entry in the reviewed manifest.
- `ring` compiles C and per-target assembly with the `cc` crate. It has
  assembly for x86, x86-64, 32-bit ARM and AArch64 and portable C for other
  targets (RISC-V, s390x). **Cost to state:** every target, including the
  emulated tiers and Android/iOS, needs a working C cross-compiler in CI.
- Not measured here (no cross toolchains, no QEMU, `cargo deny` not
  installed): cross builds for the other tiers, `cargo deny` advisories,
  and the Android Kotlin component's packaging.

## Alternatives considered

- **`aws-lc-rs` provider.** FIPS options and broader algorithm coverage,
  but it needs CMake and, on some Windows targets, NASM, and its build is
  heavier on every tier. Kept as a later option: `rustls` can switch
  providers without changing Rustic's adapter.
- **`native-tls`** (OpenSSL, Schannel, Security.framework). Three TLS
  implementations with different behaviour and error surfaces, and a
  system OpenSSL dependency on Linux and Android. Rejected: Rustic wants
  one protocol implementation and one error model everywhere.
- **`webpki-roots` only** (Mozilla's root list compiled in). Simple and
  identical everywhere, but ignores enterprise roots and user trust
  decisions, against the checklist's "each platform's trust store".
  Rejected as the default; it remains a test option.
- **hyper for HTTP/1.1.** See Context. Rejected for now; HTTP/2 may still
  use a reviewed library when it is scheduled.
- **Writing TLS ourselves.** Forbidden by DEPENDENCIES.md.

## Consequences

- `rustic-net` has no external dependency today. When the TLS part is
  accepted, its adapter is a separate module behind the same
  `Transport::start`, and `tools/check_repository.py`, `deny.toml` and
  `third_party/rust-dependencies.json` are updated in that change.
- All network access goes through one allowlist object, which M7 turns
  into a broker capability.
- Rustic owns HTTP/1.1 parsing bugs. They are covered by split-invariance
  tests, limit tests and a deterministic random-input test, and the parser
  is a fuzz target candidate (`rustic-fuzz`).

## Validation

- `crates/rustic-net/src/response.rs` unit tests parse every response at
  every two-piece split and byte by byte and compare with one-piece
  parsing; they cover interim responses, folding, conflicting lengths,
  `Transfer-Encoding` precedence, chunk extensions and trailers, and every
  limit.
- `crates/rustic-net/tests/transport.rs` runs against the loopback fixture
  server ([ADR 0053](0053-loopback-fixture-server.md)): blocked origins
  open no connection, `https` opens no connection, and protocol errors end
  the exchange with a typed error.

## Revisit when

- The maintainer accepts or rejects the TLS proposal (then this record's
  status changes, or a new ADR supersedes the TLS part).
- `ring` stops being maintained or fails to build on a release tier.
- HTTP/2 is scheduled (NET-07 or M9), or the M7 broker takes over the
  allowlist.
