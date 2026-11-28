//! Inline SVG painting (SVG 2): the shapes of an `<svg>` element's
//! subtree turned into filled polygons in CSS pixels, painted in its
//! content box.
//!
//! Supported: nested `svg`, `g`, `use` (of an element by id), `rect`
//! (with `rx`/`ry`), `circle`, `ellipse`, `line`, `polyline`, `polygon`
//! and `path` (every command, arcs included); `viewBox` with
//! `preserveAspectRatio`; `transform`; the `fill`, `stroke`,
//! `stroke-width`, `stroke-linecap`, `stroke-linejoin`,
//! `stroke-miterlimit`, `fill-opacity`, `stroke-opacity`, `opacity`,
//! `color`, `display` and `visibility` presentation attributes and the
//! same properties in `style` attributes. Fills use the nonzero rule.
//! Gradients and patterns paint their first stop's color; text, markers,
//! clipping, masks, filters, dashes, `fill-rule: evenodd` and author style
//! sheets are not applied (TD-220).
//!
//! Curves and arcs are flattened to polylines in user space; strokes are
//! built in user space (so transforms scale them as SVG requires) as a
//! union of positively wound quads, joins and caps, which the rasterizer's
//! clamped nonzero coverage merges without seams.

use rustic_css::{Color, CssParser};
use rustic_dom::{Document, Element, NodeId, NodeKind, SVG_NAMESPACE};
use rustic_layout::CssRect;

use crate::color::{Srgba, srgba};

/// One filled area: contours in CSS pixels and its color.
pub(crate) struct Painted {
    pub(crate) contours: Vec<Vec<(f64, f64)>>,
    pub(crate) color: Srgba,
}

/// Elements visited and polygon points produced, at most.
const MAX_ELEMENTS: usize = 50_000;
const MAX_POINTS: usize = 2_000_000;
/// `use` references followed inside one another, at most.
const MAX_USE_DEPTH: usize = 8;

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
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Paint {
    None,
    Color(Srgba),
    Current,
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

/// The inherited painting properties, plus `opacity` folded down the tree.
#[derive(Clone, Copy, Debug)]
struct Style {
    fill: Paint,
    stroke: Paint,
    fill_opacity: f64,
    stroke_opacity: f64,
    stroke_width: f64,
    cap: Cap,
    join: Join,
    miter_limit: f64,
    color: Srgba,
    visible: bool,
    opacity: f64,
}

struct Painter<'a> {
    document: &'a Document,
    parser: CssParser,
    out: Vec<Painted>,
    elements: usize,
    points: usize,
    /// The nearest viewport's size, for percentages.
    viewport: (f64, f64),
    use_depth: usize,
}

