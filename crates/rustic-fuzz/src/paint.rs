//! Mutated HTML/style input exercises block layout, display lists, the CPU
//! rasterizer and PNG encoding end to end.

use crate::Target;
use rustic_base::CancellationToken;
use rustic_css::CssParser;
use rustic_html::HtmlDocumentParser;
use rustic_layout::{BlockLayout, BoxTree, CssSize, DeviceScale, LayoutLimits};
use rustic_paint::{DisplayCommand, DisplayList, ItemRole, PaintLimits};
use rustic_raster::{RasterLimits, damage, encode_png, rasterize};
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
            b"<img>",
            b"img{display:block;width:7.5px;height:3px}",
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
            b"x0H w",
            b"<span>",
            b"</span>",
            b"<br>",
            b"font:9px/7px 'Rustic Test'",
            b"font-style:italic",
            b"color:lime",
            b"letter-spacing:1.5px",
            b"text-align:justify",
            b"white-space:pre",
            b"text-decoration:underline overline line-through",
            b"text-decoration:underline double red",
            b"text-decoration-style:wavy",
            b"outline:3px double blue",
            b"outline-offset:-9px",
            b"outline:2px auto",
            b"border-radius:50%",
            b"border-radius:9px 3px/40% 2px",
            b"overflow:hidden;border-radius:12px",
            b"clip-path:circle(40% at 20% 30%)",
            b"clip-path:polygon(0 0,100% 0,0 100%)",
            b"clip-path:inset(2px round 50%) content-box",
            b"transform:rotate(30deg) scale(1.5)",
            b"box-shadow:3px 4px 5px red",
            b"mix-blend-mode:multiply",
            b"mix-blend-mode:luminosity",
            b"isolation:isolate",
            b"filter:blur(2px) hue-rotate(90deg)",
            b"filter:drop-shadow(2px 3px 1px blue) invert(1)",
            b"filter:grayscale(1) sepia(0.5) saturate(3) contrast(2)",
            b"box-shadow:inset -2px 1px 6px 2px rgb(0 0 0 / 50%), 0 0 0 4px blue",
            b"box-shadow:1px 1px 40px -3px",
            b"transform:translate(3px,-50%) skew(10deg)",
            b"transform:perspective(20px) rotateY(60deg)",
            b"transform:matrix3d(1,0,0,0.01,0,1,0,0,0,0,1,0,0,0,0,1)",
            b"rotate:x 120deg",
            b"scale:0",
            b"translate:2px 3px",
            b"transform-origin:0 0",
            b"perspective:30px",
            b"backface-visibility:hidden",
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
        let Ok(mut tree) = BoxTree::build(
            document,
            document.root(),
            &styles,
            LayoutLimits::default(),
            &cancel,
        ) else {
            return Ok(());
        };
        let viewport = CssSize::new(37.0, 23.0).map_err(|e| e.to_string())?;
        // Every replaced box shows a 3×2 test image with mixed alpha.
        let image = std::sync::Arc::new(
            rustic_paint::Image::new(
                3,
                2,
                [255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0].repeat(2),
            )
            .ok_or("test image")?,
        );
        let replaced: Vec<_> = tree
            .boxes()
            .iter()
            .filter(|b| b.natural_size().is_some())
            .map(|b| b.node())
            .collect();
        let natural = rustic_layout::NaturalSize::from_dimensions(
            rustic_layout::CssPx::new(3.0).map_err(|e| e.to_string())?,
            rustic_layout::CssPx::new(2.0).map_err(|e| e.to_string())?,
        );
        let sizes: Vec<_> = replaced.iter().map(|&node| (node, natural)).collect();
        if !sizes.is_empty() {
            tree.set_natural_sizes(&sizes).map_err(|e| e.to_string())?;
        }
        let fonts = fonts();
        let resources = rustic_paint::Resources {
            fonts: Some(fonts),
            images: replaced
                .iter()
                .map(|&node| (node, std::sync::Arc::clone(&image)))
                .collect(),
        };
        let Ok(layout) = BlockLayout::compute_with_fonts(
            &tree,
            viewport,
            LayoutLimits::default(),
            fonts,
            &cancel,
        ) else {
            return Ok(());
        };
        let Ok(list) = DisplayList::build_with(
            document,
            &tree,
            &layout,
            viewport,
            &resources,
            PaintLimits::default(),
            &cancel,
        ) else {
            return Ok(());
        };
        // Every box paints at most one background, each command refers to a
        // laid-out box, and groups are balanced.
        let mut backgrounds = std::collections::HashSet::new();
        // Open layers: `true` for a clip-path layer, `false` for a group.
        let mut layers: Vec<bool> = Vec::new();
        // Open transforms.
        let mut transforms: Vec<usize> = Vec::new();
        for command in list.commands() {
            match command {
                DisplayCommand::Fill(item) => {
                    let known = layout.fragments().iter().any(|f| f.node() == item.node)
                        || layout.inlines().iter().any(|f| f.node() == item.node)
                        || layout.texts().iter().any(|f| f.node() == item.node);
                    if !known {
                        return Err("display item for a box without a fragment".into());
                    }
                    if let rustic_paint::Shape::Path(index) = item.shape {
                        if index >= list.paths().len() {
                            return Err("text item names a missing path".into());
                        }
                    }
                    if let rustic_paint::Shape::Image { image, .. } = item.shape {
                        if image >= list.images().len() {
                            return Err("image item names a missing image".into());
                        }
                    }
                    if let rustic_paint::Shape::Shadow(s) = item.shape {
                        if s >= list.shadows().len() {
                            return Err("item names a missing shadow".into());
                        }
                    }
                    let missing = match item.shape {
                        rustic_paint::Shape::Rounded(r) => r >= list.rounded().len(),
                        rustic_paint::Shape::RoundedRing { outer, inner } => {
                            outer.max(inner) >= list.rounded().len()
                        }
                        _ => false,
                    };
                    if missing {
                        return Err("item names a missing rounded shape".into());
                    }
                    if item.rounded_clip.is_some_and(|c| c >= list.clips().len()) {
                        return Err("item names a missing rounded clip".into());
                    }
                    if item.role == ItemRole::Background && !backgrounds.insert(item.node) {
                        return Err("a box painted its background twice".into());
                    }
                }
                DisplayCommand::BeginGroup { opacity, .. } => {
                    if !(*opacity > 0.0 && *opacity <= 1.0) {
                        return Err("group opacity outside (0, 1]".into());
                    }
                    layers.push(false);
                }
                DisplayCommand::BeginClip { shape, .. } => {
                    let known = match *shape {
                        rustic_paint::Shape::Rect(_) => true,
                        rustic_paint::Shape::Rounded(r) => r < list.rounded().len(),
                        rustic_paint::Shape::Path(p) => p < list.paths().len(),
                        _ => false,
                    };
                    if !known {
                        return Err("clip-path layer with an invalid shape".into());
                    }
                    layers.push(true);
                }
                DisplayCommand::EndGroup => {
                    if layers.pop() != Some(false) {
                        return Err("unbalanced group end".into());
                    }
                }
                DisplayCommand::EndClip => {
                    if layers.pop() != Some(true) {
                        return Err("unbalanced clip-path end".into());
                    }
                }
                DisplayCommand::BeginTransform { transform, .. } => {
                    let Some(node) = list.transforms().get(*transform) else {
                        return Err("transform names a missing node".into());
                    };
                    if node.rounded_clip.is_some_and(|c| c >= list.clips().len())
                        || node.parent.is_some_and(|p| p >= *transform)
                    {
                        return Err("transform node with an invalid link".into());
                    }
                    transforms.push(*transform);
                }
                DisplayCommand::EndTransform => {
                    if transforms.pop().is_none() {
                        return Err("unbalanced transform end".into());
                    }
                }
            }
        }
        if !layers.is_empty() || !transforms.is_empty() {
            return Err("unclosed layer".into());
        }
        // Every hit inside the viewport names a laid-out box; outside, none.
        for (x, y) in [
            (0.0, 0.0),
            (3.5, 7.25),
            (18.0, 11.0),
            (36.9, 22.9),
            (37.0, 1.0),
        ] {
            let point = rustic_layout::CssPoint {
                x: rustic_layout::CssPx::new(x).map_err(|e| e.to_string())?,
                y: rustic_layout::CssPx::new(y).map_err(|e| e.to_string())?,
            };
            let known = |hit: rustic_paint::Hit| {
                layout
                    .fragments()
                    .iter()
                    .any(|f| f.node() == hit.node && f.source_box() == hit.source_box)
                    || layout
                        .inlines()
                        .iter()
                        .any(|f| f.node() == hit.node && f.source_box() == hit.source_box)
            };
            match list.hit_test(point) {
                Some(hit) if x >= 37.0 => return Err(format!("hit {hit:?} outside the viewport")),
                Some(hit) if !known(hit) => return Err("hit a box without a fragment".into()),
                _ => {}
            }
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
            if !damage(&list, &list, scale)
                .map_err(|e| e.to_string())?
                .is_empty()
            {
                return Err("an unchanged list reports damage".into());
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

/// The bundled test fonts, parsed once per process.
fn fonts() -> &'static rustic_text::FontDatabase {
    static FONTS: std::sync::OnceLock<rustic_text::FontDatabase> = std::sync::OnceLock::new();
    FONTS.get_or_init(rustic_text::bundled::test_database)
}
