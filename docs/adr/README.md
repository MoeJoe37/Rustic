# Architecture decision records

An ADR records context, choice, alternatives, consequences, and the evidence that
would make us revisit it. Accepted here means the initial design baseline; it
does not mean the future subsystem is implemented. Proposed changes are reviewed
with the relevant task and tests. Supersede records with a new numbered ADR
instead of rewriting the history of a decision.

| Record | Decision | Status |
| --- | --- | --- |
| [0001](0001-original-engine.md) | Original engine/runtime with reviewed low-level libraries | Accepted baseline |
| [0002](0002-ownership-and-execution.md) | Agent-owned mutation, typed handles, traced JS/DOM graph | Accepted baseline; prototype required before bindings |
| [0003](0003-rendering-strategy.md) | Headless deterministic CPU reference before GPU | Accepted baseline |
| [0004](0004-isolation-rollout.md) | Local fixtures first; enforced isolation before arbitrary remote browsing | Accepted baseline; platform implementation pending |
| [0005](0005-tooling-and-compatibility.md) | Rust workspace, pinned tools, MSRV, incremental conformance and narrow CI | Accepted baseline |
| [0006](0006-desktop-and-mobile-platforms.md) | Original engine on desktop/Android/iOS with early mobile checks | Accepted scope update |
| [0007](0007-product-scope.md) | Required GPU/Wasm, Rustic MV2/uBO, security and competitive product targets | Accepted scope update; implementation pending |
| [0008](0008-tokenizer-drive.md) | Token-at-a-time input drive and explicit tree-builder text-mode feedback | Accepted internal contract; initial integration in 0009 |
| [0009](0009-parser-owned-construction.md) | Parser-owned inert construction and explicit unsupported-tree errors | Accepted; complete document construction in DOC-06b, script integration pending |
| [0010](0010-template-contents.md) | Template contents as separate parser-owned fragment roots | Accepted internal contract; general fragments in 0011 |
| [0011](0011-fragments-cloning-adoption.md) | General fragments, PI/CDATA kinds, cloning and cross-document adoption with ID maps | Accepted internal contract; bindings pending |
| [0012](0012-dom-tracing-prototype.md) | Per-tree DOM/host tracing prototype and the DOM pre-insert conformance gate | Accepted prototype; real heap with JS-03 |
| [0013](0013-any-capable-device.md) | Support any device that can run a browser, enforced by tiered CI | Accepted scope update |
| [0014](0014-reviewed-font-adapters.md) | Bounded HarfRust/Skrifa/Wuff adapters, exact dependency review and auditing | Accepted for M3 text |
| [0015](0015-js-value-and-memory-layout.md) | 8-byte NaN-boxed JS values, `u32`-indexed budgeted arenas, Latin-1 strings, shapes | Accepted for JS-02; collection with JS-03 |
| [0016](0016-js-garbage-collection.md) | Nonmoving mark/sweep at safe points, side mark bits, free lists, collected shapes and atoms, ephemerons, host-object tracing shared with the DOM | Accepted for JS-03 |
| [0017](0017-js-bytecode-interpreter.md) | Register bytecode compiled per function, frames on agent stacks instead of Rust recursion, safe points, shape-and-epoch inline caches | Accepted for JS-04 |
| [0018](0018-parity-and-performance-leadership.md) | Feature parity with Blink/Gecko/WebKit through a pinned inventory, performance leadership gates against all three, milestones M11–M13, `engine-v4` catalog | Accepted scope update; implementation pending |
| [0019](0019-static-module-evaluation.md) | Traced environments, import slots and namespaces for source text modules | Accepted for synchronous evaluation |
| [0020](0020-document-lifecycle.md) | Host-owned document lifetimes with bounded admission and coordinated cancellation | Accepted for the headless pipeline |
| [0021](0021-document-script-turns.md) | Document-owned script tasks and bounded microtask checkpoints | Accepted for the initial WEB-01 scheduler |
| [0022](0022-document-task-sources-and-timers.md) | Immediate and timed script sources, monotonic host clocks and timer cancellation | Accepted for WEB-01 host timers |
| [0023](0023-document-rendering-turns.md) | Coalesced rendering opportunities and document-local frame ordering | Accepted for WEB-01 rendering turns |
| [0024](0024-web-idl-conversions.md) | Rooted ECMAScript coercion and explicit Web IDL string and integer policies | Accepted for the initial WEB-02 conversion layer |
| [0025](0025-pinned-dom-idl-metadata.md) | Reproducible metadata from pinned DOM Standard IDL | Accepted for the WEB-02 text interfaces |
| [0026](0026-realm-dom-wrappers.md) | Realm-scoped wrapper identity, shared collection and adoption retargeting | Accepted for the WEB-02 host bindings |
| [0027](0027-host-exception-bindings.md) | Realm-owned interface prototypes and traced DOMException string slots | Accepted for WEB-02 |
| [0028](0028-scripted-document-collection.md) | Joint safe-point collection and live document bindings | Accepted for the first WEB-03 interfaces |
| [0029](0029-scripted-node-creation.md) | Reentrant conversions and transactional node factories and text splits | Accepted for WEB-03 |
| [0030](0030-dom-selectors-and-node-lists.md) | Scoped CSS queries and traced static and live NodeLists | Accepted for WEB-03 |
| [0031](0031-live-html-collections.md) | Live HTMLCollections, named property visibility, and direct-child indexing | Accepted for WEB-03 |
| [0032](0032-top-level-window-realm.md) | Top-level WindowProxy and realm global identity | Accepted for WEB-03 |
| [0033](0033-detached-documents-and-adoption.md) | Detached document ownership and transactional cross-document insertion | Accepted for WEB-03 |
| [0034](0034-dom-event-dispatch.md) | Scripted DOM event delivery and listener ownership | Accepted for WEB-04 |
| [0035](0035-trusted-input-and-activation.md) | Trusted host input and control activation | Accepted for WEB-04 |
| [0036](0036-window-timers-and-animation-callbacks.md) | Window timers and animation callbacks in document turns | Accepted for WEB-04 |
| [0037](0037-parser-script-boundary.md) | Parser suspension and script preparation order | Accepted for the WEB-05 parser boundary |
| [0038](0038-parser-inserted-inline-classics.md) | Inline classic execution, parser roots and adoption during construction | Accepted for the WEB-05 inline classic slice |
| [0039](0039-local-external-classics.md) | Host-supplied local sources and blocking, async and defer ordering | Accepted for the WEB-05 external classic slice |
| [0040](0040-local-module-scripts.md) | Local static module graphs, shared instances and evaluation order | Accepted for the WEB-05 local module slice |
| [0041](0041-scripted-document-readiness.md) | Document readiness, script events and CLI lifecycle | Accepted for the WEB-05 local host |
| [0042](0042-scripted-render-state.md) | Style, layout and frame validity in scripted documents | Accepted for WEB-06 |
| [0043](0043-encoding-standard.md) | Original Encoding Standard decoders and encoders over pinned, generated indexes | Accepted for `url.encoding` (M6 entry) |
| [0044](0044-local-html-encoding-replay.md) | Bounded local HTML encoding replay before author script | Accepted for WEB-05 |
| [0045](0045-live-computed-style-reads.md) | Live CSSOM style declarations, shared layout flush and resource revision | Accepted design for WEB-06; initial read-only slice implemented, gate pending |
| [0046](0046-parse-budget-degradation.md) | Parse budgets degrade: Chromium's depth cap and truncation at the node budget | Accepted for TD-02 (M6 entry) |
| [0047](0047-mutation-observers.md) | Mutation observers over a document journal, with registrations held as node edges | Accepted for WEB-07 (`dom.observers`) |
| [0048](0048-error-reporting-and-event-handlers.md) | Error reporting, rejection events, event handler attributes and a bounded console sink | Accepted for WEB-07 |
| [0049](0049-focus-and-forms.md) | Focus, text controls, form owners, validation and submission without navigation | Accepted for WEB-07 (`web.input`) |
| [0050](0050-resize-observer.md) | ResizeObserver broadcast in rendering turns from the layout snapshot | Accepted for WEB-07 (`web.observers`) |
| [0051](0051-transport-and-tls.md) | Own HTTP/1.1, loopback origin allowlist; rustls with `ring` and platform verification for TLS | Accepted |
| [0052](0052-network-io-model.md) | One thread per exchange, blocking sockets with short timeouts, byte-counted event window | Accepted for NET-01 |
| [0053](0053-loopback-fixture-server.md) | In-process loopback fixture server with scripted replies | Accepted for NET-01 and later M6 tests |
| [0054](0054-sites-and-cookie-policy.md) | Host-supplied Public Suffix List, sites, Lax-by-default cookies, blocked third-party cookies, CHIPS | Accepted |
| [0055](0055-downloads-broker-and-websocket-plan.md) | Downloads only through the broker with user decision and sanitized names; WebSocket over the allowlisted transport with Fetch policy | Accepted |
| [0056](0056-ipc-format.md) | Own bounded, versioned, canonical IPC over the child's standard streams; protocol 1 for broker and renderer | Accepted |
| [0057](0057-process-model-and-site-isolation.md) | Site-per-process keys, origin-keyed clusters, COOP group switches, ORB and COEP in the broker | Accepted |
| [0058](0058-linux-renderer-sandbox.md) | Linux renderer under bubblewrap and a seccomp allowlist: all namespaces, no files, sockets or network, on by default, fail closed, watchdog | Accepted |
| [0059](0059-mobile-renderer-isolation.md) | Android isolated-process renderer service and iOS BrowserEngineKit extensions over the same IPC | Proposed |
| [0060](0060-windows-renderer-sandbox.md) | Windows renderer in a capability-less LPAC AppContainer and a kill-on-close job; `rustic-sandbox` holds the first project `unsafe` | Accepted |
| [0061](0061-network-process.md) | Exchanges (sockets, TLS, HTTP framing, content codings) run in a `rustic __network` child; the broker keeps every decision and revalidates replies under credit-based flow control | Accepted |
| [0062](0062-out-of-process-frames.md) | Every `iframe` document renders in a renderer of its own with its own grant; the embedder sends black-painted pixels and per-frame coverage, and the broker composites the frame it never showed the embedder | Accepted |
| [0063](0063-macos-renderer-sandbox.md) | macOS renderers under a deny-by-default Seatbelt profile that each child applies to itself with `sandbox_init`; on by default since its native verification on macOS x64 and ARM64 | Accepted 2026-11-22 |
| [0064](0064-scrolling-and-compositor-scenes.md) | Scroll positions are applied to a finished layout along the containing-block chain (fixed stays, sticky re-solves, clips follow); scenes of cached layers and script scrolling build on them | Accepted (all four steps) |
| [0065](0065-desktop-window-host.md) | Desktop window host: winit 0.30 and softbuffer 0.4 in `rustic-shell`, minimal features; `rustic view` renders each frame in a sandboxed renderer | Accepted |
| [0066](0066-remote-addresses-and-windows-network-container.md) | Remote origins resolve through the system resolver in the broker and reach only public addresses (no DNS rebinding to loopback or private networks); the Windows network process runs in a `Rustic.Network` LPAC with internetClient | Accepted |
| [0067](0067-mobile-devices-last.md) | Android and iOS device work moves to the final milestone M14; M8 is the desktop alpha, M13 desktop 1.0; mobile CI checks stay; external review recorded as an external dependency | Accepted scope update |
| [0068](0068-gpu-process.md) | GPU process: frame compositing moves from the broker to a confined, stateless `rustic __gpu` child with crash/hang recovery and CPU fallback; hardware backends are a later record | Accepted (step 1) |
| [0069](0069-long-lived-renderer.md) | One long-lived renderer per page driven by input and frame requests over IPC, broker-owned time, user scrolls through the page, bounded renderer replacement; `rustic drive` headless driver | Accepted (renderer side; window adoption with UI-01) |
| [0070](0070-native-promise-continuations.md) | Traced native promise continuations and collection between jobs | Accepted for M9 `Array.fromAsync` |
| [0071](0071-gpu-backend.md) | GPU backend: wgpu 26 (Vulkan, Metal, Direct3D 12) in the GPU process; compute-shader compositing byte-identical to the CPU reference; device-loss reopen; Linux GPU sandbox profile | Accepted (desktop; window surfaces and Windows/macOS GPU profiles pending) |
| [0072](0072-automation-protocol.md) | Automation and DevTools protocol: `rustic automate` over stdio, no listening socket, broker-enforced grants, VM debugger hooks, redacted network diagnostics | Accepted |
| [0073](0073-accessibility.md) | Accessibility: tree built in the renderer (HTML-AAM, ARIA, AccName) and sent over IPC as a bounded wire message; AccessKit 0.25 bridges (UI Automation, NSAccessibility, AT-SPI) in the shell; editing, selection and IME composition in the engine | Accepted (desktop; real screen-reader acceptance external) |
| [0074](0074-browser-shell-and-native-hosts.md) | UI-01 browser shell (tabs, history, address entry, find, zoom, trusted strip) on live pages; host input, IME and lifecycle; system fonts per OS and collection members | Accepted |
| [0075](0075-web-text-encoding.md) | Window text encoding, Web IDL byte views and GC-owned native binding state | Accepted for M9 `web.encoding` |

