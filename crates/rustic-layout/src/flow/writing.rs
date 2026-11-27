//! Vertical writing modes and orthogonal flows (CSS Writing Modes 4 §7)
//! by transposition.
//!
//! The flow engine is written for `horizontal-tb`. A subtree in a vertical
//! writing mode is laid out by that same engine in its *logical* space:
//! every box gets a transposed copy of its style (its block-start side as
//! `top`, its line-left side as `left`, `height` as `width`, …), the copy
//! is laid out, and every resulting fragment is mapped back to physical
//! coordinates. Block, float, positioned, flex, grid and table layout thus
//! all work in vertical modes, and percentages resolve against the right
//! axis.
//!
//! A *run* is a subtree laid out in one space: the root's (with the
//! principal writing mode, CSS Writing Modes 4 §8) and each box whose
//! writing mode is orthogonal to its parent's. A nested run is laid out
//! first, on its own, with its root sized as `fit-content` against the
//! initial containing block's size in its inline axis (or its containing
//! block's definite size, §7.3); its parent run then sees it as an empty
//! box of that size, and its fragments are grafted in after that box.
//! Runs are processed deepest first with an explicit list, never by
//! recursion.
//!
//! Glyphs on vertical lines are turned sideways ([`GlyphRotation`]);
//! upright typesetting, vertical font metrics, a parallel child with the
//! opposite block direction and transitions inside runs are later steps
//! (TD-212).

use std::collections::HashMap;
use std::sync::Arc;

use rustic_css::{
    BoxSizing, Display, Float, LengthPercentage, LengthPercentageOrAuto, MaxSize, Position,
    PropertyId as P, PropertyValue as V, Size, WritingMode,
};
use rustic_style::ComputedStyle;
use rustic_text::FontDatabase;

use super::{BlockLayout, FlowError};
use crate::overflow::AxisRange;
use crate::text::GlyphRotation;
use crate::tree::HtmlTag;
use crate::{
    BoxKind, BoxSnapshot, BoxTree, CssPoint, CssPx, CssRect, CssSize, Edges, LayoutLimits,
};
use rustic_base::CancellationToken;

fn mode_of(style: Option<&ComputedStyle>) -> WritingMode {
    style.map_or(WritingMode::HorizontalTb, ComputedStyle::writing_mode)
}

/// Whether any box needs the writing-mode path.
pub(super) fn needed(tree: &BoxTree) -> bool {
    tree.boxes()
        .iter()
        .any(|b| b.kind() != BoxKind::Text && mode_of(b.style()).is_vertical())
}

/// The mapping between a run's logical (`horizontal-tb`) space and
/// physical coordinates in a frame of `width` × `height`.
#[derive(Clone, Copy, Debug)]
struct Frame {
    mode: WritingMode,
    width: f64,
    height: f64,
}

/// Physical sides: 0 top, 1 right, 2 bottom, 3 left.
const CORNERS: [(usize, usize); 4] = [(0, 3), (0, 1), (2, 1), (2, 3)];

impl Frame {
    fn vertical(self) -> bool {
        self.mode.is_vertical()
    }

    /// Whether the block-start side is the right one.
    fn flipped(self) -> bool {
        self.mode.is_block_flipped()
    }

    /// Whether the line-left side is the bottom one.
    fn bottom(self) -> bool {
        self.mode == WritingMode::SidewaysLr
    }

    /// The physical side of logical side `l`.
    fn side(self, l: usize) -> usize {
        if !self.vertical() {
            return l;
        }
        let block_start = if self.flipped() { 1 } else { 3 };
        let line_left = if self.bottom() { 2 } else { 0 };
        [
            block_start,
            (line_left + 2) % 4,
            (block_start + 2) % 4,
            line_left,
        ][l]
    }

    /// The physical corner (in `CORNERS` order) of logical corner `c`.
    fn corner(self, c: usize) -> usize {
        let (a, b) = CORNERS[c];
        let (a, b) = (self.side(a), self.side(b));
        let (v, h) = if a % 2 == 0 { (a, b) } else { (b, a) };
        CORNERS
            .iter()
            .position(|&k| k == (v, h))
            .expect("two adjacent sides name a corner")
    }

    fn x(self, x: f64, y: f64) -> f64 {
        if !self.vertical() {
            x
        } else if self.flipped() {
            self.width - y
        } else {
            y
        }
    }

    fn y(self, x: f64, y: f64) -> f64 {
        if !self.vertical() {
            y
        } else if self.bottom() {
            self.height - x
        } else {
            x
        }
    }

    fn rect(self, r: CssRect) -> Result<CssRect, FlowError> {
        let (x, y) = (r.origin().x.get(), r.origin().y.get());
        let (w, h) = (r.size().width().get(), r.size().height().get());
        if !self.vertical() {
            return Ok(r);
        }
        let px = if self.flipped() {
            self.width - (y + h)
        } else {
            y
        };
        let py = if self.bottom() {
            self.height - (x + w)
        } else {
            x
        };
        rect(px, py, h, w)
    }

