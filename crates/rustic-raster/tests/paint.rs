//! Display list construction and rasterization against the block layout.
use rustic_base::CancellationToken;
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_layout::{BlockLayout, BoxTree, CssSize, DeviceScale, LayoutLimits};
use rustic_paint::{
    DisplayCommand, DisplayList, ItemRole, PaintError, PaintLimits, Shape, Srgba, UnsupportedPaint,
};
use rustic_raster::{
    DeviceRect, RasterError, RasterLimits, Surface, TILE_SIZE, damage, encode_png, rasterize,
    rasterize_reference, rasterize_region, repaint,
};
mod common;

struct Page {
    doc: Document,
    tree: BoxTree,
    layout: BlockLayout,
    viewport: CssSize,
}

fn page(html: &str, css: &str, width: f64, height: f64) -> Page {
    let (doc, styles) = common::page(html, css);
    let tree = BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    let viewport = CssSize::new(width, height).unwrap();
    let layout = BlockLayout::compute(
        &tree,
        viewport,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    Page {
        doc,
        tree,
        layout,
        viewport,
    }
}

impl Page {
    fn list(&self) -> Result<DisplayList, PaintError> {
        self.list_with(PaintLimits::default(), &CancellationToken::default())
    }
    fn list_with(
        &self,
        limits: PaintLimits,
        cancel: &CancellationToken,
    ) -> Result<DisplayList, PaintError> {
        DisplayList::build(
            &self.doc,
            &self.tree,
            &self.layout,
            self.viewport,
            limits,
            cancel,
        )
    }
    /// The `id` of an element, `#text` for text, else the local name.
    fn id_name(&self, node: NodeId) -> String {
        match self.doc.node(node).unwrap().kind() {
            NodeKind::Element(e) => e
                .attributes()
                .iter()
                .find(|a| a.name().local() == "id")
                .map(|a| String::from_utf16_lossy(a.value().as_units()))
                .unwrap_or_else(|| e.name().local().to_string()),
            NodeKind::Text(_) => "#text".into(),
            _ => "?".into(),
        }
    }
    fn id(&self, name: &str) -> NodeId {
        let doc = &self.doc;
        doc.walk(doc.root())
            .unwrap()
            .map(|(n, _)| n)
            .find(|&n| match doc.node(n).unwrap().kind() {
                NodeKind::Element(e) => e.attributes().iter().any(|a| {
                    a.name().local() == "id"
                        && String::from_utf16_lossy(a.value().as_units()) == name
                }),
                _ => false,
            })
            .unwrap()
    }
}

const RESET: &str = "html, body { margin: 0 } ";

fn rgb(r: u8, g: u8, b: u8) -> Srgba {
    Srgba { r, g, b, a: 255 }
}

fn render(p: &Page, scale: f64) -> Surface {
    rasterize(
        &p.list().unwrap(),
        DeviceScale::new(scale).unwrap(),
        RasterLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap()
}

#[test]
fn body_background_propagates_to_the_canvas_unless_the_root_has_one() {
    let p = page(
        "<div id=a></div>",
        &format!("{RESET} body {{ background: red }} div {{ height: 10px }}"),
        20.0,
        30.0,
    );
    let list = p.list().unwrap();
    assert_eq!(list.canvas(), rgb(255, 0, 0));
    assert!(list.items().is_empty(), "{:?}", list.items());
    let surface = render(&p, 1.0);
    // The canvas, not the body's 10px-tall box, is red everywhere.
    assert_eq!(surface.pixel(5, 25), Some(rgb(255, 0, 0)));

    let p = page(
        "",
        &format!("{RESET} html {{ background: lime }} body {{ background: blue; height: 5px }}"),
        20.0,
        30.0,
    );
    let list = p.list().unwrap();
    assert_eq!(list.canvas(), rgb(0, 255, 0));
    assert_eq!(list.items().len(), 1);
    assert_eq!(list.items()[0].role, ItemRole::Background);
    let surface = render(&p, 1.0);
    assert_eq!(surface.pixel(0, 0), Some(rgb(0, 0, 255)));
    assert_eq!(surface.pixel(0, 5), Some(rgb(0, 255, 0)));

    // A translucent canvas background is composited over white.
    let p = page(
        "",
        &format!("{RESET} body {{ background: rgb(0 0 255 / 50%) }}"),
        4.0,
        4.0,
    );
    assert_eq!(p.list().unwrap().canvas(), rgb(127, 127, 255));
    // With nothing set, the canvas is white.
    let p = page("", RESET, 4.0, 4.0);
    assert_eq!(p.list().unwrap().canvas(), Srgba::WHITE);
}

#[test]
fn backgrounds_follow_background_clip_and_paint_in_preorder() {
    let p = page(
        "<div id=outer><div id=inner></div></div>",
        &format!(
            "{RESET} #outer {{ margin: 2px; padding: 3px; border: 1px solid black;
               background: red; background-clip: content-box; height: 10px }}
             #inner {{ height: 4px; background: blue }}"
        ),
        40.0,
        40.0,
    );
    let list = p.list().unwrap();
    let outer = p.id("outer");
    let inner = p.id("inner");
    let order: Vec<_> = list.items().iter().map(|i| (i.node, i.role)).collect();
    assert_eq!(
        order,
        [
            (outer, ItemRole::Background),
            (outer, ItemRole::Border),
            (inner, ItemRole::Background)
        ]
    );
    let Shape::Rect(bg) = list.items()[0].shape else {
        panic!("background is a rectangle")
    };
    assert_eq!(bg.origin().x.get(), 6.0);
    assert_eq!(bg.size().width().get(), 28.0);
    let surface = render(&p, 1.0);
    assert_eq!(surface.pixel(2, 2), Some(rgb(0, 0, 0)));
    assert_eq!(surface.pixel(4, 4), Some(Srgba::WHITE)); // padding: clipped away
    assert_eq!(surface.pixel(6, 6), Some(rgb(0, 0, 255))); // inner covers outer
    assert_eq!(surface.pixel(6, 12), Some(rgb(255, 0, 0)));
    assert_eq!(surface.pixel(6, 16), Some(Srgba::WHITE));
}

#[test]
fn matching_borders_form_one_ring_and_mixed_colors_mitre_at_corners() {
    let p = page(
        "<div id=a></div>",
        &format!("{RESET} div {{ border: 2px solid rgb(0 0 0); height: 4px; width: 4px }}"),
        10.0,
        10.0,
    );
    let list = p.list().unwrap();
    assert_eq!(list.items().len(), 1);
    assert!(matches!(list.items()[0].shape, Shape::Ring { .. }));
    let s = render(&p, 1.0);
    assert_eq!(s.pixel(0, 0), Some(rgb(0, 0, 0)));
    assert_eq!(s.pixel(3, 3), Some(Srgba::WHITE));
    assert_eq!(s.pixel(7, 7), Some(rgb(0, 0, 0)));

    let p = page(
        "<div id=a></div>",
        &format!(
            "{RESET} div {{ border: 4px solid; border-color: red lime blue black; height: 4px; width: 4px }}"
        ),
        12.0,
        12.0,
    );
    let list = p.list().unwrap();
    assert_eq!(list.items().len(), 4);
    let s = render(&p, 1.0);
    assert_eq!(s.pixel(5, 1), Some(rgb(255, 0, 0)));
    assert_eq!(s.pixel(10, 5), Some(rgb(0, 255, 0)));
    assert_eq!(s.pixel(5, 10), Some(rgb(0, 0, 255)));
    assert_eq!(s.pixel(1, 5), Some(rgb(0, 0, 0)));
    // Corner pixels on the mitre diagonal are split between the two sides.
    assert_eq!(s.pixel(2, 1), Some(rgb(255, 0, 0)));
    assert_eq!(s.pixel(1, 2), Some(rgb(0, 0, 0)));
    // Each side covers half of a diagonal pixel and is composited on its
    // own, so the canvas shows through (coverage conflation, TD-41).
    assert_eq!(s.pixel(0, 0), Some(rgb(128, 64, 64)));

    // A side with zero width paints nothing.
    let p = page(
        "<div></div>",
        &format!(
            "{RESET} div {{ border-top: 3px solid red; border-left: 3px solid blue; height: 4px }}"
        ),
        12.0,
        12.0,
    );
    assert_eq!(p.list().unwrap().items().len(), 2);
}

#[test]
fn fractional_geometry_and_device_scale_antialias_by_exact_area() {
    let p = page(
        "<div></div>",
        &format!(
            "{RESET} div {{ margin-left: 0.5px; width: 2px; height: 1px; background: black }}"
        ),
        4.0,
        2.0,
    );
    let s = render(&p, 1.0);
    assert_eq!(s.pixel(0, 0), Some(rgb(128, 128, 128)));
    assert_eq!(s.pixel(1, 0), Some(rgb(0, 0, 0)));
    assert_eq!(s.pixel(2, 0), Some(rgb(128, 128, 128)));
    let s = render(&p, 2.0);
    assert_eq!((s.width(), s.height()), (8, 4));
    assert_eq!(s.pixel(0, 0), Some(Srgba::WHITE));
    for x in 1..5 {
        assert_eq!(s.pixel(x, 1), Some(rgb(0, 0, 0)));
    }
    assert_eq!(s.pixel(5, 0), Some(Srgba::WHITE));
    // A non-integer viewport rounds the surface up.
    let s = render(&p, 1.25);
    assert_eq!((s.width(), s.height()), (5, 3));
}

#[test]
fn translucent_items_composite_source_over_in_paint_order() {
    let p = page(
        "<div id=a></div><div id=b></div>",
        &format!(
            "{RESET} div {{ height: 2px }} #a {{ background: rgb(255 0 0 / 50%) }}
             #b {{ margin-top: -1px; background: rgb(0 0 255 / 50%) }}"
        ),
        2.0,
        4.0,
    );
    let s = render(&p, 1.0);
    assert_eq!(s.pixel(0, 0), Some(rgb(255, 127, 127)));
    assert_eq!(s.pixel(0, 1), Some(rgb(127, 63, 191)));
    assert_eq!(s.pixel(0, 2), Some(rgb(127, 127, 255)));
}

#[test]
fn hidden_boxes_paint_nothing_of_their_own_but_children_may_paint() {
    let p = page(
        "<div id=a><div id=b></div></div>",
        &format!(
            "{RESET} #a {{ visibility: hidden; background: red; border: 1px solid red; height: 4px }}
             #b {{ visibility: visible; height: 2px; background: blue }}"
        ),
        4.0,
        6.0,
    );
    let list = p.list().unwrap();
    assert_eq!(list.items().len(), 1);
    assert_eq!(list.items()[0].node, p.id("b"));
    // Fully transparent colors produce no items.
    let p = page(
        "<div></div>",
        &format!("{RESET} div {{ background: transparent; border: 2px solid transparent }}"),
        4.0,
        4.0,
    );
    assert!(p.list().unwrap().items().is_empty());
}

