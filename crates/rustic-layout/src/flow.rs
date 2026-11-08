//! Iterative vertical flow for an explicit, margin-free block subset.
//!
//! Unsupported content fails atomically instead of being assigned invented
//! geometry. Inline/anonymous boxes and margin collapse arrive in later units.

use rustic_base::CancellationToken;
use rustic_css::{
    Direction, Display, Float, Overflow, Position, PropertyId as P, PropertyValue as V, WhiteSpace,
};
use rustic_dom::NodeId;
use rustic_style::ComputedStyle;
use std::{error::Error, fmt};

use crate::height::HeightConstraint;
use crate::{
    BlockMeasure, BoxKind, BoxSnapshot, BoxTree, CssPoint, CssPx, CssRect, CssSize, Edges,
    GeometryError, LayoutLimits, MeasureError, measure_block_width,
};

/// Why a principal box needs an unimplemented formatting algorithm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedFlow {
    /// Inline formatting, text shaping, flex/grid/table/list layout or another context.
    FormattingContext,
    /// Text is not an entirely collapsible whitespace run under normal/nowrap.
    TextLayout,
    /// Replaced elements, native controls, foreign content or special HTML layout.
    SpecialElement,
    /// Floats or non-static positioning.
    Positioning,
    /// Nonzero vertical margins need adjoining/parent-child collapse.
    VerticalMargins,
    /// Scrolling, clipping or overflow propagation.
    Overflow,
}

/// One non-replaced block's immutable, physical used geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockFragment {
    node: NodeId,
    source_box: usize,
    parent: Option<usize>,
    content: CssRect,
    padding: CssRect,
    border: CssRect,
    margin: Edges,
}

impl BlockFragment {
    /// Source document node (identity is not a strong DOM reference).
    pub const fn node(self) -> NodeId {
        self.node
    }
    /// Principal-box index in the input snapshot.
    pub const fn source_box(self) -> usize {
        self.source_box
    }
    /// Parent fragment index in this result; `None` for a top-level block.
    pub const fn parent(self) -> Option<usize> {
        self.parent
    }
    /// Used content rectangle in CSS pixels.
    pub const fn content(self) -> CssRect {
        self.content
    }
    /// Used padding rectangle in CSS pixels.
    pub const fn padding(self) -> CssRect {
        self.padding
    }
    /// Used border rectangle in CSS pixels.
    pub const fn border(self) -> CssRect {
        self.border
    }
    /// Used physical margins; vertical margins are zero in this subset.
    pub const fn margin(self) -> Edges {
        self.margin
    }
}

/// A deterministic, block-only reference layout. No glyphs or pixels yet.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockLayout {
    fragments: Vec<BlockFragment>,
}

struct Frame<'a> {
    source: usize,
    fragment: usize,
    parent: Option<usize>,
    node: NodeId,
    style: &'a ComputedStyle,
    measure: BlockMeasure,
    border_origin: CssPoint,
    content_origin: CssPoint,
    height: HeightConstraint,
    cursor: CssPx,
}

fn direction(style: &ComputedStyle) -> Result<Direction, FlowError> {
    match style.get(P::Direction) {
        V::Direction(value) => Ok(*value),
        _ => Err(FlowError::InvalidSnapshot),
    }
}

fn supported_box(b: &BoxSnapshot) -> Result<&ComputedStyle, FlowError> {
    let fail = |reason| FlowError::Unsupported {
        node: b.node(),
        reason,
    };
    if b.requires_special_layout() {
        return Err(fail(UnsupportedFlow::SpecialElement));
    }
    if !matches!(
        b.kind(),
        BoxKind::Element(Display::Block | Display::FlowRoot)
    ) {
        return Err(fail(UnsupportedFlow::FormattingContext));
    }
    let style = b.style().ok_or(FlowError::InvalidSnapshot)?;
    if !matches!(style.get(P::Position), V::Position(Position::Static))
        || !matches!(style.get(P::Float), V::Float(Float::None))
    {
        return Err(fail(UnsupportedFlow::Positioning));
    }
    for p in [P::OverflowX, P::OverflowY] {
        if !matches!(style.get(p), V::Overflow(Overflow::Visible)) {
            return Err(fail(UnsupportedFlow::Overflow));
        }
    }
    Ok(style)
}