    fn vector(self, dx: f64, dy: f64) -> (f64, f64) {
        if !self.vertical() {
            return (dx, dy);
        }
        (
            if self.flipped() { -dy } else { dy },
            if self.bottom() { -dx } else { dx },
        )
    }

    fn edges(self, e: Edges) -> Edges {
        let logical = [e.top, e.right, e.bottom, e.left];
        let mut physical = logical;
        for (l, value) in logical.into_iter().enumerate() {
            physical[self.side(l)] = value;
        }
        Edges {
            top: physical[0],
            right: physical[1],
            bottom: physical[2],
            left: physical[3],
        }
    }

    /// A logical x range as a physical range, and on which axis.
    fn range_x(self, r: AxisRange) -> (bool, AxisRange) {
        if !self.vertical() {
            return (true, r);
        }
        (false, self.flip(r, self.bottom(), self.height))
    }

    fn range_y(self, r: AxisRange) -> (bool, AxisRange) {
        if !self.vertical() {
            return (false, r);
        }
        (true, self.flip(r, self.flipped(), self.width))
    }

    fn flip(self, r: AxisRange, flip: bool, extent: f64) -> AxisRange {
        if !flip {
            return r;
        }
        let px = |v: f64| CssPx::new(v).unwrap_or(CssPx::ZERO);
        AxisRange {
            start: px(extent - r.end.get()),
            end: px(extent - r.start.get()),
        }
    }

