//! Flex layout (CSS Flexible Box Layout 1 §9, reviewed 2026-11-09) as a
//! formatting context inside the block walk.
//!
//! A flex container's frame drives a small state machine. Each phase may ask
//! for flex items to be laid out at a given width (and optionally a definite
//! height); the walk lays each one out as an independent formatting root with
//! the same explicit frame stack and reports its natural content height back.
//! There is no recursion: nested flex containers are just more frames.
//!
//! Row containers: flex base sizes come from intrinsic widths, flexible
//! lengths are resolved per line, items are laid out at their used widths to
//! find their heights, and stretched items are laid out again at their
//! stretched height. Column containers: items are laid out at their cross
//! width to find content heights, flexible lengths are resolved, and items
//! are laid out again at their used heights.

use rustic_css::{
    AlignKeyword as K, Alignment, Direction, FlexBasis, FlexDirection, FlexWrap, Gap,
    OverflowPosition, PropertyId as P, PropertyValue as V, Size,
};
use rustic_style::ComputedStyle;

use super::{Bfc, Engine, FlowError, Frame, Scheme, scroll_container};
use crate::height::HeightConstraint;
use crate::intrinsic::{self, Intrinsic, Resolver, Sizing};
use crate::sizing::{auto, box_edges, lp};
use crate::{BlockMeasure, CssPx, Edges, position, replaced};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// Row: items are being laid out at their used widths.
    RowCross,
    /// Row: stretched items are being laid out at their stretched heights.
    RowStretch,
    /// Column: items are being laid out at their cross widths.
    ColumnBase,
    /// Column: items are being laid out at their used heights.
    ColumnMain,
}

#[derive(Clone, Copy, Debug)]
struct Request {
    item: usize,
    width: f64,
    height: Option<f64>,
}

#[derive(Clone, Copy, Debug)]
struct Line {
    start: usize,
    end: usize,
    cross: f64,
    pos: f64,
}

#[derive(Clone, Copy, Debug)]
struct Item {
    node: usize,
    padding: Edges,
    border: Edges,
    /// Margins with `auto` as zero, and which ones were `auto`
    /// (top, right, bottom, left).
    margin: Edges,
    auto_margin: [bool; 4],
    grow: f64,
    shrink: f64,
    align: Alignment,
    /// Content-box flex base size, hypothetical size and bounds (main axis).
    base: f64,
    hypo: f64,
    min: f64,
    max: f64,
    target: f64,
    frozen: bool,
    /// Content-box width used for the latest layout request.
    width: f64,
    /// Content height reported by the latest layout.
    natural: f64,
    /// The definite height of the latest layout request, if any.
    laid: Option<f64>,
    /// Content-box cross size.
    cross: f64,
    main_pos: f64,
    cross_pos: f64,
}

impl Item {
    fn horizontal_extras(&self) -> f64 {
        self.padding.left.get()
            + self.padding.right.get()
            + self.border.left.get()
            + self.border.right.get()
    }
    fn vertical_extras(&self) -> f64 {
        self.padding.top.get()
            + self.padding.bottom.get()
            + self.border.top.get()
            + self.border.bottom.get()
    }
    fn horizontal_margins(&self) -> f64 {
        self.margin.left.get() + self.margin.right.get()
    }
    fn vertical_margins(&self) -> f64 {
        self.margin.top.get() + self.margin.bottom.get()
    }
    /// Main-axis padding, border and margins.
    fn main_outer(&self, row: bool) -> f64 {
        if row {
            self.horizontal_extras() + self.horizontal_margins()
        } else {
            self.vertical_extras() + self.vertical_margins()
        }
    }
    fn cross_outer(&self, row: bool) -> f64 {
        self.main_outer(!row)
    }
    /// Whether either cross-axis margin is `auto`.
    fn cross_auto(&self, row: bool) -> bool {
        if row {
            self.auto_margin[0] || self.auto_margin[2]
        } else {
            self.auto_margin[1] || self.auto_margin[3]
        }
    }
}

/// One flex container's layout state.
pub(super) struct FlexRun {
    container: usize,
    row: bool,
    reverse: bool,
    wrap: bool,
    wrap_reverse: bool,
    direction: Direction,
    width: f64,
    definite_height: Option<f64>,
    main_gap: f64,
    cross_gap: f64,
    justify: Alignment,
    align_content: Alignment,
    items: Vec<Item>,
    lines: Vec<Line>,
    main_size: f64,
    phase: Phase,
    queue: Vec<Request>,
    current: Option<usize>,
}

fn number(style: &ComputedStyle, p: P) -> Result<f64, FlowError> {
    match style.get(p) {
        V::Number(n) => Ok(*n),
        _ => Err(FlowError::InvalidSnapshot),
    }
}

fn alignment(style: &ComputedStyle, p: P) -> Result<Alignment, FlowError> {
    match style.get(p) {
        V::Alignment(a) => Ok(*a),
        _ => Err(FlowError::InvalidSnapshot),
    }
}

fn gap(style: &ComputedStyle, p: P, basis: Option<f64>) -> Result<f64, FlowError> {
    Ok(match style.get(p) {
        V::Gap(Gap::Normal) => 0.0,
        V::Gap(Gap::LengthPercentage(v)) => match basis {
            Some(b) => lp(v, b)?.max(0.0),
            None => intrinsic::fixed(v)?.unwrap_or(0.0).max(0.0),
        },
        _ => return Err(FlowError::InvalidSnapshot),
    })
}

