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

#[test]
fn grid_areas_are_containing_blocks_for_positioned_children() {
    let style = ".g { position: relative; display: grid; width: 300px; height: 100px;
                      padding: 10px; grid-template-columns: 100px 50px; grid-template-rows: 40px;
                      column-gap: 20px }
                 .g > div { position: absolute; width: 100%; height: 100%; top: 0; left: 0 }";
    let b = boxes(&page(
        style,
        "<div class=g id=g>\
           <div id=a style='grid-column: 2 / 3; grid-row: 1 / 2'></div>\
           <div id=b style='grid-column: auto; grid-row: auto'></div>\
           <div id=c style='grid-column: 2 / auto; grid-row: 1 / auto'></div>\
           <div id=d style='grid-column: 9 / 12'></div>\
         </div>",
    ));
    // Area of column 2, row 1: x = padding 10 + 100 + gap 20.
    assert_eq!(b["a"], (130.0, 10.0, 50.0, 40.0));
    // `auto` lines are the padding edges.
    assert_eq!(b["b"], (0.0, 0.0, 320.0, 120.0));
    assert_eq!(b["c"], (130.0, 10.0, 190.0, 110.0));
    // An omitted end line and lines outside the grid are `auto`.
    assert_eq!(b["d"], (0.0, 0.0, 320.0, 120.0));
}

#[test]
fn positioned_grid_children_without_insets_start_at_their_area() {
    let b = boxes(&page(
        ".g { position: relative; display: grid; width: 200px; grid-template-columns: 60px 60px;
              grid-template-rows: 30px 30px }
         #a { position: absolute; width: 5px; height: 5px; grid-area: 2 / 2 }",
        "<div class=g><div id=a></div></div>",
    ));
    assert_eq!(b["a"], (60.0, 30.0, 5.0, 5.0));
    // RTL: column 1 is at the right.
    let b = boxes(&page(
        ".g { position: relative; display: grid; direction: rtl; width: 200px;
              grid-template-columns: 60px 60px; grid-template-rows: 30px }
         #a { position: absolute; left: 0; right: 0; height: 5px; grid-column: 1 / 2 }",
        "<div class=g><div id=a></div></div>",
    ));
    assert_eq!(b["a"], (140.0, 0.0, 60.0, 5.0));
}

#[test]
fn baseline_aligned_items_share_a_row_baseline() {
    // Same font, so the text baselines line up when the item without top
    // padding moves down by 20px; the row grows by the shim.
    let b = boxes(&page(
        ".g { display: grid; grid-template-columns: 50px 50px; align-items: baseline;
              font: 10px/10px sans-serif }
         #a { padding-top: 20px }",
        "<div class=g id=g><div id=a>x</div><div id=b>y</div></div>",
    ));
    assert_eq!(b["a"], (0.0, 0.0, 50.0, 30.0));
    assert_eq!(b["b"], (50.0, 20.0, 50.0, 10.0));
    assert_eq!(b["g"].3, 30.0);
    // Without line boxes the baseline is the border box's bottom edge.
    let b = boxes(&page(
        ".g { display: grid; grid-template-columns: 50px 50px }
         #a { height: 40px; align-self: baseline } #b { height: 10px; align-self: baseline }",
        "<div class=g><div id=a></div><div id=b></div></div>",
    ));
    assert_eq!(b["b"].1, 30.0);
    // `last baseline` keeps the largest descent below the shared baseline.
    let b = boxes(&page(
        ".g { display: grid; grid-template-columns: 50px 50px; grid-template-rows: 60px;
              align-items: last baseline; font: 10px/10px sans-serif }
         #a { padding-bottom: 15px }",
        "<div class=g><div id=a>x</div><div id=b>y</div></div>",
    ));
    assert_eq!(b["a"], (0.0, 35.0, 50.0, 25.0));
    assert_eq!(b["b"], (50.0, 35.0, 50.0, 10.0));
}

