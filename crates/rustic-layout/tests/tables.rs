//! Table layout (CSS 2.2 §17): slots, column widths, row heights,
//! vertical alignment, captions and the block walk, checked against
//! hand-computed geometry. The HTML user-agent sheet gives tables 2px
//! border spacing and cells 1px padding.

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

fn page(style: &str, body: &str) -> String {
    format!("<!DOCTYPE html><style>body {{ margin: 0 }} {style}</style><body>{body}")
}

/// A block of a fixed size, so cells need no text measurement.
fn b(id: &str, w: u32, h: u32) -> String {
    format!("<div id={id} style='width:{w}px;height:{h}px'></div>")
}

#[test]
fn columns_take_the_widest_cell_and_rows_the_tallest() {
    let html = page(
        "",
        &format!(
            "<table id=t><tr id=r0><td id=a>{}</td><td id=b>{}</td></tr>\
             <tr id=r1><td id=c>{}</td><td id=d>{}</td></tr></table>",
            b("ia", 50, 20),
            b("ib", 30, 10),
            b("ic", 10, 5),
            b("id", 40, 30)
        ),
    );
    let m = boxes(&html);
    // Columns 52 and 42 (1px padding), rows 22 and 32, 2px spacing.
    assert_eq!(m["t"], (0.0, 0.0, 100.0, 60.0));
    assert_eq!(m["a"], (2.0, 2.0, 52.0, 22.0));
    assert_eq!(m["b"], (56.0, 2.0, 42.0, 22.0));
    assert_eq!(m["c"], (2.0, 26.0, 52.0, 32.0));
    assert_eq!(m["d"], (56.0, 26.0, 42.0, 32.0));
    assert_eq!(m["r1"], (2.0, 26.0, 96.0, 32.0));
    // Cells are vertically centred (the sheet's `vertical-align: middle`).
    assert_eq!(m["ib"], (57.0, 8.0, 30.0, 10.0));
    assert_eq!(m["ic"], (3.0, 39.5, 10.0, 5.0));
}

#[test]
fn colspan_and_rowspan_take_slots_and_grow_columns_and_rows() {
    let html = page(
        "table { border-spacing: 0 } td { padding: 0; vertical-align: top }",
        &format!(
            "<table id=t><tr><td id=a colspan=2>{}</td><td id=b rowspan=2>{}</td></tr>\
             <tr><td id=c>{}</td><td id=d>{}</td></tr></table>",
            b("ia", 100, 10),
            b("ib", 20, 50),
            b("ic", 30, 10),
            b("id", 10, 10)
        ),
    );
    let m = boxes(&html);
    // The spanning 100px cell grows columns 30 and 10 in a 3:1 ratio.
    // The 50px rowspan cell grows both 10px rows in proportion: 25 each.
    assert_eq!(m["a"], (0.0, 0.0, 100.0, 25.0));
    assert_eq!(m["c"], (0.0, 25.0, 75.0, 25.0));
    assert_eq!(m["d"], (75.0, 25.0, 25.0, 25.0));
    assert_eq!(m["b"], (100.0, 0.0, 20.0, 50.0));
    assert_eq!(m["t"], (0.0, 0.0, 120.0, 50.0));
}

#[test]
fn a_wider_table_shares_space_by_maximum_width_and_centres_by_margin() {
    let html = page(
        "table { width: 300px; margin: 0 auto; border-spacing: 0 } td { padding: 0 }",
        &format!(
            "<table id=t><tr><td id=a>{}</td><td id=b>{}</td></tr></table>",
            b("ia", 100, 10),
            b("ib", 50, 10)
        ),
    );
    let m = boxes(&html);
    assert_eq!(m["t"], (250.0, 0.0, 300.0, 10.0));
    assert_eq!(m["a"], (250.0, 0.0, 200.0, 10.0));
    assert_eq!(m["b"], (450.0, 0.0, 100.0, 10.0));
}

