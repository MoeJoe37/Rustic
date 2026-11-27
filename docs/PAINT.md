# Paint, CPU raster and compositing

`rustic-paint` builds an immutable display list from layout fragments.
`rustic-raster` interprets it on the CPU and writes PNG. The CPU path is the
pixel reference for the desktop compositor. M8 added confined GPU-process
compositing through `rustic-gpu`, with Vulkan, Metal and Direct3D 12 and a
CPU fallback. [ADR 0071](adr/0071-gpu-backend.md) covers that boundary and
[PERFORMANCE.md](PERFORMANCE.md#gpu-frame-budgets) records its frame-time gate.
This page describes core paint semantics and reference rasterization.

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
- **Borders.** Solid borders, and `inset`/`outset` as solid sides with the
  top and left (inset) or bottom and right (outset) at two thirds of each
  color channel; other styles fail. Cells of collapsing-border tables
  paint each side with the style and color of the border that won that
  grid line (named by the layout fragment); empty cells with
  `empty-cells: hide` paint neither background nor border. When all four
  sides share a color the
  border is one ring, so no seam appears at the corners. Otherwise each side is
  a quadrilateral mitred along the corner diagonal.
- **Colors.** Computed colors (`rgb`, `lab`, `lch`, `oklab`, `oklch` and every
  predefined `color()` space) convert to 8-bit sRGB with CSS Color 4 matrices.
  Out-of-gamut channels clip (TD-41). Fully transparent items are dropped.
- **Visibility.** `visibility: hidden/collapse` boxes paint nothing of their
  own; visible descendants still paint.
- **Unsupported, fails atomically:** background images and unsupported border
  styles or formatting. The text, image and effect sections below describe
  the implemented inline, replaced-element and compositing paths.
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

- **Tiles.** The surface is painted in `TILE_SIZE` (256) device-pixel tiles
  on a grid from its origin. Each tile renders into a frame grown by the
  list's effect margin (the largest nested sum of filter blur reaches and
  drop shadow offsets), so a tile's pixels depend only on the list and the
  tile. `rasterize_region(list, scale, region, …)` paints just the tiles
  that meet `region` and returns exactly the pixels `rasterize` would; blur
  box sums are fixed point so they do not depend on where a frame starts.
- **Culling.** Before painting, each fill's device bounds (mapped through
  transforms, grown by shadow blur, enclosing filter reach and a pixel) and
  each layer's union of its contents are computed once; a tile skips fills
  and whole layers (with their buffers) whose bounds miss its frame.
  `rasterize_reference` runs every command in every tile and is the
  reference culling must match byte for byte. The `frame_budget` example
  (`cargo run --release -p rustic-raster --example frame_budget`) measures a
  1280×720 page with shadows, groups, a transform, a filter, blending and
  text: on the development sandbox a full raster took 77 ms at scale 1
  (262 ms without culling) and 195 ms at scale 2 (1.1 s without), and
  repainting the two or four tiles damaged by one changed box took 3.5 ms
  and 11 ms, inside a 16.7 ms frame. Timings vary by machine; CI does not
  gate on them (TD-59).

- **Damage.** `damage(old, new, scale)` returns the tiles whose pixels can
  differ between two lists. Each fill is keyed by what it paints (shape with
  side-table data resolved, color, axis and rounded clips) and by the chain of
  groups, clip paths and transforms around it; node and box ids are not part
  of the key. Fills outside the common prefix and suffix of both lists damage
  their device bounds (mapped through transforms, grown by shadow blur, the
  reach of enclosing filters and one pixel). A different surface size, canvas
  color or filter margin damages every tile. `repaint(surface, list, scale,
  regions, …)` repaints regions in place; `rustic reftest` checks that
  repainting a reference's damage gives the test's pixels exactly. See TD-59.

`encode_png(surface, max_bytes)` writes straight-alpha RGBA8 PNG with stored
(uncompressed) deflate blocks, after checking the exact output size against
`max_bytes`. CRC-32 and Adler-32 are computed in the crate.

