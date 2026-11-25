//! Anchor positioning: `anchor()` insets, `anchor-size()` sizes and
//! `position-area` for absolutely positioned boxes.

use rustic_base::CancellationToken;
use rustic_layout::{BlockLayout, BoxTree, CssSize, LayoutLimits};

mod common;

const RESET: &str = "* { margin:0; }";

fn layout(html: &str, css: &str) -> BlockLayout {
    let (doc, styles) = common::page(html, &format!("{RESET} {css}"));
    let tree = BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    BlockLayout::compute(
        &tree,
        CssSize::new(300.0, 200.0).unwrap(),
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap()
}

fn border(l: &BlockLayout, i: usize) -> (f64, f64, f64, f64) {
    let r = l.fragments()[i].border();
    (
        r.origin().x.get(),
        r.origin().y.get(),
        r.size().width().get(),
        r.size().height().get(),
    )
}

// Fragment indexes: 0 html, 1 body, 2 main, 3 #pad, 4 #a, 5 #t.
const PAGE: &str = "<main><div id=pad></div><div id=a></div><div id=t></div></main>";
const BASE: &str = "main {position:relative;height:150px} #pad {height:40px} \
    #a {anchor-name:--a;margin-left:50px;width:60px;height:20px} \
    #t {position:absolute;width:10px;height:10px}";

#[test]
fn anchor_insets_follow_the_anchor_edges() {
    let l = layout(
        PAGE,
        &format!("{BASE} #t {{top:anchor(--a bottom);left:anchor(--a right)}}"),
    );
    assert_eq!(border(&l, 4), (50.0, 40.0, 60.0, 20.0));
    assert_eq!(border(&l, 5), (110.0, 60.0, 10.0, 10.0));

    // bottom/right measure from the far edges; position-anchor names the
    // default anchor; percentages and center pick points along the edge.
    let l = layout(
        PAGE,
        &format!("{BASE} #t {{position-anchor:--a;bottom:anchor(top);right:anchor(50%)}}"),
    );
    assert_eq!(border(&l, 5), (70.0, 30.0, 10.0, 10.0));
    let l = layout(
        PAGE,
        &format!("{BASE} #t {{position-anchor:--a;top:anchor(center);left:anchor(inside)}}"),
    );
    assert_eq!(border(&l, 5), (50.0, 50.0, 10.0, 10.0));
}

#[test]
fn unresolved_anchors_use_the_fallback() {
    let l = layout(
        PAGE,
        &format!("{BASE} #t {{top:anchor(--missing top, 7px);left:anchor(--a top, 9px)}}"),
    );
    // A missing anchor and a side of the wrong axis both fall back.
    assert_eq!(border(&l, 5), (9.0, 7.0, 10.0, 10.0));
    // Without a fallback the inset is auto (the static position).
    let l = layout(PAGE, &format!("{BASE} #t {{top:anchor(--missing top)}}"));
    assert_eq!(border(&l, 5).1, 60.0);
}

#[test]
fn anchor_size_sets_sizes() {
    let l = layout(
        PAGE,
        &format!(
            "{BASE} #t {{position-anchor:--a;width:anchor-size(width);height:anchor-size(--a width);max-height:anchor-size(height)}}"
        ),
    );
    assert_eq!(border(&l, 5), (0.0, 60.0, 60.0, 20.0));
}

#[test]
fn position_area_places_the_box_in_the_anchor_grid() {
    // Below and to the right: the region starts at the anchor's corner.
    let l = layout(
        PAGE,
        &format!("{BASE} #t {{position-anchor:--a;position-area:bottom right}}"),
    );
    assert_eq!(border(&l, 5), (110.0, 60.0, 10.0, 10.0));
    // Above and left: aligned toward the anchor (end of the region).
    let l = layout(
        PAGE,
        &format!("{BASE} #t {{position-anchor:--a;position-area:top left}}"),
    );
    assert_eq!(border(&l, 5), (40.0, 30.0, 10.0, 10.0));
    // Centered over the anchor column, below it.
    let l = layout(
        PAGE,
        &format!("{BASE} #t {{position-anchor:--a;position-area:bottom center}}"),
    );
    assert_eq!(border(&l, 5), (75.0, 60.0, 10.0, 10.0));
    // A single keyword spans the other axis; auto sizes fill nothing extra.
    let l = layout(
        PAGE,
        &format!("{BASE} #t {{position-anchor:--a;position-area:span-right;width:auto}}"),
    );
    assert_eq!(border(&l, 5).0, 50.0);
    // No default anchor: position-area does nothing.
    let l = layout(PAGE, &format!("{BASE} #t {{position-area:bottom right}}"));
    assert_eq!(border(&l, 5), (0.0, 60.0, 10.0, 10.0));
}

#[test]
fn the_last_anchor_in_tree_order_wins_and_descendants_are_skipped() {
    let l = layout(
        "<main><div id=a></div><div id=b></div><div id=t><div id=c></div></div></main>",
        "main {position:relative} #a,#b,#c {anchor-name:--x;height:10px} #b {height:30px} \
         #t {position:absolute;top:anchor(--x bottom);width:5px;height:5px}",
    );
    // #c is inside #t, so #b (ending at 40px) is the anchor.
    assert_eq!(border(&l, 5).1, 40.0);
}
