//! Atomic inlines (`inline-block`, `inline-flex`, inline replaced elements)
//! and block-level boxes inside inline boxes (CSS 2.2 §9.2.1.1, §9.2.2,
//! §10.3.9, §10.8.1).
//!
//! Bundled "Rustic Test" metrics at 10px: ascent 8, descent 2, no line gap,
//! so a line of plain text is 10px tall with its baseline at 8; `x` is 5px
//! wide and a space 2.5px.

use std::sync::Arc;

use rustic_base::CancellationToken;
use rustic_layout::{BlockLayout, BoxTree, CssRect, CssSize, FlowError, LayoutLimits};
use rustic_style::ComputeContext;
use rustic_text::bundled::test_database;

mod common;

const RESET: &str = "* { margin:0; font-size:10px; } .ib { display:inline-block } ";

fn try_layout(html: &str, css: &str, width: f64) -> Result<BlockLayout, FlowError> {
    let fonts = test_database();
    let context = ComputeContext::with_fonts(Arc::new(test_database()));
    let (doc, styles) = common::page_with_context(html, &format!("{RESET}{css}"), context);
    let tree = BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    BlockLayout::compute_with_fonts(
        &tree,
        CssSize::new(width, 400.0).unwrap(),
        LayoutLimits::default(),
        &fonts,
        &CancellationToken::default(),
    )
}

fn layout(html: &str, css: &str, width: f64) -> BlockLayout {
    try_layout(html, css, width).unwrap()
}

fn r(rect: CssRect) -> (f64, f64, f64, f64) {
    (
        rect.origin().x.get(),
        rect.origin().y.get(),
        rect.size().width().get(),
        rect.size().height().get(),
    )
}

/// Border box of public fragment `i` (0 html, 1 body, 2 the first block).
fn b(l: &BlockLayout, i: usize) -> (f64, f64, f64, f64) {
    r(l.fragments()[i].border())
}

fn lines(l: &BlockLayout) -> Vec<((f64, f64, f64, f64), f64)> {
    l.lines()
        .iter()
        .map(|line| (r(line.rect()), line.baseline().get()))
        .collect()
}

fn text_x(l: &BlockLayout) -> Vec<(usize, f64, f64)> {
    l.texts()
        .iter()
        .map(|t| (t.line(), t.rect().origin().x.get(), t.baseline().get()))
        .collect()
}

#[test]
fn empty_inline_block_sits_on_the_baseline_with_its_bottom_margin_edge() {
    let l = layout(
        "<p>xx<span class=ib id=a></span>xx",
        "#a { width:20px; height:30px }",
        100.0,
    );
    // No line boxes inside: the baseline is the bottom margin edge, so the
    // box rises 30px above the root baseline and the line grows to 32px.
    assert_eq!(lines(&l), [((0.0, 0.0, 100.0, 32.0), 30.0)]);
    assert_eq!(b(&l, 3), (10.0, 0.0, 20.0, 30.0));
    assert_eq!(l.fragments()[3].line(), Some(0));
    assert_eq!(l.fragments()[3].parent(), Some(2));
    assert_eq!(text_x(&l), [(0, 0.0, 30.0), (0, 30.0, 30.0)]);
    assert_eq!(b(&l, 2), (0.0, 0.0, 100.0, 32.0));
    assert!(l.dump(1 << 16).unwrap().contains(" line=0\n"));
}

#[test]
fn inline_block_baseline_is_its_last_line_and_its_width_shrinks_to_fit() {
    let l = layout("<p>x<span class=ib>xx<br>xxx</span>x", "", 100.0);
    // Shrink-to-fit: the widest line, "xxx" = 15px; two 10px lines. The
    // last line's baseline (18) aligns with the root baseline.
    assert_eq!(b(&l, 3), (5.0, 0.0, 15.0, 20.0));
    // Lines: the paragraph's own line first, then the inline-block's two.
    let rows = lines(&l);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0], ((0.0, 0.0, 100.0, 20.0), 18.0));
    assert_eq!(rows[1].0, (5.0, 0.0, 15.0, 10.0));
    assert_eq!(rows[2], ((5.0, 10.0, 15.0, 10.0), 18.0));
    assert_eq!(l.lines()[1].block(), 3);
    // "x" before and after, both on the paragraph's baseline.
    let outer: Vec<_> = text_x(&l).into_iter().filter(|t| t.0 == 0).collect();
    assert_eq!(outer, [(0, 0.0, 18.0), (0, 20.0, 18.0)]);
}

#[test]
fn overflow_and_replaced_elements_use_the_bottom_margin_edge() {
    let l = layout(
        "<p>x<span class=ib id=a>xx</span>",
        "#a { overflow:hidden; margin-bottom:4px }",
        100.0,
    );
    // 10px of content plus a 4px bottom margin, all above the baseline.
    assert_eq!(lines(&l)[0], ((0.0, 0.0, 100.0, 16.0), 14.0));
    assert_eq!(b(&l, 3), (5.0, 0.0, 10.0, 10.0));
    // A canvas is an inline replaced element with its attribute size.
    let l = layout("<p>x<canvas width=20 height=12></canvas>x", "", 100.0);
    assert_eq!(lines(&l), [((0.0, 0.0, 100.0, 14.0), 12.0)]);
    assert_eq!(b(&l, 3), (5.0, 0.0, 20.0, 12.0));
    assert_eq!(text_x(&l)[1], (0, 25.0, 12.0));
}