`rustic render --output out.png [--viewport WxH] [--scale N] [--zoom Z] <file>`
loads a local file the same way as the dump commands, lays it out for the view
(default a 1280×720 window at ratio 1 and 100% zoom) and writes the PNG. The PNG
counts against `--max-output-bytes`; nothing is written on failure.

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

## Image decoding (`rustic-codecs`)

`rustic-codecs` holds from-scratch decoders for untrusted data, the "codec
adapters" of ARCHITECTURE.md. `inflate`/`zlib_decompress` implement RFC 1951
stored, fixed and dynamic Huffman blocks and the RFC 1950 wrapper with its
Adler-32 check, under an exact output limit and cancellation, so a
decompression bomb stops at its budget. Huffman codes are validated
(over-subscribed and incomplete codes are rejected, except the single one-bit
code RFC 1951 allows) and back-references cannot reach before the output.

`decode_png(bytes, limits, cancellation)` returns straight-alpha RGBA8. It
handles every color type and bit depth, palettes, `tRNS` (compared at the
original sample depth), the five row filters and Adam7 interlacing. Chunk CRCs
are verified, chunk order is enforced, unknown critical chunks are rejected,
and the zlib stream must inflate to exactly the size the header implies.
`PngLimits` caps width, height, decoded bytes and chunk count; the size check
happens when `IHDR` is read, before any pixel allocation. 16-bit samples are
rounded to 8 bits. Color-management chunks are ignored (TD-51).

Evidence: 51 PngSuite images (vendored with their license in
`crates/rustic-codecs/tests/pngsuite`) match references from an independent
decoder (`tools/pngsuite_reference.py`); interlaced images match their
progressive twins; every truncation of twelve images fails cleanly and
single-bit flips never panic; exact limit and cancellation tests; RFC test
streams for stored, fixed and dynamic blocks; a decompression-bomb test; a
round trip of `rustic render` output through the decoder; and the `codec-png`
fuzz target seeded with PngSuite.

### JPEG, GIF and WebP

`decode_image` sniffs the format (WHATWG MIME Sniffing image patterns, see
`sniff`) and returns the first frame as straight RGBA8; `decode_animation`
returns every composited frame with its delay and the loop count. All three
decoders take `DecodeLimits` (width, height, total decoded bytes across
frames, frame count), check sizes before allocating, poll cancellation and
fail atomically with `ImageError`.

- **JPEG** (`jpeg.rs`, ITU-T T.81): Huffman baseline, extended and
  progressive (spectral selection and successive approximation) frames,
  1/3/4 components, sampling factors 1–4, restart intervals, 8/16-bit
  quantization tables, YCbCr, RGB (Adobe transform 0), CMYK and YCCK
  (Adobe-inverted). The IDCT is an exact integer product, chroma uses the
  IJG triangle filter for 2× factors, and EXIF orientation is applied, so
  natural sizes are post-rotation as `image-orientation: from-image`
  requires. Entropy data that ends early decodes as zero bits, like
  browsers. Block visits across scans are capped at 48 per block and scans
  at 1,000, bounding the work of tiny malicious progressive files. TD-80.
- **GIF** (`gif.rs`, GIF89a): LZW with deferred clear, global and local
  palettes, interlacing, Graphic Control Extension transparency, delays and
  disposal (none, background, previous), NETSCAPE2.0 loops. The canvas
  starts transparent. A file cut after a complete frame keeps the complete
  frames.
- **WebP** (`webp.rs`, `vp8l.rs`, `vp8.rs`, RFC 9649 and RFC 6386): the
  RIFF container, lossless VP8L (all four transforms, color cache, meta
  prefix codes, LZ77), lossy VP8 key frames (segmentation, probability
  updates, all intra modes, WHT/DCT, normal and simple loop filters, token
  partitions), `ALPH` (raw or lossless, all filters) and `ANIM`/`ANMF`
  animation with libwebp's straight-alpha blending. Chroma upsampling and
  YUV→RGB use libwebp's fixed-point arithmetic, so output is bit-exact with
  libwebp. The VP8 tables in `vp8_tables.rs` are RFC 6386 constants.

`probe` reads the format and natural size from the header alone.