#[test]
fn an_inline_grid_takes_its_baseline_from_its_first_row() {
    // The first row's baseline-aligned item is 50px tall with no text, so
    // the grid's baseline is 50px below its top: the 10px box beside it on
    // the line ends there.
    let b = boxes(&page(
        "div { line-height: 0 } .g { display: inline-grid; grid-auto-flow: column }
         #i1 { width: 30px; height: 100px } #i2 { width: 30px; height: 50px; align-self: baseline }
         #x { display: inline-block; width: 10px; height: 10px }",
        "<div><span id=x></span><span class=g id=g><div id=i1></div><div id=i2></div></span></div>",
    ));
    assert_eq!(b["x"].1, 40.0);
    assert_eq!(b["g"].1, 0.0);
}

#[test]
fn auto_repetitions_count_against_min_and_max_sizes() {
    // No definite height: repetitions fit the max height, but a smaller
    // max yields to the min height (130px fits two 50px rows and a gap).
    let style = ".g { display: grid; grid-template-rows: repeat(auto-fill, 50px); row-gap: 20px;
                      grid-auto-columns: 10px }
                 #a { grid-row: -2 }";
    let b = boxes(&page(
        &format!("{style} .g {{ max-height: 130px }}"),
        "<div class=g><div id=a></div></div>",
    ));
    assert_eq!(b["a"].1, 70.0);
    let b = boxes(&page(
        &format!("{style} .g {{ max-height: 60px; min-height: 130px }}"),
        "<div class=g><div id=a></div></div>",
    ));
    assert_eq!(b["a"].1, 70.0);
    // Only a min height: the fewest repetitions that fill it (three).
    let b = boxes(&page(
        &format!("{style} .g {{ min-height: 130px }}"),
        "<div class=g><div id=a></div></div>",
    ));
    assert_eq!(b["a"].1, 140.0);
}

#[test]
fn column_subgrid_items_size_and_use_the_parent_tracks() {
    // The subgrid's first item sizes the parent's `auto` column, with the
    // subgrid's 10px left padding added at that edge: 40 + 10 = 50.
    let style = ".g { display: grid; width: 400px; grid-template-columns: auto 100px 1fr;
                      column-gap: 10px }
                 .s { display: grid; grid-template-columns: subgrid; grid-column: 1 / -1;
                      padding-left: 10px; border-right: 4px solid }
                 #a { width: 40px }";
    let b = boxes(&page(
        style,
        "<div class=g><div class=s id=s><div id=a></div><div id=b></div><div id=c></div></div>\
         <div id=p></div><div id=q></div><div id=r></div></div>",
    ));
    assert_eq!(b["s"], (0.0, 0.0, 400.0, 0.0));
    assert_eq!((b["a"].0, b["a"].2), (10.0, 40.0));
    // The subgrid's columns line up with the parent's (50, 100, 230).
    assert_eq!((b["b"].0, b["b"].2), (60.0, 100.0));
    assert_eq!((b["p"].0, b["p"].2), (0.0, 50.0));
    assert_eq!((b["q"].0, b["q"].2), (60.0, 100.0));
    assert_eq!((b["r"].0, b["r"].2), (170.0, 230.0));
    // The last track loses the border at the subgrid's end edge.
    assert_eq!((b["c"].0, b["c"].2), (170.0, 226.0));
}

#[test]
fn row_subgrid_items_size_the_parent_rows() {
    let style = ".g { display: grid; grid-template-columns: 100px 100px;
                      grid-template-rows: auto auto; row-gap: 5px }
                 .s { display: grid; grid-template-rows: subgrid; grid-row: 1 / 3;
                      grid-column: 2 }
                 #x { height: 30px } #y { height: 50px } #z { height: 20px }";
    let b = boxes(&page(
        style,
        "<div class=g><div id=x></div><div class=s id=s><div id=y></div><div id=z></div></div>\
         <div id=w></div></div>",
    ));
    // Rows: max(30, 50) = 50 and 20; the subgrid spans both and the gap.
    assert_eq!(b["s"], (100.0, 0.0, 100.0, 75.0));
    assert_eq!((b["y"].1, b["y"].3), (0.0, 50.0));
    assert_eq!((b["z"].1, b["z"].3), (55.0, 20.0));
    // A stretched parent item fills the second row.
    assert_eq!((b["w"].1, b["w"].3), (55.0, 20.0));
}