#[test]
fn vertical_align_positions_atomic_inlines() {
    let html = "<p>x<span class=ib id=a></span>";
    let css = |align: &str| format!("#a {{ width:10px; height:20px; vertical-align:{align} }}");
    // (line height, inline-block top)
    for (align, height, top) in [
        // Bottom edge 20px above the baseline at 20.
        ("baseline", 22.0, 0.0),
        // Top aligned with the line top; the strut needs only 10px.
        ("top", 20.0, 0.0),
        // Bottom aligned with the line bottom.
        ("bottom", 20.0, 0.0),
        // Top aligned with the parent's text top (ascent 8 above baseline).
        ("text-top", 20.0, 0.0),
        // Raised 5px: its bottom edge is 25px above the baseline.
        ("5px", 27.0, 0.0),
    ] {
        let l = layout(html, &css(align), 100.0);
        assert_eq!(lines(&l)[0].0.3, height, "{align}");
        assert_eq!(b(&l, 3).1, top, "{align}");
    }
    // `middle`: the box's midpoint sits half an x-height above the baseline.
    let l = layout(html, &css("middle"), 100.0);
    let baseline = l.lines()[0].baseline().get();
    let mid = b(&l, 3).1 + 10.0;
    let x_height = 10.0 * 0.5;
    assert!(
        ((baseline - x_height / 2.0) - mid).abs() < 1e-9,
        "{baseline} {mid}"
    );
}

#[test]
fn atomic_inlines_break_like_ideographs_and_wrap() {
    let l = layout(
        "<div><span class=ib></span><span class=ib></span><span class=ib></span></div>",
        ".ib { width:20px; height:10px }",
        50.0,
    );
    // Two fit on the first line (no spaces between them), one wraps.
    assert_eq!(b(&l, 3), (0.0, 0.0, 20.0, 10.0));
    assert_eq!(b(&l, 4), (20.0, 0.0, 20.0, 10.0));
    assert_eq!(b(&l, 5), (0.0, 12.0, 20.0, 10.0));
    let lines: Vec<_> = l.fragments()[3..].iter().map(|f| f.line()).collect();
    assert_eq!(lines, [Some(0), Some(0), Some(1)]);
    // `white-space:nowrap` keeps them on one overflowing line.
    let l = layout(
        "<div><span class=ib></span><span class=ib></span><span class=ib></span></div>",
        ".ib { width:20px; height:10px } div { white-space:nowrap }",
        50.0,
    );
    assert_eq!(b(&l, 5), (40.0, 0.0, 20.0, 10.0));
}

#[test]
fn atomic_inlines_feed_intrinsic_sizes() {
    let html = "<div id=f><span class=ib id=a></span> <span class=ib id=b></span></div>";
    let css = "#a { width:20px; height:5px } #b { width:30px; height:5px; margin-left:4px }";
    // A float shrinks to the max-content line: 20 + 2.5 (space) + 4 + 30.
    let l = layout(html, &format!("{css} #f {{ float:left }}"), 200.0);
    assert_eq!(b(&l, 2).2, 56.5);
    // min-content: the widest atomic contribution, margins included.
    let l = layout(html, &format!("{css} #f {{ width:min-content }}"), 200.0);
    assert_eq!(b(&l, 2).2, 34.0);
    // The second box wraps: its bottom edge sits on the 2nd baseline (18).
    assert_eq!(b(&l, 4), (4.0, 13.0, 30.0, 5.0));
    // Percentage widths resolve against the containing block.
    let l = layout(
        "<div><span class=ib id=a></span></div>",
        "#a { width:25%; height:1px }",
        200.0,
    );
    assert_eq!(b(&l, 3).2, 50.0);
}

#[test]
fn inline_flex_uses_its_first_baseline_and_lays_out_items() {
    let l = layout(
        "<p>x<span id=f><u>xx</u><u>xx<br>xx</u></span>",
        "#f { display:inline-flex; align-items:flex-start }",
        100.0,
    );
    // Items 10px and 10px wide; the container is 20 wide and 20 tall. Its
    // first baseline (8) aligns with the paragraph's.
    assert_eq!(b(&l, 3), (5.0, 0.0, 20.0, 20.0));
    assert_eq!(b(&l, 4), (5.0, 0.0, 10.0, 10.0));
    assert_eq!(b(&l, 5), (15.0, 0.0, 10.0, 20.0));
    assert_eq!(lines(&l)[0], ((0.0, 0.0, 100.0, 20.0), 8.0));
}

