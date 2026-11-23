//! PERF-01 page pipeline benchmark: time each rendering stage of Rustic's
//! own fixture pages and print `sample <fixture>.<stage> <ms> <check>` for
//! every run, in the format `tools/cross_engine_bench.py` reads.
//!
//! ```text
//! cargo run --release -p rustic-raster --example page_bench -- [--runs K] [--scale N]
//! ```
//!
//! Stages are HTML parse, style (UA sheet, author sheet and cascade), layout
//! (box tree and block layout with the bundled test font), paint (display
//! list) and raster (a 1280×720 viewport), plus their total. Every fixture is
//! generated here, so the pages are original and the same on every device.
//! The check value is the same on every run of a fixture: element count for
//! parse and style, box count for layout, item count for paint and a pixel
//! sum for raster. A stage that fails prints `<fixture> not runnable: ...`.

use std::fmt::Write as _;
use std::time::Instant;

use rustic_base::CancellationToken;
use rustic_css::CssParser;
use rustic_html::HtmlDocumentParser;
use rustic_layout::{BlockLayout, BoxTree, CssSize, DeviceScale, LayoutLimits};
use rustic_paint::{DisplayList, PaintLimits};
use rustic_raster::{RasterLimits, rasterize};
use rustic_style::{
    ComputeContext, MatchContext, Origin, StyleSheet, document_cascade, style_tree,
};

/// `(name, html, css)` built on demand so large fixtures are not static data.
fn fixtures() -> Vec<(&'static str, String, String)> {
    let base = "body { margin: 8px; font: 16px 'Rustic Test' }".to_owned();

    let tiny = "<!doctype html><title>t</title><h1>Hello</h1><p>One short paragraph.</p>";

    let mut article = String::from("<!doctype html><article>");
    for section in 0..40 {
        let _ = write!(article, "<h2>Section {section}</h2>");
        for paragraph in 0..5 {
            let _ = write!(
                article,
                "<p>Paragraph {paragraph} of section {section} has <em>emphasis</em>, \
                 <a href=#s{section}>a link</a> and enough ordinary words to wrap onto \
                 several lines at the viewport width, so line breaking and shaping both \
                 do real work on every run.</p>"
            );
        }
    }
    article.push_str("</article>");
    let article_css = format!(
        "{base} h2 {{ font-size: 24px; margin: 16px 0 8px }} p {{ line-height: 1.4 }} \
         em {{ font-style: italic }} a {{ color: blue }}"
    );

    let mut large = String::from("<!doctype html><div>");
    for item in 0..5_000 {
        let _ = write!(large, "<p class=i{}>item {item}</p>", item % 10);
    }
    large.push_str("</div>");
    let large_css = format!("{base} p {{ margin: 0; padding: 1px }} .i3 {{ color: green }}");

    let mut selectors = String::from("<!doctype html><main>");
    for block in 0..400 {
        let _ = write!(
            selectors,
            "<section class=s{} data-k={}><div><span class=a>a</span><span class=b>b</span></div></section>",
            block % 16,
            block % 7
        );
    }
    selectors.push_str("</main>");
    let mut selectors_css = base.clone();
    for rule in 0..300 {
        let _ = write!(
            selectors_css,
            " main .s{} > div span.{}:nth-child({}) {{ margin-left: {}px }} \
             section[data-k=\"{}\"] div {{ padding-top: {}px }}",
            rule % 16,
            if rule % 2 == 0 { "a" } else { "b" },
            1 + rule % 2,
            rule % 5,
            rule % 7,
            rule % 3
        );
    }

    let mut deep = String::from("<!doctype html>");
    for _ in 0..400 {
        deep.push_str("<div>");
    }
    deep.push_str("leaf");
    for _ in 0..400 {
        deep.push_str("</div>");
    }
    let deep_css = format!("{base} div {{ padding-left: 1px }}");

    vec![
        ("page_tiny", tiny.to_owned(), base.clone()),
        ("page_article", article, article_css),
        ("page_large_dom", large, large_css),
        ("page_selectors", selectors, selectors_css),
        ("page_deep", deep, deep_css),
    ]
}

