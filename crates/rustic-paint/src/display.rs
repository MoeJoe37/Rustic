//! Immutable display lists built from block fragments (CSS 2.2 Appendix E
//! for the block-only subset, CSS Backgrounds 3 §2.11 canvas propagation).

use crate::color::{Srgba, srgba};
use rustic_base::CancellationToken;
use rustic_css::{BorderStyle, Image, PropertyId as P, PropertyValue as V, Visibility, VisualBox};
use rustic_dom::{Document, HTML_NAMESPACE, NodeId, NodeKind};
use rustic_layout::{BlockFragment, BlockLayout, BoxTree, CssPoint, CssPx, CssRect, CssSize};
use rustic_style::ComputedStyle;
use std::{error::Error, fmt, fmt::Write as _};

/// Display list budgets, independent of layout limits.
#[derive(Clone, Copy, Debug)]
pub struct PaintLimits {
    /// Maximum display items.
    pub max_items: usize,
}

impl Default for PaintLimits {
    fn default() -> Self {
        Self { max_items: 400_000 }
    }
}

/// What a display item paints, for dumps and later hit testing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemRole {
    /// A box's background color.
    Background,
    /// A box's border (one side, or all four sides when they match).
    Border,
}

/// A filled area in CSS pixels. Edges are not snapped to device pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// An axis-aligned rectangle.
    Rect(CssRect),
    /// The area between an outer rectangle and an inner one inside it.
    Ring {
        /// Outer edge.
        outer: CssRect,
        /// Inner edge, contained in `outer`.
        inner: CssRect,
    },
    /// A convex quadrilateral, used for one mitred border side.
    Quad([CssPoint; 4]),
}

/// One paint operation: fill `shape` with `color` using source-over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayItem {
    /// Principal-box index of the source box in the layout's snapshot.
    pub source_box: usize,
    /// Source DOM node (identity only).
    pub node: NodeId,
    /// What the item paints.
    pub role: ItemRole,
    /// The filled area.
    pub shape: Shape,
    /// Straight-alpha sRGB color; never fully transparent.
    pub color: Srgba,
}

/// Why a box needs paint features this reference does not implement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedPaint {
    /// A `background-image` other than `none`.
    BackgroundImage,
    /// A visible border style other than `solid`.
    BorderStyle,
    /// `opacity` below 1 needs group compositing.
    Opacity,
}

/// An atomic failure to build a display list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PaintError {
    /// A box needs an unimplemented paint feature.
    Unsupported {
        /// The source node.
        node: NodeId,
        /// The missing feature.
        reason: UnsupportedPaint,
    },
    /// The layout does not belong to the box snapshot, or a style is invalid.
    InvalidInput,
    /// More display items than [`PaintLimits::max_items`].
    ItemLimit,
    /// Cooperative cancellation.
    Cancelled,
    /// Dump byte budget exceeded.
    OutputLimit,
}

impl fmt::Display for PaintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { reason, .. } => {
                write!(f, "paint reference does not yet support {reason:?}")
            }
            Self::InvalidInput => f.write_str("paint input is inconsistent with its layout"),
            Self::ItemLimit => f.write_str("display item limit exceeded"),
            Self::Cancelled => f.write_str("paint cancelled"),
            Self::OutputLimit => f.write_str("display list dump output limit exceeded"),
        }
    }
}

impl Error for PaintError {}

/// An immutable list of paint operations over a canvas color, in paint order.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayList {
    viewport: CssSize,
    canvas: Srgba,
    items: Vec<DisplayItem>,
}

fn color(style: &ComputedStyle, property: P) -> Result<Srgba, PaintError> {
    match style.get(property) {
        V::Color(c) => srgba(c).ok_or(PaintError::InvalidInput),
        _ => Err(PaintError::InvalidInput),
    }
}

fn has_image(style: &ComputedStyle) -> Result<bool, PaintError> {
    match style.get(P::BackgroundImage) {
        V::BackgroundImage(layers) => Ok(layers.iter().any(|l| *l != Image::None)),
        _ => Err(PaintError::InvalidInput),
    }
}

fn element_is(document: &Document, node: NodeId, local: &str) -> bool {
    matches!(
        document.node(node).map(|n| n.kind()),
        Ok(NodeKind::Element(e))
            if e.name().namespace() == Some(HTML_NAMESPACE) && e.name().local() == local
    )
}

fn edges(r: CssRect) -> (f64, f64, f64, f64) {
    let o = r.origin();
    (
        o.x.get(),
        o.y.get(),
        o.x.get() + r.size().width().get(),
        o.y.get() + r.size().height().get(),
    )
}

fn point(x: f64, y: f64) -> Result<CssPoint, PaintError> {
    Ok(CssPoint {
        x: CssPx::new(x).map_err(|_| PaintError::InvalidInput)?,
        y: CssPx::new(y).map_err(|_| PaintError::InvalidInput)?,
    })
}

struct Builder<'a> {
    items: Vec<DisplayItem>,
    limits: PaintLimits,
    cancellation: &'a CancellationToken,
}

