//! Immutable display lists built from block fragments: CSS 2.2 Appendix E
//! paint order with stacking contexts, z-index, floats and positioned boxes;
//! overflow clips; opacity groups; CSS Backgrounds 3 §2.11 canvas propagation.

use crate::color::{Srgba, srgba};
use rustic_base::CancellationToken;
use rustic_css::{
    BorderStyle, Image, PropertyId as P, PropertyValue as V, Visibility, VisualBox, ZIndex,
};
use rustic_dom::{Document, HTML_NAMESPACE, NodeId, NodeKind};
use rustic_layout::{
    BlockFragment, BlockLayout, BoxTree, Clip, CssPoint, CssPx, CssRect, CssSize, PositionScheme,
};
use rustic_style::ComputedStyle;
use std::{error::Error, fmt, fmt::Write as _};

/// Display list budgets, independent of layout limits.
#[derive(Clone, Copy, Debug)]
pub struct PaintLimits {
    /// Maximum display commands (fills plus group begin/end markers).
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
    /// Ancestor overflow clip in CSS pixels (see [`BlockFragment::clip`]).
    pub clip: Clip,
}

/// One display list command, in paint order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DisplayCommand {
    /// Fill one shape source-over into the current group.
    Fill(DisplayItem),
    /// Start a transparent group for a stacking context with `opacity < 1`.
    BeginGroup {
        /// Principal-box index of the stacking context root.
        source_box: usize,
        /// Source DOM node.
        node: NodeId,
        /// Group opacity, greater than 0 and less than 1.
        opacity: f64,
    },
    /// Composite the innermost open group into its parent with its opacity.
    EndGroup,
}

/// Why a box needs paint features this reference does not implement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedPaint {
    /// A `background-image` other than `none`.
    BackgroundImage,
    /// A visible border style other than `solid`.
    BorderStyle,
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
    /// More display commands than [`PaintLimits::max_items`].
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
    commands: Vec<DisplayCommand>,
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
    commands: Vec<DisplayCommand>,
    limits: PaintLimits,
    cancellation: &'a CancellationToken,
}

impl Builder<'_> {
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
            clip: fragment.clip(),
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
            clip: fragment.clip(),
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

enum Task {
    Layer(usize),
    Own(usize),
    End,
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
    /// Unsupported features fail atomically.
    pub fn build(
        document: &Document,
        tree: &BoxTree,
        layout: &BlockLayout,
        viewport: CssSize,
        limits: PaintLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, PaintError> {
        let mut builder = Builder {
            commands: Vec::new(),
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
        let mut opacity = Vec::with_capacity(n);
        // Nearest stacking context, and nearest context/pseudo-context, at
        // or above each fragment (tree ancestry, not containing blocks).
        let mut context = Vec::with_capacity(n);
        let mut layer = Vec::with_capacity(n);
        let mut layers: Vec<Layers> = Vec::with_capacity(n);
        let mut tops = Vec::new();
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
                PositionScheme::Relative | PositionScheme::Absolute | PositionScheme::Fixed
            );
            let role = if parent.is_none()
                || (positioned && z.is_some())
                || fragment.scheme() == PositionScheme::Fixed
                || alpha < 1.0
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

        // Expand layers with an explicit stack: no content-controlled recursion.
        let mut stack: Vec<Task> = tops.iter().rev().map(|&i| Task::Layer(i)).collect();
        while let Some(task) = stack.pop() {
            cancellation.check().map_err(|_| PaintError::Cancelled)?;
            match task {
                Task::End => builder.command(DisplayCommand::EndGroup)?,
                Task::Own(i) => {
                    let fragment = &fragments[i];
                    let style = style_of(fragment)?;
                    if matches!(style.get(P::Visibility), V::Visibility(Visibility::Visible)) {
                        if canvas_source != Some(i) {
                            builder.background(fragment, fragment.source_box(), style)?;
                        }
                        builder.border(fragment, fragment.source_box(), style)?;
                    }
                }
                Task::Layer(i) => {
                    if opacity[i] <= 0.0 {
                        continue;
                    }
                    let group = opacity[i] < 1.0;
                    if group {
                        builder.command(DisplayCommand::BeginGroup {
                            source_box: fragments[i].source_box(),
                            node: fragments[i].node(),
                            opacity: opacity[i],
                        })?;
                        stack.push(Task::End);
                    }
                    let l = std::mem::take(&mut layers[i]);
                    // Pushed in reverse so they pop in paint order.
                    stack.extend(l.positive.iter().rev().map(|&c| Task::Layer(c)));
                    stack.extend(l.zero.iter().rev().map(|&c| Task::Layer(c)));
                    stack.extend(l.floats.iter().rev().map(|&c| Task::Layer(c)));
                    stack.extend(l.blocks.iter().rev().map(|&c| Task::Own(c)));
                    stack.extend(l.negative.iter().rev().map(|&c| Task::Layer(c)));
                    stack.push(Task::Own(i));
                }
            }
        }
        Ok(Self {
            viewport,
            canvas,
            commands: builder.commands,
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

    /// Paint commands, in paint order.
    pub fn commands(&self) -> &[DisplayCommand] {
        &self.commands
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
        let mut index = 0usize;
        for command in &self.commands {
            let item = match command {
                DisplayCommand::Fill(item) => item,
                DisplayCommand::BeginGroup {
                    source_box,
                    node,
                    opacity,
                } => {
                    line = format!(
                        "group box={source_box} node={}:{} opacity={opacity}\n",
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
            };
            let i = index;
            index += 1;
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
            // Unclipped items keep the REN-04a line format.
            let clip = if item.clip.is_none() {
                String::new()
            } else {
                format!(" clip={}", item.clip)
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
