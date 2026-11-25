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
use std::{collections::HashMap, error::Error, fmt};

mod flex;

use crate::anchor;
use crate::exclusions::{Exclusions, Side};
use crate::height::HeightConstraint;
use crate::inline::{self, Entry, Ifc};
use crate::intrinsic::{self, Intrinsic};
use crate::lines::{self, LineArea, LineSet};
use crate::margins::MarginStrut;
use crate::overflow::{self, Clip};
use crate::position::{self, AbsoluteContext, AbsoluteHeight};
use crate::replaced::{self, NaturalSize};
use crate::sizing::measure_in_band;
use crate::sticky::StickyConstraint;
use crate::text::{self, InlineFragment, LineFragment, TextFragment};
use crate::tree::HtmlTag;
use crate::{
    BlockMeasure, BoxKind, BoxSnapshot, BoxTree, CssPoint, CssPx, CssRect, CssSize, Edges,
    GeometryError, LayoutLimits, MeasureError,
};
use rustic_text::FontDatabase;

/// Work units (box visits, float scans, chain steps) one layout may spend.
///
/// Float placement and formatting-context retries are superlinear in the
/// worst case, so they are metered rather than trusted (TD-44).
/// This is the default of [`crate::LayoutLimits::max_flow_work`].
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
    /// A positioning scheme that is not implemented here (no longer used:
    /// sticky positioning is supported).
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
    /// Normal flow, then shifted by [`BlockFragment::relative_offset`] to
    /// stay inside its scrollport ([`BlockFragment::sticky`]).
    Sticky,
}

/// One non-replaced block's immutable, physical used geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockFragment {
    node: NodeId,
    source_box: usize,
    parent: Option<usize>,
    pub(crate) content: CssRect,
    pub(crate) padding: CssRect,
    pub(crate) border: CssRect,
    margin: Edges,
    containing_width: CssPx,
    scheme: PositionScheme,
    relative: CssPoint,
    containing_block: Option<usize>,
    overflow: (Overflow, Overflow),
    clip: Clip,
    pub(crate) scrollable: Option<CssRect>,
    line: Option<usize>,
    sticky: Option<StickyConstraint>,
    /// For an atomic inline, its enclosing inline fragment on the line.
    pub(crate) inline_parent: Option<usize>,
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
    /// Resolved physical margins and padding for CSSOM.
    ///
    /// Margins exposed to CSSOM exclude unused space and float intrusion
    /// that the placement equation can fold into a non-auto margin.
    pub fn resolved_edges(self, style: &ComputedStyle) -> Result<(Edges, Edges), MeasureError> {
        let basis = self.containing_width.get();
        let resolve = |property, placed: CssPx| {
            crate::sizing::auto(style, property, basis)?.map_or(Ok(placed), |value| {
                CssPx::new(value).map_err(MeasureError::from)
            })
        };
        let horizontal_auto = |placed: CssPx| match self.scheme {
            PositionScheme::Absolute | PositionScheme::Fixed => placed,
            _ if placed < CssPx::ZERO => CssPx::ZERO,
            _ => placed,
        };
        let margin = Edges {
            top: resolve(P::MarginTop, self.margin.top)?,
            right: resolve(P::MarginRight, horizontal_auto(self.margin.right))?,
            bottom: resolve(P::MarginBottom, self.margin.bottom)?,
            left: resolve(P::MarginLeft, horizontal_auto(self.margin.left))?,
        };
        let (padding, _) = crate::sizing::box_edges(style, basis)?;
        Ok((margin, padding))
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
    /// For an atomic inline (`inline-block`, `inline-flex` or an inline
    /// replaced element), the index of its line in [`BlockLayout::lines`].
    pub const fn line(self) -> Option<usize> {
        self.line
    }
    /// For a sticky box, the inputs of its offset; the offset for the
    /// initial scroll position is already applied.
    pub const fn sticky(self) -> Option<StickyConstraint> {
        self.sticky
    }
}

/// A deterministic block reference layout. No glyphs or pixels yet.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockLayout {
    fragments: Vec<BlockFragment>,
    viewport_overflow: (Overflow, Overflow),
    viewport_scrollable: CssRect,
    lines: Vec<LineFragment>,
    texts: Vec<TextFragment>,
    inlines: Vec<InlineFragment>,
    /// Preorder position (flow node) of each fragment and of the block
    /// (possibly anonymous) that produced each line, for the fragment tree.
    pub(crate) fragment_order: Vec<usize>,
    pub(crate) line_order: Vec<usize>,
}

/// A metered work counter that also polls cancellation.
pub(crate) struct Work<'c> {
    spent: usize,
    limit: usize,
    cancellation: &'c CancellationToken,
}

impl Work<'_> {
    pub(crate) fn cancellation(&self) -> &CancellationToken {
        self.cancellation
    }

    pub(crate) fn charge(&mut self, units: usize) -> Result<(), FlowError> {
        let before = self.spent;
        self.spent = self
            .spent
            .checked_add(units.max(1))
            .filter(|&n| n <= self.limit)
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
    sticky: bool,
    bfc_root: bool,
    overflow: (Overflow, Overflow),
    containing: Option<usize>,
    intrinsic: Intrinsic,
    direction: Direction,
    /// Natural size of a replaced box; its descendants are not laid out.
    natural: Option<NaturalSize>,
    /// `display:flex` (block-level flex container).
    flex_container: bool,
    /// An in-flow child of a flex container.
    flex_item: bool,
    /// Already in [`Engine::absolute`] (flex items can be laid out twice).
    queued: bool,
    // Results of the current layout attempt.
    measure: BlockMeasure,
    height: Option<HeightConstraint>,
    content_height: CssPx,
    border_height: CssPx,
    /// Border origin: in the parent's child space, or relative to the
    /// containing padding box for absolute/fixed boxes.
    pos: (f64, f64),
    top_resolved: bool,
    /// An absolute or fixed box that [`Engine::layout_absolute`] placed.
    placed: bool,
    rel: CssPoint,
    /// Absolute/fixed: hypothetical margin edge in the parent's child space.
    static_pos: (f64, f64),
    /// An anonymous block wrapping an inline run (no fragment of its own).
    anonymous: bool,
    /// Index of this block container's inline formatting context, if any.
    inline: Option<usize>,
    /// An atomic inline: (inline formatting context, atomic index).
    atomic: Option<(usize, usize)>,
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
    /// For a flex container, its run in [`Engine::flexes`]; its children are
    /// laid out by the flex algorithm rather than this walk.
    flex: Option<usize>,
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
    // Block-level replaced elements are laid out; inline-level ones need
    // inline layout, and controls and foreign content stay unsupported.
    let replaced_block = b.natural_size().is_some()
        && matches!(
            b.kind(),
            BoxKind::Element(Display::Block | Display::FlowRoot)
        );
    if b.requires_special_layout() && !replaced_block {
        return Err(fail(UnsupportedFlow::SpecialElement));
    }
    if !matches!(
        b.kind(),
        BoxKind::Element(Display::Block | Display::FlowRoot | Display::Flex)
    ) {
        return Err(fail(UnsupportedFlow::FormattingContext));
    }
    b.style().ok_or(FlowError::InvalidSnapshot)
}

