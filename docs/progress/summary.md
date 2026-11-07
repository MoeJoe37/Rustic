# Engine progress snapshot

**Overall completion: 6.64%** · 16/172 capabilities complete.

Catalog: `engine-v2`. Source fingerprint: `d7c5b660840b74daa6c58e7997d090c1578e6d2f1dc0eaade3b51dc9ec8514a0`.

100% requires all capabilities and final five-platform engine acceptance. Weights measure planned engine scope; they are not compatibility results or time estimates.

| Subsystem | Completed | Subsystem progress | Engine weight |
| --- | --- | --- | --- |
| Foundation | 5/8 | 62.50% | 2% |
| URLs & encoding | 3/12 | 25.00% | 3% |
| DOM | 3/10 | 30.00% | 6% |
| HTML | 4/10 | 40.00% | 6% |
| CSS & style | 0/8 | 0.00% | 9% |
| Text & fonts | 0/6 | 0.00% | 4% |
| Layout | 0/10 | 0.00% | 9% |
| Paint & raster | 0/7 | 0.00% | 6% |
| JavaScript & GC | 0/15 | 0.00% | 12% |
| WebAssembly | 0/8 | 0.00% | 4% |
| Bindings & web APIs | 0/8 | 0.00% | 6% |
| Network & navigation | 0/7 | 0.00% | 4% |
| Storage & workers | 0/7 | 0.00% | 3% |
| Security & IPC | 0/11 | 0.00% | 6% |
| Compositor & GPU | 0/4 | 0.00% | 4% |
| Platform hosts | 1/9 | 11.11% | 4% |
| Accessibility | 0/4 | 0.00% | 2% |
| DevTools | 0/4 | 0.00% | 1% |
| Extensions (MV2) | 0/10 | 0.00% | 4% |
| uBO port & filtering | 0/8 | 0.00% | 3% |
| Embedding & release | 0/6 | 0.00% | 2% |

## Capability evidence

### Foundation

- **complete** `base.identity` — Document/node identity and stale-handle contracts ([source](../../crates/rustic-base/src/id.rs), [test](../../crates/rustic-dom/tests/tree.rs))
- **complete** `base.budgets` — Checked resource accounting ([source](../../crates/rustic-base/src/budget.rs), [test](../../crates/rustic-base/src/budget.rs))
- **complete** `base.cancellation` — Cooperative cancellation ([source](../../crates/rustic-base/src/cancellation.rs), [test](../../crates/rustic-base/src/cancellation.rs))
- **complete** `base.spans` — Checked source byte spans ([source](../../crates/rustic-base/src/source.rs), [test](../../crates/rustic-base/src/source.rs))
- **complete** `base.clocks` — Injectable monotonic clocks ([source](../../crates/rustic-base/src/clock.rs), [test](../../crates/rustic-base/src/clock.rs))
- **planned** `base.geometry` — Typed CSS/device geometry and checked surface allocation
- **planned** `base.diagnostics` — Structured source/revision diagnostics and redaction
- **planned** `base.lifecycle` — Coordinated lifecycle, shutdown and resource ownership

### URLs & encoding

- **complete** `url.records` — Declared ASCII URL/reference parsing subset ([source](../../crates/rustic-url/src/parser.rs), [test](../../crates/rustic-url/tests/wpt.rs))
- **complete** `url.addresses` — IPv4/IPv6 host parsing and canonicalization ([source](../../crates/rustic-url/src/ipv6.rs), [test](../../crates/rustic-url/tests/url.rs))
- **complete** `url.origins` — Tuple and process-local opaque origin identity ([source](../../crates/rustic-url/src/origin.rs), [test](../../crates/rustic-url/tests/url.rs))
- **planned** `url.idna` — Unicode IDNA and data adapter
- **planned** `url.file` — File URL and platform rules
- **planned** `url.blob` — Blob registry and origin rules
- **planned** `url.setters` — URL setters and URLSearchParams
- **planned** `url.encoding` — Required legacy decoders and charset selection
- **planned** `url.restart` — Encoding sniff/restart integration
- **planned** `url.diagnostics` — Validation-error diagnostics and source locations
- **planned** `url.policy` — Navigation origin inheritance and policy integration
- **planned** `url.conformance` — Complete declared URL/encoding conformance gate

### DOM

