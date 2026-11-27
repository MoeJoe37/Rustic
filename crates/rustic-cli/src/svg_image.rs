//! SVG files as images (`<img src=x.svg>`; SVG 2 §1.4 secure static mode).
//!
//! The file is parsed with the HTML parser's foreign-content rules (after
//! its XML declaration, doctype and comments before the root are removed),
//! so ordinary SVG files work without an XML parser; a file that is not
//! well-formed XML may still render (TD-220). The document gets the user
//! agent sheet and its own `<style>` elements, never runs scripts and loads
//! nothing: `use` and paint servers resolve within it. Its outermost `svg`
//! is laid out alone in a viewport of its natural size and rasterized over
//! a transparent canvas.

use std::sync::Arc;

use rustic_base::CancellationToken;
use rustic_css::CssParser;
use rustic_dom::{NodeKind, SVG_NAMESPACE};
use rustic_html::HtmlDocumentParser;
use rustic_layout::{BlockLayout, BoxTree, CssSize, DeviceScale, LayoutLimits};
use rustic_paint::{DisplayList, Image, PaintLimits, Resources};
use rustic_style::{ComputeContext, MatchContext, document_cascade, style_tree};

/// The largest side an SVG image is rendered at, in pixels.
const MAX_SIDE: f64 = 4096.0;

/// Whether `bytes` look like an SVG document rather than a raster image.
pub(crate) fn sniff(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let Ok(text) = std::str::from_utf8(head.get(..head.len()).unwrap_or(head))
        .or_else(|e| std::str::from_utf8(&head[..e.valid_up_to()]))
    else {
        return false;
    };
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with('<') && text.contains("<svg")
}

/// The SVG document in `bytes` rendered at its natural size, or `None`
/// when it is not an SVG document or has nothing to show.
pub(crate) fn render(
    bytes: &[u8],
    fonts: &rustic_text::FontDatabase,
    cancellation: &CancellationToken,
) -> Option<Image> {
    let text = String::from_utf8_lossy(bytes);
    let markup = strip_prolog(&text);
    let mut parser = HtmlDocumentParser::default();
    parser
        .feed(format!("<!doctype html><html><body style='margin:0'>{markup}").as_bytes())
        .ok()?;
    let document = parser.finish().ok()?.into_document();
    let root = document.walk(document.root()).ok()?.map(|(n, _)| n).find(|&n| {
        matches!(document.node(n).map(|n| n.kind()),
            Ok(NodeKind::Element(e)) if e.name().namespace() == Some(SVG_NAMESPACE) && e.name().local() == "svg")
    })?;
    let NodeKind::Element(svg) = document.node(root).ok()?.kind() else {
        return None;
    };
    let (width, height) = natural_size(svg);
    let css = CssParser::default();
    let (cascade, _) = document_cascade(&document, &css).ok()?;
    let context = ComputeContext::with_fonts(Arc::new(fonts.clone()));
    let styles = style_tree(
        &document,
        document.root(),
        &cascade,
        &css,
        &MatchContext::html(),
        &context,
    )
    .ok()?;
    let tree = BoxTree::build(
        &document,
        document.root(),
        &styles,
        LayoutLimits::default(),
        cancellation,
    )
    .ok()?;
    let viewport = CssSize::new(width, height).ok()?;
    let layout = BlockLayout::compute_with_fonts(
        &tree,
        viewport,
        LayoutLimits::default(),
        fonts,
        cancellation,
    )
    .ok()?;
    let svg_styles = styles
        .iter()
        .filter(|s| {
            matches!(document.node(s.node).map(|n| n.kind()),
                Ok(NodeKind::Element(e)) if e.name().namespace() == Some(SVG_NAMESPACE))
        })
        .map(|s| (s.node, Arc::new(s.computed.clone())))
        .collect();
    let resources = Resources {
        fonts: Some(fonts),
        images: std::collections::HashMap::new(),
        svg_styles,
    };
    let list = DisplayList::build_with(
        &document,
        &tree,
        &layout,
        viewport,
        &resources,
        PaintLimits::default(),
        cancellation,
    )
    .ok()?
    .over_transparent();
    let surface = rustic_raster::rasterize(
        &list,
        DeviceScale::new(1.0).ok()?,
        rustic_raster::RasterLimits::default(),
        cancellation,
    )
    .ok()?;
    let (w, h) = (surface.width(), surface.height());
    let mut pixels = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        for x in 0..w {
            let p = surface.pixel(x, y)?;
            pixels.extend_from_slice(&[p.r, p.g, p.b, p.a]);
        }
    }
    Image::new(w, h, pixels)
}

