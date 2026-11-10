//! Measure paint frame times on this machine: a full rasterization of a
//! 1280×720 page with shadows, groups, a transform, a filter, blending and
//! text, the same without tile culling, and the damage-limited repaint of
//! one changed box. Run in release:
//!
//! ```sh
//! cargo run --release -p rustic-raster --example frame_budget
//! ```
//!
//! It prints the median of five runs of each at scales 1 and 2 and exits
//! with status 1 when a repaint misses the 60 Hz frame budget (16.7 ms).

use rustic_base::CancellationToken;
use rustic_layout::{BlockLayout, BoxTree, CssSize, DeviceScale, LayoutLimits};
use rustic_paint::{DisplayList, PaintLimits};
use rustic_raster::{RasterLimits, damage, rasterize, rasterize_reference, repaint};
use std::process::ExitCode;
use std::time::{Duration, Instant};

#[path = "../tests/common/mod.rs"]
mod common;

const BASE: &str = "html, body { margin: 0 }
    body { background: #eee; font: 16px 'Rustic Test' }
    div { position: absolute; width: 200px; height: 120px }
    #a { left: 40px; top: 40px; background: red; border-radius: 12px;
         box-shadow: 0 4px 12px black }
    #b { left: 400px; top: 60px; background: blue; opacity: 0.7 }
    #c { left: 700px; top: 300px; background: green; transform: rotate(10deg) }
    #d { left: 100px; top: 450px; background: orange; filter: blur(2px) }
    #e { left: 900px; top: 500px; background: teal; mix-blend-mode: multiply }";

fn list(css: &str) -> DisplayList {
    let fonts = rustic_text::bundled::test_database();
    let html = "<div id=a></div><div id=b></div><div id=c></div><div id=d></div>\
                <div id=e></div><p>Tiles, damage and frame budgets</p>";
    let (doc, styles) = common::page(html, css);
    let cancel = CancellationToken::default();
    let tree = BoxTree::build(&doc, doc.root(), &styles, LayoutLimits::default(), &cancel)
        .expect("the page builds");
    let viewport = CssSize::new(1280.0, 720.0).expect("valid viewport");
    let layout =
        BlockLayout::compute_with_fonts(&tree, viewport, LayoutLimits::default(), &fonts, &cancel)
            .expect("the page lays out");
    DisplayList::build_with_fonts(
        &doc,
        &tree,
        &layout,
        viewport,
        Some(&fonts),
        PaintLimits::default(),
        &cancel,
    )
    .expect("the page paints")
}

fn median(mut run: impl FnMut()) -> Duration {
    let mut times: Vec<Duration> = (0..5)
        .map(|_| {
            let start = Instant::now();
            run();
            start.elapsed()
        })
        .collect();
    times.sort();
    times[2]
}

fn main() -> ExitCode {
    let old = list(BASE);
    let new = list(&format!("{BASE} #b {{ background: navy }}"));
    let cancel = CancellationToken::default();
    let limits = RasterLimits::default();
    let budget = Duration::from_micros(16_667);
    let mut within = true;
    for scale in [1.0, 2.0] {
        let scale = DeviceScale::new(scale).expect("valid scale");
        let surface = rasterize(&old, scale, limits, &cancel).expect("rasterizes");
        let full = median(|| {
            rasterize(&old, scale, limits, &cancel).expect("rasterizes");
        });
        let unculled = median(|| {
            rasterize_reference(&old, scale, limits, &cancel).expect("rasterizes");
        });
        let mut tiles = 0;
        let update = median(|| {
            let mut next = surface.clone();
            let regions = damage(&old, &new, scale).expect("diffs");
            tiles = regions.len();
            repaint(&mut next, &new, scale, &regions, limits, &cancel).expect("repaints");
        });
        println!(
            "scale {}: full {full:.1?}, without culling {unculled:.1?}, \
             repaint of {tiles} damaged tiles {update:.1?}",
            scale.get()
        );
        within &= update <= budget;
    }
    if within {
        ExitCode::SUCCESS
    } else {
        println!("a repaint missed the 16.7 ms frame budget");
        ExitCode::from(1)
    }
}
