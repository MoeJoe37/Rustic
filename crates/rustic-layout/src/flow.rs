//! Block flow with margin collapse, floats, clearance, relative/absolute/fixed
//! positioning and overflow clipping (CSS 2.2 §8.3.1, §9.4–9.7, §10; CSS
//! Overflow 3 §3).
//!
//! One iterative top-down walk lays out each block formatting context with an
//! explicit frame stack; there is no content-controlled recursion. A margin
//! strut stays unresolved until a border, padding, content edge, clearance or
//! formatting-context boundary resolves it, and floats/static positions met in
//! the meantime are queued until then. Absolutely positioned boxes are laid out
//! after their containing block in a FIFO pass. Unsupported content fails
//! atomically instead of being assigned invented geometry.

use rustic_base::CancellationToken;
use rustic_css::{
    Clear, Direction, Display, Float, Overflow, Position, PropertyId as P, PropertyValue as V,
    WhiteSpace,
};
use rustic_dom::NodeId;
use rustic_style::ComputedStyle;
use std::{error::Error, fmt};

use crate::exclusions::{Exclusions, Side};
use crate::height::HeightConstraint;
use crate::intrinsic::{self, Intrinsic};
use crate::margins::MarginStrut;
use crate::overflow::{self, Clip};
use crate::position::{self, AbsoluteContext, AbsoluteHeight};
use crate::sizing::measure_in_band;
use crate::tree::HtmlTag;
use crate::{
    BlockMeasure, BoxKind, BoxSnapshot, BoxTree, CssPoint, CssPx, CssRect, CssSize, Edges,
    GeometryError, LayoutLimits, MeasureError,
};

/// Work units (box visits, float scans, chain steps) one layout may spend.
///
/// Float placement and formatting-context retries are superlinear in the
/// worst case, so they are metered rather than trusted (TD-44).
pub const MAX_FLOW_WORK: usize = 64_000_000;

/// Why a principal box needs an unimplemented formatting algorithm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedFlow {
    /// Inline formatting, text shaping, flex/grid/table/list layout or another context.
    FormattingContext,
    /// Text is not an entirely collapsible whitespace run under normal/nowrap.
    TextLayout,
    /// Replaced elements, native controls, foreign content or special HTML layout.
    SpecialElement,
    /// Sticky positioning, which needs scroll offsets.
    Positioning,
}

/// How a fragment is positioned (CSS 2.2 §9.3).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PositionScheme {
    /// Normal flow, `position:static`.
    Static,
    /// Normal flow, then shifted by [`BlockFragment::relative_offset`].
    Relative,
    /// A left float (after resolving logical sides).
    FloatLeft,
    /// A right float (after resolving logical sides).
    FloatRight,
    /// Out of flow, against the nearest positioned ancestor's padding box.
    Absolute,
    /// Out of flow, against the viewport.
    Fixed,
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
    scheme: PositionScheme,
    relative: CssPoint,
    containing_block: Option<usize>,
    overflow: (Overflow, Overflow),
    clip: Clip,
    scrollable: Option<CssRect>,
}

impl BlockFragment {
    pub(crate) fn set_clip(&mut self, clip: Clip) {
        self.clip = clip;
    }
    pub(crate) fn set_scrollable(&mut self, rect: CssRect) {
        self.scrollable = Some(rect);
    }
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
    /// Used content rectangle in CSS pixels, after relative offsets.
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
    /// Used margins before collapse; the rectangles include collapse effects.
    pub const fn margin(self) -> Edges {
        self.margin
    }
    /// Positioning scheme.
    pub const fn scheme(self) -> PositionScheme {
        self.scheme
    }
    /// The relative-positioning shift already applied to the rectangles.
    pub const fn relative_offset(self) -> CssPoint {
        self.relative
    }
    /// Containing-block fragment; `None` means the initial containing block
    /// (or the viewport for fixed boxes).
    pub const fn containing_block(self) -> Option<usize> {
        self.containing_block
    }
    /// Used `overflow-x`/`overflow-y`, after viewport propagation.
    pub const fn overflow(self) -> (Overflow, Overflow) {
        self.overflow
    }
    /// The clip that ancestors' overflow applies to this box, following the
    /// containing-block chain (absolute boxes escape non-containing clips).
    pub const fn clip(self) -> Clip {
        self.clip
    }
    /// For a scroll container (`hidden`, `scroll` or `auto`), its reachable
    /// scrollable overflow rectangle; `None` otherwise. No scrollbars exist,
    /// so no gutter is reserved and the scroll position is the origin.
    pub const fn scrollable_overflow(self) -> Option<CssRect> {
        self.scrollable
    }
}

/// A deterministic block reference layout. No glyphs or pixels yet.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockLayout {
    fragments: Vec<BlockFragment>,
    viewport_overflow: (Overflow, Overflow),
    viewport_scrollable: CssRect,
}

/// A metered work counter that also polls cancellation.
pub(crate) struct Work<'c> {
    spent: usize,
    cancellation: &'c CancellationToken,
}

