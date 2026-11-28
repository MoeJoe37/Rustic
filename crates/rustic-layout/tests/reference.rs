use rustic_base::CancellationToken;
use rustic_css::{CssParser, Direction, Display, PropertyId};
use rustic_dom::{Document, NodeKind};
use rustic_layout::{
    BoxKind, BoxTree, CssPoint, CssPx, CssRect, CssSize, DeviceScale, LayoutError, LayoutLimits,
    MeasureError, measure_block_width,
};
use rustic_style::{
    ComputeContext, ComputedStyle, ElementStyle, MatchContext, document_cascade, style_tree,
};

mod common;
use common::page;

fn style(css: &str) -> ComputedStyle {
    let (_, styles) = page("<div></div>", &format!("div {{ {css} }}"));
    styles.last().unwrap().computed.clone()
}
fn build(
    doc: &Document,
    styles: &[ElementStyle],
    limits: LayoutLimits,
) -> Result<BoxTree, LayoutError> {
    BoxTree::build(
        doc,
        doc.root(),
        styles,
        limits,
        &CancellationToken::default(),
    )
}

#[test]
fn geometry_rejects_nonfinite_and_out_of_budget_edges_before_conversion() {
    for n in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1_000_000_001.0] {
        assert!(CssPx::new(n).is_err());
    }
    let limit = CssPx::new(1_000_000_000.0).unwrap();
    assert!(limit.checked_add(CssPx::new(1.0).unwrap()).is_err());
    assert!(
        CssPx::new(-1_000_000_000.0)
            .unwrap()
            .checked_sub(CssPx::new(1.0).unwrap())
            .is_err()
    );
    assert!(CssSize::new(-1.0, 0.0).is_err());
    let origin = CssPoint {
        x: limit,
        y: CssPx::ZERO,
    };
    assert!(CssRect::new(origin, CssSize::new(1.0, 1.0).unwrap()).is_err());
    assert!(CssRect::new(origin, CssSize::default()).is_ok());
    for n in [0.0, -1.0, f64::NAN, 64.1] {
        assert!(DeviceScale::new(n).is_err());
    }
    assert_eq!(
        CssPx::new(2.5)
            .unwrap()
            .to_device(DeviceScale::new(1.5).unwrap())
            .get(),
        3.75
    );
    assert_eq!(
        limit.to_device(DeviceScale::new(64.0).unwrap()).get(),
        64_000_000_000.0
    );
}

#[test]
fn auto_width_subtracts_padding_and_borders_and_resolves_vertical_percentages_against_width() {
    let s = style("margin: 3% 10px; padding: 5%; border: 2px solid; width: auto");
    let m = measure_block_width(&s, CssPx::new(200.0).unwrap(), Direction::Ltr).unwrap();
    assert_eq!(m.content_width.get(), 156.0);
    assert_eq!(m.margin.top.get(), 6.0);
    assert_eq!(m.padding.top.get(), 10.0);
    assert_eq!(m.border.left.get(), 2.0);
    assert_eq!(m.margin.right.get(), 10.0);
}

#[test]
fn fixed_auto_margins_and_overconstrained_direction_follow_the_width_equation() {
    let containing = CssPx::new(200.0).unwrap();
    let s = style("width: 80px; margin: 0 auto; padding: 10px; border: 2px solid");
    let m = measure_block_width(&s, containing, Direction::Ltr).unwrap();
    assert_eq!((m.margin.left.get(), m.margin.right.get()), (48.0, 48.0));
    let s = style("width: 250px; margin: 0 auto");
    for (direction, margins) in [
        (Direction::Ltr, (0.0, -50.0)),
        (Direction::Rtl, (-50.0, 0.0)),
    ] {
        let m = measure_block_width(&s, containing, direction).unwrap();
        assert_eq!((m.margin.left.get(), m.margin.right.get()), margins);
    }
    let s = style("width: 80px; margin-left: 10px; margin-right: 20px");
    let m = measure_block_width(&s, containing, Direction::Rtl).unwrap();
    assert_eq!((m.margin.left.get(), m.margin.right.get()), (100.0, 20.0));
}

#[test]
fn border_box_min_max_precedence_and_mixed_calc_are_used_values() {
    let s =
        style("width: calc(50% - 10px); padding: 10px; border: 2px solid; box-sizing: border-box");
    let m = measure_block_width(&s, CssPx::new(200.0).unwrap(), Direction::Ltr).unwrap();
    assert_eq!(m.content_width.get(), 66.0);
    let s = style("width: auto; min-width: 120px; max-width: 80px; margin: auto");
    let m = measure_block_width(&s, CssPx::new(200.0).unwrap(), Direction::Ltr).unwrap();
    assert_eq!(m.content_width.get(), 120.0);
    assert_eq!(m.margin.left.get(), 40.0);
    let s = style("width: 2px; padding: 10px; box-sizing: border-box");
    assert_eq!(
        measure_block_width(&s, CssPx::new(100.0).unwrap(), Direction::Ltr)
            .unwrap()
            .content_width,
        CssPx::ZERO
    );
}

