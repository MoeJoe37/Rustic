//! REN-02b: relative offsets, absolute/fixed positioning, floats, clearance
//! and overflow clipping in the block reference.

use rustic_base::CancellationToken;
use rustic_css::Overflow;
use rustic_layout::{
    AxisRange, BlockLayout, BoxTree, CssPx, CssSize, FlowError, LayoutLimits, PositionScheme,
};

mod common;

const RESET: &str = "* { margin:0; }";

fn layout(html: &str, css: &str) -> BlockLayout {
    try_layout(html, css).unwrap()
}

fn try_layout(html: &str, css: &str) -> Result<BlockLayout, FlowError> {
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
}

fn rect(r: rustic_layout::CssRect) -> (f64, f64, f64, f64) {
    (
        r.origin().x.get(),
        r.origin().y.get(),
        r.size().width().get(),
        r.size().height().get(),
    )
}

fn border(l: &BlockLayout, i: usize) -> (f64, f64, f64, f64) {
    rect(l.fragments()[i].border())
}

fn range(start: f64, end: f64) -> Option<AxisRange> {
    Some(AxisRange {
        start: CssPx::new(start).unwrap(),
        end: CssPx::new(end).unwrap(),
    })
}

// Fragment indexes: 0 html, 1 body, 2 main, then main's descendants.

#[test]
fn relative_offsets_shift_a_subtree_but_not_its_siblings() {
    let l = layout(
        "<main><div id=a><div id=c></div></div><div id=b></div></main>",
        "#a {position:relative;left:10px;top:5px;height:20px} #c {height:10px} #b {height:30px}",
    );
    assert_eq!(border(&l, 3), (10.0, 5.0, 300.0, 20.0));
    assert_eq!(border(&l, 4), (10.0, 5.0, 300.0, 10.0));
    assert_eq!(border(&l, 5), (0.0, 20.0, 300.0, 30.0));
    let a = l.fragments()[3];
    assert_eq!(a.scheme(), PositionScheme::Relative);
    assert_eq!(
        (a.relative_offset().x.get(), a.relative_offset().y.get()),
        (10.0, 5.0)
    );
    // The parent's auto height ignores the shift.
    assert_eq!(border(&l, 2).3, 50.0);

    for (css, expected) in [
        ("right:10px;bottom:4px", (-10.0, -4.0)),
        ("left:10px;right:20px;top:3px;bottom:9px", (10.0, 3.0)),
        ("left:10%;top:10%", (30.0, 0.0)),
    ] {
        let l = layout(
            "<main><div id=a></div></main>",
            &format!("#a {{position:relative;height:10px;{css}}}"),
        );
        let o = l.fragments()[3].relative_offset();
        assert_eq!((o.x.get(), o.y.get()), expected, "{css}");
    }
    // RTL containing blocks keep `right`; a definite height resolves `top:%`.
    let l = layout(
        "<main><div id=a></div></main>",
        "main {direction:rtl;height:100px} #a {position:relative;left:10px;right:20px;top:10%}",
    );
    let o = l.fragments()[3].relative_offset();
    assert_eq!((o.x.get(), o.y.get()), (-20.0, 10.0));
}

const CB: &str = "main {position:relative;padding:10px;border:5px solid;width:200px;height:100px}";

