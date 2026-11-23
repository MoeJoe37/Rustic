//! CSS 2.2 §8.3.1 fixtures plus an independent margin-adjacency graph model.
use rustic_base::CancellationToken;
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_layout::{BlockLayout, BoxTree, CssSize, FlowError, LayoutLimits};
mod common;

fn render(html: &str, css: &str) -> (Document, BoxTree, BlockLayout) {
    let (doc, styles) = common::page(html, &format!("* {{margin:0}} {css}"));
    let tree = BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    let layout = BlockLayout::compute(
        &tree,
        CssSize::new(1280.0, 720.0).unwrap(),
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    (doc, tree, layout)
}
fn id(doc: &Document, name: &str) -> NodeId {
    doc.walk(doc.root())
        .unwrap()
        .map(|(n, _)| n)
        .find(|&n| match doc.node(n).unwrap().kind() {
            NodeKind::Element(e) => e.attributes().iter().any(|a| {
                a.name().local() == "id" && String::from_utf16_lossy(a.value().as_units()) == name
            }),
            _ => false,
        })
        .unwrap()
}
fn yh(doc: &Document, layout: &BlockLayout, name: &str) -> (f64, f64) {
    let node = id(doc, name);
    let f = layout
        .fragments()
        .iter()
        .find(|f| f.node() == node)
        .unwrap();
    (
        f.border().origin().y.get(),
        f.border().size().height().get(),
    )
}

#[test]
fn sibling_positive_negative_and_mixed_margins_collapse_by_extrema() {
    for (after, before, y, height) in [
        (20, 30, 40.0, 50.0),
        (-20, -30, -20.0, 0.0),
        (30, -10, 30.0, 40.0),
        (-30, 10, -10.0, 0.0),
    ] {
        let (doc, _, layout) = render(
            "<main id=p><div id=a></div><div id=b></div></main>",
            &format!(
                "main {{display:flow-root}} div {{height:10px}} #a {{margin-bottom:{after}px}} #b {{margin-top:{before}px}}"
            ),
        );
        assert_eq!(yh(&doc, &layout, "a"), (0.0, 10.0));
        assert_eq!(yh(&doc, &layout, "b"), (y, 10.0));
        assert_eq!(yh(&doc, &layout, "p"), (0.0, height));
    }
}

#[test]
fn parent_child_margins_escape_until_a_padding_border_or_flow_root_boundary() {
    for (boundary, p_y, p_h, c_y, outer_h) in [
        ("", 20.0, 10.0, 20.0, 60.0),
        ("padding-top:1px;padding-bottom:1px", 5.0, 62.0, 26.0, 74.0),
        (
            "border-top:1px solid;border-bottom:1px solid",
            5.0,
            62.0,
            26.0,
            74.0,
        ),
        ("display:flow-root", 5.0, 60.0, 25.0, 72.0),
        (
            "border-top:5px none;border-bottom:5px hidden",
            20.0,
            10.0,
            20.0,
            60.0,
        ),
    ] {
        let (doc, _, layout) = render(
            "<main id=o><section id=p><div id=c></div></section></main>",
            &format!(
                "main {{display:flow-root}} section {{margin-top:5px;margin-bottom:7px;{boundary}}} div {{height:10px;margin-top:20px;margin-bottom:30px}}"
            ),
        );
        assert_eq!(yh(&doc, &layout, "p"), (p_y, p_h), "{boundary}");
        assert_eq!(yh(&doc, &layout, "c"), (c_y, 10.0), "{boundary}");
        assert_eq!(yh(&doc, &layout, "o"), (0.0, outer_h), "{boundary}");
    }
}

#[test]
fn computed_height_and_minimum_bound_control_bottom_escape_without_changing_top_collapse() {
    for (constraint, p_h, outer_h) in [
        ("height:10px", 10.0, 37.0),
        ("min-height:30px", 30.0, 80.0),
        ("min-height:5px", 10.0, 60.0),
        ("max-height:5px", 5.0, 55.0),
    ] {
        let (doc, _, layout) = render(
            "<main id=o><section id=p><div id=c></div></section></main>",
            &format!(
                "main {{display:flow-root}} section {{margin-top:5px;margin-bottom:7px;{constraint}}} div {{height:10px;margin-top:20px;margin-bottom:30px}}"
            ),
        );
        assert_eq!(yh(&doc, &layout, "p"), (20.0, p_h), "{constraint}");
        assert_eq!(yh(&doc, &layout, "c"), (20.0, 10.0));
        assert_eq!(yh(&doc, &layout, "o"), (0.0, outer_h), "{constraint}");
    }
}

#[test]
fn empty_blocks_collapse_through_and_keep_their_specified_border_edge_position() {
    let (doc, _, layout) = render(
        "<main id=o><div id=e></div><section id=n></section></main>",
        "main {display:flow-root} #e {margin-top:10px;margin-bottom:20px} section {height:5px;margin-top:5px}",
    );
    assert_eq!(yh(&doc, &layout, "e"), (10.0, 0.0));
    assert_eq!(yh(&doc, &layout, "n"), (20.0, 5.0));
    assert_eq!(yh(&doc, &layout, "o"), (0.0, 25.0));
    let (doc, _, layout) = render(
        "<main id=o><div id=e><section id=c></section></div><article id=n></article></main>",
        "main {display:flow-root} #e {margin-top:10px;margin-bottom:50px} section {margin-top:30px;margin-bottom:20px} article {height:5px;margin-top:15px}",
    );
    assert_eq!(yh(&doc, &layout, "e"), (30.0, 0.0));
    assert_eq!(yh(&doc, &layout, "c"), (30.0, 0.0));
    assert_eq!(yh(&doc, &layout, "n"), (50.0, 5.0));
    assert_eq!(yh(&doc, &layout, "o"), (0.0, 55.0));
}

#[test]
fn a_collapsed_pair_keeps_both_extrema_when_a_later_margin_joins() {
    let (doc, _, layout) = render(
        "<main><div id=a></div><div id=e></div><div id=b></div></main>",
        "main {display:flow-root} #a,#b {height:10px} #a {margin-bottom:30px} #e {margin-top:-10px;margin-bottom:20px} #b {margin-top:-20px}",
    );
    assert_eq!(yh(&doc, &layout, "e"), (30.0, 0.0));
    assert_eq!(yh(&doc, &layout, "b"), (20.0, 10.0));
}

#[test]
fn empty_descendants_with_minimum_height_do_not_escape_both_parent_edges() {
    for (constraint, p_h, outer_h) in [
        ("", 0.0, 50.0),
        ("height:0", 0.0, 50.0),
        ("height:10px", 10.0, 65.0),
        ("min-height:30px", 30.0, 85.0),
        ("min-height:5px", 5.0, 60.0),
    ] {
        let (doc, _, layout) = render(
            "<main id=o><section id=p><div id=c></div></section></main>",
            &format!(
                "main {{display:flow-root}} section {{margin:5px 0;{constraint}}} div {{margin-top:20px;margin-bottom:50px}}"
            ),
        );
        assert_eq!(yh(&doc, &layout, "p"), (50.0, p_h), "{constraint}");
        assert_eq!(yh(&doc, &layout, "c"), (50.0, 0.0));
        assert_eq!(yh(&doc, &layout, "o"), (0.0, outer_h), "{constraint}");
    }
}

#[test]
fn single_edge_separators_keep_empty_margins_out_of_auto_content_height() {
    for (boundary, p_y, c_y) in [
        ("padding-top:1px", 5.0, 26.0),
        ("padding-bottom:1px", 50.0, 50.0),
    ] {
        let (doc, _, layout) = render(
            "<main id=o><section id=p><div id=c></div></section></main>",
            &format!(
                "main {{display:flow-root}} section {{margin:5px 0;{boundary}}} div {{margin-top:20px;margin-bottom:50px}}"
            ),
        );
        assert_eq!(yh(&doc, &layout, "p"), (p_y, 1.0));
        assert_eq!(yh(&doc, &layout, "c"), (c_y, 0.0));
        assert_eq!(yh(&doc, &layout, "o"), (0.0, 56.0));
    }
}

#[test]
fn root_edges_do_not_collapse_and_percent_margins_use_containing_width() {
    let (doc, _, layout) = render(
        "<main id=m></main>",
        "html {margin-top:10px;margin-bottom:20px} main {height:10px;margin-top:30px;margin-bottom:40px}",
    );
    assert_eq!(layout.fragments()[0].border().origin().y.get(), 10.0);
    assert_eq!(layout.fragments()[0].border().size().height().get(), 80.0);
    assert_eq!(yh(&doc, &layout, "m"), (40.0, 10.0));
    let (doc, _, layout) = render(
        "<main id=p><div id=a></div><div id=b></div></main>",
        "main {display:flow-root;width:200px} div {height:10px} #a {margin-bottom:5%} #b {margin-top:10%}",
    );
    assert_eq!(yh(&doc, &layout, "b"), (30.0, 10.0));
    assert_eq!(yh(&doc, &layout, "p"), (0.0, 40.0));
}

#[derive(Clone, Copy)]
struct ModelBox {
    top: i32,
    bottom: i32,
    height: i32,
    through: bool,
}
fn group_roots(boxes: &[ModelBox], split: Option<usize>) -> Vec<usize> {
    let mut roots: Vec<usize> = (0..boxes.len() * 2).collect();
    // Independent adjacency graph: explicit edge-component relabeling, unlike
    // the production bottom-up struts. Inputs here have at most sixteen boxes.
    for (i, current) in boxes.iter().enumerate() {
        let pairs = [
            (i > 0, i.saturating_mul(2).saturating_sub(1), i * 2),
            (current.through && split != Some(i), i * 2, i * 2 + 1),
        ];
        for (join, a, b) in pairs {
            if join {
                let old = roots[b];
                let new = roots[a];
                for r in &mut roots {
                    if *r == old {
                        *r = new
                    }
                }
            }
        }
    }
    roots
}
fn group_value(boxes: &[ModelBox], roots: &[usize], edge: usize) -> f64 {
    let mut positive = 0;
    let mut negative = 0;
    for (i, b) in boxes.iter().enumerate() {
        for (j, v) in [(2 * i, b.top), (2 * i + 1, b.bottom)] {
            if roots[j] == roots[edge] {
                positive = positive.max(v);
                negative = negative.min(v)
            }
        }
    }
    f64::from(positive + negative)
}

#[test]
fn flat_sibling_layout_matches_an_independent_margin_adjacency_graph() {
    let mut seed = 7391u64;
    let mut random = |bound: u64| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        seed % bound
    };
    for _ in 0..192 {
        let mut boxes = Vec::new();
        let mut html = String::from("<main>");
        let mut css = String::from("main {display:flow-root}");
        for i in 0..16 {
            let top = i32::try_from(random(61)).unwrap() - 30;
            let bottom = i32::try_from(random(61)).unwrap() - 30;
            let through = random(3) == 0;
            let height = if through {
                0
            } else {
                i32::try_from(random(21)).unwrap()
            };
            boxes.push(ModelBox {
                top,
                bottom,
                height,
                through,
            });
            html.push_str(&format!("<div id=n{i}></div>"));
            let context = if !through && height == 0 {
                "display:flow-root;"
            } else {
                ""
            };
            css.push_str(&format!(
                "#n{i} {{{context}height:{height}px;margin-top:{top}px;margin-bottom:{bottom}px}}"
            ));
        }
        html.push_str("</main>");
        let (_, _, layout) = render(&html, &css);
        let roots = group_roots(&boxes, None);
        let mut cursor = 0.0;
        for (i, b) in boxes.iter().enumerate() {
            let expected = if b.through {
                let split = group_roots(&boxes, Some(i));
                cursor + group_value(&boxes, &split, 2 * i)
            } else {
                cursor + group_value(&boxes, &roots, 2 * i)
            };
            assert_eq!(
                layout.fragments()[3 + i].border().origin().y.get(),
                expected,
                "box {i}"
            );
            if !b.through {
                cursor = expected + f64::from(b.height)
            }
        }
        let height = (cursor + group_value(&boxes, &roots, 2 * boxes.len() - 1)).max(0.0);
        assert_eq!(
            layout.fragments()[2].content().size().height().get(),
            height
        );
    }
}

#[test]
fn deep_collapsed_chains_are_iterative_and_fail_atomically_at_coordinate_limits() {
    let html = format!(
        "{}<section id=leaf></section>{}",
        "<div>".repeat(200),
        "</div>".repeat(200)
    );
    let (doc, tree, layout) = render(
        &html,
        "div {margin-top:3px;margin-bottom:7px} section {height:10px;margin-top:5px;margin-bottom:2px}",
    );
    assert_eq!(yh(&doc, &layout, "leaf"), (5.0, 10.0));
    assert_eq!(layout.fragments()[0].content().size().height().get(), 22.0);
    let before = tree.clone();
    let limits = LayoutLimits::default().with_max_boxes(1);
    assert_eq!(
        BlockLayout::compute(
            &tree,
            CssSize::new(1280.0, 720.0).unwrap(),
            limits,
            &CancellationToken::default()
        ),
        Err(FlowError::BoxLimit)
    );
    assert_eq!(tree, before);
    let (doc, styles) = common::page("<div></div>", "div {height:1000000000px;margin-top:1px}");
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
            CssSize::new(1280.0, 720.0).unwrap(),
            LayoutLimits::default(),
            &CancellationToken::default()
        ),
        Err(FlowError::Geometry(_))
    ));
}
