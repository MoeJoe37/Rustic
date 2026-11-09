//! REN-08: flexbox as its own formatting context (CSS Flexbox 1 §9), with
//! exact geometry for each step of the algorithm.

use rustic_base::CancellationToken;
use rustic_layout::{BlockLayout, BoxTree, CssSize, FlowError, LayoutLimits, UnsupportedFlow};

mod common;

const RESET: &str = "* { margin:0; }";

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

fn layout(html: &str, css: &str) -> BlockLayout {
    try_layout(html, css).unwrap()
}

/// Border box of fragment `i` as (x, y, width, height).
fn b(l: &BlockLayout, i: usize) -> (f64, f64, f64, f64) {
    let r = l.fragments()[i].border();
    (
        r.origin().x.get(),
        r.origin().y.get(),
        r.size().width().get(),
        r.size().height().get(),
    )
}

// Fragment indexes: 0 html, 1 body, 2 main (the flex container), then items.

#[test]
fn row_items_grow_by_their_factors_and_stretch_to_the_line() {
    let l = layout(
        "<main><div id=a><p id=pa></p></div><div id=b><p id=pb></p></div>\
         <div id=c><p id=pc></p></div></main>",
        "main {display:flex} #a {flex:1} #b {flex:2} #c {width:60px} \
         #pa {height:10px} #pb {height:30px} #pc {height:20px}",
    );
    assert_eq!(b(&l, 3), (0.0, 0.0, 80.0, 30.0));
    assert_eq!(b(&l, 5), (80.0, 0.0, 160.0, 30.0));
    assert_eq!(b(&l, 7), (240.0, 0.0, 60.0, 30.0));
    // Contents keep their own heights inside the stretched item.
    assert_eq!(b(&l, 4), (0.0, 0.0, 80.0, 10.0));
    assert_eq!(b(&l, 2), (0.0, 0.0, 300.0, 30.0));
}

#[test]
fn justify_content_and_align_items_place_fixed_items() {
    let html = "<main><div id=a></div><div id=b></div><div id=c></div></main>";
    let items = "#a {width:50px;height:20px} #b {width:50px;height:40px} \
                 #c {width:50px;height:60px}";
    for (rule, xs, ys) in [
        (
            "justify-content:space-between;align-items:center",
            [0.0, 125.0, 250.0],
            [40.0, 30.0, 20.0],
        ),
        (
            "justify-content:center;align-items:flex-end",
            [75.0, 125.0, 175.0],
            [80.0, 60.0, 40.0],
        ),
        (
            "justify-content:space-around;align-items:start",
            [25.0, 125.0, 225.0],
            [0.0, 0.0, 0.0],
        ),
        (
            "justify-content:space-evenly;align-items:end",
            [37.5, 125.0, 212.5],
            [80.0, 60.0, 40.0],
        ),
        (
            "justify-content:flex-end",
            [150.0, 200.0, 250.0],
            [0.0, 0.0, 0.0],
        ),
        (
            "justify-content:right",
            [150.0, 200.0, 250.0],
            [0.0, 0.0, 0.0],
        ),
    ] {
        let l = layout(
            html,
            &format!("{items} main {{display:flex;height:100px;{rule}}}"),
        );
        for (k, (x, y)) in xs.iter().zip(ys).enumerate() {
            let (bx, by, _, _) = b(&l, 3 + k);
            assert_eq!((bx, by), (*x, y), "{rule}: item {k}");
        }
    }
}

