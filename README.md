# Rustic

[![CI](https://github.com/MoeJoe37/Rustic/actions/workflows/ci.yml/badge.svg)](https://github.com/MoeJoe37/Rustic/actions/workflows/ci.yml)

Rustic is an independent browser engine being built from scratch in Rust. The
goal is a standards-oriented, embeddable engine with its own HTML/CSS pipeline,
DOM, layout, painting, JavaScript runtime, and web platform integration.

Rustic must run on **any device that can run a browser**. The release platforms
are **Windows, macOS, Linux, Android, and iOS/iPadOS** on every CPU they ship
on: x86-64, ARM64, 32-bit ARM and 32-bit x86, with RISC-V, big-endian and other
architectures kept working too. CI runs the full test suite natively on x64 and
ARM64 Linux, Windows and macOS, under emulation on 32-bit ARM, 32-bit x86,
RISC-V and big-endian s390x, and compile-checks Android, iOS and further targets.
Device shells, rendering, input, and runtime validation are still planned. See
[platform support](docs/PLATFORMS.md) and the [mobile plan](docs/MOBILE.md).

**Current stage: M6 networking and browser services.** M1–M5 are complete
for their declared scope. Rustic's own pipeline turns a local HTML file into a PNG
through parsing, the CSS cascade, block/inline/flex layout, OpenType text,
image decoding and a CPU rasterizer, with resize/zoom and hit testing. M4
added Rustic's own ECMAScript engine: parser, garbage-collected heap and
bytecode interpreter with the full standard library, realms, modules, BigInt,
Proxy, typed arrays and Atomics, and ECMA-402 Intl (number, date and time,
relative time, duration, list, plural, collation, display names and
segmentation) over pinned CLDR and time zone data. It is designed around
memory use and interpreter speed
([plan](docs/ROADMAP.md#memory-and-performance-design)); the
[JavaScript notes](docs/JS.md) give the Test262 results, the benchmark corpus
and what is left for later milestones.

M5 connected that engine to documents. A host can queue scripts and
trusted input against a parsed document: script sees a live `window` and
`document`, creates, moves and queries nodes, dispatches events, and uses
timers and `requestAnimationFrame`, with one garbage collector tracing the DOM
and the JavaScript heap together ([scripted documents](docs/SCRIPTING.md)).
A rendering turn gives the host the live tree to render again in full. Hosts
can opt into running inline and local external classics and static JavaScript
modules, with document and script readiness events. `rustic render --run-scripts`
can render the resulting page. Scripts can observe mutations with
`MutationObserver`, report errors to `window.onerror`, move focus and
validate and submit forms without navigation, and watch element sizes with
`ResizeObserver`. A pinned web-platform-tests selection runs through
upstream `testharness.js`, and a [first cross-engine report](docs/reviews/2026-11-19-perf-01-m5.md)
compares the JavaScript engine with V8, SpiderMonkey and JavaScriptCore
([M5 review](docs/reviews/2026-11-19-m5-completion.md)).
M6 entry work is under way: Rustic decodes and encodes all 40
[Encoding Standard](docs/ENCODING.md) encodings, and stylesheets honor their
`@charset`. Networking itself and
native app hosts are later milestones.
See the [implementation ledger](docs/IMPLEMENTATION.md) for tested contracts.

<!-- rustic-progress:start -->

## Engine progress

[![Rustic engine progress map](docs/progress/engine.svg)](docs/progress/summary.md)

**Overall engine completion: 40.66%** (38.70% verified complete + 1.96% partial credit) — 91/221 capabilities complete and 9 in progress across 25 subsystems. Current milestone: **M7 Isolation & security**, 47.65% done.

100% requires the full capability catalog and final engine acceptance on Windows, macOS, Linux, Android and iOS. This is weighted delivery progress; see [measurement and automatic updates](docs/PROGRESS.md).

<!-- rustic-progress:end -->

## Start here

| Document | Purpose |
| --- | --- |
| [Documentation index](docs/README.md) | Reading order and current implementation inventory |
| [Full-engine product requirements](docs/PRODUCT.md) | Brave-level security/privacy, broad site/CSS/JS compatibility, competitive targets, GPU and Wasm |
| [Rustic extensions and uBO port](docs/EXTENSIONS.md) | Rustic-only Manifest V2 ecosystem, blocking APIs and the planned uBlock Origin port |
| [Implementation progress](docs/IMPLEMENTATION.md) | Completed tasks, tested contracts, and the next implementation work |
| [Progress graph methodology](docs/PROGRESS.md) | Whole-engine completion criteria, weights, evidence and automatic commit updates |
| [URLs and origins](docs/URL.md) | Parser scope, identity rules, limits and pinned upstream test evidence |
| [HTML](docs/HTML.md) and [tree construction](docs/HTML_TREE.md) | Tokenizer, tree builder, fragments and `dump-dom` |
| [DOM](docs/DOM.md) | Node storage, mutation, adoption and the tracing prototype |
| [CSS](docs/CSS.md) | Syntax, selectors, values, cascade, queries, invalidation and computed style |
| [Layout](docs/LAYOUT.md), [text](docs/TEXT.md) and [paint](docs/PAINT.md) | The M3 static renderer |
| [JavaScript](docs/JS.md) | The M4 engine: parser, heap, interpreter, standard library, Intl and its Test262 results |
| [Scripted documents](docs/SCRIPTING.md) | The M5 integration: script turns, bindings, events, observers, forms and what is still open |
| [Technical debt](docs/TECH_DEBT.md) | Known shortcuts and the rules that keep them safe |
| [Platform and device support](docs/PLATFORMS.md) | Any device that can run a browser: portability rules and the CI target matrix |
| [Mobile support](docs/MOBILE.md) | Android/iOS architecture, compile checks and device milestones |
| [Architecture](docs/ARCHITECTURE.md) | Components, ownership, execution, IPC, embedding, and data flow |
| [Roadmap](docs/ROADMAP.md) | Dependency-ordered milestones, acceptance gates, first tasks, and risks |
| [Standards and compatibility](docs/STANDARDS.md) | Feature scope, authoritative references, and conformance reporting |
| [Security model](docs/SECURITY.md) | Trust boundaries, origin policy, isolation, and browsing gates |
| [Testing strategy](docs/TESTING.md) | Unit tests, conformance, rendering, fuzzing, and regression policy |
| [Performance plan](docs/PERFORMANCE.md) | Benchmarks, budgets, profiling, and optimization order |
| [Development and CI](docs/DEVELOPMENT.md) | Build instructions and exact automation coverage |
| [Dependencies and platforms](docs/DEPENDENCIES.md) | Meaning of “from scratch,” dependency policy, and platform rollout |
| [Architecture decisions](docs/adr/README.md) | Decisions, alternatives, and review triggers |

## Try it

Install Rust through [rustup](https://rustup.rs/), then:

```sh
git clone https://github.com/MoeJoe37/Rustic.git
cd Rustic
cargo run --locked -p rustic-cli -- --help
cargo run --locked -p rustic-cli -- dump-dom crates/rustic-cli/tests/fixtures/styled.html
cargo run --locked -p rustic-cli -- dump-style crates/rustic-cli/tests/fixtures/styled.html
cargo run --locked -p rustic-cli -- dump-layout crates/rustic-cli/tests/fixtures/layout.html
cargo run --locked -p rustic-cli -- render --output out.png crates/rustic-cli/tests/fixtures/m3/article.html
cargo run --locked -p rustic-js --example parse -- -e 'let f = (a, b) => a ?? b;'
cargo test --locked --workspace --all-targets
```

The toolchain file pins Rust **1.99.1**; the minimum supported Rust version is
**1.85.0** and the workspace uses the 2024 edition. `rustic` is a development
tool, not a browser: every command reads one local file (URLs are rejected) and
prints a deterministic result.

| Command | Output |
| --- | --- |
| `dump-dom` | The parsed DOM tree, within byte, token, node and output budgets |
| `dump-style` | Each element's computed style, including local `<link>`/`@import` sheets |
| `dump-boxes`, `dump-formatting` | The box tree before layout, with anonymous boxes |
| `dump-layout` | Used geometry, line boxes and text fragments |
| `dump-paint` | The display list in paint order |
| `render --output out.png` | A CPU-rasterized PNG; `--viewport WxH` and `--zoom` relayout |
| `reftest` | Compares a page with its `rel=match`/`rel=mismatch` references |
| `hit-test --at X,Y` | The topmost box at each point |

Text uses OpenType shaping with bidi and CSS line breaking; `--font FILE` grants
local TTF/OTF/WOFF/WOFF2 fonts, otherwise bundled test fonts keep output
reproducible (they cover only a few characters). Local PNG/JPEG/GIF/WebP images
next to the document paint. See [DEVELOPMENT.md](docs/DEVELOPMENT.md) for every
check CI runs and [TEXT.md](docs/TEXT.md) for font limits.

### Crates

| Crate | Role |
| --- | --- |
| `rustic-base` | IDs, budgets, cancellation, source spans, clocks, geometry, redacted diagnostics |
| `rustic-url` | URL parsing, origins, IDNA |
| `rustic-html` | Streaming tokenizer and full WHATWG tree construction |
| `rustic-dom` | Bounded node storage, checked mutation, UTF-16 text, tracing prototype |
| `rustic-css` | CSS syntax, selectors and typed property values |
| `rustic-style` | Selector matching, cascade, computed style and invalidation |
| `rustic-text` | Fonts, shaping (reviewed adapters), Unicode segmentation and bidi |
| `rustic-layout` | Box tree, block/inline/flex layout, fragments |
| `rustic-paint` | Display lists and paint order |
| `rustic-raster` | CPU rasterizer and PNG output |
| `rustic-codecs` | Bounded inflate, PNG, JPEG, GIF and WebP decoders |
| `rustic-js` | ECMAScript parser, garbage-collected heap, bytecode interpreter, standard library and Intl |
| `rustic-bindings` | Web IDL conversions, `Window`/`Document`/node wrappers, events, and the shared DOM/JS garbage collection |
| `rustic-engine` | Document lifecycle, script, input, timer and rendering turns, viewport configuration |
| `rustic-cli` | The `rustic` development tool |
| `rustic-fuzz` | Deterministic mutation fuzzing (21 targets) |

Future crates in the [architecture](docs/ARCHITECTURE.md) are created only when
their milestone needs them. All packages are unpublished.

## Development direction

The complete product targets security/privacy at least as strong as Brave, broad
standards-complete website/CSS/JavaScript support with the functionality of the
other engines (Blink, Gecko and WebKit), and measured performance better than all
three. GPU acceleration, WebAssembly, **Rustic-only Manifest V2
extensions**, and a rebuilt/ported **uBlock Origin for Rustic** are required parts
of the plan. These are future acceptance targets, not current support or measured
superiority. See [product requirements](docs/PRODUCT.md) for exact gates and scope.

1. Establish deterministic infrastructure and parser tests.
2. Render local static documents with an original HTML/CSS/layout pipeline.
3. Build the original JavaScript interpreter and connect it to DOM/event loops.
4. Add controlled networking, browser policy, storage, and platform integration.
5. Establish process isolation and security gates before general web browsing.
6. Expand modern platform compatibility, accessibility, performance, and embedding.
7. Close the remaining feature gap with Blink, Gecko and WebKit (M11).
8. Beat all three on pinned benchmarks, memory and energy on every reference device (M12).
9. Ship the full engine 1.0 once every gate passes on all five platform families (M13).

Rustic is not based on **Mozilla, Firefox, Gecko, or SpiderMonkey**. No Chromium,
Blink, WebKit, Servo, or existing JavaScript engine is embedded as its implementation.
That rule applies to desktop and mobile hosts alike. Reviewed low-level libraries
for TLS, fonts, Unicode, image codecs,
graphics APIs, and operating system integration are allowed. See
[dependency boundaries](docs/DEPENDENCIES.md) for the precise policy.

## Contributing and license

Read [CONTRIBUTING.md](CONTRIBUTING.md) before making a change. Security reports
follow [SECURITY.md](SECURITY.md). The existing [GNU Affero General Public License
version 3](LICENSE) is preserved; workspace metadata uses `AGPL-3.0-only`.
