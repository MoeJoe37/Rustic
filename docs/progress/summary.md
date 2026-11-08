# Engine progress snapshot

**Overall completion: 13.75%** = 11.96% verified complete + 1.79% partial credit. 29/172 capabilities complete, 2 in progress.

Catalog: `engine-v3`. Source fingerprint: `446fbef9576805bd34c0fe86df6a06a30ff186f05e3c80b4d1ba6ff8908db758`.

100% requires all capabilities and final five-platform engine acceptance. Weights and sizes measure planned engine scope; they are not compatibility results or time estimates. See [PROGRESS.md](../PROGRESS.md).

## Milestones

| Milestone | Capabilities complete | Progress | Verified | Share of engine |
| --- | --- | --- | --- | --- |
| M1 Document core | 28/28 | 100.00% | 100.00% | 11.27% |
| M2 CSS & computed style | 1/8 | 27.56% | 7.69% | 9.00% |
| M3 Static renderer | 0/17 | 0.00% | 0.00% | 12.22% |
| M4 ECMAScript runtime | 0/13 | 0.00% | 0.00% | 10.68% |
| M5 Scripted documents | 0/11 | 0.00% | 0.00% | 7.46% |
| M6 Networking & services | 0/12 | 0.00% | 0.00% | 5.12% |
| M7 Isolation & security | 0/8 | 0.00% | 0.00% | 4.46% |
| M8 Desktop & mobile alpha | 0/20 | 0.00% | 0.00% | 10.83% |
| M9 Compatibility expansion | 0/46 | 0.00% | 0.00% | 25.38% |
| M10 Embedding & release | 0/9 | 0.00% | 0.00% | 3.53% |

## Subsystems

| Subsystem | Completed | In progress | Progress | Verified | Engine weight | Engine points |
| --- | --- | --- | --- | --- | --- | --- |
| Foundation | 6/8 | 0 | 62.50% | 62.50% | 2% | 1.25 |
| URLs & encoding | 6/12 | 0 | 43.33% | 43.33% | 3% | 1.30 |
| DOM | 6/10 | 0 | 52.94% | 52.94% | 6% | 3.17 |
| HTML | 9/10 | 0 | 89.65% | 89.65% | 6% | 5.37 |
| CSS & style | 1/8 | 2 | 27.56% | 7.69% | 9% | 2.48 |
| Text & fonts | 0/6 | 0 | 0.00% | 0.00% | 4% | 0.00 |
| Layout | 0/10 | 0 | 0.00% | 0.00% | 9% | 0.00 |
| Paint & raster | 0/7 | 0 | 0.00% | 0.00% | 6% | 0.00 |
| JavaScript & GC | 0/15 | 0 | 0.00% | 0.00% | 12% | 0.00 |
| WebAssembly | 0/8 | 0 | 0.00% | 0.00% | 4% | 0.00 |
| Bindings & web APIs | 0/8 | 0 | 0.00% | 0.00% | 6% | 0.00 |
| Network & navigation | 0/7 | 0 | 0.00% | 0.00% | 4% | 0.00 |
| Storage & workers | 0/7 | 0 | 0.00% | 0.00% | 3% | 0.00 |
| Security & IPC | 0/11 | 0 | 0.00% | 0.00% | 6% | 0.00 |
| Compositor & GPU | 0/4 | 0 | 0.00% | 0.00% | 4% | 0.00 |
| Platform hosts | 1/9 | 0 | 4.16% | 4.16% | 4% | 0.16 |
| Accessibility | 0/4 | 0 | 0.00% | 0.00% | 2% | 0.00 |
| DevTools | 0/4 | 0 | 0.00% | 0.00% | 1% | 0.00 |
| Extensions (MV2) | 0/10 | 0 | 0.00% | 0.00% | 4% | 0.00 |
| uBO port & filtering | 0/8 | 0 | 0.00% | 0.00% | 3% | 0.00 |
| Embedding & release | 0/6 | 0 | 0.00% | 0.00% | 2% | 0.00 |

## Capability evidence

