//! Vertical writing modes and orthogonal flows (CSS Writing Modes 4 §7),
//! checked against hand-computed geometry in an 800x600 viewport.
use std::collections::HashMap;
use std::sync::Arc;

use rustic_base::CancellationToken;
use rustic_css::CssParser;
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_html::HtmlDocumentParser;
use rustic_layout::{BlockLayout, BoxTree, CssSize, GlyphRotation, LayoutLimits};
use rustic_style::{ComputeContext, MatchContext, document_cascade, style_tree};
use rustic_text::bundled::test_database;

fn id_of(doc: &Document, id: NodeId) -> Option<String> {
    match doc.node(id).map(|n| n.kind()) {
        Ok(NodeKind::Element(e)) => e
            .attributes()
            .iter()
            .find(|a| a.name().local() == "id")
            .map(|a| String::from_utf16_lossy(a.value().as_units())),
        _ => None,
    }
}

/// Lay `html` out at 800x600 and return each element id's border box
/// (x, y, width, height) in page coordinates.
/// Border boxes (x, y, width, height) by element id.
type Boxes = HashMap<String, (f64, f64, f64, f64)>;

fn try_boxes(html: &str) -> Result<Boxes, String> {
    let mut parser = HtmlDocumentParser::default();
    parser.feed(html.as_bytes()).unwrap();
    let doc = parser.finish().unwrap().into_document();
    let css = CssParser::default();
    let (cascade, _) = document_cascade(&doc, &css).unwrap();
    let context = ComputeContext {
        viewport_width: 800.0,
        viewport_height: 600.0,
        ..ComputeContext::with_fonts(Arc::new(test_database()))
    };
    let styles = style_tree(
        &doc,
        doc.root(),
        &cascade,
        &css,
        &MatchContext::html(),
        &context,
    )
    .unwrap();
    let cancel = CancellationToken::default();
    let tree = BoxTree::build(&doc, doc.root(), &styles, LayoutLimits::default(), &cancel).unwrap();
    let fonts = test_database();
    let layout = BlockLayout::compute_with_fonts(
        &tree,
        CssSize::new(800.0, 600.0).unwrap(),
        LayoutLimits::default(),
        &fonts,
        &cancel,
    )
    .map_err(|e| e.to_string())?;
    let mut out = HashMap::new();
    for f in layout.fragments() {
        if let Some(id) = id_of(&doc, f.node()) {
            let b = f.border();
            out.entry(id).or_insert((
                b.origin().x.get(),
                b.origin().y.get(),
                b.size().width().get(),
                b.size().height().get(),
            ));
        }
    }
    Ok(out)
}

fn boxes(html: &str) -> Boxes {
    try_boxes(html).unwrap()
}

fn layout_of(html: &str) -> (Document, BlockLayout) {
    let mut parser = HtmlDocumentParser::default();
    parser.feed(html.as_bytes()).unwrap();
    let doc = parser.finish().unwrap().into_document();
    let css = CssParser::default();
    let (cascade, _) = document_cascade(&doc, &css).unwrap();
    let context = ComputeContext {
        viewport_width: 800.0,
        viewport_height: 600.0,
        ..ComputeContext::with_fonts(Arc::new(test_database()))
    };
    let styles = style_tree(
        &doc,
        doc.root(),
        &cascade,
        &css,
        &MatchContext::html(),
        &context,
    )
    .unwrap();
    let cancel = CancellationToken::default();
    let tree = BoxTree::build(&doc, doc.root(), &styles, LayoutLimits::default(), &cancel).unwrap();
    let layout = BlockLayout::compute_with_fonts(
        &tree,
        CssSize::new(800.0, 600.0).unwrap(),
        LayoutLimits::default(),
        &test_database(),
        &cancel,
    )
    .unwrap();
    (doc, layout)
}

const STYLE: &str = "<style>body { margin: 0 } div { margin: 0 }</style>";

#[test]
fn vertical_rl_blocks_stack_right_to_left() {
    let b = boxes(&format!(
        "{STYLE}<style>html {{ writing-mode: vertical-rl }}</style>\
         <div id=a style='inline-size: 100px; block-size: 50px; margin-block-end: 20px'></div>\
         <div id=b style='block-size: 30px; padding-inline-start: 10px'></div>"
    ));
    // The root fills the viewport's height (its inline size) and sits at
    // the right edge, sized by its content in the block axis.
    assert_eq!(b["a"], (750.0, 0.0, 50.0, 100.0));
    assert_eq!(b["b"], (700.0, 0.0, 30.0, 600.0));
}