Evidence: `tests/images` holds 47 synthetic images drawn and encoded by
`tools/codec_reference.py` (libjpeg-turbo, Pillow's GIF writer, libwebp; no
downloaded files) with RGBA references decoded by Pillow. JPEG matches
within 3 per channel, GIF and WebP (including 6 animations) exactly.
Truncations and bit flips of every image never panic, limits and
cancellation are checked per image, and `codec-jpeg`, `codec-gif` and
`codec-webp` fuzz targets check determinism and still/animation agreement.

### Image resource lifecycle

`ImageStore` holds encoded bytes by key (the resolved URL), so elements
naming the same file share one entry. `insert` probes the header (the HTML
*available size* state); `decode` produces frames lazily and keeps them
under `max_decoded_bytes`, evicting the least recently used decoded entries
(their encoded bytes stay, and they decode again on demand); any decoding
failure other than cancellation leaves the entry *broken* for good.
`max_entries` and `max_encoded_bytes` refuse further inserts, and
`release` forgets an entry. The command line loader decodes through a
per-document store. TD-83.

## Text

`DisplayList::build_with_fonts` paints the inline content the layout
produced with `BlockLayout::compute_with_fonts`, given the same
`FontDatabase`. Each in-flow block's inline content paints in its layer after
the floats (CSS 2.2 Appendix E step 7), line by line: inline box backgrounds
and solid borders (only the sides the layout gave a width, so split inline
boxes draw their start and end edges once), then one `ItemRole::Text` item per
text fragment. That item's `Shape::Path` indexes `DisplayList::paths`: the
fragment's glyph outlines from the face's `glyf` table, scaled by
`font-size / unitsPerEm` and placed at each glyph's pen position on the
fragment's baseline, filled in the text's `color`. On a vertical line
(`TextFragment::rotation`, CSS Writing Modes 4 §5.1) each outline turns a
quarter turn about its glyph's origin: clockwise in `vertical-rl`,
`vertical-lr` and `sideways-rl`, counterclockwise in `sideways-lr`, and the
fragment's decorations become vertical bands on the line's under side.
Upright fragments (`TextFragment::upright`) are not turned, but their
decorations follow the line.
Inline content is clipped
by its block's own overflow as well as its ancestors'. Outlines are read once
per face and glyph; `PaintLimits::max_path_points` bounds the total points, and
a layout with text but no fonts is `PaintError::Font`. The rasterizer
flattens each quadratic arc into at most 128 segments within 0.05 device
pixels and fills with the same exact-area coverage, whose clamped
accumulation approximates the nonzero rule. Glyphs are not hinted or snapped
to pixels, so text renders identically on every platform. `visibility:
hidden` text is not painted, and `opacity: 0` contexts paint none. Inline
boxes take hits; text nodes do not (their inline box or block does).

Text decorations (CSS Text Decoration 3 §2) propagate from every ancestor
box with a `text-decoration-line` to the text inside it, outermost first,
stopping after a float or absolutely positioned box. Each line is one
`ItemRole::Decoration` rectangle across the fragment's advance width in the
decorating box's `text-decoration-color`; underlines and overlines paint
before the glyphs and line-throughs after. Positions come from the
fragment's face: thickness `font-size / 16`, underline top `font-size / 10`
below the baseline, overline top at the ascent, line-through centered half
an x-height above the baseline. `double` adds a second line one thickness
further out; `dotted`, `dashed` and `wavy` fail with
`UnsupportedPaint::DecorationStyle`.

The command line lays out and paints with the bundled original test fonts
(`rustic_text::bundled`), with every generic family and the fallback mapped to
them; no system fonts are read. Their `x` is a plain rectangle (500 units
advance, 50 to 450 wide, 500 high, at 1000 units per em), which the text
reftests use as an Ahem-style reference. See TD-52.

## Images

`DisplayList::build_with` takes `Resources`: the fonts plus decoded
`Image`s (straight-alpha RGBA) by element. A block-level replaced element
with an image paints it atomically at the start of its inline-content step
(CSS 2.2 Appendix E step 7) as one `ItemRole::Image` item whose
`Shape::Image` stretches the image over the content box (`object-fit: fill`),
clipped by its ancestors' overflow. The rasterizer samples each device
pixel's center bilinearly in premultiplied space with edges clamped, and
weights partial edge pixels by covered area; when image pixels map one to
one onto device pixels it copies them exactly. `image-rendering`,
`object-fit`, `object-position` and inline-level images are not supported
yet (TD-53).