#[test]
fn collapsed_table_borders_paint_each_line_once_in_the_winning_color() {
    let p = page(
        "<div class=t><div class=r><div class=c></div><div class=c></div></div></div>",
        &format!(
            "{RESET} .t {{ display: table; border-collapse: collapse; border: 2px solid blue }} \
             .r {{ display: table-row }} \
             .c {{ display: table-cell; border: 1px solid red; width: 4px; height: 4px }}"
        ),
        20.0,
        10.0,
    );
    let surface = render(&p, 1.0);
    // The table's wider blue border wins its edges; the shared 1px line
    // between the cells is red, painted once (by the second cell).
    assert_eq!(surface.pixel(0, 4), Some(rgb(0, 0, 255)));
    assert_eq!(surface.pixel(4, 0), Some(rgb(0, 0, 255)));
    assert_eq!(surface.pixel(8, 4), Some(rgb(255, 0, 0)));
    assert_eq!(surface.pixel(7, 4), Some(rgb(255, 255, 255)));
    assert_eq!(surface.pixel(9, 4), Some(rgb(255, 255, 255)));
    assert_eq!(surface.pixel(15, 4), Some(rgb(0, 0, 255)));
}

#[test]
fn hidden_empty_cells_paint_no_background_or_border() {
    let p = page(
        "<div class=t><div class=r><div class=c></div><div class=c><div></div></div></div></div>",
        &format!(
            "{RESET} .t {{ display: table; empty-cells: hide }} .r {{ display: table-row }} \
             .c {{ display: table-cell; background: red; width: 6px; height: 6px }}"
        ),
        20.0,
        10.0,
    );
    let surface = render(&p, 1.0);
    assert_eq!(surface.pixel(2, 2), Some(rgb(255, 255, 255)));
    assert_eq!(surface.pixel(8, 1), Some(rgb(255, 0, 0)));
}

#[test]
fn inset_and_outset_borders_shade_opposite_sides() {
    for (style, top_left, bottom_right) in [
        ("inset", rgb(0, 0, 170), rgb(0, 0, 255)),
        ("outset", rgb(0, 0, 255), rgb(0, 0, 170)),
    ] {
        let p = page(
            "<div></div>",
            &format!("{RESET} div {{ width: 10px; height: 10px; border: 4px {style} blue }}"),
            20.0,
            20.0,
        );
        let surface = render(&p, 1.0);
        // Middle of each side, away from the mitred corners.
        assert_eq!(surface.pixel(9, 1), Some(top_left), "{style} top");
        assert_eq!(surface.pixel(1, 9), Some(top_left), "{style} left");
        assert_eq!(surface.pixel(9, 16), Some(bottom_right), "{style} bottom");
        assert_eq!(surface.pixel(16, 9), Some(bottom_right), "{style} right");
    }
}

#[test]
fn unsupported_paint_features_fail_atomically_with_their_node() {
    // Dashed, dotted and double borders paint as solid (TD-219).
    for css in ["border: 2px dashed red", "border-left: 1px double red"] {
        let p = page(
            "<div></div>",
            &format!("{RESET} div {{ height: 2px; {css} }}"),
            4.0,
            4.0,
        );
        assert!(p.list().is_ok(), "{css}");
    }
    let p = page(
        "<div id=x></div>",
        &format!("{RESET} div {{ height: 2px; background-image: url(x.png) }}"),
        4.0,
        4.0,
    );
    assert_eq!(
        p.list(),
        Err(PaintError::Unsupported {
            node: p.id("x"),
            reason: UnsupportedPaint::BackgroundImage
        })
    );
    // The propagated canvas background image is unsupported too.
    let p = page(
        "",
        &format!("{RESET} body {{ background-image: url(x.png) }}"),
        4.0,
        4.0,
    );
    assert!(matches!(
        p.list(),
        Err(PaintError::Unsupported {
            reason: UnsupportedPaint::BackgroundImage,
            ..
        })
    ));
}

#[test]
fn item_raster_and_cancellation_budgets_are_exact() {
    let p = page(
        "<div></div><div></div>",
        &format!("{RESET} div {{ height: 1px; background: red }}"),
        3.0,
        3.0,
    );
    let list = p.list().unwrap();
    assert_eq!(list.items().len(), 2);
    assert_eq!(
        p.list_with(
            PaintLimits {
                max_items: 1,
                ..PaintLimits::default()
            },
            &CancellationToken::default()
        ),
        Err(PaintError::ItemLimit)
    );
    assert!(
        p.list_with(
            PaintLimits {
                max_items: 2,
                ..PaintLimits::default()
            },
            &CancellationToken::default()
        )
        .is_ok()
    );
    let cancel = CancellationToken::default();
    cancel.cancel();
    assert_eq!(
        p.list_with(PaintLimits::default(), &cancel),
        Err(PaintError::Cancelled)
    );
    let scale = DeviceScale::new(1.0).unwrap();
    let raster = |max_work, cancel: &CancellationToken| {
        rasterize(
            &list,
            scale,
            RasterLimits {
                max_work,
                ..RasterLimits::default()
            },
            cancel,
        )
    };
    // 9 canvas pixels plus two items of (3 + 2) x 1 pixel visits each.
    assert!(raster(19, &CancellationToken::default()).is_ok());
    assert_eq!(
        raster(18, &CancellationToken::default()),
        Err(RasterError::WorkLimit)
    );
    assert_eq!(raster(19, &cancel), Err(RasterError::Cancelled));
    let tiny = RasterLimits {
        max_width: 2,
        ..RasterLimits::default()
    };
    assert_eq!(
        rasterize(&list, scale, tiny, &CancellationToken::default()),
        Err(RasterError::SurfaceSize)
    );
}

#[test]
fn rendering_is_deterministic_and_matches_a_golden_checksum() {
    let p = page(
        "<div id=a><div id=b></div></div><div id=c></div>",
        &format!(
            "{RESET} body {{ background: oklch(0.9 0.05 200) }}
             #a {{ margin: 3.3px 7px; padding: 2.5px; border: 1.5px solid; border-color: red green blue rgb(0 0 0 / 40%);
                   background: lab(60 40 -30) }}
             #b {{ height: 9.7px; background: color(display-p3 0.2 0.6 0.3 / 0.8) }}
             #c {{ height: 5px; margin: 0 20%; background: rgb(250 200 0) }}"
        ),
        53.0,
        31.0,
    );
    let first = encode_png(&render(&p, 1.5), usize::MAX).unwrap();
    let second = encode_png(&render(&p, 1.5), usize::MAX).unwrap();
    assert_eq!(first, second);
    if let Ok(path) = std::env::var("RUSTIC_PAINT_GOLDEN_OUT") {
        std::fs::write(path, &first).unwrap();
    }
    // FNV-1a over the PNG bytes: changes only with an intended paint change,
    // and must be identical on every architecture and operating system.
    let hash = first.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    assert_eq!(
        hash, 0x54d0_e014_7383_60dc,
        "update after reviewing the image: {hash:#x}"
    );
}

fn order(p: &Page, list: &DisplayList, names: &[&str]) -> Vec<String> {
    let ids: Vec<_> = names.iter().map(|n| (p.id(n), *n)).collect();
    list.items()
        .iter()
        .filter_map(|i| {
            ids.iter()
                .find(|(id, _)| *id == i.node)
                .map(|(_, n)| n.to_string())
        })
        .collect()
}

#[test]
fn stacking_contexts_paint_in_appendix_e_order() {
    let p = page(
        "<div id=neg></div><div id=pos2></div><div id=pos1></div><div id=auto></div>\
         <div id=float></div><div id=block></div><div id=zero></div>",
        &format!(
            "{RESET} div {{ height: 2px; background: red }}
             #neg {{ position: relative; z-index: -1 }}
             #pos2 {{ position: relative; z-index: 2 }}
             #pos1 {{ position: relative; z-index: 1 }}
             #auto {{ position: relative }}
             #float {{ float: left; width: 2px }}
             #zero {{ position: relative; z-index: 0 }}"
        ),
        20.0,
        20.0,
    );
    let names = ["neg", "pos2", "pos1", "auto", "float", "block", "zero"];
    assert_eq!(
        order(&p, &p.list().unwrap(), &names),
        ["neg", "block", "float", "auto", "zero", "pos1", "pos2"]
    );
}

#[test]
fn pseudo_contexts_hand_positioned_descendants_to_the_real_context() {
    // A z-index:auto box and a float paint atomically, but a positioned
    // descendant with negative z-index belongs to the root context and paints
    // first; one with z-index 0 paints after the float.
    let p = page(
        "<div id=auto><div id=under></div><div id=in></div></div>\
         <div id=float><div id=fin></div><div id=fpos></div></div>",
        &format!(
            "{RESET} div {{ height: 2px; background: red }}
             #auto {{ position: relative; height: 6px }}
             #under {{ position: relative; z-index: -3 }}
             #float {{ float: left; width: 4px; height: 6px }}
             #fpos {{ position: relative }}"
        ),
        20.0,
        20.0,
    );
    let names = ["auto", "under", "in", "float", "fin", "fpos"];
    assert_eq!(
        order(&p, &p.list().unwrap(), &names),
        ["under", "float", "fin", "auto", "in", "fpos"]
    );
    // A real context keeps its negative children above its own background.
    let p = page(
        "<div id=ctx><div id=low></div></div><div id=after></div>",
        &format!(
            "{RESET} div {{ height: 2px; background: red }}
             #ctx {{ position: relative; z-index: 1 }}
             #low {{ position: relative; z-index: -5 }}"
        ),
        20.0,
        20.0,
    );
    assert_eq!(
        order(&p, &p.list().unwrap(), &["ctx", "low", "after"]),
        ["after", "ctx", "low"]
    );
}

#[test]
fn equal_z_index_keeps_tree_order_and_fixed_boxes_form_contexts() {
    let p = page(
        "<div id=a><div id=a1></div></div><div id=b></div><div id=f></div>",
        &format!(
            "{RESET} div {{ height: 2px; background: red }}
             #a, #b {{ position: relative; z-index: 1 }}
             #a1 {{ position: relative; z-index: 100 }}
             #f {{ position: fixed; top: 0; left: 0; width: 2px }}"
        ),
        20.0,
        20.0,
    );
    // #a1's z-index is local to #a, so #b still paints above both.
    assert_eq!(
        order(&p, &p.list().unwrap(), &["a", "a1", "b", "f"]),
        ["f", "a", "a1", "b"]
    );
}

#[test]
fn overflow_clips_follow_the_containing_block_chain() {
    let p = page(
        "<div id=clip><div id=inner></div><div id=abs></div></div>",
        &format!(
            "{RESET} #clip {{ overflow: hidden; height: 10.5px; width: 8px }}
             #inner {{ height: 30px; background: blue }}
             #abs {{ position: absolute; top: 0; left: 10px; width: 4px; height: 30px; background: lime }}"
        ),
        20.0,
        20.0,
    );
    let list = p.list().unwrap();
    let inner = list
        .items()
        .into_iter()
        .find(|i| i.node == p.id("inner"))
        .unwrap();
    assert!(!inner.clip.is_none());
    let s = render(&p, 1.0);
    assert_eq!(s.pixel(2, 9), Some(rgb(0, 0, 255)));
    // The clip edge at 10.5px covers half of row 10.
    assert_eq!(s.pixel(2, 10), Some(rgb(128, 128, 255)));
    assert_eq!(s.pixel(2, 11), Some(Srgba::WHITE));
    // The absolute box's containing block is the initial one: no clip.
    assert_eq!(s.pixel(11, 15), Some(rgb(0, 255, 0)));
    let dump = list.dump(usize::MAX).unwrap();
    assert!(dump.contains(" clip=(0..8,0..10.5)"), "{dump}");
}

