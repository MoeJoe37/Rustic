# Rustic design and implementation guide

This directory holds Rustic's plan, its design, and the record of what is
actually built. Design documents propose; only the implementation documents and
the [progress summary](progress/summary.md) describe working code, and every
status there is backed by tests. Update the relevant document (and a decision
record when a boundary changes) in the same change as the code.

## Where things stand (2026-11-22)

| Milestone | State | Read |
| --- | --- | --- |
| M0 Foundation | Done; hosted CI runs again on `moejoe873/Rustic` (2026-11-22); requiring the `CI` check on `main` is an open admin step | [DEVELOPMENT.md](DEVELOPMENT.md) |
| M1 Document core | Done (28/28 capabilities): URL, HTML tokenizer and tree builder, DOM, local `dump-dom` | [URL.md](URL.md), [HTML.md](HTML.md), [HTML_TREE.md](HTML_TREE.md), [DOM.md](DOM.md) |
| M2 Styles | Done (8/8): CSS syntax, selectors, values, cascade with layers, media/container queries, invalidation | [CSS.md](CSS.md) |
| M3 Static rendering | Done (17/17): block/inline/flex layout, OpenType text, images, CPU raster, `render`, reftests, hit testing | [LAYOUT.md](LAYOUT.md), [TEXT.md](TEXT.md), [PAINT.md](PAINT.md) |
| M4 JavaScript | Done (13/13) on 2026-11-16: parser, collected heap, bytecode interpreter, standard library, modules, realms, Intl, limits and benchmark gate. The ARM64 benchmark record is from emulation (TD-125) | [JS.md](JS.md), [ROADMAP §7](ROADMAP.md#7-m4--original-ecmascript-runtime) |
| M5 Scripted documents | Complete for the declared scope (11/11). Scripts, DOM bindings, events, mutation and resize observers, error reporting, focus, forms, the pinned WPT gate and the first cross-engine report. The emulated, MSRV and native tiers passed in the first full hosted run (2026-11-22) | [SCRIPTING.md](SCRIPTING.md), [review](reviews/2026-11-19-m5-completion.md) |
| M6 Networking | Done 2026-11-20: HTTP/1.1 transport with TLS, Fetch, CORS, cookies, CSP, cache, profiles and Web Storage over allowlisted loopback origins. Remote origins resolving to public addresses followed in M7 (ADR 0066, TD-157 paid) | [ROADMAP §9](ROADMAP.md#9-m6--controlled-networking-and-browser-services), [review](reviews/2026-11-20-m6-completion.md) |
| M7 Isolation | Current (5/8). Split renderer over bounded IPC, out-of-process iframes and popups, site process keys, COOP/COEP and ORB, renderer sandboxes on by default on Linux, Windows and macOS, a confined network process on all three with crash recovery, user activation, brokered downloads and the trusted address/permission strip. Open: Android/iOS isolation (design only, ADR 0059), the remaining recovery and audit steps, and an independent external review | [ROADMAP §10](ROADMAP.md#10-m7--isolation-and-security-gate), [security review](reviews/2026-11-20-m7-security-review.md) |
| M8 Desktop and mobile alpha | Started early (1/21): compositor scenes and scrolling (UI-05, ADR 0064) done; `rustic view` desktop window on Linux (UI-02, ADR 0065) | [ROADMAP §11](ROADMAP.md#11-m8--reference-desktop-and-mobile-alpha), [IMPLEMENTATION.md](IMPLEMENTATION.md#ui-02--desktop-window) |
| M9–M10 | Not started: compatibility, beta | [ROADMAP.md](ROADMAP.md) |
| M11–M13 | Not started: feature parity with Blink/Gecko/WebKit, performance leadership over all three, full engine 1.0 | [ROADMAP §14–16](ROADMAP.md#14-m11--web-platform-parity), [PERFORMANCE.md](PERFORMANCE.md#cross-engine-gates) |

The [progress summary](progress/summary.md) is regenerated from
`progress/catalog.json` and is the measured status. Done means done for the
milestone's declared scope; broad web compatibility is M9 work and native
Android/iOS host acceptance is M8 work. The capability counts in this table
come from that summary; when they disagree, the summary is right and this
table needs an update.

Two things are true of every milestone so far and are easy to miss:

- **Desktop evidence is back; mobile has none.** Hosted CI was blocked by a
  billing limit from 2026-11-08 to 2026-11-22, when the repository moved to
  `moejoe873/Rustic`. The first full run there (native x64 and ARM64 Linux,
  Windows and macOS, the emulated tiers and every compile check) found
  portability bugs in the work merged during the gap, which are fixed, and
  passed on `feature` at `9961c1c` ([TD-12](TECH_DEBT.md),
  [PLATFORMS.md](PLATFORMS.md#current-verification-status-2026-11-22)).
- **Nothing runs on a phone yet.** Android and iOS are compile-checked only;
  the first on-device surfaces are MOB-03/04, the first task of M8 mobile
  hosts ([ROADMAP §11](ROADMAP.md#11-m8--reference-desktop-and-mobile-alpha)),
  and mobile renderer isolation is a design only
  ([ADR 0059](adr/0059-mobile-renderer-isolation.md)).

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
| [SCRIPTING.md](SCRIPTING.md) | Scripted documents: lifecycle, script/timer/input/rendering turns, Web IDL, DOM bindings, events, observers, forms and their remaining limits |
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
| Scripted Window | WPT revision `cb97c571c3b1947e08b8615eeb4028a2794b23a4` through upstream `testharness.js`. The 59 files report 272 passes and 70 expected failures across 342 subtests. [Exact baseline](SCRIPTING.md#pinned-window-web-platform-tests) |
| DOM | 25,230 pre-insert combinations against a specification oracle; seeded model tests |
| CSS | 8,041 color cases; 1,115 of 1,455 in-scope pinned WPT parsing cases with a reasoned ratchet |
| Layout and paint | Exact-geometry tests, checked reftests, 38 of 91 static css-flexbox WPT pages |
| Text | Shaping compared against 30 committed HarfBuzz reference cases |
| JavaScript | Pinned Test262: all 23,123 `test/language` files parse as required; 21,939 of the 22,427 that run pass (97.8%); 17,545 of 18,791 in the gated `test/built-ins` areas pass (93.4%); 1,240 of 1,326 in the gated `test/intl402` areas pass (93.5%); all 696 executed module files pass. Eight benchmark workloads with pinned checksums and a committed heap-size report |
| Scripted documents | Deterministic turn-order, timer, input and frame tests on a manual clock; binding tests for identity, reentrancy, failure atomicity and collection of detached DOM/JS cycles. No WPT `dom/` or `html/` results yet (`web.conformance`) |
| Robustness | 29 deterministic fuzz targets (`cargo run -p rustic-fuzz -- list`) with a required smoke job and a nightly campaign |
| Platforms | Native tests on x64/ARM64 Linux, Windows and macOS; emulated 32-bit ARM/x86, RISC-V and big-endian; cross-checks for Android, iOS and eight more targets |

There are no whole-WPT pass rates, no Test262 rate over the whole suite
(each gate covers the implemented areas, and proposal features are skipped),
no externally reviewed sandbox, no performance comparison beyond the
[first JavaScript report](reviews/2026-11-19-perf-01-m5.md) and no releases
yet. Third-party Rust crates are limited to the reviewed and pinned ones in
[DEPENDENCIES.md](DEPENDENCIES.md): the font adapters
([ADR 0014](adr/0014-reviewed-font-adapters.md)), TLS and content decoding
([ADR 0051](adr/0051-transport-and-tls.md)) and the desktop window
([ADR 0065](adr/0065-desktop-window-host.md)).
Configuration in the repository is distinct from a successful hosted run;
inspect the [Actions page](https://github.com/moejoe873/Rustic/actions) for run
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