fn axes(style: &ComputedStyle) -> Result<(FlexDirection, FlexWrap), FlowError> {
    match (style.get(P::FlexDirection), style.get(P::FlexWrap)) {
        (V::FlexDirection(d), V::FlexWrap(w)) => Ok((*d, *w)),
        _ => Err(FlowError::InvalidSnapshot),
    }
}

fn clamp(n: f64, min: f64, max: f64) -> f64 {
    n.min(max).max(min)
}

/// Offset of the first item and extra space between items for a
/// `justify-content`/`align-content` value (CSS Box Alignment 3 §5.3/§6.1).
/// `start_is_flex_start` says whether `start` names the flex-start side;
/// `left_is_flex_start` does the same for `left` (`None` off the inline axis).
fn distribute(
    a: Alignment,
    free: f64,
    count: usize,
    start_is_flex_start: bool,
    left_is_flex_start: Option<bool>,
) -> (f64, f64) {
    let to_end = |flex_start: bool| if flex_start { 0.0 } else { free };
    if free < 0.0 && a.overflow == Some(OverflowPosition::Safe) {
        return (0.0, 0.0);
    }
    let n = count as f64;
    match a.keyword {
        K::FlexEnd => (free, 0.0),
        K::Center => (free / 2.0, 0.0),
        K::Start | K::SelfStart => (to_end(start_is_flex_start), 0.0),
        K::End | K::SelfEnd => (to_end(!start_is_flex_start), 0.0),
        K::Left => (
            to_end(left_is_flex_start.unwrap_or(start_is_flex_start)),
            0.0,
        ),
        K::Right => (
            to_end(!left_is_flex_start.unwrap_or(!start_is_flex_start)),
            0.0,
        ),
        K::SpaceBetween if count > 1 && free > 0.0 => (0.0, free / (n - 1.0)),
        K::SpaceAround if free > 0.0 => (free / n / 2.0, free / n),
        K::SpaceEvenly if free > 0.0 => (free / (n + 1.0), free / (n + 1.0)),
        // Fallbacks: space-between → flex-start; space-around/evenly → safe
        // center, which is flex-start once the free space is negative.
        K::SpaceAround | K::SpaceEvenly if count > 0 && free >= 0.0 => (free / 2.0, 0.0),
        _ => (0.0, 0.0),
    }
}

/// CSS Flexbox 1 §9.7: resolve the flexible lengths of one line.
fn resolve_line(items: &mut [Item], row: bool, available: f64) {
    let outer = |i: &Item| i.main_outer(row);
    let hypothetical: f64 = items.iter().map(|i| i.hypo + outer(i)).sum();
    let grow = hypothetical < available;
    for item in items.iter_mut() {
        let factor = if grow { item.grow } else { item.shrink };
        item.frozen =
            factor == 0.0 || (grow && item.base > item.hypo) || (!grow && item.base < item.hypo);
        item.target = if item.frozen { item.hypo } else { item.base };
    }
    let initial_free: f64 = available
        - items
            .iter()
            .map(|i| outer(i) + if i.frozen { i.target } else { i.base })
            .sum::<f64>();
    // Each pass freezes at least one item, so this ends after `n` passes.
    for _ in 0..=items.len() {
        if items.iter().all(|i| i.frozen) {
            break;
        }
        let mut remaining = available
            - items
                .iter()
                .map(|i| outer(i) + if i.frozen { i.target } else { i.base })
                .sum::<f64>();
        let unfrozen = || items.iter().filter(|i| !i.frozen);
        let factors: f64 = unfrozen()
            .map(|i| if grow { i.grow } else { i.shrink })
            .sum();
        if factors < 1.0 {
            let scaled = initial_free * factors;
            if scaled.abs() < remaining.abs() {
                remaining = scaled;
            }
        }
        let scaled_shrink: f64 = unfrozen().map(|i| i.shrink * i.base).sum();
        for item in items.iter_mut().filter(|i| !i.frozen) {
            item.target = if remaining == 0.0 {
                item.base
            } else if grow {
                item.base + remaining * item.grow / factors
            } else if scaled_shrink > 0.0 {
                item.base - remaining.abs() * (item.shrink * item.base) / scaled_shrink
            } else {
                item.base
            };
        }
        let mut total = 0.0;
        let mut adjust = Vec::with_capacity(items.len());
        for item in items.iter() {
            if item.frozen {
                adjust.push(0.0);
                continue;
            }
            let clamped = clamp(item.target, item.min, item.max).max(0.0);
            total += clamped - item.target;
            adjust.push(clamped - item.target);
        }
        for (item, delta) in items.iter_mut().zip(adjust) {
            if item.frozen {
                continue;
            }
            let violation = delta != 0.0;
            item.target += delta;
            item.frozen = total == 0.0
                || (total > 0.0 && delta > 0.0)
                || (total < 0.0 && delta < 0.0)
                || (!violation && total == 0.0);
        }
    }
    for item in items.iter_mut() {
        item.target = item.target.max(0.0);
    }
}

