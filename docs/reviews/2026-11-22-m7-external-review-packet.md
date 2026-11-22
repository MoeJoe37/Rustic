# M7 external security review packet (2026-11-22)

This packet is for the independent reviewers M7's last gate needs
([SECURITY.md](../SECURITY.md#general-remote-browsing-release-gates),
`security.audit`). It says what to review, where the code is, how to run
the checks, and what we want answers to. The internal review it builds on
is [2026-11-20-m7-security-review.md](2026-11-20-m7-security-review.md)
and its addenda.

Nobody outside the project has reviewed Rustic yet. The gate closes when a
reviewer who did not write the code signs off on the scope below (or the
findings are fixed and re-checked), and their report is added to
`docs/reviews/`.

## What Rustic is at M7

A from-scratch Rust browser engine whose development command line
(`rustic`) loads local files and granted `http(s)` origins in a
broker/renderer split:

- The **broker** (the `rustic` process) holds every capability: files in
  the document's directory, network grants, the profile, downloads, the
  trusted UI strip of `rustic view`.
- Each document runs in a **renderer** child (`rustic __renderer`) that
  receives nothing but its document and asks the broker for every
  resource over a bounded, versioned IPC protocol
  ([ADR 0056](../adr/0056-ipc-format.md)). Frames and popups get renderers
  of their own ([ADR 0062](../adr/0062-out-of-process-frames.md)).
- HTTP, TLS and content decoding run in a **network process**
  (`rustic __network`, [ADR 0061](../adr/0061-network-process.md)); the
  broker keeps every policy decision and treats its replies as hostile.
- Renderers and the network process are OS-confined by default: Linux
  bubblewrap plus a seccomp allowlist
  ([ADR 0058](../adr/0058-linux-renderer-sandbox.md)), Windows less
  privileged AppContainers in one-process jobs
  ([ADR 0060](../adr/0060-windows-renderer-sandbox.md),
  [ADR 0066](../adr/0066-remote-addresses-and-windows-network-container.md)),
  macOS Seatbelt profiles applied with `sandbox_init`
  ([ADR 0063](../adr/0063-macos-renderer-sandbox.md)). Android runs
  renderers in an isolated service
  ([ADR 0059](../adr/0059-mobile-renderer-isolation.md)); iOS is design
  only.
- Remote origins resolve through the system resolver in the broker and
  reach only public unicast addresses (ADR 0066).

## Scope, in priority order

1. **Broker authority.** `crates/rustic-cli/src/broker.rs` (`Broker`,
   `Grant`, `resource`, `open_frame`, `open_popup`, the watchdog) and
   `crates/rustic-cli/src/renderer.rs` (what a renderer may ask for).
   Question: can a compromised renderer obtain a file, origin, storage
   partition, cookie or frame grant it was not given?
2. **IPC decoding.** `crates/rustic-ipc` (framing, limits, every message
   decoder in `protocol.rs`). Question: can a hostile peer cause
   unbounded allocation, a panic in the broker, or a message to be read as
   another?
3. **Sandboxes.** `crates/rustic-sandbox` (seccomp filter generation,
   Seatbelt profiles, the Windows launcher), the launch code in
   `broker.rs` and `netproc.rs`, and the escape probes in
   `crates/rustic-cli/tests/isolation.rs`. Question: what can code
   execution in a renderer or the network process reach on each OS?
4. **Network policy.** `crates/rustic-net/src/policy.rs` (`is_public`,
   resolved routes), `fetch.rs`, `cors.rs`, `csp.rs`, `mixed_content.rs`,
   `isolation.rs` (site keys, COOP/COEP, ORB), cookies and storage
   partitioning (`rustic-storage`). Question: DNS rebinding, cross-site
   data leaks, CORS or ORB bypasses.
5. **Trusted UI.** `crates/rustic-cli/src/chrome.rs` and
   `rustic_shell::present` (address display, permission prompt and
   clipboard rules). Question: can a page spoof or overlay the address,
   or get a prompt answered without the user?
6. **Unsafe code** (the complete list): the Windows launcher
   (`rustic-sandbox/src/windows.rs`), the macOS `sandbox_init` call
   (`rustic-sandbox/src/seatbelt_enter.rs`), and the Android JNI entry
   points (`rustic-android/src/jni.rs`). Every block has a `SAFETY`
   comment. Dependencies with native or unsafe code are inventoried in the
   internal review ("Native and unsafe code").
7. **Parsers of untrusted input**: HTML, CSS, URL, encodings, JavaScript
   (parser, bytecode VM, RegExp), images (PNG, JPEG, GIF, WebP), fonts
   (through skrifa/harfrust), HTTP framing and content codings. All are
   fuzzed (`crates/rustic-fuzz`); reviewers may extend the corpora.

Out of scope for M7: the JavaScript JIT (none exists), GPU process (M8),
extensions, updates, the M8 browser shell's navigation and page input.

## How to build and run the checks

```sh
cargo build --release -p rustic-cli
# The isolation and network suites (Linux needs bubblewrap installed):
cargo test -p rustic-cli --test isolation --test network --test hosted
# Fuzz smoke campaign over every target (CI's settings):
cargo run --release -p rustic-fuzz -- list
cargo run --release -p rustic-fuzz -- run --all --seed 1 --iterations 20000 \
  --timeout-ms 10000 --artifacts fuzz-artifacts
# Load a remote page through the sandboxed renderer and network process:
./target/release/rustic dump-dom --allow-remote https://example.com/
```

Test hooks that make the renderer or network process misbehave are listed
in `SANDBOX_ENV` in `broker.rs` (`RUSTIC_TEST_COMPROMISED_RENDERER`,
`RUSTIC_TEST_NETWORK_PROCESS`, ...). They only change what the untrusted
process does; reviewers can use them to drive a compromised peer.

## Known limitations reviewers need not report

These are recorded in [TECH_DEBT.md](../TECH_DEBT.md) with a payoff
milestone:

- Windows runs with loopback fixture grants start the network process
  unconfined (no AppContainer can reach loopback; ADR 0066).
- `getaddrinfo` has no timeout of its own; DNS is not cached.
- Page APIs for notifications, geolocation and the async clipboard do not
  exist yet, so the prompt rules are reached only from tests (TD-173).
- Downloads are buffered and carry no quarantine metadata (TD-173).
- Frames are composited without the embedder seeing them, approximately
  under some blend modes (TD-177).

## What we ask the reviewers to return

- Findings with severity, a reproduction and the affected platforms.
- For each scope item above: reviewed, partially reviewed, or not
  reviewed.
- Whether the release gates in SECURITY.md are met in their judgement.

## Status

Packet ready 2026-11-22. Reviewers: not yet engaged (needs people outside
the project; the maintainer chooses them).
