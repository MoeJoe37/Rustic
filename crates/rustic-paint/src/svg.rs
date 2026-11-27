//! Inline SVG painting (SVG 2): an `<svg>` element's subtree turned into
//! filled polygons in CSS pixels, painted in its content box, with clip
//! layers and opacity groups around them.
//!
//! Supported: nested `svg`, `g`, `use` (of an element or a `symbol` by
//! id), `rect` (with `rx`/`ry`), `circle`, `ellipse`, `line`, `polyline`,
//! `polygon` and `path` (every command, arcs included); `viewBox` with
//! `preserveAspectRatio`; `transform`; fills (`fill-rule` included) and
//! strokes (width, caps, joins, miter limit, dashes); linear and radial
//! gradients (stops, units, `gradientTransform`, `href` inheritance; `pad`
//! spreading, the focal point ignored); `clipPath` (`clipPathUnits`
//! included); masks as clips of their shapes; markers (start, mid, end,
//! `orient`, `markerUnits`, `viewBox`); `opacity` on groups as a
//! composited group; the presentation attributes and the same properties
//! in `style` attributes. Text, patterns (their fallback color paints),
//! filters and author style sheets are not applied (TD-220).
//!
//! Curves and arcs are flattened to polylines in user space; strokes are
//! built in user space (so transforms scale them as SVG requires) as a
//! union of positively wound quads, joins and caps, which the rasterizer's
//! clamped nonzero coverage merges without seams. A gradient paints as
//! bands of one color each (slabs for linear, nested discs for radial),
//! each clipped to the painted shape.

use std::sync::Arc;

use rustic_css::{Color, CssParser};
use rustic_dom::{Document, Element, NodeId, NodeKind, SVG_NAMESPACE};
use rustic_layout::CssRect;

use crate::color::{Srgba, srgba};

/// One painting operation in CSS pixels.
pub(crate) enum Op {
    /// Fill the contours.
    Fill {
        contours: Vec<Vec<(f64, f64)>>,
        color: Srgba,
        even_odd: bool,
    },
    /// Until the matching [`Op::EndClip`], keep only what the contours
    /// (nonzero) cover.
    BeginClip(Vec<Vec<(f64, f64)>>),
    EndClip,
    /// Until the matching [`Op::EndGroup`], paint into a group composited
    /// with this opacity.
    BeginGroup(f64),
    EndGroup,
}

/// Elements visited and polygon points produced, at most.
const MAX_ELEMENTS: usize = 50_000;
const MAX_POINTS: usize = 2_000_000;
/// `use` and marker references followed inside one another, at most.
const MAX_USE_DEPTH: usize = 8;
/// Bands a gradient paints in over its 0–1 range.
const GRADIENT_BANDS: usize = 96;

/// A 2D affine transform `[a, b, c, d, e, f]`: (x, y) maps to
/// (a·x + c·y + e, b·x + d·y + f).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Affine([f64; 6]);

