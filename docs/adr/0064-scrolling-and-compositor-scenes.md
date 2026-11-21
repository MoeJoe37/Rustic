# ADR 0064 — Scrolling and compositor scenes

Date: 2026-11-21
Status: Accepted for scroll positions (step 1), viewport scenes (step 2, first part), incremental frames (step 3) and script scrolling (step 4); scroll-container layers remain

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
   `--scroll-box N=X,Y`; frames and popups do not inherit them.
3. **Scenes** (viewport part done; maintainer decisions 2026-11-21).
   `rustic_raster::Scene` has a content layer, the page at unscrolled
   coordinates over its whole scrollable area, rasterized lazily in
   256-pixel tiles that stay cached, and an overlay layer of fixed and
   viewport-sticky boxes (and their contents) over a transparent canvas
   at window coordinates. A frame copies the visible tiles at the
   viewport offset and composites the overlay over them; a content change
   drops only the tiles `damage` reports. The viewport position snaps to
   whole device pixels. Layering needs every overlay command after every
   content command with no group, clip or transform spanning the
   boundary; otherwise the frame is painted in one pass. **The scene's
   output is the reference for scrolled pages and pages with an overlay**:
   compositing the overlay separately can differ from one-pass painting by
   rounding (at most 1/255 per channel), and an incremental scene frame
   equals a fresh scene byte for byte. `render` uses it for such pages
   without iframes, `render --frames` per frame. Scroll containers inside
   the page are still part of the content layer, so scrolling one repaints
   its damage (TD-181); giving them layers of their own completes this
   step. The CPU scene is the reference the M8 GPU backend must match.
4. **Incremental frames** (done, catalog step 3). `rustic_raster::Frames` keeps
   the last display list and surface; each submitted list repaints only
   the tiles `damage` reports, so every frame equals a full rasterization
   of its list. `render --frames N` (with `--run-scripts`) steps the manual
   clock by 1/60 s per frame, runs the tasks that are due, offers a
   rendering opportunity, paints the DOM incrementally and writes
   `NAME-K.EXT` per frame with a repaint line per frame on standard
   output. A scroll moves every scrolled fill, so it still repaints what
   moved; caching scrolled content is what layer scenes add. Frames paint
   iframes white (TD-178).
5. **Script scrolling** (done, catalog step 4). The engine keeps the document's scroll
   positions (`ScrollState`); `window.scrollX`/`scrollY`/`pageXOffset`/
   `pageYOffset`, `scroll()`/`scrollTo()`/`scrollBy()` (numbers or
   `ScrollToOptions`) and `Element.scrollTop`/`scrollLeft`/`scroll()`/
   `scrollTo()`/`scrollBy()` read and set them, clamped to the layout's
   ranges (a layout read); the document element stands for the viewport
   in no-quirks mode. A change queues one `scroll` event per target for
   the next rendering opportunity, before animation frame callbacks: at
   the document (bubbling) for the viewport, at the element otherwise.
   `getBoundingClientRect` subtracts the positions on the element's
   containing-block chain. The CLI paints where scripts left the page;
   `--scroll` overrides the viewport and `--scroll-box` containers.

## Consequences

Scroll is cheap and exact for the reference renderer, and later layers
reuse the same offsets. Applying a position to an already scrolled layout
is undefined: callers keep the unscrolled layout and derive each scrolled
one from it. No scrollbars are painted and no gutter is reserved (overlay
scrollbars), as before.

## Revisit

When scenes land, when script scrolls, and when the GPU compositor (UI-05)
takes over layer composition.