- **complete** `dom.storage` — Bounded document-owned node storage ([source](../../crates/rustic-dom/src/storage.rs), [test](../../crates/rustic-dom/src/storage.rs))
- **complete** `dom.mutation` — Checked same-document mutation and traversal ([source](../../crates/rustic-dom/src/document.rs), [test](../../crates/rustic-dom/tests/tree.rs))
- **complete** `dom.values` — Namespace/attribute identity and UTF-16 values ([source](../../crates/rustic-dom/src/string.rs), [test](../../crates/rustic-dom/tests/tree.rs))
- **planned** `dom.nodekinds` — Fragments, processing instructions and full node kinds
- **planned** `dom.adoption` — Adoption, cloning and document lifecycle
- **planned** `dom.events` — DOM event propagation and default actions
- **planned** `dom.observers` — Mutation records and observers
- **planned** `dom.ranges` — Ranges, live collections and traversal APIs
- **planned** `dom.shadow` — Shadow DOM and custom element lifecycle
- **planned** `dom.tracing` — Tracing integration and DOM conformance gate

### HTML

- **complete** `html.input` — Bounded incremental UTF-8 decoding and input preprocessing ([source](../../crates/rustic-html/src/input.rs), [test](../../crates/rustic-html/tests/input.rs), [test](../../crates/rustic-html/tests/input.rs))
- **complete** `html.tags` — Declared data/tag/attribute tokenizer subset ([source](../../crates/rustic-html/src/tokenizer.rs), [test](../../crates/rustic-html/tests/tokenizer.rs), [test](../../crates/rustic-html/tests/tokenizer.rs), [test](../../crates/rustic-html/tests/tokenizer.rs))
- **complete** `html.references` — Full named/numeric character references ([source](../../crates/rustic-html/src/tokenizer/reference.rs), [test](../../crates/rustic-html/tests/references.rs), [test](../../crates/rustic-html/tests/references.rs), [test](../../crates/rustic-html/tests/corpus.rs))
- **complete** `html.declarations` — Comments, doctypes and declaration recovery ([source](../../crates/rustic-html/src/tokenizer/comment.rs), [source](../../crates/rustic-html/src/tokenizer/doctype.rs), [source](../../crates/rustic-html/src/tokenizer/instruction.rs), [test](../../crates/rustic-html/tests/declarations.rs), [test](../../crates/rustic-html/tests/corpus.rs))
- **planned** `html.textmodes` — RCDATA, RAWTEXT, script and plaintext modes
- **planned** `html.tree` — Core insertion modes and malformed-tree recovery
- **planned** `html.tables` — Table, formatting, template and foreign content rules
- **planned** `html.scripts` — Parser/script reentry and encoding restart
- **planned** `html.loader` — Bounded local loader and dump-dom command
- **planned** `html.conformance` — Parser corpora, chunk invariance and fuzz gate

### CSS & style

- **planned** `css.syntax` — CSS tokenizer/parser and recovery
- **planned** `css.selectors` — Required selectors and specificity
- **planned** `css.properties` — Typed property grammar and supported property catalog
- **planned** `css.cascade` — Complete cascade/origins/layers and inheritance
- **planned** `css.values` — Computed values, variables and cycle handling
- **planned** `css.queries` — Media/container queries and style attachment
- **planned** `css.invalidation` — Correct incremental invalidation against reference path
- **planned** `css.conformance` — Declared CSS conformance and regression gate

### Text & fonts

- **planned** `text.fonts` — Font selection/discovery and metrics
- **planned** `text.shaping` — Shaping, fallback and glyph clusters
- **planned** `text.bidi` — Bidi and mixed-direction text
- **planned** `text.breaking` — Line breaking and grapheme navigation
- **planned** `text.writing` — Writing modes, vertical text and font loading
- **planned** `text.conformance` — Text/font conformance and platform parity

### Layout

- **planned** `layout.boxes` — Box/fragment trees and checked geometry
- **planned** `layout.block` — Block flow, margins and anonymous boxes
- **planned** `layout.inline` — Inline formatting, line boxes and baselines
- **planned** `layout.sizing` — Intrinsic and replaced-element sizing
- **planned** `layout.position` — Floats, positioning and overflow
- **planned** `layout.flex` — Flexbox
- **planned** `layout.grid` — Grid
- **planned** `layout.tables` — Tables
- **planned** `layout.fragmentation` — Fragmentation, printing and advanced formatting
- **planned** `layout.incremental` — Incremental/full-layout parity and conformance

### Paint & raster