/// The painted areas of the `svg` element `root`, mapped into `content`
/// (its content box), with `color` as its `currentColor`.
pub(crate) fn paint(
    document: &Document,
    root: NodeId,
    content: CssRect,
    color: Srgba,
) -> Vec<Painted> {
    let Some(element) = svg_element(document, root) else {
        return Vec::new();
    };
    let (x, y) = (content.origin().x.get(), content.origin().y.get());
    let (w, h) = (content.size().width().get(), content.size().height().get());
    let mut painter = Painter {
        document,
        parser: CssParser::default(),
        out: Vec::new(),
        elements: 0,
        points: 0,
        viewport: (w, h),
        use_depth: 0,
    };
    let style = Style {
        fill: Paint::Color(Srgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        }),
        stroke: Paint::None,
        fill_opacity: 1.0,
        stroke_opacity: 1.0,
        stroke_width: 1.0,
        cap: Cap::Butt,
        join: Join::Miter,
        miter_limit: 4.0,
        color,
        visible: true,
        opacity: 1.0,
    };
    let base = Affine::translate(x, y);
    let style = painter.style(element, style);
    if let Some(style) = style {
        let (inner, size) = viewport_transform(element, (w, h));
        painter.viewport = size;
        painter.children(root, base.then(inner), style);
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

/// The `href` (or `xlink:href`) of a `use`.
fn href(element: &Element) -> Option<String> {
    element
        .attributes()
        .iter()
        .find(|a| a.name().local() == "href")
        .map(|a| String::from_utf16_lossy(a.value().as_units()))
}

/// The transform from an `svg` element's user space to its viewport of
/// `size`, and the size percentages resolve against.
fn viewport_transform(element: &Element, size: (f64, f64)) -> (Affine, (f64, f64)) {
    let Some(view_box) = attribute(element, "viewBox").and_then(|v| {
        let n = numbers(&v);
        (n.len() == 4 && n[2] > 0.0 && n[3] > 0.0).then(|| (n[0], n[1], n[2], n[3]))
    }) else {
        return (Affine::IDENTITY, size);
    };
    let (vx, vy, vw, vh) = view_box;
    let (w, h) = size;
    let aspect = attribute(element, "preserveAspectRatio").unwrap_or_default();
    let mut words = aspect.split_ascii_whitespace();
    let align = words.next().unwrap_or("xMidYMid");
    let slice = words.next() == Some("slice");
    let (sx, sy) = (w / vw, h / vh);
    if align == "none" {
        return (Affine([sx, 0.0, 0.0, sy, -vx * sx, -vy * sy]), (vw, vh));
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
    (Affine([s, 0.0, 0.0, s, tx - vx * s, ty - vy * s]), (vw, vh))
}

impl Painter<'_> {
    fn children(&mut self, parent: NodeId, transform: Affine, style: Style) {
        let Ok(children) = self.document.children(parent) else {
            return;
        };
        let children: Vec<NodeId> = children.collect();
        for child in children {
            self.element(child, transform, style);
        }
    }

    fn element(&mut self, id: NodeId, transform: Affine, style: Style) {
        self.elements += 1;
        if self.elements > MAX_ELEMENTS || self.points > MAX_POINTS {
            return;
        }
        let Some(element) = svg_element(self.document, id) else {
            return;
        };
        let Some(style) = self.style(element, style) else {
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
                self.children(id, transform, style);
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
                self.children(
                    id,
                    transform.then(Affine::translate(x, y)).then(inner),
                    style,
                );
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
                if svg_element(self.document, target).is_some_and(|t| t.name().local() == "symbol")
                {
                    // A symbol renders as a group of its children.
                    if let Some(symbol) = svg_element(self.document, target) {
                        if let Some(style) = self.style(symbol, style) {
                            let (inner, _) = attribute(element, "width")
                                .zip(attribute(element, "height"))
                                .and_then(|(w, h)| {
                                    Some((length(&w, Axis::X, vp)?, length(&h, Axis::Y, vp)?))
                                })
                                .map_or((Affine::IDENTITY, vp), |size| {
                                    viewport_transform(symbol, size)
                                });
                            self.children(target, transform.then(inner), style);
                        }
                    }
                } else {
                    self.element(target, transform, style);
                }
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
            "line" => vec![Subpath {
                points: vec![
                    (number("x1", Axis::X, vp), number("y1", Axis::Y, vp)),
                    (number("x2", Axis::X, vp), number("y2", Axis::Y, vp)),
                ],
                closed: false,
            }],
            "polyline" | "polygon" => {
                let n = attribute(element, "points")
                    .map(|p| numbers(&p))
                    .unwrap_or_default();
                let points: Vec<(f64, f64)> = n.chunks_exact(2).map(|p| (p[0], p[1])).collect();
                if points.is_empty() {
                    return;
                }
                vec![Subpath {
                    points,
                    closed: local == "polygon",
                }]
            }
            "path" => match attribute(element, "d") {
                Some(d) => parse_path(&d, transform.scale()),
                None => return,
            },
            _ => return,
        };
        if !style.visible {
            return;
        }
        // `line`, `polyline` and open path parts are not filled.
        let fill_color = resolve(style.fill, style.color, style.fill_opacity * style.opacity);
        if let Some(color) = fill_color {
            if local != "line" {
                let contours: Vec<Vec<(f64, f64)>> = subpaths
                    .iter()
                    .filter(|s| s.points.len() >= 3)
                    .map(|s| s.points.iter().map(|&p| transform.apply(p)).collect())
                    .collect();
                self.emit(contours, color);
            }
        }
        let stroke_color = resolve(
            style.stroke,
            style.color,
            style.stroke_opacity * style.opacity,
        );
        if let (Some(color), true) = (stroke_color, style.stroke_width > 0.0) {
            let mut contours = Vec::new();
            for subpath in &subpaths {
                stroke(subpath, &style, transform.scale(), &mut contours);
            }
            let contours = contours
                .into_iter()
                .map(|c| positive(c.into_iter().map(|p| transform.apply(p)).collect()))
                .collect();
            self.emit(contours, color);
        }
    }

    fn emit(&mut self, contours: Vec<Vec<(f64, f64)>>, color: Srgba) {
        if contours.is_empty() || color.is_transparent() {
            return;
        }
        self.points += contours.iter().map(Vec::len).sum::<usize>();
        if self.points > MAX_POINTS {
            return;
        }
        self.out.push(Painted { contours, color });
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

    /// The element's own style over the inherited one; `None` when it is
    /// not rendered (`display: none`) or is a non-rendering element.
    fn style(&self, element: &Element, inherited: Style) -> Option<Style> {
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
        let mut style = inherited;
        style.opacity = inherited.opacity;
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
            if let Some(color) = self.color(value, inherited.color) {
                style.color = color;
            }
        }
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
                "opacity" => style.opacity *= opacity(value).unwrap_or(1.0),
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
                _ => {}
            }
        }
        Some(style)
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
        if let Some(rest) = value.strip_prefix("url(") {
            // A paint server paints its first stop's color, else the
            // fallback after the reference.
            let (reference, fallback) = rest.split_once(')')?;
            let id = reference
                .trim()
                .trim_matches(['"', '\''])
                .strip_prefix('#')?;
            if let Some(color) = self.first_stop(id) {
                return Some(Paint::Color(color));
            }
            let fallback = fallback.trim();
            return if fallback.is_empty() {
                Some(Paint::None)
            } else {
                self.paint(fallback)
            };
        }
        let black = Srgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        };
        self.color(value, black).map(Paint::Color)
    }

    fn first_stop(&self, id: &str) -> Option<Srgba> {
        let gradient = self.by_id(id)?;
        let stop = self.document.children(gradient).ok()?.find(|&child| {
            svg_element(self.document, child).is_some_and(|e| e.name().local() == "stop")
        })?;
        let stop = svg_element(self.document, stop)?;
        let mut color = None;
        let mut alpha = 1.0;
        let mut apply = |name: &str, value: &str| match name {
            "stop-color" => {
                color = self.color(
                    value.trim(),
                    Srgba {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 255,
                    },
                )
            }
            "stop-opacity" => alpha = opacity(value.trim()).unwrap_or(1.0),
            _ => {}
        };
        for name in ["stop-color", "stop-opacity"] {
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
        let color = color.unwrap_or(Srgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        });
        Some(with_alpha(color, alpha))
    }
}