#[test]
fn subgrids_adopt_parent_line_names_and_keep_their_own_gap() {
    // Parent lines at 0, 100 | 120, 220 | 240, 340. With a 0px gap the
    // subgrid's lines sit mid-gutter, at 110 and 230.
    let style = ".g { display: grid; width: 340px;
                      grid-template-columns: [a] 100px [b] 100px [c] 100px [d];
                      column-gap: 20px }
                 .s { display: grid; grid-template-columns: subgrid [x] [y];
                      grid-column: a / d; column-gap: 0 }
                 #i { grid-column: b / d } #j { grid-column: y } #k { grid-column: 3 }";
    let b = boxes(&page(
        style,
        "<div class=g><div class=s><div id=i></div><div id=j></div><div id=k></div></div></div>",
    ));
    assert_eq!((b["i"].0, b["i"].2), (110.0, 230.0));
    assert_eq!((b["j"].0, b["j"].2), (110.0, 120.0));
    assert_eq!((b["k"].0, b["k"].2), (230.0, 110.0));
}

#[test]
fn nested_subgrids_and_auto_spans_reach_the_root_grid() {
    // A subgrid with no placement spans as many tracks as it names lines
    // minus one; a subgrid inside it still sizes the root's columns.
    let style = ".g { display: grid; grid-template-columns: auto auto auto; width: 300px;
                      justify-content: start }
                 .s { display: grid; grid-template-columns: subgrid [p] [q] [r] }
                 .t { display: grid; grid-template-columns: subgrid; grid-column: 1 / 3 }
                 #m { width: 70px } #n { width: 30px; grid-column: 2 }";
    let b = boxes(&page(
        style,
        "<div class=g><div class=s id=s><div class=t><div id=m></div><div id=n></div></div>\
         </div><div id=o></div></div>",
    ));
    assert_eq!(b["s"].2, 100.0);
    assert_eq!((b["m"].0, b["m"].2), (0.0, 70.0));
    assert_eq!((b["n"].0, b["n"].2), (70.0, 30.0));
    // The third column is empty, so the next item wraps to it.
    assert_eq!((b["o"].0, b["o"].2), (100.0, 0.0));
}

#[test]
fn an_ltr_subgrid_in_an_rtl_grid_reverses_the_adopted_tracks() {
    // RTL parent columns from the right: 50px at 350..400, 100px at
    // 250..350. The LTR subgrid's first column is the parent's last.
    let style = ".g { display: grid; width: 400px; direction: rtl;
                      grid-template-columns: 50px 100px 1fr }
                 .s { display: grid; grid-template-columns: subgrid; grid-column: 1 / -1;
                      direction: ltr }
                 #b { grid-column: 3 }";
    let b = boxes(&page(
        style,
        "<div class=g><div class=s><div id=a></div><div id=b></div></div></div>",
    ));
    assert_eq!((b["a"].0, b["a"].2), (0.0, 250.0));
    assert_eq!((b["b"].0, b["b"].2), (350.0, 50.0));
}

#[test]
fn scroll_container_baselines_clamp_to_the_border_box() {
    // The scroll container's only line is 200px below its content box, so
    // its baseline is clamped to its bottom border edge (csswg-drafts#7660).
    let b = boxes(&page(
        "#g { display: grid; grid-auto-flow: column; align-items: baseline; font: 10px/10px serif }",
        "<div id=g><div id=a>x</div>\
         <div id=s style='overflow: hidden; height: 50px'><div style='margin-top: 200px'>x</div></div></div>",
    ));
    assert_eq!(b["s"].1, 0.0);
    assert_eq!(b["a"].1 + 8.0, 50.0, "{:?}", b["a"]);
}
