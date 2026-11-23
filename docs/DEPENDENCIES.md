# Dependencies, platforms, and repository boundaries

## What “from scratch” means

Rustic owns the engine's algorithms and integration: HTML tokenization/tree
building, DOM, CSS syntax/cascade, layout and paint semantics, JavaScript runtime,
web API bindings, task scheduling, Fetch/browser policy and navigation. The
reference CPU raster backend interprets Rustic's display list. Existing browser
engines and existing JS runtimes are not implementation dependencies.
This explicitly excludes Mozilla/Firefox/Gecko, SpiderMonkey, Servo, Chromium/Blink,
WebKit, V8, and JavaScriptCore as engine/runtime implementations on every platform.

Reusing reviewed low-level libraries is permitted and expected. Do not implement
cryptographic primitives, certificate verification, operating system drivers or
general-purpose image/video codecs merely to claim zero dependencies. Unicode
data/IDNA, font parsing/shaping, TLS, HTTP framing, storage, serialization,
windowing and GPU access may use libraries behind explicit adapter boundaries.
This distinction is the project's chosen scope, recorded in
[ADR 0001](adr/0001-original-engine.md).

## Candidate evaluation map

**Adopted so far:** the font adapters HarfRust, Skrifa and Wuff
([ADR 0014](adr/0014-reviewed-font-adapters.md), see "M3 font adoption" below)
the TLS stack rustls with `ring` and `rustls-platform-verifier`
([ADR 0051](adr/0051-transport-and-tls.md)) with `brotli-decompressor` for
content decoding, `windows-sys` for the Windows sandbox
([ADR 0060](adr/0060-windows-renderer-sandbox.md)), and the desktop host winit
and softbuffer ([ADR 0065](adr/0065-desktop-window-host.md)), and wgpu for the
GPU process ([ADR 0071](adr/0071-gpu-backend.md)). The pinned graph
is [third_party/rust-dependencies.json](../third_party/rust-dependencies.json).
Every other name in this table is a candidate to evaluate at the relevant
milestone, not an installed package, version recommendation or compatibility
guarantee. The JavaScript engine (and its RegExp engine), Wasm runtime, HTML,
CSS, layout, paint, codecs and URL code stay original Rust (ADR 0001).

| Service | Candidate approach | Rustic retains responsibility for |
| --- | --- | --- |
| TLS | **Adopted (M6):** rustls 0.23.46 with the `ring` provider and `rustls-platform-verifier` 0.7.1 in `rustic-net::tls` ([ADR 0051](adr/0051-transport-and-tls.md); graph, MSRV and licenses there) | Certificate error behavior, HTTPS policy and broker authority |
| HTTP transport | **Chosen at M6 entry:** Rustic's own HTTP/1.1 in `rustic-net`, no runtime ([ADR 0051](adr/0051-transport-and-tls.md), [ADR 0052](adr/0052-network-io-model.md)); a reviewed library remains a candidate for HTTP/2 | Framing and limits, Fetch, redirects, CORS, cookies, cache, MIME and response visibility |
| Text encodings | **Chosen at M6 entry:** original Encoding Standard decoders and encoders over indexes generated from pinned whatwg/encoding data (`rustic-encoding`, [ADR 0043](adr/0043-encoding-standard.md)), no library | Charset selection, sniffing and restart, URL and form encoding |
| Unicode/IDNA | Chosen at M1: original UTS #46/NFC code over tables generated from pinned Unicode data (`tools/generate_idna_data.py`), no library | URL/origin algorithms, data updates and string/index semantics |
| Font parsing/shaping | **Adopted (M3):** HarfRust 0.16.0, Skrifa 0.50.0, Wuff 0.2.10; discovery is Rustic's own host-granted walk | Font selection, CSS metrics, bidi/line breaking integration, layout |
| Image codecs | **Chosen at M3:** original bounded inflate/PNG/JPEG/GIF/WebP decoders in `rustic-codecs`, no library; video/audio codecs remain candidates | Input/output limits, resource accounting, sandbox isolation and color handling |
| Raster/GPU | Original CPU display-list interpreter; **adopted (M8):** wgpu 26.0.1 (Vulkan, Metal, Direct3D 12 only) in `rustic-gpu`, inside the GPU process ([ADR 0071](adr/0071-gpu-backend.md); 26 is the newest major for MSRV 1.85) | Paint semantics, compositing arithmetic (byte-exact shader), scene correctness, device loss and CPU fallback |
| Desktop host | **Adopted (M8):** winit 0.30.13 and softbuffer 0.4.8 in `rustic-shell`, minimal features, Wayland/X11 loaded at run time ([ADR 0065](adr/0065-desktop-window-host.md); graph and licenses there) | Chrome, input routing, lifecycle and permission decisions; page rendering stays in renderer processes |
| Persistence | **M6:** Rustic's own versioned, checksummed atomic files in `rustic-storage` for Web Storage; a reviewed embedded transactional store or SQLite adapter remains a candidate for IndexedDB and larger stores | Partition keys, quotas, schemas, migration and private mode |
| Serialization | **Chosen at M7:** Rustic's own bounded binary IPC format in `rustic-ipc`, no library ([ADR 0056](adr/0056-ipc-format.md)) | IPC schema, validation, capabilities, bounds and versioning |
| Diagnostics/testing | Lightweight tracing, property testing, fuzzing/benchmark tooling as needed | Redaction, reproducibility, budgets and meaningful assertions |

Before adopting any candidate, check its current documentation, maintenance,
security history, supported Rust/platform versions, license, transitive/native
dependencies, and default features. Record measured alternatives in an ADR when
the choice creates a major architectural dependency. Do not add every candidate
preemptively or let an adapter choose web semantics on Rustic's behalf.

