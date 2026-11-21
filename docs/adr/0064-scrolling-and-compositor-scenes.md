# ADR 0064 — Scrolling and compositor scenes

Date: 2026-11-21
Status: Accepted for scroll positions (step 1); scenes and incremental submission are the plan for the next steps

## Context

M8 (UI-05) asks for "incremental painting, scroll/compositor scenes". Until
now layout reported each scroll container's scrollable overflow but every
scroll position was zero, and every frame was a full display-list build and
rasterization. A browser shell scrolls constantly; laying out again for
every scroll step, or repainting the whole surface, would make the M8
frame-time gate unreachable. `rustic_raster::damage` and `repaint` already
repaint only the tiles whose pixels can change between two display lists.

## Decision

1. **Scroll positions are applied after layout**
   (`BlockLayout::scrolled`, `rustic-layout/src/scroll.rs`). Layout is
   computed once at position zero. A scroll moves contents by the negated
   position: each fragment's shift is its containing block's shift, minus
   that block's position when it is a scroll container, starting from the
   viewport's position at the initial containing block. This follows the
   containing-block chain, like clips, so an absolutely positioned box
   moves with its containing block rather than its parent. Fixed boxes do
   not move. Sticky boxes take `StickyConstraint::offset` at their scroll
   container's new position. Lines, inline boxes and glyph runs move with
   the block that holds them. Clips are recomputed over the moved padding
   boxes. Positions clamp to each scrollport's range (the scrollable
   overflow beyond the padding box, negative only where RTL overflow
   reaches left). The pass is one walk over the fragments and never lays
   out again; paint, raster and hit testing consume its result unchanged.
2. **CLI.** `dump-paint`, `render` and `hit-test` take `--scroll X,Y` and
   `--scroll-box N=X,Y`; frames and popups do not inherit them. Script
   layout reads still use the unscrolled layout, because script cannot
   scroll yet (step 4).
3. **Scenes (next).** A scene splits the display list at scroll containers
   and fixed or sticky stacking contexts into layers, each rasterized into
   its own cached surface. A scroll then re-composites cached layers at
   new offsets and rasterizes only newly exposed regions; content changes
   use the existing tile damage per layer. The CPU compositor is the
   reference the M8 GPU backend must match.
4. **Script scrolling (next).** `scrollX`/`scrollY`, `scrollTo`/`scrollBy`,
   `Element.scrollTop`/`scrollLeft` and `scroll` events feed the same
   positions.

## Consequences

Scroll is cheap and exact for the reference renderer, and later layers
reuse the same offsets. Applying a position to an already scrolled layout
is undefined: callers keep the unscrolled layout and derive each scrolled
one from it. No scrollbars are painted and no gutter is reserved (overlay
scrollbars), as before.

## Revisit

When scenes land, when script scrolls, and when the GPU compositor (UI-05)
takes over layer composition.
