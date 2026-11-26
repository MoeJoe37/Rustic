//! Grid layout (CSS Grid Layout 2): placement, track sizing, alignment and
//! integration with the block walk, checked against hand-computed geometry.

use std::collections::HashMap;
use std::sync::Arc;

use rustic_base::CancellationToken;
use rustic_css::CssParser;
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_html::HtmlDocumentParser;
use rustic_layout::{BlockLayout, BoxTree, CssSize, LayoutLimits};
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
fn boxes(html: &str) -> HashMap<String, (f64, f64, f64, f64)> {
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
    .unwrap();
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
    out
}

fn page(style: &str, body: &str) -> String {
    format!("<!DOCTYPE html><style>body {{ margin: 0 }} {style}</style><body>{body}")
}

#[test]
fn auto_tracks_take_grid_auto_sizes() {
    let b = boxes(&page(
        ".g { display: grid; grid-auto-columns: 10px; grid-auto-rows: 10px }",
        "<div class=g id=g><div id=a></div></div>",
    ));
    assert_eq!(b["a"], (0.0, 0.0, 10.0, 10.0));
    assert_eq!(b["g"], (0.0, 0.0, 800.0, 10.0));
}

#[test]
fn fixed_and_flexible_columns_share_the_container_with_gaps() {
    let b = boxes(&page(
        ".g { display: grid; width: 430px; grid-template-columns: 100px 1fr 2fr;
              grid-template-rows: 40px; column-gap: 15px }",
        "<div class=g><div id=a></div><div id=b></div><div id=c></div></div>",
    ));
    assert_eq!(b["a"], (0.0, 0.0, 100.0, 40.0));
    assert_eq!(b["b"], (115.0, 0.0, 100.0, 40.0));
    assert_eq!(b["c"], (230.0, 0.0, 200.0, 40.0));
}

#[test]
fn areas_and_named_lines_place_items() {
    let b = boxes(&page(
        ".g { display: grid; width: 300px; grid-template-columns: [left] 100px [mid] 200px [right];
              grid-template-rows: 10px 20px;
              grid-template-areas: 'head head' 'side main' }
         #h { grid-area: head } #s { grid-area: side } #m { grid-column: mid / right; grid-row: 2 }",
        "<div class=g><div id=m></div><div id=s></div><div id=h></div></div>",
    ));
    assert_eq!(b["h"], (0.0, 0.0, 300.0, 10.0));
    assert_eq!(b["s"], (0.0, 10.0, 100.0, 20.0));
    assert_eq!(b["m"], (100.0, 10.0, 200.0, 20.0));
}

#[test]
fn auto_placement_spans_wraps_and_packs_densely() {
    let style = ".g { display: grid; grid-template-columns: repeat(3, 10px); grid-auto-rows: 10px }
                 .w { grid-column: span 2 }";
    let body = "<div class=g id=g><div id=a></div><div id=b class=w></div><div id=c></div>\
                <div id=d class=w></div></div>";
    let b = boxes(&page(style, body));
    assert_eq!(b["a"], (0.0, 0.0, 10.0, 10.0));
    assert_eq!(b["b"], (10.0, 0.0, 20.0, 10.0));
    assert_eq!(b["c"], (0.0, 10.0, 10.0, 10.0));
    assert_eq!(b["d"], (10.0, 10.0, 20.0, 10.0));
    // Sparse packing leaves a hole that dense packing fills.
    let body =
        "<div class=g><div id=a class=w></div><div id=b class=w></div><div id=c></div></div>";
    let sparse = boxes(&page(style, body));
    assert_eq!(sparse["c"], (20.0, 10.0, 10.0, 10.0));
    let dense = boxes(&page(
        &format!("{style} .g {{ grid-auto-flow: dense }}"),
        body,
    ));
    assert_eq!(dense["c"], (20.0, 0.0, 10.0, 10.0));
    // Column flow.
    let b = boxes(&page(
        ".g { display: grid; grid-auto-flow: column; grid-template-rows: 10px 10px;
              grid-auto-columns: 30px }",
        "<div class=g><div id=a></div><div id=b></div><div id=c></div></div>",
    ));
    assert_eq!(b["b"], (0.0, 10.0, 30.0, 10.0));
    assert_eq!(b["c"], (30.0, 0.0, 30.0, 10.0));
}

#[test]
fn negative_lines_create_implicit_tracks_before_the_explicit_grid() {
    let b = boxes(&page(
        ".g { display: grid; grid-template-columns: 50px; grid-auto-columns: 7px;
              grid-template-rows: 10px }
         #a { grid-column: -3 / -2 } #b { grid-column: 1 }",
        "<div class=g><div id=a></div><div id=b></div></div>",
    ));
    assert_eq!(b["a"], (0.0, 0.0, 7.0, 10.0));
    assert_eq!(b["b"], (7.0, 0.0, 50.0, 10.0));
}