#[test]
fn columns_flex_within_a_definite_height_and_size_to_content_otherwise() {
    let l = layout(
        "<main><div id=a></div><div id=b></div><div id=c></div></main>",
        "main {display:flex;flex-direction:column;height:200px} \
         #a {height:20px} #b {flex:1} #c {height:30px}",
    );
    assert_eq!(b(&l, 3), (0.0, 0.0, 300.0, 20.0));
    assert_eq!(b(&l, 4), (0.0, 20.0, 300.0, 150.0));
    assert_eq!(b(&l, 5), (0.0, 170.0, 300.0, 30.0));
    // Auto height: flex:1 items keep their content heights (automatic
    // minimum size), and the container is their sum.
    let l = layout(
        "<main><div id=a><p id=pa></p></div><div id=b><p id=pb></p></div></main>",
        "main {display:flex;flex-direction:column} #a, #b {flex:1} \
         #pa {height:10px} #pb {height:20px}",
    );
    assert_eq!(b(&l, 3), (0.0, 0.0, 300.0, 10.0));
    assert_eq!(b(&l, 5), (0.0, 10.0, 300.0, 20.0));
    assert_eq!(b(&l, 2).3, 30.0);
    // Cross-axis alignment shrink-wraps items that are not stretched.
    let l = layout(
        "<main><div id=a><p id=pa></p></div></main>",
        "main {display:flex;flex-direction:column;align-items:center} #pa {width:40px;height:5px}",
    );
    assert_eq!(b(&l, 3), (130.0, 0.0, 40.0, 5.0));
}

#[test]
fn wrapping_builds_lines_and_wrap_reverse_stacks_them_upwards() {
    let html = "<main><div id=a><p id=pa></p></div><div id=b><p id=pb></p></div>\
                <div id=c><p id=pc></p></div></main>";
    let items = "main > div {width:120px} #pa {height:10px} #pb {height:20px} #pc {height:30px}";
    let l = layout(
        html,
        &format!("{items} main {{display:flex;flex-wrap:wrap}}"),
    );
    assert_eq!(b(&l, 3), (0.0, 0.0, 120.0, 20.0));
    assert_eq!(b(&l, 5), (120.0, 0.0, 120.0, 20.0));
    assert_eq!(b(&l, 7), (0.0, 20.0, 120.0, 30.0));
    assert_eq!(b(&l, 2).3, 50.0);
    let l = layout(
        html,
        &format!("{items} main {{display:flex;flex-wrap:wrap-reverse}}"),
    );
    assert_eq!(b(&l, 3), (0.0, 30.0, 120.0, 20.0));
    assert_eq!(b(&l, 7), (0.0, 0.0, 120.0, 30.0));
    // Gaps separate items and lines; align-content distributes lines.
    let l = layout(
        html,
        &format!(
            "{items} main {{display:flex;flex-wrap:wrap;gap:10px 20px;height:100px;\
             align-content:space-between}}"
        ),
    );
    assert_eq!(b(&l, 5), (140.0, 0.0, 120.0, 20.0));
    assert_eq!(b(&l, 7), (0.0, 70.0, 120.0, 30.0));
    // align-content:normal stretches lines into the free space.
    let l = layout(
        html,
        &format!("{items} main {{display:flex;flex-wrap:wrap;height:100px}}"),
    );
    assert_eq!(b(&l, 3), (0.0, 0.0, 120.0, 45.0));
    assert_eq!(b(&l, 7), (0.0, 45.0, 120.0, 55.0));
}

#[test]
fn reversed_directions_rtl_and_order_flip_the_main_axis() {
    let html = "<main><div id=a></div><div id=b></div></main>";
    let items = "main > div {width:50px;height:10px}";
    for (rule, ax, bx) in [
        ("flex-direction:row-reverse", 250.0, 200.0),
        ("direction:rtl", 250.0, 200.0),
        ("direction:rtl;flex-direction:row-reverse", 0.0, 50.0),
        ("flex-direction:row", 0.0, 50.0),
    ] {
        let l = layout(html, &format!("{items} main {{display:flex;{rule}}}"));
        assert_eq!((b(&l, 3).0, b(&l, 4).0), (ax, bx), "{rule}");
    }
    let l = layout(
        html,
        &format!("{items} main {{display:flex}} #b {{order:-1}}"),
    );
    assert_eq!((b(&l, 3).0, b(&l, 4).0), (50.0, 0.0));
    let l = layout(
        html,
        &format!("{items} main {{display:flex;flex-direction:column-reverse;height:100px}}"),
    );
    assert_eq!((b(&l, 3).1, b(&l, 4).1), (90.0, 80.0));
}