Each line shows status, scope size (1, 2, 3, 5 or 8 points within its subsystem), roadmap milestone and share of the whole engine.

### Foundation

- **complete** `base.identity` — Document/node identity and stale-handle contracts · size 2 · M1 · 0.25% of engine ([source](../../crates/rustic-base/src/id.rs), [test](../../crates/rustic-dom/tests/tree.rs))
- **complete** `base.budgets` — Checked resource accounting · size 2 · M1 · 0.25% of engine ([source](../../crates/rustic-base/src/budget.rs), [test](../../crates/rustic-base/src/budget.rs))
- **complete** `base.cancellation` — Cooperative cancellation · size 2 · M1 · 0.25% of engine ([source](../../crates/rustic-base/src/cancellation.rs), [test](../../crates/rustic-base/src/cancellation.rs))
- **complete** `base.spans` — Checked source byte spans · size 1 · M1 · 0.12% of engine ([source](../../crates/rustic-base/src/source.rs), [test](../../crates/rustic-base/src/source.rs))
- **complete** `base.clocks` — Injectable monotonic clocks · size 1 · M1 · 0.12% of engine ([source](../../crates/rustic-base/src/clock.rs), [test](../../crates/rustic-base/src/clock.rs))
- **planned** `base.geometry` — Typed CSS/device geometry and checked surface allocation · size 3 · M3 · 0.37% of engine
- **complete** `base.diagnostics` — Structured source/revision diagnostics and redaction · size 2 · M1 · 0.25% of engine ([source](../../crates/rustic-base/src/diagnostic.rs), [source](../../crates/rustic-base/src/revision.rs), [test](../../crates/rustic-base/src/diagnostic.rs), [test](../../crates/rustic-base/src/revision.rs))
- **planned** `base.lifecycle` — Coordinated lifecycle, shutdown and resource ownership · size 3 · M5 · 0.37% of engine

### URLs & encoding

- **complete** `url.records` — Declared ASCII URL/reference parsing subset · size 3 · M1 · 0.30% of engine ([source](../../crates/rustic-url/src/parser.rs), [test](../../crates/rustic-url/tests/wpt.rs))
- **complete** `url.addresses` — IPv4/IPv6 host parsing and canonicalization · size 2 · M1 · 0.20% of engine ([source](../../crates/rustic-url/src/ipv6.rs), [test](../../crates/rustic-url/tests/url.rs))
- **complete** `url.origins` — Tuple and process-local opaque origin identity · size 2 · M1 · 0.20% of engine ([source](../../crates/rustic-url/src/origin.rs), [test](../../crates/rustic-url/tests/url.rs))
- **complete** `url.idna` — Unicode IDNA and data adapter · size 3 · M1 · 0.30% of engine ([source](../../crates/rustic-url/src/idna/mod.rs), [source](../../crates/rustic-url/src/host.rs), [test](../../crates/rustic-url/tests/idna.rs), [test](../../crates/rustic-url/tests/idna.rs))
- **complete** `url.file` — File URL and platform rules · size 2 · M1 · 0.20% of engine ([source](../../crates/rustic-url/src/parser.rs), [test](../../crates/rustic-url/tests/url.rs))
- **planned** `url.blob` — Blob registry and origin rules · size 2 · M9 · 0.20% of engine
- **planned** `url.setters` — URL setters and URLSearchParams · size 2 · M9 · 0.20% of engine
- **planned** `url.encoding` — Required legacy decoders and charset selection · size 5 · M6 · 0.50% of engine
- **planned** `url.restart` — Encoding sniff/restart integration · size 2 · M6 · 0.20% of engine
- **complete** `url.diagnostics` — Validation-error diagnostics and source locations · size 1 · M1 · 0.10% of engine ([source](../../crates/rustic-url/src/diagnostics.rs), [test](../../crates/rustic-url/tests/diagnostics.rs))
- **planned** `url.policy` — Navigation origin inheritance and policy integration · size 3 · M6 · 0.30% of engine
- **planned** `url.conformance` — Complete declared URL/encoding conformance gate · size 3 · M9 · 0.30% of engine