impl Engine<'_, '_> {
    /// Intrinsic inline sizes of a flex container (CSS Flexbox 1 §9.9.1,
    /// using items' outer contributions).
    pub(super) fn flex_intrinsic(&mut self, i: usize) -> Result<Intrinsic, FlowError> {
        let style = self.nodes[i].style;
        let (direction, wrap) = axes(style)?;
        let gap = gap(style, P::ColumnGap, None)?;
        let (mut min, mut max, mut count) = (0.0_f64, 0.0_f64, 0usize);
        let (mut min_sum, mut max_sum) = (0.0, 0.0);
        let mut c = i + 1;
        while c < self.nodes[i].end {
            self.work.charge(1)?;
            let child = &self.nodes[c];
            let next = child.end;
            if child.flex_item {
                let outer = intrinsic::outer_contribution(child.style, child.intrinsic)?;
                min = min.max(outer.min);
                max = max.max(outer.max);
                min_sum += outer.min;
                max_sum += outer.max;
                count += 1;
            }
            c = next;
        }
        let gaps = gap * count.saturating_sub(1) as f64;
        Ok(if direction.is_row() {
            Intrinsic {
                min: if wrap == FlexWrap::Nowrap {
                    min_sum + gaps
                } else {
                    min
                },
                max: max_sum + gaps,
            }
        } else {
            Intrinsic { min, max }
        })
    }

    /// Start laying out flex container `c`, whose width and height
    /// constraint are already set. Returns its index in `self.flexes`.
    pub(super) fn start_flex(&mut self, c: usize) -> Result<usize, FlowError> {
        let node = &self.nodes[c];
        let style = node.style;
        let (flex_direction, wrap) = axes(style)?;
        let row = flex_direction.is_row();
        let width = node.measure.content_width.get();
        let definite_height = node.constraint()?.definite().map(CssPx::get);
        let direction = node.direction;
        let end = node.end;
        let (main_gap, cross_gap) = {
            let column = gap(style, P::ColumnGap, Some(width))?;
            let row_gap = match definite_height {
                Some(h) => gap(style, P::RowGap, Some(h))?,
                None => gap(style, P::RowGap, None)?,
            };
            if row {
                (column, row_gap)
            } else {
                (row_gap, column)
            }
        };
        let align_items = alignment(style, P::AlignItems)?;
        let mut items = Vec::new();
        let mut orders = Vec::new();
        let mut i = c + 1;
        while i < end {
            self.work.charge(1)?;
            let child = &self.nodes[i];
            let next = child.end;
            match child.scheme {
                Scheme::Absolute | Scheme::Fixed => {
                    // Static position: the content box's start corner
                    // (an approximation of CSS Flexbox 1 §4.1, TD-48).
                    let x = match direction {
                        Direction::Ltr => 0.0,
                        Direction::Rtl => width,
                    };
                    self.nodes[i].static_pos = (x, 0.0);
                    self.push_absolute(i);
                }
                _ => {
                    let s = child.style;
                    let (padding, border) = box_edges(s, width)?;
                    let side = |p| auto(s, p, width);
                    let (t, r, b, l) = (
                        side(P::MarginTop)?,
                        side(P::MarginRight)?,
                        side(P::MarginBottom)?,
                        side(P::MarginLeft)?,
                    );
                    let px = |m: Option<f64>| CssPx::new(m.unwrap_or(0.0));
                    let mut align = alignment(s, P::AlignSelf)?;
                    if align.keyword == K::Auto {
                        align = align_items;
                    }
                    let order = match s.get(P::Order) {
                        V::Integer(n) => *n,
                        _ => return Err(FlowError::InvalidSnapshot),
                    };
                    orders.push(order);
                    items.push(Item {
                        node: i,
                        padding,
                        border,
                        margin: Edges {
                            top: px(t)?,
                            right: px(r)?,
                            bottom: px(b)?,
                            left: px(l)?,
                        },
                        auto_margin: [t.is_none(), r.is_none(), b.is_none(), l.is_none()],
                        grow: number(s, P::FlexGrow)?,
                        shrink: number(s, P::FlexShrink)?,
                        align,
                        base: 0.0,
                        hypo: 0.0,
                        min: 0.0,
                        max: f64::INFINITY,
                        target: 0.0,
                        frozen: false,
                        width: 0.0,
                        natural: 0.0,
                        laid: None,
                        cross: 0.0,
                        main_pos: 0.0,
                        cross_pos: 0.0,
                    });
                }
            }
            i = next;
        }
        // `order`-modified document order (a stable sort).
        let mut keyed: Vec<(i32, Item)> = orders.into_iter().zip(items).collect();
        keyed.sort_by_key(|(order, _)| *order);
        let items = keyed.into_iter().map(|(_, item)| item).collect();
        let run = FlexRun {
            container: c,
            row,
            reverse: flex_direction.is_reverse(),
            wrap: wrap != FlexWrap::Nowrap,
            wrap_reverse: wrap == FlexWrap::WrapReverse,
            direction,
            width,
            definite_height,
            main_gap,
            cross_gap,
            justify: alignment(style, P::JustifyContent)?,
            align_content: alignment(style, P::AlignContent)?,
            items,
            lines: Vec::new(),
            main_size: 0.0,
            phase: if row {
                Phase::RowCross
            } else {
                Phase::ColumnBase
            },
            queue: Vec::new(),
            current: None,
        };
        self.flexes.push(run);
        let k = self.flexes.len() - 1;
        if row {
            self.row_main(k)?;
        } else {
            self.column_cross(k)?;
        }
        Ok(k)
    }