#[test]
fn width_equation_holds_across_auto_negative_and_overconstrained_cases() {
    // Independent invariant: content + both margins/paddings/borders = the
    // containing width, for every accepted non-replaced block measurement.
    for width in ["auto", "0px", "80px", "250px", "50%", "calc(25% - 12px)"] {
        for margins in ["auto", "-10px", "10px auto", "auto 20px", "3% 5%"] {
            for sizing in ["content-box", "border-box"] {
                let s = style(&format!(
                    "width:{width}; margin:{margins}; padding:7px; border:2px solid; box-sizing:{sizing}"
                ));
                for direction in [Direction::Ltr, Direction::Rtl] {
                    let m = measure_block_width(&s, CssPx::new(200.0).unwrap(), direction).unwrap();
                    let total = m.content_width.get()
                        + m.margin.left.get()
                        + m.margin.right.get()
                        + m.padding.left.get()
                        + m.padding.right.get()
                        + m.border.left.get()
                        + m.border.right.get();
                    assert!((total - 200.0).abs() < 1e-9, "{width} {margins} {sizing}");
                    assert!(m.content_width.get() >= 0.0);
                }
            }
        }
    }
}

#[test]
fn unsupported_formatting_and_extreme_geometry_fail_without_clamping() {
    for css in [
        "display:inline",
        "display:flex",
        "float:left",
        "position:absolute",
    ] {
        assert_eq!(
            measure_block_width(&style(css), CssPx::new(100.0).unwrap(), Direction::Ltr),
            Err(MeasureError::UnsupportedFormatting)
        );
    }
    assert!(
        measure_block_width(
            &style("width:1e100px"),
            CssPx::new(100.0).unwrap(),
            Direction::Ltr
        )
        .is_err()
    );
    assert!(measure_block_width(&style(""), CssPx::new(-1.0).unwrap(), Direction::Ltr).is_err());
}

#[test]
fn principal_boxes_prune_none_promote_contents_and_preserve_dom_text_inheritance() {
    let (doc, styles) = page(
        "<main><div hidden><p>hidden</p></div><section><b>A</b></section><template><p>inert</p></template></main>",
        "section { display:contents; color:red } main { display:block }",
    );
    let before = doc.dump_tree(doc.root(), 100_000).unwrap();
    let tree = build(&doc, &styles, LayoutLimits::default()).unwrap();
    let boxes = tree.boxes();
    // html, body, main, b, text. head/hidden/template disappear, section promotes b.
    assert_eq!(boxes.len(), 5);
    assert_eq!(boxes[3].parent(), Some(2));
    assert_eq!(boxes[4].text(), &[65]);
    assert_eq!(
        boxes[4].style().unwrap().get(PropertyId::Color).to_string(),
        "rgb(255, 0, 0)"
    );
    assert_eq!(before, doc.dump_tree(doc.root(), 100_000).unwrap());
    assert_eq!(boxes[2].kind(), BoxKind::Element(Display::Block));
    let dump = tree.dump(100_000).unwrap();
    assert_eq!(tree.dump(dump.len()).unwrap(), dump);
    assert_eq!(tree.dump(dump.len() - 1), Err(LayoutError::OutputLimit));
    assert_eq!(
        build(&doc, &styles, LayoutLimits::default())
            .unwrap()
            .dump(100_000)
            .unwrap(),
        dump
    );
}

#[test]
fn contents_text_keeps_the_boxless_parents_inherited_style() {
    let (doc, styles) = page(
        "<main><section>text</section></main>",
        "section { display:contents; color:red }",
    );
    let tree = build(&doc, &styles, LayoutLimits::default()).unwrap();
    let text = tree.boxes().last().unwrap();
    assert_eq!(text.kind(), BoxKind::Text);
    assert_eq!(
        text.style().unwrap().get(PropertyId::Color).to_string(),
        "rgb(255, 0, 0)"
    );
}

#[test]
fn box_node_depth_text_and_cancellation_limits_are_exact() {
    let (doc, styles) = page("<div>abc</div>", "");
    let tree = build(&doc, &styles, LayoutLimits::default()).unwrap();
    let nodes = doc.walk(doc.root()).unwrap().count();
    let depth = doc.walk(doc.root()).unwrap().map(|(_, d)| d).max().unwrap();
    let limits = LayoutLimits::default()
        .with_max_nodes(nodes)
        .with_max_boxes(tree.boxes().len())
        .with_max_depth(depth)
        .with_max_text_units(3);
    assert!(build(&doc, &styles, limits).is_ok());
    for (limits, expected) in [
        (limits.with_max_nodes(nodes - 1), LayoutError::NodeLimit),
        (
            limits.with_max_boxes(tree.boxes().len() - 1),
            LayoutError::BoxLimit,
        ),
        (limits.with_max_depth(depth - 1), LayoutError::DepthLimit),
        (limits.with_max_text_units(2), LayoutError::TextLimit),
    ] {
        assert_eq!(build(&doc, &styles, limits), Err(expected));
    }
    let cancelled = CancellationToken::default();
    cancelled.cancel();
    assert_eq!(
        BoxTree::build(&doc, doc.root(), &styles, limits, &cancelled),
        Err(LayoutError::Cancelled)
    );
}