### DOM

- **complete** `dom.storage` — Bounded document-owned node storage · size 3 · M1 · 0.52% of engine ([source](../../crates/rustic-dom/src/storage.rs), [test](../../crates/rustic-dom/src/storage.rs))
- **complete** `dom.mutation` — Checked same-document mutation and traversal · size 3 · M1 · 0.52% of engine ([source](../../crates/rustic-dom/src/document.rs), [test](../../crates/rustic-dom/tests/tree.rs))
- **complete** `dom.values` — Namespace/attribute identity and UTF-16 values · size 2 · M1 · 0.35% of engine ([source](../../crates/rustic-dom/src/string.rs), [test](../../crates/rustic-dom/tests/tree.rs))
- **complete** `dom.nodekinds` — Fragments, processing instructions and full node kinds · size 2 · M1 · 0.35% of engine ([source](../../crates/rustic-dom/src/node.rs), [source](../../crates/rustic-dom/src/document.rs), [test](../../crates/rustic-dom/tests/node_kinds.rs), [test](../../crates/rustic-dom/tests/node_kinds.rs))
- **complete** `dom.adoption` — Adoption, cloning and document lifecycle · size 3 · M1 · 0.52% of engine ([source](../../crates/rustic-dom/src/document/adoption.rs), [source](../../crates/rustic-dom/src/document.rs), [test](../../crates/rustic-dom/tests/adoption.rs), [test](../../crates/rustic-dom/tests/adoption.rs), [test](../../crates/rustic-dom/tests/adoption.rs))
- **planned** `dom.events` — DOM event propagation and default actions · size 5 · M5 · 0.88% of engine
- **planned** `dom.observers` — Mutation records and observers · size 3 · M5 · 0.52% of engine
- **planned** `dom.ranges` — Ranges, live collections and traversal APIs · size 3 · M9 · 0.52% of engine
- **planned** `dom.shadow` — Shadow DOM and custom element lifecycle · size 5 · M9 · 0.88% of engine
- **complete** `dom.tracing` — Tracing integration and DOM conformance gate · size 5 · M1 · 0.88% of engine ([source](../../crates/rustic-dom/src/document/tracing.rs), [test](../../crates/rustic-dom/tests/tracing.rs), [test](../../crates/rustic-dom/tests/tracing.rs), [source](../../crates/rustic-dom/src/document/invariants.rs), [test](../../crates/rustic-dom/tests/conformance.rs))

### HTML

