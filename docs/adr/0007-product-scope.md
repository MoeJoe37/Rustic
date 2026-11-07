# ADR 0007: Full-engine product scope and native extension ecosystem

- Date: 2026-11-07
- Status: Accepted scope update; implementation pending

## Context

The maintainer requires Brave-level security/privacy, broad website/CSS/JS
compatibility, measurable advantages over Chrome/Firefox, GPU acceleration,
WebAssembly, Rustic-only Manifest V2 extensions and a rebuilt uBlock Origin.
Earlier planning deferred extensions outside the engine program and treated Wasm
as an evaluation item. That scope no longer describes the requested product.

## Decision

Adopt [PRODUCT.md](../PRODUCT.md) as required target scope on all five platform
families. Promote GPU/Wasm and the native extension/filtering tracks to mandatory
delivery gates. Preserve the original engine/JS runtime rule. A separately
licensed Rustic uBO extension port is allowed; it does not replace engine code.
Specify a native Rustic package/API identity with MV2 capabilities, including
blocking requests, without promising Chrome/Firefox binary/store compatibility.

Build the CPU/interpreter reference paths before acceleration. Stage extension
integration after host/network policy and before final product acceptance; do not
delay the first static renderer. Expand the progress catalog to engine-v2 with
Wasm/extensions/filtering and comparative security/performance gates. Reweight
transparently; planning earns no implementation credit and removes no old gate.

## Alternatives and consequences

Keeping extensions/Wasm optional conflicts with the maintainer's requirements.
Embedding Chromium/Gecko/WebKit or another JS VM conflicts with original-engine
scope. Advertising universal site support or superiority without measurements
would misstate evidence. Rustic therefore records targets, specification baselines
and platform-specific acceptance rather than unsupported present-tense claims.

This adds substantial implementation, security, maintenance, licensing and mobile
lifecycle work. Preserve MV2 flexibility through brokered capabilities, bounded
callbacks and explicit revocation. Re-evaluate OS distribution/JIT/extension
constraints before shipping, while retaining original interpreter operation.

## Validation and revisit triggers

Require conformance/site reports, GPU/Wasm reference parity, signed-package and
hostile-extension tests, uBO feature/list-update parity, independent security
review and controlled comparison runs. Revisit API/package details before public
stability, major upstream uBO changes, new normative web baselines, or changes in
mobile isolation/distribution feasibility. Changing the product targets or
completion denominator requires an explicit documented scope decision.