#[test]
fn nested_and_relative_atomic_inlines() {
    let l = layout(
        "<p>x<span class=ib id=o>x<span class=ib id=i>xx</span></span>",
        "#i { position:relative; left:1px; top:2px; padding:1px }",
        100.0,
    );
    // Inner: 10x10 content + 1px padding; baseline 1 + 8 = 9 from its top.
    // Outer line: its strut (8 above) and the inner box (9 above, 3 below).
    let inner = b(&l, 4);
    assert_eq!(inner, (5.0 + 5.0 + 1.0, 2.0, 12.0, 12.0));
    assert_eq!(b(&l, 3), (5.0, 0.0, 17.0, 12.0));
    assert_eq!(l.fragments()[4].parent(), Some(3));
    assert_eq!(lines(&l)[0].1, 9.0);
}

#[test]
fn rtl_lines_place_atomic_inlines_visually() {
    let l = layout(
        "<p dir=rtl>xx<span class=ib id=a></span>",
        "#a { width:20px; height:10px }",
        100.0,
    );
    // Right to left: "xx" at the right edge, the box to its left.
    assert_eq!(b(&l, 3), (70.0, 0.0, 20.0, 10.0));
    assert_eq!(text_x(&l)[0].1, 90.0);
}

#[test]
fn blocks_inside_inline_boxes_split_them() {
    let l = layout(
        "<section>x<span id=s>xx<div id=d>xxx</div>xx</span>x</section>",
        "#s { border-left:2px solid; border-right:3px solid }",
        100.0,
    );
    // Fragments: html, body, section, div (anonymous blocks have none).
    assert_eq!(l.fragments().len(), 4);
    assert_eq!(b(&l, 3), (0.0, 10.0, 100.0, 10.0));
    assert_eq!(b(&l, 2), (0.0, 0.0, 100.0, 30.0));
    let rows: Vec<_> = lines(&l).into_iter().map(|(r, _)| r.1).collect();
    assert_eq!(rows, [0.0, 10.0, 20.0]);
    // The span's first part has only its start edge, the last only its end.
    let spans: Vec<_> = l
        .inlines()
        .iter()
        .map(|f| {
            (
                f.line(),
                r(f.border()).0,
                r(f.border()).2,
                f.starts(),
                f.ends(),
            )
        })
        .collect();
    assert_eq!(
        spans,
        [(0, 5.0, 12.0, true, false), (2, 0.0, 13.0, false, true)]
    );
    // Lines in the anonymous blocks belong to the paragraph.
    assert_eq!(l.lines()[0].block(), 2);
    assert_eq!(l.lines()[1].block(), 3);
    assert_eq!(l.lines()[2].block(), 2);
}

#[test]
fn nested_split_inline_boxes_reopen_in_order() {
    let l = layout(
        "<div><u><s>x<p>x</p>x</s></u></div>",
        "p { height:5px }",
        100.0,
    );
    // div, p: anonymous block (10px line), p (5px), anonymous block.
    assert_eq!(b(&l, 3), (0.0, 10.0, 100.0, 5.0));
    assert_eq!(b(&l, 2).3, 25.0);
    // u and s each get a fragment on both lines, nested in source order.
    let lines: Vec<_> = l
        .inlines()
        .iter()
        .map(|f| (f.line(), f.starts(), f.ends()))
        .collect();
    assert_eq!(
        lines,
        [
            (0, true, false),
            (0, true, false),
            (2, false, true),
            (2, false, true)
        ]
    );
}

#[test]
fn unsupported_atomic_content_still_fails_atomically() {
    use rustic_layout::UnsupportedFlow;
    // `inline-grid` is laid out since `layout.grid` (tests/grid.rs).
    let html = "<p>x<span style='display:inline-table'></span>";
    match try_layout(html, "", 100.0) {
        Err(FlowError::Unsupported { reason, .. }) => {
            assert_eq!(reason, UnsupportedFlow::FormattingContext, "{html}");
        }
        other => panic!("{html}: {other:?}"),
    }
}

#[test]
fn without_fonts_atomic_inlines_are_refused() {
    let (doc, styles) = common::page("<div><span class=ib></span></div>", RESET);
    let tree = BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    let result = BlockLayout::compute(
        &tree,
        CssSize::new(100.0, 100.0).unwrap(),
        LayoutLimits::default(),
        &CancellationToken::default(),
    );
    assert!(
        matches!(result, Err(FlowError::Unsupported { .. })),
        "{result:?}"
    );
}

#[test]
fn flex_baseline_alignment_uses_first_line_boxes() {
    let l = layout(
        "<main><div id=a>x</div><div id=b>x<br>x</div><div id=c></div></main>",
        "main { display:flex; align-items:baseline } #b { padding-top:10px } \
         #c { height:30px; width:5px }",
        100.0,
    );
    // First baselines below the margin-box tops: a 8, b 10 + 8 = 18; c has
    // no line, so its border-box bottom (30) is synthesized. All align at 30.
    assert_eq!(b(&l, 3).1, 22.0);
    assert_eq!(b(&l, 4).1, 12.0);
    assert_eq!(b(&l, 5).1, 0.0);
    // The line is as tall as the deepest descent below the shared baseline.
    assert_eq!(b(&l, 2).3, 42.0);
    // Text items' own line baselines land on the shared baseline.
    let baselines: Vec<f64> = l.texts().iter().map(|t| t.baseline().get()).collect();
    assert_eq!(baselines, [30.0, 30.0, 40.0]);
}