impl Work<'_> {
    pub(crate) fn charge(&mut self, units: usize) -> Result<(), FlowError> {
        let before = self.spent;
        self.spent = self
            .spent
            .checked_add(units.max(1))
            .filter(|&n| n <= MAX_FLOW_WORK)
            .ok_or(FlowError::WorkLimit)?;
        if before / 1024 != self.spent / 1024 || units == 0 {
            self.cancellation
                .check()
                .map_err(|_| FlowError::Cancelled)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scheme {
    Flow,
    Float(Side),
    Absolute,
    Fixed,
}

struct Node<'a> {
    source: usize,
    parent: Option<usize>,
    node: NodeId,
    style: &'a ComputedStyle,
    end: usize,
    depth: usize,
    scheme: Scheme,
    relative: bool,
    bfc_root: bool,
    overflow: (Overflow, Overflow),
    containing: Option<usize>,
    intrinsic: Intrinsic,
    direction: Direction,
    // Results of the current layout attempt.
    measure: BlockMeasure,
    height: Option<HeightConstraint>,
    content_height: CssPx,
    border_height: CssPx,
    /// Border origin: in the parent's child space, or relative to the
    /// containing padding box for absolute/fixed boxes.
    pos: (f64, f64),
    top_resolved: bool,
    rel: CssPoint,
    /// Absolute/fixed: hypothetical margin edge in the parent's child space.
    static_pos: (f64, f64),
}

impl Node<'_> {
    fn edge(&self) -> (f64, f64) {
        let m = &self.measure;
        (
            m.border.left.get() + m.padding.left.get(),
            m.border.top.get() + m.padding.top.get(),
        )
    }
    /// Content origin in the coordinate space this box's children use.
    fn child_origin(&self) -> (f64, f64) {
        if self.bfc_root {
            (0.0, 0.0)
        } else {
            let (x, y) = self.edge();
            (self.pos.0 + x, self.pos.1 + y)
        }
    }
    fn border_width(&self) -> f64 {
        let m = &self.measure;
        m.content_width.get()
            + m.padding.left.get()
            + m.padding.right.get()
            + m.border.left.get()
            + m.border.right.get()
    }
    fn vertical_extras(&self) -> Result<CssPx, FlowError> {
        let m = &self.measure;
        Ok(m.border
            .top
            .checked_add(m.padding.top)?
            .checked_add(m.padding.bottom)?
            .checked_add(m.border.bottom)?)
    }
    fn constraint(&self) -> Result<&HeightConstraint, FlowError> {
        self.height.as_ref().ok_or(FlowError::InvalidSnapshot)
    }
}

/// A queued item awaiting the resolution of the current margin strut.
#[derive(Clone, Copy)]
enum Queued {
    Float { node: usize, left: f64, right: f64 },
    Static(usize),
}

#[derive(Default)]
struct Bfc {
    y: f64,
    strut: MarginStrut,
    pending: bool,
    queue: Vec<Queued>,
    pending_tops: Vec<usize>,
    exclusions: Exclusions,
}

#[derive(Clone, Copy)]
struct Frame {
    node: Option<usize>,
    next: usize,
    end: usize,
    content_x: f64,
    width: CssPx,
    definite: Option<CssPx>,
    direction: Direction,
    bfc_root: bool,
    /// Abstract queue length to restore if an in-flow formatting root retries.
    checkpoint: usize,
    /// Extent below its top over which an in-flow formatting root was placed.
    query: f64,
}

fn direction(style: &ComputedStyle) -> Result<Direction, FlowError> {
    match style.get(P::Direction) {
        V::Direction(value) => Ok(*value),
        _ => Err(FlowError::InvalidSnapshot),
    }
}

fn overflow_pair(style: &ComputedStyle) -> Result<(Overflow, Overflow), FlowError> {
    match (style.get(P::OverflowX), style.get(P::OverflowY)) {
        (V::Overflow(x), V::Overflow(y)) => Ok((*x, *y)),
        _ => Err(FlowError::InvalidSnapshot),
    }
}

fn scroll_container(o: (Overflow, Overflow)) -> bool {
    [o.0, o.1]
        .iter()
        .any(|o| matches!(o, Overflow::Hidden | Overflow::Scroll | Overflow::Auto))
}

fn physical_side(logical_start: bool, direction: Direction) -> Side {
    match (logical_start, direction) {
        (true, Direction::Ltr) | (false, Direction::Rtl) => Side::Left,
        _ => Side::Right,
    }
}

fn float_side(style: &ComputedStyle, containing: Direction) -> Result<Option<Side>, FlowError> {
    Ok(match style.get(P::Float) {
        V::Float(Float::None) => None,
        V::Float(Float::Left) => Some(Side::Left),
        V::Float(Float::Right) => Some(Side::Right),
        V::Float(Float::InlineStart) => Some(physical_side(true, containing)),
        V::Float(Float::InlineEnd) => Some(physical_side(false, containing)),
        _ => return Err(FlowError::InvalidSnapshot),
    })
}

/// Cleared physical sides (left, right) relative to the containing direction.
fn clear_sides(style: &ComputedStyle, containing: Direction) -> Result<(bool, bool), FlowError> {
    let side = |start| physical_side(start, containing) == Side::Left;
    Ok(match style.get(P::Clear) {
        V::Clear(Clear::None) => (false, false),
        V::Clear(Clear::Left) => (true, false),
        V::Clear(Clear::Right) => (false, true),
        V::Clear(Clear::Both) => (true, true),
        V::Clear(Clear::InlineStart) => (side(true), !side(true)),
        V::Clear(Clear::InlineEnd) => (side(false), !side(false)),
        _ => return Err(FlowError::InvalidSnapshot),
    })
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
    if matches!(style.get(P::Position), V::Position(Position::Sticky)) {
        return Err(fail(UnsupportedFlow::Positioning));
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

fn sub(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 - b.0, a.1 - b.1)
}
fn add(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 + b.0, a.1 + b.1)
}

struct Engine<'a, 'c> {
    nodes: Vec<Node<'a>>,
    viewport: CssSize,
    root_direction: Direction,
    absolute: Vec<usize>,
    work: Work<'c>,
}