#[test]
fn opacity_groups_composite_their_whole_context_once() {
    let p = page(
        "<div id=a><div id=b></div></div><div id=gone><div id=child></div></div>",
        &format!(
            "{RESET} #a {{ opacity: 0.5; height: 4px; background: red }}
             #b {{ height: 4px; background: blue }}
             #gone {{ opacity: 0; height: 4px; background: red }}
             #child {{ position: relative; z-index: 5; height: 4px; background: red }}"
        ),
        4.0,
        8.0,
    );
    let list = p.list().unwrap();
    // Blue covers red inside the group, then the group is 50% over white;
    // painting each fill at 50% would give purple instead.
    let s = render(&p, 1.0);
    assert_eq!(s.pixel(1, 1), Some(rgb(128, 128, 255)));
    // opacity:0 removes the context and its positioned descendants.
    assert_eq!(s.pixel(1, 5), Some(Srgba::WHITE));
    assert_eq!(
        list.commands().first(),
        Some(&DisplayCommand::BeginGroup {
            source_box: list.items()[0].source_box,
            node: p.id("a"),
            opacity: 0.5,
            blend: rustic_css::BlendMode::Normal,
            filter: None,
        })
    );
    assert_eq!(list.commands().last(), Some(&DisplayCommand::EndGroup));
    assert_eq!(list.items().len(), 2);
    let dump = list.dump(usize::MAX).unwrap();
    assert!(
        dump.contains("opacity=0.5\n") && dump.ends_with("end-group\n"),
        "{dump}"
    );
    // Each open group costs one surface of memory.
    let surface_bytes = 4 * 8 * 4;
    let limited = |max_bytes| {
        rasterize(
            &list,
            DeviceScale::new(1.0).unwrap(),
            RasterLimits {
                max_bytes,
                ..RasterLimits::default()
            },
            &CancellationToken::default(),
        )
    };
    assert!(limited(2 * surface_bytes).is_ok());
    assert_eq!(
        limited(2 * surface_bytes - 1),
        Err(RasterError::MemoryLimit)
    );
}

#[test]
fn layered_scene_matches_a_golden_checksum() {
    let p = page(
        "<div id=box><div id=kid></div></div><div id=over></div><div id=fl></div>",
        &format!(
            "{RESET} body {{ background: #eef }}
             #box {{ position: relative; z-index: 2; opacity: 0.75; overflow: hidden;
                     margin: 3.5px; height: 20px; width: 30px; background: rgb(200 50 50);
                     border: 1.25px solid black }}
             #kid {{ margin: -4px 10px; height: 40px; background: oklab(0.7 -0.1 0.1) }}
             #over {{ position: absolute; top: 8px; left: 20.5px; width: 25px; height: 9px;
                      background: rgb(0 0 255 / 60%); z-index: 1 }}
             #fl {{ float: right; width: 12px; height: 12px; background: teal }}"
        ),
        53.0,
        31.0,
    );
    let png = encode_png(&render(&p, 1.5), usize::MAX).unwrap();
    if let Ok(path) = std::env::var("RUSTIC_PAINT_LAYERED_OUT") {
        std::fs::write(path, &png).unwrap();
    }
    let hash = png.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    assert_eq!(
        hash, 0x0f86_e470_6462_805b,
        "update after reviewing the image: {hash:#x}"
    );
}

#[test]
fn png_output_round_trips_through_the_bounded_decoder() {
    let p = page(
        "<div id=a></div>",
        &format!(
            "{RESET} body {{ background: rgb(10 20 30) }}
             #a {{ margin: 1.5px; height: 3.25px; background: rgb(250 0 100 / 40%); border: 0.5px solid lime }}"
        ),
        9.0,
        7.0,
    );
    let surface = render(&p, 1.0);
    let png = encode_png(&surface, usize::MAX).unwrap();
    let image = rustic_codecs::decode_png(
        &png,
        rustic_codecs::PngLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    assert_eq!((image.width, image.height), (9, 7));
    for y in 0..7 {
        for x in 0..9 {
            let c = surface.pixel(x, y).unwrap();
            let i = ((y * 9 + x) * 4) as usize;
            assert_eq!(&image.pixels[i..i + 4], &[c.r, c.g, c.b, c.a]);
        }
    }
}

fn hit(p: &Page, list: &DisplayList, x: f64, y: f64) -> Option<String> {
    let point = rustic_layout::CssPoint {
        x: rustic_layout::CssPx::new(x).unwrap(),
        y: rustic_layout::CssPx::new(y).unwrap(),
    };
    let found = list.hit_test(point)?;
    assert_eq!(p.tree.boxes()[found.source_box].node(), found.node);
    let doc = &p.doc;
    let name = match doc.node(found.node).unwrap().kind() {
        NodeKind::Element(e) => e
            .attributes()
            .iter()
            .find(|a| a.name().local() == "id")
            .map(|a| String::from_utf16_lossy(a.value().as_units()))
            .unwrap_or_else(|| e.name().local().to_string()),
        _ => unreachable!("only elements generate boxes"),
    };
    Some(name)
}

#[test]
fn hit_testing_follows_paint_order_clips_and_visibility() {
    let p = page(
        "<div id=under></div><div id=over></div>\
         <div id=clip><div id=wide></div></div>\
         <div id=hidden><div id=shown></div></div>\
         <div id=ghost></div><div id=neg></div>",
        &format!(
            "{RESET} div {{ position: absolute; width: 10px; height: 10px }}
             #under {{ left: 0; top: 0; z-index: 2 }}
             #over {{ left: 5px; top: 0; z-index: 1 }}
             #clip {{ left: 0; top: 20px; overflow: hidden }}
             #wide {{ left: 0; top: 0; width: 50px }}
             #hidden {{ left: 30px; top: 0; visibility: hidden }}
             #shown {{ left: 0; top: 0; width: 5px; visibility: visible }}
             #ghost {{ left: 0; top: 40px; opacity: 0 }}
             #neg {{ left: 20px; top: 40px; z-index: -1 }}"
        ),
        60.0,
        60.0,
    );
    let list = p.list().unwrap();
    let at = |x, y| hit(&p, &list, x, y);
    // z-index 2 is above z-index 1 where they overlap, despite tree order.
    assert_eq!(at(7.0, 5.0).as_deref(), Some("under"));
    assert_eq!(at(12.0, 5.0).as_deref(), Some("over"));
    // Half-open edges: x = 15 is just outside #over.
    assert_eq!(at(15.0, 5.0).as_deref(), Some("html"));
    // Overflow clips cut hits as they cut paint.
    assert_eq!(at(5.0, 25.0).as_deref(), Some("wide"));
    assert_eq!(at(20.0, 25.0).as_deref(), Some("html"));
    // A hidden box is not a target; its visible child is.
    assert_eq!(at(32.0, 5.0).as_deref(), Some("shown"));
    assert_eq!(at(37.0, 5.0).as_deref(), Some("html"));
    // opacity: 0 paints nothing but still takes hits.
    assert!(list.items().iter().all(|i| i.node != p.id("ghost")));
    assert_eq!(at(5.0, 45.0).as_deref(), Some("ghost"));
    // Negative z-index sits above the root's own box.
    assert_eq!(at(25.0, 45.0).as_deref(), Some("neg"));
    // Outside the viewport nothing is hit.
    assert_eq!(at(60.0, 5.0), None);
    assert_eq!(at(-1.0, 5.0), None);
}

#[test]
fn inline_content_paints_after_floats_and_before_positioned_boxes() {
    let fonts = rustic_text::bundled::test_database();
    let (doc, styles) = common::page(
        "<div id=block><div id=float></div><span id=span>x</span><div id=pos></div></div>",
        &format!(
            "{RESET} #block {{ font: 10px/10px 'Rustic Test'; color: blue; background: gray;
               overflow: hidden; height: 6px }}
             #float {{ float: left; width: 4px; height: 4px; background: red }}
             #span {{ background: lime }}
             #pos {{ position: absolute; top: 0; width: 2px; height: 2px; background: black }}"
        ),
    );
    let tree = BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    let viewport = CssSize::new(20.0, 20.0).unwrap();
    let layout = BlockLayout::compute_with_fonts(
        &tree,
        viewport,
        LayoutLimits::default(),
        &fonts,
        &CancellationToken::default(),
    )
    .unwrap();
    let build = |fonts| {
        DisplayList::build_with_fonts(
            &doc,
            &tree,
            &layout,
            viewport,
            fonts,
            PaintLimits::default(),
            &CancellationToken::default(),
        )
    };
    // Text cannot be painted without the fonts it was laid out with.
    assert_eq!(build(None), Err(PaintError::Font));
    let list = build(Some(&fonts)).unwrap();
    let page = Page {
        doc,
        tree,
        layout,
        viewport,
    };
    let roles: Vec<_> = list
        .items()
        .iter()
        .map(|item| (page.id_name(item.node), item.role))
        .collect();
    assert_eq!(
        roles,
        [
            ("block".to_string(), ItemRole::Background),
            ("float".to_string(), ItemRole::Background),
            ("span".to_string(), ItemRole::Background),
            ("#text".to_string(), ItemRole::Text),
            ("pos".to_string(), ItemRole::Background),
        ]
    );
    // The text item is one path, clipped by its block's own overflow.
    let text = list.items()[3];
    let Shape::Path(index) = text.shape else {
        panic!("text is a path: {text:?}");
    };
    assert_eq!(list.paths()[index].contours().len(), 1);
    assert_eq!(text.clip.y.map(|r| r.end.get()), Some(6.0));
    assert!(
        list.dump(usize::MAX)
            .unwrap()
            .contains(" text path=0 contours=1 points=4 ")
    );
    // The span takes hits; the glyph's pixels are blue and clipped at y = 6.
    let surface = rasterize(
        &list,
        DeviceScale::new(1.0).unwrap(),
        RasterLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    assert_eq!(surface.pixel(5, 4), Some(rgb(0, 0, 255)));
    assert_eq!(surface.pixel(5, 7), Some(rgb(255, 255, 255)));
}