#[test]
fn auto_margins_absorb_free_space_before_alignment() {
    let l = layout(
        "<main><div id=a></div><div id=b></div></main>",
        "main {display:flex;height:100px;justify-content:center;align-items:end} \
         main > div {width:50px;height:20px} #b {margin:auto 0 auto auto}",
    );
    // #b's left margin takes all free space, so justify-content has none.
    assert_eq!(b(&l, 3), (0.0, 80.0, 50.0, 20.0));
    assert_eq!(b(&l, 4), (250.0, 40.0, 50.0, 20.0));
}

#[test]
fn automatic_minimum_sizes_stop_content_from_shrinking() {
    let l = layout(
        "<main><div id=a><p id=wide></p></div><div id=b></div></main>",
        "main {display:flex} #wide {width:200px;height:5px} \
         #b {width:200px;height:5px;overflow:hidden}",
    );
    assert_eq!(b(&l, 3).2, 200.0);
    assert_eq!(b(&l, 5), (200.0, 0.0, 100.0, 5.0));
    // min-width:0 lets both shrink in proportion to their bases.
    let l = layout(
        "<main><div id=a><p id=wide></p></div><div id=b></div></main>",
        "main {display:flex} #a {min-width:0} #wide {width:200px;height:5px} \
         #b {width:200px;height:5px;overflow:hidden}",
    );
    assert_eq!((b(&l, 3).2, b(&l, 5).2), (150.0, 150.0));
}

#[test]
fn baseline_alignment_uses_synthesized_border_box_baselines() {
    let l = layout(
        "<main><div id=a></div><div id=b></div></main>",
        "main {display:flex;align-items:baseline} #a {height:20px;width:10px;margin-top:10px} \
         #b {height:40px;width:10px}",
    );
    assert_eq!(b(&l, 3), (0.0, 20.0, 10.0, 20.0));
    assert_eq!(b(&l, 4), (10.0, 0.0, 10.0, 40.0));
    assert_eq!(b(&l, 2).3, 40.0);
}

#[test]
fn stretched_items_give_children_a_definite_height_and_replaced_items_stretch() {
    let l = layout(
        "<main><div id=a><p id=half></p></div><canvas width=100 height=50></canvas>\
         <div id=nest><i id=x></i><i id=y></i></div></main>",
        "main {display:flex;height:100px} #a {width:50px} #half {height:50%} \
         #nest {display:flex;flex:1} i {flex:1;height:10px}",
    );
    assert_eq!(b(&l, 4), (0.0, 0.0, 50.0, 50.0));
    assert_eq!(b(&l, 5), (50.0, 0.0, 100.0, 100.0));
    assert_eq!(b(&l, 6), (150.0, 0.0, 150.0, 100.0));
    assert_eq!(b(&l, 7), (150.0, 0.0, 75.0, 10.0));
    assert_eq!(b(&l, 8), (225.0, 0.0, 75.0, 10.0));
}

#[test]
fn flex_containers_shrink_to_fit_and_take_positioned_children() {
    let l = layout(
        "<main><div id=a></div><div id=b></div><div id=abs></div></main>",
        "main {display:flex;float:left;column-gap:5px;position:relative;padding:3px} \
         #a {width:40px;height:10px} #b {width:60px;height:20px} \
         #abs {position:absolute;width:5px;height:5px}",
    );
    assert_eq!(b(&l, 2), (0.0, 0.0, 111.0, 26.0));
    assert_eq!(b(&l, 4), (48.0, 3.0, 60.0, 20.0));
    // An absolutely positioned child is out of flow, at the content start.
    assert_eq!(b(&l, 5), (3.0, 3.0, 5.0, 5.0));
    // Relative items move after flex layout.
    let l = layout(
        "<main><div id=a></div></main>",
        "main {display:flex} #a {width:10px;height:10px;position:relative;left:5px;top:2px}",
    );
    assert_eq!(b(&l, 3), (5.0, 2.0, 10.0, 10.0));
}