## Dependency change checklist

- State the concrete needed capability and why std/existing code is insufficient.
- Minimize enabled features; inspect build scripts, native code, unsafe/FFI and transitive graph.
- Confirm license compatibility with the existing repository license and preserve notices.
- Update and commit `Cargo.lock`; build/test using `--locked` in CI.
- Document network/runtime authority, resource caps and failure/shutdown behavior.
- Pass `cargo deny --locked check advisories licenses bans sources` and update the reviewed manifest in `third_party/rust-dependencies.json`; triage advisories rather than blindly allowlisting them.
- Pin imported corpus/font/data revisions and their redistributable licenses.

Dependabot opens weekly update PRs for Cargo and pinned GitHub Actions. It does
not prove dependencies are safe and it does not auto-merge changes. `cargo-deny`
runs in the Dependency audit workflow; SBOM creation and release signing are
planned for M10. Major/authority-affecting upgrades need focused review.

## Platform rollout

The platform matrix, CI tiers and portability rules live in
[PLATFORMS.md](PLATFORMS.md); mobile host milestones live in [MOBILE.md](MOBILE.md).
A dependency must build for every tier there (including 32-bit, big-endian,
Android and iOS) before it is adopted.

## Source layout and data

Keep small fixtures and reduced regressions next to their owning crate. Future
shared test fixtures live under `tests/fixtures`, and pinned suite manifests under
`tests/conformance`; create these when used. Large upstream checkouts belong under
ignored `third_party/checkouts`, with fetching/pinning tooling and notices checked
in separately. Generated artifacts go under ignored `artifacts` or `target`.
Generated source must identify its input revision and reproducible generator;
test whether checked-in generated output is fresh once generators exist.

`rustic-net` also uses `brotli-decompressor` 6.3.1 (already reviewed in the
font graph for WOFF2) without default features for `Content-Encoding: br`,
with its own safe allocator cells so the crate's `std`/`unsafe` allocator
stays off.

`rustic-net` vendors the Public Suffix List (MPL-2.0 data, not code) at a
pinned revision with `tools/import_public_suffix_list.py`, which checks
SHA-256 digests; `tools/check_repository.py` checks the manifest and the
license notice ([ADR 0054](adr/0054-sites-and-cookie-policy.md)).

The bindings crate vendors selected WHATWG DOM Web IDL declarations, a
revision and checksum manifest, and the upstream license. The offline
generator verifies those inputs and reproduces the committed Rust metadata.
[IDL manifest](../crates/rustic-bindings/idl/dom_manifest.json),
[WHATWG DOM license](../crates/rustic-bindings/idl/WHATWG-DOM-LICENSE.txt)

Current imported test data includes a pinned BSD-licensed WPT URL subset and an
MIT-licensed html5lib declaration-tokenizer subset. Their per-crate fixture
directories retain notices, source revisions/digests, selection manifests and
reproduction tooling. These are offline test expectations, not runtime code.

The HTML runtime now uses a generated table of WHATWG named-reference data.
Its vendored JSON source, source/table digests, snapshot date, reproduction tool
and WHATWG source-code license notice are retained under `rustic-html` data/source.
This is standards data behind an original parser/lookup algorithm. No external
Rust library or existing parser was introduced. See [HTML data provenance](HTML.md).

The initial document builder also uses the WHATWG compatibility-identifier data
and a pinned MIT-licensed html5lib tree-construction file. The complete upstream
file, source/fixture digests, closed selection policy and offline generator are
retained; [tree evidence](HTML_TREE.md) records its earlier revision and unrun
scope. `rustic-html` depends on the original `rustic-dom` crate.

The AGPL license already present in the repository is retained. Workspace package
metadata uses `AGPL-3.0-only`; changing the licensing policy is a maintainer
decision, not an incidental dependency or scaffolding change.

## M3 font adoption (2026-11-10)

[ADR 0014](adr/0014-reviewed-font-adapters.md) adopts exact HarfRust 0.15.0, Skrifa
0.49.0 and Wuff 0.2.10 for the remaining low-level font functions. Default features
are disabled; only `std`, `std`, and safe `brotli` are enabled respectively.
The 24-package graph, checksums, source revisions, license expressions and
retained notices live in [the reviewed manifest](../third_party/rust-dependencies.json).
All dependencies compile with Rust 1.85. No native runtime/FFI library, font
network client, browser engine or JS VM is introduced. Build-time derives are
reviewed as part of the graph. Auditing is configured with `cargo-deny 0.20.2`;
run `cargo deny --locked check advisories licenses bans sources` before merging.
Font discovery is a separate host-granted local I/O API; the shaping libraries
receive immutable bytes. TD-85/86/87 record the limits that remain.

### Fontations/HarfRust update (2026-11-14)

Skrifa 0.50.0 brings `read-fonts` 0.47.0. HarfRust 0.16.0 uses the same
`read-fonts` version, so the lockfile retains one parser version and the
24-package graph requires no duplicate-version exception. The existing
`std` features, Rust 1.85 minimum and licenses still apply. The published
crates have no build scripts and the Type 1 feature stays disabled.
The revised registry checksums, source revisions and notices are in the
[reviewed manifest](../third_party/rust-dependencies.json).

Fontations' update includes bounds and malformed-data fixes in CFF, VARC
and other OpenType table readers. HarfRust's release adds shaping work
limits, including lookup and syllable processing. Rustic continues using
the same outline and shaping adapters and tests the resulting glyphs,
advances and offsets against its committed reference fixtures.