#[test]
fn images_copy_exactly_at_one_to_one_and_smooth_when_scaled() {
    let p = page(
        "<div id=clip><img id=img></div>",
        &format!(
            "{RESET} #clip {{ width: 3px; overflow: hidden }}
             img {{ display: block }}"
        ),
        10.0,
        10.0,
    );
    // A 4×2 image: red green blue yellow / half-transparent black row.
    let red = [255, 0, 0, 255];
    let green = [0, 255, 0, 255];
    let blue = [0, 0, 255, 255];
    let yellow = [255, 255, 0, 255];
    let shade = [0, 0, 0, 128];
    let pixels: Vec<u8> = [red, green, blue, yellow, shade, shade, shade, shade].concat();
    let image = std::sync::Arc::new(rustic_paint::Image::new(4, 2, pixels).unwrap());
    assert!(rustic_paint::Image::new(4, 2, vec![0; 31]).is_none());
    assert!(rustic_paint::Image::new(0, 2, Vec::new()).is_none());
    // Natural size 4×2 from the embedder, as the CLI's image loader does.
    let mut tree = p.tree.clone();
    let node = p.id("img");
    tree.set_natural_sizes(&[(
        node,
        rustic_layout::NaturalSize::from_dimensions(
            rustic_layout::CssPx::new(4.0).unwrap(),
            rustic_layout::CssPx::new(2.0).unwrap(),
        ),
    )])
    .unwrap();
    let layout = BlockLayout::compute(
        &tree,
        p.viewport,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    let mut resources = rustic_paint::Resources::default();
    resources.images.insert(node, image);
    let list = DisplayList::build_with(
        &p.doc,
        &tree,
        &layout,
        p.viewport,
        &resources,
        PaintLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    assert!(
        list.dump(usize::MAX)
            .unwrap()
            .contains(" image image=0 rect=(0,0,4,2) size=4x2 ")
    );
    let at = |scale| {
        rasterize(
            &list,
            DeviceScale::new(scale).unwrap(),
            RasterLimits::default(),
            &CancellationToken::default(),
        )
        .unwrap()
    };
    // One image pixel per device pixel: exact copies, alpha composited over
    // white, and the fourth column clipped by the 3px-wide overflow box.
    let one = at(1.0);
    assert_eq!(one.pixel(0, 0), Some(rgb(255, 0, 0)));
    assert_eq!(one.pixel(1, 0), Some(rgb(0, 255, 0)));
    assert_eq!(one.pixel(2, 0), Some(rgb(0, 0, 255)));
    assert_eq!(one.pixel(3, 0), Some(rgb(255, 255, 255)));
    assert_eq!(one.pixel(0, 1), Some(rgb(127, 127, 127)));
    // At scale 2, image pixel centers keep their colors and the boundary
    // between red and green is a bilinear blend.
    let two = at(2.0);
    assert_eq!(two.pixel(0, 0), Some(rgb(255, 0, 0)));
    let blend = two.pixel(2, 0).unwrap();
    assert!(blend.r > 0 && blend.g > 0 && blend.b == 0, "{blend:?}");
}

#[test]
fn decorations_propagate_to_in_flow_text_only_and_fail_on_unsupported_styles() {
    let fonts = rustic_text::bundled::test_database();
    let build = |html: &str, css: &str| {
        let (doc, styles) = common::page(
            html,
            &format!("{RESET} * {{ font: 10px/10px 'Rustic Test' }} {css}"),
        );
        let tree = BoxTree::build(
            &doc,
            doc.root(),
            &styles,
            LayoutLimits::default(),
            &CancellationToken::default(),
        )
        .unwrap();
        let viewport = CssSize::new(100.0, 50.0).unwrap();
        let layout = BlockLayout::compute_with_fonts(
            &tree,
            viewport,
            LayoutLimits::default(),
            &fonts,
            &CancellationToken::default(),
        )
        .unwrap();
        let list = DisplayList::build_with_fonts(
            &doc,
            &tree,
            &layout,
            viewport,
            Some(&fonts),
            PaintLimits::default(),
            &CancellationToken::default(),
        );
        let page = Page {
            doc,
            tree,
            layout,
            viewport,
        };
        (page, list)
    };
    // The block decorates its own text and in-flow inline text, but not the
    // float's text; both nested decorating boxes draw, outermost first.
    let (page, list) = build(
        "<div id=outer>a<span id=inner>b</span><i id=float>c</i></div>",
        "#outer { text-decoration: underline red }
         #inner { text-decoration: line-through blue }
         #float { float: left }",
    );
    let roles: Vec<_> = list
        .unwrap()
        .items()
        .iter()
        .filter(|i| i.role == ItemRole::Decoration)
        .map(|i| (page.id_name(i.node), i.color))
        .collect();
    assert_eq!(
        roles,
        [
            ("outer".to_string(), rgb(255, 0, 0)),
            ("outer".to_string(), rgb(255, 0, 0)),
            ("inner".to_string(), rgb(0, 0, 255)),
        ]
    );
    // Dotted, dashed and wavy fail atomically with the decorating node.
    let (page, list) = build("<p id=p>x</p>", "#p { text-decoration: underline dashed }");
    assert_eq!(
        list,
        Err(PaintError::Unsupported {
            node: page.id("p"),
            reason: UnsupportedPaint::DecorationStyle,
        })
    );
}

#[test]
fn outlines_paint_last_in_their_stacking_context_and_collapse_inward() {
    let p = page(
        "<div id=ctx><div id=a></div><div id=b></div></div><div id=z></div>\
         <div id=shrunk></div><div id=gone></div><div id=hidden></div><div id=none></div>",
        &format!(
            "{RESET} div {{ height: 10px; width: 10px }}
             #ctx {{ position: relative; z-index: 1; opacity: 0.5; width: auto; height: auto }}
             #a {{ outline: 2px solid red }}
             #b {{ background: blue }}
             #z {{ position: relative; z-index: 2; background: lime }}
             #shrunk {{ outline: 4px auto red; outline-offset: -8px }}
             #gone {{ outline: 4px solid red; outline-offset: -20px }}
             #hidden {{ outline: 4px solid red; visibility: hidden }}
             #none {{ outline-width: 4px; outline-color: red }}"
        ),
        40.0,
        100.0,
    );
    let list = p.list().unwrap();
    // The outline of `a` paints after its later sibling's background but
    // inside its context's opacity group; the root context's outlines
    // paint after even its positive z-index children.
    let mut steps = Vec::new();
    for command in list.commands() {
        steps.push(match command {
            DisplayCommand::Fill(item) => format!("{}:{:?}", p.id_name(item.node), item.role),
            DisplayCommand::BeginGroup { .. } => "group".into(),
            DisplayCommand::EndGroup => "end".into(),
            DisplayCommand::BeginClip { .. } => "clip".into(),
            DisplayCommand::EndClip => "end-clip".into(),
            DisplayCommand::BeginTransform { .. } => "transform".into(),
            DisplayCommand::EndTransform => "end-transform".into(),
        });
    }
    assert_eq!(
        steps,
        [
            "group",
            "b:Background",
            "a:Outline",
            "end",
            "z:Background",
            "shrunk:Outline"
        ]
    );
    let outline = |name: &str| {
        list.items()
            .into_iter()
            .filter(|i| i.node == p.id(name) && i.role == ItemRole::Outline)
            .map(|i| i.shape)
            .collect::<Vec<_>>()
    };
    // a: border box (0,0) 10x10 grown by 2 on every side, hole at the box.
    let Shape::Ring { outer, inner } = outline("a")[0] else {
        panic!("ring expected");
    };
    assert_eq!(
        (outer.origin().x.get(), outer.size().width().get()),
        (-2.0, 14.0)
    );
    assert_eq!(
        (inner.origin().x.get(), inner.size().width().get()),
        (0.0, 10.0)
    );
    // A negative offset that collapses the inner edge fills what is left.
    let Shape::Rect(rect) = outline("shrunk")[0] else {
        panic!("rect expected");
    };
    assert_eq!(
        (rect.size().width().get(), rect.size().height().get()),
        (2.0, 2.0)
    );
    // Unsupported styles fail atomically with their node.
    let p = page(
        "<div id=d></div>",
        "#d { outline: 1px dotted red }",
        20.0,
        20.0,
    );
    assert_eq!(
        p.list(),
        Err(PaintError::Unsupported {
            node: p.id("d"),
            reason: UnsupportedPaint::OutlineStyle,
        })
    );
}

#[test]
fn rounded_clips_follow_containing_blocks_and_shape_hits() {
    let p = page(
        "<div id=clip><div id=child></div><div id=abs></div></div><div id=round></div>",
        &format!(
            "{RESET} #clip {{ width: 40px; height: 40px; border-radius: 20px; overflow: hidden }}
             #child {{ height: 40px; background: red }}
             #abs {{ position: absolute; top: 0; left: 0; width: 5px; height: 5px }}
             #round {{ width: 40px; height: 40px; border-radius: 10px; background: blue }}"
        ),
        100.0,
        100.0,
    );
    let list = p.list().unwrap();
    let item = |name: &str| {
        list.items()
            .into_iter()
            .find(|i| i.node == p.id(name))
            .unwrap()
    };
    // The child is clipped to the circle; the rounded box paints a curve.
    let child = item("child");
    let node = list.clips()[child.rounded_clip.unwrap()];
    assert_eq!(node.shape.corners[0].x, 20.0);
    assert!(matches!(item("round").shape, Shape::Rounded(_)));
    // Corners of a rounded box do not take hits; the clip's circle limits
    // its in-flow child, but the absolutely positioned box escapes it.
    assert_eq!(hit(&p, &list, 1.0, 1.0).as_deref(), Some("abs"));
    assert_eq!(hit(&p, &list, 6.0, 1.0).as_deref(), Some("body"));
    assert_eq!(hit(&p, &list, 20.0, 20.0).as_deref(), Some("child"));
    assert_eq!(hit(&p, &list, 10.0, 50.0).as_deref(), Some("round"));
    let dump = list.dump(1 << 20).unwrap();
    assert!(dump.contains("rclip 0 rounded=(0,0,40,40) radii=(20/20 20/20 20/20 20/20)"));
    assert!(dump.contains(" rclip=0\n"), "{dump}");
}

#[test]
fn clip_paths_form_contexts_clip_descendants_and_hits() {
    let p = page(
        "<div id=clip><div id=inner></div><div id=abs></div></div><div id=after></div>",
        &format!(
            "{RESET} #clip {{ width: 40px; height: 40px; clip-path: circle(20px at 20px 20px) }}
             #inner {{ height: 40px; background: red }}
             #abs {{ position: absolute; left: 30px; top: 30px; width: 40px; height: 40px;
                     background: blue }}
             #after {{ height: 10px; margin-top: -45px; position: relative; z-index: -1;
                       background: lime }}"
        ),
        100.0,
        100.0,
    );
    let list = p.list().unwrap();
    // The clip-path box is a stacking context (z-index 0 in tree order), so
    // the negative z-index sibling paints first; its absolutely positioned
    // descendant is inside the clip layer even though it overflows the box.
    let mut steps = Vec::new();
    for command in list.commands() {
        steps.push(match command {
            DisplayCommand::Fill(item) => p.id_name(item.node),
            DisplayCommand::BeginClip { shape, .. } => {
                assert!(matches!(shape, Shape::Rounded(_)));
                "clip".into()
            }
            DisplayCommand::EndClip => "end-clip".into(),
            DisplayCommand::BeginTransform { .. } => "transform".into(),
            DisplayCommand::EndTransform => "end-transform".into(),
            DisplayCommand::BeginGroup { .. } => "group".into(),
            DisplayCommand::EndGroup => "end".into(),
        });
    }
    assert_eq!(steps, ["after", "clip", "inner", "abs", "end-clip"]);
    // Hits follow the circle for the box and every descendant.
    assert_eq!(hit(&p, &list, 20.0, 20.0).as_deref(), Some("inner"));
    assert_eq!(hit(&p, &list, 32.0, 32.0).as_deref(), Some("abs"));
    // Outside the circle, neither the box nor its descendants take hits.
    assert_eq!(hit(&p, &list, 36.0, 36.0).as_deref(), Some("html"));
    assert_eq!(hit(&p, &list, 60.0, 60.0).as_deref(), Some("html"));
    let dump = list.dump(1 << 20).unwrap();
    assert!(dump.contains("clip-path box="), "{dump}");
    assert!(dump.contains("end-clip-path\n"));
    // References to SVG and the evenodd rule fail atomically.
    for (css, reason) in [
        ("url(#a)", UnsupportedPaint::ClipPathUrl),
        (
            "polygon(evenodd, 0 0, 1px 0, 0 1px)",
            UnsupportedPaint::ClipPathFillRule,
        ),
    ] {
        let p = page(
            "<div id=d></div>",
            &format!("#d {{ clip-path: {css} }}"),
            20.0,
            20.0,
        );
        assert_eq!(
            p.list(),
            Err(PaintError::Unsupported {
                node: p.id("d"),
                reason,
            })
        );
    }
}