/// The markup from the root element on: no XML declaration, doctype,
/// processing instructions or comments before it.
fn strip_prolog(text: &str) -> &str {
    let mut rest = text.trim_start_matches('\u{feff}');
    loop {
        rest = rest.trim_start();
        let end = if rest.starts_with("<?") {
            rest.find("?>").map(|i| i + 2)
        } else if rest.starts_with("<!--") {
            rest.find("-->").map(|i| i + 3)
        } else if rest.starts_with("<!") {
            rest.find('>').map(|i| i + 1)
        } else {
            return rest;
        };
        match end {
            Some(end) => rest = &rest[end..],
            None => return "",
        }
    }
}

/// An SVG image's natural size: its `width` and `height` (absolute
/// lengths), else the `viewBox` size scaled to a given dimension, else
/// 300×150 (CSS replaced-element defaults); capped at [`MAX_SIDE`].
fn natural_size(svg: &rustic_dom::Element) -> (f64, f64) {
    let attribute = |name: &str| {
        svg.attributes()
            .iter()
            .find(|a| a.name().namespace().is_none() && a.name().local() == name)
            .map(|a| String::from_utf16_lossy(a.value().as_units()))
    };
    let length = |name: &str| {
        let value = attribute(name)?;
        let value = value.trim();
        let number = value.strip_suffix("px").unwrap_or(value).trim();
        number
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && *n > 0.0)
    };
    let view_box = attribute("viewBox").and_then(|v| {
        let n: Vec<f64> = v
            .split(|c: char| c.is_ascii_whitespace() || c == ',')
            .filter(|t| !t.is_empty())
            .map(str::parse::<f64>)
            .collect::<Result<_, _>>()
            .ok()?;
        (n.len() == 4 && n[2] > 0.0 && n[3] > 0.0).then(|| (n[2], n[3]))
    });
    let (w, h) = match (length("width"), length("height"), view_box) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some((vw, vh))) => (w, w * vh / vw),
        (None, Some(h), Some((vw, vh))) => (h * vw / vh, h),
        (None, None, Some((vw, vh))) => (vw, vh),
        (Some(w), None, None) => (w, 150.0),
        (None, Some(h), None) => (300.0, h),
        (None, None, None) => (300.0, 150.0),
    };
    let scale = (MAX_SIDE / w.max(h)).min(1.0);
    ((w * scale).round().max(1.0), (h * scale).round().max(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prolog_before_the_root_is_dropped() {
        let text = "\u{feff}<?xml version='1.0'?>\n<!-- c --><!DOCTYPE svg><svg/>";
        assert_eq!(strip_prolog(text), "<svg/>");
        assert!(sniff(text.as_bytes()));
        assert!(!sniff(b"\x89PNG\r\n"));
    }

    #[test]
    fn svg_images_render_with_transparency_at_their_natural_size() {
        let fonts = rustic_text::bundled::test_database();
        let image = render(
            b"<?xml version='1.0'?><svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 20 10' width='40'>\
              <style>.r { fill: #f00 }</style><rect class='r' width='10' height='10'/></svg>",
            &fonts,
            &CancellationToken::default(),
        )
        .unwrap();
        assert_eq!((image.width(), image.height()), (40, 20));
        let px = |x: usize, y: usize| {
            let i = (y * 40 + x) * 4;
            image.pixels()[i..i + 4].to_vec()
        };
        assert_eq!(px(5, 5), [255, 0, 0, 255]);
        assert_eq!(px(30, 5)[3], 0, "transparent outside the shapes");
    }
}