fn build_nodes<'a>(
    tree: &'a BoxTree,
    limits: LayoutLimits,
    cancellation: &CancellationToken,
) -> Result<Vec<Node<'a>>, FlowError> {
    let mut nodes: Vec<Node<'_>> = Vec::new();
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
            Some(p) => nodes[p].depth.checked_add(1).ok_or(FlowError::DepthLimit)?,
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
        if nodes.len() >= limits.max_boxes {
            return Err(FlowError::BoxLimit);
        }
        let style = supported_box(b)?;
        let containing_direction = match parent {
            Some(p) => nodes[p].direction,
            None => direction(style)?,
        };
        let (scheme, relative) = match style.get(P::Position) {
            V::Position(Position::Absolute) => (Scheme::Absolute, false),
            V::Position(Position::Fixed) => (Scheme::Fixed, false),
            V::Position(p) => {
                let scheme = match float_side(style, containing_direction)? {
                    Some(side) => Scheme::Float(side),
                    None => Scheme::Flow,
                };
                (scheme, *p == Position::Relative)
            }
            _ => return Err(FlowError::InvalidSnapshot),
        };
        // The nearest positioned ancestor; `None` is the initial containing block.
        let containing = match scheme {
            Scheme::Fixed => None,
            Scheme::Absolute => {
                let mut c = parent;
                while let Some(p) = c {
                    if nodes[p].relative
                        || matches!(nodes[p].scheme, Scheme::Absolute | Scheme::Fixed)
                    {
                        break;
                    }
                    c = nodes[p].parent;
                }
                c
            }
            _ => parent,
        };
        nodes.push(Node {
            source,
            parent,
            node: b.node(),
            style,
            end: nodes.len() + 1,
            depth,
            scheme,
            relative,
            bfc_root: false,
            overflow: overflow_pair(style)?,
            containing,
            intrinsic: Intrinsic::default(),
            direction: direction(style)?,
            measure: BlockMeasure {
                content_width: CssPx::ZERO,
                margin: Edges::default(),
                padding: Edges::default(),
                border: Edges::default(),
            },
            height: None,
            content_height: CssPx::ZERO,
            border_height: CssPx::ZERO,
            pos: (0.0, 0.0),
            top_resolved: false,
            rel: CssPoint::default(),
            static_pos: (0.0, 0.0),
        });
        index.push(Some(nodes.len() - 1));
    }
    Ok(nodes)
}