    fn rotation(self) -> GlyphRotation {
        match self.mode {
            WritingMode::HorizontalTb => GlyphRotation::None,
            WritingMode::SidewaysLr => GlyphRotation::Counterclockwise,
            _ => GlyphRotation::Clockwise,
        }
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Result<CssRect, FlowError> {
    let point = CssPoint {
        x: CssPx::new(x)?,
        y: CssPx::new(y)?,
    };
    let size = CssSize::new(w.max(0.0), h.max(0.0))?;
    CssRect::new(point, size).map_err(FlowError::from)
}

fn shift(r: CssRect, dx: f64, dy: f64) -> Result<CssRect, FlowError> {
    rect(
        r.origin().x.get() + dx,
        r.origin().y.get() + dy,
        r.size().width().get(),
        r.size().height().get(),
    )
}

fn px(v: f64) -> Result<CssPx, FlowError> {
    CssPx::new(v).map_err(FlowError::from)
}

const SIDES: [[P; 4]; 6] = [
    [P::MarginTop, P::MarginRight, P::MarginBottom, P::MarginLeft],
    [
        P::PaddingTop,
        P::PaddingRight,
        P::PaddingBottom,
        P::PaddingLeft,
    ],
    [P::Top, P::Right, P::Bottom, P::Left],
    [
        P::BorderTopWidth,
        P::BorderRightWidth,
        P::BorderBottomWidth,
        P::BorderLeftWidth,
    ],
    [
        P::BorderTopStyle,
        P::BorderRightStyle,
        P::BorderBottomStyle,
        P::BorderLeftStyle,
    ],
    [
        P::BorderTopColor,
        P::BorderRightColor,
        P::BorderBottomColor,
        P::BorderLeftColor,
    ],
];
const RADII: [P; 4] = [
    P::BorderTopLeftRadius,
    P::BorderTopRightRadius,
    P::BorderBottomRightRadius,
    P::BorderBottomLeftRadius,
];

/// `style` in the logical space of a run in `mode`.
fn transpose(style: &ComputedStyle, mode: WritingMode) -> ComputedStyle {
    // The horizontal engine ignores `writing-mode`; it keeps the run's mode
    // so inline layout knows which text is set upright.
    let mut out = style.clone();
    out.set(P::WritingMode, V::WritingMode(mode));
    if mode.is_horizontal() {
        return out;
    }
    let frame = Frame {
        mode,
        width: 0.0,
        height: 0.0,
    };
    for group in SIDES {
        for (l, &property) in group.iter().enumerate() {
            out.set(property, style.get(group[frame.side(l)]).clone());
        }
    }
    for (c, &property) in RADII.iter().enumerate() {
        out.set(property, style.get(RADII[frame.corner(c)]).clone());
    }
    for (a, b) in [
        (P::Width, P::Height),
        (P::MinWidth, P::MinHeight),
        (P::MaxWidth, P::MaxHeight),
        (P::OverflowX, P::OverflowY),
    ] {
        out.set(a, style.get(b).clone());
        out.set(b, style.get(a).clone());
    }
    if let V::BorderSpacing(h, v) = style.get(P::BorderSpacing) {
        out.set(P::BorderSpacing, V::BorderSpacing(v.clone(), h.clone()));
    }
    out
}

fn length(v: f64) -> LengthPercentage {
    LengthPercentage::Length(rustic_css::Length::px(v))
}

/// The block-level display of a run root inside its own run.
fn block_level(display: Display) -> Display {
    match display {
        Display::Inline | Display::InlineBlock => Display::FlowRoot,
        Display::InlineFlex => Display::Flex,
        Display::InlineGrid => Display::Grid,
        Display::InlineTable => Display::Table,
        other => other,
    }
}

/// The style of a nested run's root inside its own run: placed at the
/// origin and, with an `auto` inline size, sized `fit-content`.
fn run_root_style(style: &ComputedStyle, mode: WritingMode) -> ComputedStyle {
    let mut out = transpose(style, mode);
    let zero = V::LengthPercentageOrAuto(LengthPercentageOrAuto::LengthPercentage(
        LengthPercentage::ZERO,
    ));
    for &margin in &SIDES[0] {
        out.set(margin, zero.clone());
    }
    for &inset in &SIDES[2] {
        out.set(
            inset,
            V::LengthPercentageOrAuto(LengthPercentageOrAuto::Auto),
        );
    }
    // A positioned root stays a containing block for its run, without an
    // offset (its parent run positions its leaf).
    let positioned = !matches!(style.get(P::Position), V::Position(Position::Static));
    out.set(
        P::Position,
        V::Position(if positioned {
            Position::Relative
        } else {
            Position::Static
        }),
    );
    out.set(P::Float, V::Float(Float::None));
    if let V::Display(d) = style.get(P::Display) {
        out.set(P::Display, V::Display(block_level(*d)));
    }
    if matches!(out.get(P::Width), V::Size(Size::Auto)) {
        out.set(P::Width, V::Size(Size::FitContent(None)));
    }
    out
}

/// A nested run's root as its parent run sees it: an empty box of the
/// run's border-box size (`width` × `height` in the parent's space).
fn leaf_style(style: &ComputedStyle, mode: WritingMode, width: f64, height: f64) -> ComputedStyle {
    let mut out = transpose(style, mode);
    out.set(P::BoxSizing, V::BoxSizing(BoxSizing::BorderBox));
    out.set(P::Width, V::Size(Size::LengthPercentage(length(width))));
    out.set(P::Height, V::Size(Size::LengthPercentage(length(height))));
    out.set(P::MinWidth, V::Size(Size::Auto));
    out.set(P::MinHeight, V::Size(Size::Auto));
    out.set(P::MaxWidth, V::MaxSize(MaxSize::None));
    out.set(P::MaxHeight, V::MaxSize(MaxSize::None));
    if matches!(style.get(P::Display), V::Display(Display::Inline)) {
        out.set(P::Display, V::Display(Display::InlineBlock));
    }
    out
}

/// Whether box `i` may start an orthogonal run: an element that is not a
/// table-internal box, replaced element or form control.
fn can_start_run(b: &BoxSnapshot) -> bool {
    match b.kind() {
        BoxKind::Text => false,
        BoxKind::Element(d) => {
            !matches!(
                d,
                Display::TableRowGroup
                    | Display::TableHeaderGroup
                    | Display::TableFooterGroup
                    | Display::TableRow
                    | Display::TableCell
                    | Display::TableColumnGroup
                    | Display::TableColumn
                    | Display::TableCaption
                    | Display::Contents
                    | Display::None
            ) && b.natural_size().is_none()
                && !b.requires_special_layout()
        }
    }
}

/// One finished run in physical coordinates relative to its root's
/// border box (the viewport for the root run).
#[derive(Clone)]
struct Placed {
    layout: BlockLayout,
    /// Physical border-box size of a nested run's root.
    size: (f64, f64),
}

/// Marks lines of a nested run's (dropped) root until it is grafted.
const ROOT_BLOCK: usize = usize::MAX;

/// The logical space a nested run is laid out in: its wrapper's inline
/// size and, once its parent run has placed it, its containing block's
/// block size (so percentages resolve).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Space {
    inline: f64,
    block: Option<f64>,
}

/// One run's copy laid out in logical space.
struct Logical {
    layout: BlockLayout,
    /// Original box of each copy box (`None` for a wrapper).
    source: Vec<Option<usize>>,
    /// Copy index of each original box in the run.
    copy_of: HashMap<usize, usize>,
}

/// The box tree split into runs.
struct Runs<'a> {
    boxes: &'a [BoxSnapshot],
    modes: Vec<WritingMode>,
    ends: Vec<usize>,
    run_of: Vec<usize>,
    is_root: Vec<bool>,
    roots: Vec<usize>,
    /// The viewport's physical size.
    physical: (f64, f64),
    limits: LayoutLimits,
    fonts: Option<&'a FontDatabase>,
    cancellation: &'a CancellationToken,
}

fn display_kind(style: Option<&Arc<ComputedStyle>>, fallback: BoxKind) -> BoxKind {
    match style.map(|s| s.get(P::Display)) {
        Some(V::Display(d)) => BoxKind::Element(*d),
        _ => fallback,
    }
}