#[test]
fn vertical_lr_blocks_stack_left_to_right() {
    let b = boxes(&format!(
        "{STYLE}<style>html {{ writing-mode: vertical-lr }}</style>\
         <div id=a style='width: 40px; height: 70px; margin-left: 5px'></div>\
         <div id=b style='width: 10%; margin-top: 7px'></div>"
    ));
    assert_eq!(b["a"], (5.0, 0.0, 40.0, 70.0));
    // A percentage block size against the body's indefinite block size
    // acts as auto (like `height: 10%` in horizontal flow); margins keep
    // their physical sides.
    assert_eq!(b["b"], (45.0, 7.0, 0.0, 593.0));
}

#[test]
fn an_orthogonal_box_is_sized_in_its_own_inline_axis() {
    let b = boxes(&format!(
        "{STYLE}<div id=p style='width: 300px'>\
         <div id=v style='writing-mode: vertical-rl; height: 120px; padding-left: 4px'>\
         <div id=c style='inline-size: 50px; block-size: 30px'></div>\
         <div id=d style='block-size: 20px'></div></div>\
         <div id=after style='height: 10px'></div></div>"
    ));
    // The vertical box is as wide as its blocks (its block size) and as
    // tall as its height; its blocks stack from its right edge.
    assert_eq!(b["v"], (0.0, 0.0, 54.0, 120.0));
    assert_eq!(b["c"], (24.0, 0.0, 30.0, 50.0));
    assert_eq!(b["d"], (4.0, 0.0, 20.0, 120.0));
    assert_eq!(b["after"], (0.0, 120.0, 300.0, 10.0));
}

#[test]
fn orthogonal_percentages_and_positioning_use_the_containing_block() {
    let b = boxes(&format!(
        "{STYLE}<div id=cb style='position: relative; width: 200px; height: 100px'>\
         <div id=v style='writing-mode: vertical-lr; position: absolute; right: 0; \
         width: 25%; height: 50%'></div></div>"
    ));
    assert_eq!(b["v"], (150.0, 0.0, 50.0, 50.0));
}

#[test]
fn text_on_vertical_lines_turns_sideways() {
    let (_, layout) = layout_of(&format!(
        "{STYLE}<style>html {{ writing-mode: vertical-rl; font: 16px/20px serif }}\
         </style><div>ab</div>"
    ));
    let line = layout.lines()[0];
    // One 20px-wide line at the right edge, running downwards.
    assert_eq!(line.rect().origin().x.get(), 780.0);
    assert_eq!(line.rect().size().width().get(), 20.0);
    assert_eq!(line.rotation(), GlyphRotation::Clockwise);
    let text = &layout.texts()[0];
    assert_eq!(text.rotation(), GlyphRotation::Clockwise);
    let glyphs = text.glyphs();
    assert!(glyphs[1].y > glyphs[0].y, "glyphs advance downwards");
    assert_eq!(glyphs[0].x, glyphs[1].x);
    // Glyph tops face the line's over side (right): the baseline is
    // nearer the left edge.
    assert!(text.baseline().get() > 780.0 && text.baseline().get() < 790.0);
}

#[test]
fn horizontal_layout_is_unchanged_without_vertical_modes() {
    let b = boxes(&format!(
        "{STYLE}<div id=a style='width: 10px; height: 20px; margin-inline-start: 3px'></div>"
    ));
    assert_eq!(b["a"], (3.0, 0.0, 10.0, 20.0));
}

#[test]
fn vertical_lr_and_sideways_lr_lines_face_their_over_sides() {
    let (_, layout) = layout_of(&format!(
        "{STYLE}<style>html {{ writing-mode: vertical-lr; font: 16px/20px serif }}\
         </style><div>ab</div>"
    ));
    let line = layout.lines()[0];
    assert_eq!(line.rect().origin().x.get(), 0.0);
    // Glyph tops still face right (the line's over side), so the baseline
    // is nearer the left edge, with the descent to its left.
    let text = &layout.texts()[0];
    assert_eq!(text.rotation(), GlyphRotation::Clockwise);
    assert!(text.baseline().get() < 10.0, "{}", text.baseline().get());
    let (_, layout) = layout_of(&format!(
        "{STYLE}<style>html {{ writing-mode: sideways-lr; font: 16px/20px serif }}\
         </style><div>ab</div>"
    ));
    let text = &layout.texts()[0];
    assert_eq!(text.rotation(), GlyphRotation::Counterclockwise);
    // Lines run bottom to top, with glyph tops to the left.
    let glyphs = text.glyphs();
    assert!(glyphs[1].y < glyphs[0].y);
    assert!(text.baseline().get() > 10.0, "{}", text.baseline().get());
}