- **complete** `html.input` — Bounded incremental UTF-8 decoding and input preprocessing · size 2 · M1 · 0.41% of engine ([source](../../crates/rustic-html/src/input.rs), [test](../../crates/rustic-html/tests/input.rs), [test](../../crates/rustic-html/tests/input.rs))
- **complete** `html.tags` — Declared data/tag/attribute tokenizer subset · size 3 · M1 · 0.62% of engine ([source](../../crates/rustic-html/src/tokenizer.rs), [test](../../crates/rustic-html/tests/tokenizer.rs), [test](../../crates/rustic-html/tests/tokenizer.rs), [test](../../crates/rustic-html/tests/tokenizer.rs))
- **complete** `html.references` — Full named/numeric character references · size 2 · M1 · 0.41% of engine ([source](../../crates/rustic-html/src/tokenizer/reference.rs), [test](../../crates/rustic-html/tests/references.rs), [test](../../crates/rustic-html/tests/references.rs), [test](../../crates/rustic-html/tests/corpus.rs))
- **complete** `html.declarations` — Comments, doctypes and declaration recovery · size 2 · M1 · 0.41% of engine ([source](../../crates/rustic-html/src/tokenizer/comment.rs), [source](../../crates/rustic-html/src/tokenizer/doctype.rs), [source](../../crates/rustic-html/src/tokenizer/instruction.rs), [test](../../crates/rustic-html/tests/declarations.rs), [test](../../crates/rustic-html/tests/corpus.rs))
- **complete** `html.textmodes` — RCDATA, RAWTEXT, script and plaintext modes · size 2 · M1 · 0.41% of engine ([source](../../crates/rustic-html/src/tokenizer/text.rs), [source](../../crates/rustic-html/src/tokenizer/driver.rs), [test](../../crates/rustic-html/tests/text_modes.rs), [test](../../crates/rustic-html/tests/text_modes.rs), [test](../../crates/rustic-html/tests/corpus.rs))
- **complete** `html.tree` — Core insertion modes and malformed-tree recovery · size 5 · M1 · 1.03% of engine ([source](../../crates/rustic-html/src/parser/tree.rs), [test](../../crates/rustic-html/tests/document.rs), [test](../../crates/rustic-html/tests/tree_construction.rs))
- **complete** `html.tables` — Table, formatting, template and foreign content rules · size 5 · M1 · 1.03% of engine ([source](../../crates/rustic-html/src/parser/tree/formatting.rs), [source](../../crates/rustic-html/src/parser/tree/table.rs), [source](../../crates/rustic-html/src/parser/tree/foreign.rs), [test](../../crates/rustic-html/tests/tree_rules.rs), [test](../../crates/rustic-html/tests/tree_construction.rs))
- **planned** `html.scripts` — Parser/script reentry and encoding restart · size 3 · M5 · 0.62% of engine
- **complete** `html.loader` — Bounded local loader and dump-dom command · size 2 · M1 · 0.41% of engine ([source](../../crates/rustic-cli/src/loader.rs), [test](../../crates/rustic-cli/tests/cli.rs))
- **complete** `html.conformance` — Parser corpora, chunk invariance and fuzz gate · size 3 · M1 · 0.62% of engine ([source](../../tools/import_html_tests.py), [test](../../crates/rustic-html/tests/corpus.rs), [source](../../crates/rustic-html/src/parser/fragment.rs), [test](../../crates/rustic-html/tests/tree_construction.rs), [test](../../crates/rustic-html/tests/fragment.rs), [source](../../crates/rustic-fuzz/src/targets.rs), [test](../../crates/rustic-fuzz/tests/harness.rs))

### CSS & style

- **complete** `css.syntax` — CSS tokenizer/parser and recovery · size 3 · M2 · 0.69% of engine ([source](../../crates/rustic-css/src/tokenizer.rs), [source](../../crates/rustic-css/src/rules.rs), [source](../../tools/import_css_tests.py), [test](../../crates/rustic-css/tests/corpus.rs), [test](../../crates/rustic-css/tests/syntax.rs))
- **in progress 75.00%** `css.selectors` — Required selectors and specificity · size 5 · M2 · 1.15% of engine
  - [x] Selector syntax, specificity and CSSOM serialization (CSS-02 subset) ([source](../../crates/rustic-css/src/selector.rs), [test](../../crates/rustic-css/tests/selectors.rs))
  - [x] An+B microsyntax (pinned css-parsing-tests An+B.json) ([source](../../crates/rustic-css/src/anb.rs), [test](../../crates/rustic-css/tests/an_plus_b.rs))
  - [x] Selector matching over the DOM with HTML/quirks/XML case rules ([source](../../crates/rustic-style/src/matching.rs), [test](../../crates/rustic-style/tests/matching.rs))
  - [ ] Remaining required selectors: :has(), form-state, :lang()/:dir(), nesting & and pseudo-element states (CSS-07)
- **in progress 50.00%** `css.properties` — Typed property grammar and supported property catalog · size 8 · M2 · 1.84% of engine
  - [x] <color> values (pinned css-parsing-tests Color 3/4 files) ([source](../../crates/rustic-css/src/color.rs), [test](../../crates/rustic-css/tests/color.rs))
  - [x] Lengths, percentages and the CSS-03 longhands and shorthands with initial values and inheritance ([source](../../crates/rustic-css/src/properties.rs), [test](../../crates/rustic-css/tests/properties.rs))
  - [ ] Math functions: calc(), min(), max(), clamp()
  - [ ] Property catalog for layout, text, backgrounds, positioning and the font shorthand
