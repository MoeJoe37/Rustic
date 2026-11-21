# ADR 0064 — Scrolling and compositor scenes

Date: 2026-11-21
Status: Accepted (all four steps, 2026-11-21); remaining scene limits are TD-181

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
   without iframes, `render --frames` per frame. Top-level scroll
   containers (no scroll container above them) get layers of their own:
   the page is split in paint order into page segments and container
   contents (`LayerKind::Page` and `LayerKind::Scroller`); a container's
   contents are painted without its own and its ancestors' clips
   (`BlockLayout::without_clips_above`) and composited through its
   scrollport at its own snapped offset, so scrolling it copies cached
   tiles too. A container is layered only if its contents are one run in
   paint order, outside every group, without rounded clips; otherwise it
   paints into the page segment and scrolling it repaints its damage.
   Boxes are classified along the containing-block chain, so an absolute
   box positioned outside a container is not part of its layer. Nested
   containers stay inside their top-level container's layer (TD-181). The
   CPU scene is the reference the M8 GPU backend must match.
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
