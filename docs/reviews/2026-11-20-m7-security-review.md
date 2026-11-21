# M7 security review (ISO-06) — 2026-11-20

Scope: the `feature` branch after the M7 isolation commits of 2026-11-20
(fuzz targets `cf8a184`, IPC `270c25e`, renderer split `9557be5`, site
isolation `c0f9534`, Linux sandbox `3198657`, activation and downloads
`825997e`), on native Linux x86-64 only. This is an internal review; no
independent external review has taken place.

## Attack surface

| Surface | Fed by | Boundary today | Evidence |
| --- | --- | --- | --- |
| HTML, CSS, URL, encodings, images (PNG, JPEG, GIF, WebP), fonts, JS | Page content | Renderer process (opt-in Linux sandbox) | 24 pre-M7 fuzz targets |
| HTTP/1.1 framing, content codings | Network peers | Broker | `net-http-response`, decompression-bomb test |
| Cookies, CSP, CORS, MIME sniffing, ORB, COEP | Network peers | Broker | `net-cookie`, `net-csp`, `net-cors`, unit tests |
| TLS (rustls, webpki, ring) | Network peers | Broker | `tls.rs` certificate-error fixtures, ASan run |
| Broker/renderer IPC | A possibly compromised renderer | Broker decoding and grant checks | `ipc-frame`, `hostile_frames_are_refused_without_allocating_their_claims`, compromised-renderer tests |
| Profile files | The user's disk (possibly corrupted) | Checksums, quarantine, bounded readers | `corrupt_files_are_quarantined_and_newer_formats_refused` |
| Downloads | Network peers | Broker, granted directory only | `downloads_are_saved_only_to_the_granted_directory_without_overwriting` |
| Terminal output | Renderer standard error | Broker relay escapes control characters | relay in `broker.rs` |

## Native and unsafe code

Project crates forbid `unsafe` (workspace lint `unsafe_code = "forbid"`);
the OS sandbox uses an external launcher (ADR 0058) rather than unsafe
system calls. Unsafe code and native code exist only in dependencies.
Counts are occurrences of the `unsafe` keyword in each crate's sources
and its C and assembly files, from `cargo metadata` at the locked graph:

| Crate | Role on Linux | `unsafe` | C / asm files | Notes |
| --- | --- | --- | --- | --- |
| ring 0.17.14 | TLS cryptography | 230 | 45 / 128 | BoringSSL-derived C and assembly; the largest native surface. Instrumented with ASan below |
| brotli-decompressor 6.3.1 | `br` content coding | 234 | 1 / 0 | Bounds-checked slices; fed only through `max_body_bytes` |
| rustls 0.23.46, rustls-webpki 0.103.17 | TLS protocol, certificates | 5 / 0 | 0 | |
| libc, getrandom | system calls | 811 / 59 | 0 | |
| memchr, bytemuck, smallvec, yoke, zeroize, bytes, simdutf8 | utilities | 340 / 391 / 83 / 129 / 42 / 171 / 154 | 0 | Widely used, fuzzed upstream |
| read-fonts, skrifa, harfrust, font-types | font parsing and shaping | 0 / 0 / 30 / 4 | 0 | Fed by fuzz targets `text-sfnt`, `text-shaping` |
| windows-sys, schannel, security-framework, jni | other platforms only | — | 0 | Not built for Linux |

No dependency is new in M7.

## Dynamic checks

- **AddressSanitizer** (nightly 1.102.0 in a separate rustup home, `-Zsanitizer=address`,
  C code built with `-fsanitize=address`): 546 unit and integration tests
  of `rustic-net`, `rustic-ipc`, `rustic-storage`, `rustic-codecs`,
  `rustic-html`, `rustic-bindings`, `rustic-engine` and `rustic-cli`
  (including TLS handshakes through ring's instrumented C, the split
  renderer and the IPC child process) pass with **no sanitizer report**.
  `detect_stack_use_after_return` was turned off: its heap-allocated
  fake stack defeats the JS compiler's stack-address depth guard, which
  then reports `TooDeep` (an artifact, not a memory error).
- **Fuzzing**: all 29 targets, seeds 11 and 12, 20,000 mutations each
  after their seed corpora: clean. The new network targets ran 800,000
  further mutations; their first campaigns found and fixed a panic on
  non-ASCII CSP nonces and a split-dependent HTTP framing error.
- **GC stress**: unchanged since M5 (`js-heap`, `js-run` collect at
  every safe point).
- **Renderer compromise**: compromised-renderer tests ask the broker for
  files outside the grant, unlisted origins, other schemes, a cross-origin
  `fetch()` and storage, and send malformed frames; all are refused or end
  the renderer. Under the Linux sandbox, direct file, network and process
  access from the renderer is denied.