fn is_out_of_flow(style: Option<&ComputedStyle>) -> bool {
    style.is_some_and(|s| {
        matches!(
            s.get(P::Position),
            V::Position(Position::Absolute | Position::Fixed)
        )
    })
}

impl Runs<'_> {
    /// The viewport in the logical space of `mode`.
    fn viewport_in(&self, mode: WritingMode) -> (f64, f64) {
        if mode.is_vertical() {
            (self.physical.1, self.physical.0)
        } else {
            self.physical
        }
    }

    /// The nested runs whose parent run is `r`.
    fn children(&self, r: usize) -> Vec<usize> {
        self.roots
            .iter()
            .copied()
            .filter(|&q| q != r && self.boxes[q].parent().is_some_and(|p| self.run_of[p] == r))
            .collect()
    }

    /// Lay out run `r`'s copy: a wrapper root for a nested run (sized by
    /// `space`), the run's own boxes, and each nested run root as an empty
    /// leaf of its placed size.
    fn logical(
        &self,
        r: usize,
        space: Option<Space>,
        placed: &HashMap<usize, Placed>,
    ) -> Result<Logical, FlowError> {
        self.cancellation
            .check()
            .map_err(|_| FlowError::Cancelled)?;
        let boxes = self.boxes;
        let mode = self.modes[r];
        let nested = r != 0;
        let mut copy: Vec<BoxSnapshot> = Vec::new();
        let mut source: Vec<Option<usize>> = Vec::new();
        let mut copy_of: HashMap<usize, usize> = HashMap::new();
        let mut transposed: HashMap<*const ComputedStyle, Arc<ComputedStyle>> = HashMap::new();
        let viewport = self.viewport_in(mode);
        let mut size = viewport;
        if let Some(space) = space.filter(|_| nested) {
            let mut wrapper = ComputedStyle::initial().anonymous_flow(Display::Block);
            wrapper.set(
                P::Width,
                V::Size(Size::LengthPercentage(length(space.inline))),
            );
            if let Some(block) = space.block {
                wrapper.set(P::Height, V::Size(Size::LengthPercentage(length(block))));
            }
            size.0 = space.inline;
            copy.push(boxes[r].derived(
                None,
                Some(Arc::new(wrapper)),
                BoxKind::Element(Display::Block),
                true,
            ));
            source.push(None);
        }
        let mut i = r;
        while i < self.ends[r] {
            let b = &boxes[i];
            let parent = if i == r {
                nested.then_some(0)
            } else {
                let p = b.parent().ok_or(FlowError::InvalidSnapshot)?;
                Some(*copy_of.get(&p).ok_or(FlowError::InvalidSnapshot)?)
            };
            let original = b.style();
            let (style, kind, plain, next) = if i != r && self.is_root[i] {
                let child = placed.get(&i).ok_or(FlowError::InvalidSnapshot)?;
                let (w, h) = if mode.is_vertical() {
                    (child.size.1, child.size.0)
                } else {
                    child.size
                };
                let style = original.map(|s| Arc::new(leaf_style(s, mode, w, h)));
                let kind = display_kind(style.as_ref(), b.kind());
                (style, kind, true, self.ends[i])
            } else if i == r && nested {
                let style = original.map(|s| Arc::new(run_root_style(s, mode)));
                let kind = display_kind(style.as_ref(), b.kind());
                (style, kind, false, i + 1)
            } else {
                let style = b.shared_style().map(|shared| {
                    Arc::clone(
                        transposed
                            .entry(Arc::as_ptr(shared))
                            .or_insert_with(|| Arc::new(transpose(shared, mode))),
                    )
                });
                (style, b.kind(), false, i + 1)
            };
            copy_of.insert(i, copy.len());
            copy.push(b.derived(parent, style, kind, plain));
            source.push(Some(i));
            i = next;
        }
        let layout = BlockLayout::layout_flow(
            &BoxTree::from_boxes(copy),
            CssSize::new(size.0, size.1)?,
            self.limits,
            self.fonts,
            self.cancellation,
        )?;
        Ok(Logical {
            layout,
            source,
            copy_of,
        })
    }

    /// The space nested run `q` starts with, before its parent run is laid
    /// out: the initial containing block's extent in its inline axis, or
    /// its parent's definite physical size along that axis (CSS Writing
    /// Modes 4 §7.3).
    fn initial_space(&self, q: usize) -> Space {
        let inline = self.viewport_in(self.modes[q]).0;
        Space {
            inline: self.definite_inline(q).unwrap_or(inline),
            block: None,
        }
    }

    fn definite_inline(&self, q: usize) -> Option<f64> {
        let along = if self.modes[q].is_vertical() {
            P::Height
        } else {
            P::Width
        };
        self.boxes[q]
            .parent()
            .and_then(|p| self.boxes[p].style())
            .and_then(|s| match s.get(along) {
                V::Size(Size::LengthPercentage(LengthPercentage::Length(l))) => l.to_px(),
                _ => None,
            })
    }

    /// The space nested run `q` gets from where its parent run `parent`
    /// placed its leaf: an out-of-flow root's containing block padding box,
    /// or an in-flow root's containing block content box in its block axis.
    fn space_in(&self, q: usize, parent: &Logical) -> Result<Space, FlowError> {
        let fragments = &parent.layout.fragments;
        let copy = *parent.copy_of.get(&q).ok_or(FlowError::InvalidSnapshot)?;
        let Some(leaf) = fragments.iter().find(|f| f.source_box == copy) else {
            return Ok(self.initial_space(q));
        };
        let out_of_flow = is_out_of_flow(self.boxes[q].style());
        let block = leaf
            .containing_block
            .or(leaf.parent)
            .and_then(|c| fragments.get(c));
        let (w, h) = match block {
            Some(c) => {
                let r = if out_of_flow { c.padding } else { c.content };
                (r.size().width().get(), r.size().height().get())
            }
            None => self.viewport_in(self.modes[self.run_of[self.boxes[q].parent().unwrap_or(0)]]),
        };
        // Logical in the parent run → physical → logical in `q`'s run.
        let parent_vertical =
            self.modes[self.run_of[self.boxes[q].parent().unwrap_or(0)]].is_vertical();
        let (pw, ph) = if parent_vertical { (h, w) } else { (w, h) };
        let (inline, block) = if self.modes[q].is_vertical() {
            (ph, pw)
        } else {
            (pw, ph)
        };
        let inline = if out_of_flow {
            inline
        } else {
            self.definite_inline(q)
                .unwrap_or(self.viewport_in(self.modes[q]).0)
        };
        Ok(Space {
            inline,
            block: Some(block),
        })
    }

    /// Map run `r`'s logical layout to physical coordinates and graft the
    /// nested runs it holds.
    fn place(
        &self,
        r: usize,
        logical: Logical,
        placed: &HashMap<usize, Placed>,
    ) -> Result<Placed, FlowError> {
        let mode = self.modes[r];
        let nested = r != 0;
        let root_copy = *logical.copy_of.get(&r).ok_or(FlowError::InvalidSnapshot)?;
        let size = if nested {
            let f = logical
                .layout
                .fragments
                .iter()
                .find(|f| f.source_box == root_copy)
                .ok_or(FlowError::InvalidSnapshot)?;
            let (w, h) = (
                f.border.size().width().get(),
                f.border.size().height().get(),
            );
            if mode.is_vertical() { (h, w) } else { (w, h) }
        } else {
            self.physical
        };
        let frame = Frame {
            mode,
            width: size.0,
            height: size.1,
        };
        let mut layout = untranspose(
            logical.layout,
            frame,
            &logical.source,
            nested.then_some(root_copy),
        )?;
        // Last first, so earlier insertion points stay valid.
        let mut children = self.children(r);
        children.sort_unstable();
        for q in children.into_iter().rev() {
            let child = placed.get(&q).ok_or(FlowError::InvalidSnapshot)?;
            let Some(at) = layout.fragments.iter().position(|f| f.source_box == q) else {
                continue;
            };
            graft(&mut layout, at, child.layout.clone())?;
        }
        Ok(Placed { layout, size })
    }
}

