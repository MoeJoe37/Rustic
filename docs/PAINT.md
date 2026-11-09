# M3 paint and raster reference

REN-04a/REN-05a turn the block-only layout reference into pixels. Two crates
follow the architecture split: `rustic-paint` builds an immutable display list,
and `rustic-raster` interprets it on the CPU and writes PNG. Neither uses a GPU,
a platform graphics API, `unsafe` code or a third-party crate.

## Display list (`rustic-paint`)

`DisplayList::build(document, boxes, layout, viewport, limits, cancellation)`
reads a `BlockLayout` and its principal `BoxTree`; it never touches live DOM
beyond checking element names.

- **Canvas.** The canvas starts opaque white. The root element's background
  color is painted across the whole canvas. When an HTML root's background is
  transparent with no image, its first `body` child that generated a box
  supplies the canvas color instead (CSS Backgrounds 3 §2.11.2). The box whose
  background went to the canvas paints no background of its own.
- **Order (REN-04b).** CSS 2.2 Appendix E for boxes without inline content.
  The root, positioned boxes with an integer `z-index`, fixed boxes and boxes
  with `opacity < 1` form stacking contexts. Each context paints its own
  background and border, then child contexts with negative `z-index`, in-flow
  blocks, floats, positioned boxes with `z-index: auto` or 0 and `z-index: 0`
  contexts (in tree order), then positive `z-index`. Equal values keep tree
  order. Floats and `z-index: auto` positioned boxes paint atomically, but
  their positioned descendants and real contexts join the enclosing context.
  Membership follows the box tree; the list is built with an explicit stack.
- **Clips.** Every item carries its fragment's overflow clip from layout, which
  follows the containing-block chain (absolute boxes escape intermediate
  clips, fixed boxes escape all). The rasterizer clips shapes geometrically,
  so fractional clip edges antialias. `dump-paint` prints ` clip=(x,y)` ranges
  for clipped items; unclipped lines keep the REN-04a format.
- **Opacity.** A context with `opacity < 1` becomes a `BeginGroup`/`EndGroup`
  pair: its content is painted into a transparent buffer, then composited once
  at the group opacity. `opacity: 0` paints nothing, descendants included.
- **Backgrounds.** Background color fills the box named by the last
  `background-clip` layer (border, padding or content box).
- **Borders.** Solid borders only. When all four sides share a color the
  border is one ring, so no seam appears at the corners. Otherwise each side is
  a quadrilateral mitred along the corner diagonal.
- **Colors.** Computed colors (`rgb`, `lab`, `lch`, `oklab`, `oklch` and every
  predefined `color()` space) convert to 8-bit sRGB with CSS Color 4 matrices.
  Out-of-gamut channels clip (TD-41). Fully transparent items are dropped.
- **Visibility.** `visibility: hidden/collapse` boxes paint nothing of their
  own; visible descendants still paint.
- **Unsupported, fails atomically:** any `background-image`,
  border styles other than `solid`, and everything layout already rejects
  (text, inline content, positioning, floats, overflow, replaced elements).
- **Limits.** `PaintLimits::max_items` (default 400,000) and cancellation.

`rustic dump-paint <file>` prints the canvas color and every item with its
source box, node, role, shape in CSS pixels and `#rrggbbaa` color.

## Rasterizer (`rustic-raster`)

`rasterize(list, scale, limits, cancellation)` allocates a premultiplied RGBA8
surface of `ceil(viewport × scale)` device pixels, clears it to the canvas
color and composites each item source-over.

- **Coverage.** Each pixel's coverage is the exact area of the pixel inside the
  shape, from signed-area accumulation over the clipped edges (nonzero winding,
  clamped to one). Edges are not snapped to device pixels, so fractional
  geometry and scales antialias.
- **Determinism.** Only IEEE `f64` add/multiply/divide and rounding are used in
  coverage and compositing; no `powf`, `sin` or other libm call. Output bytes
  are the same on every architecture and OS; a golden checksum test runs on
  every CI target, including 32-bit, big-endian and RISC-V emulation. (Color
  conversion for `lab()`/`color()` uses `powf` before rasterization, so those
  colors can differ by one ULP between platforms before rounding to 8 bits.)
- **Limits.** `RasterLimits` bounds width, height and surface bytes before
  allocation (the surface plus every open opacity group, each a full surface,
  TD-46), and total pixel visits summed over item bounds (`max_work`,
  default 2³⁰). Each item uses a scratch buffer sized to its clipped bounds.

`encode_png(surface, max_bytes)` writes straight-alpha RGBA8 PNG with stored
(uncompressed) deflate blocks, after checking the exact output size against
`max_bytes`. CRC-32 and Adler-32 are computed in the crate.

`rustic render --output out.png [--scale N] <file>` loads a local file the same
way as the dump commands, renders at a 1280×720 CSS viewport and writes the PNG.
The PNG counts against `--max-output-bytes`; nothing is written on failure.

## Tests

- Unit tests: exact coverage for aligned, fractional, diagonal and clipped
  shapes; surface budgets; unpremultiply rounding; CRC check values; a
  byte-exact one-pixel PNG; stored-block splitting; CSS Color 4 reference colors.
- `crates/rustic-raster/tests/paint.rs`: canvas propagation, background-clip,
  rings and mitred borders, exact antialiasing at scales 1, 1.25 and 2,
  translucent compositing, visibility, unsupported features with their nodes,
  exact item/work budgets, cancellation, and a golden PNG checksum.
- CLI: a reviewed `dump-paint` fixture at two chunk sizes with an exact output
  budget, deterministic `render` output, budget failure without a file, and
  argument validation.
- Fuzzing: the `paint-raster` target lays out, paints and rasterizes mutated
  HTML/CSS at two scales and checks surface size, opacity, determinism and the
  PNG length formula.

Paint-order, clip and opacity tests: Appendix E order across all seven layers,
pseudo-contexts handing positioned descendants to the real context, local
`z-index` inside a context, fixed contexts, clip chains with an escaping
absolute box and a half-covered clip row, group compositing versus per-fill
alpha, `opacity: 0`, the group memory budget and a layered golden checksum.
The fuzz target also checks one background per box and balanced groups.

## Not yet implemented

Text and glyphs, images and decoding, gradients, non-solid borders,
`border-radius`, outlines, box shadows, blend modes and filters, rounded or
`clip-path` clips, transforms, scrolling offsets, coverage-conflation-free corner
joins, PNG compression and reftests. See TD-41 and docs/ROADMAP.md (REN-04 to
REN-07).