#[test]
fn absolute_boxes_use_the_positioned_ancestor_padding_box() {
    let l = layout(
        "<main><section><div id=abs></div></section></main>",
        &format!(
            "{CB} section {{height:40px}} #abs {{position:absolute;left:7px;top:3px;width:50px;height:20px}}"
        ),
    );
    assert_eq!(border(&l, 2), (0.0, 0.0, 230.0, 130.0));
    assert_eq!(rect(l.fragments()[3].content()), (15.0, 15.0, 200.0, 40.0));
    let abs = l.fragments()[4];
    assert_eq!(rect(abs.border()), (12.0, 8.0, 50.0, 20.0));
    assert_eq!(abs.scheme(), PositionScheme::Absolute);
    assert_eq!(abs.containing_block(), Some(2));
    assert_eq!(abs.parent(), Some(3));

    for (css, expected) in [
        (
            "right:10px;bottom:10px;width:50px;height:20px",
            (165.0, 95.0, 50.0, 20.0),
        ),
        ("left:0;right:0;top:0;bottom:0", (5.0, 5.0, 220.0, 120.0)),
        (
            "left:0;right:0;width:100px;margin:0 auto;top:0;height:10px",
            (65.0, 5.0, 100.0, 10.0),
        ),
        (
            "left:10px;right:10px;width:100px;top:0;height:10px",
            (15.0, 5.0, 100.0, 10.0),
        ),
        (
            "top:0;bottom:0;height:50px;margin:auto 0;left:0;width:10px",
            (5.0, 40.0, 10.0, 50.0),
        ),
        (
            "left:0;right:0;max-width:50px;top:0;height:10px",
            (5.0, 5.0, 50.0, 10.0),
        ),
        (
            "left:0;right:0;top:0;bottom:0;min-width:300px;min-height:150px",
            (5.0, 5.0, 300.0, 150.0),
        ),
        (
            "left:10%;top:10%;width:50%;height:50%",
            (27.0, 17.0, 110.0, 60.0),
        ),
    ] {
        let l = layout(
            "<main><div id=abs></div></main>",
            &format!("{CB} #abs {{position:absolute;{css}}}"),
        );
        assert_eq!(border(&l, 3), expected, "{css}");
    }
    // Over-constrained in an RTL containing block: `left` is ignored.
    let l = layout(
        "<main><div id=abs></div></main>",
        &format!(
            "{CB} main {{direction:rtl}} #abs {{position:absolute;left:10px;right:10px;width:100px;height:10px}}"
        ),
    );
    assert_eq!(border(&l, 3).0, 5.0 + 110.0);
}

#[test]
fn absolute_static_position_shrink_to_fit_and_content_heights() {
    // No insets: the static position follows the preceding in-flow block.
    let l = layout(
        "<main><div id=a></div><div id=abs><div id=k></div></div><div id=b></div></main>",
        &format!(
            "{CB} #a {{height:30px}} #b {{height:5px}} #abs {{position:absolute}} #k {{width:80px;height:12px}}"
        ),
    );
    assert_eq!(border(&l, 4), (15.0, 45.0, 80.0, 12.0));
    // Out of flow: the next sibling follows `a` directly.
    assert_eq!(border(&l, 6), (15.0, 45.0, 200.0, 5.0));
    // RTL static parents anchor `right`; shrink-to-fit is capped by the space.
    let l = layout(
        "<main><div id=abs><div id=k></div></div></main>",
        &format!(
            "{CB} main {{direction:rtl}} #abs {{position:absolute;top:0}} #k {{width:80px;height:1px}}"
        ),
    );
    assert_eq!(border(&l, 3), (135.0, 5.0, 80.0, 1.0));
    let l = layout(
        "<main><div id=abs><div id=k></div></div></main>",
        &format!("{CB} #abs {{position:absolute;left:150px;top:0}} #k {{max-width:100px}}"),
    );
    assert_eq!(border(&l, 3).2, 0.0);
    // Children see a definite height only when it is not content-based.
    let l = layout(
        "<main><div id=abs><div id=k></div></div></main>",
        &format!(
            "{CB} #abs {{position:absolute;top:0;bottom:0;left:0}} #k {{height:50%;width:4px}}"
        ),
    );
    assert_eq!(border(&l, 4).3, 60.0);
    let l = layout(
        "<main><div id=abs><div id=k></div><div id=j></div></div></main>",
        &format!(
            "{CB} #abs {{position:absolute;top:0;left:0}} #k {{height:50%;width:4px}} #j {{height:7px}}"
        ),
    );
    assert_eq!(border(&l, 4).3, 0.0);
    assert_eq!(border(&l, 3).3, 7.0);
}

#[test]
fn initial_containing_block_viewport_and_nested_absolute_boxes() {
    let l = layout(
        "<main><div id=abs></div><div id=fix></div></main>",
        "main {height:20px} #abs {position:absolute;right:0;bottom:0;width:10px;height:10px} #fix {position:fixed;right:5px;bottom:5px;width:10px;height:10px}",
    );
    assert_eq!(border(&l, 3), (290.0, 190.0, 10.0, 10.0));
    assert_eq!(l.fragments()[3].containing_block(), None);
    assert_eq!(border(&l, 4), (285.0, 185.0, 10.0, 10.0));
    assert_eq!(l.fragments()[4].scheme(), PositionScheme::Fixed);
    // A fixed box ignores positioned ancestors but its static position
    // includes their relative offsets.
    let l = layout(
        "<main><div id=fix></div></main>",
        "main {position:relative;top:50px;left:4px;padding:3px} #fix {position:fixed;width:10px;height:10px}",
    );
    assert_eq!(border(&l, 3), (7.0, 53.0, 10.0, 10.0));
    assert_eq!(l.fragments()[3].containing_block(), None);
    // Absolute inside absolute.
    let l = layout(
        "<main><div id=outer><div id=inner></div></div></main>",
        &format!(
            "{CB} #outer {{position:absolute;left:20px;top:10px;width:100px;height:60px;border:2px solid}} #inner {{position:absolute;left:5px;bottom:5px;width:10px;height:10px}}"
        ),
    );
    assert_eq!(border(&l, 3), (25.0, 15.0, 104.0, 64.0));
    assert_eq!(border(&l, 4), (32.0, 62.0, 10.0, 10.0));
    assert_eq!(l.fragments()[4].containing_block(), Some(3));
}

