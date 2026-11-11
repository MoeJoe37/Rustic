# ADR 0018: Feature parity with Blink, Gecko and WebKit, and performance leadership

- Date: 2026-11-11
- Status: Accepted scope update; implementation pending

## Context

The maintainer asked for the plan and the progress tracking to lead all the way
to a fully working engine that has all of the functionality of the other
browser engines and is better than any of them in performance. ADR 0007 already
required broad compatibility and "better than Chrome and Firefox", but the
roadmap stopped at M10 with M9 as an open-ended compatibility program. It had
no measurable definition of "all the functionality", left out WebKit, and had
no staged performance gates, so the progress chart could reach a high number
without the engine being comparable to the others.

## Decision

1. **Parity is measured against an inventory.** M11 ("Web platform parity")
   uses a pinned MDN browser-compat-data release to list every feature shipped
   by default in the stable releases of Chrome (Blink), Firefox (Gecko) or Safari
   (WebKit). Each feature maps to a catalog capability and is implemented,
   planned with a milestone, or declined with a reviewed reason. A feature
   shipped by all three can be declined only for a security or privacy reason
   recorded in an ADR. WPT, Test262, Wasm and graphics conformance must be at
   least the best engine's pass rate at the same upstream revision.
2. **Performance leadership is a gate, not a slogan.** M12 ("Performance
   leadership") requires Rustic to beat the best of the three engines by at
   least 10% on pinned Speedometer, JetStream and MotionMark scores, page-load
   time, memory and energy on named reference devices for every platform
   family, with non-overlapping 95% confidence intervals and no subtest more
   than 5% worse without a published exception. Intermediate ratchets at M8
   (2.0×) and M10 (1.25×) track the gap; measurement starts at M5 (PERF-01).
3. **M13 ("Full engine 1.0") is the final gate.** It takes over the full-product
   gate from M10, which becomes "Embedding & beta release". `release.complete`
   moves to M13 and `release.comparison` to M12.
4. **The progress catalog becomes `engine-v4`.** It adds the media, graphics web
   API, device/OS integration and performance subsystems and parity
   capabilities in existing subsystems (49 new capabilities, 221 total, 25
   subsystems), with reweighted subsystems; see [PROGRESS.md](../PROGRESS.md).
5. **Platforms stay explicit.** Every M11 API names its backend on Windows
   (x64, ARM64), macOS (x64, ARM64), Linux (x64, aarch64), Android and
   iOS/iPadOS, and every M12 gate is measured on reference devices for each of
   those families.

## Alternatives considered

- **Keep "better than Chrome and Firefox".** Rejected: WebKit is the third
  major engine and the only one on most iOS devices, so it must be compared.
- **Define parity as a WPT percentage only.** Rejected: WPT does not cover every
  shipped feature (media codecs, device APIs, DRM, installable apps), and a pass
  rate says nothing about features with no tests.
- **Add the new scope as M9 sub-items.** Rejected: M9 is already the largest
  milestone; separate milestones make the parity and leadership gates visible on
  the progress chart.

## Consequences

- The overall progress figure drops (36.99% to 23.51% at migration) because the
  denominator grows. No implementation was lost; PROGRESS.md records the
  reweighting.
- A JIT is required to win JetStream and Speedometer. Rustic forbids `unsafe`
  today, so PERF-03 starts with its own ADR confining executable-memory code to
  one small audited crate with W^X, interpreter fallback and differential
  fuzzing. Until then and where JIT is not allowed, the interpreter fast paths
  carry the targets.
- Some features depend on licenses or vendor eligibility (H.264/HEVC/AAC,
  Widevine-class CDMs, iOS engine entitlements). They are inventory entries with
  explicit decisions, never silent gaps.

## Validation

`tools/progress.py` requires the new subsystems and the `perf.gate`,
`release.inventory`, `release.interop`, `media.conformance`,
`graphics.conformance` and `devices.conformance` gates; its tests fail if any
is removed.

## Revisit trigger

A change to the comparison engines (a new major engine, or one of the three
being discontinued), a change in Apple's engine/JIT rules, or measured evidence
that a 10% target is unreachable on a device class without weakening security.
