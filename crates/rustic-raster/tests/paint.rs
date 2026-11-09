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
        p.list_with(PaintLimits { max_items: 1 }, &CancellationToken::default()),
        Err(PaintError::ItemLimit)
    );
    assert!(
        p.list_with(PaintLimits { max_items: 2 }, &CancellationToken::default())
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