#[test]
fn floats_stack_wrap_below_and_do_not_grow_non_root_parents() {
    let l = layout(
        "<main><div id=l1></div><div id=l2></div><div id=r></div><div id=l3></div></main>",
        "main div {float:left;width:100px} #l1 {height:20px} #l2 {height:30px} #r {float:right;width:50px;height:10px} #l3 {height:10px}",
    );
    assert_eq!(border(&l, 3), (0.0, 0.0, 100.0, 20.0));
    assert_eq!(border(&l, 4), (100.0, 0.0, 100.0, 30.0));
    assert_eq!(border(&l, 5), (250.0, 0.0, 50.0, 10.0));
    assert_eq!(border(&l, 6), (200.0, 10.0, 100.0, 10.0));
    assert_eq!(l.fragments()[3].scheme(), PositionScheme::FloatLeft);
    assert_eq!(l.fragments()[5].scheme(), PositionScheme::FloatRight);
    assert_eq!(border(&l, 2).3, 0.0);
    // The root establishes the formatting context and contains its floats.
    assert_eq!(border(&l, 0).3, 30.0);

    // Shrink-to-fit width, margins and logical sides in RTL.
    let l = layout(
        "<main><div id=f><div id=k></div></div><div id=g></div></main>",
        "main {direction:rtl} #f {float:inline-start;margin:5px} #k {width:40px;height:10px} #g {float:inline-end;width:20px;height:20px}",
    );
    assert_eq!(border(&l, 3), (255.0, 5.0, 40.0, 10.0));
    assert_eq!(border(&l, 5), (0.0, 0.0, 20.0, 20.0));
}

#[test]
fn clearance_moves_blocks_below_selected_floats() {
    let html = "<main><div id=f></div><div id=c></div></main>";
    let base = "#f {float:left;width:50px;height:40px} #c {height:10px;";
    let l = layout(html, &format!("{base}clear:left;margin-top:10px}}"));
    assert_eq!(border(&l, 3).1, 0.0);
    assert_eq!(border(&l, 4).1, 40.0);
    assert_eq!(border(&l, 2), (0.0, 0.0, 300.0, 50.0));
    // A hypothetical position already below the float needs no clearance.
    let l = layout(html, &format!("{base}clear:both;margin-top:45px}}"));
    assert_eq!(border(&l, 4).1, 45.0);
    // Clearing the other side ignores the float; margins still collapse.
    let l = layout(html, &format!("{base}clear:right;margin-top:10px}}"));
    assert_eq!(border(&l, 4).1, 10.0);
    assert_eq!(border(&l, 2).1, 10.0);
    // `clear:inline-end` in RTL is the left side.
    let l = layout(
        html,
        &format!("main {{direction:rtl;padding-top:1px}} {base}clear:inline-end}}"),
    );
    assert_eq!(border(&l, 4).1, 41.0);
    // Floats can clear earlier floats too.
    let l = layout(
        "<main><div id=a></div><div id=b></div></main>",
        "main div {float:left;width:10px;height:10px} #b {clear:left}",
    );
    assert_eq!(border(&l, 4), (0.0, 10.0, 10.0, 10.0));
}

