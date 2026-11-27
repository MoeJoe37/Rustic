//! REN-03b: inline formatting contexts, line boxes, baselines and text
//! fragments with the bundled test fonts.
//!
//! Bundled "Rustic Test" Regular metrics (1,000 units per em): ascent 800,
//! descent 200, no line gap; advances: space 250, `x` 500, `H` 650, `0` 550,
//! `w` 1,000 and `.notdef` 500. At 10px: `x` = 5px, space = 2.5px.

use std::sync::Arc;

use rustic_base::CancellationToken;
use rustic_layout::{BlockLayout, BoxTree, CssRect, CssSize, FlowError, LayoutLimits};
use rustic_style::ComputeContext;
use rustic_text::bundled::test_database;

mod common;

const RESET: &str = "* { margin:0; font-size:10px; } ";

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

fn line_rects(l: &BlockLayout) -> Vec<(f64, f64, f64, f64)> {
    l.lines().iter().map(|line| r(line.rect())).collect()
}

fn dump(html: &str, css: &str, width: f64) -> String {
    layout(html, css, width).dump(1 << 20).unwrap()
}

/// (line, UTF-16 source range, rect) of each text fragment.
type TextRow = (usize, std::ops::Range<usize>, (f64, f64, f64, f64));

fn texts(l: &BlockLayout) -> Vec<TextRow> {
    l.texts()
        .iter()
        .map(|t| (t.line(), t.text(), r(t.rect())))
        .collect()
}

#[test]
fn words_wrap_at_spaces_and_trailing_spaces_are_removed() {
    let l = layout("<p>xx xx xxx", "", 30.0);
    // "xx xx" is 10 + 2.5 + 10; the space before "xxx" is dropped.
    assert_eq!(
        line_rects(&l),
        [(0.0, 0.0, 30.0, 10.0), (0.0, 10.0, 30.0, 10.0)]
    );
    assert_eq!(
        texts(&l),
        [
            (0, 0..5, (0.0, 0.0, 22.5, 10.0)),
            (1, 6..9, (0.0, 10.0, 15.0, 10.0)),
        ]
    );
    assert_eq!(l.lines()[0].baseline().get(), 8.0);
    // The block's auto height is the sum of its line boxes.
    assert_eq!(r(l.fragments()[2].content()).3, 20.0);
    let glyphs: Vec<f64> = l.texts()[0].glyphs().iter().map(|g| g.x).collect();
    assert_eq!(glyphs, [0.0, 5.0, 10.0, 12.5, 17.5]);
}

#[test]
fn an_unbreakable_word_overflows_and_nowrap_keeps_one_line() {
    let l = layout("<p>xxxxxxxx xx", "", 20.0);
    assert_eq!(texts(&l)[0].2, (0.0, 0.0, 40.0, 10.0));
    assert_eq!(l.lines().len(), 2);
    let l = layout("<p style='white-space:nowrap'>xx xx xx xx", "", 20.0);
    assert_eq!(l.lines().len(), 1);
    assert_eq!(texts(&l)[0].2.2, 47.5);
    // A nowrap span inside normal text still allows breaks at its edges.
    let l = layout(
        "<p>xx <span style='white-space:nowrap'>xx xx</span> xx",
        "",
        30.0,
    );
    assert_eq!(l.lines().len(), 3);
}

#[test]
fn white_space_collapses_across_elements_and_preserves_where_asked() {
    // Collapsible spaces collapse across inline boundaries and at line edges.
    let l = layout("<p>  x  <span>  x</span>  ", "", 100.0);
    assert_eq!(
        texts(&l).iter().map(|t| t.2).collect::<Vec<_>>(),
        [(0.0, 0.0, 7.5, 10.0), (7.5, 0.0, 5.0, 10.0)]
    );
    // pre keeps spaces and newlines; a tab advances to the next 8-space stop.
    let l = layout("<p style='white-space:pre'>x  x\n\txx", "", 20.0);
    assert_eq!(
        texts(&l),
        [
            (0, 0..5, (0.0, 0.0, 15.0, 10.0)),
            (1, 5..8, (0.0, 10.0, 30.0, 10.0)),
        ]
    );
    // pre-line keeps newlines but collapses spaces around them.
    let l = layout("<p style='white-space:pre-line'>x   \n   x", "", 100.0);
    assert_eq!(l.lines().len(), 2);
    assert_eq!(texts(&l)[1].2, (0.0, 10.0, 5.0, 10.0));
    // pre-wrap spaces hang at the end of a line instead of wrapping alone.
    let l = layout("<p style='white-space:pre-wrap'>xx      xx", "", 20.0);
    assert_eq!(l.lines().len(), 2);
    assert_eq!(texts(&l)[0].2, (0.0, 0.0, 25.0, 10.0));
    // break-spaces may break between preserved spaces.
    let l = layout("<p style='white-space:break-spaces'>xx      xx", "", 20.0);
    assert_eq!(l.lines().len(), 2);
    assert_eq!(texts(&l)[0].2, (0.0, 0.0, 20.0, 10.0));
}