    /// Advance flex container `k`: push the next item frame and return
    /// `None`, or finish and return the container's natural content height.
    pub(super) fn flex_step(
        &mut self,
        frames: &mut Vec<Frame>,
        bfcs: &mut Vec<Bfc>,
        k: usize,
    ) -> Result<Option<f64>, FlowError> {
        loop {
            self.work.charge(1)?;
            if let Some(req) = self.flexes[k].queue.pop() {
                let item = self.flexes[k].items[req.item];
                let n = &mut self.nodes[item.node];
                n.measure = BlockMeasure {
                    content_width: CssPx::new(req.width.max(0.0))?,
                    margin: item.margin,
                    padding: item.padding,
                    border: item.border,
                };
                let height = req.height.map(|h| CssPx::new(h.max(0.0))).transpose()?;
                n.height = Some(HeightConstraint::solved(height));
                n.top_resolved = true;
                self.flexes[k].items[req.item].width = req.width;
                self.flexes[k].items[req.item].laid = req.height;
                if let Some(natural) = n.natural {
                    // A replaced item has no content to lay out.
                    let style = n.style;
                    let run = &self.flexes[k];
                    let (_, h) =
                        replaced::used_size(style, natural, Some(run.width), run.definite_height)?;
                    let content = match (req.height, natural.ratio()) {
                        (Some(h), _) => h,
                        (None, Some(r)) if Sizing::read(style, P::Height)?.length().is_none() => {
                            req.width / r
                        }
                        _ => h,
                    };
                    self.flexes[k].items[req.item].natural = content;
                    continue;
                }
                self.flexes[k].current = Some(req.item);
                frames.push(self.root_frame(item.node)?);
                bfcs.push(Bfc::default());
                return Ok(None);
            }
            match self.flexes[k].phase {
                Phase::RowCross => self.row_cross(k)?,
                Phase::ColumnBase => self.column_main(k)?,
                Phase::RowStretch | Phase::ColumnMain => {
                    let natural = self.finish_flex(k)?;
                    if self.flexes.len() != k + 1 {
                        return Err(FlowError::InvalidSnapshot);
                    }
                    self.flexes.pop();
                    return Ok(Some(natural));
                }
            }
        }
    }

    /// Record the natural content height of the item just laid out.
    pub(super) fn flex_item_done(
        &mut self,
        k: usize,
        node: usize,
        natural: f64,
    ) -> Result<(), FlowError> {
        let run = &mut self.flexes[k];
        let item = run.current.take().ok_or(FlowError::InvalidSnapshot)?;
        if run.items[item].node != node {
            return Err(FlowError::InvalidSnapshot);
        }
        run.items[item].natural = natural;
        Ok(())
    }

    /// A box's block-axis constraint against the container's content height.
    fn item_heights(&self, k: usize, item: &Item) -> Result<HeightConstraint, FlowError> {
        let run = &self.flexes[k];
        let basis = run.definite_height.map(CssPx::new).transpose()?;
        Ok(HeightConstraint::new(
            self.nodes[item.node].style,
            basis,
            CssPx::new(item.vertical_extras())?,
        )?)
    }

    /// Break items into lines by their outer hypothetical main sizes.
    fn collect_lines(&mut self, k: usize, available: f64) -> Result<(), FlowError> {
        let run = &mut self.flexes[k];
        let row = run.row;
        let mut lines = Vec::new();
        let (mut start, mut used) = (0usize, 0.0_f64);
        for (i, item) in run.items.iter().enumerate() {
            self.work.charge(1)?;
            let outer = item.hypo + item.main_outer(row);
            let gap = if i > start { run.main_gap } else { 0.0 };
            if run.wrap && i > start && used + gap + outer > available {
                lines.push(Line {
                    start,
                    end: i,
                    cross: 0.0,
                    pos: 0.0,
                });
                start = i;
                used = outer;
            } else {
                used += gap + outer;
            }
        }
        if start < run.items.len() || lines.is_empty() {
            lines.push(Line {
                start,
                end: run.items.len(),
                cross: 0.0,
                pos: 0.0,
            });
        }
        for line in &lines {
            let gaps = run.main_gap * (line.end - line.start).saturating_sub(1) as f64;
            resolve_line(&mut run.items[line.start..line.end], row, available - gaps);
        }
        run.lines = lines;
        run.main_size = available;
        Ok(())
    }