- **planned** `css.cascade` — Complete cascade/origins/layers and inheritance · size 5 · M2 · 1.15% of engine
- **planned** `css.values` — Computed values, variables and cycle handling · size 5 · M2 · 1.15% of engine
- **planned** `css.queries` — Media/container queries and style attachment · size 3 · M2 · 0.69% of engine
- **planned** `css.invalidation` — Correct incremental invalidation against reference path · size 5 · M2 · 1.15% of engine
- **planned** `css.conformance` — Declared CSS conformance and regression gate · size 5 · M2 · 1.15% of engine

### Text & fonts

- **planned** `text.fonts` — Font selection/discovery and metrics · size 3 · M3 · 0.54% of engine
- **planned** `text.shaping` — Shaping, fallback and glyph clusters · size 5 · M3 · 0.90% of engine
- **planned** `text.bidi` — Bidi and mixed-direction text · size 3 · M3 · 0.54% of engine
- **planned** `text.breaking` — Line breaking and grapheme navigation · size 3 · M3 · 0.54% of engine
- **planned** `text.writing` — Writing modes, vertical text and font loading · size 5 · M9 · 0.90% of engine
- **planned** `text.conformance` — Text/font conformance and platform parity · size 3 · M9 · 0.54% of engine

### Layout

- **planned** `layout.boxes` — Box/fragment trees and checked geometry · size 3 · M3 · 0.58% of engine
- **planned** `layout.block` — Block flow, margins and anonymous boxes · size 5 · M3 · 0.97% of engine
- **planned** `layout.inline` — Inline formatting, line boxes and baselines · size 5 · M3 · 0.97% of engine
- **planned** `layout.sizing` — Intrinsic and replaced-element sizing · size 3 · M3 · 0.58% of engine
- **planned** `layout.position` — Floats, positioning and overflow · size 5 · M3 · 0.97% of engine
- **planned** `layout.flex` — Flexbox · size 5 · M3 · 0.97% of engine
- **planned** `layout.grid` — Grid · size 5 · M9 · 0.97% of engine
- **planned** `layout.tables` — Tables · size 5 · M9 · 0.97% of engine
- **planned** `layout.fragmentation` — Fragmentation, printing and advanced formatting · size 5 · M9 · 0.97% of engine
- **planned** `layout.incremental` — Incremental/full-layout parity and conformance · size 5 · M9 · 0.97% of engine

### Paint & raster

- **planned** `paint.display` — Immutable display lists and paint order · size 3 · M3 · 0.66% of engine
- **planned** `paint.stacking` — Stacking, clips and transforms · size 3 · M3 · 0.66% of engine
- **planned** `paint.raster` — Deterministic CPU reference rasterizer · size 5 · M3 · 1.11% of engine
- **planned** `paint.images` — Bounded images, color/alpha and resource lifecycle · size 3 · M3 · 0.66% of engine
- **planned** `paint.hit` — Hit testing and resize/zoom geometry · size 2 · M3 · 0.44% of engine
- **planned** `paint.formats` — SVG/MathML painting integration and Canvas 2D · size 8 · M9 · 1.77% of engine
- **planned** `paint.conformance` — Reftest output, damage and paint conformance · size 3 · M3 · 0.66% of engine

### JavaScript & GC