#[test]
fn br_and_wbr_break_lines() {
    let l = layout("<p>xx<br>x<br><br>x", "", 100.0);
    assert_eq!(l.lines().len(), 4);
    assert_eq!(line_rects(&l)[3], (0.0, 30.0, 100.0, 10.0));
    let l = layout("<p>xxxx<wbr>xxxx", "", 25.0);
    assert_eq!(
        texts(&l).iter().map(|t| (t.0, t.2.2)).collect::<Vec<_>>(),
        [(0, 20.0), (1, 20.0)]
    );
}

#[test]
fn text_align_indent_and_justify() {
    let widths = |css: &str| -> Vec<f64> {
        let l = layout("<p>xx xx<br>x", css, 40.0);
        l.texts()
            .iter()
            .filter(|t| !t.glyphs().is_empty())
            .map(|t| t.rect().origin().x.get())
            .collect()
    };
    assert_eq!(widths("p { text-align:left }"), [0.0, 0.0]);
    assert_eq!(widths("p { text-align:right }"), [17.5, 35.0]);
    assert_eq!(widths("p { text-align:center }"), [8.75, 17.5]);
    assert_eq!(widths("p { text-align:end; direction:rtl }"), [0.0, 0.0]);
    assert_eq!(widths("p { text-indent:4px }"), [4.0, 0.0]);
    assert_eq!(
        widths("p { text-indent:10%; text-align:right }"),
        [17.5, 35.0]
    );
    // Justify stretches the spaces of every line but the last.
    let l = layout("<p style='text-align:justify'>xx xx xx xx", "", 30.0);
    assert_eq!(texts(&l)[0].2.2, 30.0);
    assert_eq!(texts(&l)[1].2.2, 22.5);
}

#[test]
fn inline_boxes_have_edges_on_their_first_and_last_fragment() {
    let l = layout(
        "<p>x<span style='padding:0 3px;border:1px solid;margin-left:2px'>xx xx</span> x",
        "",
        30.0,
    );
    let f: Vec<_> = l
        .inlines()
        .iter()
        .map(|f| {
            (
                f.line(),
                r(f.border()),
                r(f.content()),
                f.starts(),
                f.ends(),
            )
        })
        .collect();
    assert_eq!(
        f,
        [
            (
                0,
                (7.0, -1.0, 14.0, 12.0),
                (11.0, 0.0, 10.0, 10.0),
                true,
                false
            ),
            (
                1,
                (0.0, 9.0, 14.0, 12.0),
                (0.0, 10.0, 10.0, 10.0),
                false,
                true
            ),
        ]
    );
    assert_eq!(texts(&l)[3].2, (14.0, 10.0, 7.5, 10.0));
    // Vertical padding and borders do not change the line height.
    assert_eq!(l.lines()[0].rect().size().height().get(), 10.0);
}

#[test]
fn line_height_and_vertical_align_build_baselines() {
    let l = layout(
        "<p>x<span style='vertical-align:top;font-size:20px'>x</span><span style='vertical-align:super'>x</span>",
        "",
        100.0,
    );
    let baselines: Vec<f64> = l.texts().iter().map(|t| t.baseline().get()).collect();
    let third = 10.0 / 3.0;
    assert_eq!(l.lines()[0].rect().size().height().get(), 20.0);
    assert_eq!(baselines, [8.0 + third, 16.0, 8.0]);
    // line-height adds half-leading above and below the content area.
    let l = layout("<p style='line-height:30px'>x", "", 100.0);
    assert_eq!(line_rects(&l)[0], (0.0, 0.0, 100.0, 30.0));
    assert_eq!(texts(&l)[0].2, (0.0, 10.0, 5.0, 10.0));
    let l = layout(
        "<p style='line-height:1'>x<span style='vertical-align:-5px'>x</span><span style='vertical-align:bottom;line-height:40px'>x</span>",
        "",
        100.0,
    );
    // The bottom-aligned box sets a 40px line; the root sits at the top.
    assert_eq!(line_rects(&l)[0].3, 40.0);
    let baselines: Vec<f64> = l.texts().iter().map(|t| t.baseline().get()).collect();
    assert_eq!(baselines, [8.0, 13.0, 23.0]);
    for valign in ["middle", "text-top", "text-bottom", "sub"] {
        let l = layout(
            &format!("<p>x<span style='vertical-align:{valign};font-size:20px'>x</span>"),
            "",
            100.0,
        );
        assert!(
            l.lines()[0].rect().size().height().get() >= 20.0,
            "{valign}"
        );
    }
}

