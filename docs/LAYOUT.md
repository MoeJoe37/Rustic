# M3 layout reference

REN-01a started M3 with `rustic-layout`, a platform-independent safe-Rust crate
using only existing workspace crates. It consumes M2 computed styles; it does
not parse CSS or run another cascade. No renderer, font adapter, decoder or OS
library is embedded. ADR 0003's headless CPU-first boundary is unchanged.

## Checked geometry

`CssPx` accepts finite signed coordinates up to ±1,000,000,000 CSS pixels,
normalizes negative zero and checks addition/subtraction. `CssSize` also rejects
negative dimensions; `CssRect` checks its far edges before construction.
`DeviceScale` accepts a finite scale in `(0, 64]`; `DevicePx` keeps fractional
coordinates, without implicit pixel rounding. These are explicit prototype
coordinate limits, not a surface-allocation or total-memory guarantee.

`rustic-base` holds the unit-typed form shared outside layout:
`Length<U>`, `Point2<U>`, `Size2<U>`, `Rect<U>` and `Scale<S, D>` with `Css`
and `Device` markers, so a CSS coordinate cannot be used as a device pixel
without an explicit scale. `CssRect::to_typed` and `DeviceScale::to_typed`
convert layout geometry into it. `SurfaceAllocation::check` and `covering`
turn a device rectangle into a surface request with checked integer
arithmetic against per-axis and byte limits; `rustic-raster` allocates its
surfaces only through it. Layout keeps its own `CssPx` family (TD-73).

## Principal box snapshot

`BoxTree::build(document, root, styles, limits, cancellation)` validates the
selected root and every supplied style's document/slot/generation. Duplicate,
non-element, foreign and stale styles fail. Every visible element must have a
computed style. `display:none` prunes a subtree; `display:contents` promotes
children while retaining the DOM parent's computed style for text inheritance.
Templates remain inert. Text copies raw UTF-16, including unpaired surrogates.
The flat preorder arena owns its styles/text and uses snapshot-local parent
indexes, so source mutation does not alter an existing snapshot and destruction
does not recurse through a content-controlled tree.

Independent limits cap visited DOM nodes/indexed styles, retained boxes, copied
UTF-16 units and DOM depth. Hidden nodes still count as visited work. Checks
precede box/text retention; cancellation is checked while indexing and walking.
Failure returns no partial snapshot and cannot mutate the DOM or style input.
Allocator exhaustion is not promised recoverable. Dump output is checked before
each append and has constant-size lines with no indentation or page text.

This is an initial principal-box snapshot, **not** the final CSS box/fragment
tree: anonymous boxes, whitespace processing, generated pseudo boxes and
formatting-context fixups are pending (TD-37). The display value identifies a
future formatting context; retaining `flex` or `table` does not implement it.
Shadow hosts contribute their shadow children. Slot assignment determines
whether the box walk reaches assigned light children or fallback content.

## Horizontal block measurement

`measure_block_width(style, containing_width, containing_direction)` measures
one **non-replaced**, non-floated normal-flow block or flow-root. The caller
selects non-replaced boxes; replaced/intrinsic sizing is a separate future
algorithm. The containing direction, rather than the child's direction, selects
the margin adjusted in an over-constrained equation. Inline, flex/grid/table,
floated and out-of-flow styles fail explicitly.

The reference implements auto/fixed/percentage/mixed-calc width, horizontal auto
margins, negative margins, min/max-width (minimum wins conflicts), content-box
and border-box subtraction, computed border widths and percentage padding.
Vertical margin/padding percentages also resolve against containing width;
these values are measured without performing vertical flow or margin collapse.
Over-budget dimensions fail without silently clamping to the coordinate limit.

The width equation is checked by an independent invariant over combinations of
width, margins, box sizing and containing direction. Focused tests cover
centering, over-constraint, negative remaining width, min/max precedence, calc,
percentages, border suppression from M2 and geometry failures.

