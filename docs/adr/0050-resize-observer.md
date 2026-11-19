# ADR 0050 — ResizeObserver in rendering turns

Date: 2026-11-19
Status: Accepted for WEB-07 (second step of `web.observers`); implemented

## Context

The M5 roadmap asks for at least one rendering observer
(`ResizeObserver` or `IntersectionObserver`) integrated with rendering
turns. A rendering turn already runs animation frame callbacks and their
checkpoints before the host renders ([ADR 0036](0036-window-timers-and-animation-callbacks.md)),
and script layout reads share one revision-keyed snapshot and an
eight-flush budget per turn ([ADR 0042](0042-scripted-render-state.md)).

## Decision

1. `ResizeObserver` (`observe` with `box`, `unobserve`, `disconnect`),
   `ResizeObserverEntry` and `ResizeObserverSize` are exposed. An
   observation starts with a last reported size of −1, so the first
   rendering turn always reports.
2. After the animation frame callbacks of a rendering turn, the scripted
   document runs the Resize Observer steps of "update the rendering":
   gather observations whose size changed and whose target is deeper than
   the last broadcast's shallowest depth, broadcast them per observer in
   creation order (each callback gets `(entries, observer)`, errors are
   reported), and repeat. Skipped observations report
   "ResizeObserver loop completed with undelivered notifications." The turn
   then checkpoints and renders.
3. Sizes come from `PageTree::box_sizes`, which the engine serves from the
   layout snapshot's `LayoutGeometry::box_sizes` (content and border boxes
   and the padding offset). The CLI fills it from its block fragments.
   Reads share the turn's flush budget; elements without a box report zero.
4. The page holds an observer while it has observations, and an observer
   holds its targets.

## Consequences

- Observers stay alive until `unobserve` or `disconnect` even if script
  drops them; `devicePixelContentBoxSize` uses one device pixel per CSS
  pixel, sizes assume horizontal writing, inline boxes and SVG report
  zero, and `IntersectionObserver` is absent
  ([TD-155](../TECH_DEBT.md#td-155--resizeobserver-lifetime-and-geometry-approximations)).

## Validation

`crates/rustic-engine/tests/resize_observers.rs` checks first and changed
reports, box options, frozen size arrays, `unobserve`, `disconnect`,
argument errors and the loop error with a later delivery.

## Revisit

With `IntersectionObserver` and scrolling (M9), or when device-pixel
snapping and vertical writing modes reach layout.
