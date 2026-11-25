# Full-engine product requirements

Accepted scope update: **2026-11-07**. These are required delivery targets for
Rustic's original engine and reference browser on Windows, macOS, Linux, Android
and iOS/iPadOS. [Implementation progress](IMPLEMENTATION.md) records delivered
behavior and the [progress summary](progress/summary.md) tracks the open scope.

| Requirement | Planned delivery | Acceptance evidence |
| --- | --- | --- |
| Functionality of Blink, Gecko and WebKit | M1–M9 standards pipeline, M11 parity program | Pinned parity inventory with every shipped feature mapped; WPT, Test262, Wasm and graphics conformance at least the best engine's; site-corpus workflows (see [ROADMAP §14](ROADMAP.md#14-m11--web-platform-parity)) |
| Security and privacy at least as strong as Brave | M6/M7 policy, isolation and privacy; M10 review | Versioned Brave comparison matrix, adversarial tests, per-platform sandbox verification, independent review and update exercises |
| Broad website support, complete CSS and JavaScript targets | M1–M9 standards pipeline and web APIs | Pinned WPT/Test262 baselines, complete feature inventory, representative site workflows and published unsupported cases |
| Better performance than Blink, Gecko and WebKit | Measurement from M5, ratchets at M8/M10, M12 leadership gate | Reproducible comparisons against Chrome, Firefox and Safari for responsiveness, loading, memory and energy; compatibility/security gates first |
| GPU acceleration | M3 CPU reference, M8 GPU integration, M9 parity | Hardware-accelerated composition/raster paths on all five platform families, device-loss recovery and CPU-reference parity |
| WebAssembly support | Original runtime and Window profile in M9, using M4/M5 foundations | Core, JS and Web API suites, proposal inventory, traps/limits, interpreter fallback and mobile execution |
| Rustic-only extensions using Manifest V2 | M9 desktop capability track, M14 mobile acceptance | Native Rustic package/identity, MV2 background pages/content scripts/blocking requests, permission and malicious-package tests |
| Rebuilt uBlock Origin for Rustic | M9 implementation and evidence, M13/M14 product acceptance | Maintained Rustic-specific uBO port, full blocking feature matrix, upstream comparison fixtures and filter-list lifecycle tests |

## Compatibility target

“All websites, CSS and JS” establishes a broad standards-complete direction, not
a permanent guarantee about every site or future proposal. Define a release's
dated HTML/DOM/CSS/ECMAScript/Intl/WebAssembly/Web API baseline and track every
required feature. Mature normative features required by that baseline are release
work, not permanently excluded because an early subset was convenient. Drafts,
deprecated quirks, codecs, DRM, hardware APIs and services with vendor-specific
eligibility need explicit inventory entries and decisions. Record unsupported
behavior and site failures publicly; do not label them supported.

The site corpus must exercise navigation, login/session persistence, forms,
editing, uploads/downloads, media, canvas/graphics, complex CSS, accessibility,
workers/offline and JS/Wasm applications. Use consented test accounts or local
fixtures, never real user credentials in CI. Include mobile layouts and touch/IME.
Publish browser/OS versions, feature flags, failures, skips and denominators.
Keep privacy-induced restrictions distinguishable from implementation defects,
with user-controlled site exceptions and regression tests for each workaround.

Required WPT/Test262/Wasm selections must have zero unexplained failures,
crashes or timeouts at release. Broader suite reports retain unsupported tests;
passing a small selection cannot stand in for complete standards coverage.
Review baseline additions each release and track new web features continuously.

## Brave-level security and privacy target

Brave is a comparison target, not an engine dependency. Review its current stable
desktop/mobile protections at each release; record exact versions, defaults,
platform differences and source dates. The comparison must cover:

1. Compromised-renderer containment, site/frame/worker assignment and brokered
   capabilities; verified desktop and mobile isolation.
2. Same-origin/CORS/CSP/MIME/mixed-content rules, TLS/certificate handling,
   secure contexts, powerful-feature permissions and trusted origin display.
3. Partitioned cookies/storage/cache, private profiles, bounce-tracking defenses
   and default tracker/ad blocking.
4. Fingerprinting surface inventory, tested anti-linkability measures, consistent
   per-site/session behavior, and controls that preserve documented web semantics.
5. Navigation/download/phishing protection with reviewed data sources, privacy
   accounting, false-positive recovery and authenticated updates.
6. Signed engine/extension/filter updates, rollback protection, vulnerability
   triage, incident response, dependency review and security-fix distribution.
7. Extension, Wasm, GPU/codec and filter-list attack surfaces; denial-of-service
   budgets, hostile-peer tests, fuzzing and native-memory review.

Every gap needs an owner, severity, evidence and closure criterion. Unresolved
critical/high-risk boundary failures block protected browsing. A privacy feature
checklist, a Rust implementation, or a benchmark score does not prove equal
security. Independent review and recorded attacks/recovery exercises are required
before a Brave-parity claim. Background network connections and telemetry must be
documented; collection is minimized and optional diagnostics require user consent.
See [security gates](SECURITY.md) and [extension/filtering design](EXTENSIONS.md).

## Better-than-every-engine target