- **planned** `js.parser` — Lexer/parser, ASI, early errors and UTF-16 · size 5 · M4 · 0.82% of engine
- **planned** `js.values` — Values, coercions, references and completions · size 3 · M4 · 0.49% of engine
- **planned** `js.objects` — Object methods, descriptors and prototypes · size 3 · M4 · 0.49% of engine
- **planned** `js.heap` — Rooted traced heap and host-object cycle collection · size 8 · M4 · 1.31% of engine
- **planned** `js.interpreter` — Original interpreter/bytecode and control flow · size 8 · M4 · 1.31% of engine
- **planned** `js.functions` — Functions, closures, call/construct and exceptions · size 5 · M4 · 0.82% of engine
- **planned** `js.builtins` — Core built-ins and collections · size 8 · M4 · 1.31% of engine
- **planned** `js.jobs` — Promises, jobs and realms · size 3 · M4 · 0.49% of engine
- **planned** `js.modules` — Modules and host loading hooks · size 3 · M4 · 0.49% of engine
- **planned** `js.advanced` — BigInt, RegExp, proxies, generators and async · size 8 · M4 · 1.31% of engine
- **planned** `js.buffers` — Buffers, typed arrays and memory semantics · size 3 · M4 · 0.49% of engine
- **planned** `js.intl` — Intl and required Unicode semantics · size 5 · M4 · 0.82% of engine
- **planned** `js.wasm` — JavaScript/WebAssembly identity, conversions and tracing integration · size 3 · M9 · 0.49% of engine
- **planned** `js.limits` — Cancellation, stack/heap limits and GC stress · size 3 · M4 · 0.49% of engine
- **planned** `js.conformance` — Declared full Test262 and runtime correctness gate · size 5 · M9 · 0.82% of engine

### WebAssembly

- **planned** `wasm.decode` — Bounded binary decoder and malformed-module handling · size 2 · M9 · 0.23% of engine
- **planned** `wasm.validate` — Complete selected type/instruction/module validation · size 3 · M9 · 0.35% of engine
- **planned** `wasm.execute` — Original interpreter, numerics, control flow and traps · size 5 · M9 · 0.58% of engine
- **planned** `wasm.instances` — Memories, tables, globals, instances and resource limits · size 3 · M9 · 0.35% of engine
- **planned** `wasm.web` — Streaming Web API, MIME/CSP and capability-safe imports · size 3 · M9 · 0.35% of engine
- **planned** `wasm.features` — Required reference/GC, exception, SIMD, shared-memory and baseline features · size 8 · M9 · 0.94% of engine
- **planned** `wasm.accelerate` — Reviewed acceleration with interpreter fallback on mobile/desktop · size 5 · M9 · 0.58% of engine
- **planned** `wasm.conformance` — Core/JS/Web suites, fuzzing and five-platform acceptance · size 5 · M9 · 0.58% of engine

### Bindings & web APIs

- **planned** `web.idl` — Web IDL tooling, conversions and rooted wrappers · size 5 · M5 · 0.93% of engine
- **planned** `web.window` — Window/Document/Element bindings and identity · size 5 · M5 · 0.93% of engine
- **planned** `web.scheduler` — Tasks, microtasks and rendering opportunities · size 3 · M5 · 0.56% of engine
- **planned** `web.scripts` — Classic/module script lifecycle and readiness · size 3 · M5 · 0.56% of engine
- **planned** `web.input` — Forms, focus, editing and user input · size 5 · M5 · 0.93% of engine
- **planned** `web.observers` — Timers, animation frames and observer APIs · size 3 · M5 · 0.56% of engine
- **planned** `web.streams` — Streams, messaging and structured clone · size 5 · M9 · 0.93% of engine
- **planned** `web.conformance` — Exposed web API conformance and reentry gate · size 3 · M5 · 0.56% of engine

### Network & navigation

- **planned** `network.transport` — Reviewed TLS/HTTP transport with cancellation · size 5 · M6 · 0.68% of engine
- **planned** `network.navigation` — Navigation/history state machine and redirects · size 5 · M6 · 0.68% of engine
- **planned** `network.fetch` — Fetch, CORS and response filtering · size 5 · M6 · 0.68% of engine
- **planned** `network.cookies` — Cookies and partitioned HTTP cache · size 3 · M6 · 0.41% of engine
- **planned** `network.policy` — CSP, MIME, secure contexts and mixed-content policy · size 3 · M6 · 0.41% of engine
- **planned** `network.protocols` — Required HTTP versions, WebSockets and referrer rules · size 5 · M9 · 0.68% of engine
- **planned** `network.conformance` — Network policy and navigation conformance gate · size 3 · M6 · 0.41% of engine

### Storage & workers

