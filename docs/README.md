# Rustic design and implementation guide

This directory holds Rustic's roadmap, design decisions and implementation
evidence. Use the [implementation ledger](IMPLEMENTATION.md) for tested
behavior, the [progress summary](progress/summary.md) for weighted delivery
status and the [roadmap](ROADMAP.md) for upcoming work. Update the relevant
guide with the code and add a decision record when a boundary changes.

## Where things stand

| Milestone | State | Read |
| --- | --- | --- |
| M0 Foundation | Complete for the declared scope. The aggregate `CI` job is the integration gate. Branch rules are a repository administration setting | [DEVELOPMENT.md](DEVELOPMENT.md) |
| M1 Document core | Done (28/28 capabilities): URL, HTML tokenizer and tree builder, DOM, local `dump-dom` | [URL.md](URL.md), [HTML.md](HTML.md), [HTML_TREE.md](HTML_TREE.md), [DOM.md](DOM.md) |
| M2 Styles | Done (8/8): CSS syntax, selectors, values, cascade with layers, media/container queries, invalidation | [CSS.md](CSS.md) |
| M3 Static rendering | Done (17/17): block/inline/flex layout, OpenType text, images, CPU raster, `render`, reftests, hit testing | [LAYOUT.md](LAYOUT.md), [TEXT.md](TEXT.md), [PAINT.md](PAINT.md) |
| M4 JavaScript | Complete for the declared scope. Includes the parser, collected heap, bytecode interpreter, modules, realms, Intl, limits and benchmark gate. M8 added a native ARM64 benchmark record | [JS.md](JS.md#limits-and-performance-gate-js-09), [ROADMAP §7](ROADMAP.md#7-m4--original-ecmascript-runtime) |
| M5 Scripted documents | Complete for the declared scope (11/11). Scripts, DOM bindings, events, mutation and resize observers, error reporting, focus, forms, the pinned WPT gate and the first cross-engine report. The emulated, MSRV and native tiers passed in the first full hosted run (2026-11-22) | [SCRIPTING.md](SCRIPTING.md), [review](reviews/2026-11-19-m5-completion.md) |
| M6 Networking | Done 2026-11-20: HTTP/1.1 transport with TLS, Fetch, CORS, cookies, CSP, cache, profiles and Web Storage over allowlisted loopback origins. Remote origins resolving to public addresses followed in M7 (ADR 0066, TD-157 paid) | [ROADMAP §9](ROADMAP.md#9-m6--controlled-networking-and-browser-services), [review](reviews/2026-11-20-m6-completion.md) |
| M7 Isolation | Done 2026-11-22 (7/7 after ADR 0067). Split renderer over bounded IPC, out-of-process iframes and popups, site process keys, COOP/COEP and ORB, renderer sandboxes on by default on Linux, Windows and macOS, a confined network process on all three with crash recovery, user activation, brokered downloads and the trusted address/permission strip. The independent external review is an external dependency of the M10 beta (`security.brave`) | [ROADMAP §10](ROADMAP.md#10-m7--isolation-and-security-gate), [review](reviews/2026-11-22-m7-completion.md) |
| M8 Desktop alpha | Complete for the declared scope. The native shell runs on Windows, macOS and Linux. The desktop alpha includes long-lived page renderers, confined GPU compositing with CPU fallback, accessibility bridges and automation | [ROADMAP §11](ROADMAP.md#11-m8--reference-desktop-alpha), [M8 completion review](reviews/2026-11-23-m8-completion.md) |
| M9 Compatibility expansion | Current. CSS-09, grid layout (in progress), live URL setters and the initial Window encoding surface are delivered. JavaScript additions, buffered Fetch bodies and a bounded WebAssembly profile are also available. See the roadmap for compatibility work still in progress | [ROADMAP §12](ROADMAP.md#12-m9--compatibility-expansion-program), [Window APIs](SCRIPTING.md#window-encoding-url-fetch-and-webassembly), [CSS-09](CSS.md#transitions-and-animations) |
| M10 Embedding and beta | Planned. Independent security review, hardware GPU timing and energy measurements, and screen-reader acceptance remain external gates | [ROADMAP §13](ROADMAP.md#13-m10--embedding-releases-and-maintenance) |
| M11–M13 | Feature parity with Blink/Gecko/WebKit, performance leadership and desktop 1.0 remain open. Early compatibility and performance evidence is tracked in the catalog | [ROADMAP §14–16](ROADMAP.md#14-m11--web-platform-parity), [PERFORMANCE.md](PERFORMANCE.md#cross-engine-gates) |
| M14 Mobile devices | Deferred to the end (ADR 0067): Android and iOS on real devices, iOS entitlement, mobile shipping and the final five-platform 1.0. Mobile compile checks and the Android emulator job stay in CI | [ROADMAP §17](ROADMAP.md#17-m14--mobile-devices-and-five-platform-10), [MOBILE.md](MOBILE.md) |

The [progress summary](progress/summary.md) carries the exact capability counts
and is regenerated from `progress/catalog.json`. The [platform matrix](PLATFORMS.md)
records hosted evidence and the available test tiers. Android and iOS device
acceptance is scheduled for M14. CI compiles their shared libraries and runs
an Android isolated-renderer emulator test.

## Reading order

New to the project? Read these in order:

1. [Architecture](ARCHITECTURE.md): components, ownership, execution and data flow.
2. [Roadmap](ROADMAP.md): milestones, task IDs, gates, the [current queue](ROADMAP.md#18-current-implementation-queue) and risks.
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
| [ROADMAP.md](ROADMAP.md) | Milestones M0–M14, tasks, gates, current queue and risk register |
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
| [ACCESSIBILITY.md](ACCESSIBILITY.md) | Accessibility tree (HTML-AAM, ARIA, AccName), platform bridges, assistive actions, editing, IME and the CI acceptance probes |
| [SCRIPTING.md](SCRIPTING.md) | Scripted documents: lifecycle, script/timer/input/rendering turns, Web IDL, DOM bindings, events, observers, forms and their remaining limits |
| WebAssembly (M9) | [Window surface and limits](SCRIPTING.md#window-encoding-url-fetch-and-webassembly), [core runtime and host binding evidence](IMPLEMENTATION.md#wasm-0102--module-decoding-and-core-validation), ADRs 0077 and 0079–0089 |
| Networking (M6) | No separate guide yet: the [NET sections of IMPLEMENTATION.md](IMPLEMENTATION.md#net-01--http11-transport-without-tls), [ROADMAP §9](ROADMAP.md#9-m6--controlled-networking-and-browser-services) and ADRs 0051–0055 and 0066 |
| Isolation (M7) | [SECURITY.md](SECURITY.md) for the model; the [ISO sections of IMPLEMENTATION.md](IMPLEMENTATION.md#iso-02--split-renderer-process-and-brokered-authority), [ROADMAP §10](ROADMAP.md#10-m7--isolation-and-security-gate) and ADRs 0056–0063 for what is built |

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
| Scripted Window | Pinned upstream `testharness.js` selection covering DOM, CSSOM, encoding, URL, Blob/Fetch and Wasm. [Exact Window baseline](SCRIPTING.md#pinned-window-web-platform-tests) |
| DOM | 25,230 pre-insert combinations against a specification oracle; seeded model tests |
| CSS | 8,041 color cases; 1,115 of 1,455 in-scope pinned WPT parsing cases with a reasoned ratchet |
| Layout and paint | Exact-geometry tests, checked reftests, 49 of 91 static css-flexbox, 103 of 168 static css-grid and 20 of 35 static css-tables WPT pages |
| Text | Shaping compared against 30 committed HarfBuzz reference cases |
| JavaScript | Pinned Test262: all 23,123 `test/language` files parse as required. Pass counts are 21,939 of 22,427 language files (97.8%), 17,739 of 18,861 gated built-ins files (94.1%), 1,240 of 1,326 gated `intl402` files (93.5%) and all 696 executed module files. Eight benchmark workloads have pinned checksums and a committed heap-size report |
| Scripted documents | Deterministic turn-order, timer, input and frame tests on a manual clock. Binding tests cover identity, reentrancy, failure atomicity and collection of detached DOM/JS cycles |
| Robustness | Deterministic fuzz targets (`cargo run --locked -p rustic-fuzz -- list`), a required smoke job and a scheduled campaign over a selected matrix |
| Platforms | Native tests on x64/ARM64 Linux, Windows and macOS; emulated 32-bit ARM/x86, RISC-V and big-endian; cross-checks for Android, iOS and eight more targets |

WPT and Test262 results apply to their declared selections. The independent
security review and release gates remain open. See the [reference-device
reports](PERFORMANCE.md#reference-devices) for measured workloads and
[DEPENDENCIES.md](DEPENDENCIES.md) for the reviewed dependency boundaries.
Check the [Actions page](https://github.com/MoeJoe37/Rustic/actions) for the
result at a specific commit.

## Document conventions

“Must” states a design invariant or milestone gate. “Candidate” means a choice
still needs a focused evaluation. M0–M14 are defined in the roadmap. Task IDs
(DOC-, CSS-, REN-, JS-, WEB-, NET-, ISO-, UI-, MOB-, WASM-, EXT-, BLK-, PAR-,
MED-, GFX-, DEV-, PERF-) are
stable; new work gets a new ID rather than reusing one. Future file paths in
prose are proposed; linked paths exist now.

Design baseline: 2026-11-07. Standards are living documents: each implementation
task records the exact section, upstream revision, and tests it used. Project
decisions are not claims that the standards prescribe this Rust architecture.