The command line loads each `img` that generated a box and has a non-empty
`src`: the URL resolves against the document base and must name a file in
the document's directory or below it (like linked stylesheets); the file is
read in bounded chunks into an `ImageStore` and decoded as PNG, JPEG, GIF or
WebP with `rustic-codecs`; animated images show their first frame (TD-82). One image
pixel is one CSS pixel of natural size, passed to layout through
`BoxTree::set_natural_sizes`. `--max-images`, `--max-image-bytes` and
`--max-image-decoded-bytes` bound the count, encoded bytes and decoded bytes;
an image that is blocked, missing, over budget or not a decodable image is broken
and keeps a zero natural size, and the load continues.

## Atomic inlines

A non-positioned `inline-block`, `inline-flex` or inline replaced element
(a block fragment with `BlockFragment::line`) paints in the inline phase
(CSS 2.2 Appendix E step 7.2.1.4): after its line's inline box backgrounds,
borders and text, atomically, as if it were a stacking context, so it lies
above floats and later in-flow blocks. Its positioned descendants and
stacking contexts still belong to the enclosing stacking context. A
positioned atomic inline paints in the positioned layer like any positioned
box. Within one line, atomic inlines follow all of the line's text rather
than interleaving in tree order (TD-70).

## Hit testing

`DisplayList::hit_test` returns the topmost box whose border box contains a
point, walking boxes in reverse paint order, so z-index, floats and
positioned boxes resolve overlaps the way painting does. Overflow clips
exclude points as they exclude paint. `visibility: hidden` boxes are not
targets (their visible descendants are); `opacity: 0` boxes paint nothing but
still take hits. Edges are half-open. A point inside the viewport that hits
no box finds the root element; a point outside it finds nothing. `rustic
hit-test --at X,Y <file>` prints the result for each point. Transformed boxes
are hit through their planes and rounded clips and clip paths cut hits as they
cut paint. Text, inline boxes, `pointer-events` and scrolling offsets are not
considered yet.

### Resize and zoom

`rustic_layout::View` is the window a page is shown in: its size in pixels at
100% zoom, the device pixel ratio and the page zoom (25% to 500%). The layout
viewport is the window over the zoom and the paint scale is the ratio times the
zoom, so zooming in reflows the page into a narrower viewport and paints it
larger, as browsers do. A resize or zoom recomputes styles against the new
viewport (media queries see its size and resolution, viewport units its
dimensions), lays the page out again, rebuilds the display list and rasterizes
at `View::scale`; `View::to_css` maps a device point of the window to the CSS
point to hit-test. The surface is the window times the ratio (float error in
the zoomed viewport is ignored when rounding the surface size up). `rustic
render` and `rustic reftest` take `--viewport WxH` and `--zoom Z` alongside
`--scale`. Relayout is from scratch: no style or layout work is reused across
views (TD-59).

## Reftests

`rustic reftest [--scale N] <test.html>` renders a test page and each
reference it names with `<link rel=match href>` or `<link rel=mismatch href>`,
using the same loader, viewport and scale, and compares the surfaces with
`rustic_raster::compare` (differing pixel count and largest per-channel
difference of premultiplied RGBA). A `match` reference holds when the images are
identical or the difference lies within the test's
`<meta name=fuzzy content>` ranges; a `mismatch` reference holds otherwise.
The fuzzy value is `maxDifference=A;totalPixels=B` or `A;B`, each a number `N`
(read as `0-N`) or an inclusive range `N-M`, optionally prefixed by `href:` to
apply to one reference only. An unparsable fuzzy value or a page without
references fails the test. References resolve against the document base URL
and load only from the test's directory or below it (checked lexically and
again after symbolic links resolve), like linked stylesheets. Reference chains
are not followed yet.