    /// Row: flex base sizes from intrinsic widths, lines, flexible lengths;
    /// then request each item at its used width.
    fn row_main(&mut self, k: usize) -> Result<(), FlowError> {
        let width = self.flexes[k].width;
        let count = self.flexes[k].items.len();
        for idx in 0..count {
            self.work.charge(1)?;
            let item = self.flexes[k].items[idx];
            let n = &self.nodes[item.node];
            let style = n.style;
            let extras = item.horizontal_extras();
            let available = width - item.horizontal_margins() - extras;
            let r = Resolver::new(style, Some(width), extras, n.intrinsic);
            let (specified, minimum, maximum) = r.widths(available)?;
            let content = n.intrinsic.max;
            let base = match style.get(P::FlexBasis) {
                V::FlexBasis(FlexBasis::Content) => content,
                V::FlexBasis(FlexBasis::Size(Size::Auto)) => specified.unwrap_or(content),
                V::FlexBasis(FlexBasis::Size(size)) => r
                    .resolve(Sizing::of_size(size), available)?
                    .unwrap_or(content),
                _ => return Err(FlowError::InvalidSnapshot),
            };
            let max = maximum.unwrap_or(f64::INFINITY);
            let min = if matches!(Sizing::read(style, P::MinWidth)?, Sizing::Initial)
                && !scroll_container(n.overflow)
            {
                // Automatic minimum size (CSS Flexbox 1 §4.5).
                let suggestion = n.intrinsic.min;
                specified.map_or(suggestion, |s| s.min(suggestion)).min(max)
            } else {
                minimum
            };
            let entry = &mut self.flexes[k].items[idx];
            entry.base = base;
            entry.min = min;
            entry.max = max.max(min);
            entry.hypo = clamp(base, min, entry.max);
        }
        self.collect_lines(k, width)?;
        let mut queue = Vec::with_capacity(count);
        for idx in (0..count).rev() {
            let item = self.flexes[k].items[idx];
            let height = self.item_heights(k, &item)?.definite().map(CssPx::get);
            queue.push(Request {
                item: idx,
                width: item.target,
                height,
            });
        }
        let run = &mut self.flexes[k];
        run.queue = queue;
        run.phase = Phase::RowCross;
        Ok(())
    }

    /// Row: cross sizes, line sizes, `align-content` and stretching.
    fn row_cross(&mut self, k: usize) -> Result<(), FlowError> {
        let count = self.flexes[k].items.len();
        let definite = self.flexes[k].definite_height;
        for idx in 0..count {
            let item = self.flexes[k].items[idx];
            let cross = self
                .item_heights(k, &item)?
                .used(CssPx::new(item.natural.max(0.0))?);
            self.flexes[k].items[idx].cross = cross.get();
        }
        self.size_lines(k)?;
        let mut queue = Vec::new();
        for idx in (0..count).rev() {
            let item = self.flexes[k].items[idx];
            let run = &self.flexes[k];
            let line = run.lines.iter().find(|l| (l.start..l.end).contains(&idx));
            let Some(line) = line else {
                return Err(FlowError::InvalidSnapshot);
            };
            let style = self.nodes[item.node].style;
            let stretch = matches!(item.align.keyword, K::Stretch | K::Normal)
                && !item.cross_auto(true)
                && self.item_heights(k, &item)?.specified().is_none()
                && Sizing::read(style, P::Height)?
                    .length()
                    .is_none_or(|v| intrinsic::has_percentage(v) && run.definite_height.is_none());
            if !stretch {
                continue;
            }
            let stretched =
                (line.cross - item.vertical_margins() - item.vertical_extras()).max(0.0);
            let stretched = self
                .item_heights(k, &item)?
                .used(CssPx::new(stretched)?)
                .get();
            self.flexes[k].items[idx].cross = stretched;
            // Lay out again only if something can differ: a new height, or a
            // definite one children may resolve percentages against (TD-48).
            if item.laid == Some(stretched)
                || (item.laid.is_none() && stretched == item.cross && definite.is_none())
            {
                continue;
            }
            queue.push(Request {
                item: idx,
                width: item.width,
                height: Some(stretched),
            });
        }
        let run = &mut self.flexes[k];
        run.queue = queue;
        run.phase = Phase::RowStretch;
        Ok(())
    }

    /// Line cross sizes (with baseline alignment in rows) and positions.
    fn size_lines(&mut self, k: usize) -> Result<(), FlowError> {
        let container = self.flexes[k].container;
        let constraint = self.nodes[container].constraint()?;
        let run = &mut self.flexes[k];
        let row = run.row;
        for line in run.lines.iter_mut() {
            let (mut size, mut above, mut below) = (0.0_f64, 0.0_f64, 0.0_f64);
            for item in &run.items[line.start..line.end] {
                let outer = item.cross + item.cross_outer(row);
                if row && matches!(item.align.keyword, K::Baseline) && !item.cross_auto(row) {
                    // Synthesized from the border-box bottom (no text yet).
                    let baseline = item.margin.top.get() + item.cross + item.vertical_extras();
                    above = above.max(baseline);
                    below = below.max(outer - baseline);
                } else {
                    size = size.max(outer);
                }
            }
            line.cross = size.max(above + below);
        }
        let lines = run.lines.len();
        let gaps = run.cross_gap * lines.saturating_sub(1) as f64;
        let total: f64 = run.lines.iter().map(|l| l.cross).sum::<f64>() + gaps;
        // The container's used inner cross size.
        let cross_size = if row {
            constraint.used(CssPx::new(total.max(0.0))?).get()
        } else {
            run.width
        };
        if !run.wrap {
            if let Some(line) = run.lines.first_mut() {
                line.cross = cross_size;
            }
            return Ok(());
        }
        let free = cross_size - total;
        let mut a = run.align_content;
        if matches!(a.keyword, K::Normal | K::Stretch) {
            if free > 0.0 {
                let extra = free / lines as f64;
                for line in run.lines.iter_mut() {
                    line.cross += extra;
                }
            }
            a.keyword = K::FlexStart;
        }
        let free = cross_size - run.lines.iter().map(|l| l.cross).sum::<f64>() - gaps;
        let (offset, between) = distribute(a, free, lines, !run.wrap_reverse, None);
        let mut pos = offset;
        for line in run.lines.iter_mut() {
            line.pos = pos;
            pos += line.cross + run.cross_gap + between;
        }
        Ok(())
    }