#[test]
fn right_to_left_and_mixed_direction_lines_reorder() {
    let l = layout("<p style='direction:rtl'>xx \u{5D0}\u{5D1} x", "", 100.0);
    assert_eq!(
        l.texts()
            .iter()
            .map(|t| (t.text(), t.rect().origin().x.get(), t.bidi_level()))
            .collect::<Vec<_>>(),
        [(6..7, 70.0, 2), (2..6, 75.0, 1), (0..2, 90.0, 2)]
    );
    // Mirrored brackets in right-to-left runs and wrapping keep logical order
    // per line.
    let l = layout(
        "<p style='direction:rtl'>\u{5D0}\u{5D0} \u{5D1}\u{5D1} \u{5D2}\u{5D2}",
        "",
        25.0,
    );
    assert_eq!(l.lines().len(), 2);
    assert_eq!(
        texts(&l)
            .iter()
            .map(|t| (t.0, t.1.clone(), t.2.0))
            .collect::<Vec<_>>(),
        [(0, 0..5, 2.5), (1, 6..8, 15.0)]
    );
    // The last glyph in visual order is the logically first character.
    let glyphs = l.texts()[0].glyphs();
    assert_eq!(glyphs.last().map(|g| g.x), Some(20.0));
}

#[test]
fn lines_shorten_beside_floats_and_move_below_when_nothing_fits() {
    let l = layout(
        "<div style='float:left;width:10px;height:15px'></div><p>xx xx xx xx",
        "",
        30.0,
    );
    assert_eq!(
        line_rects(&l),
        [
            (10.0, 0.0, 20.0, 10.0),
            (10.0, 10.0, 20.0, 10.0),
            (0.0, 20.0, 30.0, 10.0)
        ]
    );
    let l = layout(
        "<div style='float:right;width:25px;height:12px'></div><p>xxxx",
        "",
        30.0,
    );
    // 20px of text cannot fit in the 5px band: the line moves below the float.
    assert_eq!(line_rects(&l), [(0.0, 12.0, 30.0, 10.0)]);
    // A float after text in the same container wraps later lines.
    let l = layout(
        "<p>xx</p><div style='float:left;width:10px;height:10px'></div><p>xx xx",
        "",
        30.0,
    );
    assert_eq!(line_rects(&l)[1], (10.0, 10.0, 20.0, 10.0));
}

#[test]
fn mixed_content_gets_anonymous_blocks_without_fragments() {
    let l = layout("<div>xx<p>x</p>xx</div>", "", 100.0);
    assert_eq!(l.fragments().len(), 4);
    assert_eq!(l.fragments()[3].parent(), Some(2));
    assert_eq!(
        l.lines()
            .iter()
            .map(|line| (line.block(), r(line.rect()).1))
            .collect::<Vec<_>>(),
        [(2, 0.0), (3, 10.0), (2, 20.0)]
    );
    // Whitespace between blocks still makes no anonymous block or line.
    let l = layout("<div>\n  <p>x</p>\n  <p>x</p>\n</div>", "", 100.0);
    assert_eq!(l.lines().len(), 2);
    // Margins collapse through an empty inline run but not through lines.
    let l = layout(
        "<div><p style='margin-bottom:5px'>x</p> <p style='margin-top:7px'>x</p></div>",
        "",
        100.0,
    );
    assert_eq!(r(l.lines()[1].rect()).1, 17.0);
}

#[test]
fn shrink_to_fit_uses_text_min_and_max_content() {
    let l = layout(
        "<div style='float:left'>xx xxx</div><div style='position:absolute;top:50px'>xxxx xx xxx</div>",
        "",
        12.0,
    );
    // The float's available width is 12px, below its min-content (15px).
    assert_eq!(r(l.fragments()[2].content()).2, 15.0);
    let l = layout("<div style='float:left'>xx xxx</div>", "", 100.0);
    assert_eq!(r(l.fragments()[2].content()).2, 27.5);
    assert_eq!(l.lines().len(), 1);
}