The checked suite lives in `crates/rustic-cli/tests/reftests` and runs at
scales 1 and 2 in `cargo test`; it covers borders against nested backgrounds,
z-index against tree order, overflow clipping, opacity groups with a fuzzy
tolerance, solid and double outlines, rounded borders and clips,
`clip-path` circles, polygons and insets, and mismatches.

## Outlines

`outline-width`, `outline-style`, `outline-color`, `outline-offset` and the
`outline` shorthand parse and compute per CSS UI 4 (the width computes to 0
when the style is `none`). Each stacking context paints the outlines of every
visible box it owns, block boxes and inline box fragments in tree order, after
all its other content and inside its opacity group (CSS 2.2 Appendix E step
10). An outline is an [`ItemRole::Outline`] ring outside the border box grown
by `outline-offset`, which may be negative; when a negative offset collapses
the ring's inner edge, the remaining area is filled. `double` paints two rings
of a third of the width; `auto` paints as `solid`; other styles fail with
`UnsupportedPaint::OutlineStyle`. Outlines are clipped by ancestors' overflow
like the box's border and never take hits. See TD-54.

## Rounded corners

The `border-*-radius` longhands and the `border-radius` shorthand (with the
`/` form) parse and compute; lengths become pixels and percentages stay. At
paint time percentages resolve against the border box, and all radii scale
down together when adjacent ones would overlap (CSS Backgrounds 3 §5.5).
Each corner is four quadratic curves built from constant tables, within
0.02% of the radius, so no platform math library is involved.

* Backgrounds are [`Shape::Rounded`] over their `background-clip` box's curve
  (the border curve shrunk by the border and padding widths, §5.2).
* Borders with one color are a [`Shape::RoundedRing`]; with several colors,
  each side is the ring cut by the rays from the outer corners through the
  inner corners, stored as a polygon path.
* Outlines follow the curve, with each rounded corner's radii grown by the
  outline's distance from the border edge.
* A box whose overflow is clipped on both axes clips its content to its
  padding edge curve, and a replaced element clips its image to its content
  edge curve. These are [`ClipNode`]s chained along the containing-block
  chain like the axis clips, so absolutely positioned boxes escape them. Each
  item names its innermost node; the rasterizer multiplies the item's
  coverage by every node's coverage.
* Hit testing excludes the corners outside a rounded border box and points
  outside a box's rounded clips.

Shapes and clips are stored once in [`DisplayList::rounded`] and
[`DisplayList::clips`] and are listed in dumps as `rounded=` shapes and
`rclip` lines. See TD-55.

## Clip paths

`clip-path` parses `none`, `url()`, a reference box alone and the `inset()`,
`circle()`, `ellipse()` and `polygon()` basic shapes with an optional
reference box, and computes their lengths and positions. A box with a clip
path other than `none` is a stacking context (CSS Masking 1 §5). Its
context paints into a layer opened by `DisplayCommand::BeginClip` and closed
by `DisplayCommand::EndClip`, which composites the layer weighted by the
shape's exact-area coverage, so the box and every descendant painted in its
context are clipped, including positioned ones that escape its overflow.
Circles and ellipses are fully rounded [`RoundedRect`]s, insets are rounded
rectangles with their `round` radii, and polygons are paths with the
nonzero rule. Reference boxes resolve against the box: `margin-box`,
`border-box` (the default), `padding-box` and `content-box`; `fill-box` is
the content box and `stroke-box` and `view-box` the border box, as for boxes
without SVG layout. A reference box alone clips to that box's
`border-radius` curve. Hit testing excludes points outside every enclosing
clip path. `url()` references and `polygon(evenodd, …)` fail with typed
errors. See TD-56.

## Transforms

`transform`, `translate`, `rotate`, `scale`, `transform-origin`,
`perspective`, `perspective-origin`, `backface-visibility` and
`transform-style` parse and compute (CSS Transforms 1 and 2). Every
transform function is supported, 3D ones included; angles take `deg`,
`grad`, `rad` and `turn`. A box with a transform, or with `perspective`, is
a stacking context. Its matrix is the containing block's perspective
(`perspective(d)` about its `perspective-origin`), then `translate`,
`rotate`, `scale` and `transform` about `transform-origin` in the border box.
Trigonometry uses plain arithmetic (exact reduction to 45° and Taylor
series), so matrices are identical on every platform.