    /// Column: cross widths, then request each item at that width.
    fn column_cross(&mut self, k: usize) -> Result<(), FlowError> {
        let width = self.flexes[k].width;
        let count = self.flexes[k].items.len();
        let mut queue = Vec::with_capacity(count);
        for idx in (0..count).rev() {
            self.work.charge(1)?;
            let item = self.flexes[k].items[idx];
            let n = &self.nodes[item.node];
            let extras = item.horizontal_extras();
            let available = width - item.horizontal_margins() - extras;
            let r = Resolver::new(n.style, Some(width), extras, n.intrinsic);
            let (specified, minimum, maximum) = r.widths(available)?;
            let stretch =
                matches!(item.align.keyword, K::Stretch | K::Normal) && !item.cross_auto(false);
            let tentative = specified.unwrap_or(if let Some(natural) = n.natural {
                replaced::used_size(n.style, natural, Some(width), None)?.0
            } else if stretch {
                available.max(0.0)
            } else {
                n.intrinsic.fit(available)
            });
            let w = maximum
                .map_or(tentative, |m| tentative.min(m))
                .max(minimum)
                .max(0.0);
            let height = self.item_heights(k, &item)?.definite().map(CssPx::get);
            queue.push(Request {
                item: idx,
                width: w,
                height,
            });
        }
        let run = &mut self.flexes[k];
        run.queue = queue;
        run.phase = Phase::ColumnBase;
        Ok(())
    }

    /// Column: flex base sizes from content heights, flexible lengths, then
    /// request each item at its used height.
    fn column_main(&mut self, k: usize) -> Result<(), FlowError> {
        let count = self.flexes[k].items.len();
        let definite = self.flexes[k].definite_height;
        for idx in 0..count {
            self.work.charge(1)?;
            let item = self.flexes[k].items[idx];
            let n = &self.nodes[item.node];
            let style = n.style;
            let heights = self.item_heights(k, &item)?;
            let specified = heights.specified().map(CssPx::get);
            let content = item.natural.max(0.0);
            let extras = item.vertical_extras();
            let border_box = matches!(
                style.get(P::BoxSizing),
                V::BoxSizing(rustic_css::BoxSizing::BorderBox)
            );
            let basis_length =
                |v: &rustic_css::LengthPercentage| -> Result<Option<f64>, FlowError> {
                    let n = match definite {
                        Some(h) => lp(v, h)?,
                        None if intrinsic::has_percentage(v) => return Ok(None),
                        None => lp(v, 0.0)?,
                    };
                    Ok(Some(if border_box {
                        (n - extras).max(0.0)
                    } else {
                        n.max(0.0)
                    }))
                };
            let base = match style.get(P::FlexBasis) {
                V::FlexBasis(FlexBasis::Size(Size::Auto)) => specified.unwrap_or(content),
                V::FlexBasis(FlexBasis::Size(Size::LengthPercentage(v))) => {
                    basis_length(v)?.unwrap_or(content)
                }
                V::FlexBasis(_) => content,
                _ => return Err(FlowError::InvalidSnapshot),
            };
            let max = heights.maximum().map_or(f64::INFINITY, CssPx::get);
            let min = if matches!(Sizing::read(style, P::MinHeight)?, Sizing::Initial)
                && !scroll_container(n.overflow)
            {
                specified.map_or(content, |s| s.min(content)).min(max)
            } else {
                heights.minimum().get()
            };
            let entry = &mut self.flexes[k].items[idx];
            entry.base = base;
            entry.min = min;
            entry.max = max.max(min);
            entry.hypo = clamp(base, min, entry.max);
            entry.cross = entry.width;
        }
        let available = match definite {
            Some(h) => h,
            None => {
                let run = &self.flexes[k];
                let gaps = run.main_gap * count.saturating_sub(1) as f64;
                let sum: f64 = run
                    .items
                    .iter()
                    .map(|i| i.hypo + i.main_outer(false))
                    .sum::<f64>()
                    + gaps;
                let container = run.container;
                self.nodes[container]
                    .constraint()?
                    .used(CssPx::new(sum.max(0.0))?)
                    .get()
            }
        };
        self.collect_lines(k, available)?;
        let mut queue = Vec::with_capacity(count);
        for idx in (0..count).rev() {
            let item = self.flexes[k].items[idx];
            // An item in an auto-height column is not definite after flexing
            // (CSS Flexbox 1 §9.8), so its content-height layout stands when
            // the size did not change.
            if item.laid == Some(item.target)
                || (item.laid.is_none() && definite.is_none() && item.target == item.natural)
            {
                continue;
            }
            queue.push(Request {
                item: idx,
                width: item.width,
                height: Some(item.target),
            });
        }
        let run = &mut self.flexes[k];
        run.queue = queue;
        run.phase = Phase::ColumnMain;
        Ok(())
    }