/// CSS Overflow 3 §3.3: root, else HTML body, overflow moves to the viewport.
fn propagate_overflow(nodes: &mut [Node<'_>], tree: &BoxTree) -> (Overflow, Overflow) {
    let viewport = |(x, y): (Overflow, Overflow)| {
        let v = |o| match o {
            Overflow::Visible => Overflow::Auto,
            Overflow::Clip => Overflow::Hidden,
            other => other,
        };
        (v(x), v(y))
    };
    let visible = (Overflow::Visible, Overflow::Visible);
    let Some(root) = nodes.iter().position(|n| n.parent.is_none()) else {
        return viewport(visible);
    };
    if tree.boxes()[nodes[root].source].html_tag() != HtmlTag::DocumentElement {
        return viewport(visible);
    }
    if nodes[root].overflow != visible {
        return viewport(std::mem::replace(&mut nodes[root].overflow, visible));
    }
    // Preorder: the first body child of the root (`end` is not computed yet).
    let body = nodes
        .iter()
        .position(|n| n.parent == Some(root) && tree.boxes()[n.source].html_tag() == HtmlTag::Body);
    match body {
        Some(b) if nodes[b].overflow != visible => {
            viewport(std::mem::replace(&mut nodes[b].overflow, visible))
        }
        _ => viewport(visible),
    }
}

impl<'a, 'c> Engine<'a, 'c> {
    fn prepare(&mut self) -> Result<(), FlowError> {
        let n = self.nodes.len();
        for i in (0..n).rev() {
            self.work.charge(1)?;
            if let Some(p) = self.nodes[i].parent {
                self.nodes[p].end = self.nodes[p].end.max(self.nodes[i].end);
            }
        }
        for i in 0..n {
            let node = &mut self.nodes[i];
            node.bfc_root = node.parent.is_none()
                || node.scheme != Scheme::Flow
                || matches!(node.style.get(P::Display), V::Display(Display::FlowRoot))
                || scroll_container(node.overflow);
        }
        // Block-only intrinsic sizes, bottom up (CSS Sizing 3 §5).
        for i in (0..n).rev() {
            let (mut min, mut max) = (0.0_f64, 0.0_f64);
            let (mut left, mut right) = (0.0_f64, 0.0_f64);
            let mut c = i + 1;
            while c < self.nodes[i].end {
                self.work.charge(1)?;
                let child = &self.nodes[c];
                let next = child.end;
                if matches!(child.scheme, Scheme::Absolute | Scheme::Fixed) {
                    c = next;
                    continue;
                }
                let outer = intrinsic::outer_contribution(child.style, child.intrinsic)?;
                min = min.max(outer.min);
                match child.scheme {
                    Scheme::Float(side) => {
                        let (cl, cr) = clear_sides(child.style, self.nodes[i].direction)?;
                        if cl {
                            left = 0.0;
                        }
                        if cr {
                            right = 0.0;
                        }
                        match side {
                            Side::Left => left += outer.max,
                            Side::Right => right += outer.max,
                        }
                        max = max.max(left + right);
                    }
                    _ => {
                        let beside = if child.bfc_root { left + right } else { 0.0 };
                        max = max.max(beside + outer.max);
                        left = 0.0;
                        right = 0.0;
                    }
                }
                c = next;
            }
            self.nodes[i].intrinsic = Intrinsic { min, max };
        }
        Ok(())
    }

    fn frame_for(&self, i: usize, content_x: f64) -> Result<Frame, FlowError> {
        let n = &self.nodes[i];
        Ok(Frame {
            node: Some(i),
            next: i + 1,
            end: n.end,
            content_x,
            width: n.measure.content_width,
            definite: n.constraint()?.definite(),
            direction: n.direction,
            bfc_root: n.bfc_root,
            checkpoint: self.absolute.len(),
            query: 0.0,
        })
    }

    /// Resolve the innermost formatting context's strut at `y`.
    fn resolve(&mut self, bfcs: &mut [Bfc], y: f64) -> Result<(), FlowError> {
        let bfc = bfcs.last_mut().ok_or(FlowError::InvalidSnapshot)?;
        for i in bfc.pending_tops.drain(..) {
            self.work.charge(1)?;
            self.nodes[i].pos.1 = y;
            self.nodes[i].top_resolved = true;
        }
        bfc.y = y;
        bfc.strut = MarginStrut::default();
        bfc.pending = false;
        let queue = std::mem::take(&mut bfc.queue);
        for item in queue {
            self.flush(bfc, item, y)?;
        }
        Ok(())
    }

    /// Place queued floats at `y` without resolving the strut (before clearance).
    fn flush_floats(&mut self, bfc: &mut Bfc, y: f64) -> Result<(), FlowError> {
        let queue = std::mem::take(&mut bfc.queue);
        for item in queue {
            match item {
                Queued::Float { .. } => self.flush(bfc, item, y)?,
                Queued::Static(_) => bfc.queue.push(item),
            }
        }
        Ok(())
    }

    fn flush(&mut self, bfc: &mut Bfc, item: Queued, y: f64) -> Result<(), FlowError> {
        self.work.charge(1)?;
        match item {
            Queued::Static(i) => self.nodes[i].static_pos.1 = y,
            Queued::Float { node, left, right } => self.place_float(bfc, node, left, right, y)?,
        }
        Ok(())
    }

    fn place_float(
        &mut self,
        bfc: &mut Bfc,
        i: usize,
        left: f64,
        right: f64,
        at: f64,
    ) -> Result<(), FlowError> {
        let n = &self.nodes[i];
        let Scheme::Float(side) = n.scheme else {
            return Err(FlowError::InvalidSnapshot);
        };
        // A parent still inside the unresolved strut will sit at `at` or lower.
        let parent_top = match n.parent {
            Some(p) if self.nodes[p].bfc_root || self.nodes[p].top_resolved => {
                self.nodes[p].child_origin().1
            }
            _ => at,
        };
        let containing = n
            .parent
            .map_or(self.root_direction, |p| self.nodes[p].direction);
        let (cl, cr) = clear_sides(n.style, containing)?;
        let mut min_top = at.max(parent_top);
        if let Some(c) = bfc.exclusions.clearance(cl, cr) {
            min_top = min_top.max(c);
        }
        let m = n.measure;
        let width = m.margin.left.get() + n.border_width() + m.margin.right.get();
        let height = m.margin.top.get() + n.border_height.get() + m.margin.bottom.get();
        let (x, y) =
            bfc.exclusions
                .place(side, width, height, left, right, min_top, &mut self.work)?;
        let n = &mut self.nodes[i];
        n.pos = (x + m.margin.left.get(), y + m.margin.top.get());
        n.top_resolved = true;
        Ok(())
    }

    fn natural_root(&mut self, bfcs: &mut Vec<Bfc>, auto: bool) -> Result<f64, FlowError> {
        let bfc = bfcs.last().ok_or(FlowError::InvalidSnapshot)?;
        let end = bfc.y + bfc.strut.value()?.get();
        self.resolve(bfcs, end)?;
        let bfc = bfcs.pop().ok_or(FlowError::InvalidSnapshot)?;
        let mut natural = end;
        if auto {
            if let Some(bottom) = bfc.exclusions.lowest() {
                natural = natural.max(bottom);
            }
        }
        Ok(natural.max(0.0))
    }

    fn set_height(&mut self, i: usize, natural: f64) -> Result<(), FlowError> {
        let n = &mut self.nodes[i];
        let h = n.constraint()?.used(CssPx::new(natural.max(0.0))?);
        n.content_height = h;
        n.border_height = h.checked_add(n.vertical_extras()?)?;
        Ok(())
    }

    /// Lay out one formatting-context root's subtree; returns its natural height.
    fn walk(&mut self, root: Frame) -> Result<f64, FlowError> {
        let mut frames = vec![root];
        let mut bfcs = vec![Bfc::default()];
        // A pending retry of an in-flow formatting root: (box, top, extent).
        let mut retry: Option<(usize, f64, f64)> = None;
        loop {
            if let Some((i, y, extent)) = retry.take() {
                self.enter_flow_root(&mut frames, &mut bfcs, i, Some((y, extent)))?;
                continue;
            }
            let frame = *frames.last().ok_or(FlowError::InvalidSnapshot)?;
            if frame.next < frame.end {
                let i = frame.next;
                self.work.charge(1)?;
                if let Some(f) = frames.last_mut() {
                    f.next = self.nodes[i].end;
                }
                self.enter(&mut frames, &mut bfcs, i)?;
                continue;
            }
            frames.pop();
            let Some(i) = frame.node.filter(|_| !frames.is_empty()) else {
                // The walk's own root.
                let auto = match frame.node {
                    Some(i) => self.nodes[i]
                        .height
                        .as_ref()
                        .is_none_or(HeightConstraint::is_auto),
                    None => false,
                };
                return self.natural_root(&mut bfcs, auto);
            };
            self.work.charge(1)?;
            if frame.bfc_root {
                let auto = self.nodes[i].constraint()?.is_auto();
                let natural = self.natural_root(&mut bfcs, auto)?;
                self.set_height(i, natural)?;
                match self.nodes[i].scheme {
                    Scheme::Float(_) => self.exit_float(&frames, &mut bfcs, i)?,
                    Scheme::Flow => {
                        retry = self.exit_flow_root(&frames, &mut bfcs, i, &frame)?;
                    }
                    Scheme::Absolute | Scheme::Fixed => return Err(FlowError::InvalidSnapshot),
                }
            } else {
                self.exit_block(&frames, &mut bfcs, i)?;
            }
        }
    }

    fn enter(
        &mut self,
        frames: &mut Vec<Frame>,
        bfcs: &mut Vec<Bfc>,
        i: usize,
    ) -> Result<(), FlowError> {
        let pf = *frames.last().ok_or(FlowError::InvalidSnapshot)?;
        match self.nodes[i].scheme {
            Scheme::Absolute | Scheme::Fixed => {
                let x = match pf.direction {
                    Direction::Ltr => pf.content_x,
                    Direction::Rtl => pf.content_x + pf.width.get(),
                };
                self.nodes[i].static_pos.0 = x;
                let bfc = bfcs.last_mut().ok_or(FlowError::InvalidSnapshot)?;
                if bfc.pending {
                    bfc.queue.push(Queued::Static(i));
                } else {
                    self.nodes[i].static_pos.1 = bfc.y;
                }
                self.absolute.push(i);
                Ok(())
            }
            Scheme::Float(_) => {
                let n = &self.nodes[i];
                let measure = position::measure_float(n.style, pf.width, n.intrinsic)?;
                self.begin(i, measure, &pf)?;
                frames.push(self.frame_for(i, 0.0)?);
                bfcs.push(Bfc::default());
                Ok(())
            }
            Scheme::Flow if self.nodes[i].bfc_root => self.enter_flow_root(frames, bfcs, i, None),
            Scheme::Flow => self.enter_block(frames, bfcs, i, &pf),
        }
    }

    /// Store measure, height constraint and relative offset for a new box.
    fn begin(&mut self, i: usize, measure: BlockMeasure, pf: &Frame) -> Result<(), FlowError> {
        let n = &mut self.nodes[i];
        n.measure = measure;
        let extras = n.vertical_extras()?;
        n.height = Some(HeightConstraint::new(n.style, pf.definite, extras)?);
        n.rel = if n.relative {
            position::relative_offset(n.style, pf.width, pf.definite, pf.direction)?
        } else {
            CssPoint::default()
        };
        n.top_resolved = false;
        Ok(())
    }

    /// Clearance or strut handling for an in-flow child; returns its resolved
    /// border top, or `None` while it stays inside an unresolved strut.
    fn begin_flow(
        &mut self,
        bfcs: &mut [Bfc],
        i: usize,
        pf: &Frame,
        force: bool,
    ) -> Result<Option<f64>, FlowError> {
        let n = &self.nodes[i];
        let top = MarginStrut::from(n.measure.margin.top);
        let top_clear = n.measure.border.top == CssPx::ZERO && n.measure.padding.top == CssPx::ZERO;
        let (cl, cr) = clear_sides(n.style, pf.direction)?;
        if cl || cr {
            let len = bfcs.len();
            let bfc = &mut bfcs[len - 1];
            if bfc.pending {
                let at = bfc.y + bfc.strut.value()?.get();
                self.flush_floats(bfc, at)?;
            }
            if let Some(clear) = bfc.exclusions.clearance(cl, cr) {
                let hypothetical = bfc.y + bfc.strut.merge(top).value()?.get();
                if hypothetical < clear {
                    let at = bfc.y + bfc.strut.value()?.get();
                    self.resolve(bfcs, at)?;
                    return Ok(Some(clear));
                }
            }
        }
        let bfc = bfcs.last_mut().ok_or(FlowError::InvalidSnapshot)?;
        bfc.strut = bfc.strut.merge(top);
        bfc.pending = true;
        if force || !top_clear {
            let at = bfc.y + bfc.strut.value()?.get();
            self.resolve(bfcs, at)?;
            return Ok(Some(at));
        }
        Ok(None)
    }

    fn enter_block(
        &mut self,
        frames: &mut Vec<Frame>,
        bfcs: &mut [Bfc],
        i: usize,
        pf: &Frame,
    ) -> Result<(), FlowError> {
        let n = &self.nodes[i];
        let measure = measure_in_band(
            n.style,
            pf.width,
            pf.direction,
            (0.0, 0.0),
            Some(n.intrinsic),
        )?;
        self.begin(i, measure, pf)?;
        let x = pf.content_x + measure.margin.left.get();
        self.nodes[i].pos.0 = x;
        match self.begin_flow(bfcs, i, pf, false)? {
            Some(y) => {
                let n = &mut self.nodes[i];
                n.pos.1 = y;
                n.top_resolved = true;
                let content = y + n.edge().1;
                let bfc = bfcs.last_mut().ok_or(FlowError::InvalidSnapshot)?;
                bfc.y = content;
                bfc.strut = MarginStrut::default();
                bfc.pending = false;
            }
            None => bfcs
                .last_mut()
                .ok_or(FlowError::InvalidSnapshot)?
                .pending_tops
                .push(i),
        }
        let edge = self.nodes[i].edge().0;
        frames.push(self.frame_for(i, x + edge)?);
        Ok(())
    }

    fn exit_block(
        &mut self,
        frames: &[Frame],
        bfcs: &mut [Bfc],
        i: usize,
    ) -> Result<(), FlowError> {
        let n = &self.nodes[i];
        let m = n.measure;
        let constraint = n.constraint()?;
        let bottom_clear = m.border.bottom == CssPx::ZERO && m.padding.bottom == CssPx::ZERO;
        let open_bottom = bottom_clear && constraint.is_auto();
        let through = bottom_clear && constraint.can_collapse_through();
        let bottom = MarginStrut::from(m.margin.bottom);
        let len = bfcs.len();
        if !n.top_resolved {
            if through {
                let parent_pending = frames
                    .last()
                    .and_then(|f| f.node.filter(|_| !f.bfc_root))
                    .is_some_and(|p| !self.nodes[p].top_resolved);
                let bfc = &mut bfcs[len - 1];
                if !parent_pending {
                    // An empty box sits above its own bottom margin (§8.3.1).
                    let top = bfc.y + bfc.strut.value()?.get();
                    let k = bfc
                        .pending_tops
                        .iter()
                        .position(|&j| j == i)
                        .ok_or(FlowError::InvalidSnapshot)?;
                    for j in bfc.pending_tops.drain(k..) {
                        self.nodes[j].pos.1 = top;
                        self.nodes[j].top_resolved = true;
                    }
                }
                self.set_height(i, 0.0)?;
                let bfc = &mut bfcs[len - 1];
                bfc.strut = bfc.strut.merge(bottom);
                bfc.pending = true;
            } else {
                let bfc = &bfcs[len - 1];
                let at = bfc.y + bfc.strut.value()?.get();
                self.resolve(bfcs, at)?;
                self.set_height(i, 0.0)?;
                let bfc = &mut bfcs[len - 1];
                bfc.y = at + self.nodes[i].border_height.get();
                bfc.strut = bottom;
                bfc.pending = true;
            }
            return Ok(());
        }
        let content_top = n.pos.1 + n.edge().1;
        let natural = if open_bottom {
            bfcs[len - 1].y - content_top
        } else {
            let bfc = &bfcs[len - 1];
            let at = bfc.y + bfc.strut.value()?.get();
            self.resolve(bfcs, at)?;
            at - content_top
        };
        self.set_height(i, natural)?;
        let n = &self.nodes[i];
        let bfc = &mut bfcs[len - 1];
        bfc.y = n.pos.1 + n.border_height.get();
        bfc.strut = if open_bottom {
            bfc.strut.merge(bottom)
        } else {
            bottom
        };
        bfc.pending = true;
        Ok(())
    }

    /// An in-flow block formatting context root avoids float margin boxes
    /// (CSS 2.2 §9.5): it narrows within, or moves below, intruding floats.
    /// A retry gives a top and the extent below it that the band must cover.
    fn enter_flow_root(
        &mut self,
        frames: &mut Vec<Frame>,
        bfcs: &mut Vec<Bfc>,
        i: usize,
        retry: Option<(f64, f64)>,
    ) -> Result<(), FlowError> {
        let pf = *frames.last().ok_or(FlowError::InvalidSnapshot)?;
        let style = self.nodes[i].style;
        let inner = Some(self.nodes[i].intrinsic);
        let plain = measure_in_band(style, pf.width, pf.direction, (0.0, 0.0), inner)?;
        self.begin(i, plain, &pf)?;
        let (mut y, extent) = match retry {
            Some(retry) => retry,
            None => (
                self.begin_flow(bfcs, i, &pf, true)?
                    .ok_or(FlowError::InvalidSnapshot)?,
                0.0,
            ),
        };
        let constraint = self.nodes[i].constraint()?;
        let base = if constraint.is_auto() {
            0.0
        } else {
            constraint.used(CssPx::ZERO).get() + self.nodes[i].vertical_extras()?.get()
        };
        let mut query = base.max(extent);
        let (left, right) = (pf.content_x, pf.content_x + pf.width.get());
        let bfc = bfcs.last().ok_or(FlowError::InvalidSnapshot)?;
        let measure = loop {
            let band = bfc
                .exclusions
                .band(y, y + query, left, right, &mut self.work)?;
            if !band.intruded {
                break plain;
            }
            let m = measure_in_band(
                style,
                pf.width,
                pf.direction,
                (band.left - left, right - band.right),
                inner,
            )?;
            let x = left + m.margin.left.get();
            let width = m.content_width.get()
                + m.padding.left.get()
                + m.padding.right.get()
                + m.border.left.get()
                + m.border.right.get();
            if x >= band.left && x + width <= band.right {
                break m;
            }
            y = bfc
                .exclusions
                .next_bottom(y, y + query, &mut self.work)?
                .ok_or(FlowError::InvalidSnapshot)?;
            query = base;
        };
        let n = &mut self.nodes[i];
        n.measure = measure;
        n.pos = (left + measure.margin.left.get(), y);
        n.top_resolved = true;
        let mut frame = self.frame_for(i, 0.0)?;
        frame.query = query;
        frames.push(frame);
        bfcs.push(Bfc::default());
        Ok(())
    }

    fn exit_flow_root(
        &mut self,
        frames: &[Frame],
        bfcs: &mut [Bfc],
        i: usize,
        own: &Frame,
    ) -> Result<Option<(usize, f64, f64)>, FlowError> {
        let pf = frames.last().ok_or(FlowError::InvalidSnapshot)?;
        let n = &self.nodes[i];
        let (y, h) = (n.pos.1, n.border_height.get());
        let (left, right) = (pf.content_x, pf.content_x + pf.width.get());
        let (x, width) = (n.pos.0, n.border_width());
        let len = bfcs.len();
        let bfc = &mut bfcs[len - 1];
        if !bfc.exclusions.is_empty() {
            let band = bfc.exclusions.band(y, y + h, left, right, &mut self.work)?;
            if band.intruded && (x < band.left || x + width > band.right) {
                self.absolute.truncate(own.checkpoint);
                // First the narrower band over the laid-out extent at the same
                // top (each such retry needs a strictly larger extent; the work
                // budget bounds the rest), then the next float bottom.
                if h > own.query {
                    return Ok(Some((i, y, h)));
                }
                let next = bfc
                    .exclusions
                    .next_bottom(y, y + h, &mut self.work)?
                    .ok_or(FlowError::InvalidSnapshot)?;
                return Ok(Some((i, next, 0.0)));
            }
        }
        bfc.y = y + h;
        bfc.strut = MarginStrut::from(self.nodes[i].measure.margin.bottom);
        bfc.pending = true;
        Ok(None)
    }

    fn exit_float(
        &mut self,
        frames: &[Frame],
        bfcs: &mut [Bfc],
        i: usize,
    ) -> Result<(), FlowError> {
        let pf = frames.last().ok_or(FlowError::InvalidSnapshot)?;
        let (left, right) = (pf.content_x, pf.content_x + pf.width.get());
        let len = bfcs.len();
        let bfc = &mut bfcs[len - 1];
        if bfc.pending {
            bfc.queue.push(Queued::Float {
                node: i,
                left,
                right,
            });
            Ok(())
        } else {
            let at = bfc.y;
            self.place_float(bfc, i, left, right, at)
        }
    }

    /// Map a point in `from`'s child space to `target`'s padding-box space.
    fn map_to_containing(
        &mut self,
        mut from: Option<usize>,
        mut p: (f64, f64),
        target: Option<usize>,
    ) -> Result<(f64, f64), FlowError> {
        loop {
            self.work.charge(1)?;
            if from == target {
                return Ok(match target {
                    Some(t) => {
                        let n = &self.nodes[t];
                        let m = n.measure;
                        add(
                            sub(p, n.child_origin()),
                            (m.padding.left.get(), m.padding.top.get()),
                        )
                    }
                    None => p,
                });
            }
            let b = from.ok_or(FlowError::InvalidSnapshot)?;
            let n = &self.nodes[b];
            let local = add(sub(p, n.child_origin()), n.edge());
            match n.scheme {
                Scheme::Absolute | Scheme::Fixed => {
                    p = add(local, n.pos);
                    from = n.containing;
                    if let Some(c) = from {
                        let c = &self.nodes[c];
                        let m = c.measure;
                        p = add(
                            sub(p, (m.padding.left.get(), m.padding.top.get())),
                            c.child_origin(),
                        );
                    }
                }
                _ => {
                    p = add(add(local, n.pos), (n.rel.x.get(), n.rel.y.get()));
                    from = n.parent;
                }
            }
        }
    }

    fn layout_absolute(&mut self, i: usize) -> Result<(), FlowError> {
        let n = &self.nodes[i];
        let containing = n.containing;
        let (width, height, containing_direction) = match containing {
            Some(c) => {
                let c = &self.nodes[c];
                let m = c.measure;
                (
                    m.content_width.get() + m.padding.left.get() + m.padding.right.get(),
                    c.content_height.get() + m.padding.top.get() + m.padding.bottom.get(),
                    c.direction,
                )
            }
            None => (
                self.viewport.width().get(),
                self.viewport.height().get(),
                self.root_direction,
            ),
        };
        let static_direction = n
            .parent
            .map_or(self.root_direction, |p| self.nodes[p].direction);
        let (parent, static_pos) = (n.parent, n.static_pos);
        let (static_x, static_y) = self.map_to_containing(parent, static_pos, containing)?;
        let cx = AbsoluteContext {
            width,
            height,
            static_x,
            static_y,
            static_direction,
            containing_direction,
        };
        let n = &self.nodes[i];
        let horizontal = position::absolute_width(n.style, &cx, n.intrinsic)?;
        let m = horizontal.measure;
        let vertical = AbsoluteHeight::new(n.style, &cx, m.padding, m.border)?;
        let definite = vertical.definite().map(CssPx::new).transpose()?;
        let extras = {
            let n = &mut self.nodes[i];
            n.measure = m;
            n.vertical_extras()?
        };
        // Children see the solved height as definite only if it is not content-based.
        self.nodes[i].height = Some(HeightConstraint::solved(definite));
        let frame = self.frame_for(i, 0.0)?;
        let natural = self.walk(frame)?;
        let solved = vertical.solve(natural)?;
        let n = &mut self.nodes[i];
        n.measure.margin.top = CssPx::new(solved.margin_start)?;
        n.measure.margin.bottom = CssPx::new(solved.margin_end)?;
        n.content_height = CssPx::new(solved.size.max(0.0))?;
        n.border_height = n.content_height.checked_add(extras)?;
        n.pos = (
            horizontal.left + m.margin.left.get(),
            solved.start + solved.margin_start,
        );
        n.top_resolved = true;
        Ok(())
    }

    fn run(&mut self) -> Result<(), FlowError> {
        self.prepare()?;
        let root = Frame {
            node: None,
            next: 0,
            end: self.nodes.len(),
            content_x: 0.0,
            width: self.viewport.width(),
            definite: Some(self.viewport.height()),
            direction: self.root_direction,
            bfc_root: true,
            checkpoint: 0,
            query: 0.0,
        };
        self.walk(root)?;
        let mut q = 0;
        while let Some(&i) = self.absolute.get(q) {
            q += 1;
            self.layout_absolute(i)?;
        }
        Ok(())
    }
}

impl BlockLayout {
    /// Lay out a principal snapshot in CSS pixels without touching the DOM.
    ///
    /// Supports non-replaced block/flow-root boxes: normal flow with margin
    /// collapse, floats and clearance, relative offsets, absolute and fixed
    /// positioning, and overflow clipping/scroll-container geometry without
    /// scrolling UI. Pure collapsible whitespace runs under normal/nowrap
    /// produce no fragment; other text, inline contexts, sticky positioning and
    /// other unsupported semantics return a typed error (TD-38). Callers must
    /// compute styles for this same viewport before calling. Limits,
    /// [`MAX_FLOW_WORK`] and cancellation apply independently of snapshot
    /// construction.
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
        let mut nodes = build_nodes(tree, limits, cancellation)?;
        let viewport_overflow = propagate_overflow(&mut nodes, tree);
        let root_direction = nodes.first().map_or(Direction::Ltr, |n| n.direction);
        let mut engine = Engine {
            nodes,
            viewport,
            root_direction,
            absolute: Vec::new(),
            work: Work {
                spent: 0,
                cancellation,
            },
        };
        engine.run()?;
        engine.finish(viewport_overflow)
    }

    /// Physical block fragments, in principal-box preorder.
    pub fn fragments(&self) -> &[BlockFragment] {
        &self.fragments
    }

    /// Used viewport overflow (CSS Overflow 3 §3.3); `visible` becomes `auto`.
    pub const fn viewport_overflow(&self) -> (Overflow, Overflow) {
        self.viewport_overflow
    }

    /// Reachable scrollable overflow of the initial containing block.
    pub const fn viewport_scrollable_overflow(&self) -> CssRect {
        self.viewport_scrollable
    }

    /// Deterministic used-geometry dump, with no partial output on byte overflow.
    ///
    /// Lines for static, visible-overflow boxes keep the REN-02a format;
    /// positioning, overflow and clips append `key=value` fields.
    pub fn dump(&self, max_bytes: usize) -> Result<String, FlowError> {
        let mut out = String::new();
        let rect = |r: CssRect| {
            format!(
                "{},{},{},{}",
                r.origin().x.get(),
                r.origin().y.get(),
                r.size().width().get(),
                r.size().height().get()
            )
        };
        let push = |line: String, out: &mut String| {
            if line.len() > max_bytes.saturating_sub(out.len()) {
                return Err(FlowError::OutputLimit);
            }
            out.push_str(&line);
            Ok(())
        };
        for (i, f) in self.fragments.iter().enumerate() {
            let parent = f.parent.map_or_else(|| "-".into(), |p| p.to_string());
            let mut line = format!(
                "fragment {i} parent={parent} box={} node={}:{} content=({}) padding=({}) border=({})",
                f.source_box,
                f.node.slot(),
                f.node.generation(),
                rect(f.content),
                rect(f.padding),
                rect(f.border)
            );
            let scheme = match f.scheme {
                PositionScheme::Static => None,
                PositionScheme::Relative => Some("relative"),
                PositionScheme::FloatLeft => Some("float-left"),
                PositionScheme::FloatRight => Some("float-right"),
                PositionScheme::Absolute => Some("absolute"),
                PositionScheme::Fixed => Some("fixed"),
            };
            if let Some(s) = scheme {
                line.push_str(&format!(" scheme={s}"));
            }
            if matches!(f.scheme, PositionScheme::Absolute | PositionScheme::Fixed) {
                let cb = f
                    .containing_block
                    .map_or_else(|| "-".into(), |c| c.to_string());
                line.push_str(&format!(" cb={cb}"));
            }
            if f.relative != CssPoint::default() {
                line.push_str(&format!(
                    " offset=({},{})",
                    f.relative.x.get(),
                    f.relative.y.get()
                ));
            }
            if f.overflow != (Overflow::Visible, Overflow::Visible) {
                line.push_str(&format!(
                    " overflow={}/{}",
                    f.overflow.0.name(),
                    f.overflow.1.name()
                ));
            }
            if !f.clip.is_none() {
                line.push_str(&format!(" clip={}", f.clip));
            }
            if let Some(s) = f.scrollable {
                line.push_str(&format!(" scroll=({})", rect(s)));
            }
            line.push('\n');
            push(line, &mut out)?;
        }
        if self.viewport_overflow != (Overflow::Auto, Overflow::Auto) {
            push(
                format!(
                    "viewport overflow={}/{}\n",
                    self.viewport_overflow.0.name(),
                    self.viewport_overflow.1.name()
                ),
                &mut out,
            )?;
        }
        Ok(out)
    }
}

