//! UI-05 scrolling: scroll positions applied to a finished layout move
//! contents, keep fixed boxes, re-solve sticky offsets and clips, and clamp
//! to each scrollport's range, without laying anything out again.

use rustic_base::CancellationToken;
use rustic_layout::{
    AxisRange, BlockLayout, BoxTree, CssPx, CssSize, LayoutLimits, ScrollPositions,
};

mod common;

const VIEWPORT: (f64, f64) = (300.0, 200.0);

fn viewport() -> CssSize {
    CssSize::new(VIEWPORT.0, VIEWPORT.1).unwrap()
}

fn layout(html: &str, css: &str) -> BlockLayout {
    let (doc, styles) = common::page(html, &format!("* {{ margin:0 }} {css}"));
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
        viewport(),
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap()
}

fn top_left(l: &BlockLayout, i: usize) -> (f64, f64) {
    let border = l.fragments()[i].border();
    (border.origin().x.get(), border.origin().y.get())
}

fn range(start: f64, end: f64) -> Option<AxisRange> {
    Some(AxisRange {
        start: CssPx::new(start).unwrap(),
        end: CssPx::new(end).unwrap(),
    })
}

// Fragment indexes: 0 html, 1 body, 2 main, then main's descendants.

#[test]
fn the_viewport_scrolls_content_but_not_fixed_boxes_within_its_range() {
    let l = layout(
        "<main><div id=tall></div><div id=fixed></div></main>",
        "#tall {height:1000px; width:500px} \
         #fixed {position:fixed; top:5px; left:5px; width:10px; height:10px}",
    );
    let range = l.viewport_scroll_range(viewport());
    assert_eq!(range.min, (0.0, 0.0));
    assert_eq!(range.max, (200.0, 800.0));
    let mut positions = ScrollPositions::new();
    positions.set_viewport(50.0, 300.0);
    let s = l.scrolled(viewport(), &positions).unwrap();
    assert_eq!(top_left(&s, 3), (-50.0, -300.0));
    assert_eq!(top_left(&s, 4), (5.0, 5.0), "fixed boxes stay put");
    // Positions clamp to the range; zero is the identity.
    positions.set_viewport(-10.0, 5000.0);
    let s = l.scrolled(viewport(), &positions).unwrap();
    assert_eq!(top_left(&s, 3), (0.0, -800.0));
    assert_eq!(l.scrolled(viewport(), &ScrollPositions::new()).unwrap(), l);
}

#[test]
fn nested_scroll_containers_move_their_contents_and_clips() {
    let l = layout(
        "<main><div id=outer><div id=inner><div id=item></div></div></div></main>",
        "#outer {overflow:hidden; height:100px; width:100px} \
         #inner {overflow:auto; height:400px; width:80px; margin-top:20px} \
         #item {height:1000px}",
    );
    let (outer, inner, item) = (3, 4, 5);
    assert_eq!(l.scroll_range(outer).unwrap().max, (0.0, 320.0));
    assert_eq!(l.scroll_range(inner).unwrap().max, (0.0, 600.0));
    assert!(l.scroll_range(item).is_none());
    let mut positions = ScrollPositions::new();
    positions.set_box(outer, 0.0, 30.0);
    positions.set_box(inner, 0.0, 100.0);
    // Not a scroll container: ignored.
    positions.set_box(item, 0.0, 999.0);
    let s = l.scrolled(viewport(), &positions).unwrap();
    // The inner box moves with the outer one; its contents with both.
    assert_eq!(top_left(&s, inner), (0.0, -10.0));
    assert_eq!(top_left(&s, item), (0.0, -110.0));
    // Clips follow the moved scrollports: the item is clipped by the outer
    // padding box (unmoved) and the inner one (moved up by 30).
    let clip = s.fragments()[item].clip();
    assert_eq!(clip.y, range(0.0, 100.0));
    assert_eq!(s.fragments()[inner].clip().y, range(0.0, 100.0));
    // The inner box's scrollable overflow moved with it.
    let area = s.fragments()[inner].scrollable_overflow().unwrap();
    assert_eq!(area.origin().y.get(), -10.0);
}

#[test]
fn sticky_boxes_take_their_offset_at_the_new_scroll_position() {
    // A sticky box with top:20px at y=50 in a 1000px page.
    let l = layout(
        "<main><div id=gap></div><div id=sticky></div><div id=rest></div></main>",
        "#gap {height:50px} #sticky {position:sticky; top:20px; height:10px} \
         #rest {height:1000px}",
    );
    assert_eq!(top_left(&l, 4), (0.0, 50.0));
    let at = |y: f64| {
        let mut positions = ScrollPositions::new();
        positions.set_viewport(0.0, y);
        let s = l.scrolled(viewport(), &positions).unwrap();
        (top_left(&s, 4).1, top_left(&s, 5).1)
    };
    // Before it reaches the inset it scrolls with the page; after, it
    // stays 20px below the top while the rest scrolls on.
    assert_eq!(at(20.0), (30.0, 40.0));
    assert_eq!(at(100.0), (20.0, -40.0));
    assert_eq!(at(500.0), (20.0, -440.0));
}

#[test]
fn absolute_boxes_scroll_with_their_containing_block_not_their_parent() {
    let l = layout(
        "<main><div id=port><div id=abs></div><div id=filler></div></div></main>",
        "main {position:relative} \
         #port {overflow:auto; height:50px} #filler {height:500px} \
         #abs {position:absolute; top:10px; left:0; width:10px; height:10px}",
    );
    let (port, abs, filler) = (3, 4, 5);
    let mut positions = ScrollPositions::new();
    positions.set_box(port, 0.0, 100.0);
    let s = l.scrolled(viewport(), &positions).unwrap();
    assert_eq!(top_left(&s, filler).1, -100.0);
    // `abs`'s containing block is `main`, outside the scrollport: it
    // neither scrolls nor is clipped by it.
    assert_eq!(top_left(&s, abs), (0.0, 10.0));
    assert_eq!(s.fragments()[abs].clip().y, None);
}

#[test]
fn a_scroll_containers_contents_can_be_unclipped_for_a_layer() {
    let l = layout(
        "<main><div id=port><div id=inner><div id=item></div></div></div></main>",
        "main {overflow:hidden; height:150px} #port {overflow:auto; height:50px} \
         #inner {overflow:hidden; height:30px} #item {height:500px}",
    );
    let (port, inner, item) = (3, 4, 5);
    assert_eq!(l.fragments()[item].clip().y, range(0.0, 30.0));
    let open = l.without_clips_above(port);
    // Neither the port nor `main` clips its contents any more; `inner`
    // still clips its own.
    assert_eq!(open.fragments()[inner].clip(), rustic_layout::Clip::NONE);
    assert_eq!(open.fragments()[item].clip().y, range(0.0, 30.0));
    // The port itself keeps its ancestors' clip.
    assert_eq!(open.fragments()[port].clip(), l.fragments()[port].clip());
}