- **planned** `paint.display` — Immutable display lists and paint order
- **planned** `paint.stacking` — Stacking, clips and transforms
- **planned** `paint.raster` — Deterministic CPU reference rasterizer
- **planned** `paint.images` — Bounded images, color/alpha and resource lifecycle
- **planned** `paint.hit` — Hit testing and resize/zoom geometry
- **planned** `paint.formats` — SVG/MathML painting integration and Canvas 2D
- **planned** `paint.conformance` — Reftest output, damage and paint conformance

### JavaScript & GC

- **planned** `js.parser` — Lexer/parser, ASI, early errors and UTF-16
- **planned** `js.values` — Values, coercions, references and completions
- **planned** `js.objects` — Object methods, descriptors and prototypes
- **planned** `js.heap` — Rooted traced heap and host-object cycle collection
- **planned** `js.interpreter` — Original interpreter/bytecode and control flow
- **planned** `js.functions` — Functions, closures, call/construct and exceptions
- **planned** `js.builtins` — Core built-ins and collections
- **planned** `js.jobs` — Promises, jobs and realms
- **planned** `js.modules` — Modules and host loading hooks
- **planned** `js.advanced` — BigInt, RegExp, proxies, generators and async
- **planned** `js.buffers` — Buffers, typed arrays and memory semantics
- **planned** `js.intl` — Intl and required Unicode semantics
- **planned** `js.wasm` — JavaScript/WebAssembly identity, conversions and tracing integration
- **planned** `js.limits` — Cancellation, stack/heap limits and GC stress
- **planned** `js.conformance` — Declared full Test262 and runtime correctness gate

### WebAssembly

- **planned** `wasm.decode` — Bounded binary decoder and malformed-module handling
- **planned** `wasm.validate` — Complete selected type/instruction/module validation
- **planned** `wasm.execute` — Original interpreter, numerics, control flow and traps
- **planned** `wasm.instances` — Memories, tables, globals, instances and resource limits
- **planned** `wasm.web` — Streaming Web API, MIME/CSP and capability-safe imports
- **planned** `wasm.features` — Required reference/GC, exception, SIMD, shared-memory and baseline features
- **planned** `wasm.accelerate` — Reviewed acceleration with interpreter fallback on mobile/desktop
- **planned** `wasm.conformance` — Core/JS/Web suites, fuzzing and five-platform acceptance

### Bindings & web APIs

- **planned** `web.idl` — Web IDL tooling, conversions and rooted wrappers
- **planned** `web.window` — Window/Document/Element bindings and identity
- **planned** `web.scheduler` — Tasks, microtasks and rendering opportunities
- **planned** `web.scripts` — Classic/module script lifecycle and readiness
- **planned** `web.input` — Forms, focus, editing and user input
- **planned** `web.observers` — Timers, animation frames and observer APIs
- **planned** `web.streams` — Streams, messaging and structured clone
- **planned** `web.conformance` — Exposed web API conformance and reentry gate

### Network & navigation

- **planned** `network.transport` — Reviewed TLS/HTTP transport with cancellation
- **planned** `network.navigation` — Navigation/history state machine and redirects
- **planned** `network.fetch` — Fetch, CORS and response filtering
- **planned** `network.cookies` — Cookies and partitioned HTTP cache
- **planned** `network.policy` — CSP, MIME, secure contexts and mixed-content policy
- **planned** `network.protocols` — Required HTTP versions, WebSockets and referrer rules
- **planned** `network.conformance` — Network policy and navigation conformance gate

### Storage & workers

- **planned** `storage.profiles` — Ephemeral/persistent profiles and private mode
- **planned** `storage.partition` — Storage keys, quotas and schema recovery
- **planned** `storage.webstorage` — Local/session storage
- **planned** `storage.indexeddb` — IndexedDB transactions and crash recovery
- **planned** `storage.workers` — Worker agents, lifetimes and messaging
- **planned** `storage.offline` — Service workers and Cache API
- **planned** `storage.conformance` — Persistence/offline/worker isolation and conformance

### Security & IPC