#[test]
fn stale_foreign_duplicate_and_missing_styles_are_rejected() {
    let (mut doc, styles) = page("<div>text</div>", "");
    let (foreign, other_styles) = page("<div></div>", "");
    assert!(matches!(
        build(&doc, &other_styles, LayoutLimits::default()),
        Err(LayoutError::Node(_))
    ));
    let mut duplicate = styles.clone();
    duplicate.push(styles[0].clone());
    assert_eq!(
        build(&doc, &duplicate, LayoutLimits::default()),
        Err(LayoutError::InvalidStyles)
    );
    assert!(matches!(
        build(&doc, &[], LayoutLimits::default()),
        Err(LayoutError::MissingStyle(_))
    ));
    assert!(
        BoxTree::build(
            &doc,
            foreign.root(),
            &styles,
            LayoutLimits::default(),
            &CancellationToken::default()
        )
        .is_err()
    );
    let div = styles.last().unwrap().node;
    doc.destroy_subtree(div).unwrap();
    assert!(matches!(
        build(&doc, &styles, LayoutLimits::default()),
        Err(LayoutError::Node(_))
    ));
}

#[test]
fn snapshot_owns_utf16_text_and_survives_source_mutations() {
    let (mut doc, _) = page("<div>text</div>", "");
    let text = doc
        .walk(doc.root())
        .unwrap()
        .map(|(id, _)| id)
        .find(|&id| matches!(doc.node(id).unwrap().kind(), NodeKind::Text(_)))
        .unwrap();
    doc.set_character_data(text, rustic_dom::DomString::from_units(vec![0xD800, 0x41]))
        .unwrap();
    let parser = CssParser::default();
    let (cascade, _) = document_cascade(&doc, &parser).unwrap();
    let styles = style_tree(
        &doc,
        doc.root(),
        &cascade,
        &parser,
        &MatchContext::html(),
        &ComputeContext::default(),
    )
    .unwrap();
    let tree = build(&doc, &styles, LayoutLimits::default()).unwrap();
    doc.set_character_data(text, "changed").unwrap();
    assert_eq!(tree.boxes().last().unwrap().text(), &[0xD800, 0x41]);
}

#[test]
fn shadow_hosts_render_assigned_nodes_and_omit_unassigned_light_children() {
    let (doc, styles) = page(
        "<div><template shadowrootmode=open><section><slot name=x>fallback</slot>shadow</section></template><b slot=x>assigned</b><i>hidden</i></div>",
        "",
    );
    let tree = build(&doc, &styles, LayoutLimits::default()).unwrap();
    let text = tree
        .boxes()
        .iter()
        .flat_map(|item| item.text().iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(String::from_utf16(&text).unwrap(), "assignedshadow");
}

#[test]
fn deep_boxless_chains_stay_iterative_and_keep_text_style() {
    let html = format!("{}text{}", "<div>".repeat(200), "</div>".repeat(200));
    let (doc, styles) = page(&html, "div { display:contents; color:blue }");
    let tree = build(&doc, &styles, LayoutLimits::default()).unwrap();
    assert_eq!(tree.boxes().len(), 3); // html, body, text
    assert_eq!(tree.boxes()[2].parent(), Some(1));
    assert_eq!(
        tree.boxes()[2]
            .style()
            .unwrap()
            .get(PropertyId::Color)
            .to_string(),
        "rgb(0, 0, 255)"
    );
    assert_eq!(
        build(&doc, &styles, LayoutLimits::default().with_max_depth(199)),
        Err(LayoutError::DepthLimit)
    );
}

#[test]
fn layout_geometry_converts_to_unit_typed_base_geometry() {
    use rustic_layout::{CssPoint, CssPx, CssRect, CssSize, DeviceScale};
    let rect = CssRect::new(
        CssPoint {
            x: CssPx::new(1.5).unwrap(),
            y: CssPx::new(-2.0).unwrap(),
        },
        CssSize::new(10.0, 4.0).unwrap(),
    )
    .unwrap();
    let typed = rect.to_typed().unwrap();
    assert_eq!(typed.origin().x.get(), 1.5);
    assert_eq!(typed.max_y(), 2.0);
    let scale = DeviceScale::new(2.0).unwrap().to_typed().unwrap();
    let device = scale.rect(typed).unwrap();
    let limits = rustic_base::SurfaceLimits {
        max_width: 100,
        max_height: 100,
        max_bytes: 40_000,
    };
    let surface = rustic_base::SurfaceAllocation::covering(device, 4, limits).unwrap();
    assert_eq!((surface.width(), surface.height()), (20, 8));
}
