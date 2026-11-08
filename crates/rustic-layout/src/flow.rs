//! Iterative vertical block flow with checked adjoining-margin collapse.
//!
//! Unsupported content fails atomically instead of being assigned invented
//! geometry. Normalized flow boxes are a separate structural path; their
//! inline geometry awaits line layout.

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
    /// Resolved margins before collapse; the rectangles include collapse effects.
    pub const fn margin(self) -> Edges {
        self.margin
    }
}

/// A deterministic, block-only reference layout. No glyphs or pixels yet.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockLayout {
    fragments: Vec<BlockFragment>,
}

struct WorkBlock<'a> {
    source: usize,
    parent: Option<usize>,
    node: NodeId,
    style: &'a ComputedStyle,
    measure: BlockMeasure,
    height: HeightConstraint,
    depth: usize,
    children: Vec<usize>,
    relative_y: CssPx,
    content_height: CssPx,
    collapse: crate::margins::CollapseChild,
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

fn build_blocks<'a>(
    tree: &'a BoxTree,
    viewport: CssSize,
    limits: LayoutLimits,
    cancellation: &CancellationToken,
) -> Result<Vec<WorkBlock<'a>>, FlowError> {
    let mut work: Vec<WorkBlock<'_>> = Vec::new();
    let mut index: Vec<Option<usize>> = Vec::with_capacity(tree.boxes().len());
    let mut text_units = 0usize;
    for (source, b) in tree.boxes().iter().enumerate() {
        cancellation.check().map_err(|_| FlowError::Cancelled)?;
        let parent = match b.parent() {
            Some(p) => Some(
                index
                    .get(p)
                    .copied()
                    .flatten()
                    .ok_or(FlowError::InvalidSnapshot)?,
            ),
            None => None,
        };
        let depth = match parent {
            Some(p) => work[p].depth.checked_add(1).ok_or(FlowError::DepthLimit)?,
            None => 0,
        };
        if depth > limits.max_depth {
            return Err(FlowError::DepthLimit);
        }
        if b.kind() == BoxKind::Text {
            text_units = text_units
                .checked_add(b.text().len())
                .filter(|&n| n <= limits.max_text_units)
                .ok_or(FlowError::TextLimit)?;
            if !ignorable_text(b, cancellation)? {
                return Err(FlowError::Unsupported {
                    node: b.node(),
                    reason: UnsupportedFlow::TextLayout,
                });
            }
            index.push(None);
            continue;
        }
        if work.len() >= limits.max_boxes {
            return Err(FlowError::BoxLimit);
        }
        let style = supported_box(b)?;
        let (width, basis_height, containing_direction) = match parent {
            Some(p) => (
                work[p].measure.content_width,
                work[p].height.definite(),
                direction(work[p].style)?,
            ),
            None => (viewport.width(), Some(viewport.height()), direction(style)?),
        };
        let measure = measure_block_width(style, width, containing_direction)?;
        let extras = measure
            .border
            .top
            .checked_add(measure.padding.top)?
            .checked_add(measure.padding.bottom)?
            .checked_add(measure.border.bottom)?;
        let height = HeightConstraint::new(style, basis_height, extras)?;
        let current = work.len();
        work.push(WorkBlock {
            source,
            parent,
            node: b.node(),
            style,
            measure,
            height,
            depth,
            children: Vec::new(),
            relative_y: CssPx::ZERO,
            content_height: CssPx::ZERO,
            collapse: crate::margins::CollapseChild {
                before: crate::margins::MarginStrut::default(),
                after: crate::margins::MarginStrut::default(),
                through: false,
                border_height: CssPx::ZERO,
            },
        });
        if let Some(p) = parent {
            work[p].children.push(current);
        }
        index.push(Some(current));
    }
    Ok(work)
}

