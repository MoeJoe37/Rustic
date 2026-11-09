//! Display list construction and rasterization against the block layout.
use rustic_base::CancellationToken;
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_layout::{BlockLayout, BoxTree, CssSize, DeviceScale, LayoutLimits};
use rustic_paint::{
    DisplayList, ItemRole, PaintError, PaintLimits, Shape, Srgba, UnsupportedPaint,
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
        ("opacity: 0.5", UnsupportedPaint::Opacity),
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