Measure against the current stable Chrome (Blink), Firefox (Gecko) and Safari
(WebKit, or WebKitGTK/WPE where Safari does not run) on the same named
hardware/OS, versions, pages, profiles and feature settings. Publish two configurations:
normal product defaults, and equivalent content-blocking/protection settings.
Separate saved network work from engine execution speed. Do not compare a blocked
page with an unblocked page and attribute the difference to the rendering engine.

Final competitive targets (the M12 gate), compared with the **best** of the
three engines on each metric; the intermediate M8/M10 ratchet is in
[PERFORMANCE.md](PERFORMANCE.md#cross-engine-gates):

| Metric | Target against the best comparison engine |
| --- | --- |
| Speedometer, JetStream and MotionMark scores | At least 10% higher |
| Navigation/render and interaction workload aggregates | At least 10% lower geometric-mean time |
| Fixed multi-tab peak/resident memory | At least 10% less memory with equivalent retained state |
| Laptop and mobile sustained-workload energy | At least 10% less energy on comparable supported devices |
| Critical p95 responsiveness and correctness | No unexplained regression over 5%; no compatibility/security tradeoff hidden by the aggregate |

Use repeated trials, cold/warm runs, confidence intervals, thermal/power controls
and pinned workloads/benchmark revisions. Report regressions and unsupported
platform comparisons; iOS browser branding does not identify its underlying
engine. Run native Rustic tests on iOS regardless of a competitor's eligibility.
These are engineering targets, not measured results or a universal superiority
claim. Publish a claim only for the dimensions and configurations actually tested.
See [performance methodology](PERFORMANCE.md).

## Required accelerated graphics and Wasm

GPU acceleration is a production requirement, with an independently testable CPU
reference/fallback. Use native Metal, Vulkan/D3D or a reviewed portable adapter
only after platform evaluation. Validate scenes, shaders, shared buffers, color,
text/clip/transform parity, memory budgets, device loss and service crashes.
The desktop compositor is delivered in M8. Hardware acceptance spans the
desktop and mobile milestones. Canvas work is in M9. WebGL and WebGPU have
separate M11 conformance and security gates.

WebAssembly is mandatory, not a proposal to evaluate away. Build Rustic's original
bounded decoder, validator and interpreter, then JS/Web integration, reference/GC
types, exceptions, SIMD, memory/bulk operations and other selected baseline
features. Threads/shared memory require isolation policy and atomics work.
Machine-code compilation is staged behind W^X, bounds/trap and platform review;
the interpreter must remain usable without executable-memory permission.
Wasm imports receive explicit capabilities, never ambient files/sockets. WASI is
a separate opt-in embedding decision, not an implicit browser privilege.

Tasks **WASM-01–08**: binary/LEB128 decoder and malformed modules; type validation;
interpreter/numerics/control flow/traps; memories/tables/globals/instances;
JS identity/conversions/GC; streaming Web API/CSP/MIME; baseline proposals and
reviewed acceleration; official suite/fuzz/cancellation/mobile acceptance.
They depend on M4 values/heap/execution and M5 host policy, with isolation required
before untrusted remote execution. See [Core](https://webassembly.github.io/spec/core/),
[JS interface](https://webassembly.github.io/spec/js-api/) and
[Web API](https://webassembly.github.io/spec/web-api/).

## Milestones and completion accounting

Extension and filtering contracts start with M5/M6 services, not by delaying the
first static renderer. Desktop extension and filtering capabilities are tracked
in M9 after isolation, with mobile extension acceptance in M14.
M11 closes the feature gap with the other engines, M12 proves performance
leadership, M13 desktop acceptance includes all goals above on Windows, macOS and Linux,
and M14 (last, [ADR 0067](adr/0067-mobile-devices-last.md)) repeats it on Android and iOS devices, with
complete declared standards/site evidence, GPU and Wasm operation, Rustic MV2
extensions and the uBO port, security review and competitive measurements.

`engine-v2` expands the progress catalog to include dedicated Wasm, extension and
filtering subsystems plus security/privacy/comparison gates. Planning adds no
completion credit. Previous completed capabilities remain complete, while the
larger scope can lower the overall score. [PROGRESS.md](PROGRESS.md) records the
weight and denominator changes. [ADR 0007](adr/0007-product-scope.md) records this
scope decision. `engine-v4` adds the parity and performance-leadership scope
(media, graphics web APIs, device APIs and a performance subsystem, milestones
M11–M13) under [ADR 0018](adr/0018-parity-and-performance-leadership.md).

Reference review: 2026-11-07. Brave's [privacy inventory](https://brave.com/privacy-features/)
and [fingerprinting design](https://github.com/brave/brave-browser/wiki/Fingerprinting-Protections)
inform the comparison checklist; [Chromium site-isolation design](https://www.chromium.org/developers/design-documents/site-isolation/)
is an external threat-model reference. Rustic implements its own boundaries.
Apple's [EU](https://developer.apple.com/support/alternative-browser-engines/) and
[Japan](https://developer.apple.com/support/alternative-browser-engines-jp/)
engine eligibility and distribution requirements must be rechecked before mobile
release. Neither distribution approval nor unrestricted JIT is assumed.
