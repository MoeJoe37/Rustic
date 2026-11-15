# Engine progress snapshot

**Overall completion: 29.18%** = 27.80% verified complete + 1.38% partial credit. 65/221 capabilities complete, 6 in progress.

Catalog: `engine-v4`. Source fingerprint: `2b42fdcef3731918befbc8e1a9ffd1336d692df195962d1b9f9df7425871dd07`.

100% requires all capabilities and final five-platform engine acceptance. Weights and sizes measure planned engine scope; they are not compatibility results or time estimates. See [PROGRESS.md](../PROGRESS.md).

## Milestones

| Milestone | Capabilities complete | Progress | Verified | Share of engine |
| --- | --- | --- | --- | --- |
| M1 Document core | 28/28 | 100.00% | 100.00% | 7.28% |
| M2 CSS & computed style | 8/8 | 100.00% | 100.00% | 4.77% |
| M3 Static renderer | 17/17 | 100.00% | 100.00% | 7.74% |
| M4 ECMAScript runtime | 11/13 | 95.38% | 87.69% | 8.90% |
| M5 Scripted documents | 1/12 | 3.54% | 3.54% | 5.29% |
| M6 Networking & services | 0/12 | 0.00% | 0.00% | 4.38% |
| M7 Isolation & security | 0/8 | 0.00% | 0.00% | 3.72% |
| M8 Desktop & mobile alpha | 0/20 | 0.00% | 0.00% | 9.83% |
| M9 Compatibility expansion | 0/52 | 2.59% | 0.00% | 22.10% |
| M10 Embedding & beta release | 0/7 | 0.00% | 0.00% | 2.67% |
| M11 Web platform parity | 0/30 | 0.82% | 0.00% | 14.85% |
| M12 Performance leadership | 0/13 | 0.00% | 0.00% | 7.91% |
| M13 Full engine 1.0 | 0/1 | 0.00% | 0.00% | 0.50% |

## Subsystems

| Subsystem | Completed | In progress | Progress | Verified | Engine weight | Engine points |
| --- | --- | --- | --- | --- | --- | --- |
| Foundation | 8/8 | 0 | 100.00% | 100.00% | 1% | 1.00 |
| URLs & encoding | 6/12 | 0 | 43.33% | 43.33% | 2% | 0.86 |
| DOM | 6/10 | 1 | 56.61% | 52.94% | 4% | 2.26 |
| HTML | 9/11 | 0 | 70.27% | 70.27% | 5% | 3.51 |
| CSS & style | 8/10 | 2 | 84.18% | 79.59% | 6% | 5.05 |
| Text & fonts | 4/7 | 0 | 56.00% | 56.00% | 3% | 1.68 |
| Layout | 6/11 | 0 | 50.98% | 50.98% | 6% | 3.05 |
| Paint & raster | 6/7 | 0 | 70.37% | 70.37% | 4% | 2.81 |
| JavaScript & GC | 11/15 | 3 | 87.67% | 78.08% | 10% | 8.76 |
| WebAssembly | 0/8 | 0 | 0.00% | 0.00% | 3% | 0.00 |
| Bindings & web APIs | 0/10 | 0 | 0.00% | 0.00% | 5% | 0.00 |
| Media & real-time | 0/9 | 0 | 0.00% | 0.00% | 5% | 0.00 |
| Graphics web APIs | 0/6 | 0 | 0.00% | 0.00% | 4% | 0.00 |
| Devices & OS integration | 0/10 | 0 | 0.00% | 0.00% | 4% | 0.00 |
| Network & navigation | 0/8 | 0 | 0.00% | 0.00% | 4% | 0.00 |
| Storage & workers | 0/8 | 0 | 0.00% | 0.00% | 3% | 0.00 |
| Security & IPC | 0/11 | 0 | 0.00% | 0.00% | 5% | 0.00 |
| Compositor & GPU | 0/4 | 0 | 0.00% | 0.00% | 3% | 0.00 |
| Platform hosts | 1/9 | 0 | 4.16% | 4.16% | 4% | 0.16 |
| Accessibility | 0/4 | 0 | 0.00% | 0.00% | 2% | 0.00 |
| DevTools | 0/4 | 0 | 0.00% | 0.00% | 1% | 0.00 |
| Extensions (MV2) | 0/10 | 0 | 0.00% | 0.00% | 3% | 0.00 |
| uBO port & filtering | 0/8 | 0 | 0.00% | 0.00% | 2% | 0.00 |
| Performance leadership | 0/13 | 0 | 0.00% | 0.00% | 8% | 0.00 |
| Embedding & release | 0/8 | 0 | 0.00% | 0.00% | 3% | 0.00 |

## Capability evidence

Each line shows status, scope size (1, 2, 3, 5 or 8 points within its subsystem), roadmap milestone and share of the whole engine.

### Foundation

- **complete** `base.identity` — Document/node identity and stale-handle contracts · size 2 · M1 · 0.12% of engine ([source](../../crates/rustic-base/src/id.rs), [test](../../crates/rustic-dom/tests/tree.rs))
- **complete** `base.budgets` — Checked resource accounting · size 2 · M1 · 0.12% of engine ([source](../../crates/rustic-base/src/budget.rs), [test](../../crates/rustic-base/src/budget.rs))
- **complete** `base.cancellation` — Cooperative cancellation · size 2 · M1 · 0.12% of engine ([source](../../crates/rustic-base/src/cancellation.rs), [test](../../crates/rustic-base/src/cancellation.rs))
- **complete** `base.spans` — Checked source byte spans · size 1 · M1 · 0.06% of engine ([source](../../crates/rustic-base/src/source.rs), [test](../../crates/rustic-base/src/source.rs))
- **complete** `base.clocks` — Injectable monotonic clocks · size 1 · M1 · 0.06% of engine ([source](../../crates/rustic-base/src/clock.rs), [test](../../crates/rustic-base/src/clock.rs))
- **complete** `base.geometry` — Typed CSS/device geometry and checked surface allocation · size 3 · M3 · 0.18% of engine ([source](../../crates/rustic-base/src/geometry.rs), [source](../../crates/rustic-base/src/geometry.rs), [test](../../crates/rustic-base/tests/geometry.rs), [test](../../crates/rustic-base/tests/geometry.rs), [test](../../crates/rustic-layout/tests/reference.rs))
- **complete** `base.diagnostics` — Structured source/revision diagnostics and redaction · size 2 · M1 · 0.12% of engine ([source](../../crates/rustic-base/src/diagnostic.rs), [source](../../crates/rustic-base/src/revision.rs), [test](../../crates/rustic-base/src/diagnostic.rs), [test](../../crates/rustic-base/src/revision.rs))
- **complete** `base.lifecycle` — Coordinated lifecycle, shutdown and resource ownership · size 3 · M5 · 0.18% of engine ([source](../../crates/rustic-engine/src/lifecycle.rs), [test](../../crates/rustic-engine/tests/lifecycle.rs), [test](../../crates/rustic-cli/src/loader.rs))

### URLs & encoding