Layout ignores transforms. Paint keeps every item of a transformed context
in its local layout coordinates and brackets the context with
`DisplayCommand::BeginTransform` and `EndTransform`, which name a
[`TransformNode`]: the plane homography of the 4×4 matrix (the z = 0 plane
is all that is drawn, since 3D contexts are flattened) and the overflow and
rounded clips of the ancestors around the box, in the parent's coordinates.
Clips inside a transformed context restart there: a containing block in
another coordinate space does not clip its descendant directly, its clips
apply through the transform node instead.

The rasterizer keeps a stack of local-to-device maps. Inside a transform,
each shape is flattened finely enough for the local stretch of the map,
clipped to its local overflow clip and to the half plane in front of the
viewer (w > 10⁻⁶), and projected; polygons stay exact because projections
map lines to lines. Rounded clips and clip paths are projected the same way,
and images sample the local point each device pixel center comes from. A
transform with clips around it opens a layer that is composited through
those clips' coverage in the enclosing coordinates. Outside every
transform, nothing changed, so earlier output is byte-identical.

A matrix that is not finite or not invertible on the plane (`scale(0)`),
and a `backface-visibility: hidden` box whose back faces the viewer, paint
and hit nothing. Hit testing maps the point into each region's local plane
through the inverse homographies and checks every enclosing transform's
clips and clip path in its own coordinates. See TD-57.

## Shadows, blending and filters

`box-shadow` parses (one to many shadows, `inset`, two to four lengths and
an optional color in either order) and computes lengths to pixels, blur
radii clamped at zero, and `currentcolor` to the box's color. Outer shadows
paint before the box's background and inset ones after it, before the
border, the first shadow on top. Each is a [`ItemRole::Shadow`] item whose
[`Shape::Shadow`] indexes a [`BoxShadow`]: the border box moved and spread
(radii grown by the spread, scaled by `1 + (r/s − 1)³` below it, so square
corners stay square), or the padding box moved and shrunk for an inset
shadow, whose shadow is everything outside that shape. The rasterizer
blurs the shape's coverage by a Gaussian of half the blur radius (exactly
below two device pixels, by three box blurs above, as `feGaussianBlur`
allows), in plain arithmetic, then keeps it outside the border box or
inside the padding box. Shadows take no hits.

`mix-blend-mode` (all sixteen modes plus `plus-lighter`), `isolation` and
`filter` make a stacking context. A context that blends, has `opacity < 1`
or has blending children (which blend only with their stacking context)
paints into a group; [`DisplayCommand::BeginGroup`] carries the blend mode,
and the group composites with `co = cs·αs·(1 − αb) + cb·αb·(1 − αs) +
αs·αb·B(cb, cs)`. A filter opens an inner group inside any clip path, since
filters apply before clipping: `blur()` blurs every premultiplied channel,
color functions are 4×5 matrices on straight-alpha sRGB (clamped after
each), and `drop-shadow()` paints the blurred, offset alpha under the
group. `url()` filters fail with a typed error. See TD-58.

## Not yet implemented

Non-solid outline styles, non-solid text decoration styles, `text-underline-offset` and
`text-decoration-thickness`, emphasis marks and shadows; `background-image`,
`object-fit` and `image-rendering`,
animated image playback, gradients, dotted, dashed, double, groove and ridge borders,
`text-shadow`, `background-blend-mode`, `backdrop-filter`, `filter` URLs, `clip-path` URLs and the
`evenodd` rule, `xywh()`, `rect()`, `path()` and `shape()`, `transform-style: preserve-3d`, coverage-conflation-free corner
joins, PNG compression, reference chains in reftests and a broad imported
reftest suite. See TD-41, TD-52, TD-53, TD-54, TD-55, TD-56, TD-57, TD-58, TD-59, TD-80 to TD-84 and docs/ROADMAP.md (REN-04 to
REN-07). The implemented viewport and container scroll paths are described
in [ADR 0064](adr/0064-scrolling-and-compositor-scenes.md).
