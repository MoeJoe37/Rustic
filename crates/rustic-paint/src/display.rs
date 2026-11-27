//! Immutable display lists built from block fragments: CSS 2.2 Appendix E
//! paint order with stacking contexts, z-index, floats and positioned boxes;
//! overflow clips; opacity groups; CSS Backgrounds 3 §2.11 canvas propagation.

use crate::clip::{Resolved, polygon_contains, resolve_shape};
use crate::color::{Srgba, srgba};
use crate::matrix::{BoxTransform, Homography, box_transform, perspective_of};
use crate::rounded::{RoundedRect, clip_convex, used_corners};
use rustic_base::CancellationToken;
use rustic_css::Overflow;
use rustic_css::{
    BackfaceVisibility, BasicShape, BorderStyle, BorderWidth, ClipPath, Float, GeometryBox,
    Image as CssImage, LengthPercentage, LengthUnit, OutlineStyle, Position, PropertyId as P,
    PropertyValue as V, TextDecorationStyle, Visibility, VisualBox, ZIndex,
};
use rustic_css::{BlendMode, FilterFunction, Isolation};
use rustic_dom::{Document, HTML_NAMESPACE, NodeId, NodeKind};
use rustic_layout::{
    AxisRange, BlockFragment, BlockLayout, BoxTree, Clip, CssPoint, CssPx, CssRect, CssSize,
    GlyphRotation, InlineFragment, PositionScheme, TextFragment,
};
use rustic_style::ComputedStyle;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::{error::Error, fmt, fmt::Write as _};

/// Display list budgets, independent of layout limits.
#[derive(Clone, Copy, Debug)]
pub struct PaintLimits {
    /// Maximum display commands (fills plus group begin/end markers).
    pub max_items: usize,
    /// Maximum outline points across all glyph paths.
    pub max_path_points: usize,
}

impl Default for PaintLimits {
    fn default() -> Self {
        Self {
            max_items: 400_000,
            max_path_points: 8_000_000,
        }
    }
}

/// One outline point of a [`Path`], in CSS pixels (y grows downward).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathPoint {
    /// Horizontal position.
    pub x: f64,
    /// Vertical position.
    pub y: f64,
    /// Whether the point lies on the curve; off-curve points are quadratic
    /// control points, and two in a row imply an on-curve midpoint
    /// (TrueType conventions).
    pub on_curve: bool,
}

/// Decoded raster image content: straight-alpha sRGB RGBA, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Image {
    /// An image of `width × height` pixels; `None` unless both are positive
    /// and `pixels` holds exactly four bytes per pixel.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        let expected = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?
            .checked_mul(4)?;
        (width > 0 && height > 0 && pixels.len() == expected).then_some(Self {
            width,
            height,
            pixels,
        })
    }

    /// Width in image pixels.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in image pixels.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Straight-alpha RGBA bytes, row-major.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

/// Fonts and decoded images a display list paints with.
#[derive(Clone, Debug, Default)]
pub struct Resources<'a> {
    /// The fonts the layout was computed with; required when it has text.
    pub fonts: Fonts<'a>,
    /// Decoded content of replaced elements, by element. Elements without
    /// an entry paint no content (only their background and border).
    pub images: HashMap<NodeId, Arc<Image>>,
    /// Computed styles of SVG elements inside outermost `svg` elements
    /// (which have no boxes), for author style sheets to reach them.
    pub svg_styles: HashMap<NodeId, Arc<ComputedStyle>>,
}

/// Closed quadratic contours filled with the nonzero rule (or, for SVG
/// `fill-rule: evenodd`, the even-odd rule), such as the glyphs of one
/// text fragment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    contours: Vec<Vec<PathPoint>>,
    even_odd: bool,
}

impl Path {
    /// The contours, each implicitly closed.
    pub fn contours(&self) -> &[Vec<PathPoint>] {
        &self.contours
    }

    /// Whether the path fills with the even-odd rule.
    pub fn even_odd(&self) -> bool {
        self.even_odd
    }

    fn points(&self) -> usize {
        self.contours.iter().map(Vec::len).sum()
    }
}

/// What a display item paints, for dumps and later hit testing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemRole {
    /// A box's background color.
    Background,
    /// A box's border (one side, or all four sides when they match).
    Border,
    /// The glyphs of one text fragment, in the text's `color`.
    Text,
    /// The content of a replaced element (its `color` is unused).
    Image,
    /// One text decoration line over one text fragment.
    Decoration,
    /// A box's outline (one ring, or two for `double`).
    Outline,
    /// One `box-shadow`.
    Shadow,
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
    /// Index into [`DisplayList::paths`].
    Path(usize),
    /// A rectangle with elliptical corners: index into
    /// [`DisplayList::rounded`].
    Rounded(usize),
    /// The area between two rounded rectangles, `inner` inside `outer`
    /// (indices into [`DisplayList::rounded`]).
    RoundedRing {
        /// Outer edge.
        outer: usize,
        /// Inner edge.
        inner: usize,
    },
    /// A blurred box shadow: index into [`DisplayList::shadows`].
    Shadow(usize),
    /// Image [`DisplayList::images`]`[image]` stretched over `rect`.
    Image {
        /// Destination rectangle (the replaced element's content box).
        rect: CssRect,
        /// Index into [`DisplayList::images`].
        image: usize,
    },
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
    /// Ancestor overflow clip in CSS pixels (see [`BlockFragment::clip`]).
    pub clip: Clip,
    /// The innermost rounded clip the item is also clipped by, as an index
    /// into [`DisplayList::clips`]; that node's ancestors clip it too.
    pub rounded_clip: Option<usize>,
}

/// A box shadow (CSS Backgrounds 3 §7.1): the coverage of `shape` (or, for
/// an inset shadow, of everything outside it) blurred by a Gaussian of
/// standard deviation `sigma`, then kept only outside `clip` (the border
/// box) for an outer shadow or only inside it (the padding box) for an
/// inset one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxShadow {
    /// The shadow's shape: the border box moved by the offset and spread,
    /// or for an inset shadow the padding box moved and shrunk; `None` when
    /// spreading left nothing.
    pub shape: Option<RoundedRect>,
    /// The Gaussian standard deviation in CSS pixels: half the blur radius.
    pub sigma: f64,
    /// The border box (outer) or padding box (inset), with its curve.
    pub clip: RoundedRect,
    /// Whether this is an inset shadow.
    pub inset: bool,
}

/// One filter operation on a group (Filter Effects 1 §13), in CSS pixels.
/// Color operations act on straight-alpha sRGB values in 0–1, each result
/// clamped to that range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FilterOp {
    /// A Gaussian blur of every channel with this standard deviation.
    Blur(f64),
    /// A 4×5 color matrix, row-major: each output channel of r, g, b, a is
    /// the dot product of its row with (r, g, b, a, 1).
    Color([f64; 20]),
    /// A shadow of the group's alpha under it.
    DropShadow {
        /// Horizontal offset.
        dx: f64,
        /// Vertical offset.
        dy: f64,
        /// Gaussian standard deviation.
        sigma: f64,
        /// Shadow color.
        color: Srgba,
    },
}

/// The color matrix of a `filter` color function, amounts clamped as
/// Filter Effects 1 §13.1 requires.
fn color_matrix(function: &FilterFunction) -> Option<[f64; 20]> {
    let linear = |slope: f64, intercept: f64| {
        let mut m = [0.0; 20];
        for c in 0..3 {
            m[c * 5 + c] = slope;
            m[c * 5 + 4] = intercept;
        }
        m[18] = 1.0;
        m
    };
    let rgb = |m: [f64; 9]| {
        let mut out = [0.0; 20];
        for r in 0..3 {
            out[r * 5..r * 5 + 3].copy_from_slice(&m[r * 3..r * 3 + 3]);
        }
        out[18] = 1.0;
        out
    };
    Some(match *function {
        FilterFunction::Brightness(a) => linear(a, 0.0),
        FilterFunction::Contrast(a) => linear(a, 0.5 - 0.5 * a),
        FilterFunction::Invert(a) => {
            let a = a.min(1.0);
            linear(1.0 - 2.0 * a, a)
        }
        FilterFunction::Opacity(a) => {
            let mut m = linear(1.0, 0.0);
            m[18] = a.min(1.0);
            m
        }
        FilterFunction::Grayscale(a) => {
            let s = 1.0 - a.min(1.0);
            rgb([
                0.2126 + 0.7874 * s,
                0.7152 - 0.7152 * s,
                0.0722 - 0.0722 * s,
                0.2126 - 0.2126 * s,
                0.7152 + 0.2848 * s,
                0.0722 - 0.0722 * s,
                0.2126 - 0.2126 * s,
                0.7152 - 0.7152 * s,
                0.0722 + 0.9278 * s,
            ])
        }
        FilterFunction::Sepia(a) => {
            let s = 1.0 - a.min(1.0);
            rgb([
                0.393 + 0.607 * s,
                0.769 - 0.769 * s,
                0.189 - 0.189 * s,
                0.349 - 0.349 * s,
                0.686 + 0.314 * s,
                0.168 - 0.168 * s,
                0.272 - 0.272 * s,
                0.534 - 0.534 * s,
                0.131 + 0.869 * s,
            ])
        }
        FilterFunction::Saturate(s) => rgb([
            0.213 + 0.787 * s,
            0.715 - 0.715 * s,
            0.072 - 0.072 * s,
            0.213 - 0.213 * s,
            0.715 + 0.285 * s,
            0.072 - 0.072 * s,
            0.213 - 0.213 * s,
            0.715 - 0.715 * s,
            0.072 + 0.928 * s,
        ]),
        FilterFunction::HueRotate(degrees) => {
            let (sin, cos) = crate::matrix::sin_cos_degrees(degrees);
            rgb([
                0.213 + cos * 0.787 - sin * 0.213,
                0.715 - cos * 0.715 - sin * 0.715,
                0.072 - cos * 0.072 + sin * 0.928,
                0.213 - cos * 0.213 + sin * 0.143,
                0.715 + cos * 0.285 + sin * 0.140,
                0.072 - cos * 0.072 - sin * 0.283,
                0.213 - cos * 0.213 - sin * 0.787,
                0.715 - cos * 0.715 + sin * 0.715,
                0.072 + cos * 0.928 + sin * 0.072,
            ])
        }
        _ => return None,
    })
}

/// One rounded clip: the padding edge curve of a box with `border-radius`
/// that clips its overflow, or a replaced element's rounded content box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipNode {
    /// The area kept.
    pub shape: RoundedRect,
    /// The next rounded clip out, if any.
    pub parent: Option<usize>,
}

/// One transformed stacking context: its items are in the box's local
/// (untransformed layout) coordinates, which `matrix` maps into its
/// parent's coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransformNode {
    /// The plane map from local to parent coordinates.
    pub matrix: Homography,
    /// The ancestors' overflow clip around the transformed box, in the
    /// parent's coordinates.
    pub clip: Clip,
    /// The ancestors' innermost rounded clip around the box, in the parent's
    /// coordinates.
    pub rounded_clip: Option<usize>,
    /// The next transform out, if any.
    pub parent: Option<usize>,
}

/// One display list command, in paint order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DisplayCommand {
    /// Fill one shape source-over into the current group.
    Fill(DisplayItem),
    /// Start a transparent group for a stacking context with `opacity < 1`,
    /// a blend mode, a filter or blending children to isolate.
    BeginGroup {
        /// Principal-box index of the stacking context root.
        source_box: usize,
        /// Source DOM node.
        node: NodeId,
        /// Group opacity, greater than 0 and at most 1.
        opacity: f64,
        /// How the group blends with what is under it.
        blend: BlendMode,
        /// Filter operations applied to the group first, as an index into
        /// [`DisplayList::filters`].
        filter: Option<usize>,
    },
    /// Composite the innermost open group into its parent: filtered, then
    /// blended with its opacity.
    EndGroup,
    /// Start a layer for a stacking context with a `clip-path`.
    BeginClip {
        /// Principal-box index of the stacking context root.
        source_box: usize,
        /// Source DOM node.
        node: NodeId,
        /// The area kept: a [`Shape::Rect`], [`Shape::Rounded`] or
        /// [`Shape::Path`].
        shape: Shape,
    },
    /// Composite the innermost open clip layer into its parent, keeping only
    /// what its shape covers.
    EndClip,
    /// Start a transformed stacking context: until the matching
    /// [`DisplayCommand::EndTransform`], coordinates are local to
    /// [`DisplayList::transforms`]`[transform]`, and what is painted is
    /// clipped by that node's clips in its parent's coordinates.
    BeginTransform {
        /// Principal-box index of the stacking context root.
        source_box: usize,
        /// Source DOM node.
        node: NodeId,
        /// Index into [`DisplayList::transforms`].
        transform: usize,
    },
    /// Return to the enclosing coordinates.
    EndTransform,
}