impl Affine {
    const IDENTITY: Self = Self([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    fn then(self, inner: Self) -> Self {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = inner.0;
        Self([
            a * a2 + c * b2,
            b * a2 + d * b2,
            a * c2 + c * d2,
            b * c2 + d * d2,
            a * e2 + c * f2 + e,
            b * e2 + d * f2 + f,
        ])
    }

    fn apply(self, (x, y): (f64, f64)) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }

    /// The square root of the absolute determinant: how much lengths grow.
    fn scale(self) -> f64 {
        let [a, b, c, d, ..] = self.0;
        (a * d - b * c).abs().sqrt()
    }

    fn translate(x: f64, y: f64) -> Self {
        Self([1.0, 0.0, 0.0, 1.0, x, y])
    }

    fn scaling(x: f64, y: f64) -> Self {
        Self([x, 0.0, 0.0, y, 0.0, 0.0])
    }

    fn rotation(degrees: f64) -> Self {
        let (s, c) = degrees.to_radians().sin_cos();
        Self([c, s, -s, c, 0.0, 0.0])
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Paint {
    None,
    Color(Srgba),
    Current,
    /// A paint server by id, with the fallback paint.
    Server(String, Box<Paint>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Cap {
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Join {
    Miter,
    Round,
    Bevel,
}

/// The inherited painting properties.
#[derive(Clone, Debug)]
struct Style {
    fill: Paint,
    stroke: Paint,
    fill_opacity: f64,
    stroke_opacity: f64,
    stroke_width: f64,
    cap: Cap,
    join: Join,
    miter_limit: f64,
    even_odd: bool,
    dashes: Option<Arc<[f64]>>,
    dash_offset: f64,
    markers: [Option<String>; 3],
    color: Srgba,
    visible: bool,
}

/// An element's own (not inherited) properties.
#[derive(Clone, Debug, Default)]
struct Own {
    opacity: Option<f64>,
    clip: Option<String>,
    mask: Option<String>,
}

const BLACK: Srgba = Srgba {
    r: 0,
    g: 0,
    b: 0,
    a: 255,
};

impl Style {
    fn initial(color: Srgba) -> Self {
        Self {
            fill: Paint::Color(BLACK),
            stroke: Paint::None,
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
            stroke_width: 1.0,
            cap: Cap::Butt,
            join: Join::Miter,
            miter_limit: 4.0,
            even_odd: false,
            dashes: None,
            dash_offset: 0.0,
            markers: [None, None, None],
            color,
            visible: true,
        }
    }
}

struct Painter<'a> {
    document: &'a Document,
    parser: CssParser,
    out: Vec<Op>,
    elements: usize,
    points: usize,
    /// The nearest viewport's size, for percentages.
    viewport: (f64, f64),
    use_depth: usize,
    /// Collecting clip geometry: every shape's fill area, unpainted.
    geometry: bool,
}

/// The painting operations of the `svg` element `root`, mapped into
/// `content` (its content box), with `color` as its `currentColor`.
pub(crate) fn paint(document: &Document, root: NodeId, content: CssRect, color: Srgba) -> Vec<Op> {
    let Some(element) = svg_element(document, root) else {
        return Vec::new();
    };
    let (x, y) = (content.origin().x.get(), content.origin().y.get());
    let (w, h) = (content.size().width().get(), content.size().height().get());
    let mut painter = Painter::new(document, (w, h), false);
    let base = Affine::translate(x, y);
    if let Some((style, own)) = painter.style(element, Style::initial(color)) {
        let (inner, size) = viewport_transform(element, (w, h));
        painter.viewport = size;
        let transform = base.then(inner);
        let closers = painter.open(&own, transform, None, 1.0);
        painter.children(root, transform, &style);
        painter.close(closers);
    }
    painter.out
}

fn svg_element(document: &Document, id: NodeId) -> Option<&Element> {
    match document.node(id).ok()?.kind() {
        NodeKind::Element(e) if e.name().namespace() == Some(SVG_NAMESPACE) => Some(e),
        _ => None,
    }
}

fn attribute(element: &Element, name: &str) -> Option<String> {
    element
        .attributes()
        .iter()
        .find(|a| a.name().namespace().is_none() && a.name().local() == name)
        .map(|a| String::from_utf16_lossy(a.value().as_units()))
}

/// The `href` (or `xlink:href`) of a `use` or a paint server.
fn href(element: &Element) -> Option<String> {
    element
        .attributes()
        .iter()
        .find(|a| a.name().local() == "href")
        .map(|a| String::from_utf16_lossy(a.value().as_units()))
}

/// The id a `url(#id)` reference names.
fn url_id(value: &str) -> Option<String> {
    let rest = value.trim().strip_prefix("url(")?;
    let (reference, _) = rest.split_once(')')?;
    Some(
        reference
            .trim()
            .trim_matches(['"', '\''])
            .strip_prefix('#')?
            .to_owned(),
    )
}

/// The transform from an `svg` element's user space to its viewport of
/// `size`, and the size percentages resolve against.
fn viewport_transform(element: &Element, size: (f64, f64)) -> (Affine, (f64, f64)) {
    match attribute(element, "viewBox").and_then(|v| view_box(&v)) {
        Some(view_box) => (
            fit(
                view_box,
                size,
                &attribute(element, "preserveAspectRatio").unwrap_or_default(),
            ),
            (view_box.2, view_box.3),
        ),
        None => (Affine::IDENTITY, size),
    }
}

fn view_box(value: &str) -> Option<(f64, f64, f64, f64)> {
    let n = numbers(value);
    (n.len() == 4 && n[2] > 0.0 && n[3] > 0.0).then(|| (n[0], n[1], n[2], n[3]))
}

/// The `preserveAspectRatio` map of `view_box` onto a viewport of `size`.
fn fit((vx, vy, vw, vh): (f64, f64, f64, f64), (w, h): (f64, f64), aspect: &str) -> Affine {
    let mut words = aspect.split_ascii_whitespace();
    let align = words.next().unwrap_or("xMidYMid");
    let slice = words.next() == Some("slice");
    let (sx, sy) = (w / vw, h / vh);
    if align == "none" {
        return Affine([sx, 0.0, 0.0, sy, -vx * sx, -vy * sy]);
    }
    let s = if slice { sx.max(sy) } else { sx.min(sy) };
    let fraction = |key: &str| {
        if align.contains(&format!("{key}Min")) {
            0.0
        } else if align.contains(&format!("{key}Max")) {
            1.0
        } else {
            0.5
        }
    };
    let tx = (w - vw * s) * fraction("x");
    let ty = (h - vh * s) * fraction("Y");
    Affine([s, 0.0, 0.0, s, tx - vx * s, ty - vy * s])
}

/// The bounding box of subpaths, `(x, y, width, height)`.
fn bounds(subpaths: &[Subpath]) -> Option<(f64, f64, f64, f64)> {
    let mut points = subpaths.iter().flat_map(|s| s.points.iter());
    let first = points.next()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
    for p in points {
        x0 = x0.min(p.0);
        y0 = y0.min(p.1);
        x1 = x1.max(p.0);
        y1 = y1.max(p.1);
    }
    Some((x0, y0, x1 - x0, y1 - y0))
}

/// What closes the layers [`Painter::open`] opened.
struct Closers {
    clip: bool,
    group: bool,
}

impl<'a> Painter<'a> {
    fn new(document: &'a Document, viewport: (f64, f64), geometry: bool) -> Self {
        Self {
            document,
            parser: CssParser::default(),
            out: Vec::new(),
            elements: 0,
            points: 0,
            viewport,
            use_depth: 0,
            geometry,
        }
    }

    /// Open the opacity group and clip layer `own` asks for, an element
    /// drawn with `transform` whose geometry bounds are `bbox` (for
    /// `objectBoundingBox` clips). `folded` is opacity the caller folds
    /// into colors instead.
    fn open(
        &mut self,
        own: &Own,
        transform: Affine,
        bbox: Option<(f64, f64, f64, f64)>,
        folded: f64,
    ) -> Closers {
        let mut closers = Closers {
            clip: false,
            group: false,
        };
        if self.geometry {
            return closers;
        }
        if let Some(opacity) = own.opacity.filter(|o| *o < 1.0 && folded >= 1.0) {
            self.out.push(Op::BeginGroup(opacity.max(1e-6)));
            closers.group = true;
        }
        let reference = own.clip.as_ref().or(own.mask.as_ref());
        if let Some(region) = reference.and_then(|id| self.clip_region(id, transform, bbox)) {
            self.out.push(Op::BeginClip(region));
            closers.clip = true;
        }
        closers
    }

    fn close(&mut self, closers: Closers) {
        if closers.clip {
            self.out.push(Op::EndClip);
        }
        if closers.group {
            self.out.push(Op::EndGroup);
        }
    }

    /// The area a `clipPath` (or, approximated, a `mask`) keeps, in CSS
    /// pixels; `None` when it is missing (no clipping).
    fn clip_region(
        &mut self,
        id: &str,
        transform: Affine,
        bbox: Option<(f64, f64, f64, f64)>,
    ) -> Option<Vec<Vec<(f64, f64)>>> {
        if self.use_depth >= MAX_USE_DEPTH {
            return None;
        }
        let node = self.by_id(id)?;
        let element = svg_element(self.document, node)?;
        if !matches!(element.name().local(), "clipPath" | "mask") {
            return None;
        }
        let units = if element.name().local() == "clipPath" {
            attribute(element, "clipPathUnits")
        } else {
            attribute(element, "maskContentUnits")
        };
        let mut space = transform;
        if units.as_deref() == Some("objectBoundingBox") {
            let (x, y, w, h) = bbox?;
            space = space.then(Affine([w, 0.0, 0.0, h, x, y]));
        }
        if let Some(t) = attribute(element, "transform") {
            space = space.then(parse_transform(&t));
        }
        let mut collector = Painter::new(self.document, self.viewport, true);
        collector.use_depth = self.use_depth + 1;
        let style = Style::initial(BLACK);
        collector.children(node, space, &style);
        self.elements += collector.elements;
        let contours: Vec<Vec<(f64, f64)>> = collector
            .out
            .into_iter()
            .flat_map(|op| match op {
                // Every shape adds: its winding is made positive.
                Op::Fill { contours, .. } => contours.into_iter().map(positive).collect(),
                _ => Vec::new(),
            })
            .collect();
        Some(contours)
    }

    fn children(&mut self, parent: NodeId, transform: Affine, style: &Style) {
        let Ok(children) = self.document.children(parent) else {
            return;
        };
        let children: Vec<NodeId> = children.collect();
        for child in children {
            self.element(child, transform, style);
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one match over the SVG element kinds"
    )]
    fn element(&mut self, id: NodeId, transform: Affine, inherited: &Style) {
        self.elements += 1;
        if self.elements > MAX_ELEMENTS || self.points > MAX_POINTS {
            return;
        }
        let Some(element) = svg_element(self.document, id) else {
            return;
        };
        let Some((style, own)) = self.style(element, inherited.clone()) else {
            return;
        };
        let local = element.name().local();
        let transform = match attribute(element, "transform") {
            Some(t) => transform.then(parse_transform(&t)),
            None => transform,
        };
        let number = |name: &str, axis: Axis, viewport: (f64, f64)| {
            attribute(element, name)
                .and_then(|v| length(&v, axis, viewport))
                .unwrap_or(0.0)
        };
        let vp = self.viewport;
        let subpaths: Vec<Subpath> = match local {
            "g" | "a" | "switch" => {
                let closers = self.open(&own, transform, None, 1.0);
                self.children(id, transform, &style);
                self.close(closers);
                return;
            }
            "svg" => {
                let (x, y) = (number("x", Axis::X, vp), number("y", Axis::Y, vp));
                let w = attribute(element, "width")
                    .and_then(|v| length(&v, Axis::X, vp))
                    .unwrap_or(vp.0);
                let h = attribute(element, "height")
                    .and_then(|v| length(&v, Axis::Y, vp))
                    .unwrap_or(vp.1);
                let (inner, size) = viewport_transform(element, (w, h));
                let saved = std::mem::replace(&mut self.viewport, size);
                let closers = self.open(&own, transform, None, 1.0);
                self.children(
                    id,
                    transform.then(Affine::translate(x, y)).then(inner),
                    &style,
                );
                self.close(closers);
                self.viewport = saved;
                return;
            }
            "use" => {
                if self.use_depth >= MAX_USE_DEPTH {
                    return;
                }
                let Some(target) = href(element)
                    .and_then(|h| h.strip_prefix('#').map(str::to_owned))
                    .and_then(|id| self.by_id(&id))
                else {
                    return;
                };
                let (x, y) = (number("x", Axis::X, vp), number("y", Axis::Y, vp));
                self.use_depth += 1;
                let transform = transform.then(Affine::translate(x, y));
                let closers = self.open(&own, transform, None, 1.0);
                match svg_element(self.document, target) {
                    Some(symbol) if symbol.name().local() == "symbol" => {
                        // A symbol renders as a group of its children.
                        if let Some((symbol_style, _)) = self.symbol_style(symbol, &style) {
                            let size = attribute(element, "width")
                                .zip(attribute(element, "height"))
                                .and_then(|(w, h)| {
                                    Some((length(&w, Axis::X, vp)?, length(&h, Axis::Y, vp)?))
                                });
                            let (inner, _) = size.map_or((Affine::IDENTITY, vp), |size| {
                                viewport_transform(symbol, size)
                            });
                            self.children(target, transform.then(inner), &symbol_style);
                        }
                    }
                    _ => self.element(target, transform, &style),
                }
                self.close(closers);
                self.use_depth -= 1;
                return;
            }
            "rect" => {
                let (x, y) = (number("x", Axis::X, vp), number("y", Axis::Y, vp));
                let (w, h) = (number("width", Axis::X, vp), number("height", Axis::Y, vp));
                if w <= 0.0 || h <= 0.0 {
                    return;
                }
                let rx = attribute(element, "rx").and_then(|v| length(&v, Axis::X, vp));
                let ry = attribute(element, "ry").and_then(|v| length(&v, Axis::Y, vp));
                let (rx, ry) = match (rx, ry) {
                    (Some(rx), Some(ry)) => (rx, ry),
                    (Some(r), None) | (None, Some(r)) => (r, r),
                    (None, None) => (0.0, 0.0),
                };
                let (rx, ry) = (rx.clamp(0.0, w / 2.0), ry.clamp(0.0, h / 2.0));
                vec![rounded_rect(x, y, w, h, rx, ry)]
            }
            "circle" => {
                let r = number("r", Axis::Other, vp);
                if r <= 0.0 {
                    return;
                }
                vec![ellipse(
                    number("cx", Axis::X, vp),
                    number("cy", Axis::Y, vp),
                    r,
                    r,
                )]
            }
            "ellipse" => {
                let (rx, ry) = (number("rx", Axis::X, vp), number("ry", Axis::Y, vp));
                if rx <= 0.0 || ry <= 0.0 {
                    return;
                }
                vec![ellipse(
                    number("cx", Axis::X, vp),
                    number("cy", Axis::Y, vp),
                    rx,
                    ry,
                )]
            }
            "line" => vec![Subpath::polyline(
                vec![
                    (number("x1", Axis::X, vp), number("y1", Axis::Y, vp)),
                    (number("x2", Axis::X, vp), number("y2", Axis::Y, vp)),
                ],
                false,
            )],
            "polyline" | "polygon" => {
                let n = attribute(element, "points")
                    .map(|p| numbers(&p))
                    .unwrap_or_default();
                let points: Vec<(f64, f64)> = n.chunks_exact(2).map(|p| (p[0], p[1])).collect();
                if points.is_empty() {
                    return;
                }
                vec![Subpath::polyline(points, local == "polygon")]
            }
            "path" => match attribute(element, "d") {
                Some(d) => parse_path(&d, transform.scale()),
                None => return,
            },
            _ => return,
        };
        if !style.visible && !self.geometry {
            return;
        }
        let bbox = bounds(&subpaths);
        let fill_contours = || -> Vec<Vec<(f64, f64)>> {
            subpaths
                .iter()
                .filter(|s| s.points.len() >= 3)
                .map(|s| s.points.iter().map(|&p| transform.apply(p)).collect())
                .collect()
        };
        if self.geometry {
            let contours = fill_contours();
            self.emit(contours, BLACK, false);
            return;
        }
        // A shape's own opacity folds into its colors, unless it is
        // clipped (the clip layer composites it as a group instead).
        let folded = own.opacity.unwrap_or(1.0);
        let closers = self.open(&own, transform, bbox, folded);
        let alpha = |opacity: f64| opacity * folded;
        // `line` has no interior; open parts of paths fill closed.
        if local != "line" {
            let contours = fill_contours();
            self.paint_area(
                contours,
                &style.fill,
                &style,
                alpha(style.fill_opacity),
                bbox,
                transform,
                style.even_odd,
            );
        }
        if style.stroke != Paint::None && style.stroke_width > 0.0 {
            let mut contours = Vec::new();
            for subpath in &subpaths {
                stroke(subpath, &style, transform.scale(), &mut contours);
            }
            let contours = contours
                .into_iter()
                .map(|c| positive(c.into_iter().map(|p| transform.apply(p)).collect()))
                .collect();
            self.paint_area(
                contours,
                &style.stroke,
                &style,
                alpha(style.stroke_opacity),
                bbox,
                transform,
                false,
            );
        }
        if matches!(local, "path" | "line" | "polyline" | "polygon") {
            self.markers(&subpaths, transform, &style);
        }
        self.close(closers);
    }

    /// Paint `contours` (CSS pixels) with `paint`.
    #[allow(
        clippy::too_many_arguments,
        reason = "the painted area and its context"
    )]
    fn paint_area(
        &mut self,
        contours: Vec<Vec<(f64, f64)>>,
        paint: &Paint,
        style: &Style,
        alpha: f64,
        bbox: Option<(f64, f64, f64, f64)>,
        transform: Affine,
        even_odd: bool,
    ) {
        match paint {
            Paint::None => {}
            Paint::Color(c) => self.emit(contours, with_alpha(*c, alpha), even_odd),
            Paint::Current => self.emit(contours, with_alpha(style.color, alpha), even_odd),
            Paint::Server(id, fallback) => {
                if let Some(gradient) = self.gradient(id, bbox, transform) {
                    self.gradient_fill(&contours, &gradient, alpha, even_odd);
                } else {
                    self.paint_area(contours, fallback, style, alpha, bbox, transform, even_odd);
                }
            }
        }
    }

    fn emit(&mut self, contours: Vec<Vec<(f64, f64)>>, color: Srgba, even_odd: bool) {
        if contours.is_empty() || (color.is_transparent() && !self.geometry) {
            return;
        }
        self.points += contours.iter().map(Vec::len).sum::<usize>();
        if self.points > MAX_POINTS {
            return;
        }
        self.out.push(Op::Fill {
            contours,
            color,
            even_odd,
        });
    }

    fn by_id(&self, id: &str) -> Option<NodeId> {
        let walk = self.document.walk(self.document.root()).ok()?;
        for (node, _) in walk {
            if let Ok(NodeKind::Element(e)) = self.document.node(node).map(|n| n.kind()) {
                if attribute(e, "id").as_deref() == Some(id) {
                    return Some(node);
                }
            }
        }
        None
    }

    /// A `symbol`'s style: it is otherwise a non-rendering element.
    fn symbol_style(&self, symbol: &Element, inherited: &Style) -> Option<(Style, Own)> {
        self.declared(symbol, inherited.clone())
    }

    /// The element's own style over the inherited one; `None` when it is
    /// not rendered (`display: none`) or is a non-rendering element.
    fn style(&self, element: &Element, inherited: Style) -> Option<(Style, Own)> {
        if matches!(
            element.name().local(),
            "defs"
                | "clipPath"
                | "mask"
                | "linearGradient"
                | "radialGradient"
                | "pattern"
                | "marker"
                | "filter"
                | "symbol"
                | "title"
                | "desc"
                | "metadata"
                | "style"
                | "script"
        ) {
            return None;
        }
        self.declared(element, inherited)
    }

    fn declared(&self, element: &Element, inherited: Style) -> Option<(Style, Own)> {
        let mut style = inherited;
        let mut own = Own::default();
        let mut declarations: Vec<(String, String)> = element
            .attributes()
            .iter()
            .filter(|a| a.name().namespace().is_none())
            .map(|a| {
                (
                    a.name().local().to_owned(),
                    String::from_utf16_lossy(a.value().as_units()),
                )
            })
            .filter(|(name, _)| name != "style")
            .collect();
        if let Some(inline) = attribute(element, "style") {
            for declaration in inline.split(';') {
                if let Some((name, value)) = declaration.split_once(':') {
                    let value = value.trim().trim_end_matches("!important").trim();
                    declarations.push((name.trim().to_ascii_lowercase(), value.to_owned()));
                }
            }
        }
        // `color` first: `currentColor` in the others refers to it.
        if let Some((_, value)) = declarations.iter().rev().find(|(name, _)| name == "color") {
            if let Some(color) = self.color(value, style.color) {
                style.color = color;
            }
        }
        let marker = |value: &str| (value != "none").then(|| url_id(value)).flatten();
        for (name, value) in &declarations {
            let value = value.trim();
            match name.as_str() {
                "display" if value == "none" => return None,
                "visibility" => style.visible = value == "visible",
                "fill" => {
                    if let Some(paint) = self.paint(value) {
                        style.fill = paint;
                    }
                }
                "stroke" => {
                    if let Some(paint) = self.paint(value) {
                        style.stroke = paint;
                    }
                }
                "fill-opacity" => style.fill_opacity = opacity(value).unwrap_or(style.fill_opacity),
                "stroke-opacity" => {
                    style.stroke_opacity = opacity(value).unwrap_or(style.stroke_opacity);
                }
                "opacity" => own.opacity = opacity(value),
                "clip-path" => own.clip = url_id(value),
                "mask" => own.mask = url_id(value),
                "fill-rule" => style.even_odd = value == "evenodd",
                "stroke-width" => {
                    if let Some(w) = length(value, Axis::Other, self.viewport).filter(|w| *w >= 0.0)
                    {
                        style.stroke_width = w;
                    }
                }
                "stroke-linecap" => {
                    style.cap = match value {
                        "round" => Cap::Round,
                        "square" => Cap::Square,
                        _ => Cap::Butt,
                    };
                }
                "stroke-linejoin" => {
                    style.join = match value {
                        "round" => Join::Round,
                        "bevel" => Join::Bevel,
                        _ => Join::Miter,
                    };
                }
                "stroke-miterlimit" => {
                    if let Some(m) = numbers(value).first().filter(|m| **m >= 1.0) {
                        style.miter_limit = *m;
                    }
                }
                "stroke-dasharray" => {
                    let mut dashes: Vec<f64> = value
                        .split(|c: char| c.is_ascii_whitespace() || c == ',')
                        .filter(|t| !t.is_empty())
                        .map_while(|t| length(t, Axis::Other, self.viewport))
                        .collect();
                    if dashes.len() % 2 == 1 {
                        dashes.extend(dashes.clone());
                    }
                    style.dashes = (value != "none"
                        && !dashes.is_empty()
                        && dashes.iter().all(|d| *d >= 0.0)
                        && dashes.iter().sum::<f64>() > 0.0)
                        .then(|| dashes.into());
                }
                "stroke-dashoffset" => {
                    style.dash_offset = length(value, Axis::Other, self.viewport).unwrap_or(0.0);
                }
                "marker" => {
                    let id = marker(value);
                    style.markers = [id.clone(), id.clone(), id];
                }
                "marker-start" => style.markers[0] = marker(value),
                "marker-mid" => style.markers[1] = marker(value),
                "marker-end" => style.markers[2] = marker(value),
                _ => {}
            }
        }
        Some((style, own))
    }

    fn color(&self, value: &str, current: Srgba) -> Option<Srgba> {
        if value.eq_ignore_ascii_case("currentcolor") || value == "inherit" {
            return Some(current);
        }
        let parsed = self.parser.component_values(value).ok()?.value;
        let mut values = parsed.iter().filter(|v| !v.is_whitespace());
        let first = values.next()?;
        if values.next().is_some() {
            return None;
        }
        match Color::parse(first)? {
            Color::CurrentColor => Some(current),
            color => srgba(&color),
        }
    }

    fn paint(&self, value: &str) -> Option<Paint> {
        if value == "none" {
            return Some(Paint::None);
        }
        if value.eq_ignore_ascii_case("currentcolor") {
            return Some(Paint::Current);
        }
        if value.starts_with("url(") {
            let id = url_id(value)?;
            let fallback = value.split_once(')').map_or("", |(_, rest)| rest).trim();
            let fallback = if fallback.is_empty() {
                Paint::None
            } else {
                self.paint(fallback)?
            };
            return Some(Paint::Server(id, Box::new(fallback)));
        }
        self.color(value, BLACK).map(Paint::Color)
    }
}

/// A marker position with its incoming and outgoing directions (degrees).
type Vertex = ((f64, f64), Option<f64>, Option<f64>);

/// A resolved gradient: its kind with geometry in gradient space, the
/// map from gradient space to CSS pixels, and its stops.
struct Gradient {
    radial: bool,
    /// Linear: `(x1, y1, x2, y2)`; radial: `(cx, cy, r, 0)`.
    geometry: (f64, f64, f64, f64),
    space: Affine,
    stops: Vec<(f64, Srgba)>,
}

impl Gradient {
    fn color_at(&self, t: f64) -> Srgba {
        let t = t.clamp(0.0, 1.0);
        let stops = &self.stops;
        let Some(first) = stops.first() else {
            return Srgba { a: 0, ..BLACK };
        };
        if t <= first.0 {
            return first.1;
        }
        for pair in stops.windows(2) {
            let ((t0, c0), (t1, c1)) = (pair[0], pair[1]);
            if t <= t1 {
                let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 1.0 };
                let mix =
                    |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * f).round() as u8;
                return Srgba {
                    r: mix(c0.r, c1.r),
                    g: mix(c0.g, c1.g),
                    b: mix(c0.b, c1.b),
                    a: mix(c0.a, c1.a),
                };
            }
        }
        stops.last().map_or(BLACK, |s| s.1)
    }
}

impl Painter<'_> {
    /// The gradient `id` names, for a shape with user-space bounds `bbox`
    /// drawn with `transform`; `None` for a missing or non-gradient
    /// element (its fallback paints) or a zero-size bounding box.
    fn gradient(
        &self,
        id: &str,
        bbox: Option<(f64, f64, f64, f64)>,
        transform: Affine,
    ) -> Option<Gradient> {
        let node = self.by_id(id)?;
        let element = svg_element(self.document, node)?;
        let radial = match element.name().local() {
            "linearGradient" => false,
            "radialGradient" => true,
            _ => return None,
        };
        // Attributes and stops come from the `href` chain when missing.
        let mut chain = vec![node];
        while chain.len() < MAX_USE_DEPTH {
            let last = svg_element(self.document, *chain.last()?)?;
            let Some(next) = href(last)
                .and_then(|h| h.strip_prefix('#').map(str::to_owned))
                .and_then(|id| self.by_id(&id))
            else {
                break;
            };
            if chain.contains(&next) {
                break;
            }
            chain.push(next);
        }
        let lookup = |name: &str| {
            chain
                .iter()
                .find_map(|&n| svg_element(self.document, n).and_then(|e| attribute(e, name)))
        };
        let bounding = lookup("gradientUnits").as_deref() != Some("userSpaceOnUse");
        let mut space = transform;
        let viewport = if bounding {
            let (x, y, w, h) = bbox?;
            if w <= 0.0 || h <= 0.0 {
                return None;
            }
            space = space.then(Affine([w, 0.0, 0.0, h, x, y]));
            (1.0, 1.0)
        } else {
            self.viewport
        };
        if let Some(t) = lookup("gradientTransform") {
            space = space.then(parse_transform(&t));
        }
        let get = |name: &str, axis: Axis, default: f64| {
            lookup(name)
                .and_then(|v| {
                    let v = v.trim();
                    match v.strip_suffix('%') {
                        Some(p) if bounding => p.trim().parse::<f64>().ok().map(|p| p / 100.0),
                        _ => length(v, axis, viewport),
                    }
                })
                .unwrap_or(default)
        };
        let geometry = if radial {
            let r = get("r", Axis::Other, 0.5);
            (get("cx", Axis::X, 0.5), get("cy", Axis::Y, 0.5), r, 0.0)
        } else {
            (
                get("x1", Axis::X, 0.0),
                get("y1", Axis::Y, 0.0),
                get("x2", Axis::X, if bounding { 1.0 } else { viewport.0 }),
                get("y2", Axis::Y, 0.0),
            )
        };
        let stops = chain
            .iter()
            .map(|&n| self.stops(n))
            .find(|s| !s.is_empty())
            .unwrap_or_default();
        Some(Gradient {
            radial,
            geometry,
            space,
            stops,
        })
    }