impl Engine<'_, '_> {
    fn finish(mut self, viewport_overflow: (Overflow, Overflow)) -> Result<BlockLayout, FlowError> {
        let n = self.nodes.len();
        let mut borders: Vec<(f64, f64)> = Vec::with_capacity(n);
        for i in 0..n {
            self.work.charge(1)?;
            let node = &self.nodes[i];
            let origin = match node.scheme {
                Scheme::Absolute | Scheme::Fixed => {
                    let base = match node.containing {
                        Some(c) => {
                            let m = self.nodes[c].measure;
                            add(borders[c], (m.border.left.get(), m.border.top.get()))
                        }
                        None => (0.0, 0.0),
                    };
                    add(base, node.pos)
                }
                _ => {
                    let rel = (node.rel.x.get(), node.rel.y.get());
                    match node.parent {
                        Some(p) => {
                            let parent = &self.nodes[p];
                            let content = add(borders[p], parent.edge());
                            add(add(content, sub(node.pos, parent.child_origin())), rel)
                        }
                        None => add(node.pos, rel),
                    }
                }
            };
            borders.push(origin);
        }
        let mut fragments = Vec::with_capacity(n);
        for (i, node) in self.nodes.iter().enumerate() {
            fragments.push(fragment(node, borders[i])?);
        }
        let parents: Vec<Option<usize>> = self
            .nodes
            .iter()
            .map(|n| match n.scheme {
                Scheme::Fixed => None,
                _ => n.containing,
            })
            .collect();
        let fixed: Vec<bool> = self
            .nodes
            .iter()
            .map(|n| n.scheme == Scheme::Fixed)
            .collect();
        let directions: Vec<Direction> = self.nodes.iter().map(|n| n.direction).collect();
        let viewport_scrollable = overflow::assign(
            &mut fragments,
            &parents,
            &fixed,
            &directions,
            self.viewport,
            self.root_direction,
            &mut self.work,
        )?;
        Ok(BlockLayout {
            fragments,
            viewport_overflow,
            viewport_scrollable,
        })
    }
}