#[test]
fn transforms_map_paint_clips_and_hits_through_their_planes() {
    // A 40px square at (30, 30) turned 45° about its center (50, 50): a
    // diamond reaching 28.28px from the center along the axes.
    let p = page(
        "<div id=t></div>",
        &format!(
            "{RESET} body {{ padding: 30px }}
             #t {{ width: 40px; height: 40px; background: lime; transform: rotate(45deg) }}"
        ),
        100.0,
        100.0,
    );
    let list = p.list().unwrap();
    let steps: Vec<String> = list
        .commands()
        .iter()
        .map(|c| match c {
            DisplayCommand::Fill(item) => p.id_name(item.node),
            DisplayCommand::BeginTransform { .. } => "transform".into(),
            DisplayCommand::EndTransform => "end-transform".into(),
            _ => "other".into(),
        })
        .collect();
    assert_eq!(steps, ["transform", "t", "end-transform"]);
    let surface = render(&p, 1.0);
    let white = rgb(255, 255, 255);
    assert_eq!(surface.pixel(50, 50), Some(rgb(0, 255, 0)));
    // The square's corner is outside the diamond; the diamond's tip is
    // outside the square.
    assert_eq!(surface.pixel(31, 31), Some(white));
    assert_eq!(surface.pixel(49, 24), Some(rgb(0, 255, 0)));
    assert_eq!(hit(&p, &list, 31.0, 31.0).as_deref(), Some("body"));
    assert_eq!(hit(&p, &list, 50.0, 23.0).as_deref(), Some("t"));
    // The diamond is symmetric about its center to within rounding.
    for (x, y) in [(40, 35), (45, 28), (33, 47)] {
        let a = surface.pixel(x, y).unwrap();
        let b = surface.pixel(100 - x - 1, 100 - y - 1).unwrap();
        assert!(a.r.abs_diff(b.r) <= 1, "({x},{y}): {a:?} vs {b:?}");
    }
    let dump = list.dump(1 << 20).unwrap();
    assert!(dump.contains("transform box="), "{dump}");
    assert!(dump.contains("end-transform\n"));

    // An overflow clip outside the transform clips it in the outer plane,
    // and hits outside that clip miss.
    let p = page(
        "<div id=clip><div id=moved></div></div>",
        &format!(
            "{RESET} #clip {{ width: 50px; height: 50px; overflow: hidden }}
             #moved {{ height: 20px; background: blue; transform: translateX(30px) }}"
        ),
        100.0,
        100.0,
    );
    let list = p.list().unwrap();
    let surface = render(&p, 2.0);
    assert_eq!(surface.pixel(2 * 45, 10), Some(rgb(0, 0, 255)));
    assert_eq!(surface.pixel(2 * 55, 10), Some(rgb(255, 255, 255)));
    assert_eq!(hit(&p, &list, 45.0, 5.0).as_deref(), Some("moved"));
    assert_eq!(hit(&p, &list, 10.0, 5.0).as_deref(), Some("clip"));
    assert_eq!(hit(&p, &list, 55.0, 5.0).as_deref(), Some("body"));

    // A containing block's perspective: translateZ(50px) at a distance of
    // 100px doubles the size about the perspective origin (the center).
    let p = page(
        "<div id=stage><div id=near></div></div>",
        &format!(
            "{RESET} #stage {{ position: relative; width: 100px; height: 100px;
                       perspective: 100px }}
             #near {{ position: absolute; left: 40px; top: 40px; width: 20px; height: 20px;
                      background: red; transform: translateZ(50px) }}"
        ),
        100.0,
        100.0,
    );
    let list = p.list().unwrap();
    let surface = render(&p, 1.0);
    assert_eq!(surface.pixel(31, 31), Some(rgb(255, 0, 0)));
    assert_eq!(surface.pixel(68, 68), Some(rgb(255, 0, 0)));
    assert_eq!(surface.pixel(29, 50), Some(rgb(255, 255, 255)));
    assert_eq!(hit(&p, &list, 32.0, 50.0).as_deref(), Some("near"));
    assert_eq!(hit(&p, &list, 28.0, 50.0).as_deref(), Some("stage"));

    // A singular transform, or a hidden back face, paints and hits nothing.
    for css in [
        "transform: scale(0)",
        "transform: rotateX(180deg); backface-visibility: hidden",
    ] {
        let p = page(
            "<div id=gone></div>",
            &format!("{RESET} #gone {{ height: 20px; background: red; {css} }}"),
            40.0,
            40.0,
        );
        let list = p.list().unwrap();
        assert!(list.items().is_empty(), "{css}");
        assert_eq!(hit(&p, &list, 5.0, 5.0).as_deref(), Some("body"), "{css}");
    }

    // A transform that tips the plane behind the viewer paints only the
    // part in front of it, deterministically.
    let p = page(
        "<div id=stage><div id=tilt></div></div>",
        &format!(
            "{RESET} #stage {{ perspective: 30px }}
             #tilt {{ height: 60px; background: blue; transform: rotateX(80deg) }}"
        ),
        60.0,
        60.0,
    );
    assert_eq!(render(&p, 1.0), render(&p, 1.0));
}

#[test]
fn box_shadows_paint_around_backgrounds_blur_and_take_no_hits() {
    let p = page(
        "<div id=s></div>",
        &format!(
            "{RESET} body {{ padding: 30px }}
             #s {{ width: 40px; height: 40px; background: white; border: 1px solid black;
                   box-shadow: inset 0 0 0 2px lime, 0 0 10px blue, 2px 2px red }}"
        ),
        100.0,
        100.0,
    );
    let list = p.list().unwrap();
    // Outer shadows (last first) under the background, inset ones over it
    // and under the border.
    let roles: Vec<ItemRole> = list.items().iter().map(|i| i.role).collect();
    assert_eq!(
        roles,
        [
            ItemRole::Shadow,
            ItemRole::Shadow,
            ItemRole::Background,
            ItemRole::Shadow,
            ItemRole::Border
        ]
    );
    let shadows = list.shadows();
    assert_eq!(shadows.len(), 3);
    assert!(!shadows[0].inset && shadows[0].sigma == 0.0);
    assert!(!shadows[1].inset && shadows[1].sigma == 5.0);
    assert!(shadows[2].inset);
    let dump = list.dump(1 << 20).unwrap();
    assert!(dump.contains(" shadow shadow=("), "{dump}");
    assert!(dump.contains("sigma=5 outside="), "{dump}");
    // Shadows take no hits.
    assert_eq!(hit(&p, &list, 75.0, 50.0).as_deref(), Some("body"));
    let surface = render(&p, 1.0);
    // The blur fades away from the box, the same on opposite sides.
    let blue = |x: u32, y: u32| 255 - surface.pixel(x, y).unwrap().r;
    assert!(blue(29, 50) > blue(25, 50) && blue(25, 50) > blue(20, 50));
    assert_eq!(blue(29, 50), blue(50, 29));
    assert!(blue(15, 50) < 3);
    // The hard red shadow peeks out 2px right of the box, under the blue
    // blur listed before it; inside, the inset lime ring sits inside the
    // border.
    let under = surface.pixel(72, 50).unwrap();
    assert!(under.r > 100 && under.g == 0 && under.b > 50, "{under:?}");
    assert_eq!(surface.pixel(32, 50), Some(rgb(0, 255, 0)));
    assert_eq!(surface.pixel(50, 50), Some(rgb(255, 255, 255)));
    // Deterministic at another scale, too.
    let p2 = render(&p, 1.5);
    assert_eq!(p2, render(&p, 1.5));
}

#[test]
fn blend_modes_and_filters_composite_groups_in_order() {
    // Each blend mode over rgb(51 102 204): the blended color of a solid
    // source over a solid backdrop is B(cb, cs) exactly.
    for (mode, expected) in [
        ("normal", rgb(153, 102, 101)),
        ("screen", rgb(173, 163, 224)),
        ("difference", rgb(102, 0, 103)),
        ("darken", rgb(51, 102, 101)),
        ("exclusion", rgb(143, 122, 143)),
        ("luminosity", rgb(70, 121, 223)),
        ("plus-lighter", rgb(204, 204, 255)),
    ] {
        let p = page(
            "<div id=b><div id=s></div></div>",
            &format!(
                "{RESET} #b {{ height: 10px; background: rgb(51 102 204) }}
                 #s {{ height: 10px; background: rgb(153 102 101); mix-blend-mode: {mode} }}"
            ),
            20.0,
            20.0,
        );
        assert_eq!(render(&p, 1.0).pixel(5, 5), Some(expected), "{mode}");
    }
    // A blur filter spreads the box symmetrically within its group, and a
    // filtered context's commands nest: blend group, clip, filter group.
    let p = page(
        "<div id=f></div>",
        &format!(
            "{RESET} body {{ padding: 20px }}
             #f {{ width: 20px; height: 20px; background: black; filter: blur(3px);
                   mix-blend-mode: multiply; clip-path: inset(-10px) }}"
        ),
        60.0,
        60.0,
    );
    let list = p.list().unwrap();
    let steps: Vec<&str> = list
        .commands()
        .iter()
        .map(|c| match c {
            DisplayCommand::BeginGroup { filter: None, .. } => "group",
            DisplayCommand::BeginGroup {
                filter: Some(_), ..
            } => "filter",
            DisplayCommand::EndGroup => "end",
            DisplayCommand::BeginClip { .. } => "clip",
            DisplayCommand::EndClip => "end-clip",
            DisplayCommand::Fill(_) => "fill",
            _ => "other",
        })
        .collect();
    // The root isolates its blending child.
    assert_eq!(
        steps,
        [
            "group", "group", "clip", "filter", "fill", "end", "end-clip", "end", "end"
        ]
    );
    let surface = render(&p, 1.0);
    let ink = |x: u32, y: u32| 255 - surface.pixel(x, y).unwrap().r;
    assert!(ink(30, 30) > 200);
    assert!(ink(18, 30) > 0 && ink(18, 30) < 128);
    assert_eq!(ink(18, 30), ink(41, 30));
    assert_eq!(ink(30, 18), ink(30, 41));
    // The clip path cuts the blur 10px outside the box.
    assert_eq!(ink(9, 30), 0);
    assert!(list.dump(1 << 20).unwrap().contains("filter=(blur:3)"));
    let p = page(
        "<div id=u></div>",
        &format!("{RESET} #u {{ filter: url(#f) }}"),
        20.0,
        20.0,
    );
    assert_eq!(
        p.list(),
        Err(PaintError::Unsupported {
            node: p.id("u"),
            reason: UnsupportedPaint::FilterUrl,
        })
    );
}