- **complete** `url.records` — Declared ASCII URL/reference parsing subset · size 3 · M1 · 0.20% of engine ([source](../../crates/rustic-url/src/parser.rs), [test](../../crates/rustic-url/tests/wpt.rs))
- **complete** `url.addresses` — IPv4/IPv6 host parsing and canonicalization · size 2 · M1 · 0.13% of engine ([source](../../crates/rustic-url/src/ipv6.rs), [test](../../crates/rustic-url/tests/url.rs))
- **complete** `url.origins` — Tuple and process-local opaque origin identity · size 2 · M1 · 0.13% of engine ([source](../../crates/rustic-url/src/origin.rs), [test](../../crates/rustic-url/tests/url.rs))
- **complete** `url.idna` — Unicode IDNA and data adapter · size 3 · M1 · 0.20% of engine ([source](../../crates/rustic-url/src/idna/mod.rs), [source](../../crates/rustic-url/src/host.rs), [test](../../crates/rustic-url/tests/idna.rs), [test](../../crates/rustic-url/tests/idna.rs))
- **complete** `url.file` — File URL and platform rules · size 2 · M1 · 0.13% of engine ([source](../../crates/rustic-url/src/parser.rs), [test](../../crates/rustic-url/tests/url.rs))
- **planned** `url.blob` — Blob registry and origin rules · size 2 · M9 · 0.13% of engine
- **planned** `url.setters` — URL setters and URLSearchParams · size 2 · M9 · 0.13% of engine
- **planned** `url.encoding` — Required legacy decoders and charset selection · size 5 · M6 · 0.33% of engine
- **planned** `url.restart` — Encoding sniff/restart integration · size 2 · M6 · 0.13% of engine
- **complete** `url.diagnostics` — Validation-error diagnostics and source locations · size 1 · M1 · 0.06% of engine ([source](../../crates/rustic-url/src/diagnostics.rs), [test](../../crates/rustic-url/tests/diagnostics.rs))
- **planned** `url.policy` — Navigation origin inheritance and policy integration · size 3 · M6 · 0.20% of engine
- **planned** `url.conformance` — Complete declared URL/encoding conformance gate · size 3 · M9 · 0.20% of engine

### DOM

- **complete** `dom.storage` — Bounded document-owned node storage · size 3 · M1 · 0.35% of engine ([source](../../crates/rustic-dom/src/storage.rs), [test](../../crates/rustic-dom/src/storage.rs))
- **complete** `dom.mutation` — Checked same-document mutation and traversal · size 3 · M1 · 0.35% of engine ([source](../../crates/rustic-dom/src/document.rs), [test](../../crates/rustic-dom/tests/tree.rs))
- **complete** `dom.values` — Namespace/attribute identity and UTF-16 values · size 2 · M1 · 0.23% of engine ([source](../../crates/rustic-dom/src/string.rs), [test](../../crates/rustic-dom/tests/tree.rs))
- **complete** `dom.nodekinds` — Fragments, processing instructions and full node kinds · size 2 · M1 · 0.23% of engine ([source](../../crates/rustic-dom/src/node.rs), [source](../../crates/rustic-dom/src/document.rs), [test](../../crates/rustic-dom/tests/node_kinds.rs), [test](../../crates/rustic-dom/tests/node_kinds.rs))
- **complete** `dom.adoption` — Adoption, cloning and document lifecycle · size 3 · M1 · 0.35% of engine ([source](../../crates/rustic-dom/src/document/adoption.rs), [source](../../crates/rustic-dom/src/document.rs), [test](../../crates/rustic-dom/tests/adoption.rs), [test](../../crates/rustic-dom/tests/adoption.rs), [test](../../crates/rustic-dom/tests/adoption.rs))
- **planned** `dom.events` — DOM event propagation and default actions · size 5 · M5 · 0.58% of engine
- **planned** `dom.observers` — Mutation records and observers · size 3 · M5 · 0.35% of engine
- **planned** `dom.ranges` — Ranges, live collections and traversal APIs · size 3 · M9 · 0.35% of engine
- **in progress 25.00%** `dom.shadow` — Shadow DOM and custom element lifecycle · size 5 · M9 · 0.58% of engine
  - [x] Shadow roots: attachment, ownership, destruction, dumps, adoption and tracing ([source](../../crates/rustic-dom/src/document.rs), [test](../../crates/rustic-dom/tests/shadow.rs))
  - [ ] Slot assignment, the flat tree and slotchange
  - [ ] Declarative shadow DOM in the HTML parser
  - [ ] Custom element registry, upgrades and lifecycle callbacks
- **complete** `dom.tracing` — Tracing integration and DOM conformance gate · size 5 · M1 · 0.58% of engine ([source](../../crates/rustic-dom/src/document/marking.rs), [test](../../crates/rustic-bindings/tests/dom_gc.rs), [test](../../crates/rustic-bindings/tests/dom_gc.rs), [source](../../crates/rustic-dom/src/document/invariants.rs), [test](../../crates/rustic-dom/tests/conformance.rs))

### HTML

- **complete** `html.input` — Bounded incremental UTF-8 decoding and input preprocessing · size 2 · M1 · 0.27% of engine ([source](../../crates/rustic-html/src/input.rs), [test](../../crates/rustic-html/tests/input.rs), [test](../../crates/rustic-html/tests/input.rs))
- **complete** `html.tags` — Declared data/tag/attribute tokenizer subset · size 3 · M1 · 0.40% of engine ([source](../../crates/rustic-html/src/tokenizer.rs), [test](../../crates/rustic-html/tests/tokenizer.rs), [test](../../crates/rustic-html/tests/tokenizer.rs), [test](../../crates/rustic-html/tests/tokenizer.rs))
- **complete** `html.references` — Full named/numeric character references · size 2 · M1 · 0.27% of engine ([source](../../crates/rustic-html/src/tokenizer/reference.rs), [test](../../crates/rustic-html/tests/references.rs), [test](../../crates/rustic-html/tests/references.rs), [test](../../crates/rustic-html/tests/corpus.rs))
- **complete** `html.declarations` — Comments, doctypes and declaration recovery · size 2 · M1 · 0.27% of engine ([source](../../crates/rustic-html/src/tokenizer/comment.rs), [source](../../crates/rustic-html/src/tokenizer/doctype.rs), [source](../../crates/rustic-html/src/tokenizer/instruction.rs), [test](../../crates/rustic-html/tests/declarations.rs), [test](../../crates/rustic-html/tests/corpus.rs))
- **complete** `html.textmodes` — RCDATA, RAWTEXT, script and plaintext modes · size 2 · M1 · 0.27% of engine ([source](../../crates/rustic-html/src/tokenizer/text.rs), [source](../../crates/rustic-html/src/tokenizer/driver.rs), [test](../../crates/rustic-html/tests/text_modes.rs), [test](../../crates/rustic-html/tests/text_modes.rs), [test](../../crates/rustic-html/tests/corpus.rs))
- **complete** `html.tree` — Core insertion modes and malformed-tree recovery · size 5 · M1 · 0.67% of engine ([source](../../crates/rustic-html/src/parser/tree.rs), [test](../../crates/rustic-html/tests/document.rs), [test](../../crates/rustic-html/tests/tree_construction.rs))
- **complete** `html.tables` — Table, formatting, template and foreign content rules · size 5 · M1 · 0.67% of engine ([source](../../crates/rustic-html/src/parser/tree/formatting.rs), [source](../../crates/rustic-html/src/parser/tree/table.rs), [source](../../crates/rustic-html/src/parser/tree/foreign.rs), [test](../../crates/rustic-html/tests/tree_rules.rs), [test](../../crates/rustic-html/tests/tree_construction.rs))
- **planned** `html.scripts` — Parser/script reentry and encoding restart · size 3 · M5 · 0.40% of engine
- **complete** `html.loader` — Bounded local loader and dump-dom command · size 2 · M1 · 0.27% of engine ([source](../../crates/rustic-cli/src/loader.rs), [test](../../crates/rustic-cli/tests/cli.rs))
- **complete** `html.conformance` — Parser corpora, chunk invariance and fuzz gate · size 3 · M1 · 0.40% of engine ([source](../../tools/import_html_tests.py), [test](../../crates/rustic-html/tests/corpus.rs), [source](../../crates/rustic-html/src/parser/fragment.rs), [test](../../crates/rustic-html/tests/tree_construction.rs), [test](../../crates/rustic-html/tests/fragment.rs), [source](../../crates/rustic-fuzz/src/targets.rs), [test](../../crates/rustic-fuzz/tests/harness.rs))
- **planned** `html.elements` — Complete element and form-control semantics: dialog, popover, details, picture/srcset, iframes and lazy loading · size 8 · M9 · 1.08% of engine