#[test]
fn flex_text_and_inline_flex_stay_explicitly_unsupported() {
    let err = try_layout("<main>text</main>", "main {display:flex}").unwrap_err();
    assert!(matches!(
        err,
        FlowError::Unsupported {
            reason: UnsupportedFlow::TextLayout,
            ..
        }
    ));
    let err = try_layout("<main><span></span></main>", "span {display:inline-flex}").unwrap_err();
    assert!(matches!(
        err,
        FlowError::Unsupported {
            reason: UnsupportedFlow::FormattingContext,
            ..
        }
    ));
    // Whitespace between items is ignored.
    let l = layout(
        "<main>\n  <div></div>\n  <div></div>\n</main>",
        "main {display:flex} div {width:10px;height:10px}",
    );
    assert_eq!(b(&l, 4).0, 10.0);
}

#[test]
fn deep_nested_flex_containers_are_laid_out_without_recursion() {
    let depth = 120;
    let html = format!("{}{}", "<div>".repeat(depth), "</div>".repeat(depth));
    let l = layout(
        &html,
        "div {display:flex;padding-left:1px;flex-direction:column} div:empty {height:3px}",
    );
    let last = l.fragments().len() - 1;
    assert_eq!(
        b(&l, last),
        (depth as f64 - 1.0, 0.0, 300.0 - (depth as f64 - 1.0), 3.0)
    );
    assert_eq!(b(&l, 2).3, 3.0);
}

/// Invariants of flexible length resolution over generated rows: every item
/// stays within its min/max bounds; when no bound binds, the items exactly
/// fill the line; and items with equal grow factors and bases grow equally.
#[test]
fn generated_rows_fill_the_line_within_their_bounds() {
    let mut seed = 0x2545_f491_u32;
    let mut next = |n: u32| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed % n
    };
    let mut checked = 0;
    for _ in 0..60 {
        let count = 1 + next(5) as usize;
        let mut css = String::from("main {display:flex} main > div {height:1px;min-width:0} ");
        let mut bounds = Vec::new();
        for i in 0..count {
            let basis = next(8) * 10;
            let grow = next(3);
            let shrink = next(3);
            let (min, max) = match next(4) {
                0 => (next(5) as f64 * 10.0, f64::INFINITY),
                1 => (0.0, 20.0 + next(10) as f64 * 10.0),
                _ => (0.0, f64::INFINITY),
            };
            css.push_str(&format!(
                "#i{i} {{flex:{grow} {shrink} {basis}px;min-width:{min}px;{}}} ",
                if max.is_finite() {
                    format!("max-width:{max}px")
                } else {
                    String::new()
                }
            ));
            bounds.push((min, max, grow, shrink));
        }
        let html = format!(
            "<main>{}</main>",
            (0..count)
                .map(|i| format!("<div id=i{i}></div>"))
                .collect::<String>()
        );
        let l = layout(&html, &css);
        let widths: Vec<f64> = (0..count).map(|i| b(&l, 3 + i).2).collect();
        let mut binding = false;
        for (w, (min, max, _, _)) in widths.iter().zip(&bounds) {
            assert!(*w >= min - 1e-9 && *w <= max + 1e-9, "{css}: {widths:?}");
            binding |= (*w - min).abs() < 1e-9 || (*w - max).abs() < 1e-9;
        }
        let total: f64 = widths.iter().sum();
        let flexible = bounds
            .iter()
            .all(|&(_, _, g, s)| if total <= 300.0 { g > 0 } else { s > 0 });
        if flexible && !binding {
            assert!((total - 300.0).abs() < 1e-6, "{css}: {widths:?}");
            checked += 1;
        }
        // Positions are the running sum of widths.
        let mut x = 0.0;
        for (i, w) in widths.iter().enumerate() {
            assert!((b(&l, 3 + i).0 - x).abs() < 1e-9);
            x += w;
        }
    }
    assert!(checked > 5, "only {checked} unconstrained rows");
}
