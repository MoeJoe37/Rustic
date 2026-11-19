# M5 scripted documents completion review — 2026-11-19

## Starting revision

Reviewed `feature` from `a2dc45c` (CSSOM box edges, TD-148 and TD-149
recorded) through the WEB-07 commits of 2026-11-19: mutation observers
(`4625da1`), error reporting and the `Node` members (`0184bb0`), focus and
forms (`f8aa0ce`) and `ResizeObserver` (`3d04368`), plus this change
(PERF-01 report, reentry and teardown gate, debt rescheduling).

## Delivered behavior

| Task | Result | Records |
| --- | --- | --- |
| WEB-01 scheduler, checkpoints, frames | Done before this review | ADR 0020–0023 |
| WEB-02 Web IDL and wrappers | Done before this review | ADR 0024–0027 |
| WEB-03 Window, Document, nodes, selectors | Done before this review; `Node` gained names, values, relatives, containment, `compareDocumentPosition`, `replaceChild` and `normalize` | ADR 0028–0033 |
| WEB-04 events, input, timers, frames | Done before this review; handler IDL attributes added | ADR 0034–0036, 0048 |
| WEB-05 classic and module scripts, readiness | Done before this review; a checkpoint now precedes each parser-inserted classic | ADR 0037–0041, 0044, 0047 |
| WEB-06 invalidation and layout reads | Done for the M5 scope: geometry, live computed style and resource completions share one snapshot; pseudo-elements and asynchronous loading are rescheduled (TD-149, TD-134) | ADR 0042, 0045 |
| WEB-07 observers, errors, focus, forms | Done: `MutationObserver`, HTML error reporting and rejection events, the console sink, focus and Tab navigation, text editing, form owners (TD-31 paid), constraint validation, submission without navigation, `ResizeObserver` | ADR 0047–0050 |
| WEB-09 conformance gate | 59 pinned Window WPT files, 342 subtests, 272 passes, 70 expected failures with diagnoses | [SCRIPTING.md](../SCRIPTING.md#pinned-window-web-platform-tests) |
| PERF-01 (M5 slice) | First report: JS-09 corpus against V8, SpiderMonkey and JavaScriptCore; DOM microbenchmarks | [Report](2026-11-19-perf-01-m5.md) |

## Gate

**Selected event and task ordering tests pass.** Turn and checkpoint order
(`rustic-engine/tests/scripting.rs`, `timers.rs`, `frames.rs`), mutation
records against promise jobs and parser scripts
(`rustic-bindings/tests/mutation_observer.rs`,
`rustic-engine/tests/mutation_observers.rs`), error and rejection
notification order (`errors.rs`, `error_reporting.rs`), focus event order
(`forms.rs`) and ResizeObserver delivery in rendering turns
(`resize_observers.rs`), plus the upstream ordering files in the WPT
selection.

**Getters and callbacks reenter without borrow failures.** Earlier units
test reentrant conversions for DOMRect, `textContent`, attributes and
selectors (`rustic-bindings/tests/page.rs`). `m5_gate.rs` adds the WEB-07
surfaces: a dictionary getter that edits the tree during `observe()`, an
observer callback that mutates again, focus handlers that move focus while
focus is changing, an `unhandledrejection` handler that rejects again, an
`invalid` handler that revalidates and a `ResizeObserver` that disconnects
itself. Every binding releases its borrows before calling script; journal
draining skips while a borrow is held and the outer operation drains.

**Detached DOM cycles are collected.** `rustic-bindings/tests/dom_gc.rs`
covers wrapper, listener and live collection cycles. Observer registrations
are node-held heap edges and are collected with their nodes
(`observers_survive_collection_through_their_nodes_and_follow_adoption`
runs under GC stress). Two retention cases are recorded rather than hidden:
dead observer records (TD-150) and observing ResizeObservers (TD-155).

**Close cancels pending work.** `Engine::shutdown` cancels documents and
their queues (`lifecycle.rs`, `scripting.rs`); `m5_gate.rs` checks that a
queued rejection notification and pending mutation delivery never run after
shutdown and that no submission or console message appears. Navigation does
not exist yet (TD-130, M6).

**API exposure follows implemented bindings.** Missing interfaces stay
absent (`typeof window.fetch === "undefined"`); the WPT expected failures
name each missing API (`classList`, `Attr`, Range, `DOMParser`,
`innerHTML`, `location`, `onmessage`).

## Exit checklist

- [x] Five demo fixtures pass as scripted reftests at device scales one and
      two: counter, timer, event bubbling, style and tree mutation, and form
      validation (`rustic-cli/tests/reftests.rs`).
- [x] `web.scripts`, `html.scripts`, `dom.observers`, `web.input`,
      `web.observers` and `web.conformance` are complete in the catalog.
- [x] WEB-09 expectations, revision and counts are in SCRIPTING.md and the
      evidence table.
- [x] The first PERF-01 report is committed.
- [x] Fuzz targets for scripted mutation and the event/timer pump exist
      (unchanged).
- [ ] **Tiers.** Native x86-64 passes the full gate (formatting, Clippy,
      all-target tests, doctests, rustdoc, repository and tool tests,
      progress check, pinned WPT). This sandbox has no QEMU and no 1.85
      toolchain (the pinned toolchain directory is not modified), so the
      emulated 32-bit ARM, 32-bit x86, RISC-V and s390x runs, MSRV and the
      cross-checks were **not run** for these commits; hosted CI is still
      blocked (TD-12). Run them before merging to `main`.
- [x] M5-triggered debt: TD-31 paid; TD-131 and TD-132 partly paid and moved
      to M9; TD-134 and TD-149 moved to M6 entry; TD-125 to M8; TD-126 and
      TD-129 to M9; TD-118, TD-130 and TD-133 already had later triggers.
      Each entry records the reason.
- [x] This review, the roadmap status, docs/README and the root README are
      updated in the same change.

## Scope decisions to confirm

1. **`perf.harness` moves from M5 to M8.** The roadmap already said the
   capability "stays open until M8; M5 needs only the first report". The
   catalog listed it under M5, so M5 could never reach 100%. It is now
   in progress under M8 with the first report as its completed step and
   reference devices on five platform families as the open step. The
   denominator is unchanged.
2. **MOB-03/04 become the first task of M8 mobile hosts.** Hosted CI is
   still blocked and this machine has neither the Android NDK nor Xcode, as
   the roadmap anticipated.
3. **Pinned capabilities closed for their declared scope** with recorded
   debt: `web.input` (TD-154), `web.observers` (TD-155, no
   `IntersectionObserver`), `dom.observers` (TD-150, TD-151).

## Next

M6 entry: the transport/TLS ADR (NET-01), the JavaScript fixes due at M6
entry (TD-137 to TD-140), and the rescheduled TD-134, TD-149 and TD-152.
