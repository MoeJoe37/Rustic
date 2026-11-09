//! Display list construction and rasterization against the block layout.
use rustic_base::CancellationToken;
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_layout::{BlockLayout, BoxTree, CssSize, DeviceScale, LayoutLimits};
use rustic_paint::{
    DisplayCommand, DisplayList, ItemRole, PaintError, PaintLimits, Shape, Srgba, UnsupportedPaint,
};
use rustic_raster::{RasterError, RasterLimits, Surface, encode_png, rasterize};
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
fn unsupported_paint_features_fail_atomically_with_their_node() {
    for (css, reason) in [
        ("border: 2px dashed red", UnsupportedPaint::BorderStyle),
        ("border-left: 1px double red", UnsupportedPaint::BorderStyle),
        (
            "background-image: url(x.png)",
            UnsupportedPaint::BackgroundImage,
        ),
    ] {
        let p = page(
            "<div id=x></div>",
            &format!("{RESET} div {{ height: 2px; {css} }}"),
            4.0,
            4.0,
        );
        assert_eq!(
            p.list(),
            Err(PaintError::Unsupported {
                node: p.id("x"),
                reason
            }),
            "{css}"
        );
    }
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
            opacity: 0.5
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
