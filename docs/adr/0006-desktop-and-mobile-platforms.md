# ADR 0006: One original engine across desktop and mobile

Date: 2026-11-07. Status: accepted following explicit maintainer direction.

## Context

The maintainer requires independence from Mozilla and other existing engines,
and Android/iOS alongside Windows/macOS/Linux. Earlier roadmap language treated
mobile as outside initial scope, risking desktop-only dependencies and assumptions.

## Decision

Make all five platforms architectural targets now. Exclude Mozilla/Firefox/Gecko,
SpiderMonkey, Servo, Chromium/Blink, WebKit and existing JS VMs as implementation
foundations. Share the original Rust document, scripting and rendering core.
Keep surfaces, input/IME, lifecycle, accessibility and process/service mechanisms
in OS adapters. No system WebView silently replaces the engine on mobile.

Add early library cross-checks for Android ARM64/x86-64 and iOS ARM64 device/
simulator. Prototype local mobile surfaces alongside M3 and integrate native
reference hosts by M8. Follow [MOBILE.md](../MOBILE.md) for platform gates. This
supersedes the earlier exclusion of mobile ports without weakening ADR 0004's
isolation gates for general browsing.

## Alternatives and consequences

A desktop-only core with mobile deferred may bake incompatible dependencies into
the design. A WebView/other-engine mobile implementation changes the project's
goal and is rejected. Complete hosts before a renderer would create empty
infrastructure; early cross-checks and staged prototypes provide concrete evidence.

Metadata checks do not prove linking, signing, lifecycle, rendering or device
behavior. Android/iOS do not automatically support desktop process/JIT mechanisms.
iOS alternative-engine capabilities/distribution depend on current platform rules
and entitlements; universal shipping availability is not promised.

## Validation and revisit

Require mobile core checks now. Add link/package, emulator/simulator and physical
device tests with actual adapters. Audit native dependencies for both mobile
systems. Revisit OS baselines and graphics/process choices with measured device
evidence and current platform requirements while preserving the original-engine goal.