fn ignorable_text(b: &BoxSnapshot, cancellation: &CancellationToken) -> Result<bool, FlowError> {
    let style = b.style().ok_or(FlowError::InvalidSnapshot)?;
    if !matches!(
        style.get(P::WhiteSpace),
        V::WhiteSpace(WhiteSpace::Normal | WhiteSpace::Nowrap)
    ) {
        return Ok(false);
    }
    // Check cancellation inside a large text run too, not only once per node.
    for chunk in b.text().chunks(1024) {
        cancellation.check().map_err(|_| FlowError::Cancelled)?;
        if !chunk.iter().all(|u| matches!(u, 9 | 10 | 12 | 13 | 32)) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn open<'a>(
    b: &'a BoxSnapshot,
    source: usize,
    fragment: usize,
    parent: Option<&Frame<'_>>,
    root_cursor: CssPx,
    viewport: CssSize,
) -> Result<Frame<'a>, FlowError> {
    let style = supported_box(b)?;
    let (width, basis_height, containing_direction, origin) = match parent {
        Some(parent) => (
            parent.measure.content_width,
            parent.height.definite(),
            direction(parent.style)?,
            CssPoint {
                x: parent.content_origin.x,
                y: parent.content_origin.y.checked_add(parent.cursor)?,
            },
        ),
        None => (
            viewport.width(),
            Some(viewport.height()),
            direction(style)?,
            CssPoint {
                x: CssPx::ZERO,
                y: root_cursor,
            },
        ),
    };
    let measure = measure_block_width(style, width, containing_direction)?;
    if measure.margin.top != CssPx::ZERO || measure.margin.bottom != CssPx::ZERO {
        return Err(FlowError::Unsupported {
            node: b.node(),
            reason: UnsupportedFlow::VerticalMargins,
        });
    }
    let border_origin = CssPoint {
        x: origin.x.checked_add(measure.margin.left)?,
        y: origin.y,
    };
    let content_origin = CssPoint {
        x: border_origin
            .x
            .checked_add(measure.border.left)?
            .checked_add(measure.padding.left)?,
        y: border_origin
            .y
            .checked_add(measure.border.top)?
            .checked_add(measure.padding.top)?,
    };
    let extras = measure
        .border
        .top
        .checked_add(measure.padding.top)?
        .checked_add(measure.padding.bottom)?
        .checked_add(measure.border.bottom)?;
    let height = HeightConstraint::new(style, basis_height, extras)?;
    Ok(Frame {
        source,
        fragment,
        parent: parent.map(|f| f.fragment),
        node: b.node(),
        style,
        measure,
        border_origin,
        content_origin,
        height,
        cursor: CssPx::ZERO,
    })
}

fn finish(frame: Frame<'_>) -> Result<BlockFragment, FlowError> {
    let m = frame.measure;
    let h = frame.height.used(frame.cursor);
    let content = CssRect::new(
        frame.content_origin,
        CssSize::new(m.content_width.get(), h.get())?,
    )?;
    let padding_size = CssSize::new(
        m.content_width
            .checked_add(m.padding.left)?
            .checked_add(m.padding.right)?
            .get(),
        h.checked_add(m.padding.top)?
            .checked_add(m.padding.bottom)?
            .get(),
    )?;
    let padding_origin = CssPoint {
        x: frame.border_origin.x.checked_add(m.border.left)?,
        y: frame.border_origin.y.checked_add(m.border.top)?,
    };
    let padding = CssRect::new(padding_origin, padding_size)?;
    let border_size = CssSize::new(
        padding_size
            .width()
            .checked_add(m.border.left)?
            .checked_add(m.border.right)?
            .get(),
        padding_size
            .height()
            .checked_add(m.border.top)?
            .checked_add(m.border.bottom)?
            .get(),
    )?;
    let border = CssRect::new(frame.border_origin, border_size)?;
    Ok(BlockFragment {
        node: frame.node,
        source_box: frame.source,
        parent: frame.parent,
        content,
        padding,
        border,
        margin: m.margin,
    })
}

fn close(
    stack: &mut Vec<Frame<'_>>,
    fragments: &mut [Option<BlockFragment>],
    root_cursor: &mut CssPx,
) -> Result<(), FlowError> {
    let frame = stack.pop().ok_or(FlowError::InvalidSnapshot)?;
    let index = frame.fragment;
    let fragment = finish(frame)?;
    let cursor = match stack.last_mut() {
        Some(parent) => &mut parent.cursor,
        None => root_cursor,
    };
    *cursor = cursor.checked_add(fragment.border.size().height())?;
    *fragments.get_mut(index).ok_or(FlowError::InvalidSnapshot)? = Some(fragment);
    Ok(())
}