    fn stops(&self, gradient: NodeId) -> Vec<(f64, Srgba)> {
        let Ok(children) = self.document.children(gradient) else {
            return Vec::new();
        };
        let mut out: Vec<(f64, Srgba)> = Vec::new();
        for child in children {
            let Some(stop) =
                svg_element(self.document, child).filter(|e| e.name().local() == "stop")
            else {
                continue;
            };
            let mut color = BLACK;
            let mut alpha = 1.0;
            let mut offset = 0.0;
            let mut apply = |name: &str, value: &str| match name {
                "stop-color" => color = self.color(value.trim(), BLACK).unwrap_or(BLACK),
                "stop-opacity" => alpha = opacity(value.trim()).unwrap_or(1.0),
                "offset" => offset = opacity(value.trim()).unwrap_or(0.0),
                _ => {}
            };
            for name in ["offset", "stop-color", "stop-opacity"] {
                if let Some(value) = attribute(stop, name) {
                    apply(name, &value);
                }
            }
            if let Some(inline) = attribute(stop, "style") {
                for declaration in inline.split(';') {
                    if let Some((name, value)) = declaration.split_once(':') {
                        apply(name.trim(), value);
                    }
                }
            }
            // Offsets never decrease.
            let offset = out.last().map_or(offset, |last| offset.max(last.0));
            out.push((offset, with_alpha(color, alpha)));
        }
        out
    }