## Records by area

| Area | Records |
| --- | --- |
| Scope, product and platforms | 0001, 0006, 0007, 0013, 0018, 0067 |
| Ownership, isolation and tooling | 0002, 0004, 0005 |
| HTML and DOM (M1) | 0008, 0009, 0010, 0011, 0012, 0046 |
| Rendering and text (M3) | 0003, 0014 |
| JavaScript engine (M4) | 0015, 0016, 0017, 0019 |
| Document lifecycle and scheduling (M5, WEB-01) | 0020, 0021, 0022, 0023 |
| Web IDL and wrappers (M5, WEB-02) | 0024, 0025, 0026, 0027 |
| Document, Window and node bindings (M5, WEB-03) | 0028, 0029, 0030, 0031, 0032, 0033 |
| Events, input and timers (M5, WEB-04) | 0034, 0035, 0036 |
| Script loading (M5, WEB-05) | 0037, 0038, 0039, 0040, 0041, 0044 |
| Scripted rendering (M5, WEB-06) | 0023, 0042, 0045 |
| Observers, errors, focus and forms (M5, WEB-07) | 0047, 0048, 0049, 0050 |
| Networking and resources (M6) | 0043, 0051, 0052, 0053, 0054, 0055 |
| Isolation and security (M7) | 0055, 0056, 0057, 0058, 0059, 0060, 0061, 0062, 0063, 0066 |
| Compositor and desktop host (M8) | 0064, 0065, 0068, 0069, 0071, 0074 |
| Developer tools and automation (M8) | 0072 |
| Accessibility and editing (M8) | 0073 |