/// An atomic inline: `inline-block`, `inline-flex` or an inline replaced
/// element. Inline grids/tables, controls and foreign content stay refused.
fn supported_atomic(b: &BoxSnapshot) -> Result<&ComputedStyle, FlowError> {
    let fail = |reason| FlowError::Unsupported {
        node: b.node(),
        reason,
    };
    let replaced = b.natural_size().is_some();
    if b.requires_special_layout() && !replaced {
        return Err(fail(UnsupportedFlow::SpecialElement));
    }
    match b.kind() {
        BoxKind::Element(Display::InlineBlock | Display::InlineFlex) => {}
        BoxKind::Element(Display::Inline) if replaced => {}
        _ => return Err(fail(UnsupportedFlow::FormattingContext)),
    }
    b.style().ok_or(FlowError::InvalidSnapshot)
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
    /// Flex containers being laid out, innermost last.
    flexes: Vec<flex::FlexRun>,
    ifcs: Vec<Ifc<'a>>,
    /// Lines per inline formatting context, relative to its content origin.
    line_sets: Vec<Option<LineSet>>,
}

fn build_nodes<'a>(
    tree: &'a BoxTree,
    limits: LayoutLimits,
    cancellation: &CancellationToken,
) -> Result<Vec<Node<'a>>, FlowError> {
    let mut nodes: Vec<Node<'_>> = Vec::new();
    let mut index: Vec<Option<usize>> = Vec::with_capacity(tree.boxes().len());
    let mut text_units = 0usize;
    // Boxes inside a replaced element (fallback content) are not rendered.
    let mut pruned: Vec<bool> = Vec::with_capacity(tree.boxes().len());
    for (source, b) in tree.boxes().iter().enumerate() {
        cancellation.check().map_err(|_| FlowError::Cancelled)?;
        let hidden = b.parent().is_some_and(|p| {
            pruned.get(p).copied().unwrap_or(false) || tree.boxes()[p].natural_size().is_some()
        });
        pruned.push(hidden);
        if hidden {
            index.push(None);
            continue;
        }
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
        let node = new_node(&nodes, source, b, style, parent, depth)?;
        nodes.push(node);
        index.push(Some(nodes.len() - 1));
    }
    Ok(nodes)
}