    /// Paint `contours` with `gradient`, band by band, each band the
    /// contours clipped to a convex slab or disc of one color.
    fn gradient_fill(
        &mut self,
        contours: &[Vec<(f64, f64)>],
        gradient: &Gradient,
        alpha: f64,
        even_odd: bool,
    ) {
        if gradient.stops.is_empty() {
            return;
        }
        if gradient.stops.len() == 1 {
            let color = with_alpha(gradient.stops[0].1, alpha);
            self.emit(contours.to_vec(), color, even_odd);
            return;
        }
        let space = gradient.space;
        let clipped = |region: &[(f64, f64)]| -> Vec<Vec<(f64, f64)>> {
            contours
                .iter()
                .map(|c| crate::rounded::clip_convex(c, region))
                .filter(|c| c.len() >= 3)
                .collect()
        };
        let n = GRADIENT_BANDS;
        if gradient.radial {
            let (cx, cy, r, _) = gradient.geometry;
            if r <= 0.0 {
                let color = gradient.stops.last().map_or(BLACK, |s| s.1);
                self.emit(contours.to_vec(), with_alpha(color, alpha), even_odd);
                return;
            }
            // The pad color first, then discs from the outside in.
            self.emit(
                contours.to_vec(),
                with_alpha(gradient.color_at(1.0), alpha),
                even_odd,
            );
            for k in (1..=n).rev() {
                let radius = r * k as f64 / n as f64;
                let color = with_alpha(gradient.color_at((k as f64 - 0.5) / n as f64), alpha);
                let disc: Vec<(f64, f64)> = (0..64)
                    .map(|i| {
                        let (s, c) = (std::f64::consts::TAU * f64::from(i) / 64.0).sin_cos();
                        space.apply((cx + radius * c, cy + radius * s))
                    })
                    .collect();
                let disc = positive(disc);
                self.emit(clipped(&disc), color, even_odd);
            }
            return;
        }
        let (x1, y1, x2, y2) = gradient.geometry;
        let (dx, dy) = (x2 - x1, y2 - y1);
        let length = (dx * dx + dy * dy).sqrt();
        if length <= 0.0 {
            let color = gradient.stops.last().map_or(BLACK, |s| s.1);
            self.emit(contours.to_vec(), with_alpha(color, alpha), even_odd);
            return;
        }
        // Far enough along the normal to cover any shape.
        let far = 1e4 * length.max(1.0);
        let (nx, ny) = (-dy / length * far, dx / length * far);
        let at = |t: f64| (x1 + dx * t, y1 + dy * t);
        let slab = |t0: f64, t1: f64| {
            let (a, b) = (at(t0), at(t1));
            positive(vec![
                space.apply((a.0 + nx, a.1 + ny)),
                space.apply((b.0 + nx, b.1 + ny)),
                space.apply((b.0 - nx, b.1 - ny)),
                space.apply((a.0 - nx, a.1 - ny)),
            ])
        };
        let pad = 1e4;
        // Two CSS pixels in `t`: how far an opaque band reaches under the
        // next one, so antialiased band edges never show what is below.
        let span = {
            let (a, b) = (space.apply(at(0.0)), space.apply(at(1.0)));
            ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
        };
        let overlap = if span > 0.0 { 2.0 / span } else { 0.0 };
        for k in 0..n {
            let (t0, t1) = (k as f64 / n as f64, (k + 1) as f64 / n as f64);
            let color = with_alpha(gradient.color_at((t0 + t1) / 2.0), alpha);
            // The ends pad out; opaque bands reach under the next ones.
            let from = if k == 0 { -pad } else { t0 };
            let to = if k + 1 == n {
                pad
            } else if color.a == 255 {
                t1 + overlap
            } else {
                t1
            };
            self.emit(clipped(&slab(from, to)), color, even_odd);
        }
    }