#[test]
fn upright_text_stands_on_the_line_center() {
    let (_, layout) = layout_of(&format!(
        "{STYLE}<style>html {{ writing-mode: vertical-rl; text-orientation: upright; \
         font: 16px/20px serif }}</style><div>ab</div>"
    ));
    let text = &layout.texts()[0];
    assert!(text.upright());
    assert_eq!(text.rotation(), GlyphRotation::None);
    let g = text.glyphs();
    // The 20px line spans x 780..800; each glyph (8px advance) is centered
    // on x 790 and hangs 13.6px below its vertical origin (the font has no
    // `vmtx`, so its ink is centered in a 1em vertical advance).
    assert_eq!((g[0].x, g[0].y), (786.0, 13.6));
    assert_eq!((g[1].x, g[1].y), (786.0, 29.6));
    assert_eq!(text.rect().size().height().get(), 32.0);
    // In vertical-lr the line sits at the left edge.
    let (_, layout) = layout_of(&format!(
        "{STYLE}<style>html {{ writing-mode: vertical-lr; text-orientation: upright; \
         font: 16px/20px serif }}</style><div>ab</div>"
    ));
    assert_eq!(layout.texts()[0].glyphs()[0].x, 6.0);
}

#[test]
fn mixed_orientation_turns_only_sideways_characters() {
    let (_, layout) = layout_of(&format!(
        "{STYLE}<style>html {{ writing-mode: vertical-rl; font: 16px/20px serif }}\
         </style><div>ab\u{6C34}\u{3002}</div>"
    ));
    let texts = layout.texts();
    assert_eq!(texts.len(), 2);
    assert!(!texts[0].upright());
    assert_eq!(texts[0].rotation(), GlyphRotation::Clockwise);
    // 水 (U) and 。 (Tu) stand upright after the sideways Latin letters.
    assert!(texts[1].upright());
    assert_eq!(texts[1].glyphs().len(), 2);
    assert!(texts[1].rect().origin().y.get() >= texts[0].rect().size().height().get());
    // sideways-rl turns everything, whatever text-orientation says.
    let (_, layout) = layout_of(&format!(
        "{STYLE}<style>html {{ writing-mode: sideways-rl; text-orientation: upright; \
         font: 16px/20px serif }}</style><div>ab\u{6C34}</div>"
    ));
    assert!(layout.texts().iter().all(|t| !t.upright()));
}

#[test]
fn vertical_lines_align_on_the_central_baseline() {
    // Inline-blocks without lines synthesize a central baseline halfway
    // across their margin boxes, so their centers line up.
    let b = boxes(&format!(
        "{STYLE}<style>html {{ writing-mode: vertical-rl; line-height: 0 }} \
         span {{ display: inline-block; height: 10px }}</style>\
         <div><span id=a style='width: 10px'></span><span id=b style='width: 30px'></span></div>"
    ));
    let center = |r: (f64, f64, f64, f64)| r.0 + r.2 / 2.0;
    assert_eq!(center(b["a"]), center(b["b"]));
    assert_eq!(b["b"].0, 770.0);
    // sideways-rl keeps the alphabetic baseline: line-under (left) edges
    // line up.
    let b = boxes(&format!(
        "{STYLE}<style>html {{ writing-mode: sideways-rl; line-height: 0 }} \
         span {{ display: inline-block; height: 10px }}</style>\
         <div><span id=a style='width: 10px'></span><span id=b style='width: 30px'></span></div>"
    ));
    assert_eq!(b["a"].0, b["b"].0);
}

#[test]
fn vertical_grid_items_share_a_central_baseline() {
    let b = boxes(&format!(
        "{STYLE}<style>html {{ writing-mode: vertical-rl }} \
         #g {{ display: grid; grid-auto-flow: column; align-items: baseline }}</style>\
         <div id=g><div id=a style='width: 10px; height: 5px'></div>\
         <div id=b style='width: 30px; height: 5px'></div></div>"
    ));
    let center = |r: (f64, f64, f64, f64)| r.0 + r.2 / 2.0;
    assert_eq!(center(b["a"]), center(b["b"]));
}

#[test]
fn combined_text_sets_horizontally_in_one_em() {
    let (_, layout) = layout_of(&format!(
        "{STYLE}<style>html {{ writing-mode: vertical-rl; font: 16px/20px 'Rustic Test' }}\
         span {{ text-combine-upright: all }}</style><div>x<span>xx</span>x</div>"
    ));
    let texts = layout.texts();
    assert_eq!(texts.len(), 3, "{texts:#?}");
    let c = &texts[1];
    assert!(c.upright());
    assert_eq!(c.rotation(), GlyphRotation::None);
    let g = c.glyphs();
    // The two glyphs run left to right on one baseline, centered on the
    // line (x 780..800) and one em long along it.
    assert_eq!(g.len(), 2);
    assert_eq!(g[0].y, g[1].y);
    assert!(g[1].x > g[0].x);
    let advance = g[1].x - g[0].x;
    assert!((g[0].x + advance - 790.0).abs() < 1e-9, "{g:?}");
    assert_eq!(c.rect().size().height().get(), 16.0);
    assert_eq!(
        texts[2].rect().origin().y.get(),
        c.rect().origin().y.get() + 16.0
    );
}