#[test]
fn fixed_layout_uses_the_first_row_widths() {
    let html = page(
        "table { width: 300px; table-layout: fixed; border-spacing: 0 } td { padding: 0 }",
        &format!(
            "<table id=t><tr><td id=a style='width:100px'></td><td id=b></td><td id=c></td></tr>\
             <tr><td id=d>{}</td><td></td><td></td></tr></table>",
            b("wide", 500, 10)
        ),
    );
    let m = boxes(&html);
    assert_eq!(m["a"].2, 100.0);
    assert_eq!(m["b"], (100.0, 0.0, 100.0, 0.0));
    assert_eq!(m["c"].0, 200.0);
    // Content wider than its fixed column overflows it.
    assert_eq!(m["d"].2, 100.0);
    assert_eq!(m["wide"].2, 500.0);
}

#[test]
fn captions_sit_above_and_below_the_rows() {
    let html = page(
        "table { border-spacing: 0 } td { padding: 0 }",
        &format!(
            "<table id=t><caption id=top>{}</caption><tr><td id=a>{}</td></tr>\
             <caption id=bottom style='caption-side: bottom'>{}</caption></table>",
            b("c1", 10, 15),
            b("ia", 40, 20),
            b("c2", 10, 5)
        ),
    );
    let m = boxes(&html);
    assert_eq!(m["top"], (0.0, 0.0, 40.0, 15.0));
    assert_eq!(m["a"], (0.0, 15.0, 40.0, 20.0));
    assert_eq!(m["bottom"], (0.0, 35.0, 40.0, 5.0));
    assert_eq!(m["t"], (0.0, 0.0, 40.0, 40.0));
}

#[test]
fn header_and_footer_groups_move_to_the_edges() {
    let html = page(
        "table { border-spacing: 0 } td { padding: 0 }",
        &format!(
            "<table><tfoot id=f><tr><td>{}</td></tr></tfoot>\
             <tbody id=body><tr><td>{}</td></tr></tbody>\
             <thead id=h><tr><td>{}</td></tr></thead></table>",
            b("foot", 10, 5),
            b("body1", 10, 7),
            b("head", 10, 3)
        ),
    );
    let m = boxes(&html);
    assert_eq!(m["h"].1, 0.0);
    assert_eq!(m["body"].1, 3.0);
    assert_eq!(m["f"], (0.0, 10.0, 10.0, 5.0));
    assert_eq!(m["foot"].1, 10.0);
}

#[test]
fn specified_heights_grow_rows_and_baselines_align() {
    let html = page(
        "table { height: 100px; border-spacing: 0 } td { padding: 0; vertical-align: baseline }",
        &format!(
            "<table id=t><tr id=r><td id=a>{}</td><td id=b style='vertical-align: bottom'>{}</td></tr>\
             <tr style='height: 30px'><td>{}</td></tr></table>",
            b("ia", 10, 20),
            b("ib", 10, 10),
            b("ic", 10, 10)
        ),
    );
    let m = boxes(&html);
    assert_eq!(m["t"].3, 100.0);
    // 100px over rows 20 and 30, in that ratio: 40 and 60.
    assert_eq!(m["r"].3, 40.0);
    assert_eq!(m["ib"].1, 30.0);
    // A block without lines has its baseline at its bottom edge.
    assert_eq!(m["ia"].1, 0.0);
}

#[test]
fn right_to_left_tables_mirror_their_columns() {
    let html = page(
        "table { border-spacing: 0; direction: rtl } td { padding: 0 }",
        &format!(
            "<table id=t><tr><td id=a>{}</td><td id=b>{}</td></tr></table>",
            b("ia", 30, 10),
            b("ib", 20, 10)
        ),
    );
    let m = boxes(&html);
    assert_eq!(m["t"].2, 50.0);
    assert_eq!(m["a"].0, 20.0);
    assert_eq!(m["b"].0, 0.0);
}

#[test]
fn table_parts_outside_tables_are_still_refused() {
    let html = page("", "<div style='display: table-cell'>x</div>");
    assert!(try_boxes(&html).is_err());
    let html = page(
        "",
        "<div style='display: table'><div>block in a table</div></div>",
    );
    assert!(try_boxes(&html).is_err());
}