fn with_alpha(color: Srgba, alpha: f64) -> Srgba {
    Srgba {
        a: (f64::from(color.a) * alpha.clamp(0.0, 1.0)).round() as u8,
        ..color
    }
}

fn resolve(paint: Paint, current: Srgba, alpha: f64) -> Option<Srgba> {
    let color = match paint {
        Paint::None => return None,
        Paint::Color(c) => c,
        Paint::Current => current,
    };
    Some(with_alpha(color, alpha))
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
    let flush = |points: &mut Vec<(f64, f64)>, out: &mut Vec<Subpath>, closed: bool| {
        if points.len() >= 2 || (closed && !points.is_empty()) {
            out.push(Subpath {
                points: std::mem::take(points),
                closed,
            });
        } else {
            points.clear();
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
                flush(&mut points, &mut out, false);
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
                    flush(&mut points, &mut out, true);
                }
                current = start;
            }
            _ => break,
        }
        last_control = control;
        last_quad = quad;
    }
    flush(&mut points, &mut out, false);
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
    Subpath {
        points: (0..n)
            .map(|k| {
                let (s, c) = (std::f64::consts::TAU * k as f64 / n as f64).sin_cos();
                (cx + rx * c, cy + ry * s)
            })
            .collect(),
        closed: true,
    }
}

fn rounded_rect(x: f64, y: f64, w: f64, h: f64, rx: f64, ry: f64) -> Subpath {
    if rx <= 0.0 || ry <= 0.0 {
        return Subpath {
            points: vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)],
            closed: true,
        };
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
    Subpath {
        points,
        closed: true,
    }
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

/// The stroke of one subpath as polygons in user space.
fn stroke(subpath: &Subpath, style: &Style, scale: f64, out: &mut Vec<Vec<(f64, f64)>>) {
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
            fill: Paint::None,
            stroke: Paint::Current,
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
            stroke_width: 2.0,
            cap: Cap::Butt,
            join: Join::Miter,
            miter_limit: 4.0,
            color: Srgba::WHITE,
            visible: true,
            opacity: 1.0,
        };
        let mut out = Vec::new();
        let path = Subpath {
            points: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)],
            closed: false,
        };
        stroke(&path, &style, 1.0, &mut out);
        // Two segment quads and one miter join reaching the outer corner.
        assert_eq!(out.len(), 3);
        assert!(
            out[2]
                .iter()
                .any(|&(x, y)| (x - 11.0).abs() < 1e-9 && (y + 1.0).abs() < 1e-9)
        );
    }
}