impl BlockLayout {
    /// Lay out a principal snapshot in CSS pixels without touching the DOM.
    ///
    /// Supports static, non-replaced block/flow-root boxes with visible overflow
    /// and zero vertical margins. Auto/fixed/min/max heights and percentages
    /// against definite containing heights are resolved. Pure collapsible
    /// whitespace runs under normal/nowrap produce no fragment; all other text,
    /// inline contexts and unsupported semantics return a typed error (TD-38).
    /// Callers must compute styles for this same viewport before calling.
    /// Limits and cancellation apply independently of snapshot construction.
    pub fn compute(
        tree: &BoxTree,
        viewport: CssSize,
        limits: LayoutLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, FlowError> {
        cancellation.check().map_err(|_| FlowError::Cancelled)?;
        if tree.boxes().len() > limits.max_nodes {
            return Err(FlowError::NodeLimit);
        }
        let mut fragments = Vec::new();
        let mut stack: Vec<Frame<'_>> = Vec::new();
        let mut root_cursor = CssPx::ZERO;
        let mut text_units = 0usize;
        for (source, b) in tree.boxes().iter().enumerate() {
            cancellation.check().map_err(|_| FlowError::Cancelled)?;
            while stack.last().is_some_and(|f| Some(f.source) != b.parent()) {
                cancellation.check().map_err(|_| FlowError::Cancelled)?;
                close(&mut stack, &mut fragments, &mut root_cursor)?;
            }
            if stack.last().map(|f| f.source) != b.parent() {
                return Err(FlowError::InvalidSnapshot);
            }
            if stack.len() > limits.max_depth {
                return Err(FlowError::DepthLimit);
            }
            if b.kind() == BoxKind::Text {
                text_units = text_units
                    .checked_add(b.text().len())
                    .filter(|&n| n <= limits.max_text_units)
                    .ok_or(FlowError::TextLimit)?;
                if ignorable_text(b, cancellation)? {
                    continue;
                }
                return Err(FlowError::Unsupported {
                    node: b.node(),
                    reason: UnsupportedFlow::TextLayout,
                });
            }
            if fragments.len() >= limits.max_boxes {
                return Err(FlowError::BoxLimit);
            }
            let frame = open(
                b,
                source,
                fragments.len(),
                stack.last(),
                root_cursor,
                viewport,
            )?;
            fragments.push(None);
            stack.push(frame);
        }
        while !stack.is_empty() {
            cancellation.check().map_err(|_| FlowError::Cancelled)?;
            close(&mut stack, &mut fragments, &mut root_cursor)?;
        }
        Ok(Self {
            fragments: fragments
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or(FlowError::InvalidSnapshot)?,
        })
    }

    /// Physical block fragments, in principal-box preorder.
    pub fn fragments(&self) -> &[BlockFragment] {
        &self.fragments
    }

    /// Deterministic used-geometry dump, with no partial output on byte overflow.
    pub fn dump(&self, max_bytes: usize) -> Result<String, FlowError> {
        let mut out = String::new();
        for (i, f) in self.fragments.iter().enumerate() {
            let parent = f.parent.map_or_else(|| "-".into(), |p| p.to_string());
            let rect = |r: CssRect| {
                format!(
                    "{},{},{},{}",
                    r.origin().x.get(),
                    r.origin().y.get(),
                    r.size().width().get(),
                    r.size().height().get()
                )
            };
            let line = format!(
                "fragment {i} parent={parent} box={} node={}:{} content=({}) padding=({}) border=({})\n",
                f.source_box,
                f.node.slot(),
                f.node.generation(),
                rect(f.content),
                rect(f.padding),
                rect(f.border)
            );
            if line.len() > max_bytes.saturating_sub(out.len()) {
                return Err(FlowError::OutputLimit);
            }
            out.push_str(&line);
        }
        Ok(out)
    }
}

/// An atomic failure to compute or dump the block-only reference layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FlowError {
    /// Geometry exceeded its checked range.
    Geometry(GeometryError),
    /// A computed width/height could not be measured.
    Measure(MeasureError),
    /// Unsupported content/formatting, with its validated source node.
    Unsupported {
        /// The source node.
        node: NodeId,
        /// The missing algorithm.
        reason: UnsupportedFlow,
    },
    /// The principal snapshot has missing styles or invalid parent indexes.
    InvalidSnapshot,
    /// Visited principal-box budget exceeded.
    NodeLimit,
    /// Output fragment budget exceeded.
    BoxLimit,
    /// Principal-box nesting budget exceeded.
    DepthLimit,
    /// Inspected text budget exceeded.
    TextLimit,
    /// Cooperative cancellation.
    Cancelled,
    /// Dump byte budget exceeded.
    OutputLimit,
}
impl From<GeometryError> for FlowError {
    fn from(value: GeometryError) -> Self {
        Self::Geometry(value)
    }
}
impl From<MeasureError> for FlowError {
    fn from(value: MeasureError) -> Self {
        Self::Measure(value)
    }
}
impl fmt::Display for FlowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { reason, .. } => {
                write!(f, "block reference does not yet support {reason:?}")
            }
            other => write!(f, "block layout failed: {other:?}"),
        }
    }
}
impl Error for FlowError {}