    /// Main/cross alignment and final item geometry; returns the
    /// container's natural content height.
    fn finish_flex(&mut self, k: usize) -> Result<f64, FlowError> {
        if !self.flexes[k].row {
            self.size_lines(k)?;
        }
        let container = self.flexes[k].container;
        let constraint = self.nodes[container].constraint()?;
        let run = &mut self.flexes[k];
        let row = run.row;
        let lines_total = run.lines.iter().map(|l| l.cross).sum::<f64>()
            + run.cross_gap * run.lines.len().saturating_sub(1) as f64;
        if !run.wrap {
            if let Some(line) = run.lines.first_mut() {
                line.pos = 0.0;
            }
        }
        let cross_size = if row {
            constraint.used(CssPx::new(lines_total.max(0.0))?).get()
        } else {
            run.width
        };
        let main_size = run.main_size;
        let rtl = run.direction == Direction::Rtl;
        let main_reverse = if row { rtl != run.reverse } else { run.reverse };
        let cross_reverse = if row {
            run.wrap_reverse
        } else {
            rtl != run.wrap_reverse
        };
        let lines = run.lines.clone();
        for line in &lines {
            self.work.charge(line.end - line.start)?;
            let items = &mut run.items[line.start..line.end];
            let count = items.len();
            // Main axis: auto margins first, then justify-content.
            let used: f64 = items
                .iter()
                .map(|i| i.target + i.main_outer(row))
                .sum::<f64>()
                + run.main_gap * count.saturating_sub(1) as f64;
            let mut free = main_size - used;
            let autos: usize = items
                .iter()
                .map(|i| {
                    let (a, b) = if row { (1, 3) } else { (0, 2) };
                    usize::from(i.auto_margin[a]) + usize::from(i.auto_margin[b])
                })
                .sum();
            if free > 0.0 && autos > 0 {
                let each = free / autos as f64;
                for item in items.iter_mut() {
                    let px = |n: f64| CssPx::new(n);
                    if row {
                        if item.auto_margin[1] {
                            item.margin.right = px(each)?;
                        }
                        if item.auto_margin[3] {
                            item.margin.left = px(each)?;
                        }
                    } else {
                        if item.auto_margin[0] {
                            item.margin.top = px(each)?;
                        }
                        if item.auto_margin[2] {
                            item.margin.bottom = px(each)?;
                        }
                    }
                }
                free = 0.0;
            }
            let left = row.then_some(!main_reverse);
            let (offset, between) = distribute(run.justify, free, count, !run.reverse, left);
            let mut pos = offset;
            for item in items.iter_mut() {
                item.main_pos = pos;
                pos += item.target + item.main_outer(row) + run.main_gap + between;
            }
            // Cross axis: auto margins, then align-self.
            let (mut above, mut below_last) = (0.0_f64, 0.0_f64);
            for item in items.iter() {
                if row && item.align.keyword == K::Baseline && !item.cross_auto(row) {
                    above = above.max(item.margin.top.get() + item.cross + item.vertical_extras());
                }
                if row && item.align.keyword == K::LastBaseline && !item.cross_auto(row) {
                    below_last = below_last.max(item.margin.bottom.get());
                }
            }
            for item in items.iter_mut() {
                let outer = item.cross + item.cross_outer(row);
                let free = line.cross - outer;
                let offset = if item.cross_auto(row) {
                    let (start, end) = if row { (0, 2) } else { (3, 1) };
                    let share = free.max(0.0);
                    let (s, e) = match (item.auto_margin[start], item.auto_margin[end]) {
                        (true, true) => (share / 2.0, share / 2.0),
                        (true, false) => (share, 0.0),
                        _ => (0.0, share),
                    };
                    let px = |n: f64| CssPx::new(n);
                    if row {
                        item.margin.top = px(item.margin.top.get() + s)?;
                        item.margin.bottom = px(item.margin.bottom.get() + e)?;
                    } else {
                        // In flow-relative cross terms, left is cross-start
                        // unless the cross axis is reversed.
                        let (l, r) = if cross_reverse { (e, s) } else { (s, e) };
                        item.margin.left = px(item.margin.left.get() + l)?;
                        item.margin.right = px(item.margin.right.get() + r)?;
                    }
                    0.0
                } else if row && item.align.keyword == K::Baseline {
                    above - (item.margin.top.get() + item.cross + item.vertical_extras())
                } else if row && item.align.keyword == K::LastBaseline {
                    free - (below_last - item.margin.bottom.get())
                } else {
                    let mut a = item.align;
                    if matches!(
                        a.keyword,
                        K::Stretch | K::Normal | K::Baseline | K::LastBaseline
                    ) {
                        a.keyword = K::FlexStart;
                    }
                    if a.keyword == K::SelfStart || a.keyword == K::SelfEnd {
                        // In a column the cross axis is inline: the item's
                        // own direction decides; in a row it is the block axis.
                        let own = self.nodes[item.node].direction;
                        let same = row || own == run.direction;
                        a.keyword = match (a.keyword == K::SelfStart, same) {
                            (true, true) | (false, false) => K::Start,
                            _ => K::End,
                        };
                    }
                    distribute(a, free, 1, !run.wrap_reverse, None).0
                };
                item.cross_pos = line.pos + offset;
            }
        }
        // Physical geometry.
        let width = run.width;
        let definite_height = run.definite_height;
        let direction = run.direction;
        let items = run.items.clone();
        for item in &items {
            self.work.charge(1)?;
            let outer_main = item.target + item.main_outer(row);
            let outer_cross = item.cross + item.cross_outer(row);
            let main = if main_reverse {
                main_size - item.main_pos - outer_main
            } else {
                item.main_pos
            };
            let cross = if cross_reverse {
                cross_size - item.cross_pos - outer_cross
            } else {
                item.cross_pos
            };
            let (x, y, w, h) = if row {
                (main, cross, item.target, item.cross)
            } else {
                (cross, main, item.cross, item.target)
            };
            let n = &mut self.nodes[item.node];
            n.measure = BlockMeasure {
                content_width: CssPx::new(w.max(0.0))?,
                margin: item.margin,
                padding: item.padding,
                border: item.border,
            };
            n.content_height = CssPx::new(h.max(0.0))?;
            n.border_height = n.content_height.checked_add(n.vertical_extras()?)?;
            n.pos = (x + item.margin.left.get(), y + item.margin.top.get());
            n.top_resolved = true;
            if n.relative {
                n.rel = position::relative_offset(
                    n.style,
                    CssPx::new(width)?,
                    definite_height.map(CssPx::new).transpose()?,
                    direction,
                )?;
            }
        }
        Ok(if row { lines_total } else { main_size })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(base: f64, grow: f64, shrink: f64, min: f64, max: f64) -> Item {
        Item {
            node: 0,
            padding: Edges::default(),
            border: Edges::default(),
            margin: Edges::default(),
            auto_margin: [false; 4],
            grow,
            shrink,
            align: Alignment::new(K::Normal),
            base,
            hypo: clamp(base, min, max),
            min,
            max,
            target: 0.0,
            frozen: false,
            width: 0.0,
            natural: 0.0,
            laid: None,
            cross: 0.0,
            main_pos: 0.0,
            cross_pos: 0.0,
        }
    }

    fn targets(items: &mut [Item], available: f64) -> Vec<f64> {
        resolve_line(items, true, available);
        items.iter().map(|i| i.target).collect()
    }

    #[test]
    fn flexible_lengths_freeze_min_and_max_violations() {
        let inf = f64::INFINITY;
        // Grow 1:2 into 90px of free space.
        let mut items = [
            item(10.0, 1.0, 1.0, 0.0, inf),
            item(0.0, 2.0, 1.0, 0.0, inf),
        ];
        assert_eq!(targets(&mut items, 100.0), [40.0, 60.0]);
        // A max violation freezes and the rest goes to the other item.
        let mut items = [
            item(0.0, 1.0, 1.0, 0.0, 20.0),
            item(0.0, 1.0, 1.0, 0.0, inf),
        ];
        assert_eq!(targets(&mut items, 100.0), [20.0, 80.0]);
        // Shrinking is weighted by shrink × base size.
        let mut items = [
            item(100.0, 0.0, 1.0, 0.0, inf),
            item(50.0, 0.0, 2.0, 0.0, inf),
        ];
        assert_eq!(targets(&mut items, 100.0), [75.0, 25.0]);
        // A min violation while shrinking.
        let mut items = [
            item(100.0, 0.0, 1.0, 90.0, inf),
            item(100.0, 0.0, 1.0, 0.0, inf),
        ];
        assert_eq!(targets(&mut items, 100.0), [90.0, 10.0]);
        // Factors summing below one take only that share of the free space.
        let mut items = [
            item(0.0, 0.25, 1.0, 0.0, inf),
            item(0.0, 0.25, 1.0, 0.0, inf),
        ];
        assert_eq!(targets(&mut items, 100.0), [25.0, 25.0]);
        // Inflexible items keep their hypothetical size.
        let mut items = [
            item(30.0, 0.0, 0.0, 0.0, inf),
            item(30.0, 0.0, 0.0, 0.0, inf),
        ];
        assert_eq!(targets(&mut items, 10.0), [30.0, 30.0]);
    }

    #[test]
    fn content_distribution_falls_back_when_space_is_short() {
        let a = |keyword| Alignment::new(keyword);
        assert_eq!(
            distribute(a(K::SpaceBetween), 30.0, 4, true, None),
            (0.0, 10.0)
        );
        assert_eq!(
            distribute(a(K::SpaceBetween), 30.0, 1, true, None),
            (0.0, 0.0)
        );
        assert_eq!(
            distribute(a(K::SpaceAround), 40.0, 2, true, None),
            (10.0, 20.0)
        );
        assert_eq!(
            distribute(a(K::SpaceEvenly), 30.0, 2, true, None),
            (10.0, 10.0)
        );
        assert_eq!(
            distribute(a(K::SpaceAround), -10.0, 2, true, None),
            (0.0, 0.0)
        );
        assert_eq!(distribute(a(K::Center), -10.0, 1, true, None), (-5.0, 0.0));
        let safe = Alignment {
            keyword: K::Center,
            overflow: Some(OverflowPosition::Safe),
        };
        assert_eq!(distribute(safe, -10.0, 1, true, None), (0.0, 0.0));
        assert_eq!(distribute(a(K::End), 8.0, 1, false, None), (0.0, 0.0));
        assert_eq!(
            distribute(a(K::Right), 8.0, 1, true, Some(true)),
            (8.0, 0.0)
        );
        assert_eq!(
            distribute(a(K::Right), 8.0, 1, true, Some(false)),
            (0.0, 0.0)
        );
    }
}