impl Builder<'_> {
    fn push(&mut self, item: DisplayItem) -> Result<(), PaintError> {
        self.cancellation
            .check()
            .map_err(|_| PaintError::Cancelled)?;
        if item.color.is_transparent() {
            return Ok(());
        }
        if self.items.len() >= self.limits.max_items {
            return Err(PaintError::ItemLimit);
        }
        self.items.push(item);
        Ok(())
    }

    fn background(
        &mut self,
        fragment: &BlockFragment,
        source: usize,
        style: &ComputedStyle,
    ) -> Result<(), PaintError> {
        let clip = match style.get(P::BackgroundClip) {
            V::BackgroundBox(boxes) => *boxes.last().ok_or(PaintError::InvalidInput)?,
            _ => return Err(PaintError::InvalidInput),
        };
        let area = match clip {
            VisualBox::BorderBox => fragment.border(),
            VisualBox::PaddingBox => fragment.padding(),
            VisualBox::ContentBox => fragment.content(),
        };
        self.push(DisplayItem {
            source_box: source,
            node: fragment.node(),
            role: ItemRole::Background,
            shape: Shape::Rect(area),
            color: color(style, P::BackgroundColor)?,
        })
    }

    fn border(
        &mut self,
        fragment: &BlockFragment,
        source: usize,
        style: &ComputedStyle,
    ) -> Result<(), PaintError> {
        let sides = [
            (P::BorderTopStyle, P::BorderTopColor),
            (P::BorderRightStyle, P::BorderRightColor),
            (P::BorderBottomStyle, P::BorderBottomColor),
            (P::BorderLeftStyle, P::BorderLeftColor),
        ];
        let (x0, y0, x1, y1) = edges(fragment.border());
        let (ix0, iy0, ix1, iy1) = edges(fragment.padding());
        let widths = [iy0 - y0, x1 - ix1, y1 - iy1, ix0 - x0];
        let mut colors = [Srgba::WHITE; 4];
        for (i, &(style_property, color_property)) in sides.iter().enumerate() {
            colors[i] = color(style, color_property)?;
            if widths[i] > 0.0
                && !matches!(
                    style.get(style_property),
                    V::BorderStyle(BorderStyle::Solid)
                )
            {
                return Err(PaintError::Unsupported {
                    node: fragment.node(),
                    reason: UnsupportedPaint::BorderStyle,
                });
            }
        }
        if widths.iter().all(|&w| w <= 0.0) {
            return Ok(());
        }
        let item = |role, shape, color| DisplayItem {
            source_box: source,
            node: fragment.node(),
            role,
            shape,
            color,
        };
        // Matching sides are one ring, so no antialiased seam appears at
        // the corner diagonals where separately composited quads would meet.
        if colors.iter().all(|&c| c == colors[0]) {
            return self.push(item(
                ItemRole::Border,
                Shape::Ring {
                    outer: fragment.border(),
                    inner: fragment.padding(),
                },
                colors[0],
            ));
        }
        let quads = [
            [(x0, y0), (x1, y0), (ix1, iy0), (ix0, iy0)],
            [(x1, y0), (x1, y1), (ix1, iy1), (ix1, iy0)],
            [(x1, y1), (x0, y1), (ix0, iy1), (ix1, iy1)],
            [(x0, y1), (x0, y0), (ix0, iy0), (ix0, iy1)],
        ];
        for (i, quad) in quads.iter().enumerate() {
            if widths[i] <= 0.0 {
                continue;
            }
            let mut points = [CssPoint::default(); 4];
            for (p, &(x, y)) in points.iter_mut().zip(quad) {
                *p = point(x, y)?;
            }
            self.push(item(ItemRole::Border, Shape::Quad(points), colors[i]))?;
        }
        Ok(())
    }
}