    /// Paint the markers of a shape's vertices (SVG 2 §11.6).
    fn markers(&mut self, subpaths: &[Subpath], transform: Affine, style: &Style) {
        if style.markers.iter().all(Option::is_none) || self.use_depth >= MAX_USE_DEPTH {
            return;
        }
        // Every command vertex with its incoming and outgoing directions.
        let mut vertices: Vec<Vertex> = Vec::new();
        for subpath in subpaths {
            let points = &subpath.points;
            let direction = |a: (f64, f64), b: (f64, f64)| {
                (a != b).then(|| (b.1 - a.1).atan2(b.0 - a.0).to_degrees())
            };
            for (k, &i) in subpath.vertices.iter().enumerate() {
                let p = points[i];
                let incoming = (i > 0)
                    .then(|| direction(points[i - 1], p))
                    .flatten()
                    .or_else(|| {
                        (subpath.closed && k == 0)
                            .then(|| points.last().and_then(|&l| direction(l, p)))
                            .flatten()
                    });
                let outgoing = points.get(i + 1).and_then(|&n| direction(p, n));
                vertices.push((p, incoming, outgoing));
            }
        }
        let last = vertices.len().saturating_sub(1);
        for (k, &(p, incoming, outgoing)) in vertices.iter().enumerate() {
            let (which, reverse) = match k {
                0 => (0, true),
                k if k == last => (2, false),
                _ => (1, false),
            };
            let Some(id) = style.markers[which].clone() else {
                continue;
            };
            let angle = match (incoming, outgoing) {
                (Some(a), Some(b)) => {
                    let mut d = b - a;
                    if d > 180.0 {
                        d -= 360.0;
                    } else if d < -180.0 {
                        d += 360.0;
                    }
                    a + d / 2.0
                }
                (Some(a), None) | (None, Some(a)) => a,
                (None, None) => 0.0,
            };
            self.marker(&id, p, angle, which == 0 && reverse, transform, style);
        }
    }

    fn marker(
        &mut self,
        id: &str,
        at: (f64, f64),
        auto_angle: f64,
        start: bool,
        transform: Affine,
        style: &Style,
    ) {
        let Some(node) = self.by_id(id) else {
            return;
        };
        let Some(marker) =
            svg_element(self.document, node).filter(|e| e.name().local() == "marker")
        else {
            return;
        };
        let vp = self.viewport;
        let get = |name: &str, default: f64| {
            attribute(marker, name)
                .and_then(|v| length(&v, Axis::Other, vp))
                .unwrap_or(default)
        };
        let (w, h) = (get("markerWidth", 3.0), get("markerHeight", 3.0));
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let angle = match attribute(marker, "orient").as_deref().map(str::trim) {
            Some("auto") => auto_angle,
            Some("auto-start-reverse") if start => auto_angle + 180.0,
            Some("auto-start-reverse") => auto_angle,
            Some(value) => numbers(value).first().copied().unwrap_or(0.0),
            None => 0.0,
        };
        let units = if attribute(marker, "markerUnits").as_deref() == Some("userSpaceOnUse") {
            1.0
        } else {
            style.stroke_width
        };
        let inner = match attribute(marker, "viewBox").and_then(|v| view_box(&v)) {
            Some(view_box) => fit(
                view_box,
                (w, h),
                &attribute(marker, "preserveAspectRatio").unwrap_or_default(),
            ),
            None => Affine::IDENTITY,
        };
        let reference = inner.apply((get("refX", 0.0), get("refY", 0.0)));
        let space = transform
            .then(Affine::translate(at.0, at.1))
            .then(Affine::rotation(angle))
            .then(Affine::scaling(units, units))
            .then(Affine::translate(-reference.0, -reference.1))
            .then(inner);
        // Marker contents inherit from the marker, not the shape.
        let Some((marker_style, _)) = self.declared(marker, Style::initial(style.color)) else {
            return;
        };
        self.use_depth += 1;
        self.children(node, space, &marker_style);
        self.use_depth -= 1;
    }
}

fn with_alpha(color: Srgba, alpha: f64) -> Srgba {
    Srgba {
        a: (f64::from(color.a) * alpha.clamp(0.0, 1.0)).round() as u8,
        ..color
    }
}

fn opacity(value: &str) -> Option<f64> {
    let value = value.trim();
    let n = match value.strip_suffix('%') {
        Some(p) => p.trim().parse::<f64>().ok()? / 100.0,
        None => value.parse::<f64>().ok()?,
    };
    n.is_finite().then(|| n.clamp(0.0, 1.0))
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
    Other,
}