/// Why a box needs paint features this reference does not implement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedPaint {
    /// A `background-image` other than `none`.
    BackgroundImage,
    /// A visible border style other than `solid`.
    BorderStyle,
    /// A `text-decoration-style` other than `solid` or `double`.
    DecorationStyle,
    /// A visible `outline-style` other than `solid`, `double` or `auto`.
    OutlineStyle,
    /// A `clip-path` that references an SVG `<clipPath>` by URL.
    ClipPathUrl,
    /// A `clip-path: polygon()` with the `evenodd` fill rule.
    ClipPathFillRule,
    /// A `filter` that references an SVG `<filter>` by URL.
    FilterUrl,
}

/// The fonts a text layout was computed with, needed to paint its glyphs.
pub type Fonts<'a> = Option<&'a rustic_text::FontDatabase>;

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
    /// More display commands than [`PaintLimits::max_items`], or more path
    /// points than [`PaintLimits::max_path_points`].
    ItemLimit,
    /// The layout has text but no fonts were given, or a glyph outline is
    /// unreadable.
    Font,
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
            Self::Font => f.write_str("glyph outlines are unavailable"),
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
    commands: Vec<DisplayCommand>,
    /// Glyph paths referenced by [`Shape::Path`].
    paths: Vec<Path>,
    /// Images referenced by [`Shape::Image`].
    images: Vec<Arc<Image>>,
    /// Rounded clips referenced by [`DisplayItem::rounded_clip`].
    clips: Vec<ClipNode>,
    /// Rounded rectangles referenced by [`Shape::Rounded`] and
    /// [`Shape::RoundedRing`].
    rounded: Vec<RoundedRect>,
    /// Box shadows referenced by [`Shape::Shadow`].
    shadows: Vec<BoxShadow>,
    /// Filter chains referenced by [`DisplayCommand::BeginGroup`].
    filters: Vec<Vec<FilterOp>>,
    /// `clip-path` areas for hit testing.
    clip_paths: Vec<ClipPathNode>,
    /// Transformed stacking contexts referenced by
    /// [`DisplayCommand::BeginTransform`].
    transforms: Vec<TransformNode>,
    /// For each transform, the map from its local coordinates to the
    /// viewport.
    spaces: Vec<Homography>,
    /// Hit-testable border boxes in paint order.
    hits: Vec<HitRegion>,
    /// The root fragment, the target when no box is hit.
    root: Option<Hit>,
}

/// The box a hit test found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Hit {
    /// Index of the box in the [`BoxTree`].
    pub source_box: usize,
    /// The element that generated the box.
    pub node: NodeId,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct HitRegion {
    hit: Hit,
    border: CssRect,
    clip: Clip,
    /// The rounded border box, when any corner is rounded.
    rounded: Option<RoundedRect>,
    rounded_clip: Option<usize>,
    /// The innermost `clip-path` around the box, an index into
    /// [`DisplayList::clip_paths`].
    clip_path: Option<usize>,
    /// The innermost transform around the box, whose local coordinates the
    /// other fields are in.
    transform: Option<usize>,
}

/// A `clip-path` area for hit testing, inside its `parent`.
#[derive(Clone, Debug, PartialEq)]
struct ClipPathNode {
    shape: Resolved,
    parent: Option<usize>,
    /// The transform whose local coordinates `shape` is in.
    transform: Option<usize>,
}

/// Whether `(x, y)` lies within `clip` on both axes (half-open).
fn in_axis_clip(clip: Clip, x: f64, y: f64) -> bool {
    let inside = |range: Option<rustic_layout::AxisRange>, v: f64| {
        range.is_none_or(|r| r.start.get() <= v && v < r.end.get())
    };
    inside(clip.x, x) && inside(clip.y, y)
}

/// Whether `(x, y)` survives the rounded clip `node` and its ancestors.
fn in_clips(clips: &[ClipNode], mut node: Option<usize>, x: f64, y: f64) -> bool {
    while let Some(n) = node.and_then(|i| clips.get(i)) {
        if !n.shape.contains(x, y) {
            return false;
        }
        node = n.parent;
    }
    true
}

impl HitRegion {
    fn contains(&self, x: f64, y: f64, clips: &[ClipNode]) -> bool {
        if self.rounded.is_some_and(|r| !r.contains(x, y))
            || !in_clips(clips, self.rounded_clip, x, y)
        {
            return false;
        }
        let (left, top, right, bottom) = edges(self.border);
        left <= x && x < right && top <= y && y < bottom && in_axis_clip(self.clip, x, y)
    }
}

fn color(style: &ComputedStyle, property: P) -> Result<Srgba, PaintError> {
    match style.get(property) {
        V::Color(c) => srgba(c).ok_or(PaintError::InvalidInput),
        _ => Err(PaintError::InvalidInput),
    }
}