#[test]
fn regions_rasterize_to_exactly_the_pixels_of_the_full_surface() {
    let p = page(
        "<div id=a><div id=b></div></div><div id=c></div><div id=d></div><div id=e></div>",
        &format!(
            "{RESET} body {{ padding: 7px }}
             #a {{ width: 50px; height: 30px; border: 3px solid red; border-radius: 9px 4px;
                   overflow: hidden; background: rgb(0 128 255 / 60%);
                   box-shadow: 3px 4px 6px 1px green, inset 0 0 5px black }}
             #b {{ width: 80px; height: 15px; background: lime; transform: rotate(17deg) }}
             #c {{ width: 40px; height: 20px; margin: 12px; background: orange;
                   filter: blur(2.5px) drop-shadow(4px -3px 1.5px blue) hue-rotate(40deg);
                   clip-path: circle(30px) }}
             #d {{ width: 30px; height: 25px; margin-top: -30px; background: purple;
                   mix-blend-mode: screen; opacity: 0.7; transform: perspective(60px) rotateY(25deg) }}
             #e {{ width: 20px; height: 20px; margin-left: 160px; background: teal;
                   box-shadow: 0 0 20px 4px red }}"
        ),
        200.0,
        131.0,
    );
    let list = p.list().unwrap();
    let cancel = CancellationToken::default();
    for scale in [1.0, 1.5] {
        let scale = DeviceScale::new(scale).unwrap();
        let full = rasterize(&list, scale, RasterLimits::default(), &cancel).unwrap();
        let (w, h) = (full.width(), full.height());
        // At 1.5 the surface spans two tile columns and #e's shadow crosses
        // the seam; the regions straddle tiles at odd sizes.
        for (tw, th) in [(61, 37), (128, 200)] {
            let mut y = 0;
            while y < h {
                let mut x = 0;
                while x < w {
                    let region = DeviceRect {
                        x,
                        y,
                        width: tw.min(w - x),
                        height: th.min(h - y),
                    };
                    let tile =
                        rasterize_region(&list, scale, region, RasterLimits::default(), &cancel)
                            .unwrap();
                    for ty in 0..region.height {
                        for tx in 0..region.width {
                            assert_eq!(
                                tile.pixel(tx, ty),
                                full.pixel(x + tx, y + ty),
                                "tile {region:?} pixel ({}, {})",
                                x + tx,
                                y + ty
                            );
                        }
                    }
                    x += tw;
                }
                y += th;
            }
        }
    }
    // A region outside the surface is refused.
    let outside = DeviceRect {
        x: 195,
        y: 0,
        width: 8,
        height: 1,
    };
    assert_eq!(
        rasterize_region(
            &list,
            DeviceScale::new(1.0).unwrap(),
            outside,
            RasterLimits::default(),
            &cancel
        ),
        Err(RasterError::SurfaceSize)
    );
}

#[test]
fn damaged_tiles_repaint_to_exactly_the_new_surface() {
    const HTML: &str = "<div id=a></div><div id=b></div><div id=c></div>\
        <div id=d></div><div id=e></div><div id=f><div id=g></div></div>";
    const BASE: &str = "
        /* #e's shadow and #g's blur reach across tile seams their boxes do not. */
        #a, #b, #c, #d, #e, #f, #g { position: absolute; width: 40px; height: 30px }
        #a { left: 10px; top: 10px; background: red }
        #b { left: 300px; top: 20px; background: blue; border-radius: 8px }
        #c { left: 520px; top: 40px; background: green; opacity: 0.6 }
        #d { left: 30px; top: 270px; background: purple; transform: rotate(10deg) }
        #e { left: 208px; top: 200px; background: teal; box-shadow: 4px 4px 6px black }
        #f { left: 462px; top: 280px; filter: blur(3px) }
        #g { left: 5px; top: 5px; background: orange }";
    let variants = [
        "#a { background: lime }",
        "#b { left: 330px }",
        "#c { opacity: 0.3 }",
        "#d { transform: rotate(25deg) }",
        "#e { box-shadow: -6px 2px 9px 3px red }",
        "#g { background: navy }",
        "#e { display: none }",
        "#f { filter: blur(5px) }",
        "body { background: yellow }",
    ];
    let cancel = CancellationToken::default();
    let list = |extra: &str| {
        page(HTML, &format!("{RESET}{BASE}{extra}"), 600.0, 340.0)
            .list()
            .unwrap()
    };
    let old = list("");
    let tiles =
        |s: &Surface| (s.width().div_ceil(TILE_SIZE) * s.height().div_ceil(TILE_SIZE)) as usize;
    // Every change at scale 1; at 1.5 the tile grid falls elsewhere.
    for (scale, variants) in [(1.0, &variants[..]), (1.5, &variants[3..6])] {
        let scale = DeviceScale::new(scale).unwrap();
        let before = rasterize(&old, scale, RasterLimits::default(), &cancel).unwrap();
        assert_eq!(damage(&old, &old, scale), Ok(Vec::new()));
        for &extra in variants {
            let new = list(extra);
            let regions = damage(&old, &new, scale).unwrap();
            let mut surface = before.clone();
            repaint(
                &mut surface,
                &new,
                scale,
                &regions,
                RasterLimits::default(),
                &cancel,
            )
            .unwrap();
            let want = rasterize(&new, scale, RasterLimits::default(), &cancel).unwrap();
            assert!(
                surface.premultiplied() == want.premultiplied(),
                "{extra}: repainting {regions:?} differs from a full raster"
            );
            // A change in one place repaints only the tiles near it; a new
            // filter margin or canvas color repaints everything.
            let all = extra.starts_with("#f") || extra.starts_with("body");
            assert_eq!(regions.len() == tiles(&before), all, "{extra}: {regions:?}");
            assert!(!regions.is_empty(), "{extra}");
        }
    }
}

#[test]
fn resizing_and_zooming_relayout_paint_and_hit_test_through_the_view() {
    use rustic_layout::View;
    let css = format!("{RESET} #a {{ width: 50%; height: 20px; background: red }}");
    // Lay out and paint `view`; return the device pixel and the box at a
    // device point of the window.
    let probe = |view: View, x: f64, y: f64| {
        let viewport = view.viewport();
        let p = page(
            "<div id=a></div>",
            &css,
            viewport.width().get(),
            viewport.height().get(),
        );
        let list = p.list().unwrap();
        let surface = rasterize(
            &list,
            view.scale(),
            RasterLimits::default(),
            &CancellationToken::default(),
        )
        .unwrap();
        let point = view.to_css(x, y).unwrap();
        let found = hit(&p, &list, point.x.get(), point.y.get());
        let pixel = surface.pixel(x as u32, y as u32).unwrap();
        ((surface.width(), surface.height()), pixel, found)
    };
    let red = rgb(255, 0, 0);
    let white = rgb(255, 255, 255);
    let view = View::new(400.0, 100.0, 1.0, 1.0).unwrap();
    assert_eq!(
        probe(view, 150.0, 10.0),
        ((400, 100), red, Some("a".into()))
    );
    assert_eq!(
        probe(view, 250.0, 10.0),
        ((400, 100), white, Some("body".into()))
    );
    // Narrowing the window reflows the percentage width.
    let narrow = view.resized(200.0, 100.0).unwrap();
    assert_eq!(
        probe(narrow, 150.0, 10.0),
        ((200, 100), white, Some("body".into()))
    );
    // At 200% the viewport is half the window and everything paints twice
    // as large: the box is 100 CSS pixels, 200 device pixels, 40 tall.
    let zoomed = view.zoomed(2.0).unwrap();
    assert_eq!(zoomed.viewport().width().get(), 200.0);
    assert_eq!(
        probe(zoomed, 150.0, 30.0),
        ((400, 100), red, Some("a".into()))
    );
    assert_eq!(
        probe(zoomed, 150.0, 41.0),
        ((400, 100), white, Some("html".into()))
    );
    // A device pixel ratio composes with zoom, and an odd zoom's surface is
    // still the window times the ratio.
    let dense = View::new(400.0, 100.0, 1.5, 1.1).unwrap();
    assert_eq!(probe(dense, 10.0, 10.0).0, (600, 150));
    // Out-of-range views are refused.
    assert!(View::new(400.0, 100.0, 1.0, 6.0).is_err());
    assert!(View::new(400.0, 100.0, 1.0, 0.2).is_err());
    assert!(View::new(0.0, 100.0, 1.0, 1.0).is_err());
    assert!(View::new(400.0, 100.0, 40.0, 2.0).is_err());
}

#[test]
fn tiles_skip_what_cannot_reach_them_and_match_the_reference_exactly() {
    // Groups, clips, transforms, shadows, text and filters spread over
    // several tiles, some reaching across seams. Without the filter each
    // tile's frame is the tile itself, so a fill whose bounds were
    // underestimated near a seam would go missing.
    for filter in ["blur(4px) drop-shadow(8px 8px 2px red)", "none"] {
        let list = text_list(
            "<div id=a><div id=b></div></div><div id=c></div><div id=d></div>\
             <div id=e></div><div id=f><div id=g></div></div><p id=t>Tiles</p>",
            &format!(
                "{RESET} div, p {{ position: absolute; width: 60px; height: 40px; margin: 0 }}
                 #a {{ left: 230px; top: 10px; opacity: 0.5; background: red;
                       border-radius: 9px; overflow: hidden }}
                 #b {{ left: 20px; top: 5px; background: blue; transform: rotate(30deg) }}
                 #c {{ left: 500px; top: 230px; background: green; mix-blend-mode: multiply;
                       clip-path: circle(25px) }}
                 #d {{ left: 10px; top: 300px; background: purple;
                       transform: perspective(90px) rotateY(35deg) }}
                 #e {{ left: 440px; top: 40px; background: teal;
                       box-shadow: 0 0 30px 10px black }}
                 #f {{ left: 240px; top: 240px; filter: {filter} }}
                 #g {{ left: 5px; top: 5px; background: orange }}
                 #t {{ left: 250px; top: 250px; width: 80px; font: 20px 'Rustic Test';
                       transform: rotate(-20deg) }}"
            ),
            700.0,
            400.0,
        );
        let cancel = CancellationToken::default();
        for scale in [1.0, 1.5] {
            let scale = DeviceScale::new(scale).unwrap();
            let culled = rasterize(&list, scale, RasterLimits::default(), &cancel).unwrap();
            let reference =
                rasterize_reference(&list, scale, RasterLimits::default(), &cancel).unwrap();
            assert!(
                culled == reference,
                "culling changed pixels at {scale:?} with filter {filter}"
            );
        }
    }
    let cancel = CancellationToken::default();
    // Culling is what keeps a mostly empty page cheap: one small group in
    // one of 15 tiles costs its layer buffers in that tile only.
    let sparse = page(
        "<div id=a></div>",
        &format!("{RESET} #a {{ width: 20px; height: 20px; opacity: 0.5; background: red }}"),
        1280.0,
        720.0,
    )
    .list()
    .unwrap();
    let budget = RasterLimits {
        max_work: 1280 * 720 * 2,
        ..RasterLimits::default()
    };
    let scale = DeviceScale::new(1.0).unwrap();
    assert!(rasterize(&sparse, scale, budget, &cancel).is_ok());
    assert_eq!(
        rasterize_reference(&sparse, scale, budget, &cancel),
        Err(RasterError::WorkLimit)
    );
}