#[test]
fn spacing_and_transform_change_advances_not_sources() {
    let l = layout(
        "<p style='letter-spacing:1px;word-spacing:2px'>xx x",
        "",
        100.0,
    );
    // Each of the four clusters gains 1px; the space also gains 2px.
    assert_eq!(texts(&l)[0].2.2, 5.0 * 3.0 + 2.5 + 4.0 + 2.0);
    let l = layout("<p style='text-transform:uppercase'>stra\u{DF}e", "", 100.0);
    // ß uppercases to SS: one more glyph, same UTF-16 source range.
    assert_eq!(l.texts()[0].glyphs().len(), 7);
    assert_eq!(l.texts()[0].text(), 0..6);
}

#[test]
fn relative_inline_boxes_shift_their_fragments() {
    let l = layout(
        "<p>x<span style='position:relative;left:3px;top:2px'>x</span>",
        "",
        100.0,
    );
    assert_eq!(texts(&l)[1].2, (8.0, 2.0, 5.0, 10.0));
    assert_eq!(r(l.inlines()[0].border()), (8.0, 2.0, 5.0, 10.0));
}

#[test]
fn unsupported_inline_content_fails_atomically() {
    use rustic_layout::UnsupportedFlow;
    // `inline-table` is laid out since `layout.tables` (tests/tables.rs).
    for (html, reason) in [
        ("<p>x<svg></svg>", UnsupportedFlow::SpecialElement),
        ("<p>x<math></math>", UnsupportedFlow::SpecialElement),
    ] {
        match try_layout(html, "", 100.0) {
            Err(FlowError::Unsupported { reason: got, .. }) => assert_eq!(got, reason, "{html}"),
            other => panic!("{html}: {other:?}"),
        }
    }
    // Without fonts the block reference keeps refusing text.
    let (doc, styles) = common::page("<p>x", "");
    let tree = BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    assert!(matches!(
        BlockLayout::compute(
            &tree,
            CssSize::new(100.0, 100.0).unwrap(),
            LayoutLimits::default(),
            &CancellationToken::default()
        ),
        Err(FlowError::Unsupported {
            reason: rustic_layout::UnsupportedFlow::TextLayout,
            ..
        })
    ));
}

#[test]
fn cancellation_and_limits_stop_text_layout() {
    let fonts = test_database();
    let (doc, styles) = common::page("<p>xx xx xx", "");
    let tree = BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    let cancelled = CancellationToken::default();
    cancelled.cancel();
    assert_eq!(
        BlockLayout::compute_with_fonts(
            &tree,
            CssSize::new(100.0, 100.0).unwrap(),
            LayoutLimits::default(),
            &fonts,
            &cancelled
        ),
        Err(FlowError::Cancelled)
    );
    let limits = LayoutLimits::default().with_max_text_units(7);
    assert_eq!(
        BlockLayout::compute_with_fonts(
            &tree,
            CssSize::new(100.0, 100.0).unwrap(),
            limits,
            &fonts,
            &CancellationToken::default()
        ),
        Err(FlowError::TextLimit)
    );
}

/// Independent model: greedy filling of space-separated words of `x`.
#[test]
fn greedy_breaking_matches_an_independent_model() {
    let mut seed = 0x2545_F491_u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut checked = 0;
    for _ in 0..60 {
        let words: Vec<usize> = (0..12).map(|_| 1 + (next() % 6) as usize).collect();
        let text = words
            .iter()
            .map(|&n| "x".repeat(n))
            .collect::<Vec<_>>()
            .join(" ");
        for width in [12.0, 23.0, 37.5, 61.0] {
            // Model: lines of word indexes.
            let mut model: Vec<Vec<usize>> = vec![];
            let mut current = 0.0;
            for (i, &n) in words.iter().enumerate() {
                let w = 5.0 * n as f64;
                match model.last_mut() {
                    Some(line) if current + 2.5 + w <= width => {
                        line.push(i);
                        current += 2.5 + w;
                    }
                    _ => {
                        model.push(vec![i]);
                        current = w;
                    }
                }
            }
            let l = layout(&format!("<p>{text}"), "", width);
            assert_eq!(l.lines().len(), model.len(), "{text} @ {width}");
            for (line, ws) in model.iter().enumerate() {
                let start: usize = words[..ws[0]].iter().map(|n| n + 1).sum();
                let end: usize = start + ws.iter().map(|&i| words[i]).sum::<usize>() + ws.len() - 1;
                let t = &l.texts()[line];
                assert_eq!((t.line(), t.text()), (line, start..end), "{text} @ {width}");
                checked += 1;
            }
        }
    }
    assert!(checked > 600);
}