### CSS & style

- **complete** `css.syntax` — CSS tokenizer/parser and recovery · size 3 · M2 · 0.36% of engine ([source](../../crates/rustic-css/src/tokenizer.rs), [source](../../crates/rustic-css/src/rules.rs), [source](../../tools/import_css_tests.py), [test](../../crates/rustic-css/tests/corpus.rs), [test](../../crates/rustic-css/tests/syntax.rs))
- **complete** `css.selectors` — Required selectors and specificity · size 5 · M2 · 0.61% of engine ([source](../../crates/rustic-css/src/selector.rs), [test](../../crates/rustic-css/tests/selectors.rs), [source](../../crates/rustic-css/src/anb.rs), [test](../../crates/rustic-css/tests/an_plus_b.rs), [source](../../crates/rustic-style/src/matching.rs), [test](../../crates/rustic-style/tests/matching.rs), [source](../../crates/rustic-css/src/selector.rs), [test](../../crates/rustic-css/tests/selectors_level4.rs), [source](../../crates/rustic-style/src/html_state.rs), [test](../../crates/rustic-style/tests/selectors_level4.rs), [test](../../crates/rustic-style/tests/selectors_level4.rs), [test](../../crates/rustic-style/tests/selectors_level4.rs))
- **complete** `css.properties` — Typed property grammar and supported property catalog · size 8 · M2 · 0.97% of engine ([source](../../crates/rustic-css/src/color.rs), [test](../../crates/rustic-css/tests/color.rs), [source](../../crates/rustic-css/src/properties.rs), [test](../../crates/rustic-css/tests/properties.rs), [source](../../crates/rustic-css/src/calc.rs), [test](../../crates/rustic-css/tests/calc.rs), [source](../../crates/rustic-css/src/background.rs), [source](../../crates/rustic-css/src/properties.rs), [test](../../crates/rustic-css/tests/catalog.rs), [test](../../crates/rustic-style/tests/computed.rs))
- **complete** `css.cascade` — Complete cascade/origins/layers and inheritance · size 5 · M2 · 0.61% of engine ([source](../../crates/rustic-style/src/cascade.rs), [test](../../crates/rustic-style/tests/cascade.rs), [source](../../crates/rustic-style/src/cascade.rs), [test](../../crates/rustic-style/tests/cascade.rs), [source](../../crates/rustic-style/src/layer.rs), [source](../../crates/rustic-style/src/sheet.rs), [test](../../crates/rustic-style/tests/layers.rs), [test](../../crates/rustic-style/tests/layers.rs), [source](../../crates/rustic-style/src/cascade.rs), [source](../../crates/rustic-style/src/matching.rs), [test](../../crates/rustic-style/tests/shadow.rs), [test](../../crates/rustic-style/tests/layers.rs))
- **complete** `css.values` — Computed values, variables, registered properties and cycle handling · size 5 · M2 · 0.61% of engine ([source](../../crates/rustic-style/src/computed.rs), [test](../../crates/rustic-style/tests/computed.rs), [source](../../crates/rustic-css/src/calc.rs), [test](../../crates/rustic-style/tests/computed.rs), [source](../../crates/rustic-css/src/custom.rs), [test](../../crates/rustic-style/tests/variables.rs), [source](../../crates/rustic-style/src/variables.rs), [test](../../crates/rustic-style/tests/variables.rs), [source](../../crates/rustic-style/src/variables.rs), [test](../../crates/rustic-style/tests/registered.rs))
- **complete** `css.queries` — Media/container queries and style attachment · size 3 · M2 · 0.36% of engine ([source](../../crates/rustic-style/src/html.rs), [test](../../crates/rustic-style/tests/computed.rs), [source](../../crates/rustic-style/src/html.rs), [source](../../crates/rustic-style/src/loading.rs), [test](../../crates/rustic-style/tests/external.rs), [test](../../crates/rustic-cli/tests/cli.rs), [source](../../crates/rustic-css/src/media.rs), [test](../../crates/rustic-style/tests/cascade.rs), [source](../../crates/rustic-css/src/container.rs), [source](../../crates/rustic-style/src/containers.rs), [test](../../crates/rustic-style/tests/containers.rs), [source](../../crates/rustic-style/src/sheet.rs), [test](../../crates/rustic-style/tests/containers.rs))
- **complete** `css.invalidation` — Correct incremental invalidation against reference path · size 5 · M2 · 0.61% of engine ([source](../../crates/rustic-style/src/invalidation.rs), [test](../../crates/rustic-style/tests/invalidation.rs), [source](../../crates/rustic-style/src/cascade.rs), [test](../../crates/rustic-style/tests/invalidation.rs), [source](../../crates/rustic-style/src/variables.rs), [test](../../crates/rustic-style/tests/invalidation.rs), [source](../../crates/rustic-style/src/invalidation.rs), [test](../../crates/rustic-style/tests/containers.rs), [source](../../crates/rustic-style/src/invalidation.rs), [test](../../crates/rustic-style/tests/invalidation.rs), [test](../../crates/rustic-style/tests/invalidation.rs))
- **complete** `css.conformance` — Declared CSS conformance and regression gate · size 5 · M2 · 0.61% of engine ([source](../../tools/import_wpt_css_tests.py), [test](../../crates/rustic-css/tests/wpt_conformance.rs), [test](../../tools/tests/test_import_wpt_css_tests.py), [test](../../crates/rustic-css/tests/corpus.rs), [test](../../crates/rustic-fuzz/tests/harness.rs))
- **in progress 25.00%** `css.animations` — Transitions, CSS animations and Web Animations · size 5 · M9 · 0.61% of engine
  - [x] Timing-function longhands and deterministic CSS easing evaluation ([source](../../crates/rustic-css/src/easing.rs), [test](../../crates/rustic-css/tests/easing.rs), [test](../../crates/rustic-style/tests/cascade.rs))
  - [ ] CSS transition properties, interpolation, replacement and events
  - [ ] CSS keyframes, animation properties, playback and events
  - [ ] Web Animations timelines, keyframe effects and script API
- **in progress 20.00%** `css.modern` — Newer CSS shipped by other engines: anchor positioning, view transitions, scroll-driven animations and @scope · size 5 · M11 · 0.61% of engine
  - [x] Explicit stylesheet scopes, limits, relative selectors and cascade proximity ([source](../../crates/rustic-style/src/scope.rs), [test](../../crates/rustic-style/tests/cascade.rs), [test](../../crates/rustic-style/tests/invalidation.rs))
  - [ ] Implicit and nested scopes, scoped declarations and imports, and CSSOM
  - [ ] CSS anchor positioning
  - [ ] View transitions
  - [ ] Scroll-driven animations

### Text & fonts