#[test]
fn intrinsic_columns_follow_item_contributions() {
    // auto columns grow to max-content, the rest is stretched equally.
    let b = boxes(&page(
        ".g { display: grid; width: 200px; grid-template-columns: auto auto; grid-auto-rows: 5px }
         .x { width: 40px } .y { width: 60px }",
        "<div class=g><div id=a><div class=x></div></div><div id=b><div class=y></div></div></div>",
    ));
    assert_eq!(b["a"], (0.0, 0.0, 90.0, 5.0));
    assert_eq!(b["b"], (90.0, 0.0, 110.0, 5.0));
    // An inline grid shrinks to its max-content width.
    let b = boxes(&page(
        ".g { display: inline-grid; grid-template-columns: min-content 1fr; grid-auto-rows: 5px }
         .x { width: 40px } .y { width: 60px }",
        "<div><span class=g id=g><div id=a><div class=x></div></div>\
         <div id=b><div class=y></div></div></span></div>",
    ));
    assert_eq!(b["g"].2, 100.0);
    assert_eq!(b["b"].2, 60.0);
}

#[test]
fn rows_size_from_item_heights_and_stretch_items() {
    let b = boxes(&page(
        ".g { display: grid; grid-template-columns: 50px 50px }
         .t { height: 30px }",
        "<div class=g id=g><div id=a class=t></div><div id=b></div><div id=c></div></div>",
    ));
    assert_eq!(b["a"], (0.0, 0.0, 50.0, 30.0));
    // `b` stretches to the row; the empty second row is 0px.
    assert_eq!(b["b"], (50.0, 0.0, 50.0, 30.0));
    assert_eq!(b["c"], (0.0, 30.0, 50.0, 0.0));
    assert_eq!(b["g"].3, 30.0);
    // Percentage heights resolve against the grid area.
    let b = boxes(&page(
        ".g { display: grid; grid-template-rows: 80px; grid-template-columns: 10px }
         #a { height: 50% }",
        "<div class=g><div id=a></div></div>",
    ));
    assert_eq!(b["a"].3, 40.0);
}

#[test]
fn content_and_self_alignment_position_tracks_and_items() {
    let b = boxes(&page(
        ".g { display: grid; width: 200px; height: 100px; grid-template-columns: 50px 50px;
              grid-template-rows: 40px; justify-content: space-between; align-content: center }
         #a { width: 10px; height: 10px; justify-self: end; align-self: center }
         #b { width: 10px; margin-left: auto; margin-right: auto }",
        "<div class=g><div id=a></div><div id=b></div></div>",
    ));
    assert_eq!(b["a"], (40.0, 45.0, 10.0, 10.0));
    assert_eq!(b["b"], (170.0, 30.0, 10.0, 40.0));
}

#[test]
fn rtl_grids_run_columns_from_the_right() {
    let b = boxes(&page(
        ".g { display: grid; direction: rtl; width: 200px; grid-template-columns: 50px 30px;
              grid-template-rows: 10px }
         #c { width: 5px; justify-self: start } #d { width: 5px; justify-self: left }",
        "<div class=g><div id=a></div><div id=b></div></div>\
         <div class=g><div id=c></div><div id=d></div></div>",
    ));
    assert_eq!(b["a"], (150.0, 0.0, 50.0, 10.0));
    assert_eq!(b["b"], (120.0, 0.0, 30.0, 10.0));
    assert_eq!(b["c"].0, 195.0);
    assert_eq!(b["d"].0, 120.0);
}

#[test]
fn auto_fit_collapses_empty_repetitions_and_their_gutters() {
    let style = ".g { display: grid; width: 200px; column-gap: 20px; grid-auto-rows: 5px;
                      grid-template-columns: repeat(auto-fit, minmax(50px, 1fr)) }";
    let one = boxes(&page(style, "<div class=g><div id=a></div></div>"));
    assert_eq!(one["a"].2, 200.0);
    let fill = boxes(&page(
        &style.replace("auto-fit", "auto-fill"),
        "<div class=g><div id=a></div></div>",
    ));
    // 200px fits three 50px tracks with two 20px gutters: (200 - 40) / 3.
    assert!((fill["a"].2 - 160.0 / 3.0).abs() < 1e-9);
}

#[test]
fn order_and_floats_do_not_apply_to_grid_items_as_in_flow() {
    let b = boxes(&page(
        ".g { display: grid; grid-template-columns: 10px 10px; grid-auto-rows: 5px }
         #a { order: 2; float: left } #b { order: 1 }",
        "<div class=g><div id=a></div><div id=b></div></div>",
    ));
    assert_eq!(b["b"].0, 0.0);
    assert_eq!(b["a"].0, 10.0);
}

#[test]
fn absolutely_positioned_grids_keep_their_min_height() {
    let b = boxes(&page(
        ".w { position: relative; width: 100px; height: 100px }
         .g { position: absolute; display: grid; min-height: 60px; width: 50px }",
        "<div class=w><div class=g><div id=a></div></div></div>",
    ));
    assert_eq!(b["a"].3, 60.0);
}

#[test]
fn huge_repeats_and_lines_clamp_instead_of_allocating() {
    // repeat(99999) clamps at parse time, lines at layout time; the work
    // budget bounds placement.
    let b = boxes(&page(
        ".g { display: grid; grid-template-columns: repeat(99999, 0px); grid-auto-rows: 1px }
         #a { grid-column: 999999 }",
        "<div class=g><div id=a></div></div>",
    ));
    assert_eq!(b["a"].3, 1.0);
}

#[test]
fn text_in_a_grid_is_an_anonymous_item() {
    let b = boxes(&page(
        ".g { display: grid; grid-template-columns: 30px 40px; font: 10px/10px sans-serif }",
        "<div class=g id=g>text<div id=b></div></div>",
    ));
    assert_eq!(b["b"].0, 30.0);
    assert_eq!(b["g"].3, 10.0);
}
