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

There are **no external Rust dependencies at M0**. Names below are candidates to
evaluate at the relevant milestone, not adopted or installed packages, version
recommendations, or compatibility guarantees.

| Service | Candidate approach | Rustic retains responsibility for |
| --- | --- | --- |
| TLS | Maintained Rust TLS library such as rustls with reviewed crypto/provider/root strategy | Certificate error behavior, HTTPS policy and broker authority |
| HTTP transport | Reviewed HTTP library such as hyper, with a bounded async runtime if needed | Fetch, redirects, CORS, cookies, cache, MIME and response visibility |
| Unicode/IDNA | Maintained Unicode/IDNA data library; pin data versions | URL/origin algorithms and string/index semantics |
| Font parsing/shaping | Reviewed Rust font parser and shaping library; OS font discovery adapter | Font selection, CSS metrics, bidi/line breaking integration, layout |
| Image codecs | Maintained bounded decoder libraries | Input/output limits, resource accounting, sandbox isolation and color handling |
| Raster/GPU | Original CPU display-list interpreter; GPU adapter such as wgpu later | Paint semantics, scene correctness, device loss and CPU fallback |
| Desktop host | Window/event library such as winit with platform adapters | Chrome, input routing, lifecycle and permission decisions |
| Persistence | Reviewed embedded transactional store or SQLite adapter | Partition keys, quotas, schemas, migration and private mode |
| Serialization | Reviewed format/library after protocol prototype | IPC schema, validation, capabilities, bounds and versioning |
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
- Add vulnerability/license checking when external dependencies arrive; triage advisories rather than blindly allowlisting them.
- Pin imported corpus/font/data revisions and their redistributable licenses.

Dependabot opens weekly update PRs for Cargo and pinned GitHub Actions. It does
not prove dependencies are safe and it does not auto-merge changes. Future
auditing tools, SBOM creation and release signing are planned, not configured in
the first commit. Major/authority-affecting upgrades need focused review.

## Platform rollout

| Platform | M0 automation | Runtime rollout target | Required gate |
| --- | --- | --- | --- |
| Linux x86-64 | Ubuntu 24.04 build/tests; quality and docs checks | Reference headless renderer, then desktop host | Linux broker/sandbox and runtime integration evidence |
| Windows x86-64 | Windows Server 2022 build/tests | Headless first, desktop/input/accessibility next | Windows isolation and platform flow evidence |
| macOS runner architecture | macOS 15 native runner build/tests | Headless first; desktop feasibility by platform ADR | Actual macOS sandbox and bridge evidence |
| Additional CPU architectures | Not configured | Cross-compile then native testing on demand | Native runtime tests before support claims |
| Android ARM64 and x86-64 emulator | Shared library cross-checks for `aarch64-linux-android` and `x86_64-linux-android` | Native Android shell, surfaces, input and lifecycle adapters | NDK link/package, emulator and real-device rendering/security tests |
| iOS/iPadOS ARM64 and ARM64 simulator | Shared library cross-checks for `aarch64-apple-ios` and `aarch64-apple-ios-sim` on macOS | Native UIKit/engine host and platform-constrained process adapters | Xcode link/sign/package, simulator/device tests and applicable distribution entitlements |
| Other mobile platforms / older CPU architectures | Not configured | Evaluate separately after the initial five-platform targets | Explicit scope and platform support decision |

Hosted macOS runner architecture may differ by availability; inspect the actual
runner image instead of claiming both Intel and Apple Silicon are tested.
Cross-platform CI currently exercises std-only code, not fonts, GPU, sandbox,
network, desktop/mobile UI, or end-to-end browser behavior. The four mobile jobs
run `cargo check --workspace --lib --target ...`: they compile the shared library
code without linking an application, running target tests, or proving device
support. The desktop CLI and subprocess tests are not mobile app entry points.
See [mobile architecture and gates](MOBILE.md) for the rest of the required work.

## Source layout and data

Keep small fixtures and reduced regressions next to their owning crate. Future
shared test fixtures live under `tests/fixtures`, and pinned suite manifests under
`tests/conformance`; create these when used. Large upstream checkouts belong under
ignored `third_party/checkouts`, with fetching/pinning tooling and notices checked
in separately. Generated artifacts go under ignored `artifacts` or `target`.
Generated source must identify its input revision and reproducible generator;
test whether checked-in generated output is fresh once generators exist.

Current imported test data includes a pinned BSD-licensed WPT URL subset and an
MIT-licensed html5lib declaration-tokenizer subset. Their per-crate fixture
directories retain notices, source revisions/digests, selection manifests and
reproduction tooling. These are offline test expectations; runtime crates still
have no third-party Rust dependencies or parser/engine implementations.

The AGPL license already present in the repository is retained. Workspace package
metadata uses `AGPL-3.0-only`; changing the licensing policy is a maintainer
decision, not an incidental dependency or scaffolding change.
