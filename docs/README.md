# Rustic design and implementation guide

This directory holds Rustic's plan, its design, and the record of what is
actually built. Design documents propose; only the implementation documents and
the [progress summary](progress/summary.md) describe working code, and every
status there is backed by tests. Update the relevant document (and a decision
record when a boundary changes) in the same change as the code.

## Where things stand (2026-11-11)

| Milestone | State | Read |
| --- | --- | --- |
| M0 Foundation | Done; requiring the `CI` check on `main` is an open admin step | [DEVELOPMENT.md](DEVELOPMENT.md) |
| M1 Document core | Done (28/28 capabilities): URL, HTML tokenizer and tree builder, DOM, local `dump-dom` | [URL.md](URL.md), [HTML.md](HTML.md), [HTML_TREE.md](HTML_TREE.md), [DOM.md](DOM.md) |
| M2 Styles | Done (8/8): CSS syntax, selectors, values, cascade with layers, media/container queries, invalidation | [CSS.md](CSS.md) |
| M3 Static rendering | Done (17/17): block/inline/flex layout, OpenType text, images, CPU raster, `render`, reftests, hit testing | [LAYOUT.md](LAYOUT.md), [TEXT.md](TEXT.md), [PAINT.md](PAINT.md) |
| M4 JavaScript | Current: parser done, values/objects in progress, nothing executes yet | [JS.md](JS.md), [ROADMAP §7](ROADMAP.md#7-m4--original-ecmascript-runtime) |
| M5–M10 | Not started | [ROADMAP.md](ROADMAP.md) |

The [progress summary](progress/summary.md) is regenerated from
`progress/catalog.json` and is the measured status. Done means done for the
milestone's declared scope; broad web compatibility is M9 work and native
Android/iOS host acceptance is M8 work.

## Reading order

New to the project? Read these in order:

1. [Architecture](ARCHITECTURE.md): components, ownership, execution and data flow.
2. [Roadmap](ROADMAP.md): milestones, task IDs, gates, the current queue and risks.
3. [Implementation progress](IMPLEMENTATION.md): every finished task and its tested contract.
4. [Technical debt](TECH_DEBT.md): known shortcuts (TD-NN) and the rules that keep them safe.
5. [Platform support](PLATFORMS.md): the any-device rule and the CI target matrix.
6. [Development and CI](DEVELOPMENT.md): commands, CI jobs and the merge workflow.

## Document map

**Plan and scope**

| Document | Purpose |
| --- | --- |
| [PRODUCT.md](PRODUCT.md) | Full-product requirements: Brave-level privacy, Chrome/Firefox comparisons, GPU, Wasm |
| [EXTENSIONS.md](EXTENSIONS.md) | Rustic-only Manifest V2 extensions and the uBlock Origin port |
| [ROADMAP.md](ROADMAP.md) | Milestones M0–M10, tasks, gates, current queue and risk register |
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
| [adr/](adr/README.md) | Architecture decision records |

**Implemented subsystems** (each states its scope, limits and evidence)

| Document | Subsystem |
| --- | --- |
| [URL.md](URL.md) | URL parsing, origins, file URLs, IDNA |
| [HTML.md](HTML.md) | Byte decoding and the HTML tokenizer |
| [HTML_TREE.md](HTML_TREE.md) | Tree construction, fragments and `dump-dom` |
| [DOM.md](DOM.md) | Node storage, mutation, cloning/adoption and the tracing prototype |
| [CSS.md](CSS.md) | CSS syntax, selectors, values, cascade, queries, invalidation and computed style |
| [LAYOUT.md](LAYOUT.md) | Box tree, block/inline/flex layout, floats and positioning |
| [TEXT.md](TEXT.md) | Unicode data, fonts, shaping, bidi and line breaking |
| [PAINT.md](PAINT.md) | Display lists, CPU raster, image decoding and hit testing |
| [JS.md](JS.md) | The JavaScript lexer and parser (more as M4 lands) |

**History:** [reviews/](reviews/) holds dated milestone and checkpoint reviews.
[IMPLEMENTATION.md](IMPLEMENTATION.md) and [PROGRESS.md](PROGRESS.md) keep
their sections in the order the work landed, so a later section can supersede
an earlier one's "remaining" notes.

## Evidence at a glance

| Area | Evidence |
| --- | --- |
| URL | 887 pinned WPT URL records; all 6,389 Unicode IdnaTestV2 cases |
| HTML | 1,079 declaration, 4,626 character-reference and 282 text-mode tokenizer cases; all 1,776 runnable html5lib tree records at every two-chunk split |
| DOM | 25,230 pre-insert combinations against a specification oracle; seeded model tests |
| CSS | 8,041 color cases; 1,115 of 1,455 in-scope pinned WPT parsing cases with a reasoned ratchet |
| Layout and paint | Exact-geometry tests, checked reftests, 38 of 91 static css-flexbox WPT pages |
| Text | Shaping compared against 30 committed HarfBuzz reference cases |
| JavaScript | 22,956 of 23,123 Test262 `test/language` files parse correctly (parse phase only) |
| Robustness | 18 deterministic fuzz targets with a required smoke job and a nightly campaign |
| Platforms | Native tests on x64/ARM64 Linux, Windows and macOS; emulated 32-bit ARM/x86, RISC-V and big-endian; cross-checks for Android, iOS and eight more targets |

There are no whole-WPT or full Test262 pass rates, sandbox guarantees,
comparative performance results or releases yet. The only third-party Rust
crates are the reviewed font adapters of [ADR 0014](adr/0014-reviewed-font-adapters.md).
Configuration in the repository is distinct from a successful hosted run;
inspect the [Actions page](https://github.com/MoeJoe37/Rustic/actions) for run
results.

## Document conventions

“Must” states a design invariant or milestone gate. “Candidate” means a choice
still needs a focused evaluation. M0–M10 are defined in the roadmap. Task IDs
(DOC-, CSS-, REN-, JS-, WEB-, NET-, ISO-, UI-, MOB-, WASM-, EXT-, BLK-) are
stable; new work gets a new ID rather than reusing one. Future file paths in
prose are proposed; linked paths exist now.

Design baseline: 2026-11-07. Standards are living documents: each implementation
task records the exact section, upstream revision, and tests it used. Project
decisions are not claims that the standards prescribe this Rust architecture.
