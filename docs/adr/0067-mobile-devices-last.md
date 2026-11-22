# ADR 0067 — Android and iOS device work moves to the final milestone (M14)

Date: 2026-11-22
Status: Accepted scope update (amends the schedule of ADR 0006, ADR 0013 and ADR 0018; the five release platforms are unchanged)

## Context

ADR 0006 made Android and iOS release platforms of the same engine and put
mobile hosts in M8 next to the desktop hosts. On 2026-11-22 M7 stood at
86% with only these items open:

- the Android isolation checks on a physical device (the emulator run is
  green in CI);
- iOS: BrowserEngineKit extensions on the simulator, then on a device with
  Apple's browser-engine entitlement, which needs Apple's approval;
- GPU process crash and restart, which needs the M8 GPU process;
- an independent external security review, which needs reviewers outside
  the project.

None of the four can be finished with the project's own CI and effort,
and two of them gated a milestone on mobile hardware and an Apple
decision. The maintainer asked to put Android and iOS support aside and
make it the final thing to do.

## Decision

1. **A new last milestone, M14 "Mobile devices & five-platform 1.0".** It
   holds the Android and iOS device work: `security.mobile` (its finished
   design and emulator steps keep their credit), `platform.android`,
   `platform.ios`, `platform.device` (signed mobile artifacts,
   device/simulator suites, store and OS policy), and the mobile halves of
   mixed capabilities, split out as `compositor.mobile`, `a11y.mobile`,
   `extensions.mobile` and `perf.mobile`. `release.mobile` is the final
   five-platform 1.0 acceptance and is a mandatory catalog gate, like
   `platform.android` and `platform.ios`.
2. **M8 becomes "Desktop alpha" and M13 "Desktop engine 1.0".** Their
   desktop capabilities keep their sizes; `compositor.backend`,
   `a11y.bridges` and `extensions.platforms` lose the mobile part of their
   scope to the new M14 capabilities, so subsystem totals are unchanged
   except `perf` (+2) and `release` (+3). MOB-03 to MOB-10 move to M14.
3. **M7 is recomputed without the deferred items.** The GPU process crash
   step moves to `compositor.recovery` (M8), where the GPU process is
   built. The independent external review moves to `security.brave`
   (M10, "independent external security review"), marked in the catalog
   as an **external dependency**: the review packet is ready, and the
   step completes when outside reviewers report. Remote browsing still
   ships behind the beta gate that requires it. With those moves M7 is
   complete for its declared scope.
4. **Deferred, not dropped.** Android and iOS stay release platforms. The
   mobile compile checks for all seven Android and iOS targets and the
   Android emulator isolation job stay required in CI, so the shared core
   keeps building and the Android renderer service keeps working while
   desktop work continues. Engineering rules in PLATFORMS.md still apply to
   every change. 100% overall still requires M14.

## Alternatives considered

- **Keep mobile in M8.** M8 would stall on hardware and an Apple decision
  the project cannot schedule.
- **Drop mobile from the catalog.** Contradicts ADR 0006/0013 and would
  inflate the percentage without any work.
- **Stop the mobile CI jobs too.** Saves CI minutes but lets the shared
  core drift from the mobile targets, which is the regression ADR 0013
  exists to prevent.
- **Count the external review as done.** Not true; it is recorded as an
  external dependency instead.

## Consequences

- The overall percentage moves a little (new mobile capabilities add scope
  to `perf` and `release`); no completed work loses credit.
- M8 is the current milestone and starts with the GPU process (UI-05) and
  the long-lived renderer (TD-182).
- TECH_DEBT items due on mobile hosts (TD-12's Android/iOS part) move to
  the M14 row of the payoff schedule.

## Validation

`tools/tests/test_progress.py` asserts that `platform.android`,
`platform.ios`, `security.mobile` and `release.mobile` sit in the last
milestone and that the three gates stay mandatory. The CI `Mobile core`
and `android-emulator` jobs remain required by the aggregate `CI` job.

## Revisit trigger

Desktop engine 1.0 (M13) passes, or Apple grants the browser-engine
entitlement, or a physical device or device farm becomes available
earlier: then the device steps may start before M13 without changing the
order of the gates.