/// The display list of a page laid out and painted with the bundled fonts.
fn text_list(html: &str, css: &str, width: f64, height: f64) -> DisplayList {
    let fonts = rustic_text::bundled::test_database();
    let (doc, styles) = common::page(html, css);
    let tree = BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    let viewport = CssSize::new(width, height).unwrap();
    let layout = BlockLayout::compute_with_fonts(
        &tree,
        viewport,
        LayoutLimits::default(),
        &fonts,
        &CancellationToken::default(),
    )
    .unwrap();
    DisplayList::build_with_fonts(
        &doc,
        &tree,
        &layout,
        viewport,
        Some(&fonts),
        PaintLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap()
}

#[test]
fn atomic_inlines_paint_after_their_line_above_later_blocks_and_floats() {
    let list = text_list(
        "<div id=wrap><div id=float></div>ab<span id=ib><span id=inner></span></span>cd</div>\
         <div id=after></div>",
        &format!(
            "{RESET} #wrap {{ font: 10px/10px 'Rustic Test' }}
             #float {{ float: left; width: 5px; height: 5px; background: rgb(1 0 0) }}
             #ib {{ display: inline-block; width: 8px; height: 8px; background: rgb(2 0 0) }}
             #inner {{ display: block; height: 2px; background: rgb(3 0 0) }}
             #after {{ height: 10px; margin-top: -10px; background: rgb(4 0 0) }}"
        ),
        100.0,
        40.0,
    );
    let items = list.items();
    let at = |r: u8| {
        items
            .iter()
            .position(|i| i.role == ItemRole::Background && i.color == rgb(r, 0, 0))
            .unwrap_or_else(|| panic!("no background rgb({r} 0 0)"))
    };
    let texts: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| i.role == ItemRole::Text)
        .map(|(k, _)| k)
        .collect();
    assert!(!texts.is_empty());
    // CSS 2.2 Appendix E: in-flow blocks (step 4), floats (step 5), then the
    // inline content of the line (step 7), where the inline-block paints
    // atomically, its own block content included.
    assert!(at(4) < at(1), "the later block paints under the float");
    assert!(at(1) < at(2), "the float paints under the inline-block");
    assert!(
        texts.iter().all(|&t| t < at(2)),
        "the line's text paints first"
    );
    assert_eq!(
        at(3),
        at(2) + 1,
        "the inline-block's content paints with it"
    );
}

#[test]
fn submitted_frames_repaint_only_damage_and_equal_full_rasters() {
    use rustic_raster::Frames;
    let scale = DeviceScale::new(1.0).unwrap();
    let cancel = CancellationToken::default();
    let frame = |left: u32, color: &str| {
        page(
            "<div id=a></div><div id=b></div>",
            &format!(
                "body {{ margin:0 }} div {{ height:20px }} \
                 #a {{ margin-left:{left}px; width:20px; background:{color} }} \
                 #b {{ width:20px; background:#00f }}"
            ),
            1000.0,
            600.0,
        )
        .list()
        .unwrap()
    };
    let mut frames = Frames::new(scale, RasterLimits::default());
    let lists = [
        frame(0, "#f00"),
        frame(0, "#f00"),
        frame(0, "#0f0"),
        frame(600, "#0f0"),
    ];
    let mut stats = Vec::new();
    for list in &lists {
        stats.push(frames.submit(list.clone(), &cancel).unwrap());
        let full = rasterize(list, scale, RasterLimits::default(), &cancel).unwrap();
        assert_eq!(
            frames.surface().unwrap(),
            &full,
            "every frame equals a full raster"
        );
    }
    // 1000x600 is 4x3 tiles. The first frame paints all, an identical one
    // nothing, a recolor one tile, a move the tiles it leaves and enters.
    let tiles: Vec<_> = stats.iter().map(|s| (s.repainted_tiles, s.full)).collect();
    assert_eq!(tiles, [(12, true), (0, false), (1, false), (2, false)]);
    assert!(stats.iter().all(|s| s.total_tiles == 12));
    assert_eq!(stats[1].repainted_pixels, 0);
    // A new size repaints everything.
    let wide = page("<div></div>", "div{height:5px}", 500.0, 600.0)
        .list()
        .unwrap();
    let stat = frames.submit(wide, &cancel).unwrap();
    assert!(stat.full && stat.total_tiles == 6);
}

#[test]
fn scenes_reuse_content_tiles_across_scrolls_and_equal_fresh_scenes() {
    use rustic_raster::{LayerKind, Scene, SceneFrame, SceneLayer};
    let scale = DeviceScale::new(1.0).unwrap();
    let cancel = CancellationToken::default();
    let limits = RasterLimits::default();
    // A 600x1200 page of four 300px bands; the window is 300x200.
    let content = |third: &str| {
        page(
            "<div></div><div></div><div id=c></div><div></div>",
            &format!(
                "body {{ margin:0 }} div {{ height:300px }} \
                 div:nth-child(1) {{ background:#f00 }} div:nth-child(2) {{ background:#0f0 }} \
                 #c {{ background:{third} }} div:nth-child(4) {{ background:#ff0 }}"
            ),
            600.0,
            1200.0,
        )
        .list()
        .unwrap()
    };
    let overlay = page(
        "<div></div>",
        "body { margin:0 } div { width:10px; height:10px; background:rgba(0,0,0,0.5) }",
        300.0,
        200.0,
    )
    .list()
    .unwrap()
    .over_transparent();
    let blue = content("#00f");
    let gray = content("#888");
    let full_blue = rasterize(&blue, scale, limits, &cancel).unwrap();
    let crop = |surface: &Surface, x: u32, y: u32| rasterize_region_like(surface, x, y, 300, 200);
    let mut scene = Scene::new(scale, limits);
    let run = |scene: &mut Scene, list: &DisplayList, y: u32, with_overlay: bool| {
        scene
            .submit(
                SceneFrame {
                    layers: vec![SceneLayer {
                        list,
                        kind: LayerKind::Page,
                    }],
                    overlay: with_overlay.then_some(&overlay),
                    offset: (0, y),
                    size: (300, 200),
                },
                &cancel,
            )
            .unwrap()
    };
    // Without an overlay a frame is exactly the visible part of the page.
    let (frame, stats) = run(&mut scene, &blue, 0, false);
    assert_eq!(frame.premultiplied(), crop(&full_blue, 0, 0));
    assert_eq!((stats.painted_tiles, stats.reused_tiles), (2, 0));
    // Scrolling within the cached tiles paints nothing.
    let (frame, stats) = run(&mut scene, &blue, 50, false);
    assert_eq!(frame.premultiplied(), crop(&full_blue, 0, 50));
    assert_eq!((stats.painted_tiles, stats.reused_tiles), (0, 2));
    // Scrolling further paints only newly exposed tiles: two tile rows
    // (256..768) of the two columns the 300px window spans.
    let (frame, stats) = run(&mut scene, &blue, 400, false);
    assert_eq!(frame.premultiplied(), crop(&full_blue, 0, 400));
    assert_eq!((stats.painted_tiles, stats.reused_tiles), (4, 0));
    // A content change invalidates only the damaged tiles; the result
    // equals a fresh scene of the new content.
    let (changed, _) = run(&mut scene, &gray, 400, false);
    let mut fresh = Scene::new(scale, limits);
    let (expected, _) = run(&mut fresh, &gray, 400, false);
    assert_eq!(changed, expected);
    // The overlay composites over the content: black at alpha 128 keeps
    // 127/255 of the red band.
    let (frame, stats) = run(&mut scene, &gray, 0, true);
    assert_eq!(stats.overlay_pixels, 300 * 200);
    assert_eq!(&frame.premultiplied()[..4], &[127, 0, 0, 255]);
    assert_eq!(&frame.premultiplied()[40..44], &[255, 0, 0, 255]);
    let mut fresh = Scene::new(scale, limits);
    assert_eq!(run(&mut fresh, &gray, 0, true).0, frame);
    // A window past the content is refused, and the cache starts over.
    let past = scene.submit(
        SceneFrame {
            layers: vec![SceneLayer {
                list: &gray,
                kind: LayerKind::Page,
            }],
            overlay: None,
            offset: (0, 1100),
            size: (300, 200),
        },
        &cancel,
    );
    assert_eq!(past.unwrap_err(), RasterError::SurfaceSize);
}

/// The `w x h` pixels of `surface` at `x, y`, premultiplied.
fn rasterize_region_like(surface: &Surface, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for row in y..y + h {
        let start = ((row * surface.width() + x) * 4) as usize;
        out.extend_from_slice(&surface.premultiplied()[start..start + (w * 4) as usize]);
    }
    out
}