pub(super) fn layout(
    tree: &BoxTree,
    viewport: CssSize,
    limits: LayoutLimits,
    fonts: Option<&FontDatabase>,
    cancellation: &CancellationToken,
) -> Result<BlockLayout, FlowError> {
    let boxes = tree.boxes();
    let n = boxes.len();
    if n == 0 {
        return BlockLayout::layout_flow(tree, viewport, limits, fonts, cancellation);
    }
    // Writing modes: the root takes the principal one (its `body` child's
    // when it has one, CSS Writing Modes 4 §8); text takes its parent's.
    let body =
        (1..n).find(|&i| boxes[i].parent() == Some(0) && boxes[i].html_tag() == HtmlTag::Body);
    let mut modes = Vec::with_capacity(n);
    for (i, b) in boxes.iter().enumerate() {
        let mode = match (b.parent(), b.kind()) {
            (None, _) => mode_of(body.map_or(b.style(), |body| boxes[body].style())),
            (Some(p), BoxKind::Text) => modes[p],
            (Some(_), _) if Some(i) == body => modes[0],
            (Some(_), _) => mode_of(b.style()),
        };
        modes.push(mode);
    }
    // Subtree ends (boxes are in preorder) and run roots.
    let mut ends: Vec<usize> = (1..=n).collect();
    for i in (0..n).rev() {
        if let Some(p) = boxes[i].parent() {
            ends[p] = ends[p].max(ends[i]);
        }
    }
    let mut run_of = vec![0; n];
    let mut roots = vec![0];
    let mut is_root = vec![false; n];
    is_root[0] = true;
    for i in 1..n {
        let p = boxes[i].parent().ok_or(FlowError::InvalidSnapshot)?;
        if can_start_run(&boxes[i]) && modes[i].is_vertical() != modes[p].is_vertical() {
            run_of[i] = i;
            roots.push(i);
            is_root[i] = true;
        } else {
            run_of[i] = run_of[p];
            // A box in a run uses the run's mode (a parallel child with
            // the opposite block direction is not supported, TD-212).
            modes[i] = modes[run_of[i]];
        }
    }
    let runs = Runs {
        boxes,
        modes,
        ends,
        run_of,
        is_root,
        roots,
        physical: (viewport.width().get(), viewport.height().get()),
        limits,
        fonts,
        cancellation,
    };
    // Deepest first. A run is laid out once with its nested runs at their
    // first sizes; each nested run is then laid out again in the space its
    // leaf's containing block gives, and if any changed size the parent
    // run is laid out again with the new leaf sizes.
    let mut placed: HashMap<usize, Placed> = HashMap::new();
    for &r in runs.roots.iter().rev() {
        let space = (r != 0).then(|| runs.initial_space(r));
        let mut logical = runs.logical(r, space, &placed)?;
        let children = runs.children(r);
        let mut changed = false;
        for &q in &children {
            let want = runs.space_in(q, &logical)?;
            let again = runs.logical(q, Some(want), &placed)?;
            let before = placed.get(&q).map(|p| p.size);
            let replaced = runs.place(q, again, &placed)?;
            changed |= before != Some(replaced.size);
            placed.insert(q, replaced);
        }
        if changed {
            logical = runs.logical(r, space, &placed)?;
        }
        let done = runs.place(r, logical, &placed)?;
        placed.insert(r, done);
    }
    placed
        .remove(&0)
        .map(|p| p.layout)
        .ok_or(FlowError::InvalidSnapshot)
}