- **planned** `storage.profiles` — Ephemeral/persistent profiles and private mode · size 2 · M6 · 0.27% of engine
- **planned** `storage.partition` — Storage keys, quotas and schema recovery · size 3 · M6 · 0.40% of engine
- **planned** `storage.webstorage` — Local/session storage · size 1 · M6 · 0.13% of engine
- **planned** `storage.indexeddb` — IndexedDB transactions and crash recovery · size 5 · M9 · 0.68% of engine
- **planned** `storage.workers` — Worker agents, lifetimes and messaging · size 3 · M9 · 0.40% of engine
- **planned** `storage.offline` — Service workers and Cache API · size 5 · M9 · 0.68% of engine
- **planned** `storage.conformance` — Persistence/offline/worker isolation and conformance · size 3 · M9 · 0.40% of engine

### Security & IPC

- **planned** `security.ipc` — Bounded versioned hostile-peer-safe IPC · size 3 · M7 · 0.41% of engine
- **planned** `security.authority` — Browser-assigned authority and capability brokering · size 3 · M7 · 0.41% of engine
- **planned** `security.isolation` — Site/agent/process assignment and origin separation · size 5 · M7 · 0.69% of engine
- **planned** `security.desktop` — Verified Windows/macOS/Linux sandboxing · size 5 · M7 · 0.69% of engine
- **planned** `security.mobile` — Verified Android/iOS process and broker restrictions · size 5 · M7 · 0.69% of engine
- **planned** `security.permissions` — Trusted permissions, activation, files and downloads · size 3 · M7 · 0.41% of engine
- **planned** `security.recovery` — Renderer/service crash recovery without stale authority · size 3 · M7 · 0.41% of engine
- **planned** `security.brave` — Versioned Brave security comparison and independent review · size 3 · M10 · 0.41% of engine
- **planned** `security.privacy` — Default tracking/bounce/fingerprinting defenses and privacy matrix · size 5 · M10 · 0.69% of engine
- **planned** `security.updates` — Authenticated updates, incident response and vulnerability triage · size 3 · M10 · 0.41% of engine
- **planned** `security.audit` — Fuzz/native-memory review and security release gate · size 5 · M7 · 0.69% of engine

### Compositor & GPU

- **planned** `compositor.scene` — Incremental scene submission and scrolling · size 3 · M8 · 0.92% of engine
- **planned** `compositor.backend` — Mobile/desktop GPU adapters and CPU fallback · size 5 · M8 · 1.53% of engine
- **planned** `compositor.recovery` — Surface/device loss and resource recovery · size 2 · M8 · 0.61% of engine
- **planned** `compositor.parity` — GPU/reference parity, performance and energy gates · size 3 · M8 · 0.92% of engine

### Platform hosts

- **complete** `platform.crosschecks` — Android/iOS shared-core cross-compilation · size 1 · M1 · 0.16% of engine ([source](../../.github/workflows/ci.yml), [test](../../.github/workflows/ci.yml))
- **planned** `platform.linux` — Linux runtime and native host acceptance · size 3 · M8 · 0.50% of engine
- **planned** `platform.windows` — Windows runtime and native host acceptance · size 3 · M8 · 0.50% of engine
- **planned** `platform.macos` — macOS runtime and native host acceptance · size 3 · M8 · 0.50% of engine
- **planned** `platform.android` — Android runtime and native host acceptance · size 3 · M8 · 0.50% of engine
- **planned** `platform.ios` — iOS/iPadOS runtime and native host acceptance · size 3 · M8 · 0.50% of engine
- **planned** `platform.lifecycle` — Surface, touch/IME and background lifecycle parity · size 3 · M8 · 0.50% of engine
- **planned** `platform.native` — Native font/codec/OS service adapters · size 3 · M8 · 0.50% of engine
- **planned** `platform.device` — Linked artifacts, device/simulator suites and OS policy · size 2 · M8 · 0.33% of engine

### Accessibility

