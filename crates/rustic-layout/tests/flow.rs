use rustic_base::CancellationToken;
use rustic_layout::{BlockLayout, BoxTree, CssSize, FlowError, LayoutLimits, UnsupportedFlow};
use rustic_style::ComputeContext;

mod common;

const RESET: &str = "* { margin:0; }";

fn tree(html: &str, css: &str) -> BoxTree {
    let (doc, styles) = common::page(html, &format!("{RESET} {css}"));
    BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap()
}
fn layout(tree: &BoxTree, width: f64, height: f64) -> Result<BlockLayout, FlowError> {
    BlockLayout::compute(
        tree,
        CssSize::new(width, height).unwrap(),
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
fn unsupported(html: &str, css: &str, reason: UnsupportedFlow) {
    assert!(
        matches!(layout(&tree(html,css),300.0,200.0),Err(FlowError::Unsupported {reason:r,..}) if r == reason),
        "{html}: {css}"
    );
}

#[test]
fn nested_block_flow_has_exact_content_padding_border_and_sibling_geometry() {
    let t = tree(
        "<main><div id=a></div><div id=b></div></main>",
        "main {width:200px;padding:10px;border:2px solid} #a {width:50%;height:30px;padding:3px;border:1px solid} #b {width:80%;height:34px;padding:5px;border:2px solid;box-sizing:border-box;margin:0 auto}",
    );
    let result = layout(&t, 300.0, 200.0).unwrap();
    let f = result.fragments();
    assert_eq!(f.len(), 5);
    assert_eq!(f[3].parent(), Some(2));
    assert_eq!(f[4].parent(), Some(2));
    assert_eq!(rect(f[2].content()), (12.0, 12.0, 200.0, 72.0));
    assert_eq!(rect(f[2].padding()), (2.0, 2.0, 220.0, 92.0));
    assert_eq!(rect(f[2].border()), (0.0, 0.0, 224.0, 96.0));
    assert_eq!(rect(f[3].content()), (16.0, 16.0, 100.0, 30.0));
    assert_eq!(rect(f[3].border()), (12.0, 12.0, 108.0, 38.0));
    assert_eq!(rect(f[4].content()), (39.0, 57.0, 146.0, 20.0));
    assert_eq!(rect(f[4].border()), (32.0, 50.0, 160.0, 34.0));
    assert_eq!(f[0].content().size().height().get(), 96.0);
}

#[test]
fn fixed_height_overflow_does_not_push_a_following_sibling() {
    let t = tree(
        "<main><div id=short><div id=tall></div></div><div id=next></div></main>",
        "#short {height:20px} #tall {height:50px} #next {height:10px}",
    );
    let result = layout(&t, 300.0, 200.0).unwrap();
    let f = result.fragments();
    assert_eq!(f[4].border().size().height().get(), 50.0);
    assert_eq!(f[5].border().origin().y.get(), 20.0);
    assert_eq!(f[2].content().size().height().get(), 30.0);
}

#[test]
fn percentage_height_uses_definite_content_height_and_minimum_wins() {
    let t = tree(
        "<main><div></div></main>",
        "main {height:100px;min-height:120px;max-height:80px} div {height:50%}",
    );
    let result = layout(&t, 300.0, 200.0).unwrap();
    assert_eq!(result.fragments()[2].content().size().height().get(), 120.0);
    assert_eq!(result.fragments()[3].content().size().height().get(), 60.0);
    let t = tree(
        "<main><div></div></main>",
        "main {height:100px;padding:10px;border:2px solid;box-sizing:border-box} div {height:50%}",
    );
    let result = layout(&t, 300.0, 200.0).unwrap();
    assert_eq!(result.fragments()[2].content().size().height().get(), 76.0);
    assert_eq!(result.fragments()[3].content().size().height().get(), 38.0);
}

#[test]
fn indefinite_percentage_height_and_bounds_do_not_use_eventual_auto_height() {
    let t = tree(
        "<main><section><div></div></section></main>",
        "main {min-height:120px} section {height:50%;min-height:50%;max-height:50%} div {height:20px}",
    );
    let result = layout(&t, 300.0, 200.0).unwrap();
    assert_eq!(result.fragments()[2].content().size().height().get(), 120.0);
    assert_eq!(result.fragments()[3].content().size().height().get(), 20.0);
}

#[test]
fn calc_heights_resolve_against_definite_bases_and_clamp_nonnegative() {
    let t = tree(
        "<main><div></div></main>",
        "main {height:100px} div {height:calc(50% - 10px)}",
    );
    assert_eq!(
        layout(&t, 300.0, 200.0).unwrap().fragments()[3]
            .content()
            .size()
            .height()
            .get(),
        40.0
    );
    let t = tree("<main><div></div></main>", "div {height:calc(10px - 20px)}");
    assert_eq!(
        layout(&t, 300.0, 200.0).unwrap().fragments()[3]
            .content()
            .size()
            .height()
            .get(),
        0.0
    );
}

#[test]
fn containing_direction_controls_used_horizontal_position() {
    let t = tree(
        "<main><div></div></main>",
        "main {direction:rtl;width:200px;margin:0 auto} div {direction:ltr;width:80px;margin-left:10px;margin-right:20px;height:5px}",
    );
    let result = layout(&t, 400.0, 200.0).unwrap();
    assert_eq!(result.fragments()[2].content().origin().x.get(), 100.0);
    assert_eq!(result.fragments()[3].content().origin().x.get(), 200.0);
}

#[test]
fn resize_reflows_percentages_and_viewport_units_with_a_matching_style_snapshot() {
    let t = tree(
        "<main><div></div></main>",
        "main {width:50%} div {height:20px}",
    );
    assert_eq!(
        layout(&t, 800.0, 200.0).unwrap().fragments()[2]
            .content()
            .size()
            .width()
            .get(),
        400.0
    );
    assert_eq!(
        layout(&t, 400.0, 200.0).unwrap().fragments()[2]
            .content()
            .size()
            .width()
            .get(),
        200.0
    );
    for width in [400.0, 800.0] {
        let (doc, styles) = common::page_with_context(
            "<main></main>",
            &format!("{RESET} main {{width:50vw;height:25vh}}"),
            ComputeContext {
                viewport_width: width,
                viewport_height: 200.0,
                ..ComputeContext::default()
            },
        );
        let t = BoxTree::build(
            &doc,
            doc.root(),
            &styles,
            LayoutLimits::default(),
            &CancellationToken::default(),
        )
        .unwrap();
        let f = layout(&t, width, 200.0).unwrap().fragments()[2];
        assert_eq!(rect(f.content()), (0.0, 0.0, width / 2.0, 50.0));
    }
}

#[test]
fn whitespace_only_runs_vanish_but_preserved_or_non_css_whitespace_needs_inline_layout() {
    let t = tree("<main> \t\n <div></div>\r\n</main>", "div {height:7px}");
    let result = layout(&t, 300.0, 200.0).unwrap();
    assert_eq!(result.fragments().len(), 4);
    assert_eq!(result.fragments()[2].content().size().height().get(), 7.0);
    for content in ["x", "\u{a0}", "\u{2003}", "\u{b}"] {
        unsupported(
            &format!("<main>{content}</main>"),
            "",
            UnsupportedFlow::TextLayout,
        );
    }
    for space in ["pre", "pre-wrap", "break-spaces", "pre-line"] {
        unsupported(
            "<main> \n</main>",
            &format!("main {{white-space:{space}}}"),
            UnsupportedFlow::TextLayout,
        );
    }
}

#[test]
fn unsupported_contexts_and_controls_return_typed_errors() {
    for css in [
        "display:inline",
        "display:inline-flex",
        "display:inline-grid",
        "display:table-cell",
        "display:inline-table",
    ] {
        unsupported(
            "<main></main>",
            &format!("main {{{css}}}"),
            UnsupportedFlow::FormattingContext,
        );
    }
    // Block-level replaced elements are laid out since REN-02c (tests/sizing.rs).
    unsupported("<br>", "* {display:block}", UnsupportedFlow::SpecialElement);
}

#[test]
fn failure_does_not_change_the_snapshot_or_an_earlier_result() {
    let t = tree("<main><div></div></main>", "div {height:20px}");
    let before = t.clone();
    let result = layout(&t, 300.0, 200.0).unwrap();
    let expected = result.clone();
    let limits = LayoutLimits::default().with_max_boxes(1);
    assert_eq!(
        BlockLayout::compute(
            &t,
            CssSize::new(300.0, 200.0).unwrap(),
            limits,
            &CancellationToken::default()
        ),
        Err(FlowError::BoxLimit)
    );
    assert_eq!(t, before);
    assert_eq!(result, expected);
    let dump = result.dump(100_000).unwrap();
    assert_eq!(result.dump(dump.len()).unwrap(), dump);
    assert_eq!(result.dump(dump.len() - 1), Err(FlowError::OutputLimit));
}

#[test]
fn independent_layout_limits_and_cancellation_are_exact() {
    let t = tree("<main>   </main>", "");
    let viewport = CssSize::new(300.0, 200.0).unwrap();
    let limits = LayoutLimits::default()
        .with_max_nodes(4)
        .with_max_boxes(3)
        .with_max_depth(3)
        .with_max_text_units(3);
    assert!(BlockLayout::compute(&t, viewport, limits, &CancellationToken::default()).is_ok());
    for (limits, error) in [
        (limits.with_max_nodes(3), FlowError::NodeLimit),
        (limits.with_max_boxes(2), FlowError::BoxLimit),
        (limits.with_max_depth(2), FlowError::DepthLimit),
        (limits.with_max_text_units(2), FlowError::TextLimit),
    ] {
        assert_eq!(
            BlockLayout::compute(&t, viewport, limits, &CancellationToken::default()),
            Err(error)
        );
    }
    let token = CancellationToken::default();
    token.cancel();
    assert_eq!(
        BlockLayout::compute(&t, viewport, limits, &token),
        Err(FlowError::Cancelled)
    );
}

#[test]
fn extreme_dimensions_and_accumulated_positions_fail_without_partial_layout() {
    for css in ["height:1e100px", "height:1000000000px;padding:1px"] {
        let t = tree("<main></main>", &format!("main {{{css}}}"));
        assert!(layout(&t, 300.0, 200.0).is_err());
    }
    let t = tree(
        "<main><div></div><div></div></main>",
        "div {height:600000000px}",
    );
    assert!(matches!(
        layout(&t, 300.0, 200.0),
        Err(FlowError::Geometry(_))
    ));
}

#[test]
fn sequential_blocks_match_an_independent_prefix_sum_model() {
    let mut html = String::from("<main>");
    let mut css = String::from("main {padding:3px;border:2px solid}");
    let mut heights = Vec::new();
    for i in 0..64 {
        html.push_str(&format!("<div id=n{i}></div>"));
        let h = (i * 7) % 31;
        css.push_str(&format!(
            "#n{i} {{height:{h}px;padding:2px;border:1px solid}}"
        ));
        heights.push(h as f64 + 6.0);
    }
    html.push_str("</main>");
    let result = layout(&tree(&html, &css), 300.0, 200.0).unwrap();
    let mut y = 5.0;
    for (f, h) in result.fragments()[3..].iter().zip(heights) {
        assert_eq!(f.border().origin().y.get(), y);
        assert_eq!(f.border().size().height().get(), h);
        y += h;
    }
    assert_eq!(
        result.fragments()[0].content().size().height().get(),
        y + 5.0
    );
}

#[test]
fn deep_auto_height_chains_finish_and_dump_without_recursion() {
    let html = format!(
        "{}<section></section>{}",
        "<div>".repeat(200),
        "</div>".repeat(200)
    );
    let t = tree(&html, "section {height:7px}");
    let result = layout(&t, 300.0, 200.0).unwrap();
    assert_eq!(result.fragments().len(), 203);
    assert!(
        result
            .fragments()
            .iter()
            .all(|f| f.content().size().height().get() == 7.0)
    );
    assert!(result.dump(100_000).unwrap().len() < 203 * 220);
}

#[test]
fn many_floats_cost_work_per_row_not_per_earlier_float() {
    // TD-44: 4,000 floats in rows of 30. Scanning every earlier float would
    // spend about 8 × 10⁶ units; the live-row scan stays far below 10⁶.
    let html = format!("<main>{}</main>", "<i></i>".repeat(4_000));
    let t = tree(
        &html,
        "i { display: block; float: left; width: 10px; height: 5px }",
    );
    let viewport = CssSize::new(300.0, 200.0).unwrap();
    let limits = LayoutLimits::default().with_max_flow_work(1_000_000);
    let result = BlockLayout::compute(&t, viewport, limits, &CancellationToken::default())
        .expect("float placement stays within a linear budget");
    let last = result
        .fragments()
        .last()
        .map(|f| rect(f.content()))
        .unwrap();
    // 4,000 floats / 30 per row = 133 full rows, then 10 floats on row 134.
    assert_eq!((last.0, last.1), (90.0, 133.0 * 5.0));
    let tight = LayoutLimits::default().with_max_flow_work(1_000);
    assert_eq!(
        BlockLayout::compute(&t, viewport, tight, &CancellationToken::default()).err(),
        Some(FlowError::WorkLimit)
    );
}