/// Map a run's logical layout into physical coordinates in `frame`,
/// rewriting box indices to the original tree (`source`) and dropping a
/// nested run's wrapper and root fragments.
fn untranspose(
    mut layout: BlockLayout,
    frame: Frame,
    source: &[Option<usize>],
    root: Option<usize>,
) -> Result<BlockLayout, FlowError> {
    let original = |b: usize| source.get(b).copied().flatten();
    // Fragments kept, and where each old index goes.
    let mut keep: Vec<Option<usize>> = Vec::with_capacity(layout.fragments.len());
    let mut root_index = None;
    let mut next = 0;
    for f in &layout.fragments {
        let dropped =
            root.is_some() && (original(f.source_box).is_none() || Some(f.source_box) == root);
        if root.is_some() && Some(f.source_box) == root {
            root_index = Some(keep.len());
        }
        keep.push((!dropped).then(|| {
            next += 1;
            next - 1
        }));
    }
    let remap =
        |i: Option<usize>| -> Option<usize> { i.and_then(|i| keep.get(i).copied().flatten()) };
    let vertical_lr = frame.mode == WritingMode::VerticalLr;
    let rotation = frame.rotation();
    // Lines first: atomic inlines and text are mirrored inside them in
    // `vertical-lr`, where the line's over side is not its block start.
    for line in &mut layout.lines {
        let b = line.baseline.get();
        line.rect = frame.rect(line.rect)?;
        line.baseline = px(if frame.vertical() { frame.x(0.0, b) } else { b })?;
        line.rotation = rotation;
        line.block = if Some(line.block) == root_index {
            ROOT_BLOCK
        } else {
            remap(Some(line.block)).ok_or(FlowError::InvalidSnapshot)?
        };
    }
    let mirror = |line: usize, x: f64| -> f64 {
        layout.lines.get(line).map_or(x, |l| {
            let r = l.rect;
            2.0 * r.origin().x.get() + r.size().width().get() - x
        })
    };
    // Atomic inlines in vertical-lr move by their mirror offset, with
    // everything inside them (`delta`, cumulative in preorder).
    let mut delta = vec![0.0f64; layout.fragments.len()];
    let mut fragments = Vec::with_capacity(next);
    for (i, f) in layout.fragments.iter().enumerate() {
        let mut f = *f;
        f.content = frame.rect(f.content)?;
        f.padding = frame.rect(f.padding)?;
        f.border = frame.rect(f.border)?;
        if vertical_lr {
            let own = match f.line {
                Some(l) => {
                    mirror(l, f.border.origin().x.get() + f.border.size().width().get())
                        - f.border.origin().x.get()
                }
                None => 0.0,
            };
            delta[i] = own
                + f.parent
                    .map_or(0.0, |p| delta.get(p).copied().unwrap_or(0.0));
        }
        let dx = delta[i];
        f.content = shift(f.content, dx, 0.0)?;
        f.padding = shift(f.padding, dx, 0.0)?;
        f.border = shift(f.border, dx, 0.0)?;
        f.margin = frame.edges(f.margin);
        let (rx, ry) = frame.vector(f.relative.x.get(), f.relative.y.get());
        f.relative = CssPoint {
            x: px(rx)?,
            y: px(ry)?,
        };
        if let Some(s) = f.scrollable {
            f.scrollable = Some(shift(frame.rect(s)?, dx, 0.0)?);
        }
        f.clip = clip(frame, f.clip, dx);
        f.overflow = if frame.vertical() {
            (f.overflow.1, f.overflow.0)
        } else {
            f.overflow
        };
        if let Some(c) = f.collapsed.as_mut() {
            let logical = *c;
            for (l, side) in logical.iter().enumerate() {
                c[frame.side(l)] = side.and_then(|(b, s)| {
                    Some((original(b)?, u8::try_from(frame.side(usize::from(s))).ok()?))
                });
            }
        }
        if let Some(sticky) = f.sticky.as_mut() {
            sticky.scrollport = frame.rect(sticky.scrollport)?;
            sticky.border = frame.rect(sticky.border)?;
            sticky.containing = frame.rect(sticky.containing)?;
            let (insets, margin) = (sticky.insets, sticky.margin);
            for l in 0..4 {
                sticky.insets[frame.side(l)] = insets[l];
                sticky.margin[frame.side(l)] = margin[l];
            }
            sticky.scroll_container = remap(sticky.scroll_container);
        }
        if keep[i].is_some() {
            f.source_box = original(f.source_box).ok_or(FlowError::InvalidSnapshot)?;
            f.parent = remap(f.parent);
            f.containing_block = remap(f.containing_block);
            fragments.push(f);
        }
    }
    for text in &mut layout.texts {
        let line = text.line;
        let bx = text.baseline.get();
        text.rect = frame.rect(text.rect)?;
        for g in &mut text.glyphs {
            let (x, y) = (frame.x(g.x, g.y), frame.y(g.x, g.y));
            g.x = x;
            g.y = y;
        }
        text.baseline = px(if frame.vertical() {
            frame.x(0.0, bx)
        } else {
            bx
        })?;
        text.rotation = if text.upright {
            GlyphRotation::None
        } else {
            rotation
        };
        if vertical_lr {
            let r = text.rect;
            let x = mirror(line, r.origin().x.get() + r.size().width().get());
            text.rect = rect(
                x,
                r.origin().y.get(),
                r.size().width().get(),
                r.size().height().get(),
            )?;
            for g in &mut text.glyphs {
                g.x = mirror(line, g.x);
            }
            text.baseline = px(mirror(line, text.baseline.get()))?;
        }
        text.source_box = original(text.source_box).ok_or(FlowError::InvalidSnapshot)?;
    }
    for inline in &mut layout.inlines {
        let line = inline.line;
        for r in [&mut inline.border, &mut inline.padding, &mut inline.content] {
            *r = frame.rect(*r)?;
            if vertical_lr {
                let x = mirror(line, r.origin().x.get() + r.size().width().get());
                *r = rect(
                    x,
                    r.origin().y.get(),
                    r.size().width().get(),
                    r.size().height().get(),
                )?;
            }
        }
        inline.source_box = original(inline.source_box).ok_or(FlowError::InvalidSnapshot)?;
    }
    if vertical_lr {
        // Lines (and their texts and inline boxes) inside a moved atomic
        // inline move with it.
        let block_of: Vec<usize> = layout.lines.iter().map(|l| l.block).collect();
        // `delta` is indexed by old fragment index; lines now hold new
        // indices, so look the shift up through `keep`.
        let mut by_new = vec![0.0f64; fragments.len()];
        for (old, new) in keep.iter().enumerate() {
            if let Some(new) = new {
                by_new[*new] = delta[old];
            }
        }
        let dx_of = |line: usize| -> f64 {
            block_of
                .get(line)
                .and_then(|&b| by_new.get(b).copied())
                .unwrap_or(0.0)
        };
        let shifts: Vec<f64> = (0..layout.lines.len()).map(dx_of).collect();
        for (line, dx) in layout.lines.iter_mut().zip(&shifts) {
            line.rect = shift(line.rect, *dx, 0.0)?;
            line.baseline = px(line.baseline.get() + dx)?;
        }
        for text in &mut layout.texts {
            let dx = shifts.get(text.line).copied().unwrap_or(0.0);
            text.rect = shift(text.rect, dx, 0.0)?;
            text.baseline = px(text.baseline.get() + dx)?;
            for g in &mut text.glyphs {
                g.x += dx;
            }
        }
        for inline in &mut layout.inlines {
            let dx = shifts.get(inline.line).copied().unwrap_or(0.0);
            for r in [&mut inline.border, &mut inline.padding, &mut inline.content] {
                *r = shift(*r, dx, 0.0)?;
            }
        }
    }
    let order: Vec<usize> = layout
        .fragment_order
        .iter()
        .enumerate()
        .filter(|(i, _)| keep.get(*i).copied().flatten().is_some())
        .map(|(_, &o)| o)
        .collect();
    layout.fragment_order = order;
    layout.fragments = fragments;
    if root.is_none() {
        layout.viewport_scrollable = frame.rect(layout.viewport_scrollable)?;
        if frame.vertical() {
            layout.viewport_overflow = (layout.viewport_overflow.1, layout.viewport_overflow.0);
        }
    }
    Ok(layout)
}