- **complete** `text.fonts` — Font selection/discovery and metrics, including computed ex/ch/cap/ic values from loaded fonts · size 3 · M3 · 0.36% of engine ([source](../../crates/rustic-text/src/face.rs), [source](../../crates/rustic-text/src/glyf.rs), [test](../../crates/rustic-text/tests/fonts.rs), [test](../../crates/rustic-text/tests/fonts.rs), [test](../../crates/rustic-text/tests/fonts.rs), [source](../../crates/rustic-text/src/database.rs), [test](../../crates/rustic-text/tests/fonts.rs), [source](../../crates/rustic-style/src/fonts.rs), [test](../../crates/rustic-style/tests/fonts.rs), [test](../../crates/rustic-style/tests/fonts.rs), [source](../../crates/rustic-text/src/discovery.rs), [source](../../crates/rustic-text/src/containers.rs), [source](../../crates/rustic-text/src/cff.rs), [test](../../crates/rustic-text/tests/opentype.rs), [test](../../crates/rustic-text/tests/opentype.rs))
- **complete** `text.shaping` — Shaping, fallback and glyph clusters · size 5 · M3 · 0.60% of engine ([source](../../crates/rustic-text/src/shape.rs), [source](../../crates/rustic-text/src/shape.rs), [test](../../crates/rustic-text/tests/shape.rs), [test](../../crates/rustic-text/tests/shape.rs), [source](../../crates/rustic-text/src/shape.rs), [test](../../crates/rustic-text/tests/opentype.rs), [test](../../crates/rustic-text/tests/opentype.rs), [test](../../crates/rustic-text/tests/opentype.rs), [test](../../crates/rustic-text/tests/shaping_reference.rs))
- **complete** `text.bidi` — Bidi and mixed-direction text · size 3 · M3 · 0.36% of engine ([source](../../crates/rustic-text/src/bidi.rs), [test](../../crates/rustic-text/tests/bidi.rs), [test](../../crates/rustic-text/tests/bidi.rs), [source](../../crates/rustic-layout/src/lines.rs), [test](../../crates/rustic-layout/tests/inline.rs), [source](../../crates/rustic-text/src/bidi.rs), [source](../../crates/rustic-layout/src/inline.rs), [test](../../crates/rustic-text/tests/scopes.rs), [test](../../crates/rustic-layout/tests/text_policies.rs))
- **complete** `text.breaking` — Line breaking and grapheme navigation · size 3 · M3 · 0.36% of engine ([source](../../crates/rustic-text/src/segment.rs), [test](../../crates/rustic-text/tests/segment.rs), [source](../../crates/rustic-text/src/segment.rs), [test](../../crates/rustic-text/tests/segment.rs), [source](../../crates/rustic-layout/src/tailoring.rs), [test](../../crates/rustic-layout/tests/inline.rs), [test](../../crates/rustic-layout/tests/inline.rs), [source](../../crates/rustic-text/src/hyphen.rs), [test](../../crates/rustic-layout/tests/text_policies.rs), [test](../../crates/rustic-layout/tests/text_policies.rs), [test](../../crates/rustic-layout/tests/text_policies.rs), [test](../../crates/rustic-layout/tests/text_policies.rs))
- **planned** `text.writing` — Writing modes, vertical text and font loading · size 5 · M9 · 0.60% of engine
- **planned** `text.conformance` — Text/font conformance and platform parity · size 3 · M9 · 0.36% of engine
- **planned** `text.color` — Variable and color fonts (COLRv1, SVG-in-OpenType), palettes and the Font Loading API · size 3 · M9 · 0.36% of engine

### Layout

- **complete** `layout.boxes` — Box/fragment trees and checked geometry · size 3 · M3 · 0.35% of engine ([source](../../crates/rustic-layout/src/geometry.rs), [test](../../crates/rustic-layout/tests/reference.rs), [source](../../crates/rustic-layout/src/formatting.rs), [test](../../crates/rustic-layout/tests/formatting.rs), [test](../../crates/rustic-layout/tests/formatting.rs), [test](../../crates/rustic-cli/tests/cli.rs), [source](../../crates/rustic-layout/src/fragments.rs), [test](../../crates/rustic-layout/tests/fragments.rs), [test](../../crates/rustic-layout/tests/fragments.rs))
- **complete** `layout.block` — Block flow, margins and anonymous boxes · size 5 · M3 · 0.58% of engine ([source](../../crates/rustic-layout/src/sizing.rs), [test](../../crates/rustic-layout/tests/reference.rs), [source](../../crates/rustic-layout/src/flow.rs), [test](../../crates/rustic-layout/tests/flow.rs), [test](../../crates/rustic-layout/tests/flow.rs), [source](../../crates/rustic-layout/src/flow.rs), [test](../../crates/rustic-layout/tests/margins.rs), [test](../../crates/rustic-layout/tests/margins.rs), [test](../../crates/rustic-layout/tests/margins.rs), [source](../../crates/rustic-layout/src/flow.rs), [test](../../crates/rustic-layout/tests/inline.rs))
- **complete** `layout.inline` — Inline formatting, line boxes and baselines · size 5 · M3 · 0.58% of engine ([source](../../crates/rustic-layout/src/lines.rs), [source](../../crates/rustic-layout/src/inline.rs), [test](../../crates/rustic-layout/tests/inline.rs), [test](../../crates/rustic-layout/tests/inline.rs), [source](../../crates/rustic-layout/src/flow.rs), [source](../../crates/rustic-layout/src/text.rs), [test](../../crates/rustic-layout/tests/inline.rs), [test](../../crates/rustic-layout/tests/inline.rs), [source](../../crates/rustic-layout/src/inline.rs), [source](../../crates/rustic-layout/src/flow.rs), [test](../../crates/rustic-layout/tests/atomic.rs), [test](../../crates/rustic-layout/tests/atomic.rs))
- **complete** `layout.sizing` — Intrinsic and replaced-element sizing · size 3 · M3 · 0.35% of engine ([source](../../crates/rustic-layout/src/intrinsic.rs), [test](../../crates/rustic-layout/tests/sizing.rs), [test](../../crates/rustic-layout/tests/sizing.rs), [source](../../crates/rustic-layout/src/replaced.rs), [test](../../crates/rustic-layout/tests/sizing.rs), [test](../../crates/rustic-layout/tests/sizing.rs), [source](../../crates/rustic-layout/src/lines.rs), [test](../../crates/rustic-layout/tests/atomic.rs), [test](../../crates/rustic-layout/tests/atomic.rs))
- **complete** `layout.position` — Floats, positioning and overflow · size 5 · M3 · 0.58% of engine ([source](../../crates/rustic-layout/src/position.rs), [test](../../crates/rustic-layout/tests/position.rs), [test](../../crates/rustic-layout/tests/position.rs), [source](../../crates/rustic-layout/src/exclusions.rs), [test](../../crates/rustic-layout/tests/position.rs), [test](../../crates/rustic-layout/tests/position.rs), [source](../../crates/rustic-layout/src/overflow.rs), [test](../../crates/rustic-layout/tests/position.rs), [source](../../crates/rustic-layout/src/sticky.rs), [test](../../crates/rustic-layout/src/sticky.rs), [test](../../crates/rustic-layout/tests/position.rs))
- **complete** `layout.flex` — Flexbox · size 5 · M3 · 0.58% of engine ([source](../../crates/rustic-css/src/flex.rs), [test](../../crates/rustic-css/tests/flex.rs), [source](../../crates/rustic-layout/src/flow/flex.rs), [test](../../crates/rustic-layout/tests/flex.rs), [test](../../crates/rustic-layout/tests/flex.rs), [test](../../crates/rustic-layout/tests/flex.rs), [source](../../crates/rustic-layout/src/flow.rs), [test](../../crates/rustic-layout/tests/atomic.rs), [test](../../crates/rustic-layout/tests/atomic.rs), [test](../../crates/rustic-layout/tests/inline.rs), [source](../../tools/import_wpt_flexbox_tests.py), [test](../../crates/rustic-layout/tests/wpt_flexbox.rs))
- **planned** `layout.grid` — Grid · size 5 · M9 · 0.58% of engine
- **planned** `layout.tables` — Tables · size 5 · M9 · 0.58% of engine
- **planned** `layout.fragmentation` — Fragmentation, printing and advanced formatting · size 5 · M9 · 0.58% of engine
- **planned** `layout.incremental` — Incremental/full-layout parity and conformance · size 5 · M9 · 0.58% of engine
- **planned** `layout.formats` — SVG, MathML, ruby and multi-column layout · size 5 · M9 · 0.58% of engine