#[test]
fn formatting_roots_avoid_floats_narrow_and_move_down() {
    let html = "<main><div id=f></div><div id=m></div></main>";
    let l = layout(
        html,
        "#f {float:left;width:100px;height:50px} #m {overflow:hidden;height:30px;margin-left:20px}",
    );
    assert_eq!(border(&l, 4), (100.0, 0.0, 200.0, 30.0));
    let l = layout(
        html,
        "#f {float:left;width:100px;height:50px} #m {display:flow-root;width:250px;height:30px}",
    );
    assert_eq!(border(&l, 4), (0.0, 50.0, 250.0, 30.0));
    // An auto-height root that later meets a wider float is laid out again in
    // the narrower band over its extent, then lower if it cannot fit.
    let html = "<main><div id=a></div><div id=b></div><div id=m><div id=k></div></div></main>";
    let floats = "#a {float:left;width:100px;height:10px} #b {float:left;width:250px;height:10px} #k {height:30px}";
    let l = layout(html, &format!("{floats} #m {{display:flow-root}}"));
    assert_eq!(border(&l, 4), (0.0, 10.0, 250.0, 10.0));
    assert_eq!(border(&l, 5), (250.0, 0.0, 50.0, 30.0));
    assert_eq!(border(&l, 6), (250.0, 0.0, 50.0, 30.0));
    let l = layout(
        html,
        &format!("{floats} #m {{display:flow-root;min-width:100px}}"),
    );
    assert_eq!(border(&l, 5), (0.0, 20.0, 300.0, 30.0));
    // A formatting root's auto height contains its floats; a block's does not.
    let l = layout(
        "<main><div id=o><div id=f></div></div><div id=p><div id=g></div></div></main>",
        "#o {overflow:hidden} #p {} #f, #g {float:left;width:50px;height:40px}",
    );
    assert_eq!(border(&l, 3).3, 40.0);
    assert_eq!(border(&l, 5), (0.0, 40.0, 300.0, 0.0));
}

/// A row-raster float model: 10px rows, moved one row at a time.
fn raster_floats(floats: &[(bool, u32, u32)]) -> Vec<(u32, u32)> {
    let width = 300;
    let mut rows: Vec<(u32, u32)> = Vec::new(); // per row: (left edge, right edge)
    let mut out = Vec::new();
    let mut min_row = 0usize;
    for &(left, w, h) in floats {
        let hr = (h / 10) as usize;
        let mut r = min_row;
        loop {
            while rows.len() < r + hr {
                rows.push((0, width));
            }
            let span = &rows[r..r + hr];
            let l = span.iter().map(|s| s.0).max().unwrap();
            let rr = span.iter().map(|s| s.1).min().unwrap();
            let untouched = l == 0 && rr == width;
            if untouched || rr >= l + w {
                let x = if left { l } else { rr.saturating_sub(w) };
                for s in &mut rows[r..r + hr] {
                    if left {
                        s.0 = s.0.max(x + w);
                    } else {
                        s.1 = s.1.min(x);
                    }
                }
                out.push((x, r as u32 * 10));
                min_row = r;
                break;
            }
            r += 1;
        }
    }
    out
}

#[test]
fn float_placement_matches_an_independent_row_raster_model() {
    let mut seed = 0x2545_f491_u32;
    let mut next = |n: u32| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed % n
    };
    let mut checked = 0;
    for _ in 0..150 {
        let floats: Vec<(bool, u32, u32)> = (0..8)
            .map(|_| (next(2) == 0, 10 * (1 + next(20)), 10 * (1 + next(5))))
            .collect();
        let mut html = String::from("<main>");
        let mut css = String::new();
        for (i, (left, w, h)) in floats.iter().enumerate() {
            html.push_str(&format!("<div id=f{i}></div>"));
            let side = if *left { "left" } else { "right" };
            css.push_str(&format!("#f{i} {{float:{side};width:{w}px;height:{h}px}} "));
        }
        let l = layout(&html, &css);
        for (i, (x, y)) in raster_floats(&floats).into_iter().enumerate() {
            let b = border(&l, 3 + i);
            assert_eq!((b.0, b.1), (f64::from(x), f64::from(y)), "{css}");
            checked += 1;
        }
    }
    assert_eq!(checked, 1200);
}

