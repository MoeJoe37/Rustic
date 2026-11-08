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
Visible shadow hosts fail explicitly until flattened-tree/slot assignment exists.

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

## Initial vertical block reference (REN-01b, partial)

`BlockLayout::compute(tree, viewport, limits, cancellation)` now produces an
immutable preorder list of physical `BlockFragment`s: source node/box, parent
fragment and checked content, padding and border rectangles. It consumes the
existing snapshot and never reads or mutates the live DOM. An explicit stack
opens blocks top down and resolves auto heights bottom up; a fixed-height
parent advances following siblings by its own used border height, even when
its children visibly overflow it. Widths use the REN-01a reference algorithm.

Supported height behavior includes fixed and automatic content heights,
min/max-height (minimum wins), border-box subtraction, percentages and calc.
A percentage height resolves against a definite containing content height,
including min/max adjustment; percentages against an automatic containing
height stay automatic rather than using its eventual height. Percentage
min/max bounds on indefinite containing heights use their initial bounds.
Styles must be recomputed for a changed viewport before layout when viewport
units/media conditions change; layout itself re-resolves containing-block
percentages. The geometry test exercises both resize paths.

The accepted subset is explicit (TD-38): static non-replaced block/flow-root
boxes, visible overflow and **zero vertical margins**. Horizontal margins
(including auto/negative values) remain supported. Pure CSS collapsible
whitespace under normal/nowrap produces no fragment. Preserved whitespace,
non-whitespace text, inline/list/flex/grid/table contexts, positioning/floats,
nonzero vertical margins, overflow clipping/scrolling, replaced elements,
native controls, special HTML layout and foreign content fail atomically with
a typed reason. Empty inline boxes are not silently treated as empty blocks.
No shaping metrics, anonymous line boxes or margin-collapse approximations are
invented. This is useful for inspecting controlled nested block fixtures; it
is not yet arbitrary article layout.

Layout checks its own node/fragment/depth/text limits, cancellation during
traversal, long whitespace runs and frame completion, and geometry bounds
before returning a result. Failure leaves the input snapshot and any earlier
layout unchanged. Dumps check the full next line before appending. The
reference uses no content-controlled recursion and no platform dependency.

Fourteen flow scenarios cover exact nested geometry, overflow and sibling
positions, definite/indefinite percentage heights, min/max and box sizing,
calc, RTL, resize, whitespace semantics, unsupported content, atomic failure,
exact budgets, coordinate overflow, an independent 64-block prefix-sum model
and a 200-level auto-height chain. This complements the existing thirteen
geometry/snapshot/width tests. No full block-flow or fragment catalog step is
credited until its remaining algorithms land.

Specification basis, reviewed 2026-11-08: CSS 2.2
[normal flow](https://www.w3.org/TR/CSS22/visuren.html#normal-flow),
[§10.5–10.7 heights](https://www.w3.org/TR/CSS22/visudet.html#the-height-property)
and [margin collapse](https://www.w3.org/TR/CSS22/box.html#collapsing-margins).
Margin collapse is deliberately refused, not claimed implemented. These are
focused reference cases, not a layout-WPT result.

## Inspect a local fixture

```sh
cargo run --locked -p rustic-cli -- dump-boxes crates/rustic-cli/tests/fixtures/boxes.html
cargo run --locked -p rustic-cli -- dump-layout crates/rustic-cli/tests/fixtures/layout.html
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
unshaped text fails with empty stdout. All dump commands remain local-only.

## Next units

1. Finish REN-01b: normalize principal/anonymous boxes and mixed inline/block
   content, then expand the initial block geometry reference.
2. REN-02: intrinsic/replaced sizing, margin collapse, overflow, positioning and floats.
3. REN-03/04: font adapter, shaping/bidi/line layout, fragments and paint order.
4. REN-05/06/07: CPU raster reference, bounded decode, PNG `render` and reftests.

No image output or whole REN-01 completion is claimed by this first slice.