fn fragment(node: &Node<'_>, border_origin: (f64, f64)) -> Result<BlockFragment, FlowError> {
    let m = node.measure;
    let point = |p: (f64, f64)| -> Result<CssPoint, GeometryError> {
        Ok(CssPoint {
            x: CssPx::new(p.0)?,
            y: CssPx::new(p.1)?,
        })
    };
    let border_origin = point(border_origin)?;
    let padding_origin = CssPoint {
        x: border_origin.x.checked_add(m.border.left)?,
        y: border_origin.y.checked_add(m.border.top)?,
    };
    let content_origin = CssPoint {
        x: padding_origin.x.checked_add(m.padding.left)?,
        y: padding_origin.y.checked_add(m.padding.top)?,
    };
    let h = node.content_height;
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
    let padding = CssRect::new(padding_origin, padding_size)?;
    let border_size = CssSize::new(
        padding_size
            .width()
            .checked_add(m.border.left)?
            .checked_add(m.border.right)?
            .get(),
        node.border_height.get(),
    )?;
    let border = CssRect::new(border_origin, border_size)?;
    let scheme = match node.scheme {
        Scheme::Flow if node.relative => PositionScheme::Relative,
        Scheme::Flow => PositionScheme::Static,
        Scheme::Float(Side::Left) => PositionScheme::FloatLeft,
        Scheme::Float(Side::Right) => PositionScheme::FloatRight,
        Scheme::Absolute => PositionScheme::Absolute,
        Scheme::Fixed => PositionScheme::Fixed,
    };
    Ok(BlockFragment {
        node: node.node,
        source_box: node.source,
        parent: node.parent,
        content,
        padding,
        border,
        margin: m.margin,
        scheme,
        relative: node.rel,
        containing_block: node.containing,
        overflow: node.overflow,
        clip: Clip::NONE,
        scrollable: None,
    })
}

/// An atomic failure to compute or dump the block reference layout.
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
    /// [`MAX_FLOW_WORK`] exceeded (float placement or retries).
    WorkLimit,
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