#[test]
fn overflow_clips_follow_the_containing_block_chain() {
    let l = layout(
        "<main><div id=s><div id=t></div></div></main>",
        "#s {overflow:hidden;height:50px;padding:5px} #t {height:100px;width:400px}",
    );
    let s = l.fragments()[3];
    assert_eq!(s.overflow(), (Overflow::Hidden, Overflow::Hidden));
    let t = l.fragments()[4];
    assert_eq!(t.clip().x, range(0.0, 300.0));
    assert_eq!(t.clip().y, range(0.0, 60.0));
    assert_eq!(
        rect(s.scrollable_overflow().unwrap()),
        (0.0, 0.0, 405.0, 105.0)
    );
    assert!(s.clip().is_none());
    // The clipped child does not extend the viewport's scrollable area.
    assert_eq!(
        rect(l.viewport_scrollable_overflow()),
        (0.0, 0.0, 300.0, 200.0)
    );

    // `overflow-x:clip` clips one axis, is no scroll container and keeps
    // margin collapse; its vertical overflow still reaches the viewport.
    let l = layout(
        "<main><div id=s><div id=t></div></div></main>",
        "#s {overflow-x:clip;height:50px} #t {height:300px;width:400px;margin-top:7px}",
    );
    let (s, t) = (l.fragments()[3], l.fragments()[4]);
    assert_eq!(s.overflow(), (Overflow::Clip, Overflow::Visible));
    assert_eq!(s.scrollable_overflow(), None);
    assert_eq!(border(&l, 3).1, 7.0);
    assert_eq!((t.clip().x, t.clip().y), (range(0.0, 300.0), None));
    assert_eq!(
        rect(l.viewport_scrollable_overflow()),
        (0.0, 0.0, 300.0, 307.0)
    );

    // An absolute box escapes a clip between it and its containing block,
    // but not its containing block's own clip.
    let html = "<main><div id=s><div id=abs></div></div></main>";
    let css = "main {position:relative} #s {overflow:hidden;height:20px} #abs {position:absolute;top:30px;width:5px;height:5px}";
    let l = layout(html, css);
    assert!(l.fragments()[4].clip().is_none());
    assert_eq!(
        rect(l.fragments()[3].scrollable_overflow().unwrap()),
        (0.0, 0.0, 300.0, 20.0)
    );
    let l = layout(html, &format!("{css} #s {{position:relative}}"));
    assert_eq!(l.fragments()[4].clip().y, range(0.0, 20.0));
    assert_eq!(
        rect(l.fragments()[3].scrollable_overflow().unwrap()),
        (0.0, 0.0, 300.0, 35.0)
    );
}

#[test]
fn scrollable_overflow_is_clamped_to_the_scroll_origin() {
    let l = layout(
        "<main><div id=s><div id=t></div></div></main>",
        "#s {overflow:auto;height:50px} #t {margin-left:-50px;margin-top:-20px;width:100px;height:10px}",
    );
    assert_eq!(
        rect(l.fragments()[3].scrollable_overflow().unwrap()),
        (0.0, 0.0, 300.0, 50.0)
    );
    let l = layout(
        "<main><div id=s><div id=t></div></div></main>",
        "#s {overflow:scroll;direction:rtl;height:50px} #t {width:400px;height:10px}",
    );
    assert_eq!(border(&l, 4).0, -100.0);
    assert_eq!(
        rect(l.fragments()[3].scrollable_overflow().unwrap()),
        (-100.0, 0.0, 400.0, 50.0)
    );
}

#[test]
fn root_and_body_overflow_propagate_to_the_viewport() {
    let l = layout(
        "<main></main>",
        "html {overflow:hidden} body {overflow:scroll}",
    );
    assert_eq!(l.viewport_overflow(), (Overflow::Hidden, Overflow::Hidden));
    assert_eq!(
        l.fragments()[0].overflow(),
        (Overflow::Visible, Overflow::Visible)
    );
    assert_eq!(
        l.fragments()[1].overflow(),
        (Overflow::Scroll, Overflow::Scroll)
    );
    let l = layout(
        "<main id=m></main>",
        "body {overflow:hidden} main {margin-top:9px;height:1px}",
    );
    assert_eq!(l.viewport_overflow(), (Overflow::Hidden, Overflow::Hidden));
    assert_eq!(
        l.fragments()[1].overflow(),
        (Overflow::Visible, Overflow::Visible)
    );
    // The body is no longer a formatting root, so main's margin escapes it.
    assert_eq!(border(&l, 1).1, 9.0);
    let l = layout("<main></main>", "");
    assert_eq!(l.viewport_overflow(), (Overflow::Auto, Overflow::Auto));
}

#[test]
fn dump_appends_positioning_and_overflow_fields_only_when_needed() {
    let l = layout(
        "<main><div id=s><div id=abs></div></div><div id=f></div></main>",
        "main {position:relative;left:1px} #s {overflow:hidden;height:20px} #abs {position:absolute;width:5px;height:5px} #f {float:right;width:5px;height:5px}",
    );
    let dump = l.dump(100_000).unwrap();
    let lines: Vec<&str> = dump.lines().collect();
    assert!(!lines[1].contains("scheme="), "{}", lines[1]);
    assert!(
        lines[2].ends_with("scheme=relative offset=(1,0)"),
        "{}",
        lines[2]
    );
    assert!(
        lines[3].ends_with("overflow=hidden/hidden scroll=(1,0,300,20)"),
        "{}",
        lines[3]
    );
    assert!(lines[4].ends_with("scheme=absolute cb=2"), "{}", lines[4]);
    assert!(lines[5].ends_with("scheme=float-right"), "{}", lines[5]);
    assert_eq!(l.dump(dump.len()).unwrap(), dump);
    assert_eq!(l.dump(dump.len() - 1), Err(FlowError::OutputLimit));
}