/// Stage times in milliseconds and their check values, or why a stage failed.
fn run_once(
    html: &str,
    css: &str,
    scale: DeviceScale,
) -> Result<Vec<(&'static str, f64, u64)>, String> {
    let fonts = rustic_text::bundled::test_database();
    let cancel = CancellationToken::default();
    let mut stages = Vec::new();

    let start = Instant::now();
    let mut parser = HtmlDocumentParser::default();
    parser
        .feed(html.as_bytes())
        .map_err(|e| format!("parse: {e:?}"))?;
    let doc = parser
        .finish()
        .map_err(|e| format!("parse: {e:?}"))?
        .into_document();
    let elements = doc.node_count() as u64;
    stages.push(("parse", ms(start), elements));

    let start = Instant::now();
    let css_parser = CssParser::default();
    let (mut cascade, _) =
        document_cascade(&doc, &css_parser).map_err(|e| format!("style: {e:?}"))?;
    let (sheet, _) =
        StyleSheet::parse(&css_parser, css, Origin::Author).map_err(|e| format!("style: {e:?}"))?;
    cascade.push(sheet);
    let styles = style_tree(
        &doc,
        doc.root(),
        &cascade,
        &css_parser,
        &MatchContext::html(),
        &ComputeContext::default(),
    )
    .map_err(|e| format!("style: {e:?}"))?;
    stages.push(("style", ms(start), styles.len() as u64));

    let start = Instant::now();
    let tree = BoxTree::build(&doc, doc.root(), &styles, LayoutLimits::default(), &cancel)
        .map_err(|e| format!("layout: {e:?}"))?;
    let viewport = CssSize::new(1280.0, 720.0).map_err(|e| format!("layout: {e:?}"))?;
    let layout =
        BlockLayout::compute_with_fonts(&tree, viewport, LayoutLimits::default(), &fonts, &cancel)
            .map_err(|e| format!("layout: {e:?}"))?;
    stages.push(("layout", ms(start), tree.boxes().len() as u64));

    let start = Instant::now();
    let list = DisplayList::build_with_fonts(
        &doc,
        &tree,
        &layout,
        viewport,
        Some(&fonts),
        PaintLimits::default(),
        &cancel,
    )
    .map_err(|e| format!("paint: {e:?}"))?;
    stages.push(("paint", ms(start), list.items().len() as u64));

    let start = Instant::now();
    let surface = rasterize(&list, scale, RasterLimits::default(), &cancel)
        .map_err(|e| format!("raster: {e:?}"))?;
    let pixels = surface
        .premultiplied()
        .iter()
        .fold(0_u64, |sum, byte| sum.wrapping_add(u64::from(*byte)));
    stages.push(("raster", ms(start), pixels));

    let total = stages.iter().map(|(_, time, _)| time).sum();
    stages.push(("total", total, elements));
    Ok(stages)
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let mut runs = 11_usize;
    let mut scale = 1.0_f64;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--runs" => runs = args.next().and_then(|v| v.parse().ok()).unwrap_or(runs),
            "--scale" => scale = args.next().and_then(|v| v.parse().ok()).unwrap_or(scale),
            _ => {}
        }
    }
    let Ok(scale) = DeviceScale::new(scale) else {
        eprintln!("invalid --scale");
        std::process::exit(2);
    };
    let mut failed = false;
    for (name, html, css) in fixtures() {
        for _ in 0..runs.max(1) {
            match run_once(&html, &css, scale) {
                Ok(stages) => {
                    for (stage, time, check) in stages {
                        println!("sample {name}.{stage} {time} {check}");
                    }
                }
                Err(error) => {
                    println!("{name} not runnable: {error}");
                    failed = true;
                    break;
                }
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
}