/// A flow node for a supported block box (or an anonymous block).
fn new_node<'a>(
    nodes: &[Node<'a>],
    source: usize,
    b: &BoxSnapshot,
    style: &'a ComputedStyle,
    parent: Option<usize>,
    depth: usize,
) -> Result<Node<'a>, FlowError> {
    let parent_flex = parent.is_some_and(|p| nodes[p].flex_container);
    let containing_direction = match parent {
        Some(p) => nodes[p].direction,
        None => direction(style)?,
    };
    let (scheme, relative) = match style.get(P::Position) {
        V::Position(Position::Absolute) => (Scheme::Absolute, false),
        V::Position(Position::Fixed) => (Scheme::Fixed, false),
        V::Position(p) => {
            // `float` does not apply to flex items (CSS Flexbox 1 §3).
            let scheme = match float_side(style, containing_direction)? {
                Some(side) if !parent_flex => Scheme::Float(side),
                _ => Scheme::Flow,
            };
            (scheme, *p == Position::Relative)
        }
        _ => return Err(FlowError::InvalidSnapshot),
    };
    let sticky = matches!(style.get(P::Position), V::Position(Position::Sticky));
    let flex_item = parent_flex && scheme == Scheme::Flow;
    // The nearest positioned ancestor; `None` is the initial containing block.
    let containing = match scheme {
        Scheme::Fixed => None,
        Scheme::Absolute => {
            let mut c = parent;
            while let Some(p) = c {
                if nodes[p].relative
                    || nodes[p].sticky
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
    Ok(Node {
        source,
        parent,
        node: b.node(),
        style,
        end: nodes.len() + 1,
        depth,
        scheme,
        relative,
        sticky,
        bfc_root: false,
        overflow: overflow_pair(style)?,
        containing,
        intrinsic: Intrinsic::default(),
        direction: direction(style)?,
        natural: b.natural_size(),
        flex_container: matches!(
            b.kind(),
            BoxKind::Element(Display::Flex | Display::InlineFlex)
        ),
        flex_item,
        queued: false,
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
        placed: false,
        rel: CssPoint::default(),
        static_pos: (0.0, 0.0),
        anonymous: false,
        inline: None,
        atomic: None,
    })
}

/// How a child takes part in its block container's content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Participation {
    /// Inline-level content of an inline formatting context (including
    /// atomic inlines).
    Inline,
    /// Collapsible whitespace that disappears next to block-level siblings.
    Whitespace,
    /// A block-level box (including floats and absolutely positioned boxes).
    Block,
    /// An inline box with a block-level descendant, which splits it.
    Mixed,
}

fn participation(
    b: &BoxSnapshot,
    cancellation: &CancellationToken,
) -> Result<Participation, FlowError> {
    if inline::is_atomic(b) {
        return Ok(Participation::Inline);
    }
    Ok(match b.kind() {
        BoxKind::Text if ignorable_text(b, cancellation)? => Participation::Whitespace,
        BoxKind::Text => Participation::Inline,
        BoxKind::Element(Display::Inline) => {
            let style = b.style().ok_or(FlowError::InvalidSnapshot)?;
            let out_of_flow = matches!(
                style.get(P::Position),
                V::Position(Position::Absolute | Position::Fixed)
            ) || !matches!(style.get(P::Float), V::Float(Float::None));
            if out_of_flow {
                Participation::Block
            } else {
                Participation::Inline
            }
        }
        BoxKind::Element(_) => Participation::Block,
    })
}

/// [`participation`] of every box; an inline box with a block-level
/// descendant (through other inline boxes) is [`Participation::Mixed`].
fn participations(tree: &BoxTree, work: &mut Work<'_>) -> Result<Vec<Participation>, FlowError> {
    let boxes = tree.boxes();
    let mut kinds = Vec::with_capacity(boxes.len());
    for b in boxes {
        work.charge(1)?;
        kinds.push(participation(b, work.cancellation)?);
    }
    for i in (0..boxes.len()).rev() {
        if !matches!(kinds[i], Participation::Block | Participation::Mixed) {
            continue;
        }
        if let Some(p) = boxes[i].parent() {
            if kinds[p] == Participation::Inline && !inline::is_atomic(&boxes[p]) {
                kinds[p] = Participation::Mixed;
            }
        }
    }
    Ok(kinds)
}

/// One step of a block container's flattened content.
#[derive(Clone, Copy, Debug)]
enum Item {
    Block(usize),
    Inline(usize),
    Whitespace(usize),
    Open(usize),
    Close(usize),
}

/// A block container's content in document order, descending into inline
/// boxes that block-level descendants split (CSS 2.2 §9.2.1.1).
fn flatten(
    ends: &[usize],
    kinds: &[Participation],
    container: usize,
    work: &mut Work<'_>,
) -> Result<Vec<Item>, FlowError> {
    let mut out = Vec::new();
    let mut stack: Vec<(Option<usize>, usize, usize)> =
        vec![(None, container + 1, ends[container])];
    while let Some(top) = stack.last_mut() {
        let (owner, c, end) = *top;
        if c >= end {
            stack.pop();
            if let Some(o) = owner {
                out.push(Item::Close(o));
            }
            continue;
        }
        top.1 = ends[c];
        work.charge(1)?;
        match kinds[c] {
            Participation::Block => out.push(Item::Block(c)),
            Participation::Inline => out.push(Item::Inline(c)),
            Participation::Whitespace => out.push(Item::Whitespace(c)),
            Participation::Mixed => {
                out.push(Item::Open(c));
                stack.push((Some(c), c + 1, ends[c]));
            }
        }
    }
    Ok(out)
}

/// Subtree ends (exclusive) of every principal box, from parent links.
fn subtree_ends(tree: &BoxTree) -> Result<Vec<usize>, FlowError> {
    let boxes = tree.boxes();
    let mut ends: Vec<usize> = (1..=boxes.len()).collect();
    for i in (0..boxes.len()).rev() {
        if let Some(p) = boxes[i].parent() {
            if p >= i {
                return Err(FlowError::InvalidSnapshot);
            }
            ends[p] = ends[p].max(ends[i]);
        }
    }
    Ok(ends)
}

fn children_of(ends: &[usize], parent: Option<usize>) -> Vec<usize> {
    let (mut c, end) = match parent {
        Some(p) => (p + 1, ends[p]),
        None => (0, ends.len()),
    };
    let mut out = Vec::new();
    while c < end {
        out.push(c);
        c = ends[c];
    }
    out
}

/// Pending work of the text-aware node builder.
enum Pending {
    Box {
        source: usize,
        parent: Option<usize>,
        /// For an atomic inline: (inline formatting context, atomic index).
        atomic: Option<(usize, usize)>,
    },
    Anonymous {
        parent: usize,
        container: usize,
        entries: Vec<Entry>,
    },
}

/// Anonymous block styles for every block container that mixes inline
/// content with block-level boxes (CSS 2.2 §9.2.1.1).
fn anonymous_styles(
    tree: &BoxTree,
    ends: &[usize],
    kinds: &[Participation],
) -> Result<(Vec<ComputedStyle>, Vec<Option<usize>>), FlowError> {
    let boxes = tree.boxes();
    let mut styles = Vec::new();
    let mut of = vec![None; boxes.len()];
    for (i, b) in boxes.iter().enumerate() {
        let flex = match b.kind() {
            BoxKind::Element(Display::Block | Display::FlowRoot | Display::InlineBlock) => false,
            // Text directly in a flex container gets anonymous flex items.
            BoxKind::Element(Display::Flex | Display::InlineFlex) => true,
            _ => continue,
        };
        let (mut inline, mut block) = (false, flex);
        for c in children_of(ends, Some(i)) {
            match kinds[c] {
                Participation::Inline => inline = true,
                Participation::Block => block = true,
                Participation::Mixed => {
                    inline = true;
                    block = true;
                }
                Participation::Whitespace => {}
            }
        }
        if inline && block {
            let style = b.style().ok_or(FlowError::InvalidSnapshot)?;
            styles.push(style.anonymous_flow(Display::Block));
            of[i] = Some(styles.len() - 1);
        }
    }
    Ok((styles, of))
}

/// Build flow nodes when fonts are available: block containers with inline
/// content get an inline formatting context, inline runs next to
/// block-level siblings are wrapped in anonymous blocks (anonymous flex
/// items in flex containers), inline boxes are split around block-level
/// descendants, and atomic inlines become formatting-root nodes under the
/// block that owns their inline formatting context.
#[allow(
    clippy::too_many_arguments,
    reason = "the snapshot, its indexes and budgets"
)]
fn build_text_nodes<'a>(
    tree: &'a BoxTree,
    ends: &[usize],
    kinds: &[Participation],
    anonymous: &'a [ComputedStyle],
    anonymous_of: &[Option<usize>],
    fonts: &FontDatabase,
    limits: LayoutLimits,
    work: &mut Work<'_>,
    ifcs: &mut Vec<Ifc<'a>>,
) -> Result<Vec<Node<'a>>, FlowError> {
    let boxes = tree.boxes();
    let mut text_units = 0usize;
    for b in boxes {
        if b.kind() == BoxKind::Text {
            text_units = text_units
                .checked_add(b.text().len())
                .filter(|&n| n <= limits.max_text_units)
                .ok_or(FlowError::TextLimit)?;
        }
    }
    let mut nodes: Vec<Node<'a>> = Vec::new();
    let mut stack: Vec<Pending> = children_of(ends, None)
        .into_iter()
        .rev()
        .map(|source| Pending::Box {
            source,
            parent: None,
            atomic: None,
        })
        .collect();
    // Atomic inlines of a new inline formatting context, pushed so that
    // they are built next, in preorder, as children of `owner`.
    let push_atomics = |stack: &mut Vec<Pending>, ifc: &Ifc<'_>, k: usize, owner: usize| {
        for (a, item) in ifc.atomics.iter().enumerate().rev() {
            stack.push(Pending::Box {
                source: item.source,
                parent: Some(owner),
                atomic: Some((k, a)),
            });
        }
    };
    while let Some(pending) = stack.pop() {
        work.charge(1)?;
        let parent = match &pending {
            Pending::Box { parent, .. } => *parent,
            Pending::Anonymous { parent, .. } => Some(*parent),
        };
        let depth = match parent {
            Some(p) => nodes[p].depth.checked_add(1).ok_or(FlowError::DepthLimit)?,
            None => 0,
        };
        if depth > limits.max_depth {
            return Err(FlowError::DepthLimit);
        }
        match pending {
            Pending::Box {
                source,
                parent,
                atomic,
            } => {
                let b = &boxes[source];
                if b.kind() == BoxKind::Text {
                    if !ignorable_text(b, work.cancellation)? {
                        return Err(FlowError::Unsupported {
                            node: b.node(),
                            reason: UnsupportedFlow::TextLayout,
                        });
                    }
                    continue;
                }
                if nodes.len() >= limits.max_boxes {
                    return Err(FlowError::BoxLimit);
                }
                let style = match atomic {
                    Some(_) => supported_atomic(b)?,
                    None => supported_box(b)?,
                };
                let mut node = new_node(&nodes, source, b, style, parent, depth)?;
                node.atomic = atomic;
                nodes.push(node);
                let index = nodes.len() - 1;
                if let Some((k, a)) = atomic {
                    let item = ifcs
                        .get_mut(k)
                        .and_then(|ifc| ifc.atomics.get_mut(a))
                        .ok_or(FlowError::InvalidSnapshot)?;
                    item.node = index;
                }
                // Fallback content inside a replaced element is not rendered.
                if nodes[index].natural.is_some() {
                    continue;
                }
                let items = flatten(ends, kinds, source, work)?;
                let has_block = items.iter().any(|i| matches!(i, Item::Block(_)));
                let has_inline = items.iter().any(|i| matches!(i, Item::Inline(_)));
                if !has_block && !nodes[index].flex_container {
                    if has_inline {
                        let entries: Vec<Entry> = items
                            .iter()
                            .map(|i| match *i {
                                Item::Inline(c) | Item::Whitespace(c) => Ok(Entry::Box(c)),
                                _ => Err(FlowError::InvalidSnapshot),
                            })
                            .collect::<Result<_, _>>()?;
                        let ifc = inline::build(tree, ends, style, &entries, fonts, work)?;
                        ifcs.push(ifc);
                        let k = ifcs.len() - 1;
                        nodes[index].inline = Some(k);
                        push_atomics(&mut stack, &ifcs[k], k, index);
                    }
                    continue;
                }
                // Group inline runs; push in reverse so preorder is kept.
                let mut groups: Vec<Pending> = Vec::new();
                let mut run: Vec<Entry> = Vec::new();
                let mut run_inline = false;
                // Inline boxes split by the block-level boxes seen so far.
                let mut open: Vec<usize> = Vec::new();
                for item in items {
                    match item {
                        Item::Block(c) => {
                            if run_inline {
                                for &o in open.iter().rev() {
                                    run.push(Entry::Close {
                                        source: o,
                                        last: false,
                                    });
                                }
                                groups.push(Pending::Anonymous {
                                    parent: index,
                                    container: source,
                                    entries: std::mem::take(&mut run),
                                });
                            }
                            run.clear();
                            run_inline = false;
                            for &o in &open {
                                run.push(Entry::Open {
                                    source: o,
                                    first: false,
                                });
                            }
                            groups.push(Pending::Box {
                                source: c,
                                parent: Some(index),
                                atomic: None,
                            });
                        }
                        Item::Inline(c) => {
                            run.push(Entry::Box(c));
                            run_inline = true;
                        }
                        Item::Whitespace(c) => run.push(Entry::Box(c)),
                        Item::Open(c) => {
                            run.push(Entry::Open {
                                source: c,
                                first: true,
                            });
                            open.push(c);
                            run_inline = true;
                        }
                        Item::Close(c) => {
                            run.push(Entry::Close {
                                source: c,
                                last: true,
                            });
                            open.pop();
                            run_inline = true;
                        }
                    }
                }
                if run_inline {
                    groups.push(Pending::Anonymous {
                        parent: index,
                        container: source,
                        entries: run,
                    });
                }
                stack.extend(groups.into_iter().rev());
            }
            Pending::Anonymous {
                parent,
                container,
                entries,
            } => {
                if nodes.len() >= limits.max_boxes {
                    return Err(FlowError::BoxLimit);
                }
                let style = anonymous_of[container]
                    .and_then(|i| anonymous.get(i))
                    .ok_or(FlowError::InvalidSnapshot)?;
                let container_box = &boxes[nodes[parent].source];
                let mut node = new_node(
                    &nodes,
                    nodes[parent].source,
                    container_box,
                    style,
                    Some(parent),
                    depth,
                )?;
                node.anonymous = true;
                node.natural = None;
                node.flex_container = false;
                // In a flex container this is an anonymous flex item.
                node.flex_item = nodes[parent].flex_container;
                let ifc = inline::build(tree, ends, style, &entries, fonts, work)?;
                ifcs.push(ifc);
                let k = ifcs.len() - 1;
                node.inline = Some(k);
                nodes.push(node);
                let index = nodes.len() - 1;
                push_atomics(&mut stack, &ifcs[k], k, index);
            }
        }
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
    let body = nodes.iter().position(|n| {
        n.parent == Some(root) && !n.anonymous && tree.boxes()[n.source].html_tag() == HtmlTag::Body
    });
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
                || scroll_container(node.overflow)
                || node.natural.is_some()
                || node.flex_container
                || node.flex_item
                || node.atomic.is_some();
        }
        // Block-only intrinsic sizes, bottom up (CSS Sizing 3 §5).
        for i in (0..n).rev() {
            if let Some(natural) = self.nodes[i].natural {
                // Percentages are cyclic here and behave as `auto`.
                let (w, _) = replaced::used_size(self.nodes[i].style, natural, None, None)?;
                self.nodes[i].intrinsic = Intrinsic { min: w, max: w };
                continue;
            }
            if self.nodes[i].flex_container {
                self.nodes[i].intrinsic = self.flex_intrinsic(i)?;
                continue;
            }
            let (mut min, mut max) = (0.0_f64, 0.0_f64);
            let (mut left, mut right) = (0.0_f64, 0.0_f64);
            let mut c = i + 1;
            while c < self.nodes[i].end {
                self.work.charge(1)?;
                let child = &self.nodes[c];
                let next = child.end;
                // Atomic inlines contribute through their line content.
                if matches!(child.scheme, Scheme::Absolute | Scheme::Fixed)
                    || child.atomic.is_some()
                {
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
            if let Some(k) = self.nodes[i].inline {
                for a in 0..self.ifcs[k].atomics.len() {
                    self.work.charge(1)?;
                    let node = &self.nodes[self.ifcs[k].atomics[a].node];
                    let outer = intrinsic::outer_contribution(node.style, node.intrinsic)?;
                    self.ifcs[k].atomics[a].contribution = outer;
                }
                let text = lines::intrinsic(&self.ifcs[k], &mut self.work)?;
                min = min.max(text.min);
                max = max.max(text.max);
            }
            self.nodes[i].intrinsic = Intrinsic { min, max };
        }
        Ok(())
    }

    /// A replaced box's used content size against a containing block.
    fn replaced_size(
        &self,
        i: usize,
        width: f64,
        height: Option<CssPx>,
    ) -> Result<Option<(f64, f64)>, FlowError> {
        let n = &self.nodes[i];
        Ok(match n.natural {
            Some(natural) => Some(replaced::used_size(
                n.style,
                natural,
                Some(width),
                height.map(CssPx::get),
            )?),
            None => None,
        })
    }

    /// Fix a replaced box's content height after [`Self::begin`].
    fn force_height(&mut self, i: usize, size: Option<(f64, f64)>) -> Result<(), FlowError> {
        if let Some((_, h)) = size {
            self.nodes[i].height = Some(HeightConstraint::solved(Some(CssPx::new(h)?)));
        }
        Ok(())
    }

    /// The frame for a formatting-context root; a flex container starts its run.
    fn root_frame(&mut self, i: usize) -> Result<Frame, FlowError> {
        let mut frame = self.frame_for(i, 0.0)?;
        if self.nodes[i].flex_container {
            frame.flex = Some(self.start_flex(i)?);
            frame.next = frame.end;
        }
        Ok(frame)
    }

    fn push_absolute(&mut self, i: usize) {
        if !std::mem::replace(&mut self.nodes[i].queued, true) {
            self.absolute.push(i);
        }
    }

    fn truncate_absolute(&mut self, len: usize) {
        for &i in self.absolute.get(len..).unwrap_or(&[]) {
            self.nodes[i].queued = false;
        }
        self.absolute.truncate(len);
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
            flex: None,
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
            if let Some(k) = frame.flex {
                let Some(natural) = self.flex_step(&mut frames, &mut bfcs, k)? else {
                    continue;
                };
                frames.pop();
                bfcs.pop();
                let i = frame.node.ok_or(FlowError::InvalidSnapshot)?;
                if frames.is_empty() {
                    return Ok(natural);
                }
                retry = self.close_root(&frames, &mut bfcs, i, natural, &frame)?;
                continue;
            }
            if frame.next < frame.end {
                let i = frame.next;
                self.work.charge(1)?;
                if let Some(f) = frames.last_mut() {
                    f.next = self.nodes[i].end;
                }
                self.enter(&mut frames, &mut bfcs, i)?;
                continue;
            }
            if let Some(i) = frame.node {
                if self.nodes[i].inline.is_some() {
                    self.layout_inline(i, &frame, &mut bfcs)?;
                }
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
                retry = self.close_root(&frames, &mut bfcs, i, natural, &frame)?;
            } else {
                self.exit_block(&frames, &mut bfcs, i)?;
            }
        }
    }

    /// Finish a formatting-context root that is not the walk's own root.
    fn close_root(
        &mut self,
        frames: &[Frame],
        bfcs: &mut [Bfc],
        i: usize,
        natural: f64,
        frame: &Frame,
    ) -> Result<Option<(usize, f64, f64)>, FlowError> {
        if self.nodes[i].atomic.is_some() {
            self.set_height(i, natural)?;
            return self.atomic_done(i).map(|()| None);
        }
        if let Some(k) = frames.last().and_then(|f| f.flex) {
            self.flex_item_done(k, i, natural)?;
            return Ok(None);
        }
        self.set_height(i, natural)?;
        match self.nodes[i].scheme {
            Scheme::Float(_) => {
                self.exit_float(frames, bfcs, i)?;
                Ok(None)
            }
            Scheme::Flow => self.exit_flow_root(frames, bfcs, i, frame),
            Scheme::Absolute | Scheme::Fixed => Err(FlowError::InvalidSnapshot),
        }
    }

    /// Lay out the lines of block `i`'s inline formatting context at the
    /// current position of the innermost formatting context.
    fn layout_inline(
        &mut self,
        i: usize,
        frame: &Frame,
        bfcs: &mut [Bfc],
    ) -> Result<(), FlowError> {
        let k = self.nodes[i].inline.ok_or(FlowError::InvalidSnapshot)?;
        if !self.ifcs[k].has_content {
            self.line_sets[k] = None;
            return Ok(());
        }
        let len = bfcs.len();
        if bfcs[len - 1].pending {
            let at = bfcs[len - 1].y + bfcs[len - 1].strut.value()?.get();
            self.resolve(bfcs, at)?;
        }
        let bfc = &mut bfcs[len - 1];
        let area = LineArea {
            left: frame.content_x,
            width: frame.width.get(),
            top: bfc.y,
            exclusions: Some(&bfc.exclusions),
        };
        let (mut set, bottom) = lines::layout(&self.ifcs[k], &area, &mut self.work)?;
        if set.lines.is_empty() {
            self.line_sets[k] = None;
            return Ok(());
        }
        bfc.y = bottom;
        bfc.strut = MarginStrut::default();
        bfc.pending = false;
        // Atomic inlines: margin box on the line, in this block's child space.
        for placed in &set.atomics {
            self.work.charge(1)?;
            let node = self.ifcs[k].atomics[placed.atomic].node;
            let n = self.nodes.get_mut(node).ok_or(FlowError::InvalidSnapshot)?;
            let m = n.measure;
            n.pos = (
                placed.x + m.margin.left.get(),
                placed.y + m.margin.top.get(),
            );
            n.top_resolved = true;
        }
        let (ox, oy) = self.nodes[i].child_origin();
        lines::translate(&mut set, -ox, -oy);
        self.line_sets[k] = Some(set);
        Ok(())
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
                self.push_absolute(i);
                Ok(())
            }
            Scheme::Float(_) => {
                let replaced = self.replaced_size(i, pf.width.get(), pf.definite)?;
                let n = &self.nodes[i];
                let measure = position::measure_float(
                    n.style,
                    pf.width,
                    n.intrinsic,
                    replaced.map(|(w, _)| w),
                )?;
                self.begin(i, measure, &pf)?;
                self.force_height(i, replaced)?;
                frames.push(self.root_frame(i)?);
                bfcs.push(Bfc::default());
                Ok(())
            }
            Scheme::Flow if self.nodes[i].atomic.is_some() => {
                self.enter_atomic(frames, bfcs, i, &pf)
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

    /// Size an atomic inline against its block container and lay out its
    /// content; [`Self::layout_inline`] places it on a line afterwards.
    fn enter_atomic(
        &mut self,
        frames: &mut Vec<Frame>,
        bfcs: &mut Vec<Bfc>,
        i: usize,
        pf: &Frame,
    ) -> Result<(), FlowError> {
        let replaced = self.replaced_size(i, pf.width.get(), pf.definite)?;
        let n = &self.nodes[i];
        // `inline-block` widths are shrink-to-fit like floats (CSS 2.2 §10.3.9).
        let measure =
            position::measure_float(n.style, pf.width, n.intrinsic, replaced.map(|(w, _)| w))?;
        self.begin(i, measure, pf)?;
        self.force_height(i, replaced)?;
        self.nodes[i].top_resolved = true;
        frames.push(self.root_frame(i)?);
        bfcs.push(Bfc::default());
        Ok(())
    }

    /// Record a laid-out atomic inline's margin box and baseline (CSS 2.2
    /// §10.8.1: the last line box in normal flow; the bottom margin edge
    /// for replaced elements, `overflow` other than `visible`, or no line
    /// boxes; CSS Flexbox 1 §8.5: an `inline-flex` uses its first baseline).
    fn atomic_done(&mut self, i: usize) -> Result<(), FlowError> {
        let n = &self.nodes[i];
        let (k, a) = n.atomic.ok_or(FlowError::InvalidSnapshot)?;
        let m = n.measure;
        let width = m.margin.left.get() + n.border_width() + m.margin.right.get();
        let height = m.margin.top.get() + n.border_height.get() + m.margin.bottom.get();
        let content_top = m.margin.top.get() + m.border.top.get() + m.padding.top.get();
        let own = n.natural.is_none() && !scroll_container(n.overflow);
        let last = !n.flex_container;
        let baseline = match own {
            true => self.content_baseline(i, last)?,
            false => None,
        };
        let item = self
            .ifcs
            .get_mut(k)
            .and_then(|ifc| ifc.atomics.get_mut(a))
            .ok_or(FlowError::InvalidSnapshot)?;
        item.size = (width, height);
        item.baseline = baseline.map_or(height, |b| content_top + b);
        Ok(())
    }

    /// The first (or last) line box baseline in normal flow inside box `i`,
    /// below its content-box top. Lines inside nested formatting roots,
    /// floats, absolutely positioned boxes and atomic inlines are skipped;
    /// flex items count (their container's baseline comes from them).
    pub(super) fn content_baseline(
        &mut self,
        i: usize,
        last: bool,
    ) -> Result<Option<f64>, FlowError> {
        let end = self.nodes[i].end;
        let len = end - i;
        for step in 0..len {
            self.work.charge(1)?;
            let j = if last { end - 1 - step } else { i + step };
            let Some(k) = self.nodes[j].inline else {
                continue;
            };
            let Some(set) = self.line_sets[k].as_ref() else {
                continue;
            };
            let line = if last {
                set.lines.last()
            } else {
                set.lines.first()
            };
            let Some(line) = line else {
                continue;
            };
            let mut y = line.baseline;
            let mut p = j;
            let reachable = loop {
                self.work.charge(1)?;
                if p == i {
                    break true;
                }
                let n = &self.nodes[p];
                let Some(parent) = n.parent else {
                    break false;
                };
                if n.scheme != Scheme::Flow || n.atomic.is_some() || (n.bfc_root && !n.flex_item) {
                    break false;
                }
                y += n.pos.1 + n.edge().1 - self.nodes[parent].child_origin().1;
                p = parent;
            };
            if reachable {
                return Ok(Some(y));
            }
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
            None,
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
        let replaced = self.replaced_size(i, pf.width.get(), pf.definite)?;
        let forced = replaced.map(|(w, _)| w);
        let plain = measure_in_band(style, pf.width, pf.direction, (0.0, 0.0), inner, forced)?;
        self.begin(i, plain, &pf)?;
        self.force_height(i, replaced)?;
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
                forced,
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
        let mut frame = self.root_frame(i)?;
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
                self.truncate_absolute(own.checkpoint);
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

    /// The border box of the anchor named `name` for the absolutely
    /// positioned box `i`, in `containing`'s padding-box space: the last
    /// placed box in tree order with that `anchor-name`, other than `i` and
    /// its descendants (TD-216).
    fn anchor_rect(
        &mut self,
        i: usize,
        name: &str,
        containing: Option<usize>,
    ) -> Result<Option<anchor::Rect>, FlowError> {
        let end = self.nodes[i].end;
        let mut found = None;
        'search: for j in (0..self.nodes.len()).rev() {
            self.work.charge(1)?;
            let n = &self.nodes[j];
            if (i..end).contains(&j) || n.anonymous || !anchor::is_named(n.style, name) {
                continue;
            }
            // Every absolute box from here up must already be placed.
            let mut k = Some(j);
            while let Some(a) = k {
                self.work.charge(1)?;
                let n = &self.nodes[a];
                if matches!(n.scheme, Scheme::Absolute | Scheme::Fixed) && !n.placed {
                    continue 'search;
                }
                k = n.parent;
            }
            found = Some(j);
            break;
        }
        let Some(j) = found else {
            return Ok(None);
        };
        let n = &self.nodes[j];
        let origin = sub(n.child_origin(), n.edge());
        let (width, height) = (n.border_width(), n.border_height.get());
        let (x, y) = self.map_to_containing(Some(j), origin, None)?;
        let (cx, cy) = match containing {
            Some(c) => {
                let n = &self.nodes[c];
                let m = n.measure;
                let padding = sub(
                    n.child_origin(),
                    (m.padding.left.get(), m.padding.top.get()),
                );
                self.map_to_containing(Some(c), padding, None)?
            }
            None => (0.0, 0.0),
        };
        Ok(Some(anchor::Rect {
            x: x - cx,
            y: y - cy,
            width,
            height,
        }))
    }

    fn layout_absolute(&mut self, i: usize) -> Result<(), FlowError> {
        let result = self.layout_absolute_box(i);
        self.nodes[i].placed = true;
        result
    }

    fn layout_absolute_box(&mut self, i: usize) -> Result<(), FlowError> {
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
        let (parent, static_pos, base) = (n.parent, n.static_pos, n.style);
        let (static_x, static_y) = self.map_to_containing(parent, static_pos, containing)?;
        // Anchor functions and position-area: a substituted style, and a
        // grid region that replaces the containing block.
        let mut anchored = None;
        if anchor::uses_anchors(base) {
            let cb = anchor::Containing {
                width,
                height,
                direction: containing_direction,
            };
            let mut rects: Vec<(String, Option<anchor::Rect>)> = Vec::new();
            let mut wanted = Vec::new();
            anchor::resolve(base, &cb, |name| {
                wanted.push(name.to_owned());
                None
            });
            for name in wanted {
                if rects.iter().all(|(n, _)| *n != name) {
                    let rect = self.anchor_rect(i, &name, containing)?;
                    rects.push((name, rect));
                }
            }
            anchored = anchor::resolve(base, &cb, |name| {
                rects.iter().find(|(n, _)| n == name).and_then(|(_, r)| *r)
            });
        }
        let region = anchored.as_ref().and_then(|(_, region)| *region);
        let style: &ComputedStyle = anchored.as_ref().map_or(base, |(style, _)| style);
        let (width, height, offset) = match region {
            Some(r) => (r.rect.width, r.rect.height, (r.rect.x, r.rect.y)),
            None => (width, height, (0.0, 0.0)),
        };
        let cx = AbsoluteContext {
            width,
            height,
            static_x: static_x - offset.0,
            static_y: static_y - offset.1,
            static_direction,
            containing_direction,
        };
        let replaced = self.replaced_size(i, cx.width, CssPx::new(cx.height).ok())?;
        let n = &self.nodes[i];
        let horizontal =
            position::absolute_width(style, &cx, n.intrinsic, replaced.map(|(w, _)| w))?;
        let m = horizontal.measure;
        let vertical =
            AbsoluteHeight::new(style, &cx, m.padding, m.border, replaced.map(|(_, h)| h))?;
        let definite = vertical.definite().map(CssPx::new).transpose()?;
        let extras = {
            let n = &mut self.nodes[i];
            n.measure = m;
            n.vertical_extras()?
        };
        // Children see the solved height as definite only if it is not content-based.
        self.nodes[i].height = Some(HeightConstraint::solved(definite));
        let frame = self.root_frame(i)?;
        let natural = self.walk(frame)?;
        let solved = vertical.solve(natural)?;
        let n = &mut self.nodes[i];
        n.measure.margin.top = CssPx::new(solved.margin_start)?;
        n.measure.margin.bottom = CssPx::new(solved.margin_end)?;
        n.content_height = CssPx::new(solved.size.max(0.0))?;
        n.border_height = n.content_height.checked_add(extras)?;
        n.pos = (
            horizontal.left + m.margin.left.get() + offset.0,
            solved.start + solved.margin_start + offset.1,
        );
        if let Some(region) = region {
            if region.center_x {
                let outer = m.margin.left.get() + n.border_width() + m.margin.right.get();
                n.pos.0 += (width - outer) / 2.0;
            }
            if region.center_y {
                let outer = solved.margin_start + n.border_height.get() + solved.margin_end;
                n.pos.1 += (height - outer) / 2.0;
            }
        }
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
            flex: None,
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
    /// `max_flow_work` and cancellation apply independently of snapshot
    /// construction.
    pub fn compute(
        tree: &BoxTree,
        viewport: CssSize,
        limits: LayoutLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, FlowError> {
        Self::layout(tree, viewport, limits, None, cancellation)
    }

    /// [`Self::compute`] with inline formatting: text, `display:inline`
    /// boxes, `<br>` and `<wbr>` inside block containers are shaped with
    /// `fonts` and laid out in line boxes (REN-03b). Inline runs beside
    /// block-level siblings get anonymous blocks, which have no fragment of
    /// their own; their lines belong to the enclosing block. Atomic inlines
    /// (`inline-block`, replaced elements), floats or absolutely positioned
    /// boxes inside inline boxes, and blocks inside inline boxes still fail
    /// with a typed error (TD-50).
    pub fn compute_with_fonts(
        tree: &BoxTree,
        viewport: CssSize,
        limits: LayoutLimits,
        fonts: &FontDatabase,
        cancellation: &CancellationToken,
    ) -> Result<Self, FlowError> {
        Self::layout(tree, viewport, limits, Some(fonts), cancellation)
    }

    fn layout(
        tree: &BoxTree,
        viewport: CssSize,
        limits: LayoutLimits,
        fonts: Option<&FontDatabase>,
        cancellation: &CancellationToken,
    ) -> Result<Self, FlowError> {
        cancellation.check().map_err(|_| FlowError::Cancelled)?;
        if tree.boxes().len() > limits.max_nodes {
            return Err(FlowError::NodeLimit);
        }
        let mut work = Work {
            spent: 0,
            limit: limits.max_flow_work,
            cancellation,
        };
        let ends;
        let anonymous;
        let mut ifcs = Vec::new();
        let mut nodes = match fonts {
            None => build_nodes(tree, limits, cancellation)?,
            Some(fonts) => {
                ends = subtree_ends(tree)?;
                let kinds = participations(tree, &mut work)?;
                let (styles, of) = anonymous_styles(tree, &ends, &kinds)?;
                anonymous = styles;
                build_text_nodes(
                    tree, &ends, &kinds, &anonymous, &of, fonts, limits, &mut work, &mut ifcs,
                )?
            }
        };
        let viewport_overflow = propagate_overflow(&mut nodes, tree);
        let root_direction = nodes.first().map_or(Direction::Ltr, |n| n.direction);
        let line_sets = vec![None; ifcs.len()];
        let mut engine = Engine {
            nodes,
            viewport,
            root_direction,
            absolute: Vec::new(),
            flexes: Vec::new(),
            work,
            ifcs,
            line_sets,
        };
        engine.run()?;
        engine.finish(viewport_overflow)
    }

    /// Physical block fragments, in principal-box preorder.
    pub fn fragments(&self) -> &[BlockFragment] {
        &self.fragments
    }

    /// Line boxes, grouped by block in fragment order, top to bottom.
    pub fn lines(&self) -> &[LineFragment] {
        &self.lines
    }

    /// Glyph runs, by line and then in visual left-to-right order.
    pub fn texts(&self) -> &[TextFragment] {
        &self.texts
    }

    /// Inline box fragments (one per box per line), by line, in box preorder.
    pub fn inlines(&self) -> &[InlineFragment] {
        &self.inlines
    }

    pub(crate) fn fragments_mut(&mut self) -> &mut [BlockFragment] {
        &mut self.fragments
    }

    pub(crate) fn lines_mut(&mut self) -> &mut [LineFragment] {
        &mut self.lines
    }

    pub(crate) fn texts_mut(&mut self) -> &mut [TextFragment] {
        &mut self.texts
    }

    pub(crate) fn inlines_mut(&mut self) -> &mut [InlineFragment] {
        &mut self.inlines
    }

    /// Rounded widths of the untransformed border-box unions for CSSOM View.
    /// For a split inline, include the nonempty block fragments that split it.
    pub fn offset_widths(
        &self,
        tree: &BoxTree,
        cancellation: &CancellationToken,
    ) -> Result<HashMap<NodeId, i32>, FlowError> {
        let mut work = Work {
            spent: 0,
            limit: MAX_FLOW_WORK,
            cancellation,
        };
        work.charge(0)?;
        let mut bounds = HashMap::<NodeId, (f64, f64)>::new();
        let mut include = |node, rect: CssRect| {
            let left = rect.origin().x.get();
            let right = left + rect.size().width().get();
            let span = bounds.entry(node).or_insert((left, right));
            span.0 = span.0.min(left);
            span.1 = span.1.max(right);
        };
        let boxes = tree.boxes();
        for fragment in &self.fragments {
            work.charge(1)?;
            let rect = fragment.border();
            include(fragment.node(), rect);
            if fragment.line().is_some()
                || rect.size().width().get() == 0.0
                || rect.size().height().get() == 0.0
            {
                continue;
            }
            let mut parent = boxes
                .get(fragment.source_box())
                .ok_or(FlowError::InvalidSnapshot)?
                .parent();
            while let Some(index) = parent {
                work.charge(1)?;
                let source = boxes.get(index).ok_or(FlowError::InvalidSnapshot)?;
                if source.kind() != BoxKind::Element(Display::Inline)
                    || inline::is_atomic(source)
                    || participation(source, cancellation)? != Participation::Inline
                {
                    break;
                }
                include(source.node(), rect);
                parent = source.parent();
            }
        }
        for fragment in &self.inlines {
            work.charge(1)?;
            include(fragment.node(), fragment.border());
        }
        cancellation.check().map_err(|_| FlowError::Cancelled)?;
        Ok(bounds
            .into_iter()
            .map(|(node, (left, right))| (node, (right - left).round() as i32))
            .collect())
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
                PositionScheme::Sticky => Some("sticky"),
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
            if let Some(l) = f.line {
                line.push_str(&format!(" line={l}"));
            }
            line.push('\n');
            push(line, &mut out)?;
        }
        for (i, l) in self.lines.iter().enumerate() {
            push(
                format!(
                    "line {i} block={} rect=({}) baseline={}\n",
                    l.block(),
                    rect(l.rect()),
                    l.baseline().get()
                ),
                &mut out,
            )?;
        }
        for (i, t) in self.texts.iter().enumerate() {
            let range = t.text();
            push(
                format!(
                    "text {i} line={} box={} node={}:{} utf16={}..{} rect=({}) baseline={} level={} face={} size={} glyphs={}\n",
                    t.line(),
                    t.source_box(),
                    t.node().slot(),
                    t.node().generation(),
                    range.start,
                    range.end,
                    rect(t.rect()),
                    t.baseline().get(),
                    t.bidi_level(),
                    t.face(),
                    t.font_size(),
                    t.glyphs().len()
                ),
                &mut out,
            )?;
        }
        for (i, f) in self.inlines.iter().enumerate() {
            push(
                format!(
                    "inline {i} line={} box={} node={}:{} border=({}){}{}\n",
                    f.line(),
                    f.source_box(),
                    f.node().slot(),
                    f.node().generation(),
                    rect(f.border()),
                    if f.starts() { " start" } else { "" },
                    if f.ends() { " end" } else { "" }
                ),
                &mut out,
            )?;
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
    /// The sticky inputs of box `i` with unshifted border origin `origin`
    /// and containing-block content origin `content`; `borders` holds the
    /// final border origins of earlier boxes in preorder.
    fn sticky_constraint(
        &mut self,
        i: usize,
        origin: (f64, f64),
        content: (f64, f64),
        borders: &[(f64, f64)],
    ) -> Result<StickyConstraint, FlowError> {
        let rect = |(x, y): (f64, f64), w: f64, h: f64| -> Result<CssRect, FlowError> {
            Ok(CssRect::new(
                CssPoint {
                    x: CssPx::new(x)?,
                    y: CssPx::new(y)?,
                },
                CssSize::new(w.max(0.0), h.max(0.0))?,
            )?)
        };
        let n = &self.nodes[i];
        let p = n.parent.ok_or(FlowError::InvalidSnapshot)?;
        let parent = &self.nodes[p];
        let containing = rect(
            content,
            parent.measure.content_width.get(),
            parent.content_height.get(),
        )?;
        let border = rect(origin, n.border_width(), n.border_height.get())?;
        // The nearest ancestor scroll container, else the viewport.
        let mut a = Some(p);
        let mut port = None;
        while let Some(x) = a {
            self.work.charge(1)?;
            if scroll_container(self.nodes[x].overflow) {
                port = Some(x);
                break;
            }
            a = self.nodes[x].parent;
        }
        let scrollport = match port {
            Some(x) => {
                let s = &self.nodes[x];
                let m = s.measure;
                rect(
                    add(borders[x], (m.border.left.get(), m.border.top.get())),
                    m.content_width.get() + m.padding.left.get() + m.padding.right.get(),
                    s.content_height.get() + m.padding.top.get() + m.padding.bottom.get(),
                )?
            }
            None => rect(
                (0.0, 0.0),
                self.viewport.width().get(),
                self.viewport.height().get(),
            )?,
        };
        let (w, h) = (
            scrollport.size().width().get(),
            scrollport.size().height().get(),
        );
        let style = n.style;
        let inset = |property, basis| crate::sizing::auto(style, property, basis);
        let m = n.measure.margin;
        Ok(StickyConstraint {
            scroll_container: port,
            scrollport,
            insets: [
                inset(P::Top, h)?,
                inset(P::Right, w)?,
                inset(P::Bottom, h)?,
                inset(P::Left, w)?,
            ],
            border,
            margin: [m.top.get(), m.right.get(), m.bottom.get(), m.left.get()],
            containing,
        })
    }

    fn finish(mut self, viewport_overflow: (Overflow, Overflow)) -> Result<BlockLayout, FlowError> {
        let n = self.nodes.len();
        let mut borders: Vec<(f64, f64)> = Vec::with_capacity(n);
        let mut sticky: Vec<Option<StickyConstraint>> = vec![None; n];
        for (i, slot) in sticky.iter_mut().enumerate() {
            self.work.charge(1)?;
            if self.nodes[i].sticky {
                if let Some(p) = self.nodes[i].parent {
                    let parent = &self.nodes[p];
                    let content = add(borders[p], parent.edge());
                    let origin = add(content, sub(self.nodes[i].pos, parent.child_origin()));
                    let c = self.sticky_constraint(i, origin, content, &borders)?;
                    self.nodes[i].rel = c.offset(CssPoint::default())?;
                    *slot = Some(c);
                }
            }
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
        let mut fragments: Vec<BlockFragment> = Vec::with_capacity(n);
        for (i, node) in self.nodes.iter().enumerate() {
            let containing_width = match node.scheme {
                Scheme::Absolute | Scheme::Fixed => node.containing.map_or_else(
                    || self.viewport.width(),
                    |parent| fragments[parent].padding().size().width(),
                ),
                _ => node.parent.map_or_else(
                    || self.viewport.width(),
                    |parent| self.nodes[parent].measure.content_width,
                ),
            };
            let mut f = fragment(node, borders[i], containing_width)?;
            f.sticky = sticky[i];
            fragments.push(f);
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
        // Anonymous blocks have no fragment of their own: their lines belong
        // to the enclosing block, and public indexes skip them.
        let mut public: Vec<Option<usize>> = Vec::with_capacity(n);
        let mut count = 0;
        for node in &self.nodes {
            if node.anonymous {
                public.push(None);
            } else {
                public.push(Some(count));
                count += 1;
            }
        }
        let owner = |i: usize| -> usize {
            match self.nodes[i].parent {
                Some(p) if self.nodes[i].anonymous => p,
                _ => i,
            }
        };
        let mut lines_out = Vec::new();
        let mut line_order = Vec::new();
        let mut texts_out = Vec::new();
        let mut inlines_out = Vec::new();
        for i in 0..n {
            let Some(k) = self.nodes[i].inline else {
                continue;
            };
            let Some(set) = self.line_sets[k].take() else {
                continue;
            };
            let origin = fragments[i].content().origin();
            let block = public[owner(i)].ok_or(FlowError::InvalidSnapshot)?;
            let base = lines_out.len();
            let atomics = set.atomics.clone();
            let index = text::convert(
                &self.ifcs[k],
                set,
                (origin.x.get(), origin.y.get()),
                block,
                &mut lines_out,
                &mut texts_out,
                &mut inlines_out,
                &mut self.work,
            )?;
            line_order.resize(lines_out.len(), i);
            for placed in &atomics {
                let item = &self.ifcs[k].atomics[placed.atomic];
                let f = fragments
                    .get_mut(item.node)
                    .ok_or(FlowError::InvalidSnapshot)?;
                f.line = Some(base + placed.line);
                f.inline_parent = item
                    .parent
                    .and_then(|p| index.get(&(placed.line, p)).copied());
            }
        }
        let mut kept = Vec::with_capacity(count);
        let mut fragment_order = Vec::with_capacity(count);
        for (i, mut f) in fragments.into_iter().enumerate() {
            if self.nodes[i].anonymous {
                continue;
            }
            fragment_order.push(i);
            f.parent = f.parent.and_then(|p| public[owner(p)]);
            f.containing_block = f.containing_block.and_then(|c| public[owner(c)]);
            if let Some(c) = f.sticky.as_mut() {
                c.scroll_container = c.scroll_container.and_then(|s| public[owner(s)]);
            }
            kept.push(f);
        }
        Ok(BlockLayout {
            fragments: kept,
            viewport_overflow,
            viewport_scrollable,
            lines: lines_out,
            texts: texts_out,
            inlines: inlines_out,
            fragment_order,
            line_order,
        })
    }
}

fn fragment(
    node: &Node<'_>,
    border_origin: (f64, f64),
    containing_width: CssPx,
) -> Result<BlockFragment, FlowError> {
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
        Scheme::Flow if node.sticky => PositionScheme::Sticky,
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
        containing_width,
        scheme,
        relative: node.rel,
        containing_block: node.containing,
        overflow: node.overflow,
        clip: Clip::NONE,
        scrollable: None,
        line: None,
        sticky: None,
        inline_parent: None,
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