fn has_image(style: &ComputedStyle) -> Result<bool, PaintError> {
    match style.get(P::BackgroundImage) {
        V::BackgroundImage(layers) => Ok(layers.iter().any(|l| *l != CssImage::None)),
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

/// Whether `node` is an outermost SVG `svg` element.
fn element_is_svg_root(document: &Document, node: NodeId) -> bool {
    let Ok(n) = document.node(node) else {
        return false;
    };
    let svg = |kind: &NodeKind| matches!(kind, NodeKind::Element(e) if e.name().namespace() == Some(rustic_dom::SVG_NAMESPACE));
    matches!(n.kind(), NodeKind::Element(e) if e.name().local() == "svg")
        && svg(n.kind())
        && !n
            .parent()
            .and_then(|p| document.node(p).ok())
            .is_some_and(|p| svg(p.kind()))
}

/// A fieldset's rendered legend: its first child that is an HTML
/// `legend` (HTML §15.3.13; floated and positioned legends are not told
/// apart yet, TD-219).
fn rendered_legend(document: &Document, fieldset: NodeId) -> Option<NodeId> {
    if !element_is(document, fieldset, "fieldset") {
        return None;
    }
    let mut children = document.children(fieldset).ok()?;
    children
        .find(|&child| {
            matches!(
                document.node(child).map(|n| n.kind()),
                Ok(NodeKind::Element(_))
            )
        })
        .filter(|&child| element_is(document, child, "legend"))
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

/// The boxes and clip of one block or inline fragment.
#[derive(Clone, Copy)]
struct Geometry {
    node: NodeId,
    border: CssRect,
    padding: CssRect,
    content: CssRect,
    clip: Clip,
    /// The rounded border box, when any corner is rounded.
    rounded: Option<RoundedRect>,
    rounded_clip: Option<usize>,
    /// A horizontal span of the top border left unpainted: a fieldset's
    /// rendered legend (HTML §15.3.13).
    gap: Option<(f64, f64)>,
}

impl Geometry {
    fn block(
        f: &BlockFragment,
        clip: Clip,
        rounded: Option<RoundedRect>,
        rounded_clip: Option<usize>,
    ) -> Self {
        Self {
            node: f.node(),
            border: f.border(),
            padding: f.padding(),
            content: f.content(),
            clip,
            rounded,
            rounded_clip,
            gap: None,
        }
    }

    fn inline(
        f: &InlineFragment,
        clip: Clip,
        rounded: Option<RoundedRect>,
        rounded_clip: Option<usize>,
    ) -> Self {
        Self {
            node: f.node(),
            border: f.border(),
            padding: f.padding(),
            content: f.content(),
            clip,
            rounded,
            rounded_clip,
            gap: None,
        }
    }

    /// A fieldset's geometry around its rendered legend, whose border box
    /// is `legend`: the top border runs through the legend's middle with a
    /// gap behind it, and nothing of the fieldset paints above that line.
    /// Square-cornered fieldsets only; others keep their geometry.
    fn around_legend(mut self, legend: CssRect) -> Self {
        let (x0, y0, x1, y1) = edges(self.border);
        let (_, py0, _, _) = edges(self.padding);
        let (lx0, ly0, lx1, ly1) = edges(legend);
        let width = py0 - y0;
        let top = (ly0 + ly1) / 2.0 - width / 2.0;
        if self.rounded.is_some() || top <= y0 || top + width > y1 {
            return self;
        }
        let rect = |top: f64, bottom: f64, left: f64, right: f64| {
            CssRect::new(
                CssPoint {
                    x: CssPx::new(left).ok()?,
                    y: CssPx::new(top).ok()?,
                },
                CssSize::new(right - left, bottom - top).ok()?,
            )
            .ok()
        };
        let (px0, _, px1, py1) = edges(self.padding);
        let (Some(border), Some(padding)) = (
            rect(top, y1, x0, x1),
            rect((top + width).min(py1), py1, px0, px1),
        ) else {
            return self;
        };
        self.border = border;
        self.padding = padding;
        self.gap = Some((lx0, lx1));
        self
    }

    /// `area` (one of this fragment's boxes) with the matching curve.
    fn curve(&self, area: CssRect) -> Option<RoundedRect> {
        self.rounded.map(|r| r.inset(area)).filter(|r| !r.is_rect())
    }
}

/// The convex region owned by the border side from outer corner `a.0` to
/// outer corner `b.0` (clockwise): bounded by the side and the rays from
/// each outer corner through its inner corner, cut where the rays meet or
/// `far` away from the corners.
fn side_wedge(
    a: ((f64, f64), (f64, f64)),
    b: ((f64, f64), (f64, f64)),
    far: f64,
) -> Vec<(f64, f64)> {
    let direction = |(o, i): ((f64, f64), (f64, f64))| {
        let (dx, dy) = (i.0 - o.0, i.1 - o.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len > 0.0 {
            (dx / len, dy / len)
        } else {
            (0.0, 0.0)
        }
    };
    let (da, db) = (direction(a), direction(b));
    let (pa, pb) = (a.0, b.0);
    // Solve pa + s·da = pb + t·db.
    let det = db.0 * da.1 - da.0 * db.1;
    if det.abs() > 1e-12 {
        let (ex, ey) = (pb.0 - pa.0, pb.1 - pa.1);
        let s = (db.0 * ey - db.1 * ex) / det;
        let t = (da.0 * ey - da.1 * ex) / det;
        if s > 0.0 && t > 0.0 && s < far && t < far {
            return vec![pa, pb, (pa.0 + s * da.0, pa.1 + s * da.1)];
        }
    }
    vec![
        pa,
        pb,
        (pb.0 + far * db.0, pb.1 + far * db.1),
        (pa.0 + far * da.0, pa.1 + far * da.1),
    ]
}

/// The used rounded border box of a box, or `None` when every corner is
/// square.
fn rounded_box(style: &ComputedStyle, border: CssRect) -> Result<Option<RoundedRect>, PaintError> {
    let radius = |property| match style.get(property) {
        V::BorderRadius(r) => Ok(r),
        _ => Err(PaintError::InvalidInput),
    };
    let radii = [
        radius(P::BorderTopLeftRadius)?,
        radius(P::BorderTopRightRadius)?,
        radius(P::BorderBottomRightRadius)?,
        radius(P::BorderBottomLeftRadius)?,
    ];
    let rounded = used_corners(radii, border).ok_or(PaintError::InvalidInput)?;
    Ok((!rounded.is_rect()).then_some(rounded))
}

/// The clip a block applies to its own in-flow content: its ancestors' clip
/// `clip` narrowed to its padding box on axes whose overflow is not visible.
fn content_clip(f: &BlockFragment, clip: Clip) -> Clip {
    let own = |overflow: Overflow, start: CssPx, size: CssPx| {
        (overflow != Overflow::Visible).then(|| AxisRange {
            start,
            end: CssPx::new(start.get() + size.get()).unwrap_or(start),
        })
    };
    let p = f.padding();
    let (ox, oy) = f.overflow();
    let narrow = |a: Option<AxisRange>, b: Option<AxisRange>| match (a, b) {
        (Some(a), Some(b)) => Some(AxisRange {
            start: if a.start > b.start { a.start } else { b.start },
            end: if a.end < b.end { a.end } else { b.end },
        }),
        (a, None) => a,
        (None, b) => b,
    };
    Clip {
        x: narrow(clip.x, own(ox, p.origin().x, p.size().width())),
        y: narrow(clip.y, own(oy, p.origin().y, p.size().height())),
    }
}

struct Builder<'a> {
    commands: Vec<DisplayCommand>,
    limits: PaintLimits,
    cancellation: &'a CancellationToken,
    fonts: Fonts<'a>,
    svg_styles: &'a HashMap<NodeId, Arc<ComputedStyle>>,
    paths: Vec<Path>,
    path_points: usize,
    /// Outlines by (face, glyph), each read once.
    outlines: HashMap<(usize, u16), rustic_text::Outline>,
    images: Vec<Arc<Image>>,
    clips: Vec<ClipNode>,
    rounded: Vec<RoundedRect>,
    shadows: Vec<BoxShadow>,
    filters: Vec<Vec<FilterOp>>,
}

impl Builder<'_> {
    /// Store a box's `filter` as operations, or `None` for `none`.
    fn filter(&mut self, node: NodeId, style: &ComputedStyle) -> Result<Option<usize>, PaintError> {
        let V::Filter(functions) = style.get(P::Filter) else {
            return Err(PaintError::InvalidInput);
        };
        if functions.is_empty() {
            return Ok(None);
        }
        let px = |lp: Option<&LengthPercentage>| match lp {
            None => Ok(0.0),
            Some(lp) => crate::clip::length(lp, 0.0).ok_or(PaintError::InvalidInput),
        };
        let mut ops = Vec::with_capacity(functions.len());
        for function in functions {
            ops.push(match function {
                FilterFunction::Url(_) => {
                    return Err(PaintError::Unsupported {
                        node,
                        reason: UnsupportedPaint::FilterUrl,
                    });
                }
                FilterFunction::Blur(r) => FilterOp::Blur(px(Some(r))?.max(0.0)),
                FilterFunction::DropShadow(s) => FilterOp::DropShadow {
                    dx: px(Some(&s.x))?,
                    dy: px(Some(&s.y))?,
                    sigma: px(s.blur.as_ref())?.max(0.0) / 2.0,
                    color: match s.color {
                        Some(c) => srgba(&c).ok_or(PaintError::InvalidInput)?,
                        None => color(style, P::Color)?,
                    },
                },
                other => FilterOp::Color(color_matrix(other).ok_or(PaintError::InvalidInput)?),
            });
        }
        self.filters.push(ops);
        Ok(Some(self.filters.len() - 1))
    }

    /// Paint a box's outer shadows (before its background) or inset ones
    /// (after it), the first shadow on top.
    fn shadows(
        &mut self,
        fragment: Geometry,
        source: usize,
        style: &ComputedStyle,
        inset: bool,
    ) -> Result<(), PaintError> {
        let V::BoxShadow(shadows) = style.get(P::BoxShadow) else {
            return Err(PaintError::InvalidInput);
        };
        let area = if inset {
            fragment.padding
        } else {
            fragment.border
        };
        let clip = fragment.curve(area).unwrap_or(RoundedRect {
            rect: area,
            corners: [crate::rounded::Corner::default(); 4],
        });
        let px = |lp: Option<&LengthPercentage>| match lp {
            None => Ok(0.0),
            Some(lp) => crate::clip::length(lp, 0.0).ok_or(PaintError::InvalidInput),
        };
        for shadow in shadows.iter().rev().filter(|s| s.inset == inset) {
            let color = match shadow.color {
                Some(c) => srgba(&c).ok_or(PaintError::InvalidInput)?,
                None => color(style, P::Color)?,
            };
            if color.is_transparent() {
                continue;
            }
            let (dx, dy) = (px(Some(&shadow.x))?, px(Some(&shadow.y))?);
            let blur = px(shadow.blur.as_ref())?.max(0.0);
            let spread = px(shadow.spread.as_ref())?;
            let shape = clip.shadow(dx, dy, if inset { -spread } else { spread });
            self.shadows.push(BoxShadow {
                shape,
                sigma: blur / 2.0,
                clip,
                inset,
            });
            let index = self.shadows.len() - 1;
            self.push(DisplayItem {
                source_box: source,
                node: fragment.node,
                role: ItemRole::Shadow,
                shape: Shape::Shadow(index),
                color,
                clip: fragment.clip,
                rounded_clip: fragment.rounded_clip,
            })?;
        }
        Ok(())
    }

    /// The layer shape and hit area of a box's `clip-path`, or `None` for
    /// `none`. The reference box is the border box unless one is named;
    /// `fill-box` is the content box and `stroke-box` and `view-box` the
    /// border box, as for boxes without SVG layout. A reference box alone
    /// clips to its curve when the box has `border-radius`.
    fn clip_path(
        &mut self,
        fragment: &BlockFragment,
        style: &ComputedStyle,
        path: &ClipPath,
    ) -> Result<Option<(Shape, Resolved)>, PaintError> {
        let unsupported = |reason| PaintError::Unsupported {
            node: fragment.node(),
            reason,
        };
        let (shape, reference) = match path {
            ClipPath::None => return Ok(None),
            ClipPath::Url(_) => return Err(unsupported(UnsupportedPaint::ClipPathUrl)),
            ClipPath::Shape(shape, reference) => (Some(shape), *reference),
            ClipPath::Box(reference) => (None, Some(*reference)),
        };
        let border = fragment.border();
        let area = match reference.unwrap_or(GeometryBox::BorderBox) {
            GeometryBox::MarginBox => {
                let (x0, y0, x1, y1) = edges(border);
                let m = fragment.margin();
                let (l, t) = (x0 - m.left.get(), y0 - m.top.get());
                let (r, b) = (x1 + m.right.get(), y1 + m.bottom.get());
                CssRect::new(
                    point(l, t)?,
                    CssSize::new((r - l).max(0.0), (b - t).max(0.0))
                        .map_err(|_| PaintError::InvalidInput)?,
                )
                .map_err(|_| PaintError::InvalidInput)?
            }
            GeometryBox::BorderBox | GeometryBox::StrokeBox | GeometryBox::ViewBox => border,
            GeometryBox::PaddingBox => fragment.padding(),
            GeometryBox::ContentBox | GeometryBox::FillBox => fragment.content(),
        };
        let resolved = match shape {
            Some(BasicShape::Polygon { evenodd: true, .. }) => {
                return Err(unsupported(UnsupportedPaint::ClipPathFillRule));
            }
            Some(shape) => {
                let (x0, y0, x1, y1) = edges(area);
                resolve_shape(shape, (x0, y0, x1 - x0, y1 - y0)).ok_or(PaintError::InvalidInput)?
            }
            None => {
                let curve = rounded_box(style, border)?.map(|r| r.inset(area));
                Resolved::Rounded(curve.unwrap_or(RoundedRect {
                    rect: area,
                    corners: [crate::rounded::Corner::default(); 4],
                }))
            }
        };
        let shape = match &resolved {
            Resolved::Rounded(r) if r.is_rect() => Shape::Rect(r.rect),
            Resolved::Rounded(r) => Shape::Rounded(self.rounded(*r)),
            Resolved::Polygon(points) => Shape::Path(self.polygon_path(vec![points.clone()])?),
        };
        Ok(Some((shape, resolved)))
    }

    /// Store a rounded rectangle for a shape.
    fn rounded(&mut self, shape: RoundedRect) -> usize {
        self.rounded.push(shape);
        self.rounded.len() - 1
    }

    /// A rounded clip of `shape` inside `parent`, or `parent` when `shape`
    /// has square corners (the rectangular part is in the axis clip).
    fn rounded_clip(&mut self, shape: RoundedRect, parent: Option<usize>) -> Option<usize> {
        if shape.is_rect() {
            return parent;
        }
        self.clips.push(ClipNode { shape, parent });
        Some(self.clips.len() - 1)
    }

    /// Store `contours` (polygons in CSS pixels) as a nonzero path.
    fn polygon_path(&mut self, contours: Vec<Vec<(f64, f64)>>) -> Result<usize, PaintError> {
        self.polygon_path_with(contours, false)
    }

    /// Store `contours` as a path filled by the even-odd rule or not.
    fn polygon_path_with(
        &mut self,
        contours: Vec<Vec<(f64, f64)>>,
        even_odd: bool,
    ) -> Result<usize, PaintError> {
        let points: usize = contours.iter().map(Vec::len).sum();
        self.path_points = self
            .path_points
            .checked_add(points)
            .filter(|&n| n <= self.limits.max_path_points)
            .ok_or(PaintError::ItemLimit)?;
        let path = Path {
            contours: contours
                .into_iter()
                .map(|c| {
                    c.into_iter()
                        .map(|(x, y)| PathPoint {
                            x,
                            y,
                            on_curve: true,
                        })
                        .collect()
                })
                .collect(),
            even_odd,
        };
        self.paths.push(path);
        Ok(self.paths.len() - 1)
    }

    fn command(&mut self, command: DisplayCommand) -> Result<(), PaintError> {
        self.cancellation
            .check()
            .map_err(|_| PaintError::Cancelled)?;
        if self.commands.len() >= self.limits.max_items {
            return Err(PaintError::ItemLimit);
        }
        self.commands.push(command);
        Ok(())
    }

    fn push(&mut self, item: DisplayItem) -> Result<(), PaintError> {
        if item.color.is_transparent() {
            return self.cancellation.check().map_err(|_| PaintError::Cancelled);
        }
        self.command(DisplayCommand::Fill(item))
    }

    fn background(
        &mut self,
        fragment: Geometry,
        source: usize,
        style: &ComputedStyle,
    ) -> Result<(), PaintError> {
        let clip = match style.get(P::BackgroundClip) {
            V::BackgroundBox(boxes) => *boxes.last().ok_or(PaintError::InvalidInput)?,
            _ => return Err(PaintError::InvalidInput),
        };
        let area = match clip {
            VisualBox::BorderBox => fragment.border,
            VisualBox::PaddingBox => fragment.padding,
            VisualBox::ContentBox => fragment.content,
        };
        let color = color(style, P::BackgroundColor)?;
        let shape = match fragment.curve(area) {
            Some(r) if !color.is_transparent() => Shape::Rounded(self.rounded(r)),
            _ => Shape::Rect(area),
        };
        self.push(DisplayItem {
            source_box: source,
            node: fragment.node,
            role: ItemRole::Background,
            shape,
            color,
            clip: fragment.clip,
            rounded_clip: fragment.rounded_clip,
        })
    }

    /// Paint the native parts of a form control (`crate::controls`) in
    /// `fragment`'s content box.
    fn control(
        &mut self,
        document: &Document,
        fragment: Geometry,
        source: usize,
        checked: Option<bool>,
    ) -> Result<(), PaintError> {
        for (rect, color, radius) in
            crate::controls::parts(document, fragment.node, fragment.content, checked)
        {
            let shape = if radius > 0.0 {
                let corner = crate::rounded::Corner {
                    x: radius,
                    y: radius,
                };
                Shape::Rounded(self.rounded(RoundedRect {
                    rect,
                    corners: [corner; 4],
                }))
            } else {
                Shape::Rect(rect)
            };
            self.push(DisplayItem {
                source_box: source,
                node: fragment.node,
                role: ItemRole::Background,
                shape,
                color,
                clip: fragment.clip,
                rounded_clip: fragment.rounded_clip,
            })?;
        }
        Ok(())
    }

    /// Paint an outermost `svg` element's content (`crate::svg`) in
    /// `fragment`'s content box, clipped to it.
    fn svg(
        &mut self,
        document: &Document,
        fragment: Geometry,
        source: usize,
        style: &ComputedStyle,
    ) -> Result<(), PaintError> {
        if !element_is_svg_root(document, fragment.node) {
            return Ok(());
        }
        let current = color(style, P::Color)?;
        let content = fragment.content;
        // The viewport clips (`overflow: hidden` is the UA default).
        let (x0, y0, x1, y1) = edges(content);
        let narrow = |outer: Option<rustic_layout::AxisRange>, start: f64, end: f64| {
            let (mut start, mut end) = (start, end);
            if let Some(o) = outer {
                start = start.max(o.start.get());
                end = end.min(o.end.get());
            }
            Some(rustic_layout::AxisRange {
                start: CssPx::new(start).ok()?,
                end: CssPx::new(end.max(start)).ok()?,
            })
        };
        let clip = rustic_layout::Clip {
            x: narrow(fragment.clip.x, x0, x1),
            y: narrow(fragment.clip.y, y0, y1),
        };
        // Layers the SVG opened, closed here if it left any open.
        let mut open: Vec<DisplayCommand> = Vec::new();
        let font = crate::svg::Font {
            size: style.font_size_px(),
            families: match style.get(P::FontFamily) {
                V::FontFamily(families) => families
                    .iter()
                    .map(|f| match f {
                        rustic_css::FamilyName::Named(n) => n.clone(),
                        rustic_css::FamilyName::Generic(g) => g.name().to_owned(),
                    })
                    .collect::<Vec<_>>()
                    .into(),
                _ => crate::svg::Font::default().families,
            },
            weight: match style.get(P::FontWeight) {
                V::FontWeight(rustic_css::FontWeight::Number(n)) => *n,
                V::FontWeight(rustic_css::FontWeight::Bold) => 700.0,
                _ => 400.0,
            },
            italic: !matches!(
                style.get(P::FontStyle),
                V::FontStyle(rustic_css::FontStyle::Normal)
            ),
        };
        for op in crate::svg::paint(
            document,
            fragment.node,
            content,
            current,
            self.fonts,
            font,
            self.svg_styles,
        ) {
            match op {
                crate::svg::Op::Fill {
                    contours,
                    color,
                    even_odd,
                } => {
                    let index = self.polygon_path_with(contours, even_odd)?;
                    self.push(DisplayItem {
                        source_box: source,
                        node: fragment.node,
                        role: ItemRole::Background,
                        shape: Shape::Path(index),
                        color,
                        clip,
                        rounded_clip: fragment.rounded_clip,
                    })?;
                }
                crate::svg::Op::BeginClip(contours) => {
                    let index = self.polygon_path(contours)?;
                    self.command(DisplayCommand::BeginClip {
                        source_box: source,
                        node: fragment.node,
                        shape: Shape::Path(index),
                    })?;
                    open.push(DisplayCommand::EndClip);
                }
                crate::svg::Op::BeginGroup(opacity) => {
                    self.command(DisplayCommand::BeginGroup {
                        source_box: source,
                        node: fragment.node,
                        opacity,
                        blend: BlendMode::Normal,
                        filter: None,
                    })?;
                    open.push(DisplayCommand::EndGroup);
                }
                crate::svg::Op::EndClip | crate::svg::Op::EndGroup => {
                    if let Some(end) = open.pop() {
                        self.command(end)?;
                    }
                }
            }
        }
        while let Some(end) = open.pop() {
            self.command(end)?;
        }
        Ok(())
    }

    /// Paint `style`'s border on `fragment`. For a cell of a table with
    /// collapsing borders, `collapsed` gives per side the style and side
    /// whose border wins that grid line (`None`: no border there).
    fn border(
        &mut self,
        fragment: Geometry,
        source: usize,
        style: &ComputedStyle,
        collapsed: Option<[Option<(&ComputedStyle, u8)>; 4]>,
    ) -> Result<(), PaintError> {
        let sides = [
            (P::BorderTopStyle, P::BorderTopColor),
            (P::BorderRightStyle, P::BorderRightColor),
            (P::BorderBottomStyle, P::BorderBottomColor),
            (P::BorderLeftStyle, P::BorderLeftColor),
        ];
        let (x0, y0, x1, y1) = edges(fragment.border);
        let (ix0, iy0, ix1, iy1) = edges(fragment.padding);
        let widths = [iy0 - y0, x1 - ix1, y1 - iy1, ix0 - x0];
        let mut colors = [Srgba::WHITE; 4];
        for i in 0..4 {
            let (style, (style_property, color_property)) = match collapsed {
                Some(list) => match list[i] {
                    Some((winner, side)) => (winner, sides[usize::from(side) % 4]),
                    None => {
                        colors[i] = Srgba {
                            r: 0,
                            g: 0,
                            b: 0,
                            a: 0,
                        };
                        continue;
                    }
                },
                None => (style, sides[i]),
            };
            colors[i] = color(style, color_property)?;
            // `inset` and `outset` are solid with the top and left sides
            // darker (inset) or the bottom and right ones (outset); CSS
            // Backgrounds 3 §4.2 leaves the shades to the implementation.
            // `groove` and `ridge` paint as `inset` and `outset`, and
            // `double`, `dashed` and `dotted` as `solid` (TD-219).
            let top_left = i == 0 || i == 3;
            match style.get(style_property) {
                V::BorderStyle(
                    BorderStyle::Solid
                    | BorderStyle::Double
                    | BorderStyle::Dashed
                    | BorderStyle::Dotted,
                ) => {}
                V::BorderStyle(BorderStyle::Inset | BorderStyle::Groove) if top_left => {
                    colors[i] = darker(colors[i]);
                }
                V::BorderStyle(BorderStyle::Outset | BorderStyle::Ridge) if !top_left => {
                    colors[i] = darker(colors[i]);
                }
                V::BorderStyle(
                    BorderStyle::Inset
                    | BorderStyle::Outset
                    | BorderStyle::Groove
                    | BorderStyle::Ridge,
                ) => {}
                _ if widths[i] <= 0.0 => {}
                _ => {
                    return Err(PaintError::Unsupported {
                        node: fragment.node,
                        reason: UnsupportedPaint::BorderStyle,
                    });
                }
            }
        }
        if widths.iter().all(|&w| w <= 0.0) {
            return Ok(());
        }
        let item = |role, shape, color| DisplayItem {
            source_box: source,
            node: fragment.node,
            role,
            shape,
            color,
            clip: fragment.clip,
            rounded_clip: fragment.rounded_clip,
        };
        if let Some(outer) = fragment.rounded {
            let inner = outer.inset(fragment.padding);
            if colors.iter().all(|&c| c == colors[0]) {
                if colors[0].is_transparent() {
                    return Ok(());
                }
                let shape = Shape::RoundedRing {
                    outer: self.rounded(outer),
                    inner: self.rounded(inner),
                };
                return self.push(item(ItemRole::Border, shape, colors[0]));
            }
            // Each side is the ring cut by the rays from the outer corners
            // through the inner ones (CSS Backgrounds 3 §5.5 leaves the
            // exact color transition to the implementation).
            let mut hole = inner.polygon();
            hole.reverse();
            let ring = [outer.polygon(), hole];
            let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
            let inner_corners = [(ix0, iy0), (ix1, iy0), (ix1, iy1), (ix0, iy1)];
            for side in 0..4 {
                if widths[side] <= 0.0 || colors[side].is_transparent() {
                    continue;
                }
                let (a, b) = (side, (side + 1) % 4);
                let wedge = side_wedge(
                    (corners[a], inner_corners[a]),
                    (corners[b], inner_corners[b]),
                    2.0 * ((x1 - x0) + (y1 - y0)) + 1.0,
                );
                let pieces: Vec<Vec<(f64, f64)>> = ring
                    .iter()
                    .map(|c| clip_convex(c, &wedge))
                    .filter(|c| c.len() >= 3)
                    .collect();
                if pieces.is_empty() {
                    continue;
                }
                let index = self.polygon_path(pieces)?;
                self.push(item(ItemRole::Border, Shape::Path(index), colors[side]))?;
            }
            return Ok(());
        }
        // Matching sides are one ring, so no antialiased seam appears at
        // the corner diagonals where separately composited quads would meet.
        if fragment.gap.is_none() && colors.iter().all(|&c| c == colors[0]) {
            return self.push(item(
                ItemRole::Border,
                Shape::Ring {
                    outer: fragment.border,
                    inner: fragment.padding,
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
            if let (0, Some((g0, g1))) = (i, fragment.gap) {
                // The top side in two pieces around the legend.
                let (g0, g1) = (g0.clamp(ix0, ix1), g1.clamp(ix0, ix1));
                let pieces = [
                    [(x0, y0), (g0, y0), (g0, iy0), (ix0, iy0)],
                    [(g1, y0), (x1, y0), (ix1, iy0), (g1, iy0)],
                ];
                for piece in pieces {
                    let mut points = [CssPoint::default(); 4];
                    for (p, &(x, y)) in points.iter_mut().zip(&piece) {
                        *p = point(x, y)?;
                    }
                    self.push(item(ItemRole::Border, Shape::Quad(points), colors[i]))?;
                }
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

    /// The outline of one box fragment (CSS UI 4 §5): a ring of
    /// `outline-width` outside the border box grown by `outline-offset`
    /// (which may be negative), in `outline-color`. `auto` paints as
    /// `solid`; `double` paints two rings of a third of the width each.
    /// A negative offset that collapses the inner edge fills the area.
    fn outline(
        &mut self,
        fragment: Geometry,
        source: usize,
        style: &ComputedStyle,
    ) -> Result<(), PaintError> {
        let width = match style.get(P::OutlineWidth) {
            V::BorderWidth(BorderWidth::Length(l))
                if l.unit == LengthUnit::Px && l.value.is_finite() =>
            {
                l.value.max(0.0)
            }
            _ => return Err(PaintError::InvalidInput),
        };
        let offset = match style.get(P::OutlineOffset) {
            V::LengthPercentage(LengthPercentage::Length(l))
                if l.unit == LengthUnit::Px && l.value.is_finite() =>
            {
                l.value
            }
            _ => return Err(PaintError::InvalidInput),
        };
        if width <= 0.0 {
            return Ok(());
        }
        let double = match style.get(P::OutlineStyle) {
            V::OutlineStyle(OutlineStyle::None) => return Ok(()),
            V::OutlineStyle(OutlineStyle::Solid | OutlineStyle::Auto) => false,
            V::OutlineStyle(OutlineStyle::Double) => true,
            V::OutlineStyle(_) => {
                return Err(PaintError::Unsupported {
                    node: fragment.node,
                    reason: UnsupportedPaint::OutlineStyle,
                });
            }
            _ => return Err(PaintError::InvalidInput),
        };
        let color = color(style, P::OutlineColor)?;
        if color.is_transparent() {
            return Ok(());
        }
        // Edges of the border box grown by `by` on every side; an axis that
        // collapses becomes empty at its center.
        let (x0, y0, x1, y1) = edges(fragment.border);
        let grow = |by: f64| -> Result<Option<CssRect>, PaintError> {
            let axis = |a: f64, b: f64| {
                let (lo, hi) = (a - by, b + by);
                if lo <= hi {
                    (lo, hi)
                } else {
                    ((a + b) / 2.0, (a + b) / 2.0)
                }
            };
            let ((l, r), (t, b)) = (axis(x0, x1), axis(y0, y1));
            let rect = CssRect::new(
                point(l, t)?,
                CssSize::new(r - l, b - t).map_err(|_| PaintError::InvalidInput)?,
            )
            .map_err(|_| PaintError::InvalidInput)?;
            Ok((r > l && b > t).then_some(rect))
        };
        let rings: &[(f64, f64)] = if double {
            &[(0.0, 1.0 / 3.0), (2.0 / 3.0, 1.0)]
        } else {
            &[(0.0, 1.0)]
        };
        // With `border-radius`, each rounded corner's radii grow by the
        // same distance as the edges (CSS UI 4 §5.1).
        let round = |rect: CssRect, by: f64| {
            fragment.rounded.map(|r| RoundedRect {
                rect,
                corners: r.corners.map(|c| {
                    if c.x > 0.0 && c.y > 0.0 {
                        crate::rounded::Corner {
                            x: (c.x + by).max(0.0),
                            y: (c.y + by).max(0.0),
                        }
                    } else {
                        crate::rounded::Corner::default()
                    }
                }),
            })
        };
        for &(from, to) in rings {
            let (out_by, in_by) = (offset + width * (1.0 - from), offset + width * (1.0 - to));
            let Some(outer) = grow(out_by)? else {
                continue;
            };
            let shape = match (grow(in_by)?, round(outer, out_by)) {
                (Some(inner), Some(rounded)) => Shape::RoundedRing {
                    outer: self.rounded(rounded),
                    inner: self.rounded(round(inner, in_by).unwrap_or(rounded)),
                },
                (Some(inner), None) => Shape::Ring { outer, inner },
                (None, Some(rounded)) => Shape::Rounded(self.rounded(rounded)),
                (None, None) => Shape::Rect(outer),
            };
            self.push(DisplayItem {
                source_box: source,
                node: fragment.node,
                role: ItemRole::Outline,
                shape,
                color,
                clip: fragment.clip,
                rounded_clip: fragment.rounded_clip,
            })?;
        }
        Ok(())
    }

    /// One decoration line across `text`'s advance width, placed from the
    /// fragment's face metrics: thickness `font-size / 16`; underline top
    /// `font-size / 10` below the baseline; overline top at the ascent;
    /// line-through centered half an x-height above the baseline. `double`
    /// draws a second line one thickness further out (through: below).
    fn decoration(
        &mut self,
        text: &TextFragment,
        line: Decoration,
        clip: Clip,
        rounded_clip: Option<usize>,
    ) -> Result<(), PaintError> {
        let face = self
            .fonts
            .and_then(|fonts| fonts.faces().get(text.face()))
            .ok_or(PaintError::Font)?;
        let size = text.font_size();
        let metrics = face.metrics();
        let em = f64::from(metrics.units_per_em);
        let thickness = size / 16.0;
        let baseline = text.baseline().get();
        let x_height = metrics
            .x_height
            .map_or(size / 2.0, |x| f64::from(x) * size / em);
        // Offsets below the baseline, in the line's own orientation.
        let (top, step) = match line.kind {
            Line::Under => (size / 10.0, 2.0 * thickness),
            Line::Over => (-f64::from(metrics.ascent) * size / em, -2.0 * thickness),
            Line::Through => (-x_height / 2.0 - thickness / 2.0, 2.0 * thickness),
        };
        let rect = text.rect();
        let tops: &[f64] = if line.double {
            &[top, top + step]
        } else {
            &[top]
        };
        for &offset in tops {
            let (x, y) = (rect.origin().x.get(), rect.origin().y.get());
            let (w, h) = (rect.size().width().get(), rect.size().height().get());
            // A sideways line's "below" is its left (clockwise) or right
            // (counterclockwise) side.
            // Upright text keeps its line's (clockwise) orientation.
            let along = if text.upright() {
                GlyphRotation::Clockwise
            } else {
                text.rotation()
            };
            let (x, y, w, h) = match along {
                GlyphRotation::None => (x, baseline + offset, w, thickness),
                GlyphRotation::Clockwise => (baseline - offset - thickness, y, thickness, h),
                GlyphRotation::Counterclockwise => (baseline + offset, y, thickness, h),
            };
            let area = CssRect::new(
                point(x, y)?,
                CssSize::new(w, h).map_err(|_| PaintError::InvalidInput)?,
            )
            .map_err(|_| PaintError::InvalidInput)?;
            self.push(DisplayItem {
                source_box: line.source_box,
                node: line.node,
                role: ItemRole::Decoration,
                shape: Shape::Rect(area),
                color: line.color,
                clip,
                rounded_clip,
            })?;
        }
        Ok(())
    }

    /// One path item with every glyph of `text`, scaled from font units and
    /// placed on its baseline (font y grows upward, CSS y downward).
    fn text(
        &mut self,
        text: &TextFragment,
        style: &ComputedStyle,
        clip: Clip,
        rounded_clip: Option<usize>,
    ) -> Result<(), PaintError> {
        let fill = color(style, P::Color)?;
        if fill.is_transparent() || text.glyphs().is_empty() {
            return Ok(());
        }
        let face = self
            .fonts
            .and_then(|fonts| fonts.faces().get(text.face()))
            .ok_or(PaintError::Font)?;
        let scale = text.font_size() / f64::from(face.units_per_em());
        // Sideways glyphs on vertical lines turn about their origins.
        let place: fn(f64, f64, f64, f64) -> (f64, f64) = match text.rotation() {
            GlyphRotation::None => |x, y, u, v| (x + u, y - v),
            GlyphRotation::Clockwise => |x, y, u, v| (x + v, y + u),
            GlyphRotation::Counterclockwise => |x, y, u, v| (x - v, y - u),
        };
        let mut path = Path::default();
        for glyph in text.glyphs() {
            self.cancellation
                .check()
                .map_err(|_| PaintError::Cancelled)?;
            let key = (text.face(), glyph.glyph.0);
            let outline = match self.outlines.entry(key) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    entry.insert(face.outline(glyph.glyph).map_err(|_| PaintError::Font)?)
                }
            };
            for contour in outline.contours() {
                self.path_points = self
                    .path_points
                    .checked_add(contour.len())
                    .filter(|&n| n <= self.limits.max_path_points)
                    .ok_or(PaintError::ItemLimit)?;
                path.contours.push(
                    contour
                        .iter()
                        .map(|p| {
                            let (x, y) = place(glyph.x, glyph.y, p.x * scale, p.y * scale);
                            PathPoint {
                                x,
                                y,
                                on_curve: p.on_curve,
                            }
                        })
                        .collect(),
                );
            }
        }
        if path.contours.is_empty() {
            return Ok(());
        }
        if !path
            .contours
            .iter()
            .flatten()
            .all(|p| p.x.is_finite() && p.y.is_finite())
        {
            return Err(PaintError::InvalidInput);
        }
        let index = self.paths.len();
        self.push(DisplayItem {
            source_box: text.source_box(),
            node: text.node(),
            role: ItemRole::Text,
            shape: Shape::Path(index),
            color: fill,
            clip,
            rounded_clip,
        })?;
        self.paths.push(path);
        Ok(())
    }
}

/// Which line a text decoration draws.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Line {
    Under,
    Over,
    Through,
}

/// One decoration a text fragment carries from a decorating box.
#[derive(Clone, Copy, Debug)]
struct Decoration {
    kind: Line,
    color: Srgba,
    double: bool,
    node: NodeId,
    source_box: usize,
}

/// The decorations propagated to the text of box `text` (CSS Text
/// Decoration 3 §2.1): from each ancestor with a `text-decoration-line`,
/// outermost first, stopping after a float or an absolutely positioned box
/// (decorations do not propagate into out-of-flow descendants).
fn decorations(tree: &BoxTree, text: usize) -> Result<Vec<Decoration>, PaintError> {
    let boxes = tree.boxes();
    let mut found = Vec::new();
    let mut current = boxes.get(text).ok_or(PaintError::InvalidInput)?.parent();
    let mut steps = 0usize;
    while let Some(index) = current {
        // Parents precede children, so this ends; the bound is a safeguard.
        steps += 1;
        if steps > boxes.len() {
            return Err(PaintError::InvalidInput);
        }
        let b = &boxes[index];
        let Some(style) = b.style() else {
            break;
        };
        if let V::TextDecorationLine(line) = style.get(P::TextDecorationLine) {
            if !line.is_none() {
                let double = match style.get(P::TextDecorationStyle) {
                    V::TextDecorationStyle(TextDecorationStyle::Solid) => false,
                    V::TextDecorationStyle(TextDecorationStyle::Double) => true,
                    _ => {
                        return Err(PaintError::Unsupported {
                            node: b.node(),
                            reason: UnsupportedPaint::DecorationStyle,
                        });
                    }
                };
                let color = color(style, P::TextDecorationColor)?;
                for (on, kind) in [
                    (line.line_through, Line::Through),
                    (line.overline, Line::Over),
                    (line.underline, Line::Under),
                ] {
                    if on {
                        found.push(Decoration {
                            kind,
                            color,
                            double,
                            node: b.node(),
                            source_box: index,
                        });
                    }
                }
            }
        }
        let out_of_flow = !matches!(style.get(P::Float), V::Float(Float::None))
            || matches!(
                style.get(P::Position),
                V::Position(Position::Absolute | Position::Fixed)
            );
        if out_of_flow {
            break;
        }
        current = b.parent();
    }
    found.reverse();
    Ok(found)
}

/// How a fragment takes part in painting (CSS 2.2 Appendix E, CSS 2.2 §9.9).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Role {
    /// A stacking context with its z-index (0 for non-positioned contexts).
    Context(i32),
    /// A positioned box with `z-index:auto`: painted atomically in its
    /// stacking context's z=0 layer, but its positioned descendants and
    /// stacking contexts belong to the enclosing stacking context.
    Positioned,
    /// A non-positioned float, painted like `Positioned` in the float layer.
    Float,
    /// A non-positioned atomic inline (`inline-block`, `inline-flex` or an
    /// inline replaced element), painted like `Positioned` in the inline
    /// layer, after the rest of its line (CSS 2.2 Appendix E step 7.2.1.4).
    Atomic,
    /// An in-flow, non-positioned block.
    Block,
}

/// Paint-order lists of one stacking context or pseudo-context.
#[derive(Default)]
struct Layers {
    negative: Vec<usize>,
    blocks: Vec<usize>,
    floats: Vec<usize>,
    zero: Vec<usize>,
    positive: Vec<usize>,
}

/// One step of the paint-order walk. `bool` is whether painting is muted
/// (inside an `opacity: 0` context, which still takes hits).
enum Task {
    Layer(usize, bool),
    Own(usize, bool),
    /// The inline content (inline box backgrounds and borders, then text,
    /// then atomic inlines, line by line) of these in-flow blocks, in tree
    /// order, resuming at block position `.1`, line position `.2`.
    Inline(Vec<usize>, usize, usize, bool),
    /// The outlines of every box in this stacking context, in tree order,
    /// after all of its other content (CSS 2.2 Appendix E step 10).
    Outlines(usize, bool),
    End,
    /// Leave a `clip-path` context: restore the enclosing clip path and
    /// close its layer if one was opened.
    EndClip(Option<usize>, bool),
    /// Leave a transformed context: restore the enclosing transform and
    /// close it if it was opened.
    EndTransform(Option<usize>, bool),
}

/// A box whose outline a stacking context paints.
#[derive(Clone, Copy)]
enum Outlined {
    Block(usize),
    Inline(usize),
}

impl DisplayList {
    /// Build the display list for a block layout of `tree`.
    ///
    /// The canvas starts opaque white; the root element's background color,
    /// or an HTML root's first `body` child's when the root's is transparent
    /// and has no image, is painted across it, and that box then paints no
    /// background of its own. Boxes paint in CSS 2.2 Appendix E order: each
    /// stacking context paints its own background and border, then children
    /// with negative z-index, in-flow blocks, floats, positioned boxes with
    /// `z-index` auto or 0 (and opacity contexts) in tree order, then positive
    /// z-index; equal z-index keeps tree order. Positioned boxes form stacking
    /// contexts when `z-index` is an integer, fixed boxes and `opacity < 1`
    /// always do. Each item carries its fragment's overflow clip. A context
    /// with `opacity < 1` is a group; `opacity: 0` paints nothing. Backgrounds
    /// are clipped to the bottom layer's `background-clip` box and borders
    /// must be solid. `visibility:hidden` boxes paint nothing of their own.
    /// Unsupported features fail atomically. A layout with text needs
    /// [`DisplayList::build_with_fonts`].
    pub fn build(
        document: &Document,
        tree: &BoxTree,
        layout: &BlockLayout,
        viewport: CssSize,
        limits: PaintLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, PaintError> {
        Self::build_with_fonts(document, tree, layout, viewport, None, limits, cancellation)
    }

    /// Like [`DisplayList::build`], also painting the layout's inline
    /// content with `fonts`, the database the layout was computed with.
    /// Each in-flow block's inline content paints after the floats of its
    /// layer (CSS 2.2 Appendix E step 7): line by line, inline box
    /// backgrounds and solid borders, then one [`ItemRole::Text`] path per
    /// text fragment in the text's `color`, clipped by the block's own
    /// overflow as well as its ancestors'. Text decorations are not painted.
    pub fn build_with_fonts(
        document: &Document,
        tree: &BoxTree,
        layout: &BlockLayout,
        viewport: CssSize,
        fonts: Fonts<'_>,
        limits: PaintLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, PaintError> {
        let resources = Resources {
            fonts,
            images: HashMap::new(),
            svg_styles: HashMap::new(),
        };
        Self::build_with(
            document,
            tree,
            layout,
            viewport,
            &resources,
            limits,
            cancellation,
        )
    }

    /// Like [`DisplayList::build_with_fonts`], also painting replaced
    /// content from `resources.images`: a block-level replaced element's
    /// image paints atomically at the start of its inline-content step
    /// (CSS 2.2 Appendix E step 7), stretched over its content box
    /// (`object-fit: fill`) and clipped by its ancestors' overflow.
    pub fn build_with(
        document: &Document,
        tree: &BoxTree,
        layout: &BlockLayout,
        viewport: CssSize,
        resources: &Resources<'_>,
        limits: PaintLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, PaintError> {
        let fonts = resources.fonts;
        let mut builder = Builder {
            commands: Vec::new(),
            limits,
            cancellation,
            fonts,
            svg_styles: &resources.svg_styles,
            paths: Vec::new(),
            path_points: 0,
            outlines: HashMap::new(),
            images: Vec::new(),
            clips: Vec::new(),
            rounded: Vec::new(),
            shadows: Vec::new(),
            filters: Vec::new(),
        };
        let mut image_index: HashMap<NodeId, usize> = HashMap::new();
        let fragments = layout.fragments();
        // Rendered legends, by node, for their fieldsets' borders.
        let legend_boxes: HashMap<NodeId, usize> = fragments
            .iter()
            .enumerate()
            .filter(|(_, f)| element_is(document, f.node(), "legend"))
            .map(|(i, f)| (f.node(), i))
            .collect();
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
                // The first `body` child of the root that generated a box.
                let body = fragments
                    .iter()
                    .position(|f| f.parent() == Some(0) && element_is(document, f.node(), "body"));
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

        // Classify every fragment and file it under its paint-order owner.
        let n = fragments.len();
        let mut roles = Vec::with_capacity(n);
        // Atomic inlines by the line they sit on, in tree order.
        let mut atomic_lines: Vec<Vec<usize>> = vec![Vec::new(); layout.lines().len()];
        let mut opacity = Vec::with_capacity(n);
        // Nearest stacking context, and nearest context/pseudo-context, at
        // or above each fragment (tree ancestry, not containing blocks).
        let mut context = Vec::with_capacity(n);
        let mut layer = Vec::with_capacity(n);
        let mut layers: Vec<Layers> = Vec::with_capacity(n);
        let mut tops = Vec::new();
        // Each box's transform, and the nearest transformed box at or above
        // it, whose local coordinates it paints in.
        let mut matrices: Vec<BoxTransform> = Vec::with_capacity(n);
        let mut space: Vec<Option<usize>> = Vec::with_capacity(n);
        let rect = |r: CssRect| {
            let (x0, y0, x1, y1) = edges(r);
            (x0, y0, x1 - x0, y1 - y0)
        };
        for (i, fragment) in fragments.iter().enumerate() {
            cancellation.check().map_err(|_| PaintError::Cancelled)?;
            let style = style_of(fragment)?;
            if canvas_source != Some(i) && has_image(style)? {
                return Err(PaintError::Unsupported {
                    node: fragment.node(),
                    reason: UnsupportedPaint::BackgroundImage,
                });
            }
            let alpha = match style.get(P::Opacity) {
                V::Opacity(o) if o.is_finite() => o.clamp(0.0, 1.0),
                _ => return Err(PaintError::InvalidInput),
            };
            let z = match style.get(P::ZIndex) {
                V::ZIndex(ZIndex::Auto) => None,
                V::ZIndex(ZIndex::Integer(z)) => Some(*z),
                _ => return Err(PaintError::InvalidInput),
            };
            let parent = match fragment.parent() {
                Some(p) if p < i => Some(p),
                Some(_) => return Err(PaintError::InvalidInput),
                None => None,
            };
            let positioned = matches!(
                fragment.scheme(),
                PositionScheme::Relative
                    | PositionScheme::Absolute
                    | PositionScheme::Fixed
                    | PositionScheme::Sticky
            );
            // The containing block's perspective applies to this box's
            // transform (CSS Transforms 2 §6).
            let perspective = match fragment.containing_block() {
                Some(c) if c < i => {
                    let f = &fragments[c];
                    perspective_of(style_of(f)?, rect(f.border()))
                        .map_err(|_| PaintError::InvalidInput)?
                }
                Some(_) => return Err(PaintError::InvalidInput),
                None => None,
            };
            let mut transform = box_transform(style, rect(fragment.border()), perspective)
                .map_err(|_| PaintError::InvalidInput)?;
            if let BoxTransform::Matrix(m) = transform {
                let hidden = matches!(
                    style.get(P::BackfaceVisibility),
                    V::BackfaceVisibility(BackfaceVisibility::Hidden)
                );
                if m.plane().inverse().is_none() || (hidden && m.back_facing()) {
                    transform = BoxTransform::Degenerate;
                }
            }
            let transformed = transform != BoxTransform::None;
            matrices.push(transform);
            space.push(if transformed {
                Some(i)
            } else {
                parent.and_then(|p| space[p])
            });
            // Fixed and sticky boxes always establish stacking contexts.
            let role = if parent.is_none()
                || (positioned && z.is_some())
                || matches!(
                    fragment.scheme(),
                    PositionScheme::Fixed | PositionScheme::Sticky
                )
                || alpha < 1.0
                || !matches!(style.get(P::ClipPath), V::ClipPath(ClipPath::None))
                || transformed
                || matches!(style.get(P::Perspective), V::Perspective(Some(_)))
                || !matches!(style.get(P::Filter), V::Filter(f) if f.is_empty())
                || !matches!(style.get(P::MixBlendMode), V::BlendMode(BlendMode::Normal))
                || matches!(style.get(P::Isolation), V::Isolation(Isolation::Isolate))
            {
                // z-index applies only to positioned boxes here.
                Role::Context(if positioned { z.unwrap_or(0) } else { 0 })
            } else if positioned {
                Role::Positioned
            } else if matches!(
                fragment.scheme(),
                PositionScheme::FloatLeft | PositionScheme::FloatRight
            ) {
                Role::Float
            } else if fragment.line().is_some() {
                Role::Atomic
            } else {
                Role::Block
            };
            roles.push(role);
            opacity.push(alpha);
            layers.push(Layers::default());
            let own_layer = !matches!(role, Role::Block);
            context.push(match (role, parent) {
                (Role::Context(_), _) | (_, None) => i,
                (_, Some(p)) => context[p],
            });
            layer.push(match parent {
                Some(p) if !own_layer => layer[p],
                _ => i,
            });
            let Some(p) = parent else {
                tops.push(i);
                continue;
            };
            match role {
                Role::Context(z) if z < 0 => layers[context[p]].negative.push(i),
                Role::Context(z) if z > 0 => layers[context[p]].positive.push(i),
                Role::Context(_) | Role::Positioned => layers[context[p]].zero.push(i),
                Role::Float => layers[layer[p]].floats.push(i),
                Role::Block => layers[layer[p]].blocks.push(i),
                Role::Atomic => {
                    let line = fragment.line().ok_or(PaintError::InvalidInput)?;
                    atomic_lines
                        .get_mut(line)
                        .ok_or(PaintError::InvalidInput)?
                        .push(i);
                }
            }
        }
        // Contexts that must isolate blending children, and those that
        // ask to.
        let mut isolates = vec![false; n];
        for (i, fragment) in fragments.iter().enumerate() {
            let style = style_of(fragment)?;
            if matches!(style.get(P::Isolation), V::Isolation(Isolation::Isolate)) {
                isolates[i] = true;
            }
            let blends = !matches!(style.get(P::MixBlendMode), V::BlendMode(BlendMode::Normal));
            if let (true, Some(p)) = (blends, fragment.parent()) {
                isolates[context[p]] = true;
            }
        }
        for l in &mut layers {
            cancellation.check().map_err(|_| PaintError::Cancelled)?;
            // Stable sorts keep tree order among equal z-index values.
            l.negative.sort_by_key(|&i| match roles[i] {
                Role::Context(z) => z,
                _ => 0,
            });
            l.positive.sort_by_key(|&i| match roles[i] {
                Role::Context(z) => z,
                _ => 0,
            });
        }

        // Inline content by block fragment: per line, inline boxes then text.
        let lines = layout.lines();
        let mut inline_lines: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (index, line) in lines.iter().enumerate() {
            inline_lines
                .get_mut(line.block())
                .ok_or(PaintError::InvalidInput)?
                .push(index);
        }
        let mut line_inlines: Vec<Vec<usize>> = vec![Vec::new(); lines.len()];
        for (index, inline) in layout.inlines().iter().enumerate() {
            line_inlines
                .get_mut(inline.line())
                .ok_or(PaintError::InvalidInput)?
                .push(index);
        }
        let mut line_texts: Vec<Vec<usize>> = vec![Vec::new(); lines.len()];
        for (index, text) in layout.texts().iter().enumerate() {
            line_texts
                .get_mut(text.line())
                .ok_or(PaintError::InvalidInput)?
                .push(index);
        }
        // Rounded border boxes, and the axis and rounded clips that apply
        // to each box (`local`, `outer`) and to its own content (`inner`),
        // following the containing-block chain within one coordinate space.
        // A transformed box paints in its own space, so the clips around it
        // go to its transform node instead (`transform_clip`); clips from a
        // containing block in another space do not apply. A box whose
        // overflow is clipped on both axes clips its content to its padding
        // edge curve (CSS Backgrounds 3 §5.3).
        let mut rounded = Vec::with_capacity(n);
        let mut local_clip: Vec<Clip> = Vec::with_capacity(n);
        let mut inner_axis: Vec<Clip> = Vec::with_capacity(n);
        let mut outer_clip: Vec<Option<usize>> = Vec::with_capacity(n);
        let mut inner_clip: Vec<Option<usize>> = Vec::with_capacity(n);
        let mut transform_clip: Vec<(Clip, Option<usize>)> = vec![(Clip::NONE, None); n];
        for (i, f) in fragments.iter().enumerate() {
            cancellation.check().map_err(|_| PaintError::Cancelled)?;
            let shape = rounded_box(style_of(f)?, f.border())?;
            let cb = match f.containing_block() {
                Some(c) if c < i => Some(c),
                Some(_) => return Err(PaintError::InvalidInput),
                None => None,
            };
            // The space the box sits in: its parent's.
            let around = f.parent().and_then(|p| space[p]);
            let rounded_parent = cb
                .filter(|&c| space[c] == around)
                .and_then(|c| inner_clip[c]);
            // Outside every transform the layout's clip is exact.
            let axis = match (around, cb) {
                (None, _) => f.clip(),
                (Some(_), Some(c)) if space[c] == around => inner_axis[c],
                _ => Clip::NONE,
            };
            let (axis, parent) = if matrices[i] == BoxTransform::None {
                (axis, rounded_parent)
            } else {
                transform_clip[i] = (axis, rounded_parent);
                (Clip::NONE, None)
            };
            local_clip.push(axis);
            inner_axis.push(content_clip(f, axis));
            let (ox, oy) = f.overflow();
            let inner = match shape {
                Some(r) if ox != Overflow::Visible && oy != Overflow::Visible => {
                    builder.rounded_clip(r.inset(f.padding()), parent)
                }
                _ => parent,
            };
            rounded.push(shape);
            outer_clip.push(parent);
            inner_clip.push(inner);
        }

        // Outlines by stacking context, in tree order: each block, then the
        // inline boxes of its lines.
        let mut outlined: Vec<Vec<Outlined>> = vec![Vec::new(); n];
        for i in 0..n {
            outlined[context[i]].push(Outlined::Block(i));
            for &line in &inline_lines[i] {
                outlined[context[i]]
                    .extend(line_inlines[line].iter().map(|&k| Outlined::Inline(k)));
            }
        }
        if !layout.texts().is_empty() && fonts.is_none() {
            return Err(PaintError::Font);
        }
        let box_style = |source: usize| -> Result<&ComputedStyle, PaintError> {
            tree.boxes()
                .get(source)
                .and_then(|b| b.style())
                .ok_or(PaintError::InvalidInput)
        };
        let visible = |style: &ComputedStyle| {
            matches!(style.get(P::Visibility), V::Visibility(Visibility::Visible))
        };

        // Expand layers with an explicit stack: no content-controlled recursion.
        let mut stack: Vec<Task> = tops.iter().rev().map(|&i| Task::Layer(i, false)).collect();
        let mut hits = Vec::new();
        let mut clip_paths: Vec<ClipPathNode> = Vec::new();
        let mut clip_path: Option<usize> = None;
        let mut transforms: Vec<TransformNode> = Vec::new();
        let mut spaces: Vec<Homography> = Vec::new();
        let mut transform: Option<usize> = None;
        while let Some(task) = stack.pop() {
            cancellation.check().map_err(|_| PaintError::Cancelled)?;
            match task {
                Task::End => builder.command(DisplayCommand::EndGroup)?,
                Task::EndClip(previous, opened) => {
                    clip_path = previous;
                    if opened {
                        builder.command(DisplayCommand::EndClip)?;
                    }
                }
                Task::EndTransform(previous, opened) => {
                    transform = previous;
                    if opened {
                        builder.command(DisplayCommand::EndTransform)?;
                    }
                }
                Task::Outlines(_, true) => {}
                Task::Outlines(c, false) => {
                    for &entry in &std::mem::take(&mut outlined[c]) {
                        let (geometry, source) = match entry {
                            Outlined::Block(b) => {
                                let f = &fragments[b];
                                (
                                    Geometry::block(f, local_clip[b], rounded[b], outer_clip[b]),
                                    f.source_box(),
                                )
                            }
                            Outlined::Inline(k) => {
                                let inline = &layout.inlines()[k];
                                let block = lines
                                    .get(inline.line())
                                    .ok_or(PaintError::InvalidInput)?
                                    .block();
                                let clip =
                                    *inner_axis.get(block).ok_or(PaintError::InvalidInput)?;
                                let shape =
                                    rounded_box(box_style(inline.source_box())?, inline.border())?;
                                (
                                    Geometry::inline(inline, clip, shape, inner_clip[block]),
                                    inline.source_box(),
                                )
                            }
                        };
                        let style = box_style(source)?;
                        if visible(style) {
                            builder.outline(geometry, source, style)?;
                        }
                    }
                }
                Task::Inline(blocks, from_block, from_line, muted) => {
                    'blocks: for (position, &b) in blocks.iter().enumerate().skip(from_block) {
                        let fragment = &fragments[b];
                        let resumed = position == from_block && from_line > 0;
                        let first_line = if position == from_block { from_line } else { 0 };
                        if let (false, Some(image)) =
                            (resumed, resources.images.get(&fragment.node()))
                        {
                            let style = style_of(fragment)?;
                            if !muted && visible(style) {
                                let index =
                                    *image_index.entry(fragment.node()).or_insert_with(|| {
                                        builder.images.push(Arc::clone(image));
                                        builder.images.len() - 1
                                    });
                                // Replaced content is clipped to the content
                                // edge curve.
                                let rounded_clip = match rounded[b] {
                                    Some(r) => builder
                                        .rounded_clip(r.inset(fragment.content()), outer_clip[b]),
                                    None => outer_clip[b],
                                };
                                builder.push(DisplayItem {
                                    source_box: fragment.source_box(),
                                    node: fragment.node(),
                                    role: ItemRole::Image,
                                    shape: Shape::Image {
                                        rect: fragment.content(),
                                        image: index,
                                    },
                                    color: Srgba::WHITE,
                                    clip: local_clip[b],
                                    rounded_clip,
                                })?;
                            }
                        }
                        let clip = inner_axis[b];
                        for (line_position, &line) in
                            inline_lines[b].iter().enumerate().skip(first_line)
                        {
                            for &k in &line_inlines[line] {
                                let inline = &layout.inlines()[k];
                                let style = box_style(inline.source_box())?;
                                if !visible(style) {
                                    continue;
                                }
                                let shape = rounded_box(style, inline.border())?;
                                let geometry = Geometry::inline(inline, clip, shape, inner_clip[b]);
                                hits.push(HitRegion {
                                    hit: Hit {
                                        source_box: inline.source_box(),
                                        node: inline.node(),
                                    },
                                    border: inline.border(),
                                    clip,
                                    rounded: shape,
                                    rounded_clip: inner_clip[b],
                                    clip_path,
                                    transform,
                                });
                                if !muted {
                                    let source = inline.source_box();
                                    builder.shadows(geometry, source, style, false)?;
                                    builder.background(geometry, source, style)?;
                                    builder.shadows(geometry, source, style, true)?;
                                    builder.border(geometry, source, style, None)?;
                                }
                            }
                            // Atomic inlines paint after their line, each
                            // atomically; the rest of the content resumes
                            // after them.
                            let atomics = &atomic_lines[line];
                            let resume = (!atomics.is_empty()).then(|| {
                                Task::Inline(blocks.clone(), position, line_position + 1, muted)
                            });
                            for &t in line_texts[line].iter().filter(|_| !muted) {
                                let text = &layout.texts()[t];
                                let style = box_style(text.source_box())?;
                                if visible(style) {
                                    let lines = decorations(tree, text.source_box())?;
                                    // Underlines and overlines under the text,
                                    // line-throughs over it (CSS Text Decoration 3 §2.1).
                                    for over in [false, true] {
                                        if over {
                                            builder.text(text, style, clip, inner_clip[b])?;
                                        }
                                        for line in lines
                                            .iter()
                                            .filter(|l| (l.kind == Line::Through) == over)
                                        {
                                            builder.decoration(text, *line, clip, inner_clip[b])?;
                                        }
                                    }
                                }
                            }
                            if let Some(resume) = resume {
                                stack.push(resume);
                                stack.extend(atomics.iter().rev().map(|&a| Task::Layer(a, muted)));
                                break 'blocks;
                            }
                        }
                    }
                }
                Task::Own(i, muted) => {
                    let fragment = &fragments[i];
                    let style = style_of(fragment)?;
                    if matches!(style.get(P::Visibility), V::Visibility(Visibility::Visible)) {
                        hits.push(HitRegion {
                            hit: Hit {
                                source_box: fragment.source_box(),
                                node: fragment.node(),
                            },
                            border: fragment.border(),
                            clip: local_clip[i],
                            rounded: rounded[i],
                            rounded_clip: outer_clip[i],
                            clip_path,
                            transform,
                        });
                        if muted {
                            continue;
                        }
                        let mut geometry =
                            Geometry::block(fragment, local_clip[i], rounded[i], outer_clip[i]);
                        if let Some(legend) = rendered_legend(document, fragment.node())
                            .and_then(|legend| legend_boxes.get(&legend))
                        {
                            geometry = geometry.around_legend(fragments[*legend].border());
                        }
                        let source = fragment.source_box();
                        if fragment.hides_background_and_border() {
                            continue;
                        }
                        builder.shadows(geometry, source, style, false)?;
                        if canvas_source != Some(i) {
                            builder.background(geometry, source, style)?;
                        }
                        if visible(style) {
                            let checked = tree.checked(fragment.node());
                            builder.control(document, geometry, source, checked)?;
                            builder.svg(document, geometry, source, style)?;
                        }
                        builder.shadows(geometry, source, style, true)?;
                        let collapsed = match fragment.collapsed_borders() {
                            Some(list) => {
                                let mut out = [None; 4];
                                for (slot, side) in out.iter_mut().zip(list) {
                                    if let Some((b, s)) = side {
                                        *slot = Some((box_style(b)?, s));
                                    }
                                }
                                Some(out)
                            }
                            None => None,
                        };
                        builder.border(geometry, fragment.source_box(), style, collapsed)?;
                    }
                }
                Task::Layer(i, muted) => {
                    // A box whose transform renders nothing, or whose hidden
                    // back faces the viewer, paints and hits nothing.
                    let matrix = match matrices[i] {
                        BoxTransform::Degenerate => continue,
                        BoxTransform::None => None,
                        BoxTransform::Matrix(m) => Some(m.plane()),
                    };
                    // `opacity: 0` paints nothing but still takes hits.
                    let muted = muted || opacity[i] <= 0.0;
                    if let Some(matrix) = matrix {
                        let (clip, rounded_clip) = transform_clip[i];
                        let outer = transform.map_or(Homography::IDENTITY, |t| spaces[t]);
                        transforms.push(TransformNode {
                            matrix,
                            clip,
                            rounded_clip,
                            parent: transform,
                        });
                        spaces.push(outer.then_after(&matrix));
                        let index = transforms.len() - 1;
                        if !muted {
                            builder.command(DisplayCommand::BeginTransform {
                                source_box: fragments[i].source_box(),
                                node: fragments[i].node(),
                                transform: index,
                            })?;
                        }
                        stack.push(Task::EndTransform(transform, !muted));
                        transform = Some(index);
                    }
                    let style = style_of(&fragments[i])?;
                    let blend = match style.get(P::MixBlendMode) {
                        V::BlendMode(b) => *b,
                        _ => return Err(PaintError::InvalidInput),
                    };
                    // A group isolates blending children, so they blend
                    // only with their stacking context (Compositing 1 §3.2).
                    let group =
                        !muted && (opacity[i] < 1.0 || blend != BlendMode::Normal || isolates[i]);
                    if group {
                        builder.command(DisplayCommand::BeginGroup {
                            source_box: fragments[i].source_box(),
                            node: fragments[i].node(),
                            opacity: opacity[i],
                            blend,
                            filter: None,
                        })?;
                        stack.push(Task::End);
                    }
                    // Filters apply before clipping (Filter Effects 1 §4).
                    let filter = builder.filter(fragments[i].node(), style)?;
                    if let V::ClipPath(path) = style.get(P::ClipPath) {
                        if let Some((shape, area)) =
                            builder.clip_path(&fragments[i], style, path)?
                        {
                            if !muted {
                                builder.command(DisplayCommand::BeginClip {
                                    source_box: fragments[i].source_box(),
                                    node: fragments[i].node(),
                                    shape,
                                })?;
                            }
                            stack.push(Task::EndClip(clip_path, !muted));
                            clip_paths.push(ClipPathNode {
                                shape: area,
                                parent: clip_path,
                                transform,
                            });
                            clip_path = Some(clip_paths.len() - 1);
                        }
                    }
                    if let (Some(filter), false) = (filter, muted) {
                        builder.command(DisplayCommand::BeginGroup {
                            source_box: fragments[i].source_box(),
                            node: fragments[i].node(),
                            opacity: 1.0,
                            blend: BlendMode::Normal,
                            filter: Some(filter),
                        })?;
                        stack.push(Task::End);
                    }
                    if context[i] == i {
                        stack.push(Task::Outlines(i, muted));
                    }
                    let l = std::mem::take(&mut layers[i]);
                    // Pushed in reverse so they pop in paint order.
                    stack.extend(l.positive.iter().rev().map(|&c| Task::Layer(c, muted)));
                    stack.extend(l.zero.iter().rev().map(|&c| Task::Layer(c, muted)));
                    // The layer root's inline content, then its blocks'.
                    let mut inline = Vec::with_capacity(l.blocks.len() + 1);
                    inline.push(i);
                    inline.extend(&l.blocks);
                    stack.push(Task::Inline(inline, 0, 0, muted));
                    stack.extend(l.floats.iter().rev().map(|&c| Task::Layer(c, muted)));
                    stack.extend(l.blocks.iter().rev().map(|&c| Task::Own(c, muted)));
                    stack.extend(l.negative.iter().rev().map(|&c| Task::Layer(c, muted)));
                    stack.push(Task::Own(i, muted));
                }
            }
        }
        let root = fragments
            .first()
            .filter(|f| f.parent().is_none())
            .map(|f| Hit {
                source_box: f.source_box(),
                node: f.node(),
            });
        Ok(Self {
            viewport,
            canvas,
            commands: builder.commands,
            paths: builder.paths,
            images: builder.images,
            clips: builder.clips,
            rounded: builder.rounded,
            shadows: builder.shadows,
            filters: builder.filters,
            clip_paths,
            transforms,
            spaces,
            hits,
            root,
        })
    }

    /// The viewport size the list was built for, in CSS pixels.
    pub const fn viewport(&self) -> CssSize {
        self.viewport
    }

    /// The color the whole canvas is cleared to: opaque, or fully
    /// transparent for an overlay layer ([`Self::over_transparent`]).
    pub const fn canvas(&self) -> Srgba {
        self.canvas
    }

    /// Paint commands, in paint order.
    pub fn commands(&self) -> &[DisplayCommand] {
        &self.commands
    }

    /// This list with only the commands `keep` accepts (by index), side
    /// tables kept whole so every index stays valid (scene layers, ADR
    /// 0064). The caller keeps groups, clips and transforms balanced.
    pub fn retain(&self, mut keep: impl FnMut(usize, &DisplayCommand) -> bool) -> Self {
        let mut list = self.clone();
        list.commands = self
            .commands
            .iter()
            .enumerate()
            .filter(|(index, command)| keep(*index, command))
            .map(|(_, command)| *command)
            .collect();
        list
    }

    /// This list over a transparent canvas, for an overlay layer.
    pub fn over_transparent(mut self) -> Self {
        self.canvas = Srgba {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        };
        self
    }

    /// The topmost box whose border box contains `point` (CSS pixels from the
    /// viewport origin) and whose overflow clips do not exclude it, in paint
    /// order, as `document.elementFromPoint` would find it among blocks.
    /// `visibility: hidden` boxes are skipped (their visible descendants are
    /// not); `opacity: 0` boxes still take hits. A point inside the viewport
    /// that hits no box finds the root element; a point outside it, or a
    /// list with no boxes, finds nothing. Edges are half-open: a box covers
    /// `left <= x < right` and `top <= y < bottom`.
    pub fn hit_test(&self, point: CssPoint) -> Option<Hit> {
        let (x, y) = (point.x.get(), point.y.get());
        let (width, height) = (self.viewport.width().get(), self.viewport.height().get());
        if !(0.0..width).contains(&x) || !(0.0..height).contains(&y) {
            return None;
        }
        self.hits
            .iter()
            .rev()
            .find(|region| self.hits_region(region, x, y))
            .map(|region| region.hit)
            .or(self.root)
    }

    /// `(x, y)` in the viewport mapped into the local coordinates of
    /// `transform`, or `None` where no point of that plane shows there.
    fn to_space(&self, transform: Option<usize>, x: f64, y: f64) -> Option<(f64, f64)> {
        match transform {
            None => Some((x, y)),
            Some(t) => self.spaces.get(t)?.unapply(x, y),
        }
    }

    /// Whether the viewport point `(x, y)` hits `region`: inside its box and
    /// its clips, every enclosing transform's clips and every enclosing
    /// clip path, each tested in its own coordinates.
    fn hits_region(&self, region: &HitRegion, x: f64, y: f64) -> bool {
        let Some((lx, ly)) = self.to_space(region.transform, x, y) else {
            return false;
        };
        if !region.contains(lx, ly, &self.clips) {
            return false;
        }
        let mut transform = region.transform;
        while let Some(node) = transform.and_then(|t| self.transforms.get(t)) {
            let Some((px, py)) = self.to_space(node.parent, x, y) else {
                return false;
            };
            if !in_axis_clip(node.clip, px, py) || !in_clips(&self.clips, node.rounded_clip, px, py)
            {
                return false;
            }
            transform = node.parent;
        }
        let mut clip_path = region.clip_path;
        while let Some(node) = clip_path.and_then(|c| self.clip_paths.get(c)) {
            let Some((px, py)) = self.to_space(node.transform, x, y) else {
                return false;
            };
            let inside = match &node.shape {
                Resolved::Rounded(r) => r.contains(px, py),
                Resolved::Polygon(points) => polygon_contains(points, px, py),
            };
            if !inside {
                return false;
            }
            clip_path = node.parent;
        }
        true
    }

    /// Transformed stacking contexts, indexed by
    /// [`DisplayCommand::BeginTransform`].
    pub fn transforms(&self) -> &[TransformNode] {
        &self.transforms
    }

    /// Rounded rectangles, indexed by [`Shape::Rounded`] and
    /// [`Shape::RoundedRing`].
    pub fn rounded(&self) -> &[RoundedRect] {
        &self.rounded
    }

    /// Filter chains, indexed by [`DisplayCommand::BeginGroup`].
    pub fn filters(&self) -> &[Vec<FilterOp>] {
        &self.filters
    }

    /// Box shadows, indexed by [`Shape::Shadow`].
    pub fn shadows(&self) -> &[BoxShadow] {
        &self.shadows
    }

    /// Rounded clips, indexed by [`DisplayItem::rounded_clip`].
    pub fn clips(&self) -> &[ClipNode] {
        &self.clips
    }

    /// Glyph paths, indexed by [`Shape::Path`].
    pub fn paths(&self) -> &[Path] {
        &self.paths
    }

    /// Images, indexed by [`Shape::Image`].
    pub fn images(&self) -> &[Arc<Image>] {
        &self.images
    }

    /// Copies of the fills alone, in paint order (group markers skipped).
    pub fn items(&self) -> Vec<DisplayItem> {
        self.commands
            .iter()
            .filter_map(|c| match c {
                DisplayCommand::Fill(item) => Some(*item),
                _ => None,
            })
            .collect()
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
        let rounded = |s: &RoundedRect| {
            let mut text = format!("({}) radii=(", r(s.rect));
            for (n, c) in s.corners.iter().enumerate() {
                if n > 0 {
                    text.push(' ');
                }
                // Writing to a String cannot fail.
                let _ = write!(text, "{}/{}", c.x, c.y);
            }
            text.push(')');
            text
        };
        for (n, node) in self.clips.iter().enumerate() {
            let parent = node
                .parent
                .map_or_else(String::new, |p| format!(" parent={p}"));
            line = format!("rclip {n} rounded={}{parent}\n", rounded(&node.shape));
            append(&mut out, &line, max_bytes)?;
        }
        let shape_text = |shape: Shape| match shape {
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
            Shape::Image { rect, image } => {
                let data = &self.images[image];
                format!(
                    "image={image} rect=({}) size={}x{}",
                    r(rect),
                    data.width,
                    data.height
                )
            }
            Shape::Rounded(shape) => format!("rounded={}", rounded(&self.rounded[shape])),
            Shape::Shadow(index) => {
                let s = &self.shadows[index];
                let shape = s.shape.map_or_else(|| "none".to_owned(), |r| rounded(&r));
                format!(
                    "shadow={shape} sigma={} {}={}",
                    s.sigma,
                    if s.inset { "inside" } else { "outside" },
                    rounded(&s.clip)
                )
            }
            Shape::RoundedRing { outer, inner } => format!(
                "rounded-ring={} inner={}",
                rounded(&self.rounded[outer]),
                rounded(&self.rounded[inner])
            ),
            Shape::Path(index) => {
                let path = &self.paths[index];
                format!(
                    "path={index} contours={} points={}",
                    path.contours.len(),
                    path.points()
                )
            }
        };
        let mut index = 0usize;
        for command in &self.commands {
            let item = match command {
                DisplayCommand::Fill(item) => item,
                DisplayCommand::BeginGroup {
                    source_box,
                    node,
                    opacity,
                    blend,
                    filter,
                } => {
                    let blend = if *blend == BlendMode::Normal {
                        String::new()
                    } else {
                        format!(" blend={blend}")
                    };
                    let filter = filter.map_or_else(String::new, |f| {
                        let mut text = String::from(" filter=(");
                        for (n, op) in self.filters[f].iter().enumerate() {
                            if n > 0 {
                                text.push(' ');
                            }
                            // Writing to a String cannot fail.
                            let _ = match op {
                                FilterOp::Blur(s) => write!(text, "blur:{s}"),
                                FilterOp::Color(m) => write!(text, "color:{m:?}"),
                                FilterOp::DropShadow {
                                    dx,
                                    dy,
                                    sigma,
                                    color,
                                } => write!(text, "shadow:{dx},{dy},{sigma},{}", hex(*color)),
                            };
                        }
                        text.push(')');
                        text
                    });
                    line = format!(
                        "group box={source_box} node={}:{} opacity={opacity}{blend}{filter}\n",
                        node.slot(),
                        node.generation()
                    );
                    append(&mut out, &line, max_bytes)?;
                    continue;
                }
                DisplayCommand::EndGroup => {
                    append(&mut out, "end-group\n", max_bytes)?;
                    continue;
                }
                DisplayCommand::BeginClip {
                    source_box,
                    node,
                    shape,
                } => {
                    line = format!(
                        "clip-path box={source_box} node={}:{} {}\n",
                        node.slot(),
                        node.generation(),
                        shape_text(*shape)
                    );
                    append(&mut out, &line, max_bytes)?;
                    continue;
                }
                DisplayCommand::EndClip => {
                    append(&mut out, "end-clip-path\n", max_bytes)?;
                    continue;
                }
                DisplayCommand::BeginTransform {
                    source_box,
                    node,
                    transform,
                } => {
                    let t = &self.transforms[*transform];
                    let mut matrix = String::new();
                    for (n, v) in t.matrix.0.iter().enumerate() {
                        let separator = match n {
                            0 => "",
                            3 | 6 => "; ",
                            _ => " ",
                        };
                        // Writing to a String cannot fail.
                        let _ = write!(matrix, "{separator}{v}");
                    }
                    let clip = if t.clip.is_none() {
                        String::new()
                    } else {
                        format!(" clip={}", t.clip)
                    };
                    let clip = match t.rounded_clip {
                        Some(n) => format!("{clip} rclip={n}"),
                        None => clip,
                    };
                    line = format!(
                        "transform box={source_box} node={}:{} matrix=({matrix}){clip}\n",
                        node.slot(),
                        node.generation()
                    );
                    append(&mut out, &line, max_bytes)?;
                    continue;
                }
                DisplayCommand::EndTransform => {
                    append(&mut out, "end-transform\n", max_bytes)?;
                    continue;
                }
            };
            let i = index;
            index += 1;
            let role = match item.role {
                ItemRole::Background => "background",
                ItemRole::Border => "border",
                ItemRole::Text => "text",
                ItemRole::Image => "image",
                ItemRole::Decoration => "decoration",
                ItemRole::Outline => "outline",
                ItemRole::Shadow => "shadow",
            };
            let shape = shape_text(item.shape);
            // Unclipped items keep the REN-04a line format.
            let clip = if item.clip.is_none() {
                String::new()
            } else {
                format!(" clip={}", item.clip)
            };
            let clip = match item.rounded_clip {
                Some(n) => format!("{clip} rclip={n}"),
                None => clip,
            };
            line = format!(
                "item {i} box={} node={}:{} {role} {shape} color={}{clip}\n",
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
/// The shaded side of an `inset` or `outset` border: two thirds of each
/// channel, alpha kept (black stays black).
fn darker(c: Srgba) -> Srgba {
    let scale = |v: u8| ((u16::from(v) * 2 + 1) / 3) as u8;
    Srgba {
        r: scale(c.r),
        g: scale(c.g),
        b: scale(c.b),
        a: c.a,
    }
}