/// An SVG length: a number with an optional absolute unit, or a
/// percentage of the viewport's width, height or normalized diagonal.
fn length(value: &str, axis: Axis, viewport: (f64, f64)) -> Option<f64> {
    let value = value.trim();
    let split = value
        .find(|c: char| c.is_ascii_alphabetic() || c == '%')
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let n: f64 = number.trim().parse().ok().filter(|n: &f64| n.is_finite())?;
    let factor = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "px" => 1.0,
        "in" => 96.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        "pt" => 96.0 / 72.0,
        "pc" => 16.0,
        "em" => 16.0,
        "ex" => 8.0,
        "%" => {
            let (w, h) = viewport;
            let base = match axis {
                Axis::X => w,
                Axis::Y => h,
                Axis::Other => ((w * w + h * h) / 2.0).sqrt(),
            };
            base / 100.0
        }
        _ => return None,
    };
    Some(n * factor)
}

/// The numbers of a list separated by whitespace and commas (also the
/// compact forms `1-2` and `.5.5`).
fn numbers(value: &str) -> Vec<f64> {
    let mut scanner = Scanner::new(value);
    let mut out = Vec::new();
    while let Some(n) = scanner.number() {
        out.push(n);
        if out.len() >= 1_000_000 {
            break;
        }
    }
    out
}

struct Scanner<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Scanner<'a> {
    fn new(value: &'a str) -> Self {
        Self {
            bytes: value.as_bytes(),
            at: 0,
        }
    }

    fn skip_separators(&mut self) {
        while let Some(&b) = self.bytes.get(self.at) {
            if b.is_ascii_whitespace() || b == b',' {
                self.at += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_separators();
        self.bytes.get(self.at).copied()
    }

    fn number(&mut self) -> Option<f64> {
        self.skip_separators();
        let start = self.at;
        let mut end = start;
        let b = self.bytes;
        if matches!(b.get(end), Some(b'+' | b'-')) {
            end += 1;
        }
        let digits = |mut i: usize| {
            while b.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            i
        };
        let whole_end = digits(end);
        let mut fraction = false;
        let mut stop = whole_end;
        if b.get(whole_end) == Some(&b'.') {
            let f = digits(whole_end + 1);
            fraction = f > whole_end + 1;
            stop = f;
        }
        if whole_end == end && !fraction {
            return None;
        }
        if matches!(b.get(stop), Some(b'e' | b'E')) {
            let mut e = stop + 1;
            if matches!(b.get(e), Some(b'+' | b'-')) {
                e += 1;
            }
            let ee = digits(e);
            if ee > e {
                stop = ee;
            }
        }
        let text = std::str::from_utf8(&b[start..stop]).ok()?;
        let n: f64 = text.parse().ok()?;
        self.at = stop;
        n.is_finite().then_some(n)
    }

    /// An arc flag: one `0` or `1`, which needs no separator after it.
    fn flag(&mut self) -> Option<bool> {
        match self.peek()? {
            b'0' => {
                self.at += 1;
                Some(false)
            }
            b'1' => {
                self.at += 1;
                Some(true)
            }
            _ => None,
        }
    }
}

/// A flattened subpath in user space.
#[derive(Clone, Debug, PartialEq)]
struct Subpath {
    points: Vec<(f64, f64)>,
    closed: bool,
    /// Indices of the points that end a command (marker positions).
    vertices: Vec<usize>,
}

impl Subpath {
    /// A subpath whose every point is a vertex.
    fn polyline(points: Vec<(f64, f64)>, closed: bool) -> Self {
        let vertices = (0..points.len()).collect();
        Self {
            points,
            closed,
            vertices,
        }
    }

    /// A closed shape with markers only at its start (basic shapes take
    /// no markers, but this keeps them well-formed).
    fn shape(points: Vec<(f64, f64)>) -> Self {
        Self {
            points,
            closed: true,
            vertices: vec![0],
        }
    }
}

/// Segments for a curve whose control polygon is `length` user units long
/// at `scale` CSS pixels per unit.
fn segments(length: f64, scale: f64) -> usize {
    ((length * scale).sqrt() * 1.5).ceil().clamp(2.0, 128.0) as usize
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// The path data grammar (SVG 2 §9.3.9); parsing stops at the first error,
/// keeping what came before it.
fn parse_path(d: &str, scale: f64) -> Vec<Subpath> {
    let mut s = Scanner::new(d);
    let mut out: Vec<Subpath> = Vec::new();
    let mut current = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    let mut last_control: Option<(f64, f64)> = None;
    let mut last_quad: Option<(f64, f64)> = None;
    let mut command = None;
    let mut points: Vec<(f64, f64)> = Vec::new();
    let mut vertices: Vec<usize> = Vec::new();
    let flush = |points: &mut Vec<(f64, f64)>,
                 vertices: &mut Vec<usize>,
                 out: &mut Vec<Subpath>,
                 closed: bool| {
        if points.len() >= 2 || (closed && !points.is_empty()) {
            out.push(Subpath {
                points: std::mem::take(points),
                closed,
                vertices: std::mem::take(vertices),
            });
        } else {
            points.clear();
            vertices.clear();
        }
    };
    while let Some(next) = s.peek() {
        let letter = if next.is_ascii_alphabetic() {
            s.at += 1;
            next
        } else {
            match command {
                // Implicit repeats; a moveto repeats as a lineto.
                Some(b'M') => b'L',
                Some(b'm') => b'l',
                Some(c) if c != b'Z' && c != b'z' => c,
                _ => break,
            }
        };
        command = Some(letter);
        let relative = letter.is_ascii_lowercase();
        let base = if relative { current } else { (0.0, 0.0) };
        let pair = |s: &mut Scanner<'_>| -> Option<(f64, f64)> {
            let x = s.number()?;
            let y = s.number()?;
            Some((base.0 + x, base.1 + y))
        };
        let mut control = None;
        let mut quad = None;
        match letter.to_ascii_uppercase() {
            b'M' => {
                let Some(p) = pair(&mut s) else { break };
                flush(&mut points, &mut vertices, &mut out, false);
                points.push(p);
                current = p;
                start = p;
            }
            b'L' => {
                let Some(p) = pair(&mut s) else { break };
                if points.is_empty() {
                    points.push(current);
                }
                points.push(p);
                current = p;
            }
            b'H' => {
                let Some(x) = s.number() else { break };
                let p = (if relative { current.0 + x } else { x }, current.1);
                if points.is_empty() {
                    points.push(current);
                }
                points.push(p);
                current = p;
            }
            b'V' => {
                let Some(y) = s.number() else { break };
                let p = (current.0, if relative { current.1 + y } else { y });
                if points.is_empty() {
                    points.push(current);
                }
                points.push(p);
                current = p;
            }
            b'C' | b'S' => {
                let c1 = if letter.eq_ignore_ascii_case(&b'C') {
                    let Some(c1) = pair(&mut s) else { break };
                    c1
                } else {
                    last_control.map_or(current, |c| (2.0 * current.0 - c.0, 2.0 * current.1 - c.1))
                };
                let (Some(c2), Some(p)) = (pair(&mut s), pair(&mut s)) else {
                    break;
                };
                if points.is_empty() {
                    points.push(current);
                }
                let n = segments(
                    distance(current, c1) + distance(c1, c2) + distance(c2, p),
                    scale,
                );
                for k in 1..=n {
                    let t = k as f64 / n as f64;
                    let u = 1.0 - t;
                    let (a, b, c, e) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    points.push((
                        a * current.0 + b * c1.0 + c * c2.0 + e * p.0,
                        a * current.1 + b * c1.1 + c * c2.1 + e * p.1,
                    ));
                }
                control = Some(c2);
                current = p;
            }
            b'Q' | b'T' => {
                let c = if letter.eq_ignore_ascii_case(&b'Q') {
                    let Some(c) = pair(&mut s) else { break };
                    c
                } else {
                    last_quad.map_or(current, |q| (2.0 * current.0 - q.0, 2.0 * current.1 - q.1))
                };
                let Some(p) = pair(&mut s) else { break };
                if points.is_empty() {
                    points.push(current);
                }
                let n = segments(distance(current, c) + distance(c, p), scale);
                for k in 1..=n {
                    let t = k as f64 / n as f64;
                    let u = 1.0 - t;
                    points.push((
                        u * u * current.0 + 2.0 * u * t * c.0 + t * t * p.0,
                        u * u * current.1 + 2.0 * u * t * c.1 + t * t * p.1,
                    ));
                }
                quad = Some(c);
                current = p;
            }
            b'A' => {
                let (Some(rx), Some(ry), Some(angle)) = (s.number(), s.number(), s.number()) else {
                    break;
                };
                let (Some(large), Some(sweep)) = (s.flag(), s.flag()) else {
                    break;
                };
                let Some(p) = pair(&mut s) else { break };
                if points.is_empty() {
                    points.push(current);
                }
                arc(
                    &mut points,
                    current,
                    p,
                    (rx, ry),
                    angle,
                    large,
                    sweep,
                    scale,
                );
                current = p;
            }
            b'Z' => {
                if !points.is_empty() {
                    flush(&mut points, &mut vertices, &mut out, true);
                }
                current = start;
            }
            _ => break,
        }
        if !points.is_empty() && !matches!(letter, b'Z' | b'z') {
            if vertices.is_empty() {
                vertices.push(0);
            }
            let end = points.len() - 1;
            if vertices.last() != Some(&end) {
                vertices.push(end);
            }
        }
        last_control = control;
        last_quad = quad;
    }
    flush(&mut points, &mut vertices, &mut out, false);
    out
}

/// Flatten an elliptical arc (SVG 2 §9.5 endpoint-to-center conversion)
/// into `points`, ending at `to`.
#[allow(clippy::too_many_arguments, reason = "the arc command's parameters")]
fn arc(
    points: &mut Vec<(f64, f64)>,
    from: (f64, f64),
    to: (f64, f64),
    (rx, ry): (f64, f64),
    angle: f64,
    large: bool,
    sweep: bool,
    scale: f64,
) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if from == to {
        return;
    }
    if rx == 0.0 || ry == 0.0 {
        points.push(to);
        return;
    }
    let (sin, cos) = angle.to_radians().sin_cos();
    let (dx, dy) = ((from.0 - to.0) / 2.0, (from.1 - to.1) / 2.0);
    let (x1, y1) = (cos * dx + sin * dy, -sin * dx + cos * dy);
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let k = lambda.sqrt();
        rx *= k;
        ry *= k;
    }
    let numerator = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let denominator = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut root = if denominator > 0.0 {
        (numerator / denominator).max(0.0).sqrt()
    } else {
        0.0
    };
    if large == sweep {
        root = -root;
    }
    let (cx1, cy1) = (root * rx * y1 / ry, -root * ry * x1 / rx);
    let (cx, cy) = (
        cos * cx1 - sin * cy1 + (from.0 + to.0) / 2.0,
        sin * cx1 + cos * cy1 + (from.1 + to.1) / 2.0,
    );
    let vector_angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 { -a } else { a }
    };
    let theta = vector_angle(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut delta = vector_angle(
        (x1 - cx1) / rx,
        (y1 - cy1) / ry,
        (-x1 - cx1) / rx,
        (-y1 - cy1) / ry,
    );
    let tau = std::f64::consts::TAU;
    if !sweep && delta > 0.0 {
        delta -= tau;
    } else if sweep && delta < 0.0 {
        delta += tau;
    }
    let n =
        segments(delta.abs() * rx.max(ry), scale).max((delta.abs() / tau * 32.0).ceil() as usize);
    for k in 1..=n {
        let t = theta + delta * k as f64 / n as f64;
        let (s, c) = t.sin_cos();
        let (x, y) = (rx * c, ry * s);
        points.push(if k == n {
            to
        } else {
            (cos * x - sin * y + cx, sin * x + cos * y + cy)
        });
    }
}

fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64) -> Subpath {
    let n = 64;
    Subpath::shape(
        (0..n)
            .map(|k| {
                let (s, c) = (std::f64::consts::TAU * k as f64 / n as f64).sin_cos();
                (cx + rx * c, cy + ry * s)
            })
            .collect(),
    )
}

fn rounded_rect(x: f64, y: f64, w: f64, h: f64, rx: f64, ry: f64) -> Subpath {
    if rx <= 0.0 || ry <= 0.0 {
        return Subpath::shape(vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)]);
    }
    let mut points = Vec::new();
    let corners = [
        (x + w - rx, y + ry, -0.25),
        (x + w - rx, y + h - ry, 0.0),
        (x + rx, y + h - ry, 0.25),
        (x + rx, y + ry, 0.5),
    ];
    for (cx, cy, start) in corners {
        for k in 0..=8 {
            let t = std::f64::consts::TAU * (start + 0.25 * k as f64 / 8.0);
            let (s, c) = t.sin_cos();
            points.push((cx + rx * c, cy + ry * s));
        }
    }
    Subpath::shape(points)
}

/// Reverse a polygon wound negatively, so every stroke piece adds.
fn positive(mut polygon: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    let area: f64 = polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .map(|(a, b)| a.0 * b.1 - b.0 * a.1)
        .sum();
    if area < 0.0 {
        polygon.reverse();
    }
    polygon
}

fn disc(center: (f64, f64), r: f64, scale: f64) -> Vec<(f64, f64)> {
    let n = segments(r * 6.0, scale).max(8);
    (0..n)
        .map(|k| {
            let (s, c) = (std::f64::consts::TAU * k as f64 / n as f64).sin_cos();
            (center.0 + r * c, center.1 + r * s)
        })
        .collect()
}

/// The stroke of one subpath as polygons in user space, dashed when the
/// style has dashes.
fn stroke(subpath: &Subpath, style: &Style, scale: f64, out: &mut Vec<Vec<(f64, f64)>>) {
    match &style.dashes {
        Some(dashes) => {
            for dash in dash(subpath, dashes, style.dash_offset) {
                stroke_solid(&dash, style, scale, out);
            }
        }
        None => stroke_solid(subpath, style, scale, out),
    }
}