fn clip(frame: Frame, c: crate::overflow::Clip, dx: f64) -> crate::overflow::Clip {
    let mut out = crate::overflow::Clip::NONE;
    for (range, from_x) in [(c.x, true), (c.y, false)] {
        let Some(range) = range else { continue };
        let (to_x, mut r) = if from_x {
            frame.range_x(range)
        } else {
            frame.range_y(range)
        };
        if to_x && dx != 0.0 {
            r = AxisRange {
                start: CssPx::new(r.start.get() + dx).unwrap_or(r.start),
                end: CssPx::new(r.end.get() + dx).unwrap_or(r.end),
            };
        }
        if to_x {
            out.x = Some(r);
        } else {
            out.y = Some(r);
        }
    }
    out
}

/// Insert `child` (a nested run, relative to its root's border box) after
/// fragment `at`, the run root's leaf box in `target`.
fn graft(target: &mut BlockLayout, at: usize, mut child: BlockLayout) -> Result<(), FlowError> {
    let origin = target.fragments[at].border.origin();
    let (ox, oy) = (origin.x.get(), origin.y.get());
    let k = child.fragments.len();
    let (l0, i0) = (target.lines.len(), target.inlines.len());
    let after = |i: usize| if i > at { i + k } else { i };
    for f in &mut target.fragments {
        f.parent = f.parent.map(after);
        f.containing_block = f.containing_block.map(after);
        if let Some(s) = f.sticky.as_mut() {
            s.scroll_container = s.scroll_container.map(after);
        }
    }
    for line in &mut target.lines {
        line.block = after(line.block);
    }
    let inside = |i: usize| i + at + 1;
    for f in &mut child.fragments {
        for r in [&mut f.content, &mut f.padding, &mut f.border] {
            *r = shift(*r, ox, oy)?;
        }
        if let Some(s) = f.scrollable {
            f.scrollable = Some(shift(s, ox, oy)?);
        }
        f.clip = offset_clip(f.clip, ox, oy);
        if let Some(s) = f.sticky.as_mut() {
            s.scrollport = shift(s.scrollport, ox, oy)?;
            s.border = shift(s.border, ox, oy)?;
            s.containing = shift(s.containing, ox, oy)?;
            s.scroll_container = s.scroll_container.map(inside);
        }
        f.parent = Some(f.parent.map_or(at, inside));
        f.containing_block = Some(f.containing_block.map_or(at, inside));
        f.line = f.line.map(|l| l + l0);
        f.inline_parent = f.inline_parent.map(|i| i + i0);
    }
    for line in &mut child.lines {
        line.block = if line.block == ROOT_BLOCK {
            at
        } else {
            inside(line.block)
        };
        line.rect = shift(line.rect, ox, oy)?;
        let along = if line.rotation == GlyphRotation::None {
            oy
        } else {
            ox
        };
        line.baseline = px(line.baseline.get() + along)?;
    }
    for text in &mut child.texts {
        text.line += l0;
        text.parent = text.parent.map(|i| i + i0);
        text.rect = shift(text.rect, ox, oy)?;
        let along = if text.rotation == GlyphRotation::None {
            oy
        } else {
            ox
        };
        text.baseline = px(text.baseline.get() + along)?;
        for g in &mut text.glyphs {
            g.x += ox;
            g.y += oy;
        }
    }
    for inline in &mut child.inlines {
        inline.line += l0;
        inline.parent = inline.parent.map(|i| i + i0);
        for r in [&mut inline.border, &mut inline.padding, &mut inline.content] {
            *r = shift(*r, ox, oy)?;
        }
    }
    let tail = target.fragments.split_off(at + 1);
    target.fragments.extend(child.fragments);
    target.fragments.extend(tail);
    let order_tail = target
        .fragment_order
        .split_off((at + 1).min(target.fragment_order.len()));
    target.fragment_order.extend(child.fragment_order);
    target.fragment_order.extend(order_tail);
    target.lines.extend(child.lines);
    target.line_order.extend(child.line_order);
    target.texts.extend(child.texts);
    target.inlines.extend(child.inlines);
    Ok(())
}

fn offset_clip(c: crate::overflow::Clip, ox: f64, oy: f64) -> crate::overflow::Clip {
    let move_range = |r: AxisRange, d: f64| AxisRange {
        start: CssPx::new(r.start.get() + d).unwrap_or(r.start),
        end: CssPx::new(r.end.get() + d).unwrap_or(r.end),
    };
    crate::overflow::Clip {
        x: c.x.map(|r| move_range(r, ox)),
        y: c.y.map(|r| move_range(r, oy)),
    }
}