### Paint & raster

- **complete** `paint.display` — Immutable display lists and paint order · size 3 · M3 · 0.44% of engine ([source](../../crates/rustic-paint/src/display.rs), [test](../../crates/rustic-raster/tests/paint.rs), [test](../../crates/rustic-raster/tests/paint.rs), [test](../../crates/rustic-cli/tests/cli.rs), [source](../../crates/rustic-paint/src/display.rs), [test](../../crates/rustic-raster/tests/paint.rs), [test](../../crates/rustic-raster/tests/paint.rs), [test](../../crates/rustic-raster/tests/paint.rs), [source](../../crates/rustic-paint/src/display.rs), [test](../../crates/rustic-raster/tests/paint.rs), [source](../../crates/rustic-paint/src/display.rs), [test](../../crates/rustic-raster/tests/paint.rs), [test](../../crates/rustic-raster/tests/paint.rs))
- **complete** `paint.stacking` — Stacking, clips and transforms · size 3 · M3 · 0.44% of engine ([source](../../crates/rustic-paint/src/display.rs), [test](../../crates/rustic-raster/tests/paint.rs), [source](../../crates/rustic-raster/src/raster.rs), [test](../../crates/rustic-raster/tests/paint.rs), [source](../../crates/rustic-paint/src/clip.rs), [test](../../crates/rustic-raster/tests/paint.rs), [test](../../crates/rustic-raster/tests/paint.rs), [source](../../crates/rustic-paint/src/matrix.rs), [test](../../crates/rustic-raster/tests/paint.rs))
- **complete** `paint.raster` — Deterministic CPU reference rasterizer · size 5 · M3 · 0.74% of engine ([source](../../crates/rustic-raster/src/raster.rs), [test](../../crates/rustic-raster/src/raster.rs), [test](../../crates/rustic-raster/tests/paint.rs), [test](../../crates/rustic-raster/tests/paint.rs), [source](../../crates/rustic-raster/src/raster.rs), [source](../../crates/rustic-paint/src/display.rs), [test](../../crates/rustic-raster/src/raster.rs), [test](../../crates/rustic-raster/tests/paint.rs), [source](../../crates/rustic-raster/src/raster.rs), [source](../../crates/rustic-cli/src/images.rs), [test](../../crates/rustic-raster/tests/paint.rs), [test](../../crates/rustic-cli/tests/cli.rs), [source](../../crates/rustic-raster/src/raster.rs), [test](../../crates/rustic-raster/tests/paint.rs), [test](../../crates/rustic-raster/tests/paint.rs), [source](../../crates/rustic-raster/src/raster.rs), [test](../../crates/rustic-raster/tests/paint.rs), [test](../../crates/rustic-raster/tests/paint.rs), [source](../../crates/rustic-raster/src/damage.rs), [source](../../crates/rustic-raster/examples/frame_budget.rs), [test](../../crates/rustic-raster/tests/paint.rs), [test](../../crates/rustic-raster/tests/paint.rs))
- **complete** `paint.images` — Bounded images, color/alpha and resource lifecycle · size 3 · M3 · 0.44% of engine ([source](../../crates/rustic-paint/src/color.rs), [test](../../crates/rustic-paint/src/color.rs), [test](../../crates/rustic-raster/tests/paint.rs), [source](../../crates/rustic-codecs/src/png.rs), [source](../../crates/rustic-codecs/src/inflate.rs), [test](../../crates/rustic-codecs/tests/pngsuite.rs), [test](../../crates/rustic-codecs/src/inflate.rs), [source](../../crates/rustic-codecs/src/jpeg.rs), [source](../../crates/rustic-codecs/src/gif.rs), [source](../../crates/rustic-codecs/src/webp.rs), [test](../../crates/rustic-codecs/tests/images.rs), [test](../../crates/rustic-codecs/tests/images.rs), [test](../../crates/rustic-codecs/tests/images.rs), [test](../../crates/rustic-codecs/tests/images.rs), [source](../../crates/rustic-codecs/src/store.rs), [source](../../crates/rustic-cli/src/images.rs), [test](../../crates/rustic-codecs/src/store.rs), [test](../../crates/rustic-codecs/src/store.rs), [test](../../crates/rustic-codecs/tests/images.rs))
- **complete** `paint.hit` — Hit testing and resize/zoom geometry · size 2 · M3 · 0.29% of engine ([source](../../crates/rustic-paint/src/display.rs), [test](../../crates/rustic-raster/tests/paint.rs), [test](../../crates/rustic-cli/tests/cli.rs), [source](../../crates/rustic-layout/src/geometry.rs), [source](../../crates/rustic-cli/src/loader.rs), [test](../../crates/rustic-raster/tests/paint.rs), [test](../../crates/rustic-cli/tests/cli.rs))
- **planned** `paint.formats` — SVG and MathML painting integration · size 8 · M9 · 1.18% of engine
- **complete** `paint.conformance` — Reftest output, damage and paint conformance · size 3 · M3 · 0.44% of engine ([source](../../crates/rustic-raster/src/png.rs), [test](../../crates/rustic-raster/src/png.rs), [test](../../crates/rustic-cli/tests/cli.rs), [source](../../crates/rustic-cli/src/reftest.rs), [source](../../crates/rustic-raster/src/compare.rs), [test](../../crates/rustic-cli/tests/reftests.rs), [test](../../crates/rustic-cli/tests/cli.rs), [source](../../crates/rustic-raster/src/damage.rs), [source](../../crates/rustic-cli/src/loader.rs), [test](../../crates/rustic-raster/tests/paint.rs))

### JavaScript & GC