/// The dashes of a subpath (SVG 2 §13.5.7): open subpaths along it,
/// starting `offset` into the repeating `pattern`.
fn dash(subpath: &Subpath, pattern: &[f64], offset: f64) -> Vec<Subpath> {
    let total: f64 = pattern.iter().sum();
    let mut points = subpath.points.clone();
    if subpath.closed {
        if let Some(&first) = points.first() {
            points.push(first);
        }
    }
    // Where in the pattern the subpath starts.
    let mut index = 0;
    let mut left = pattern[0];
    let mut skip = offset.rem_euclid(total);
    while skip > 0.0 {
        if skip >= left {
            skip -= left;
            index = (index + 1) % pattern.len();
            left = pattern[index];
        } else {
            left -= skip;
            skip = 0.0;
        }
    }
    let mut out = Vec::new();
    let mut current: Vec<(f64, f64)> = Vec::new();
    let on = |i: usize| i % 2 == 0;
    if on(index) {
        if let Some(&first) = points.first() {
            current.push(first);
        }
    }
    for pair in points.windows(2) {
        let (mut a, b) = (pair[0], pair[1]);
        let mut remaining = distance(a, b);
        while remaining > 0.0 {
            if remaining < left {
                left -= remaining;
                if on(index) {
                    current.push(b);
                }
                remaining = 0.0;
            } else {
                let f = left / distance(a, b).max(f64::MIN_POSITIVE);
                let p = (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
                remaining -= left;
                a = p;
                if on(index) {
                    current.push(p);
                    out.push(Subpath::polyline(std::mem::take(&mut current), false));
                } else {
                    current = vec![p];
                }
                index = (index + 1) % pattern.len();
                left = pattern[index];
                if out.len() > 100_000 {
                    return out;
                }
            }
        }
    }
    if on(index) && current.len() >= 2 {
        out.push(Subpath::polyline(current, false));
    }
    out
}

fn stroke_solid(subpath: &Subpath, style: &Style, scale: f64, out: &mut Vec<Vec<(f64, f64)>>) {
    let half = style.stroke_width / 2.0;
    let mut points: Vec<(f64, f64)> = Vec::with_capacity(subpath.points.len());
    for &p in &subpath.points {
        if points.last() != Some(&p) {
            points.push(p);
        }
    }
    if subpath.closed && points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    if points.len() < 2 {
        // A zero-length subpath shows round and square caps.
        if let Some(&p) = points.first() {
            match style.cap {
                Cap::Round => out.push(disc(p, half, scale)),
                Cap::Square => out.push(vec![
                    (p.0 - half, p.1 - half),
                    (p.0 + half, p.1 - half),
                    (p.0 + half, p.1 + half),
                    (p.0 - half, p.1 + half),
                ]),
                Cap::Butt => {}
            }
        }
        return;
    }
    let n = points.len();
    let segment_count = if subpath.closed { n } else { n - 1 };
    let normal = |a: (f64, f64), b: (f64, f64)| {
        let len = distance(a, b);
        (-(b.1 - a.1) / len * half, (b.0 - a.0) / len * half)
    };
    for i in 0..segment_count {
        let (mut a, mut b) = (points[i], points[(i + 1) % n]);
        let (nx, ny) = normal(a, b);
        if !subpath.closed && style.cap == Cap::Square {
            let (tx, ty) = (ny, -nx);
            if i == 0 {
                a = (a.0 - tx, a.1 - ty);
            }
            if i + 1 == segment_count {
                b = (b.0 + tx, b.1 + ty);
            }
        }
        out.push(vec![
            (a.0 + nx, a.1 + ny),
            (b.0 + nx, b.1 + ny),
            (b.0 - nx, b.1 - ny),
            (a.0 - nx, a.1 - ny),
        ]);
    }
    // Joins at interior vertices (every vertex of a closed subpath).
    let joins: Box<dyn Iterator<Item = usize>> = if subpath.closed {
        Box::new(0..n)
    } else {
        Box::new(1..n - 1)
    };
    for i in joins {
        let (prev, at, next) = (points[(i + n - 1) % n], points[i], points[(i + 1) % n]);
        match style.join {
            Join::Round => out.push(disc(at, half, scale)),
            Join::Bevel | Join::Miter => {
                let (n1, n2) = (normal(prev, at), normal(at, next));
                let cross = (at.0 - prev.0) * (next.1 - at.1) - (at.1 - prev.1) * (next.0 - at.0);
                // The outer side of the turn.
                let sign = if cross > 0.0 { -1.0 } else { 1.0 };
                let o1 = (at.0 + sign * n1.0, at.1 + sign * n1.1);
                let o2 = (at.0 + sign * n2.0, at.1 + sign * n2.1);
                let mut polygon = vec![at, o1];
                if style.join == Join::Miter {
                    // The miter tip: where the two outer edges meet.
                    let (d1, d2) = (
                        (at.0 - prev.0, at.1 - prev.1),
                        (next.0 - at.0, next.1 - at.1),
                    );
                    let det = d1.0 * d2.1 - d1.1 * d2.0;
                    if det.abs() > 1e-12 {
                        let t = ((o2.0 - o1.0) * d2.1 - (o2.1 - o1.1) * d2.0) / det;
                        let tip = (o1.0 + d1.0 * t, o1.1 + d1.1 * t);
                        if distance(tip, at) <= style.miter_limit * half {
                            polygon.push(tip);
                        }
                    }
                }
                polygon.push(o2);
                out.push(polygon);
            }
        }
    }
    if !subpath.closed && style.cap == Cap::Round {
        out.push(disc(points[0], half, scale));
        out.push(disc(points[n - 1], half, scale));
    }
}

/// The `transform` attribute's list of functions (SVG 2 §8.5); an invalid
/// list is the identity.
fn parse_transform(value: &str) -> Affine {
    let mut result = Affine::IDENTITY;
    let mut rest = value.trim();
    while !rest.is_empty() {
        let Some(open) = rest.find('(') else {
            return Affine::IDENTITY;
        };
        let Some(close) = rest.find(')') else {
            return Affine::IDENTITY;
        };
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let n = numbers(&rest[open + 1..close]);
        let m = match (name, n.as_slice()) {
            ("matrix", [a, b, c, d, e, f]) => Affine([*a, *b, *c, *d, *e, *f]),
            ("translate", [x]) => Affine::translate(*x, 0.0),
            ("translate", [x, y]) => Affine::translate(*x, *y),
            ("scale", [s]) => Affine([*s, 0.0, 0.0, *s, 0.0, 0.0]),
            ("scale", [x, y]) => Affine([*x, 0.0, 0.0, *y, 0.0, 0.0]),
            ("rotate", [a, rest @ ..]) if rest.is_empty() || rest.len() == 2 => {
                let (s, c) = a.to_radians().sin_cos();
                let r = Affine([c, s, -s, c, 0.0, 0.0]);
                match rest {
                    [x, y] => Affine::translate(*x, *y)
                        .then(r)
                        .then(Affine::translate(-x, -y)),
                    _ => r,
                }
            }
            ("skewX", [a]) => Affine([1.0, 0.0, a.to_radians().tan(), 1.0, 0.0, 0.0]),
            ("skewY", [a]) => Affine([1.0, a.to_radians().tan(), 0.0, 1.0, 0.0, 0.0]),
            _ => return Affine::IDENTITY,
        };
        result = result.then(m);
        rest = rest[close + 1..].trim_start_matches(|c: char| c.is_whitespace() || c == ',');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_data_parses_every_command_and_compact_forms() {
        let p = parse_path("M10-20L.5.5h10v-5zm1,1 l2 2", 1.0);
        assert_eq!(p.len(), 2);
        assert_eq!(
            p[0].points,
            [(10.0, -20.0), (0.5, 0.5), (10.5, 0.5), (10.5, -4.5)]
        );
        assert!(p[0].closed);
        // After `z` the current point is the subpath start; `m` is relative to it.
        assert_eq!(p[1].points, [(11.0, -19.0), (13.0, -17.0)]);
        // Arcs end exactly on their endpoint; flags need no separators.
        let a = parse_path("M0 0a10 10 0 1110 10", 1.0);
        assert_eq!(*a[0].points.last().unwrap(), (10.0, 10.0));
        // Curves end on their endpoints; parsing stops at an error.
        let c = parse_path("M0 0C1 1 2 2 3 3Q4 4 5 5T7 7S9 9 10 10 L x", 1.0);
        assert_eq!(*c[0].points.last().unwrap(), (10.0, 10.0));
    }

    #[test]
    fn transforms_compose_left_to_right() {
        let t = parse_transform("translate(10 20) scale(2)");
        assert_eq!(t.apply((1.0, 1.0)), (12.0, 22.0));
        let r = parse_transform("rotate(90 5 5)");
        let (x, y) = r.apply((10.0, 5.0));
        assert!((x - 5.0).abs() < 1e-9 && (y - 10.0).abs() < 1e-9);
        assert_eq!(parse_transform("bogus(1)"), Affine::IDENTITY);
    }

    #[test]
    fn lengths_take_absolute_units_and_percentages() {
        assert_eq!(length("1in", Axis::X, (0.0, 0.0)), Some(96.0));
        assert_eq!(length("50%", Axis::Y, (10.0, 40.0)), Some(20.0));
        assert_eq!(length("10q", Axis::X, (0.0, 0.0)), None);
    }

    #[test]
    fn strokes_are_positively_wound_pieces() {
        let style = Style {
            stroke: Paint::Current,
            stroke_width: 2.0,
            ..Style::initial(Srgba::WHITE)
        };
        let mut out = Vec::new();
        let path = Subpath::polyline(vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], false);
        stroke(&path, &style, 1.0, &mut out);
        // Two segment quads and one miter join reaching the outer corner.
        assert_eq!(out.len(), 3);
        assert!(
            out[2]
                .iter()
                .any(|&(x, y)| (x - 11.0).abs() < 1e-9 && (y + 1.0).abs() < 1e-9)
        );
    }

    #[test]
    fn dashes_split_a_path_from_their_offset() {
        let line = Subpath::polyline(vec![(0.0, 0.0), (10.0, 0.0)], false);
        let dashes = dash(&line, &[2.0, 3.0], 0.0);
        let starts: Vec<f64> = dashes.iter().map(|d| d.points[0].0).collect();
        assert_eq!(starts, [0.0, 5.0]);
        assert_eq!(dashes[1].points.last().unwrap().0, 7.0);
        // An offset of 4 starts one unit before the next dash.
        let shifted = dash(&line, &[2.0, 3.0], 4.0);
        assert_eq!(shifted[0].points[0].0, 1.0);
    }

    #[test]
    fn path_commands_mark_vertices_for_markers() {
        let p = parse_path("M0 0L10 0Q15 5 10 10Z", 1.0);
        assert_eq!(p[0].vertices.len(), 3);
        assert_eq!(p[0].points[*p[0].vertices.last().unwrap()], (10.0, 10.0));
    }

    #[test]
    fn gradient_colors_interpolate_between_stops() {
        let g = Gradient {
            radial: false,
            geometry: (0.0, 0.0, 1.0, 0.0),
            space: Affine::IDENTITY,
            stops: vec![(0.25, BLACK), (0.75, Srgba::WHITE)],
        };
        assert_eq!(g.color_at(0.0), BLACK);
        assert_eq!(g.color_at(0.5).r, 128);
        assert_eq!(g.color_at(1.0), Srgba::WHITE);
    }
}