fn resolve_blocks(
    work: &mut [WorkBlock<'_>],
    cancellation: &CancellationToken,
) -> Result<(), FlowError> {
    use crate::margins::{CollapseChild, CollapseInput, collapse_block};
    for i in (0..work.len()).rev() {
        cancellation.check().map_err(|_| FlowError::Cancelled)?;
        let b = &work[i];
        let m = b.measure;
        let boundary =
            b.parent.is_none() || matches!(b.style.get(P::Display), V::Display(Display::FlowRoot));
        let top_clear = m.border.top == CssPx::ZERO && m.padding.top == CssPx::ZERO;
        let bottom_clear = m.border.bottom == CssPx::ZERO && m.padding.bottom == CssPx::ZERO;
        let mut child_data = Vec::with_capacity(b.children.len());
        for &child in &b.children {
            cancellation.check().map_err(|_| FlowError::Cancelled)?;
            child_data.push(work[child].collapse);
        }
        let result = collapse_block(
            CollapseInput {
                top: m.margin.top,
                bottom: m.margin.bottom,
                open_top: !boundary && top_clear,
                open_bottom: !boundary && bottom_clear && b.height.is_auto(),
                can_through: !boundary
                    && top_clear
                    && bottom_clear
                    && b.height.can_collapse_through(),
                nonzero_minimum: b.height.minimum() != CssPx::ZERO,
            },
            &child_data,
            cancellation,
        )?;
        let content_height = b.height.used(result.natural_height);
        let border_height = content_height
            .checked_add(m.border.top)?
            .checked_add(m.padding.top)?
            .checked_add(m.padding.bottom)?
            .checked_add(m.border.bottom)?;
        for (child_offset, &position) in result.child_positions.iter().enumerate() {
            cancellation.check().map_err(|_| FlowError::Cancelled)?;
            let child = work[i].children[child_offset];
            work[child].relative_y = position;
        }
        work[i].content_height = content_height;
        work[i].collapse = CollapseChild {
            before: result.before,
            after: result.after,
            through: result.through,
            border_height,
        };
    }
    Ok(())
}

fn finish(b: &WorkBlock<'_>, border_origin: CssPoint) -> Result<BlockFragment, FlowError> {
    let m = b.measure;
    let content_origin = CssPoint {
        x: border_origin
            .x
            .checked_add(m.border.left)?
            .checked_add(m.padding.left)?,
        y: border_origin
            .y
            .checked_add(m.border.top)?
            .checked_add(m.padding.top)?,
    };
    let h = b.content_height;
    let content = CssRect::new(
        content_origin,
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
        x: border_origin.x.checked_add(m.border.left)?,
        y: border_origin.y.checked_add(m.border.top)?,
    };
    let padding = CssRect::new(padding_origin, padding_size)?;
    let border_size = CssSize::new(
        padding_size
            .width()
            .checked_add(m.border.left)?
            .checked_add(m.border.right)?
            .get(),
        b.collapse.border_height.get(),
    )?;
    let border = CssRect::new(border_origin, border_size)?;
    Ok(BlockFragment {
        node: b.node,
        source_box: b.source,
        parent: b.parent,
        content,
        padding,
        border,
        margin: m.margin,
    })
}

fn place_blocks(
    work: &[WorkBlock<'_>],
    cancellation: &CancellationToken,
) -> Result<Vec<BlockFragment>, FlowError> {
    let mut fragments: Vec<BlockFragment> = Vec::with_capacity(work.len());
    let mut root_cursor = CssPx::ZERO;
    for b in work {
        cancellation.check().map_err(|_| FlowError::Cancelled)?;
        let border_origin = match b.parent {
            Some(p) => {
                let parent = fragments
                    .get(p)
                    .ok_or(FlowError::InvalidSnapshot)?
                    .content
                    .origin();
                CssPoint {
                    x: parent.x.checked_add(b.measure.margin.left)?,
                    y: parent.y.checked_add(b.relative_y)?,
                }
            }
            None => CssPoint {
                x: b.measure.margin.left,
                y: root_cursor.checked_add(b.collapse.before.value()?)?,
            },
        };
        let fragment = finish(b, border_origin)?;
        if b.parent.is_none() {
            root_cursor = border_origin
                .y
                .checked_add(b.collapse.border_height)?
                .checked_add(b.collapse.after.value()?)?;
        }
        fragments.push(fragment);
    }
    Ok(fragments)
}

impl BlockLayout {
    /// Lay out a principal snapshot in CSS pixels without touching the DOM.
    ///
    /// Supports static, non-replaced block/flow-root boxes with visible overflow,
    /// including sibling/parent-child/empty-block margin collapse. Auto/fixed/min/max heights and percentages
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
        let mut work = build_blocks(tree, viewport, limits, cancellation)?;
        resolve_blocks(&mut work, cancellation)?;
        Ok(Self {
            fragments: place_blocks(&work, cancellation)?,
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