- **planned** `a11y.semantics` — Semantic roles, names and state tree · size 3 · M8 · 0.42% of engine
- **planned** `a11y.bridges` — Desktop and mobile accessibility bridges · size 5 · M8 · 0.71% of engine
- **planned** `a11y.editing` — Keyboard/IME, focus and assistive editing flows · size 3 · M8 · 0.42% of engine
- **planned** `a11y.acceptance` — Assistive-technology and accessibility acceptance · size 3 · M8 · 0.42% of engine

### DevTools

- **planned** `devtools.inspection` — DOM/style/layout inspection and console · size 3 · M8 · 0.30% of engine
- **planned** `devtools.debugger` — Source debugging, breakpoints and network diagnostics · size 3 · M8 · 0.30% of engine
- **planned** `devtools.automation` — Secure headless automation and profiling protocol · size 3 · M8 · 0.30% of engine
- **planned** `devtools.acceptance` — Debugger privacy, authority and diagnostic acceptance · size 1 · M8 · 0.10% of engine

### Extensions (MV2)

- **planned** `extensions.manifest` — Rustic-native Manifest V2 schema and API version contract · size 2 · M9 · 0.25% of engine
- **planned** `extensions.packages` — Package identity, signatures and hostile archive validation · size 3 · M9 · 0.37% of engine
- **planned** `extensions.worlds` — Isolated content-script worlds and extension principals · size 5 · M9 · 0.62% of engine
- **planned** `extensions.background` — MV2 background pages and mobile lifecycle · size 3 · M9 · 0.37% of engine
- **planned** `extensions.messaging` — Attributed bounded messaging and storage APIs · size 3 · M9 · 0.37% of engine
- **planned** `extensions.ui` — Actions, options, permissions and developer diagnostics · size 3 · M9 · 0.37% of engine
- **planned** `extensions.requests` — Blocking webRequest and reviewed response filtering · size 5 · M9 · 0.62% of engine
- **planned** `extensions.updates` — Signed updates, revocation and rollback protection · size 2 · M9 · 0.25% of engine
- **planned** `extensions.platforms` — Native desktop/Android/iOS extension acceptance · size 3 · M9 · 0.37% of engine
- **planned** `extensions.conformance` — Supported-API parity and hostile-extension security gate · size 3 · M9 · 0.37% of engine

### uBO port & filtering

- **planned** `filtering.inventory` — Pinned uBlock Origin port feature/license inventory · size 1 · M9 · 0.11% of engine
- **planned** `filtering.network` — Reference static/dynamic network rules and exceptions · size 5 · M9 · 0.55% of engine
- **planned** `filtering.cosmetic` — Cosmetic/procedural filters, packaged scriptlets and resources · size 5 · M9 · 0.55% of engine
- **planned** `filtering.broker` — Redirect/cache/worker/WebSocket and request-policy integration · size 3 · M9 · 0.33% of engine
- **planned** `filtering.port` — Rustic uBO port dashboard, logger, picker and advanced controls · size 5 · M9 · 0.55% of engine
- **planned** `filtering.lists` — Bounded transactional filter-list updates and rollback · size 2 · M9 · 0.22% of engine
- **planned** `filtering.optimize` — Rust acceleration/reference parity and measured budgets · size 3 · M9 · 0.33% of engine
- **planned** `filtering.acceptance` — Full declared uBO feature, mobile and security acceptance · size 3 · M9 · 0.33% of engine

### Embedding & release

- **planned** `release.embedding` — Versioned ownership/thread/cancellation embedding contract · size 3 · M10 · 0.27% of engine
- **planned** `release.compatibility` — Representative sites and complete declared conformance gates · size 5 · M10 · 0.45% of engine
- **planned** `release.performance` — Measured time, memory, reliability and energy budgets · size 3 · M10 · 0.27% of engine
- **planned** `release.artifacts` — Reproducible signed desktop/mobile artifacts and updates · size 3 · M10 · 0.27% of engine
- **planned** `release.comparison` — Controlled Chrome/Firefox speed, memory and energy comparison gates · size 3 · M10 · 0.27% of engine
- **planned** `release.complete` — Final full-engine acceptance across all five target platforms · size 5 · M10 · 0.45% of engine
