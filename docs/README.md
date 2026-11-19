# Rustic design and implementation guide

This directory holds Rustic's plan, its design, and the record of what is
actually built. Design documents propose; only the implementation documents and
the [progress summary](progress/summary.md) describe working code, and every
status there is backed by tests. Update the relevant document (and a decision
record when a boundary changes) in the same change as the code.

## Where things stand (2026-11-18)

| Milestone | State | Read |
| --- | --- | --- |
| M0 Foundation | Done; requiring the `CI` check on `main` is an open admin step | [DEVELOPMENT.md](DEVELOPMENT.md) |
| M1 Document core | Done (28/28 capabilities): URL, HTML tokenizer and tree builder, DOM, local `dump-dom` | [URL.md](URL.md), [HTML.md](HTML.md), [HTML_TREE.md](HTML_TREE.md), [DOM.md](DOM.md) |
| M2 Styles | Done (8/8): CSS syntax, selectors, values, cascade with layers, media/container queries, invalidation | [CSS.md](CSS.md) |
| M3 Static rendering | Done (17/17): block/inline/flex layout, OpenType text, images, CPU raster, `render`, reftests, hit testing | [LAYOUT.md](LAYOUT.md), [TEXT.md](TEXT.md), [PAINT.md](PAINT.md) |
| M4 JavaScript | Done (13/13) on 2026-11-16: parser, collected heap, bytecode interpreter, standard library, modules, realms, Intl, limits and benchmark gate. The ARM64 benchmark record is from emulation (TD-125) | [JS.md](JS.md), [ROADMAP §7](ROADMAP.md#7-m4--original-ecmascript-runtime) |
| M5 Scripted documents | Current (7/12). The local script scheduler, Web IDL conversions, DOM bindings, events, script readiness and local encoding replay are in place. Remaining work includes resolved style and resource ordering, forms, focus, observers, WPT expansion and the first benchmark report. | [SCRIPTING.md](SCRIPTING.md), [ROADMAP §8](ROADMAP.md#8-m5--scripted-document-integration) |
| M6 Networking | Entry work started alongside M5. All 40 Encoding Standard encodings and stylesheet charset selection are in place (ADR 0043, TD-28 paid). Transport encoding confidence, URL query encoding and the transport/TLS ADR remain open. | [ENCODING.md](ENCODING.md), [ROADMAP §9](ROADMAP.md#9-m6--controlled-networking-and-browser-services) |
| M7–M10 | Not started: isolation, desktop/mobile alpha, compatibility, beta. M7 has an entry checklist | [ROADMAP.md](ROADMAP.md) |
| M11–M13 | Not started: feature parity with Blink/Gecko/WebKit, performance leadership over all three, full engine 1.0 | [ROADMAP §14–16](ROADMAP.md#14-m11--web-platform-parity), [PERFORMANCE.md](PERFORMANCE.md#cross-engine-gates) |

The [progress summary](progress/summary.md) is regenerated from
`progress/catalog.json` and is the measured status. Done means done for the
milestone's declared scope; broad web compatibility is M9 work and native
Android/iOS host acceptance is M8 work. The capability counts in this table
come from that summary; when they disagree, the summary is right and this
table needs an update.

Two things are true of every milestone so far and are easy to miss:

- **Platform evidence is uneven.** Hosted CI has been blocked by a billing
  limit since 2026-11-08, so recent merges are verified in a Linux sandbox:
  native x86-64, emulated 32-bit ARM, 32-bit x86, RISC-V and big-endian, and
  compile checks for Linux aarch64, Windows, Android and iOS. Native Windows
  (x64 and ARM64), macOS (x64 and ARM64) and Linux aarch64 test runs are
  waiting for CI to come back ([TD-12](TECH_DEBT.md), [PLATFORMS.md](PLATFORMS.md)).
- **Nothing runs on a phone yet.** Android and iOS are compile-checked only;
  the first on-device surface is MOB-03/04, scheduled in
  [ROADMAP §8](ROADMAP.md#8-m5--scripted-document-integration).

## Reading order

New to the project? Read these in order:

1. [Architecture](ARCHITECTURE.md): components, ownership, execution and data flow.
2. [Roadmap](ROADMAP.md): milestones, task IDs, gates, the [current queue](ROADMAP.md#17-current-implementation-queue) and risks.
3. [Implementation progress](IMPLEMENTATION.md): every finished task and its tested contract.
4. [Technical debt](TECH_DEBT.md): known shortcuts (TD-NN) and the rules that keep them safe.
5. [Platform support](PLATFORMS.md): the any-device rule and the CI target matrix.
6. [Development and CI](DEVELOPMENT.md): commands, CI jobs and the merge workflow.

## Document map

**Plan and scope**

| Document | Purpose |
| --- | --- |
| [PRODUCT.md](PRODUCT.md) | Full-product requirements: Brave-level privacy, parity with and performance leadership over Blink/Gecko/WebKit, GPU, Wasm |
| [EXTENSIONS.md](EXTENSIONS.md) | Rustic-only Manifest V2 extensions and the uBlock Origin port |
| [ROADMAP.md](ROADMAP.md) | Milestones M0–M13, tasks, gates, current queue and risk register |
| [PROGRESS.md](PROGRESS.md) | How the README progress map is scored and updated |
| [STANDARDS.md](STANDARDS.md) | Which standards are targeted and how conformance is reported |

**Cross-cutting design**

| Document | Purpose |
| --- | --- |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Crates, ownership, execution model, IPC and embedding |
| [SECURITY.md](SECURITY.md) | Trust boundaries, origin policy, isolation and browsing gates |
| [PERFORMANCE.md](PERFORMANCE.md) | Benchmarks, budgets, the JavaScript memory/speed plan and optimization order |
| [TESTING.md](TESTING.md) | Unit, conformance, rendering, fuzzing and regression policy |
| [PLATFORMS.md](PLATFORMS.md) | Portability rules and CI tiers for every CPU and OS |
| [MOBILE.md](MOBILE.md) | Android and iOS architecture, MOB-01–10 |
| [DEPENDENCIES.md](DEPENDENCIES.md) | What "from scratch" means, the dependency policy and the reviewed crates |
| [DEVELOPMENT.md](DEVELOPMENT.md) | Toolchain, local checks, CI jobs and failure diagnosis |
| [adr/](adr/README.md) | Architecture decision records, grouped by area, and the decisions still to be written |

**Implemented subsystems** (each states its scope, limits and evidence)

| Document | Subsystem |
| --- | --- |
| [URL.md](URL.md) | URL parsing, origins, file URLs, IDNA |
| [ENCODING.md](ENCODING.md) | Encoding Standard decoders and encoders, stylesheet charset selection |
| [HTML.md](HTML.md) | Byte decoding and the HTML tokenizer |
| [HTML_TREE.md](HTML_TREE.md) | Tree construction, fragments and `dump-dom` |
| [DOM.md](DOM.md) | Node storage, mutation, cloning/adoption and the tracing prototype |
| [CSS.md](CSS.md) | CSS syntax, selectors, values, cascade, queries, invalidation and computed style |
| [LAYOUT.md](LAYOUT.md) | Box tree, block/inline/flex layout, floats and positioning |
| [TEXT.md](TEXT.md) | Unicode data, fonts, shaping, bidi and line breaking |
| [PAINT.md](PAINT.md) | Display lists, CPU raster, image decoding and hit testing |
| [JS.md](JS.md) | The JavaScript engine: parser, heap and collector, bytecode interpreter, built-ins, modules, realms, Intl, Test262 gates and benchmarks |
| [SCRIPTING.md](SCRIPTING.md) | Scripted documents: lifecycle, script/timer/input/rendering turns, Web IDL, DOM bindings, events and the open M5 work |

**History:** [reviews/](reviews/) holds dated milestone and checkpoint reviews.
[IMPLEMENTATION.md](IMPLEMENTATION.md) and [PROGRESS.md](PROGRESS.md) keep
their sections in the order the work landed, so a later section can supersede
an earlier one's "remaining" notes.

## Evidence at a glance

| Area | Evidence |
| --- | --- |
| URL | 887 pinned WPT URL records; all 6,389 Unicode IdnaTestV2 cases |
| Encodings | All 40 Encoding Standard encodings from pinned indexes; 44.5 million decoder and encoder cases compared with Chromium 157, differing only where Chromium departs from the standard (242 cases, [documented](ENCODING.md#reference-comparison)) |
| HTML | 1,079 declaration, 4,626 character-reference and 282 text-mode tokenizer cases, plus all 1,792 html5lib tree records at every two-chunk split with four expectations adapted to current HTML |
| Scripted Window | WPT revision `cb97c571c3b1947e08b8615eeb4028a2794b23a4` through upstream `testharness.js`. The 23 files report 72 passes and 11 expected failures across 83 subtests. [Exact baseline](SCRIPTING.md#pinned-window-web-platform-tests) |
| DOM | 25,230 pre-insert combinations against a specification oracle; seeded model tests |
| CSS | 8,041 color cases; 1,115 of 1,455 in-scope pinned WPT parsing cases with a reasoned ratchet |
| Layout and paint | Exact-geometry tests, checked reftests, 38 of 91 static css-flexbox WPT pages |
| Text | Shaping compared against 30 committed HarfBuzz reference cases |
| JavaScript | Pinned Test262: all 23,123 `test/language` files parse as required; 21,939 of the 22,427 that run pass (97.8%); 17,545 of 18,791 in the gated `test/built-ins` areas pass (93.4%); 1,240 of 1,326 in the gated `test/intl402` areas pass (93.5%); all 696 executed module files pass. Eight benchmark workloads with pinned checksums and a committed heap-size report |
| Scripted documents | Deterministic turn-order, timer, input and frame tests on a manual clock; binding tests for identity, reentrancy, failure atomicity and collection of detached DOM/JS cycles. No WPT `dom/` or `html/` results yet (`web.conformance`) |
| Robustness | 24 deterministic fuzz targets (`cargo run -p rustic-fuzz -- list`) with a required smoke job and a nightly campaign |
| Platforms | Native tests on x64/ARM64 Linux, Windows and macOS; emulated 32-bit ARM/x86, RISC-V and big-endian; cross-checks for Android, iOS and eight more targets |

There are no whole-WPT pass rates, no Test262 rate over the whole suite
(each gate covers the implemented areas, and proposal features are skipped),
no sandbox guarantees, no comparison with another engine's performance and no
releases yet. The only third-party Rust
crates are the reviewed font adapters of [ADR 0014](adr/0014-reviewed-font-adapters.md).
Configuration in the repository is distinct from a successful hosted run;
inspect the [Actions page](https://github.com/MoeJoe37/Rustic/actions) for run
results.

## Document conventions

“Must” states a design invariant or milestone gate. “Candidate” means a choice
still needs a focused evaluation. M0–M13 are defined in the roadmap. Task IDs
(DOC-, CSS-, REN-, JS-, WEB-, NET-, ISO-, UI-, MOB-, WASM-, EXT-, BLK-, PAR-,
MED-, GFX-, DEV-, PERF-) are
stable; new work gets a new ID rather than reusing one. Future file paths in
prose are proposed; linked paths exist now.

Design baseline: 2026-11-07. Standards are living documents: each implementation
task records the exact section, upstream revision, and tests it used. Project
decisions are not claims that the standards prescribe this Rust architecture.