- **complete** `js.parser` — Lexer/parser, ASI, early errors and UTF-16 · size 5 · M4 · 0.68% of engine ([source](../../crates/rustic-js/src/lexer.rs), [test](../../crates/rustic-js/tests/lexer.rs), [source](../../crates/rustic-js/src/parser/mod.rs), [test](../../crates/rustic-js/tests/parse.rs), [source](../../crates/rustic-js/src/parser/scope.rs), [test](../../crates/rustic-js/tests/parse.rs), [source](../../crates/rustic-fuzz/src/js.rs), [test](../../crates/rustic-js/tests/parse.rs), [source](../../crates/rustic-js/src/test262.rs), [test](../../crates/rustic-js/tests/test262.rs), [source](../../crates/rustic-js/src/parser/expression.rs), [test](../../crates/rustic-js/tests/parse.rs))
- **complete** `js.values` — Values, coercions, references and completions · size 3 · M4 · 0.41% of engine ([source](../../crates/rustic-js/src/runtime/value.rs), [test](../../crates/rustic-js/tests/runtime.rs), [source](../../crates/rustic-js/src/runtime/convert.rs), [test](../../crates/rustic-js/tests/runtime.rs), [source](../../crates/rustic-js/src/runtime/env.rs), [test](../../crates/rustic-js/tests/runtime.rs), [source](../../crates/rustic-js/src/runtime/builtins/bigint.rs), [test](../../crates/rustic-js/tests/interp.rs))
- **complete** `js.objects` — Object methods, descriptors and prototypes · size 3 · M4 · 0.41% of engine ([source](../../crates/rustic-js/src/runtime/shape.rs), [test](../../crates/rustic-js/tests/runtime.rs), [source](../../crates/rustic-js/src/runtime/internal.rs), [test](../../crates/rustic-js/tests/runtime.rs), [source](../../crates/rustic-js/src/runtime/internal.rs), [test](../../crates/rustic-js/tests/runtime.rs), [source](../../crates/rustic-js/src/runtime/exotic.rs), [source](../../crates/rustic-js/src/runtime/builtins/proxy.rs), [source](../../crates/rustic-js/src/runtime/builtins/function.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/tests/interp.rs))
- **complete** `js.heap` — Rooted traced heap and host-object cycle collection · size 8 · M4 · 1.09% of engine ([source](../../crates/rustic-js/src/runtime/gc.rs), [test](../../crates/rustic-js/tests/gc.rs), [source](../../crates/rustic-js/src/runtime/shape.rs), [test](../../crates/rustic-js/tests/gc.rs), [source](../../crates/rustic-fuzz/src/js_heap.rs), [test](../../crates/rustic-js/tests/gc.rs), [source](../../crates/rustic-js/src/runtime/weak.rs), [test](../../crates/rustic-js/tests/gc.rs), [source](../../crates/rustic-bindings/src/heap.rs), [test](../../crates/rustic-bindings/tests/dom_gc.rs))
- **complete** `js.interpreter` — Original interpreter/bytecode and control flow · size 8 · M4 · 1.09% of engine ([source](../../crates/rustic-js/src/vm/compile.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/vm/interp.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/runtime/builtins.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/vm/interp.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/vm/interp.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-fuzz/src/js_run.rs), [test](../../crates/rustic-fuzz/tests/harness.rs), [source](../../crates/rustic-js/src/vm/compile.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/vm/compile.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/module/dynamic.rs), [test](../../crates/rustic-js/tests/module.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/tests/interp.rs))
- **complete** `js.functions` — Functions, closures, call/construct and exceptions · size 5 · M4 · 0.68% of engine ([source](../../crates/rustic-js/src/vm/interp.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/vm/interp.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/vm/interp.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/vm/interp.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/vm/compile.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/vm/compile.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/runtime/internal.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/vm/scope.rs), [source](../../crates/rustic-js/src/vm/interp.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/tests/interp.rs))
- **complete** `js.builtins` — Core built-ins and collections · size 8 · M4 · 1.09% of engine ([source](../../crates/rustic-js/src/runtime/builtins.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/tests/test262.rs))
- **complete** `js.jobs` — Promises, jobs and realms · size 3 · M4 · 0.41% of engine ([source](../../crates/rustic-js/src/runtime/builtins/promise.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/tests/gc.rs), [source](../../crates/rustic-js/src/runtime/builtins/promise.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/runtime/agent.rs), [source](../../crates/rustic-js/src/runtime/agent.rs), [test](../../crates/rustic-js/tests/realm.rs), [test](../../crates/rustic-js/tests/realm.rs), [test](../../crates/rustic-js/tests/realm.rs), [source](../../crates/rustic-js/src/test262.rs), [test](../../crates/rustic-js/tests/fixtures/test262_built_ins_passing.txt))
- **complete** `js.modules` — Modules and host loading hooks · size 3 · M4 · 0.41% of engine ([source](../../crates/rustic-js/src/module.rs), [source](../../crates/rustic-js/src/module/graph.rs), [test](../../crates/rustic-js/tests/module.rs), [test](../../crates/rustic-js/tests/module.rs), [source](../../crates/rustic-js/src/module/evaluate.rs), [source](../../crates/rustic-js/src/runtime/exotic.rs), [test](../../crates/rustic-js/tests/module.rs), [test](../../crates/rustic-js/tests/module.rs), [source](../../crates/rustic-js/src/module/evaluate.rs), [test](../../crates/rustic-js/tests/module.rs), [source](../../crates/rustic-js/src/module/evaluate.rs), [source](../../crates/rustic-js/src/module/dynamic.rs), [test](../../crates/rustic-js/tests/module.rs), [test](../../crates/rustic-js/tests/fixtures/test262_modules.txt))
- **complete** `js.advanced` — BigInt, RegExp, proxies, generators and async · size 8 · M4 · 1.09% of engine ([source](../../crates/rustic-js/src/vm/interp.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/tests/gc.rs), [source](../../crates/rustic-js/src/vm/interp.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/vm/interp.rs), [source](../../crates/rustic-js/src/runtime/builtins/async_generator.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/tests/gc.rs), [source](../../crates/rustic-js/src/runtime/bigint.rs), [source](../../crates/rustic-js/src/runtime/builtins/bigint.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/src/runtime/bigint.rs), [source](../../crates/rustic-js/src/regexp/vm.rs), [source](../../crates/rustic-js/src/runtime/builtins/regexp.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/tests/gc.rs), [source](../../crates/rustic-js/src/runtime/builtins/proxy.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/tests/interp.rs))
- **complete** `js.buffers` — Buffers, typed arrays and memory semantics · size 3 · M4 · 0.41% of engine ([source](../../crates/rustic-js/src/runtime/builtins/buffer.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/src/runtime/builtins/buffer.rs), [source](../../crates/rustic-js/src/runtime/builtins/typed_array.rs), [test](../../crates/rustic-js/tests/interp.rs), [source](../../crates/rustic-js/src/runtime/builtins/atomics.rs), [source](../../crates/rustic-js/src/runtime/builtins/buffer.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/tests/gc.rs))
- **in progress 50.00%** `js.intl` — Intl and required Unicode semantics · size 5 · M4 · 0.68% of engine
  - [x] Unicode normalization forms, shared data and exact UTF-16 handling ([source](../../crates/rustic-base/src/unicode/normalize.rs), [source](../../crates/rustic-js/src/runtime/builtins/string_object.rs), [test](../../crates/rustic-js/tests/interp.rs))
  - [x] Locale identifiers, Locale objects with preference data and locale-sensitive casing ([source](../../crates/rustic-js/src/runtime/builtins/intl/locale_object.rs), [test](../../crates/rustic-js/tests/interp.rs), [test](../../crates/rustic-js/tests/test262.rs))
  - [ ] Intl service locale negotiation and locale-sensitive string comparison
  - [ ] Intl services, locale data and Test262 Intl evidence
- **planned** `js.wasm` — JavaScript/WebAssembly identity, conversions and tracing integration · size 3 · M9 · 0.41% of engine
- **in progress 83.33%** `js.limits` — Cancellation, stack/heap limits and GC stress · size 3 · M4 · 0.41% of engine
  - [x] Frame, stack and heap limits, cancellation and the step budget ([source](../../crates/rustic-js/src/vm/interp.rs), [test](../../crates/rustic-js/tests/limits.rs), [test](../../crates/rustic-js/tests/limits.rs), [test](../../crates/rustic-js/tests/limits.rs))
  - [x] String ropes: appending is linear, with a late-charge bound (TD-116) ([source](../../crates/rustic-js/src/runtime/heap.rs), [test](../../crates/rustic-js/tests/limits.rs), [test](../../crates/rustic-js/tests/limits.rs))
  - [x] Benchmark corpus with deterministic gates: checksums, cache and GC-stress independence, no quadratic workload ([source](../../crates/rustic-js/benchmarks/framework_diff.js), [test](../../crates/rustic-js/tests/limits.rs), [test](../../crates/rustic-js/tests/limits.rs))
  - [x] Committed heap-size report ([source](../../crates/rustic-js/tests/fixtures/heap_report.txt), [test](../../crates/rustic-js/tests/limits.rs))
  - [x] Benchmark runner and recorded x86-64 results ([source](../../crates/rustic-js/examples/js_bench.rs), [test](../../crates/rustic-js/tests/limits.rs))
  - [ ] Recorded results on a native ARM64 machine