Specification basis, reviewed 2026-11-08: CSS 2.2
[§10.3.3](https://www.w3.org/TR/CSS22/visudet.html#blockwidth) and
[§10.4](https://www.w3.org/TR/CSS22/visudet.html#min-max-widths), and CSS Sizing 3
[box sizing](https://www.w3.org/TR/css-sizing-3/#box-sizing). This is a focused
algorithm/fixture claim, not a layout-WPT conformance result.

## Static vertical block reference (REN-01b / REN-02a)

`BlockLayout::compute(tree, viewport, limits, cancellation)` now produces an
immutable preorder list of physical `BlockFragment`s: source node/box, parent
fragment and checked content, padding and border rectangles. It consumes the
existing snapshot and never reads or mutates the live DOM. Three bounded
iterative passes measure containing-block constraints top down, resolve heights
and collapsed margins bottom up, then place absolute rectangles top down.
Relative child positions avoid repeatedly translating completed subtrees.
A fixed-height parent advances following siblings by its own used border height,
even when its children visibly overflow it. Widths use the REN-01a algorithm.

Supported height behavior includes fixed and automatic content heights,
min/max-height (minimum wins), border-box subtraction, percentages and calc.
A percentage height resolves against a definite containing content height,
including min/max adjustment; percentages against an automatic containing
height stay automatic rather than using its eventual height. Percentage
min/max bounds on indefinite containing heights use their initial bounds.
Styles must be recomputed for a changed viewport before layout when viewport
units/media conditions change; layout itself re-resolves containing-block
percentages. The geometry test exercises both resize paths.

The accepted subset is explicit (TD-38): non-replaced block/flow-root boxes.
Horizontal margins (including auto/negative values) and collapsed vertical
margins are supported; REN-02b adds floats, clearance, relative/absolute/fixed
positioning and overflow clipping (below). Pure CSS collapsible whitespace
under normal/nowrap produces no fragment. Preserved whitespace, non-whitespace
text, inline/list/flex/grid/table contexts, replaced
elements, native controls, special HTML layout and foreign content fail
atomically with a typed reason. Empty inline boxes are not silently treated as
empty blocks. No shaping metrics or anonymous line boxes are invented. This is
useful for inspecting controlled block fixtures; it is not yet arbitrary
article layout.

Layout checks its own node/fragment/depth/text limits, cancellation during
traversal, long whitespace runs and all geometry passes, and coordinate bounds
before returning a result. Failure leaves the input snapshot and any earlier
layout unchanged. Dumps check the full next line before appending. The
reference uses no content-controlled recursion and no platform dependency.

Fourteen flow scenarios cover exact nested geometry, overflow and sibling
positions, definite/indefinite percentage heights, min/max and box sizing,
calc, RTL, resize, whitespace semantics, unsupported content, atomic failure,
exact budgets, coordinate overflow, an independent 64-block prefix-sum model
and a 200-level auto-height chain. This complements the existing thirteen
geometry/snapshot/width tests. The additional margin fixtures and independent
model below complete the normal block-flow and collapse steps of
`layout.block`; mixed inline/block anonymous flow and the general box/fragment
capability remain unfinished.

Specification basis, reviewed 2026-11-08: CSS 2.2
[normal flow](https://www.w3.org/TR/CSS22/visuren.html#normal-flow),
[§10.5–10.7 heights](https://www.w3.org/TR/CSS22/visudet.html#the-height-property)
and [margin collapse](https://www.w3.org/TR/CSS22/box.html#collapsing-margins).
These are focused reference cases, not a layout-WPT result. Clearance and
floats arrived with REN-02b.

## Adjoining margin collapse (REN-02a)

`margins.rs` retains a margin strut as its greatest positive and most negative
member; its resolved size is their sum. It never adds a new margin to an already
resolved pair, which would lose the original extrema. Adjacent siblings join
struts, parent top/bottom margins join eligible first/last in-flow children, and
empty zero/auto-height blocks can pass a strut through themselves. Root edges,
flow-root contexts, borders, padding and fixed heights enforce the specified
collapse boundaries. A nonzero minimum height handles the special all-through
child case without letting that child's margin escape both parent edges.

An empty block's border position is computed as if it had a bottom border unless
its margin joins the parent's top; its own bottom margin still influences later
siblings. Negative margins can move boxes above earlier boxes, and a negative
automatic content height clamps to zero. Percentage margins use containing
width. The fragment's `margin()` accessor retains resolved values before
collapse; its rectangles include the collapsed positioning effects.

Ten regression scenarios cover signs, parent/child and empty-through behavior,
border/padding/flow-root boundaries, min/max/fixed heights, the root exception,
percentage bases and a 200-level chain. A separate adjacency-graph model checks
192 sixteen-sibling layouts (3,072 child positions), including virtual bottom
borders for empty-box positioning; it uses component relabeling rather than the
implementation's strut algorithm. A unit test independently checks all 125
three-margin combinations for grouping/order invariance. Existing zero-margin
geometry remains unchanged. `margins.html` adds a checked CLI snapshot with
signed sibling margins, an empty block and parent-child collapse.

The specification basis is CSS 2.2
[§8.3.1](https://www.w3.org/TR/CSS22/box.html#collapsing-margins), reviewed
2026-11-08, together with §10.6–10.7 height rules. No browser/WPT pass count is
inferred from these focused fixtures. No dependency or browser engine was added.

## Inspect a local fixture

```sh
cargo run --locked -p rustic-cli -- dump-boxes crates/rustic-cli/tests/fixtures/boxes.html
cargo run --locked -p rustic-cli -- dump-layout crates/rustic-cli/tests/fixtures/layout.html
cargo run --locked -p rustic-cli -- dump-layout crates/rustic-cli/tests/fixtures/margins.html
cargo test --locked -p rustic-layout
```

`dump-boxes` uses the same HTML loader, UA/author cascade, local stylesheet
fetch policy and loader/output limits as `dump-style`. No network, scripts,
images or fonts load. Its checked fixture is repeated with different chunk
sizes; an output-budget failure emits no partial stdout. Use `dump-style` to
inspect stylesheet diagnostics and skipped resources. The structural dump lists
box index, parent index, source slot/generation, computed display or text-unit
count. It has no positions or painted pixels yet and is not a stable wire format.

`dump-layout` uses the same local loader/cascade and output limit, with the
same default computed-style viewport. It reports `content=(x,y,width,height)`,
`padding=(...)` and `border=(...)`, all in CSS pixels. The golden geometry
fixture is checked at chunk sizes 1, 7 and 65536 and exact output boundaries;
an integration case also loads a local linked stylesheet and verifies that
unshaped text fails with empty stdout. These commands also accept HTTP(S)
documents through the broker when the CLI is given explicit origin grants.
See [the network loader contract](IMPLEMENTATION.md#net-02--documents-and-subresources-over-http-in-the-cli).

## Status of the M3 layout units

The sections below were written slice by slice (REN-01a onward), so earlier
ones describe the scope at the time and later sections extend them. As of the
M3 completion (2026-11-10) every declared layout unit is implemented: block
flow and margin collapse, anonymous flow normalization, floats, clearance,
relative/absolute/fixed/sticky positioning, overflow clipping, intrinsic and
replaced sizing (REN-02c), inline formatting with OpenType shaping and atomic
inlines (REN-03), and flexbox with text items and `inline-flex` (REN-08; 49 of
91 static css-flexbox WPT pages pass, TD-72). M9 added grid,
well-formed tables and vertical writing modes (TD-209, TD-211, TD-212);
multicolumn, upright vertical text and fragmentation are still M9 work. Open layout debt is in
[TECH_DEBT.md](TECH_DEBT.md) (TD-48/49/70–73).

## Anonymous flow normalization (REN-01c)

`FormattingTree::build(&principal, limits, cancellation)` derives a separate,
borrowed formatting forest from the immutable principal snapshot. Source indexes
map element boxes and text back to validated DOM identities; inline continuations
share a source. Anonymous block wrappers have no invented DOM node. Formatting
parents can differ from principal/DOM parents. Flat arenas avoid recursive walks
and recursive destruction, including on an error.

Static non-floating block, flow-root, inline and inline-block elements are
supported. A block container with block children wraps its intervening inline
runs in minimal anonymous blocks; a pure inline context needs no wrapper.
Blocks inside inlines split each inline ancestor into before/after continuations
and lift the block group to the containing block. Consecutive blocks, including
ones separated only by collapsible whitespace, share the split. Empty leading
and trailing inline pieces remain, with explicit original start/end edge flags
for later decoration layout. Inline-blocks remain atomic outside and normalize
inside; block descendants cannot escape that boundary.

Anonymous blocks inherit supported inherited properties from their enclosing
non-anonymous source, with other properties initially valued and `display:block`.
Text is represented by anonymous inline style rather than copying its parent's
background/borders. Initial `currentcolor` and border widths are computed through
the existing value machinery. Custom properties follow registered inheritance
flags; unregistered custom properties inherit. No selectors are rematched.
`display:contents` text still inherits from the DOM parent saved by `BoxTree`.

Only direct text runs consisting entirely of CSS collapsible ASCII whitespace
under normal/nowrap are omitted at block boundaries. Their principal indexes are
reported in `suppressed_whitespace()` and their original UTF-16 remains in the
principal snapshot. Explicit empty inline boxes, preserved whitespace, NBSP,
non-Latin text and lone surrogates remain. General whitespace transformation,
segment-break handling, bidi, shaping, line breaking and inline positions await
REN-03. No text-to-pixel metrics are guessed.

`FormattingLimits` caps all arena allocations (100,000, including temporary text
boxes later omitted), published depth (512 edges from a root), and work
(16,000,000 source/arena visits, sequence entries and UTF-16 units). Limits are
independent of principal-snapshot limits. Sequence bubbling through nested
inlines can revisit entries; every visit consumes work, so expansion cannot run
without a bound (TD-39). Cancellation is checked on work charges, including text
scans. Failure returns no partial formatting tree. The budgets do not promise a
total process memory cap or recovery from allocator exhaustion.

`rustic dump-formatting <file>` uses the same local stylesheet loader as the
other styled dumps. `--max-format-boxes`, `--max-format-depth` and
`--max-format-steps` override its limits. Output contains parent/source/style
indexes, roles, continuation edges and text-unit counts, without page text;
`--max-output-bytes` caps the complete dump. Existing `dump-boxes` and
`dump-layout` fixtures remain unchanged. The block geometry reference still
requires its own static block-only input; normalized inline flow is not yet
connected to used geometry.

Sixteen normalization scenarios cover exact mixed trees, nested/multiple/empty
splits, atomic inline-blocks, whitespace/inheritance/UTF-16, unsupported input,
limits/cancellation/atomicity and deep expansion. A token-level block-group model
checks all masks through six tokens with zero through three inline ancestors
(508 trees), plus parent/preorder, homogeneous child roles and text-order
invariants. A reviewed CLI fixture is checked at chunk sizes 1, 7 and 65,536,
exact output limits, normalization limits and local stylesheet behavior.
The `layout-formatting` fuzz target mutates HTML/styles and checks text
partition/order, UTF-16 preservation, source mapping and child-role invariants.

Specification basis, reviewed 2026-11-08: CSS 2.2
[§9.2.1.1 anonymous blocks](https://www.w3.org/TR/CSS22/visuren.html#anonymous-block-level),
[§9.2.2 inline-level boxes](https://www.w3.org/TR/CSS22/visuren.html#inline-boxes),
CSS Display 3 [anonymous boxes](https://www.w3.org/TR/css-display-3/#anonymous)
and CSS Text 3 [white-space processing](https://www.w3.org/TR/css-text-3/#white-space-phase-1).
These are focused structural fixtures/model checks, not layout-WPT or browser
comparison results. List markers, tables, flex/grid, floats/out-of-flow boxes,
replaced/control/foreign elements and pseudo boxes
remain separate work and fail explicitly where required (TD-37/TD-39).

## Font adapter (REN-03a)

`rustic-text` is a platform-independent, dependency-free safe-Rust crate (the
`rustic-text` row of [ARCHITECTURE.md](ARCHITECTURE.md)). `Face::parse` accepts
a TrueType-flavored sfnt (`0x00010000` or `true`) and rejects CFF (`OTTO`),
collections and WOFF/WOFF2 with typed `FontError::Unsupported` values. It reads
every structure through a checked big-endian reader that reports the table on
truncation. Directory records must lie inside the file and be unique; `head`
(version, magic, units per em 16–16,384, loca format), `maxp` (glyph count),
`hhea` (`numberOfHMetrics` within 1..=glyphs), `hmtx` length, `loca` length,
`name` (typographic then legacy family; bounded length) and `OS/2` (version
sized; weight, width class, `fsSelection`, typo metrics, x-height/cap height
from version 2) are validated at load. The best Unicode `cmap` subtable
(format 12, else format 4) is fully checked: sorted, non-overlapping ranges,
the format 4 terminator, and `idRangeOffset` reads bounded per lookup. Glyph
ids beyond the glyph count count as unmapped.

`Face::outline` decodes simple glyphs (contour end points, repeated flags,
short/same delta coordinates, quadratic on/off-curve points) and flattens
composites with scale, x/y scale and 2×2 transforms, using OpenType's unscaled
offsets unless the font asks for scaled ones. An explicit stack replaces
recursion; `FontLimits` bounds file bytes, table count, points, components and
nesting depth, so reference cycles fail with `LimitExceeded`. Point-matching
composites are explicitly unsupported. Nothing is silently truncated.

`FontDatabase` holds explicitly added faces, generic-family mappings and an
optional fallback family. `select` walks a CSS family list (ASCII
case-insensitive), maps generics, and picks the *first available font* by the
CSS Fonts 4 §5.2 order: stretch, then style (italic → oblique → normal, and so
on), then the weight rule around 400–500. `FontMetrics::em_ratios` gives
x-height (OS/2 `sxHeight`, else the `x` glyph top), cap height (`sCapHeight`,
else the `H` top, else ascent), and the `0` and 水 advances, with the CSS
Values 4 fallbacks of 0.5em, ascent, 0.5em and 1em.

`rustic-style` consumes it through `FontMetricsSource` (implemented for
`FontDatabase`) on `ComputeContext::fonts`; see
[CSS.md](CSS.md#computed-style). Elements whose font properties equal their
parent's reuse its ratios, so a source is queried once per distinct font.
There is still no system font discovery, shaping, glyph cache, line layout or
painted text; the layout references continue to refuse unshaped text.

The bundled faces in `crates/rustic-text/fonts/` are original CC0 fixtures
written by `tools/generate_test_fonts.py` with distinctive metrics (1,000 and
2,048 units per em, OS/2 versions 1 and 4, both loca formats, short and full `hmtx`, one
or both cmap formats, a composite glyph). A Python test checks the committed
binaries, whole-font checksum and directory. They were also cross-checked once
with fontTools in a throwaway environment; that tool is not a dependency.

Specification basis, reviewed 2026-11-09: OpenType 1.9
([sfnt](https://learn.microsoft.com/en-us/typography/opentype/spec/otff),
[cmap](https://learn.microsoft.com/en-us/typography/opentype/spec/cmap),
[glyf](https://learn.microsoft.com/en-us/typography/opentype/spec/glyf),
[OS/2](https://learn.microsoft.com/en-us/typography/opentype/spec/os2)),
CSS Fonts 4 [§5.2 font matching](https://www.w3.org/TR/css-fonts-4/#font-matching-algorithm)
and CSS Values 4 [§6.1.1 font-relative lengths](https://www.w3.org/TR/css-values-4/#font-relative-lengths).
These are focused fixtures, not a font-conformance or WPT result. Remaining
work is tracked in TD-42 and TD-85 (TD-43 is paid).

## Floats, positioning and overflow (REN-02b)

### One top-down walk

`BlockLayout::compute` now lays out each block formatting context (BFC) in one
iterative top-down walk with an explicit frame stack. Each BFC keeps a cursor,
one unresolved margin strut and a queue. The strut resolves at a border or
padding edge, a non-empty box, clearance, or a BFC boundary. Boxes still inside
it get their top at that resolution point. An empty box that can collapse
through takes `cursor + strut` before adding its own bottom margin, as in
REN-02a. All REN-02a margin fixtures, the 192-layout adjacency-graph model and
the prefix-sum model pass unchanged; the old bottom-up `collapse_block` pass was
removed.

BFC roots are the top-level boxes, `display:flow-root`, floats, absolute/fixed
boxes and boxes whose used `overflow` is `hidden`, `scroll` or `auto`. Each BFC
has its own coordinate space with the root's content box at the origin. A box
stores its border origin relative to its parent's child space. Final rectangles
are composed in preorder, so relative offsets move the whole subtree.

### Floats and clearance

Floats are blockified by M2, sized by CSS 2.2 §10.3.5 shrink-to-fit with auto
margins as zero, and laid out as independent BFCs. A float met while the strut
is unresolved waits in the queue. It is placed at the resolution point, which
matches the later adjoining margins, and never above its containing block's
content top. `exclusions.rs` places a margin box as high and then as far to its
side as possible. The top stays at or below every earlier float (rule 6). A band
narrower than the box moves it to the next float bottom. A box wider than an
unintruded containing block keeps its side's edge. `inline-start`/`inline-end`
floats and `clear` values use the containing block's direction. Negative-height
margin boxes exclude nothing.

Clearance (§9.5.2) first places queued floats at `cursor + strut` without
resolving the strut. It then compares the hypothetical border top
`cursor + (strut ∪ margin-top)` with the lowest relevant float bottom. If the
hypothetical position is higher, the preceding strut resolves without the
cleared box's top margin, and the border top moves to the float bottom.
Otherwise the margin collapses normally. Floats clear earlier floats too.

An in-flow BFC root's border box avoids float margin boxes in its BFC; its
margins may overlap them. It narrows inside the free band: a margin smaller than
the float intrusion is raised to it, and auto margins centre within the band. It
moves down to the next float bottom when it cannot fit. A fixed height is
checked over its whole extent before layout. An auto height is checked after
layout. On overlap, the subtree is laid out again at the same top in the band
over its laid-out extent. If that still overlaps without growing, it moves to
the next float bottom. This matches trying layout opportunities in order. Each
same-top retry needs a strictly larger extent, and every retry is charged to the
work budget (TD-44). Auto-height BFC roots grow to contain their floats (§10.6.7); ordinary
blocks do not.

### Relative, absolute and fixed positioning

Relative offsets (§9.4.3) apply after flow layout: `left` wins over `right` in
an LTR containing block and loses in RTL, and `top` wins over `bottom`.
Percentage `top`/`bottom` against an indefinite containing height are treated as
`auto`. Siblings and parent heights ignore the shift.

Absolute boxes use the padding box of the nearest `relative`/`absolute`/`fixed`
ancestor, or the initial containing block. Fixed boxes use the viewport, since
there is no transform or scrolling. They are out of flow: siblings skip them and
margins collapse past them. They are laid out after their containing block in a
FIFO pass, so its final size is known. `position.rs` solves the §10.3.7
horizontal and §10.6.4 vertical equations: all six rules, both-auto margins
(including the negative LTR/RTL case), over-constraint by containing direction,
and max then min reruns. Shrink-to-fit uses block-only intrinsic sizes.
Children resolve percentages against the box's height only when that height
does not depend on content.

The static position is the hypothetical margin edge in the parent's content box:
the left edge for an LTR parent and the right edge for RTL. Its top is the
strut's resolution point (the spec allows an approximation; TD-45). It is mapped
to the containing block along the parent chain, including relative offsets of
intermediate boxes for fixed boxes.

### Overflow without scrolling UI

Overflow values come from M2 after its axis pairing. The root's non-visible
overflow, or else the HTML body's, propagates to the viewport (CSS Overflow 3
§3.3), and that element then uses `visible`. The viewport turns `visible` into
`auto` and `clip` into `hidden`. `viewport_overflow()` reports the result; the
dump adds a `viewport` line only when it is not `auto/auto`.

Each fragment reports its used `overflow()` and the `clip()` that ancestors
apply to it: padding-box intervals on each non-visible axis, intersected along
the containing-block chain. An absolute box therefore escapes the clip of a
non-positioned ancestor between it and its containing block, and fixed boxes
escape every clip. `overflow:clip` clips without making a scroll container, so
margins still collapse through it. Scroll containers report
`scrollable_overflow()`: the union of the padding box and contributing border
boxes, each restricted by any intermediate `clip`. The area is clamped to what a
scroll origin can reach: nothing above the top, nothing before the start edge in
LTR, nothing past the right edge in RTL. The viewport reports the same for the
initial containing block. Layout itself runs at scroll position zero;
`BlockLayout::scrolled` applies viewport and scroll-container positions
afterwards without laying out again (ADR 0064): contents move along the
containing-block chain, fixed boxes stay, sticky boxes re-solve and clips
follow. No scrollbar gutter is reserved, like overlay scrollbars. `overflow-clip-margin` and end-padding
inflation are not implemented (TD-45).

### Limits and evidence

Float placement scans earlier floats, and BFC retries repeat subtrees. Every
visit, float scan and chain step is charged to `MAX_FLOW_WORK` (64,000,000
units) and also polls cancellation. Exceeding it returns `FlowError::WorkLimit`
atomically (TD-44). The former `UnsupportedFlow::Overflow` and sticky
`UnsupportedFlow::Positioning` refusals were removed.

### Sticky positioning

A `position:sticky` box is laid out in normal flow (CSS Positioned Layout 3
§3.4). Layout records a `StickyConstraint` per sticky fragment: the nearest
scroll container (or the viewport), its scrollport at scroll offset zero, the
used insets (percentages against the scrollport), the static border box, the
margins and the containing block's content box. The offset for the initial
scroll position is applied to the fragment and its descendants, and
`StickyConstraint::offset(scroll)` gives the offset at any other position: the
box shifts towards each non-`auto` inset edge of the view rectangle while its
margin box stays inside the containing block, and the start inset wins when
both apply. Sticky boxes are containing blocks for absolute descendants and
always form stacking contexts in paint. Non-atomic inline sticky boxes are
not shifted (TD-71).

`tests/position.rs` has fifteen scenarios with exact geometry:
- relative offsets in LTR and RTL, including percentages;
- every absolute equation rule against a bordered, padded containing block;
- static positions, shrink-to-fit, and definite versus content-based heights;
- the initial containing block, fixed boxes and nested absolute boxes;
- float stacking, wrapping and logical sides;
- clearance with and without collapse;
- float avoidance, including the retry case;
- clip chains, scroll clamping and viewport propagation;
- dump fields;
- sticky boxes against the viewport and a nested scroll container, with top
  and bottom insets, the containing-block limit and absolute descendants.

`sticky.rs` checks the offset for four inset combinations at 501 scroll
positions against an independent scan over every whole-pixel shift.

An independent model rasterizes 10px rows and moves one row at a time. It checks
1,200 float positions from 150 generated eight-float sequences against the
interval-scanning implementation.

Specification basis, reviewed 2026-11-09: CSS 2.2
[§9.3–9.7](https://www.w3.org/TR/CSS22/visuren.html#positioning-scheme),
[§10.3.5/10.3.7](https://www.w3.org/TR/CSS22/visudet.html#float-width),
[§10.6.4/10.6.7](https://www.w3.org/TR/CSS22/visudet.html#abs-non-replaced-height),
CSS Position 3 [relative](https://www.w3.org/TR/css-position-3/#relpos-insets),
CSS Overflow 3 [§2–3](https://www.w3.org/TR/css-overflow-3/#overflow-properties)
and CSS Logical 1 [float/clear](https://www.w3.org/TR/css-logical-1/#float-clear).
These are focused fixtures and models, not a layout-WPT or browser comparison
result.

## Intrinsic and replaced sizing (REN-02c)

### Sizing keywords

`width`, `min-width` and `max-width` accept `min-content`, `max-content`,
`fit-content` and `fit-content(<length-percentage>)` (CSS Sizing 3 §3 and
Sizing 4's bare `fit-content`). The block-axis properties parse the same
keywords and behave as their initial values in a block container (Sizing 3
§3.1). Each node's min- and max-content content-box widths are computed
bottom-up before the walk (`intrinsic.rs`), so a keyword never triggers a
nested layout. `fit-content` clamps the available width between the two; a
`box-sizing:border-box` keyword still names the content box.
`measure_block_width` returns `MeasureError::IntrinsicSizeRequired` for a
keyword, since it has no subtree to measure.

Floats and absolutely positioned boxes use the same contributions for
shrink-to-fit. With `compute_with_fonts`, a block container's inline
formatting context adds its text min- and max-content widths (REN-03b,
below). `compute` still refuses text with `UnsupportedFlow::TextLayout`.

### Replaced elements

`img`, `canvas`, `video`, `iframe`, `embed` and `object` are atomic boxes;
their children produce no fragments. `replaced.rs` implements CSS 2.2
§10.3.2/§10.6.2 with the §10.4 min/max table for `width:auto;height:auto`,
and the 300×150 default object size (CSS Images 3 §5.2). `canvas` reads its
`width`/`height` attributes with HTML's non-negative integer rules. `img` is
0×0 until the embedder passes decoded sizes to `BoxTree::set_natural_sizes`,
which rejects non-replaced nodes atomically (`LayoutError::NotReplaced`).
Nothing is decoded or fetched in layout. Inline-level replaced elements wait
for line layout and stay `UnsupportedFlow::SpecialElement`.

### Evidence

`tests/sizing.rs` has eleven scenarios: every width keyword with min/max
interplay, fit-content in a narrow containing block, contributions through
floats and absolute boxes, block-axis keywords, the public measure error,
flex-item blockification, the natural-size table, auto-margin centring,
floats and positioning of replaced boxes, and embedder sizes. Unit tests
cover the §10.4 table and canvas attribute parsing. The 34 WPT
parse/serialize cases for the keywords now pass (see [CSS.md](CSS.md)).
Approximations are tracked in TD-49.

## Flexbox (REN-08)

### Properties

`rustic-css` parses `flex-direction`, `flex-wrap`, `flex-grow`,
`flex-shrink`, `flex-basis`, `order`, `justify-content`, `align-content`,
`align-items`, `align-self`, `row-gap` and `column-gap`, plus the `flex`,
`flex-flow` and `gap` shorthands. Alignment values keep their `safe`/`unsafe`
overflow position. `rustic-style` blockifies the children of flex
containers (TD-48 covers `display:contents` parents).

### Algorithm

A `display:flex` container is its own formatting context, driven as a state
machine on the container's frame in the same explicit walk (`flow/flex.rs`).
Each phase asks for items to be laid out as formatting roots at a given width
and optional definite height and receives their content heights back; nested
containers are more frames, never recursion. It implements CSS Flexbox 1 §9:
- flex base and hypothetical sizes, including `flex-basis:content` and
  automatic minimum sizes (§4.5);
- line breaking with gaps, and §9.7 flexible length resolution with freezing
  of min/max violations;
- line cross sizes, first/last baseline alignment, `align-content`
  (with `safe` fallback) and stretched lines;
- `align-self`/`align-items` with stretch, which relays out an item at its
  stretched height so children see a definite height;
- auto margins before `justify-content`, and every distribution keyword;
- `row-reverse`, `column-reverse`, `wrap-reverse`, `order` and RTL;
- column containers that flex within a definite height and size to content
  otherwise;
- static positions for absolutely positioned children, and shrink-to-fit
  containers whose intrinsic sizes come from their items.

An item is laid out again only when its size changed or became definite.
Every step charges `MAX_FLOW_WORK`, so worst-case nested relayout fails with
`FlowError::WorkLimit` rather than running long (TD-49). Text directly in a
flex container becomes an anonymous flex item, and items with inline content
take their first and last baselines from their line boxes. `inline-flex`
containers are atomic inlines whose baseline is their first item's. Multi-line
column containers stretch items to their line's cross size.

### Evidence

`tests/flex.rs` has thirteen scenarios with exact geometry: growth and
stretch, justify/align keywords, column flexing, wrap and wrap-reverse,
reversal with RTL and `order`, auto margins, automatic minimum sizes,
baselines, stretched children and replaced items, shrink-to-fit containers
with positioned children, the explicit refusals, 120 nested containers, and
60 generated rows checked for bounds, exact fill and equal growth. The fuzz
corpus has a flex page (`paint-flex.html`) and flex dictionary tokens.

`tests/wpt_flexbox.rs` lays out the 91 css-flexbox pages from the pinned WPT
revision whose only script is `checkLayout(...)`, copied (scripts removed,
`support/` stylesheets inlined) by `tools/import_wpt_flexbox_tests.py`, and
compares every `data-expected-*`/`data-offset-*` attribute as
`check-layout-th.js` would. 40 pages pass. `expected_failures.txt` lists the
other 51 with a reviewed reason; the test fails when a listed page passes or
an unlisted one fails (TD-72).

Specification basis, reviewed 2026-11-09: CSS Flexible Box Layout 1
[§4–§9](https://www.w3.org/TR/css-flexbox-1/#layout-algorithm), CSS Box
Alignment 3 [§4–§8](https://www.w3.org/TR/css-align-3/), CSS Sizing 3
[§3–§5](https://www.w3.org/TR/css-sizing-3/#sizing-values) and CSS 2.2
[§10.3.2/§10.4/§10.6.2](https://www.w3.org/TR/CSS22/visudet.html#inline-replaced-width).
The css-grid comparison below is the second layout-WPT comparison.

## Grid (`layout.grid`, M9)

### Properties

`rustic-css` parses, serializes and computes `grid-template-rows`,
`grid-template-columns` (line names, `repeat()` with counts, `auto-fill`
and `auto-fit`, `minmax()`, `fit-content()`, `fr`), `grid-template-areas`
(validated rectangles), `grid-auto-rows`/`-columns`, `grid-auto-flow`, the
four placement longhands and the `grid-row`, `grid-column`, `grid-area`,
`grid-template` and `grid` shorthands, plus `justify-items`, `justify-self`
(with `legacy`), `place-content`/`-items`/`-self`, `flow-start`/`flow-end`,
overflow positions on `normal` and the `grid-gap` aliases. Track lists keep
their written structure; `repeat()` counts clamp at 10,000.
`grid-template-rows`/`-columns` also take `subgrid` with a line name list
(`[]` entries count, `repeat()` with a count or `auto-fill`). Masonry is not
parsed (TD-209).

### Algorithm

`flow/grid.rs` runs as a formatting context inside the block walk, as flex
does: a grid container's frame owns a run that asks the walk to lay its
items out and receives their content heights, so nesting adds frames, not
recursion. A run:

1. expands the explicit grid (auto repetitions count against the definite
   size, else a `max-*` size, else fill a `min-*` size; areas add tracks
   sized by `grid-auto-*` and `-start`/`-end` line names), resolves each
   item's lines (numbers, names, spans, negative lines, swapped and equal
   lines) and runs the auto-placement algorithm (row or column flow, sparse
   or dense) over `order`-modified document order; implicit tracks before
   the explicit grid run the `grid-auto-*` pattern backwards; empty
   `auto-fit` tracks collapse with their gutters;
2. sizes columns with the track sizing algorithm (CSS Grid 2 §11.3–§11.8):
   initialization, intrinsic sizes for single spans then span groups through
   "distribute extra space" (minimum, min-content and max-content steps for
   base sizes and growth limits), items crossing flexible tracks, maximization,
   "find the size of an fr" and stretching `auto` tracks; contributions come
   from the bottom-up intrinsic sizes, with the automatic minimum size
   clamped by fixed max sizing functions and zero for scroll containers in
   that axis;
3. aligns the columns (`justify-content`, distributed gutters widen spanning
   areas) and lays every item out at its area width (stretched, specified
   or fit-content);
4. sizes rows from those heights the same way; an indefinite height whose
   flexible rows break `min-height`/`max-height` is sized again against the
   bound; then aligns rows and lays out again items whose height depends on
   the row (stretched, or percentages against the grid area);
5. places items with `justify-self`/`align-self` and auto margins; RTL grids
   mirror columns, and `left`/`right` stay physical. Relative offsets
   resolve against the grid area. Items with `align-self: baseline` or
   `last baseline` share a baseline with the others in their start (end)
   row: each moves by its shim, and the shims count in the rows' sizes
   (synthesized from the border box without a line box). The run then
   keeps its line positions and names.

The grid container's intrinsic widths run the column algorithm under min-
and max-content constraints with limited contributions. A grid container's
first (last) baseline comes from its first (last) row: the item taking part
in baseline alignment there, else the first (last) in grid order, so an
`inline-grid` sits on the line by it. Absolutely positioned descendants
whose containing block is a grid container use their grid area (CSS Grid 2
§9.1): their lines resolve against the laid-out grid, and `auto` lines or
lines outside it are the padding edges; a positioned child without insets
starts at its area's start corner (§9.2).
Lines past 10,000 tracks clamp into the last track and placement is
metered by the flow work budget. Absolutely positioned grids carry their
`min-height`/`max-height` into their row sizing, and a `max-*` size below
the `min-*` size yields to it when auto repetitions are counted.

### Subgrid

A grid item that is itself a grid container with `subgrid` in an axis
adopts the tracks it spans in its parent there (CSS Grid 2 §9); without a
grid parent `subgrid` behaves as `none`. The parent registers each subgrid
item with its span, the parent's names for the spanned lines (reversed for
a subgrid against the parent's inline direction) and the parent's gap. The
subgrid's explicit grid in that axis is those tracks, with the inherited
names plus its own line name list; lines clamp to it and it has no implicit
tracks, its content alignment there is `start`, and it always fills its
area. A subgrid placed with an automatic span spans its named lines minus
one.

- Columns: the parent sizes its columns with the subgrid's items in place
  of the subgrid, collected through any depth of nested subgrids from a
  work stack. The subgrid's margin, border and padding add to the
  contributions of items at its edges, and half the difference between its
  gap and its parent's to the other sides; an empty subgrid contributes one
  item with its edges. After aligning its columns the parent hands each
  subgrid its track sizes and starts.
- Rows: a row subgrid records its items' row contributions (its own row
  subgrids' already folded in) whenever it sizes its rows. The parent uses
  them, adjusted the same way, in place of the subgrid's height, then
  lays the subgrid out again at its area height with the sized rows.
- Gaps: a `normal` gap keeps the parent's gutters, distributed space
  included; a subgrid's own gap is centred in each parent gutter, so the
  tracks beside it grow or shrink. The outer tracks lose the subgrid's
  margin, border and padding.

Baseline sharing does not cross subgrid boundaries (TD-209).

### Evidence

`crates/rustic-css/tests/grid.rs` covers valid and invalid values and
shorthand expansion; the pinned WPT `css-grid/parsing` and
`css-align/parsing` cases joined the `css.conformance` gate (2353 of 2724
in-scope cases pass, three grid cases reviewed as failures).
`crates/rustic-layout/tests/grid.rs` has nineteen scenarios with exact
geometry (fixed and flexible tracks with gaps, areas and named lines, auto
placement with spans, dense and column flow, negative lines, intrinsic and
inline grids, row sizing and stretch, content and self alignment, RTL,
`auto-fit` collapse, `order`, abspos `min-height`, clamped lines, anonymous
items, grid-area containing blocks, baseline alignment, inline-grid
baselines, repetitions against min/max sizes, and five subgrid scenarios:
column and row subgrids sizing parent tracks, inherited and own line names
with an own gap, nested subgrids with automatic spans, and an LTR subgrid
in an RTL grid); `flow/grid.rs` unit-tests the sizing and placement
algorithms
directly. `tests/wpt_grid.rs` compares the 168 static `checkLayout` pages
of the pinned css-grid directory (`tools/import_wpt_grid_tests.py`): 79
pass and the other 89 are reviewed failures (TD-210). The pinned
`css-grid/subgrid` parsing cases joined the `css.conformance` gate with
the subgrid step (all 60 pass).

Specification basis, reviewed 2026-11-26: CSS Grid Layout 2
[§6–§11](https://drafts.csswg.org/css-grid-2/) and CSS Box Alignment 3
[§5–§8](https://drafts.csswg.org/css-align-3/).

## Tables (`layout.tables`, M9)

### Properties

`rustic-css` parses and computes `border-collapse`, `border-spacing`,
`table-layout`, `caption-side` and `empty-cells` ([CSS guide](CSS.md)). The
HTML user-agent sheet gives `table` 2px spacing, cells 1px padding and row
groups `vertical-align: middle`.

### Layout

`flow/table.rs` runs a table as a formatting context inside the block walk,
like grid layout: the table's frame queues its captions and cells, each is
laid out as a formatting root at its width, and the table finishes once
every one reported its height. No recursion: nested tables are more frames.

- Structure: header groups display first and footer groups last; rows
  directly in the table form implicit groups. Cells take slots by the HTML
  table processing model: `colspan` 1–1000, `rowspan` 0–65,534 where `0`
  and spans past the group stop at the group's last row. `BoxSnapshot`
  carries the spans of `td`/`th`.
- Columns (CSS 2.2 §17.5.2.2, automatic layout): each column takes its
  cells' largest border-box min-content and max-content contributions;
  spanning cells (by increasing span) widen the columns they cross in
  proportion to their maximums. `<col>`/`<colgroup>` boxes (with `span`)
  give fixed widths as column maximums and percentages; percentage cells
  and columns (capped at a 100% total) take their share of the width
  first and widen a shrink-to-fit table so they get it (a 100% column
  beside others makes it as wide as possible). The table's width is its
  specified width or shrink-to-fit, never below its minimum; space between
  the minimum and maximum interpolates the other columns, and space beyond
  grows auto columns by their maximums (evenly if all are zero), fixed ones
  only when every column is fixed. `table-layout: fixed` with a definite width takes the
  first row's fixed (or percentage) widths and shares the rest among the
  other columns, or with no other columns among the fixed-length ones. A
  cell's minimum is its min-content width or `min-width`, not its `width`;
  floated, inline and absolutely positioned tables are also never narrower
  than their minimum, a table's `max-width` never lowers its contribution
  below it, and an absolutely positioned table grows to its content.
  Cell padding percentages resolve against the row width.
- Rows: the tallest single-row cell, a specified row height (a minimum)
  and baseline-aligned cells' ascent plus descent. Spanning cells, and a
  taller specified table height, grow rows in proportion to their heights.
- Placement: captions above or below the rows at the table's content
  width, `border-spacing` around columns and rows, row groups and rows as
  boxes across the columns, cells relative to their first row, mirrored in
  RTL. `vertical-align` `top`, `middle`, `bottom` and `baseline` (others act
  as `baseline`) moves a cell's content by growing its top padding in the
  used geometry, so the cell's border box fills its slot.
- Anonymous boxes (CSS Tables 3 §3.9.1): the node builder fixes up each
  box's flattened children. In a table, row group or row, runs of other
  content are wrapped in an anonymous row (cells in a table or group) or
  cell (anything else in a row), and whitespace-only runs are dropped;
  column groups keep only columns. Elsewhere, runs of misparented table
  parts (with the whitespace between them) are wrapped in an anonymous
  table, which is fixed up in turn, so a lone `display: table-cell` box
  becomes table, row and cell. Their styles inherit from the box whose
  children they wrap; like anonymous blocks they have no fragment of
  their own. `inline-table` boxes are atomic inlines sized like a table,
  with the first line's baseline.
- Collapsing borders (CSS 2.2 §17.6.2): the table's border and padding
  move into its outer cells and the spacing is dropped. Each grid line's
  winner is resolved among the cells beside it, the rows and row groups
  along it and the table at its edges (`hidden` suppresses, then width,
  then style order, then cell over row over group over table). A cell
  owns its top and left lines, and its bottom and right ones only at the
  table's edges, so every line is painted once; the cell's used border is
  the winner's width, and its fragment names the winning box and side
  (`BlockFragment::collapsed_borders`) so painting uses that style and
  color. This keeps lines crisp but places cells by whole borders rather
  than the specification's half borders (TD-211).
- `visibility: collapse` rows and columns take no space (with their
  spacing); `empty-cells: hide` cells without content paint no background
  or border in the separated model.
- HTML attributes (`border`, `cellpadding`, `cellspacing`, `width`,
  `bgcolor`, `align`, `valign`, ...) become presentational hints
  ([CSS guide](CSS.md#presentational-hints)); the sheet centres `th` and
  `caption` text.
- Without fonts (the block-only reference), anonymous table boxes are not
  generated and misparented parts are refused (TD-211).

### Evidence

`crates/rustic-css/tests/tables.rs` covers the properties, and the pinned
WPT `css-tables/parsing` cases joined the `css.conformance` gate (all 26
pass). `crates/rustic-layout/tests/tables.rs` checks exact geometry for
column and row sizing, slots with `colspan`/`rowspan`, specified and
centred tables, fixed layout, captions, header and footer groups,
specified heights with baseline and bottom alignment, RTL, anonymous
tables, rows and cells, and inline tables; `flow/table.rs` unit-tests the column algorithms. `tests/wpt_tables.rs` compares the 35 static
`checkLayout` pages of the pinned css-tables directory
(`tools/import_wpt_tables_tests.py`, sharing the css-grid importer): 20
pass and the other 15 are reviewed failures (list items, tentative
width-merging rules, collapsing borders, Ahem text,
`height: stretch`; TD-211). Its `offsetParent` also stops at `td`, `th`
and `table` for static elements, as CSSOM View specifies.

Specification basis, reviewed 2026-11-27: CSS 2.2
[§17](https://www.w3.org/TR/CSS22/tables.html), CSS Tables 3
[Editor's Draft](https://drafts.csswg.org/css-tables-3/) and the HTML
[table processing model](https://html.spec.whatwg.org/multipage/tables.html#forming-a-table).

## Writing modes (`text.writing`, M9)

Vertical writing modes reuse the `horizontal-tb` engine by transposition
(`flow/writing.rs`). A *run* is a subtree laid out in one space: the root's,
which takes the principal writing mode (the `body` child's when there is
one, CSS Writing Modes 4 §8), and each box whose writing mode is orthogonal
to its parent's. A run in a vertical mode is laid out on a copy of its
boxes whose styles are transposed into its logical space: its block-start
side becomes `top` and its line-left side `left` for margins, paddings,
insets, borders and corner radii, `width`/`height` (with their minimums and
maximums), `overflow-x`/`-y` and `border-spacing` swap, and the result is
mapped back to physical coordinates. Block flow, floats, positioning,
flex, grid and tables therefore all lay out in every vertical mode, and
percentages resolve against the right axis. `float: left`/`right`,
`text-align` and `caption-side` are line-relative, so they keep their
values.

An orthogonal box starts a nested run, laid out first: inside it the box is
placed at the origin, sized `fit-content` in its inline axis against the
initial containing block's extent in that axis (or its parent's definite
size along it, §7.3), and stays a containing block if positioned. Its
parent run sees it as an empty box of that border-box size, keeping its
margins, positioning and floating; the nested run's fragments are then
grafted in right after that box, in preorder. Once the parent run has
placed the box, the nested run is laid out again in the space its
containing block gives (the padding box of an absolutely positioned box's
containing block, or the content box's extent in the box's block axis), so
percentages resolve, and the parent run is laid out again if the box
changed size. Runs are processed deepest first from an explicit list.

Lines in a vertical run are columns: they stack in the block direction and
text runs down (`sideways-lr`: up) with its glyphs turned sideways
(`GlyphRotation`). In `vertical-rl` and `vertical-lr`, `text-orientation`
sets characters upright instead (CSS Writing Modes 4 §5.1): all of them
with `upright`, and with `mixed` each grapheme cluster whose first
character has UAX #50 `Vertical_Orientation` `U` or `Tu`. Upright runs are
shaped top to bottom (vertical advances from `vmtx`, else one em, and the
`vert`/`vrt2` alternates), become their own text fragments
(`TextFragment::upright`, rotation `None`), and are centered on the em
box with each glyph hanging from its vertical origin. The line's over side is the right one in `vertical-rl`,
`vertical-lr` and `sideways-rl` and the left one in `sideways-lr`; in
`vertical-lr`, where that is not the block-start side, each line's
contents are mirrored within the line so glyph tops face right. Line and
text baselines on vertical lines are x coordinates.

Absolutely positioned tables are re-solved at their content height, so
`bottom` (and, in vertical runs, `right`) offsets account for a table
taller than its specified height.

In `vertical-rl` and `vertical-lr` (unless `text-orientation` is
`sideways`) the dominant baseline is the central one (CSS Inline 3 §4.1):
each font box is centered on it and sideways glyphs sit on the alphabetic
baseline below it, inline-blocks without lines synthesize it halfway
across their margin boxes, and flex and grid items without lines halfway
across their border boxes. A grid container's baseline comes from an item
in its first (last) row that takes part in baseline alignment of either
kind, else the first (last) item in grid order; a scroll container
item's baselines are clamped to its border box (csswg-drafts#7660).
`text-combine-upright: all` sets a text box's characters horizontally as
one upright cluster one em long, centered on the line.

Limits (TD-212): `vertical-align: middle` ignores the central baseline;
`text-combine-upright` does not compress wider compositions or apply in
`sideways-*` modes; a child with the same axis but the opposite block direction (`vertical-lr`
in `vertical-rl`) flows in its parent's direction; an absolutely positioned
box inside an orthogonal box whose containing block is outside it is
placed against the nested run's root; baselines of orthogonal items are
synthesized from their border boxes only; and a flex or grid container that stretches an orthogonal
item does not lay its contents out again.

### Evidence

`crates/rustic-layout/tests/writing.rs` checks exact geometry for
`vertical-rl` and `vertical-lr` roots, orthogonal boxes in both directions,
percentages and positioning against an orthogonal box's containing block,
and text direction, rotation and baselines in `vertical-rl`, `vertical-lr`
and `sideways-lr`. `crates/rustic-raster/tests/paint.rs` rasterizes
sideways and upright glyphs and their underlines. Upright, mixed and
combined text and central and clamped baselines have exact-geometry tests
in `writing.rs` and `grid.rs`. The pinned WPT comparisons gained
pages that need vertical writing modes: css-flexbox 49 of 91 (was 40),
css-grid 103 of 168 (was 79) and css-tables 20 of 35 (was 18).

Specification basis, reviewed 2026-11-27: CSS Writing Modes 4
[§6–8](https://drafts.csswg.org/css-writing-modes-4/#abstract-box) and CSS
Logical Properties 1.

## Inline formatting and text fragments (REN-03b)

`BlockLayout::compute_with_fonts(tree, viewport, limits, &fonts, cancel)` lays
out text. `BlockLayout::compute` keeps refusing text with
`UnsupportedFlow::TextLayout`, so callers without a font database (the CLI
dumps, paint and fuzzers for now) behave as before.

### Building an inline formatting context

A block container whose children include text, `display:inline` boxes, `<br>`
or `<wbr>` gets one inline formatting context (`inline.rs`). It is built once,
independent of the available width, and reused by intrinsic sizing, float
retries and line breaking:

- **Mixed content.** Inline runs beside block-level siblings are wrapped in
  anonymous block boxes (CSS 2.2 §9.2.1.1). Runs of collapsible whitespace
  alone are dropped. Anonymous blocks have no `BlockFragment`; their lines
  report the nearest real ancestor.
- **White space** (CSS Text 3 §4.1.1). Phase I collapses spaces and tabs, and
  removes them around segment breaks, across element boundaries, for
  `normal`/`nowrap`/`pre-line`. Segment breaks become spaces except under
  `pre`/`pre-wrap`/`break-spaces`/`pre-line`, where they are forced breaks.
  `<br>` is a forced break. `<wbr>` is a zero-width break opportunity.
  Tabs advance to the next multiple of 8 spaces (`tab-size` is not parsed).
- **Text transform.** `uppercase`, `lowercase` and `full-width` change the
  shaped characters but not the source ranges, so `ß` → `SS` is two glyphs
  over one UTF-16 unit.
- **Bidi.** Every IFC is one UAX #9 paragraph with the container's
  `direction` as its base level. Inline boxes do not yet open embeddings or
  isolates for `unicode-bidi`.
- **Breaks.** UAX #14 opportunities over the whole IFC, with the
  `white-space` of the nearest common ancestor of the two sides deciding
  whether wrapping is allowed. `break-spaces` breaks after each space.
- **Shaping.** Runs split at bidi level, style and box boundaries are shaped
  with each style's `font-family` fallback chain ([TEXT.md](TEXT.md#opentype-shaping-and-positions)).
  Inline box margins, borders and padding become zero-height edge pieces.

### Line boxes

`lines.rs` breaks greedily: each line takes as many pieces as fit, breaking at
the last opportunity, and an unbreakable piece wider than the line overflows
it. For each line:

- **Floats.** The line's width comes from the BFC's float band at its top.
  If the first piece cannot fit, the line moves down to the next float bottom.
  If the line turns out taller than the band it was measured in, it is
  re-measured (up to four times).
- **Trailing spaces.** Collapsible spaces at line ends are removed and
  `pre-wrap` spaces hang. Leading collapsible spaces after a soft break are
  dropped.
- **Alignment.** `text-indent` (percentages of the container width) applies
  to the first line. `text-align` `start`/`end`/`left`/`right`/`center`
  resolve against `direction`. `justify` distributes free space over the
  justifiable spaces of every line but the last and lines ending in forced
  breaks.
- **Bidi order.** Levels come from UAX #9 L1 for the line's range. Pieces are
  reordered with L2, and consecutive glyphs of one text box, face and level
  merge into one `TextFragment`.
- **Height.** Each inline box contributes its font's ascent and descent plus
  half-leading from its `line-height` (`normal` uses the face's ascent +
  descent + line gap). The root's strut always counts (CSS 2.2 §10.8).
  `vertical-align` supports `baseline`, `sub`, `super`, lengths,
  percentages of `line-height`, `middle`, `text-top`, `text-bottom`, `top` and
  `bottom`; `top`/`bottom` subtrees are aligned after the rest of the line is
  sized.
- **Relative boxes.** `position:relative` on inline boxes shifts their
  fragments and those of their descendants.

### Fragments and sizing

`BlockLayout::lines()` gives `LineFragment{block, rect, baseline}`. `texts()`
gives `TextFragment`s: their line, node, UTF-16 source range, rect, baseline,
bidi level, face index, font size and glyphs with absolute pen positions.
Forced breaks and invisible break hints produce no text fragment.
`inlines()` gives one `InlineFragment` per inline box per line, with border,
padding and content rectangles and whether the box starts or ends on that
line. Edges are drawn only where the box starts or ends, in visual order. The
layout dump adds `line`, `text` and `inline` rows.

The IFC also seeds shrink-to-fit: min-content breaks at every opportunity,
and max-content only at forced breaks. Floats and absolutely positioned boxes
therefore size to their text.

### Limits and evidence

Collection, shaping, breaking, placement and conversion are charged to the
same `MAX_FLOW_WORK` budget and poll cancellation. Text over
`LayoutLimits::max_text_units` returns `FlowError::TextLimit`. Unsupported
inline content fails the whole layout atomically:

- `inline-grid` and `inline-table`;
- native form controls and other replaced content without a natural size.

Remaining approximations are listed under TD-50 and TD-70.

### Atomic inlines and blocks inside inline boxes

`inline-block`, `inline-flex` and inline replaced elements with a natural
size (such as an `<img>` with a decoded size) are atomic inlines (CSS 2.2
§9.2.2, §10.8). Each is laid out first as a formatting root, then enters line
layout as one U+FFFC piece that breaks like an ideograph, so atomic inlines
wrap between each other. Its baseline is its last line box, the first item's
baseline for `inline-flex`, and the bottom margin edge for replaced elements,
boxes with `overflow` other than `visible` and boxes without lines.
`vertical-align` places it like any inline box. Its min-/max-content
contributions feed intrinsic sizing of the surrounding block. An atomic
inline's block fragment records its line (`BlockFragment::line`).

A block inside an inline box splits the inline box around it (§9.2.1.1): the
inline content before and after goes into anonymous blocks, and the inline
box's start edge is on its first part and its end edge on its last. Floats
and absolutely positioned boxes inside inline boxes split them the same way
(TD-70).

### Fragment tree

`BlockLayout::fragment_tree` builds the general fragment tree over block,
line, inline-box and text fragments in preorder, without recursion: each node
has its kind, parent, depth, subtree end and border rectangle, and `dump`
prints it with a byte limit. Lines hang off their block, inline boxes and
texts off their line or enclosing inline box, and atomic inlines sit inside
their inline box on their line.

`tests/atomic.rs` has fourteen scenarios with exact geometry (baselines,
shrink-to-fit, `vertical-align`, wrapping, intrinsic sizes, `inline-flex`,
nesting and RTL, block-in-inline splits, refusals and flex baselines), and
`tests/fragments.rs` five (nesting, atomic inlines, floats and positioned
boxes, 200-deep trees and block-only layouts).

`tests/inline.rs` has 18 scenarios with exact geometry over the bundled test
fonts:

- wrapping and trailing-space removal, overflow and `nowrap`;
- white-space collapsing across elements and the preserving modes, `<br>` and
  `<wbr>`;
- every `text-align` value, `text-indent` and justify;
- inline box edges across lines;
- line-height and each `vertical-align` group;
- RTL and mixed-direction reordering;
- float bands and moving below floats;
- anonymous blocks and shrink-to-fit;
- spacing and transforms;
- relative inline boxes;
- atomic failures, cancellation and the text limit;
- the dump rows;
- sizing keywords, flex items with text, and refusals for text directly in
  flex containers.

An independent greedy model checks line breaks for generated word sequences
against the implementation.

Specification basis, reviewed 2026-11-09: CSS 2.2
[§9.2.1.1, §9.4.2](https://www.w3.org/TR/CSS22/visuren.html#inline-formatting)
and [§10.8](https://www.w3.org/TR/CSS22/visudet.html#line-height),
CSS Text 3 [§4–§8](https://www.w3.org/TR/css-text-3/#white-space-processing)
and CSS Writing Modes 4
[§2](https://www.w3.org/TR/css-writing-modes-4/#text-direction).
These are focused fixtures and models, not a layout-WPT or browser comparison
result.


## REN-03 completion update

The earlier font/inline sections describe the initial slices. CFF/CFF2, WOFF1/2,
host-granted discovery, OpenType/Arabic/Indic shaping, mark offsets, scoped bidi,
CSS wrapping and manual/language-gated automatic hyphenation are now integrated.
Absolute `PositionedGlyph.y` carries vertical offsets and translations into
paint. Min-content includes anywhere/hyphen opportunities and excludes emergency
break-word. [TEXT.md](TEXT.md) is the current complete text contract; TD-85/86/87
record the limits still pending. No native service or full CSS Text conformance
claim follows from the static-renderer milestone.
