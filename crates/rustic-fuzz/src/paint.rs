//! Mutated HTML/style input exercises block layout, display lists, the CPU
//! rasterizer and PNG encoding end to end.

use crate::Target;
use rustic_base::CancellationToken;
use rustic_css::CssParser;
use rustic_html::HtmlDocumentParser;
use rustic_layout::{BlockLayout, BoxTree, CssSize, DeviceScale, LayoutLimits};
use rustic_paint::{DisplayCommand, DisplayList, ItemRole, PaintLimits};
use rustic_raster::{RasterLimits, encode_png, rasterize};
use rustic_style::{ComputeContext, MatchContext, document_cascade, style_tree};

pub(crate) struct PaintTarget;
impl Target for PaintTarget {
    fn name(&self) -> &'static str {
        "paint-raster"
    }
    fn max_len(&self) -> usize {
        4096
    }
    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"<div>",
            b"</div>",
            b"<style>",
            b"</style>",
            b"<body style='margin:0'>",
            b"background:",
            b"rgb(0 128 255 / 40%)",
            b"oklch(0.7 0.2 30)",
            b"red",
            b"border:3.5px solid",
            b"border-color:red green blue black",
            b"background-clip:content-box",
            b"padding:2.25px",
            b"margin:-3px",
            b"height:7.3px",
            b"width:150%",
            b"visibility:hidden",
            b"display:flow-root",
            b"display:flex",
            b"flex:1 0 2px",
            b"flex-flow:column wrap-reverse",
            b"align-items:baseline",
            b"justify-content:space-around",
            b"width:fit-content",
            b"<canvas width=9 height=4>",
            b"position:relative",
            b"position:absolute;top:2px",
            b"position:fixed",
            b"z-index:-1",
            b"z-index:3",
            b"float:left;width:5px",
            b"overflow:hidden",
            b"overflow:clip",
            b"opacity:0.5",
            b"opacity:0",
            b"{",
            b"}",
        ]
    }
    fn check(&self, input: &[u8]) -> Result<(), String> {
        let mut parser = HtmlDocumentParser::default();
        if parser.feed(input).is_err() {
            return Ok(());
        }
        let Ok(parsed) = parser.finish() else {
            return Ok(());
        };
        let document = parsed.document();
        let parser = CssParser::default();
        let (cascade, _) = document_cascade(document, &parser).map_err(|e| e.to_string())?;
        let context = ComputeContext {
            viewport_width: 37.0,
            viewport_height: 23.0,
            ..ComputeContext::default()
        };
        let styles = style_tree(
            document,
            document.root(),
            &cascade,
            &parser,
            &MatchContext::html(),
            &context,
        )
        .map_err(|e| e.to_string())?;
        let cancel = CancellationToken::default();
        let Ok(tree) = BoxTree::build(
            document,
            document.root(),
            &styles,
            LayoutLimits::default(),
            &cancel,
        ) else {
            return Ok(());
        };
        let viewport = CssSize::new(37.0, 23.0).map_err(|e| e.to_string())?;
        let Ok(layout) = BlockLayout::compute(&tree, viewport, LayoutLimits::default(), &cancel)
        else {
            return Ok(());
        };
        let Ok(list) = DisplayList::build(
            document,
            &tree,
            &layout,
            viewport,
            PaintLimits::default(),
            &cancel,
        ) else {
            return Ok(());
        };
        // Every box paints at most one background, each command refers to a
        // laid-out box, and groups are balanced.
        let mut backgrounds = std::collections::HashSet::new();
        let mut depth = 0usize;
        for command in list.commands() {
            match command {
                DisplayCommand::Fill(item) => {
                    if !layout.fragments().iter().any(|f| f.node() == item.node) {
                        return Err("display item for a box without a fragment".into());
                    }
                    if item.role == ItemRole::Background && !backgrounds.insert(item.node) {
                        return Err("a box painted its background twice".into());
                    }
                }
                DisplayCommand::BeginGroup { opacity, .. } => {
                    if !(*opacity > 0.0 && *opacity < 1.0) {
                        return Err("group opacity outside (0, 1)".into());
                    }
                    depth += 1;
                }
                DisplayCommand::EndGroup => {
                    depth = depth.checked_sub(1).ok_or("unbalanced group end")?;
                }
            }
        }
        if depth != 0 {
            return Err("unclosed group".into());
        }
        for (scale, size) in [(1.0, (37, 23)), (1.5, (56, 35))] {
            let scale = DeviceScale::new(scale).map_err(|e| e.to_string())?;
            let surface = rasterize(&list, scale, RasterLimits::default(), &cancel)
                .map_err(|e| e.to_string())?;
            if (surface.width(), surface.height()) != size {
                return Err("surface size does not follow viewport and scale".into());
            }
            // Source-over onto an opaque canvas must stay opaque everywhere.
            if surface.premultiplied().chunks_exact(4).any(|p| p[3] != 255) {
                return Err("canvas pixel lost opacity".into());
            }
            let again = rasterize(&list, scale, RasterLimits::default(), &cancel)
                .map_err(|e| e.to_string())?;
            if again != surface {
                return Err("rasterization is not deterministic".into());
            }
            let png = encode_png(&surface, usize::MAX).map_err(|e| e.to_string())?;
            let raw = (surface.width() as usize * 4 + 1) * surface.height() as usize;
            let expected = 8 + 25 + 12 + 12 + 6 + raw + 5 * raw.div_ceil(65_535);
            if png.len() != expected {
                return Err("PNG length does not match its stored-block layout".into());
            }
        }
        Ok(())
    }
}