- **in progress 40.00%** `js.conformance` — Declared full Test262 and runtime correctness gate · size 5 · M9 · 0.68% of engine
  - [x] Test262 runner: metadata, strict and sloppy modes, negative tests, deterministic step and heap limits ([source](../../crates/rustic-js/src/test262.rs), [test](../../crates/rustic-js/src/test262.rs))
  - [x] Exact passing list for test/language at the pinned revision ([source](../../crates/rustic-js/src/test262.rs), [test](../../crates/rustic-js/tests/test262.rs))
  - [ ] test/built-ins in the gate
  - [ ] Module and async files, and the $262 host object
  - [ ] Declared full pass rate with every exception justified

### WebAssembly

- **planned** `wasm.decode` — Bounded binary decoder and malformed-module handling · size 2 · M9 · 0.17% of engine
- **planned** `wasm.validate` — Complete selected type/instruction/module validation · size 3 · M9 · 0.26% of engine
- **planned** `wasm.execute` — Original interpreter, numerics, control flow and traps · size 5 · M9 · 0.44% of engine
- **planned** `wasm.instances` — Memories, tables, globals, instances and resource limits · size 3 · M9 · 0.26% of engine
- **planned** `wasm.web` — Streaming Web API, MIME/CSP and capability-safe imports · size 3 · M9 · 0.26% of engine
- **planned** `wasm.features` — Required reference/GC, exception, SIMD, shared-memory and baseline features · size 8 · M9 · 0.70% of engine
- **planned** `wasm.accelerate` — Reviewed acceleration with interpreter fallback on mobile/desktop · size 5 · M9 · 0.44% of engine
- **planned** `wasm.conformance` — Core/JS/Web suites, fuzzing and five-platform acceptance · size 5 · M9 · 0.44% of engine

### Bindings & web APIs

- **planned** `web.idl` — Web IDL tooling, conversions and rooted wrappers · size 5 · M5 · 0.62% of engine
- **planned** `web.window` — Window/Document/Element bindings and identity · size 5 · M5 · 0.62% of engine
- **planned** `web.scheduler` — Tasks, microtasks and rendering opportunities · size 3 · M5 · 0.37% of engine
- **planned** `web.scripts` — Classic/module script lifecycle and readiness · size 3 · M5 · 0.37% of engine
- **planned** `web.input` — Forms, focus, editing and user input · size 5 · M5 · 0.62% of engine
- **planned** `web.observers` — Timers, animation frames and observer APIs · size 3 · M5 · 0.37% of engine
- **planned** `web.streams` — Streams, messaging and structured clone · size 5 · M9 · 0.62% of engine
- **planned** `web.conformance` — Exposed web API conformance and reentry gate · size 3 · M5 · 0.37% of engine
- **planned** `web.platform` — Remaining HTML/DOM APIs: Navigation API, drag and drop, selection, pointer events, fullscreen and file APIs · size 5 · M9 · 0.62% of engine
- **planned** `web.performance` — Performance timeline, Navigation/Resource Timing and Reporting APIs · size 3 · M11 · 0.37% of engine

### Media & real-time

- **planned** `media.pipeline` — Audio/video elements, demuxers, buffering and A/V sync · size 5 · M11 · 0.65% of engine
- **planned** `media.codecs` — Reviewed codecs (AV1, VP8/VP9, Opus, FLAC; H.264/HEVC/AAC via licensed platform decoders) and hardware decode · size 5 · M11 · 0.65% of engine
- **planned** `media.mse` — Media Source Extensions and adaptive streaming · size 3 · M11 · 0.39% of engine
- **planned** `media.eme` — Encrypted Media Extensions with Clear Key and a documented CDM decision · size 3 · M11 · 0.39% of engine
- **planned** `media.webaudio` — Web Audio graph, AudioWorklet and low-latency output · size 5 · M11 · 0.65% of engine
- **planned** `media.webcodecs` — WebCodecs and Media Capabilities · size 3 · M11 · 0.39% of engine
- **planned** `media.capture` — Camera, microphone and screen capture with trusted permission prompts · size 3 · M11 · 0.39% of engine
- **planned** `media.webrtc` — WebRTC peer connections, ICE/DTLS/SRTP and data channels · size 8 · M11 · 1.05% of engine
- **planned** `media.conformance` — Media/WebRTC WPT, codec corpus and five-platform playback acceptance · size 3 · M11 · 0.39% of engine

### Graphics web APIs

- **planned** `graphics.canvas` — Canvas 2D, OffscreenCanvas and ImageBitmap · size 5 · M9 · 0.66% of engine
- **planned** `graphics.webgl` — WebGL 1 and 2 with shader translation and validation · size 8 · M11 · 1.06% of engine
- **planned** `graphics.webgpu` — WebGPU and WGSL with validation in a sandboxed GPU process · size 8 · M11 · 1.06% of engine
- **planned** `graphics.color` — Color management, wide gamut and HDR output · size 3 · M11 · 0.40% of engine
- **planned** `graphics.images` — Further image formats: AVIF, APNG and reviewed HEIF/JPEG XL decisions · size 3 · M11 · 0.40% of engine
- **planned** `graphics.conformance` — Khronos WebGL CTS, WebGPU CTS and canvas WPT gate · size 3 · M11 · 0.40% of engine

### Devices & OS integration

- **planned** `devices.credentials` — WebAuthn/passkeys and Credential Management · size 5 · M11 · 0.66% of engine
- **planned** `devices.payments` — Payment Request and platform payment handlers · size 3 · M11 · 0.40% of engine
- **planned** `devices.notifications` — Notifications, Push API and Badging · size 3 · M11 · 0.40% of engine
- **planned** `devices.sensors` — Geolocation, device orientation/motion and sensor APIs · size 3 · M11 · 0.40% of engine
- **planned** `devices.input` — Gamepad, Pointer Lock, Keyboard Lock and Screen Wake Lock · size 3 · M11 · 0.40% of engine
- **planned** `devices.share` — Async Clipboard, Web Share and file handling · size 2 · M11 · 0.26% of engine
- **planned** `devices.hardware` — WebUSB/Bluetooth/HID/Serial/NFC with per-API privacy decisions · size 3 · M11 · 0.40% of engine
- **planned** `devices.speech` — Web Speech synthesis and recognition · size 2 · M11 · 0.26% of engine
- **planned** `devices.apps` — Installable web apps: manifest, install and window controls · size 3 · M11 · 0.40% of engine
- **planned** `devices.conformance` — Device API WPT, permission/privacy review and five-platform acceptance · size 3 · M11 · 0.40% of engine

### Network & navigation

- **planned** `network.transport` — Reviewed TLS/HTTP transport with cancellation · size 5 · M6 · 0.62% of engine
- **planned** `network.navigation` — Navigation/history state machine and redirects · size 5 · M6 · 0.62% of engine
- **planned** `network.fetch` — Fetch, CORS and response filtering · size 5 · M6 · 0.62% of engine
- **planned** `network.cookies` — Cookies and partitioned HTTP cache · size 3 · M6 · 0.37% of engine
- **planned** `network.policy` — CSP, MIME, secure contexts and mixed-content policy · size 3 · M6 · 0.37% of engine
- **planned** `network.protocols` — Required HTTP versions, WebSockets and referrer rules · size 5 · M9 · 0.62% of engine
- **planned** `network.conformance` — Network policy and navigation conformance gate · size 3 · M6 · 0.37% of engine
- **planned** `network.webtransport` — WebTransport, fetch priorities and Early Hints · size 3 · M11 · 0.37% of engine

### Storage & workers