#[test]
fn dump_lists_lines_texts_and_inline_boxes() {
    let out = dump("<p>x <span style='padding:1px'>xx</span>", "", 100.0);
    let rows: Vec<&str> = out.lines().filter(|l| !l.starts_with("fragment")).collect();
    assert_eq!(
        rows,
        [
            "line 0 block=2 rect=(0,0,100,10) baseline=8",
            "text 0 line=0 box=3 node=5:1 utf16=0..2 rect=(0,0,7.5,10) baseline=8 level=0 face=0 size=10 glyphs=2",
            "text 1 line=0 box=5 node=7:1 utf16=0..2 rect=(8.5,0,10,10) baseline=8 level=0 face=0 size=10 glyphs=2",
            "inline 0 line=0 box=4 node=6:1 border=(7.5,-1,12,12) start end",
        ]
    );
}

#[test]
fn text_sizes_keywords_and_flex_items() {
    // fit-content/min-content/max-content use the text's contributions.
    let widths = |css: &str| -> f64 {
        let l = layout("<div id=t>xx xxx</div>", css, 100.0);
        r(l.fragments()[2].content()).2
    };
    assert_eq!(widths("#t { width:min-content }"), 15.0);
    assert_eq!(widths("#t { width:max-content }"), 27.5);
    assert_eq!(widths("#t { width:fit-content }"), 27.5);
    // Flex items are block roots, so their text gets lines and sizes them.
    let l = layout(
        "<main><div>xx</div><div>xxx</div></main>",
        "main { display:flex }",
        100.0,
    );
    assert_eq!(
        l.fragments()[3..]
            .iter()
            .map(|f| r(f.content()))
            .collect::<Vec<_>>(),
        [(0.0, 0.0, 10.0, 10.0), (10.0, 0.0, 15.0, 10.0)]
    );
    assert_eq!(
        texts(&l).iter().map(|t| t.2.0).collect::<Vec<_>>(),
        [0.0, 10.0]
    );
    // Text directly in a flex container gets an anonymous flex item, which
    // has no fragment of its own; its line belongs to the container.
    let l = layout(
        "<main>xx<div>x</div></main>",
        "main { display:flex }",
        100.0,
    );
    assert_eq!(r(l.fragments()[3].border()), (10.0, 0.0, 5.0, 10.0));
    assert_eq!(texts(&l)[0].2, (0.0, 0.0, 10.0, 10.0));
    assert_eq!(l.lines()[0].block(), 2);
    // Fallback content of a replaced element is not laid out.
    let l = layout("<canvas style='display:block'>xx</canvas>", "", 100.0);
    assert!(l.lines().is_empty() && l.texts().is_empty());
}

#[test]
fn outside_list_markers_hang_before_the_first_line() {
    // "•" is .notdef here (5px) and the marker keeps its space (2.5px).
    let l = layout("<ul><li>x xx</ul>", "ul { padding-left: 40px }", 55.0);
    let rows = texts(&l);
    assert_eq!(rows[0].2, (32.5, 0.0, 7.5, 10.0), "{rows:?}");
    assert_eq!(rows[1].2.0, 40.0, "{rows:?}");
    // Wrapped lines start at the content edge.
    assert_eq!(line_rects(&l)[1].0, 40.0);
    // Inside markers are ordinary inline content.
    let l = layout(
        "<ul><li>x</ul>",
        "ul { padding-left: 40px; list-style-position: inside }",
        100.0,
    );
    assert_eq!(texts(&l)[0].2.0, 40.0);
    assert_eq!(texts(&l)[1].2.0, 47.5);
    // `none` generates no marker.
    let l = layout("<ul style='list-style: none'><li>x</ul>", "", 100.0);
    assert_eq!(texts(&l).len(), 1);
}

#[test]
fn form_controls_lay_out_as_inline_blocks_with_their_value() {
    // 20ch of 5.5px, 2px+2px padding, 1px+1px border; one 10px line plus
    // 1px+1px padding and border.
    let l = layout(
        "<p><input value=xx><input><select><option>x<option selected>xxx</select>",
        "",
        400.0,
    );
    let sizes: Vec<(f64, f64)> = l
        .fragments()
        .iter()
        .map(|f| {
            let size = f.border().size();
            (
                (size.width().get() * 1e6).round() / 1e6,
                size.height().get(),
            )
        })
        .collect();
    assert_eq!(
        sizes.iter().filter(|s| **s == (116.0, 14.0)).count(),
        2,
        "{sizes:?}"
    );
    // The select shows its selected option: 15px of text, 4px+4px padding.
    assert!(sizes.contains(&(25.0, 14.0)), "{sizes:?}");
    // Each field's text: the value and the selected option (the empty
    // field's zero-width strut only holds its line).
    let widths: Vec<f64> = texts(&l).iter().map(|t| t.2.2).collect();
    assert_eq!(widths, [10.0, 15.0]);
}