- **planned** `security.ipc` — Bounded versioned hostile-peer-safe IPC
- **planned** `security.authority` — Browser-assigned authority and capability brokering
- **planned** `security.isolation` — Site/agent/process assignment and origin separation
- **planned** `security.desktop` — Verified Windows/macOS/Linux sandboxing
- **planned** `security.mobile` — Verified Android/iOS process and broker restrictions
- **planned** `security.permissions` — Trusted permissions, activation, files and downloads
- **planned** `security.recovery` — Renderer/service crash recovery without stale authority
- **planned** `security.brave` — Versioned Brave security comparison and independent review
- **planned** `security.privacy` — Default tracking/bounce/fingerprinting defenses and privacy matrix
- **planned** `security.updates` — Authenticated updates, incident response and vulnerability triage
- **planned** `security.audit` — Fuzz/native-memory review and security release gate

### Compositor & GPU

- **planned** `compositor.scene` — Incremental scene submission and scrolling
- **planned** `compositor.backend` — Mobile/desktop GPU adapters and CPU fallback
- **planned** `compositor.recovery` — Surface/device loss and resource recovery
- **planned** `compositor.parity` — GPU/reference parity, performance and energy gates

### Platform hosts

- **complete** `platform.crosschecks` — Android/iOS shared-core cross-compilation ([source](../../.github/workflows/ci.yml), [test](../../.github/workflows/ci.yml))
- **planned** `platform.linux` — Linux runtime and native host acceptance
- **planned** `platform.windows` — Windows runtime and native host acceptance
- **planned** `platform.macos` — macOS runtime and native host acceptance
- **planned** `platform.android` — Android runtime and native host acceptance
- **planned** `platform.ios` — iOS/iPadOS runtime and native host acceptance
- **planned** `platform.lifecycle` — Surface, touch/IME and background lifecycle parity
- **planned** `platform.native` — Native font/codec/OS service adapters
- **planned** `platform.device` — Linked artifacts, device/simulator suites and OS policy

### Accessibility

- **planned** `a11y.semantics` — Semantic roles, names and state tree
- **planned** `a11y.bridges` — Desktop and mobile accessibility bridges
- **planned** `a11y.editing` — Keyboard/IME, focus and assistive editing flows
- **planned** `a11y.acceptance` — Assistive-technology and accessibility acceptance

### DevTools

- **planned** `devtools.inspection` — DOM/style/layout inspection and console
- **planned** `devtools.debugger` — Source debugging, breakpoints and network diagnostics
- **planned** `devtools.automation` — Secure headless automation and profiling protocol
- **planned** `devtools.acceptance` — Debugger privacy, authority and diagnostic acceptance

### Extensions (MV2)

- **planned** `extensions.manifest` — Rustic-native Manifest V2 schema and API version contract
- **planned** `extensions.packages` — Package identity, signatures and hostile archive validation
- **planned** `extensions.worlds` — Isolated content-script worlds and extension principals
- **planned** `extensions.background` — MV2 background pages and mobile lifecycle
- **planned** `extensions.messaging` — Attributed bounded messaging and storage APIs
- **planned** `extensions.ui` — Actions, options, permissions and developer diagnostics
- **planned** `extensions.requests` — Blocking webRequest and reviewed response filtering
- **planned** `extensions.updates` — Signed updates, revocation and rollback protection
- **planned** `extensions.platforms` — Native desktop/Android/iOS extension acceptance
- **planned** `extensions.conformance` — Supported-API parity and hostile-extension security gate

### uBO port & filtering

- **planned** `filtering.inventory` — Pinned uBlock Origin port feature/license inventory
- **planned** `filtering.network` — Reference static/dynamic network rules and exceptions
- **planned** `filtering.cosmetic` — Cosmetic/procedural filters, packaged scriptlets and resources
- **planned** `filtering.broker` — Redirect/cache/worker/WebSocket and request-policy integration
- **planned** `filtering.port` — Rustic uBO port dashboard, logger, picker and advanced controls
- **planned** `filtering.lists` — Bounded transactional filter-list updates and rollback
- **planned** `filtering.optimize` — Rust acceleration/reference parity and measured budgets
- **planned** `filtering.acceptance` — Full declared uBO feature, mobile and security acceptance

### Embedding & release

- **planned** `release.embedding` — Versioned ownership/thread/cancellation embedding contract
- **planned** `release.compatibility` — Representative sites and complete declared conformance gates
- **planned** `release.performance` — Measured time, memory, reliability and energy budgets
- **planned** `release.artifacts` — Reproducible signed desktop/mobile artifacts and updates
- **planned** `release.comparison` — Controlled Chrome/Firefox speed, memory and energy comparison gates
- **planned** `release.complete` — Final full-engine acceptance across all five target platforms