#[test]
fn sticky_boxes_stay_in_flow_and_shift_inside_their_containing_block() {
    // Viewport scrollport 300x200. A sticky box with `top:20px` at y=0 is
    // pushed down 20px at the initial scroll position; its sibling is not.
    let l = layout(
        "<main><div id=s><p></p></div><div id=n></div></main>",
        "main {height:100px} #s {position:sticky; top:20px; height:10px} \
         #n {height:10px} p {height:5px}",
    );
    let s = l.fragments()[3];
    assert_eq!(s.scheme(), PositionScheme::Sticky);
    assert_eq!(border(&l, 3), (0.0, 20.0, 300.0, 10.0));
    assert_eq!(s.relative_offset().y.get(), 20.0);
    // Descendants move with it; the following sibling keeps its flow position.
    assert_eq!(border(&l, 4), (0.0, 20.0, 300.0, 5.0));
    assert_eq!(border(&l, 5), (0.0, 10.0, 300.0, 10.0));
    let c = s.sticky().unwrap();
    assert_eq!(c.scroll_container(), None);
    assert_eq!(rect(c.static_border()), (0.0, 0.0, 300.0, 10.0));
    assert_eq!(rect(c.containing_block()), (0.0, 0.0, 300.0, 100.0));
    assert_eq!(c.insets(), [Some(20.0), None, None, None]);
    // Scrolling 50px: the view top is at 70, the box sticks there...
    let at = |y: f64| rustic_layout::CssPoint {
        x: CssPx::ZERO,
        y: CssPx::new(y).unwrap(),
    };
    assert_eq!(c.offset(at(50.0)).unwrap().y.get(), 70.0);
    // ...until its margin box reaches the containing block's bottom (90).
    assert_eq!(c.offset(at(200.0)).unwrap().y.get(), 90.0);
    assert!(
        l.dump(1 << 16)
            .unwrap()
            .contains("scheme=sticky offset=(0,20)")
    );
}

#[test]
fn sticky_boxes_use_the_nearest_scroll_container_and_bottom_insets() {
    let l = layout(
        "<main><section><div id=s></div></section></main>",
        "main {overflow:hidden; height:50px; padding:5px} \
         section {height:200px} \
         #s {position:sticky; bottom:10%; height:10px}",
    );
    // The scrollport is main's padding box (0..60): the view's bottom edge
    // is at 60 - 6 = 54, so the box at 5..15 is already inside it.
    let s = l.fragments()[4];
    let c = s.sticky().unwrap();
    assert_eq!(c.scroll_container(), Some(2));
    assert_eq!(rect(c.scrollport()), (0.0, 0.0, 300.0, 60.0));
    assert_eq!(c.insets()[2], Some(6.0));
    assert_eq!(border(&l, 4), (5.0, 5.0, 290.0, 10.0));
    // A box below the view's bottom moves up, but not above its containing
    // block's top.
    let l = layout(
        "<main><section><p></p><div id=s></div></section></main>",
        "main {overflow:hidden; height:50px} section {height:200px} \
         p {height:100px} #s {position:sticky; bottom:0; height:10px}",
    );
    assert_eq!(border(&l, 5), (0.0, 40.0, 300.0, 10.0));
    assert_eq!(l.fragments()[5].relative_offset().y.get(), -60.0);
}

#[test]
fn sticky_boxes_are_containing_blocks_for_absolute_descendants() {
    let l = layout(
        "<main><div id=s><i></i></div></main>",
        "#s {position:sticky; top:30px; height:10px; margin-top:5px} \
         main {height:150px} i {display:block; position:absolute; top:1px; height:2px}",
    );
    // The border edge (not the margin edge) meets the view's top at 30.
    assert_eq!(border(&l, 3), (0.0, 30.0, 300.0, 10.0));
    assert_eq!(l.fragments()[4].containing_block(), Some(3));
    assert_eq!(border(&l, 4).1, 31.0);
}