## Release gates ([SECURITY.md](../SECURITY.md#general-remote-browsing-release-gates))

| Gate | Status on Linux x86-64 |
| --- | --- |
| URL/origin, redirects, inherited/opaque origins, response visibility have negative tests | Met (M6 tests) |
| CSP and loading policy cover all exposed script and resource paths | Met, except inline event-handler attributes, which are not compiled (TD-153) |
| Cookies, storage, cache, permissions, private profiles cannot cross keys | Met for cookies, storage, HTTP and preflight caches and private profiles; no permission store exists (TD-173) |
| Renderer sandbox launches with verified restrictions on each OS | Linux only, opt-in, no seccomp (ADR 0058, TD-172); not met elsewhere |
| Hostile renderer IPC cannot mint capabilities, escape budgets or reach other documents | Met for the CLI broker |
| Infinite scripts, decompression bombs, deep inputs terminate | Met (budgets, watchdog, bomb test) |
| Crashes cancel work without killing chrome | Met for renderers and (addendum 2026-11-21) the network process; no GPU process exists |
| GC tracing, codecs, FFI, shared memory have stress/fuzz coverage | Met; no shared memory exists (TD-170) |
| Certificate verification on, no silent fallback | Met |
| No unintended remote listener | Met: the CLI opens no listening socket |
| Documented review without release-blocking defects | This review: **blocking items remain** (below) |
| Brave comparison, independent review, extensions, updates | Not started |

## Release-blocking items

1. The sandbox is opt-in and Linux-only; Windows, macOS, Android and iOS
   have none (ISO-04, ISO-08), and no native runners exist (TD-12).
2. No seccomp filter (TD-172).
3. No independent external review.

General remote browsing stays disabled: the transport still reaches only
allowlisted loopback origins (TD-157).

## Addendum 2026-11-21 — default sandboxes on Linux and Windows

Release-blocking item 2 (no seccomp filter) is closed, and item 1 is
narrowed: the renderer is confined **by default** on Linux (bubblewrap
plus a measured seccomp-bpf allowlist, ADR 0058, TD-172 paid) and on
Windows (a capability-less LPAC AppContainer in a kill-on-close job,
ADR 0060), and failing to set up the sandbox is an error. The escape
probe in `crates/rustic-cli/tests/isolation.rs` (read a user file, list
a directory, write a file, connect to loopback, start a shell) succeeds
unconfined and fails confined on native Linux x86-64 (WSL 2) and native
Windows 11 x64; every CLI suite and, on Linux, the pinned WPT selection
pass with the sandbox on.

New native code: `crates/rustic-sandbox/src/windows.rs` is the first
project `unsafe` (Win32 process, AppContainer, job and pipe calls, one
documented operation per block, enforced by Clippy). It is reviewed here
as broker code: handles are owned by `OwnedHandle`, the attribute list
and the values it points to outlive `CreateProcessW`, strings are
NUL-terminated and arguments containing NUL are refused, the process is
created suspended and terminated if the job cannot be applied, and no
code path starts it unconfined. One finding during the work: a plain
AppContainer still reached loopback services on this Windows 11 23H2
machine with the firewall on, so the launcher uses LPAC, under which
Winsock cannot start.

Still blocking remote browsing: macOS has no sandbox (TD-174); Linux
aarch64, Windows ARM64, macOS, Android and iOS have no native run
(TD-12); no independent external review.

## Addendum 2026-11-21 — network process

The broker no longer parses server bytes in split runs: sockets, TLS
(rustls), HTTP/1.1 framing and content decoding (gzip, deflate, brotli)
run in a `rustic __network` child ([ADR 0061](../adr/0061-network-process.md)).
Reviewed as broker code, `rustic-cli/src/netproc.rs` treats the child as
hostile: heads are rebuilt through the same validators the parser uses,
body bytes beyond the per-exchange credit window, events out of order,
answers for exchanges never opened and unknown error codes kill the
child, and its standard error is relayed escaped. A crashed child fails
only its in-flight requests; restarts are capped at four per run. Tests
cover a crash with replacement, a child that always crashes, and four
compromised behaviours; the IPC decoder is in the `ipc-frame` fuzz
target.

The "crashes cancel work without killing chrome" gate is now met for the
network process as well as renderers (no GPU process exists). On Linux
the child is confined by default (bubblewrap keeping only the network
namespace, a seccomp filter that admits `AF_INET`/`AF_INET6` sockets
only; the escape probe reads a file, lists a directory, starts a shell
and connects to an abstract Unix socket, all denied). On Windows and
macOS it is not confined (TD-176), which joins the items that block
remote browsing there.