- **planned** `storage.profiles` — Ephemeral/persistent profiles and private mode · size 2 · M6 · 0.24% of engine
- **planned** `storage.partition` — Storage keys, quotas and schema recovery · size 3 · M6 · 0.36% of engine
- **planned** `storage.webstorage` — Local/session storage · size 1 · M6 · 0.12% of engine
- **planned** `storage.indexeddb` — IndexedDB transactions and crash recovery · size 5 · M9 · 0.60% of engine
- **planned** `storage.workers` — Worker agents, lifetimes and messaging · size 3 · M9 · 0.36% of engine
- **planned** `storage.offline` — Service workers and Cache API · size 5 · M9 · 0.60% of engine
- **planned** `storage.conformance` — Persistence/offline/worker isolation and conformance · size 3 · M9 · 0.36% of engine
- **planned** `storage.filesystem` — Origin-private file system, File System Access and storage buckets · size 3 · M11 · 0.36% of engine

### Security & IPC

- **planned** `security.ipc` — Bounded versioned hostile-peer-safe IPC · size 3 · M7 · 0.34% of engine
- **planned** `security.authority` — Browser-assigned authority and capability brokering · size 3 · M7 · 0.34% of engine
- **planned** `security.isolation` — Site/agent/process assignment and origin separation · size 5 · M7 · 0.58% of engine
- **planned** `security.desktop` — Verified Windows/macOS/Linux sandboxing · size 5 · M7 · 0.58% of engine
- **planned** `security.mobile` — Verified Android/iOS process and broker restrictions · size 5 · M7 · 0.58% of engine
- **planned** `security.permissions` — Trusted permissions, activation, files and downloads · size 3 · M7 · 0.34% of engine
- **planned** `security.recovery` — Renderer/service crash recovery without stale authority · size 3 · M7 · 0.34% of engine
- **planned** `security.brave` — Versioned Brave security comparison and independent review · size 3 · M10 · 0.34% of engine
- **planned** `security.privacy` — Default tracking/bounce/fingerprinting defenses and privacy matrix · size 5 · M10 · 0.58% of engine
- **planned** `security.updates` — Authenticated updates, incident response and vulnerability triage · size 3 · M10 · 0.34% of engine
- **planned** `security.audit` — Fuzz/native-memory review and security release gate · size 5 · M7 · 0.58% of engine

### Compositor & GPU

- **planned** `compositor.scene` — Incremental scene submission and scrolling · size 3 · M8 · 0.69% of engine
- **planned** `compositor.backend` — Mobile/desktop GPU adapters and CPU fallback · size 5 · M8 · 1.15% of engine
- **planned** `compositor.recovery` — Surface/device loss and resource recovery · size 2 · M8 · 0.46% of engine
- **planned** `compositor.parity` — GPU/reference parity, performance and energy gates · size 3 · M8 · 0.69% of engine

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

- **planned** `extensions.manifest` — Rustic-native Manifest V2 schema and API version contract · size 2 · M9 · 0.18% of engine
- **planned** `extensions.packages` — Package identity, signatures and hostile archive validation · size 3 · M9 · 0.28% of engine
- **planned** `extensions.worlds` — Isolated content-script worlds and extension principals · size 5 · M9 · 0.46% of engine
- **planned** `extensions.background` — MV2 background pages and mobile lifecycle · size 3 · M9 · 0.28% of engine
- **planned** `extensions.messaging` — Attributed bounded messaging and storage APIs · size 3 · M9 · 0.28% of engine
- **planned** `extensions.ui` — Actions, options, permissions and developer diagnostics · size 3 · M9 · 0.28% of engine
- **planned** `extensions.requests` — Blocking webRequest and reviewed response filtering · size 5 · M9 · 0.46% of engine
- **planned** `extensions.updates` — Signed updates, revocation and rollback protection · size 2 · M9 · 0.18% of engine
- **planned** `extensions.platforms` — Native desktop/Android/iOS extension acceptance · size 3 · M9 · 0.28% of engine
- **planned** `extensions.conformance` — Supported-API parity and hostile-extension security gate · size 3 · M9 · 0.28% of engine

### uBO port & filtering

- **planned** `filtering.inventory` — Pinned uBlock Origin port feature/license inventory · size 1 · M9 · 0.07% of engine
- **planned** `filtering.network` — Reference static/dynamic network rules and exceptions · size 5 · M9 · 0.37% of engine
- **planned** `filtering.cosmetic` — Cosmetic/procedural filters, packaged scriptlets and resources · size 5 · M9 · 0.37% of engine
- **planned** `filtering.broker` — Redirect/cache/worker/WebSocket and request-policy integration · size 3 · M9 · 0.22% of engine
- **planned** `filtering.port` — Rustic uBO port dashboard, logger, picker and advanced controls · size 5 · M9 · 0.37% of engine
- **planned** `filtering.lists` — Bounded transactional filter-list updates and rollback · size 2 · M9 · 0.14% of engine
- **planned** `filtering.optimize` — Rust acceleration/reference parity and measured budgets · size 3 · M9 · 0.22% of engine
- **planned** `filtering.acceptance` — Full declared uBO feature, mobile and security acceptance · size 3 · M9 · 0.22% of engine

### Performance leadership

- **planned** `perf.harness` — Cross-engine benchmark harness, reference devices and published reports · size 3 · M5 · 0.38% of engine
- **planned** `perf.interpreter` — Interpreter fast paths: quickening, superinstructions and call inline caches · size 5 · M12 · 0.64% of engine
- **planned** `perf.baseline_jit` — Baseline JIT for x64 and ARM64 behind W^X with interpreter fallback · size 8 · M12 · 1.03% of engine
- **planned** `perf.optimizing_jit` — Optimizing JIT with speculation, deoptimization and on-stack replacement · size 8 · M12 · 1.03% of engine
- **planned** `perf.gc` — Generational, incremental and concurrent garbage collection · size 5 · M12 · 0.64% of engine
- **planned** `perf.style` — Parallel style resolution and selector-matching caches · size 5 · M12 · 0.64% of engine
- **planned** `perf.layout` — Incremental and parallel layout with layout caches · size 5 · M12 · 0.64% of engine
- **planned** `perf.raster` — GPU raster, off-main-thread compositing and threaded scrolling · size 5 · M12 · 0.64% of engine
- **planned** `perf.loading` — Loading pipeline: preload scanner, streaming parse, priorities, prerender and back/forward cache · size 5 · M12 · 0.64% of engine
- **planned** `perf.memory` — Memory leadership: compact DOM/style data, tab discarding and memory-pressure handling · size 3 · M12 · 0.38% of engine
- **planned** `perf.energy` — Energy: timer coalescing, background throttling and idle power · size 3 · M12 · 0.38% of engine
- **planned** `perf.startup` — Startup and new-tab time with snapshots and code caching · size 2 · M12 · 0.25% of engine
- **planned** `perf.gate` — Benchmark leadership gate against Blink, Gecko and WebKit on every reference device · size 5 · M12 · 0.64% of engine

### Embedding & release

- **planned** `release.embedding` — Versioned ownership/thread/cancellation embedding contract · size 3 · M10 · 0.30% of engine
- **planned** `release.compatibility` — Representative sites and complete declared conformance gates · size 5 · M10 · 0.50% of engine
- **planned** `release.performance` — Measured time, memory, reliability and energy budgets for the beta · size 3 · M10 · 0.30% of engine
- **planned** `release.artifacts` — Reproducible signed desktop/mobile artifacts and updates · size 3 · M10 · 0.30% of engine
- **planned** `release.comparison` — Published controlled Blink/Gecko/WebKit speed, memory and energy comparison reports · size 3 · M12 · 0.30% of engine
- **planned** `release.complete` — Final full-engine 1.0 acceptance across all five target platforms · size 5 · M13 · 0.50% of engine
- **planned** `release.inventory` — Feature parity inventory against Blink, Gecko and WebKit from pinned browser-compat-data · size 3 · M11 · 0.30% of engine
- **planned** `release.interop` — WPT parity gate: at least the best engine's pass rate on every required directory · size 5 · M11 · 0.50% of engine
