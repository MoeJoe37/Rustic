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

For a new record, use the next four-digit ID and include date, status, context,
decision, alternatives considered, consequences, validation and revisit trigger.
Changes to public API stability, native/unsafe boundaries, runtime replacement,
process authority, GC, or a major platform dependency require an ADR.