#[test]
fn scroller_layers_show_their_contents_through_the_port_at_their_offset() {
    use rustic_raster::{LayerKind, Scene, SceneFrame, SceneLayer};
    let scale = DeviceScale::new(1.0).unwrap();
    let cancel = CancellationToken::default();
    let limits = RasterLimits::default();
    // The page: white, 300x300. The scroller's contents: three 100px
    // bands at y 0..300 of x 0..100, over a transparent canvas.
    let page_list = page("<p></p>", "body { margin:0 }", 300.0, 300.0)
        .list()
        .unwrap();
    let contents = page(
        "<div></div><div></div><div></div>",
        "body { margin:0 } div { width:100px; height:100px } \
         div:nth-child(1) { background:#f00 } div:nth-child(2) { background:#0f0 } \
         div:nth-child(3) { background:#00f }",
        300.0,
        300.0,
    )
    .list()
    .unwrap()
    .over_transparent();
    let port = DeviceRect {
        x: 0,
        y: 0,
        width: 100,
        height: 100,
    };
    let mut scene = Scene::new(scale, limits);
    let at = |scene: &mut Scene, scroll: u32| {
        scene
            .submit(
                SceneFrame {
                    layers: vec![
                        SceneLayer {
                            list: &page_list,
                            kind: LayerKind::Page,
                        },
                        SceneLayer {
                            list: &contents,
                            kind: LayerKind::Scroller {
                                port,
                                scroll: (0, scroll),
                            },
                        },
                    ],
                    overlay: None,
                    offset: (0, 0),
                    size: (200, 200),
                },
                &cancel,
            )
            .unwrap()
    };
    let pixel = |s: &Surface, x: u32, y: u32| {
        let i = ((y * s.width() + x) * 4) as usize;
        s.premultiplied()[i..i + 4].to_vec()
    };
    let (frame, first) = at(&mut scene, 0);
    assert_eq!(pixel(&frame, 5, 5), [255, 0, 0, 255]);
    assert_eq!(
        pixel(&frame, 5, 150),
        [255, 255, 255, 255],
        "outside the port"
    );
    // Scrolled by 150: green at the top half of the port, blue below.
    let (frame, scrolled) = at(&mut scene, 150);
    assert_eq!(pixel(&frame, 5, 5), [0, 255, 0, 255]);
    assert_eq!(pixel(&frame, 5, 60), [0, 0, 255, 255]);
    assert_eq!(pixel(&frame, 150, 5), [255, 255, 255, 255]);
    // Scrolling the container painted nothing new: all tiles were cached.
    assert_eq!(first.painted_tiles, 2);
    assert_eq!((scrolled.painted_tiles, scrolled.reused_tiles), (0, 2));
    let mut fresh = Scene::new(scale, limits);
    assert_eq!(at(&mut fresh, 150).0, frame);
}

#[test]
fn sideways_glyphs_turn_with_vertical_lines() {
    let fonts = rustic_text::bundled::test_database();
    let (doc, styles) = common::page(
        "<div id=v>xx</div>",
        &format!(
            "{RESET} html {{ writing-mode: vertical-rl }}
             #v {{ font: 10px/10px 'Rustic Test'; color: blue; text-decoration: underline }}"
        ),
    );
    let tree = BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    let viewport = CssSize::new(20.0, 30.0).unwrap();
    let layout = BlockLayout::compute_with_fonts(
        &tree,
        viewport,
        LayoutLimits::default(),
        &fonts,
        &CancellationToken::default(),
    )
    .unwrap();
    let list = DisplayList::build_with_fonts(
        &doc,
        &tree,
        &layout,
        viewport,
        Some(&fonts),
        PaintLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    let surface = rasterize(
        &list,
        DeviceScale::new(1.0).unwrap(),
        RasterLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    let column: Vec<String> = (0..20)
        .map(|x| {
            (0..30)
                .map(|y| match surface.pixel(x, y) {
                    Some(c) if c == rgb(255, 255, 255) => '.',
                    Some(c) if c == rgb(0, 0, 255) => 'b',
                    _ => '?',
                })
                .collect()
        })
        .collect();
    // The line is the rightmost 10px column: the two glyphs run down it
    // with their tops to the right, and the underline is on its left
    // (under) side.
    let empty = ".".repeat(30);
    assert!(column[..10].iter().all(|c| *c == empty), "{column:#?}");
    assert_eq!(column[10], format!("{}{}", "?".repeat(10), ".".repeat(20)));
    for (x, c) in column.iter().enumerate().take(17).skip(12) {
        assert_eq!(*c, format!("?bbb??bbb?{}", ".".repeat(20)), "{x}");
    }
    assert!(column[17..].iter().all(|c| *c == empty), "{column:#?}");
}

#[test]
fn upright_glyphs_stand_on_vertical_lines() {
    let fonts = rustic_text::bundled::test_database();
    let (doc, styles) = common::page(
        "<div id=v>xx</div>",
        &format!(
            "{RESET} html {{ writing-mode: vertical-rl; text-orientation: upright }}
             #v {{ font: 10px/10px 'Rustic Test'; color: blue; text-decoration: underline }}"
        ),
    );
    let tree = BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    let viewport = CssSize::new(20.0, 30.0).unwrap();
    let layout = BlockLayout::compute_with_fonts(
        &tree,
        viewport,
        LayoutLimits::default(),
        &fonts,
        &CancellationToken::default(),
    )
    .unwrap();
    let list = DisplayList::build_with_fonts(
        &doc,
        &tree,
        &layout,
        viewport,
        Some(&fonts),
        PaintLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    let surface = rasterize(
        &list,
        DeviceScale::new(1.0).unwrap(),
        RasterLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    let column: Vec<String> = (0..20)
        .map(|x| {
            (0..30)
                .map(|y| match surface.pixel(x, y) {
                    Some(c) if c == rgb(255, 255, 255) => '.',
                    Some(c) if c == rgb(0, 0, 255) => 'b',
                    _ => '?',
                })
                .collect()
        })
        .collect();
    // The glyphs are not turned: each one is a horizontal glyph centered
    // on the line (x 10..20) and stacked one em apart down it, and the
    // underline stays on the line's left (under) side.
    let empty = ".".repeat(30);
    assert!(column[..10].iter().all(|c| *c == empty), "{column:#?}");
    assert_eq!(column[10], format!("{}{}", "?".repeat(20), ".".repeat(10)));
    for (x, c) in column.iter().enumerate().take(17).skip(13) {
        assert_eq!(*c, format!("..?bbbb?....?bbbb?{}", ".".repeat(12)), "{x}");
    }
    assert!(column[17..].iter().all(|c| *c == empty), "{column:#?}");
}

#[test]
fn inline_svg_paints_shapes_strokes_and_transforms_in_its_viewport() {
    let p = page(
        "<svg width=40 height=40 viewBox='0 0 20 20' style='display:block;color:#00f'>\
         <rect width=10 height=10 fill=red />\
         <g transform='translate(10 0)'><rect width=10 height=10 fill=currentColor /></g>\
         <path d='M0 10h10v10H0z' style='fill: lime' />\
         <line x1=10 y1=15 x2=20 y2=15 stroke=black stroke-width=2 />\
         <circle cx=100 cy=100 r=50 fill=red /></svg>",
        RESET,
        60.0,
        60.0,
    );
    let surface = render(&p, 1.0);
    // The viewBox doubles everything.
    assert_eq!(surface.pixel(10, 10), Some(rgb(255, 0, 0)));
    assert_eq!(surface.pixel(30, 10), Some(rgb(0, 0, 255)), "currentColor");
    assert_eq!(
        surface.pixel(10, 30),
        Some(rgb(0, 255, 0)),
        "style attribute"
    );
    assert_eq!(
        surface.pixel(30, 30),
        Some(rgb(0, 0, 0)),
        "stroke, 4px wide"
    );
    assert_eq!(surface.pixel(30, 26), Some(rgb(255, 255, 255)));
    // Content outside the viewport is clipped.
    assert_eq!(surface.pixel(50, 50), Some(rgb(255, 255, 255)));
}

#[test]
fn form_controls_paint_their_state_and_fieldsets_frame_their_legend() {
    let p = page(
        "<progress value=1 max=4></progress><meter value=1></meter>",
        &format!(
            "{RESET} progress, meter {{ display: block; width: 40px; height: 10px; border-radius: 0 }}"
        ),
        60.0,
        30.0,
    );
    let surface = render(&p, 1.0);
    // A quarter of the progress bar is filled, the rest is the track.
    assert_eq!(surface.pixel(5, 5), Some(rgb(0x00, 0x75, 0xff)));
    assert_eq!(surface.pixel(15, 5), Some(rgb(0xef, 0xef, 0xef)));
    // A full meter in its optimum region.
    assert_eq!(surface.pixel(35, 15), Some(rgb(0x10, 0x7c, 0x10)));
}

#[test]
fn mathml_fraction_rules_and_radical_overlines_paint() {
    // Block-level and empty, so no text needs fonts: the rule is the
    // numerator's bottom border (the radical's overline is tested with
    // fonts in rustic-layout's inline tests).
    let p = page(
        "<math style='display:block;font-size:50px'><mfrac style='display:flex'>\
         <mrow style='display:block;width:40px;height:20px'></mrow>\
         <mrow style='display:block;height:20px'></mrow></mfrac></math>",
        RESET,
        200.0,
        100.0,
    );
    let list = p.list().unwrap();
    let black = rgb(0, 0, 0);
    let borders: Vec<_> = list
        .commands()
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::Fill(item) if item.role == ItemRole::Border && item.color == black => {
                Some(p.id_name(item.node))
            }
            _ => None,
        })
        .collect();
    assert_eq!(borders, ["mrow"], "{list:?}");
    let surface = render(&p, 1.0);
    // The 3px rule (0.06em) under the 20px numerator, which the flex
    // column centers in the 200px line.
    assert_eq!(surface.pixel(100, 21), Some(black));
    assert_eq!(surface.pixel(100, 10), Some(rgb(255, 255, 255)));
    assert_eq!(surface.pixel(20, 21), Some(rgb(255, 255, 255)));
}

#[test]
fn svg_gradients_clips_fill_rules_dashes_and_group_opacity_paint() {
    let p = page(
        "<svg width=200 height=100 style='display:block'><defs>\
         <linearGradient id=g><stop offset=0 stop-color=#000 /><stop offset=1 stop-color=#fff /></linearGradient>\
         <clipPath id=c><rect x=60 y=0 width=10 height=100 /></clipPath></defs>\
         <rect width=50 height=20 fill='url(#g)' />\
         <g clip-path='url(#c)'><rect x=50 y=0 width=40 height=20 fill=red /></g>\
         <path d='M100 0h40v40h-40z M110 10v20h20v-20z' fill=blue fill-rule=evenodd />\
         <line x1=0 y1=50 x2=40 y2=50 stroke=black stroke-width=4 stroke-dasharray='10 10' />\
         <g opacity=.5><rect x=150 y=0 width=20 height=20 fill=#f00 /><rect x=160 y=0 width=20 height=20 fill=#f00 /></g>\
         </svg>",
        RESET,
        200.0,
        100.0,
    );
    let surface = render(&p, 1.0);
    let white = rgb(255, 255, 255);
    // The gradient darkens toward its start.
    let (left, right) = (
        surface.pixel(5, 10).unwrap(),
        surface.pixel(45, 10).unwrap(),
    );
    assert!(left.r < 40 && right.r > 215, "{left:?} {right:?}");
    // Only the clip's strip of the red rect paints.
    assert_eq!(surface.pixel(65, 10), Some(rgb(255, 0, 0)));
    assert_eq!(surface.pixel(55, 10), Some(white));
    assert_eq!(surface.pixel(80, 10), Some(white));
    // The even-odd inner square is a hole.
    assert_eq!(surface.pixel(105, 5), Some(rgb(0, 0, 255)));
    assert_eq!(surface.pixel(120, 20), Some(white));
    // Dash, gap, dash.
    assert_eq!(surface.pixel(5, 50), Some(rgb(0, 0, 0)));
    assert_eq!(surface.pixel(15, 50), Some(white));
    assert_eq!(surface.pixel(25, 50), Some(rgb(0, 0, 0)));
    // A group composites once: the overlap is no darker.
    assert_eq!(surface.pixel(155, 10), surface.pixel(165, 10));
    assert_eq!(surface.pixel(165, 10), Some(rgb(255, 128, 128)));
}