impl DisplayList {
    /// Build the display list for a block layout of `tree`.
    ///
    /// The canvas starts opaque white; the root element's background color,
    /// or an HTML root's first `body` child's when the root's is transparent
    /// and has no image, is painted across it, and that box then paints no
    /// background of its own. Each block then paints its background color
    /// (clipped to the bottom layer's `background-clip` box) and solid
    /// borders, in preorder. `visibility:hidden` boxes paint nothing of their
    /// own. Unsupported features fail atomically.
    pub fn build(
        document: &Document,
        tree: &BoxTree,
        layout: &BlockLayout,
        viewport: CssSize,
        limits: PaintLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, PaintError> {
        let mut builder = Builder {
            items: Vec::new(),
            limits,
            cancellation,
        };
        let fragments = layout.fragments();
        let style_of = |fragment: &BlockFragment| -> Result<&ComputedStyle, PaintError> {
            let b = tree
                .boxes()
                .get(fragment.source_box())
                .filter(|b| b.node() == fragment.node())
                .ok_or(PaintError::InvalidInput)?;
            b.style().ok_or(PaintError::InvalidInput)
        };
        // Canvas propagation (CSS Backgrounds 3 §2.11.2).
        let mut canvas = Srgba::WHITE;
        let mut canvas_source = None;
        if let Some(root) = fragments.first().filter(|f| f.parent().is_none()) {
            let root_style = style_of(root)?;
            let root_color = color(root_style, P::BackgroundColor)?;
            let mut source = 0;
            if root_color.is_transparent()
                && !has_image(root_style)?
                && element_is(document, root.node(), "html")
            {
                let body = fragments
                    .iter()
                    .position(|f| f.parent() == Some(0) && element_is(document, f.node(), "body"));
                // The first `body` child of the root that generated a box.
                if let Some(body) = body {
                    source = body;
                }
            }
            let chosen = style_of(&fragments[source])?;
            if has_image(chosen)? {
                return Err(PaintError::Unsupported {
                    node: fragments[source].node(),
                    reason: UnsupportedPaint::BackgroundImage,
                });
            }
            let c = color(chosen, P::BackgroundColor)?;
            canvas = over(c, Srgba::WHITE);
            canvas_source = Some(source);
        }
        for (index, fragment) in fragments.iter().enumerate() {
            cancellation.check().map_err(|_| PaintError::Cancelled)?;
            let style = style_of(fragment)?;
            match style.get(P::Opacity) {
                V::Opacity(o) if *o >= 1.0 => {}
                V::Opacity(_) => {
                    return Err(PaintError::Unsupported {
                        node: fragment.node(),
                        reason: UnsupportedPaint::Opacity,
                    });
                }
                _ => return Err(PaintError::InvalidInput),
            }
            if canvas_source != Some(index) && has_image(style)? {
                return Err(PaintError::Unsupported {
                    node: fragment.node(),
                    reason: UnsupportedPaint::BackgroundImage,
                });
            }
            if !matches!(style.get(P::Visibility), V::Visibility(Visibility::Visible)) {
                continue;
            }
            if canvas_source != Some(index) {
                builder.background(fragment, fragment.source_box(), style)?;
            }
            builder.border(fragment, fragment.source_box(), style)?;
        }
        Ok(Self {
            viewport,
            canvas,
            items: builder.items,
        })
    }

    /// The viewport size the list was built for, in CSS pixels.
    pub const fn viewport(&self) -> CssSize {
        self.viewport
    }

    /// The opaque color the whole canvas is cleared to.
    pub const fn canvas(&self) -> Srgba {
        self.canvas
    }

    /// Paint operations, in paint order.
    pub fn items(&self) -> &[DisplayItem] {
        &self.items
    }

    /// Deterministic text dump with no page text; no partial output on overflow.
    pub fn dump(&self, max_bytes: usize) -> Result<String, PaintError> {
        let hex = |c: Srgba| format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a);
        let r = |r: CssRect| {
            format!(
                "{},{},{},{}",
                r.origin().x.get(),
                r.origin().y.get(),
                r.size().width().get(),
                r.size().height().get()
            )
        };
        let mut out = String::new();
        let mut line = format!(
            "canvas {}x{} color={}\n",
            self.viewport.width().get(),
            self.viewport.height().get(),
            hex(self.canvas)
        );
        append(&mut out, &line, max_bytes)?;
        for (i, item) in self.items.iter().enumerate() {
            let role = match item.role {
                ItemRole::Background => "background",
                ItemRole::Border => "border",
            };
            let shape = match item.shape {
                Shape::Rect(rect) => format!("rect=({})", r(rect)),
                Shape::Ring { outer, inner } => format!("ring=({}) inner=({})", r(outer), r(inner)),
                Shape::Quad(points) => {
                    let mut s = String::from("quad=(");
                    for (n, p) in points.iter().enumerate() {
                        if n > 0 {
                            s.push(' ');
                        }
                        // Writing to a String cannot fail.
                        let _ = write!(s, "{},{}", p.x.get(), p.y.get());
                    }
                    s.push(')');
                    s
                }
            };
            line = format!(
                "item {i} box={} node={}:{} {role} {shape} color={}\n",
                item.source_box,
                item.node.slot(),
                item.node.generation(),
                hex(item.color)
            );
            append(&mut out, &line, max_bytes)?;
        }
        Ok(out)
    }
}

fn append(out: &mut String, line: &str, max_bytes: usize) -> Result<(), PaintError> {
    if out
        .len()
        .checked_add(line.len())
        .is_none_or(|n| n > max_bytes)
    {
        return Err(PaintError::OutputLimit);
    }
    out.push_str(line);
    Ok(())
}

/// Source-over of a straight-alpha color onto an opaque one, rounded once.
pub(crate) fn over(src: Srgba, dst: Srgba) -> Srgba {
    let a = f64::from(src.a) / 255.0;
    let mix = |s: u8, d: u8| (f64::from(s) * a + f64::from(d) * (1.0 - a)).round() as u8;
    Srgba {
        r: mix(src.r, dst.r),
        g: mix(src.g, dst.g),
        b: mix(src.b, dst.b),
        a: 255,
    }
}