[SCRIPTING.md](../SCRIPTING.md) summarizes how the M5 records fit together.

## Decisions still to be written

These boundaries have no record yet. Each must be accepted before the first
code of the task that needs it, because each one adds a dependency, an
authority boundary or `unsafe` code.

| Decision | Needed before | Why it needs a record |
| --- | --- | --- |
| Profile and storage format, partition keys and schema versions | The next change to the on-disk format (NET-06 shipped format version 1 without a record; its contract is in [IMPLEMENTATION.md](../IMPLEMENTATION.md#net-06--profiles-storage-keys-and-web-storage)) | Persistent on-disk compatibility promise |
| Android and iOS sandbox acceptance ([0059](0059-mobile-renderer-isolation.md) is Proposed; Android implemented and green on the emulator) | Accepting ISO-08/MOB-07 (M14, ADR 0067) | Platform security boundary verified only on a device, emulator or simulator |
| Mobile input and accessibility bridges; mobile windowing (desktop: [0065](0065-desktop-window-host.md), [0073](0073-accessibility.md)) | UI-02/03 (mobile in M14) | Platform dependencies on five OS families |
| WebAssembly execution tiers | WASM-01 (M9) | Runtime architecture shared with the JS heap |
| Confined `unsafe` code-memory crate for the JIT | PERF-03 (M12) | New `unsafe` outside `rustic-sandbox` |

For a new record, use the next four-digit ID and include date, status, context,
decision, alternatives considered, consequences, validation and revisit trigger.
Changes to public API stability, native/unsafe boundaries, runtime replacement,
process authority, GC, or a major platform dependency require an ADR.
