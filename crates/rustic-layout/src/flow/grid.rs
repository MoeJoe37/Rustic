//! Grid layout (CSS Grid Layout 2 §6–§11, Editor's Draft reviewed
//! 2026-11-26) as a formatting context inside the block walk.
//!
//! A grid container's frame drives a small state machine, like flex
//! layout. Items are placed (§8.5) and columns are sized from the items'
//! intrinsic inline contributions; every item is then laid out at its grid
//! area width to find its height, rows are sized from those heights, and
//! items whose block size depends on the row (stretched, or with a definite
//! size against their grid area) are laid out again. There is no recursion:
//! nested grids are more frames.
//!
//! Track sizing follows §11.3–§11.8: initialization, intrinsic sizes (single
//! spans, then growing span groups through "distribute extra space", then
//! items crossing flexible tracks), maximization, flexible lengths and
//! stretched `auto` tracks. Limits (subgrid, masonry, baseline alignment,
//! grid-placed absolutely positioned children, vertical writing modes) are
//! TD-209; the WPT comparison's reviewed failures are TD-210.

use rustic_css::{
    AlignKeyword as K, Alignment, Direction, Gap, GridLine, GridTemplate, GridValue,
    OverflowPosition, PropertyId as P, PropertyValue as V, RepeatCount, TrackBreadth,
    TrackComponent, TrackSize,
};
use rustic_style::ComputedStyle;

use super::{Bfc, Engine, FlowError, Frame, Scheme, Work};
use crate::height::HeightConstraint;
use crate::intrinsic::{self, Intrinsic, Resolver, Sizing};
use crate::sizing::{auto, box_edges, lp};
use crate::{BlockMeasure, CssPx, Edges, position, replaced};

/// Tracks per axis, explicit and implicit. Lines past it clamp (CSS Grid 2
/// §8.4 lets implementations clamp the grid).
pub(crate) const MAX_TRACKS: i64 = 10_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// Items being laid out at their grid area widths.
    Measure,
    /// Items being laid out again at their final heights.
    Final,
}

#[derive(Clone, Copy, Debug)]
struct Request {
    item: usize,
    width: f64,
    height: Option<f64>,
}

/// A resolved track sizing function.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Breadth {
    Fixed(f64),
    MinContent,
    MaxContent,
    Auto,
    Flex(f64),
}

impl Breadth {
    fn intrinsic(self) -> bool {
        matches!(self, Self::MinContent | Self::MaxContent | Self::Auto)
    }
}

#[derive(Clone, Copy, Debug)]
struct Track {
    min: Breadth,
    max: Breadth,
    /// The `fit-content()` limit, if any.
    fit: Option<f64>,
    base: f64,
    limit: f64,
    /// An empty `auto-fit` repetition (CSS Grid 2 §7.2.3.2).
    collapsed: bool,
}

impl Track {
    fn flexible(&self) -> bool {
        matches!(self.max, Breadth::Flex(_))
    }
    fn factor(&self) -> f64 {
        match self.max {
            Breadth::Flex(f) => f,
            _ => 0.0,
        }
    }
    fn intrinsic_max(&self) -> bool {
        self.max.intrinsic() || self.fit.is_some()
    }
}

/// The sizing constraint of one axis (CSS Grid 2 §11.1).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Available {
    Definite(f64),
    MinContent,
    MaxContent,
}

/// An item's contributions in one axis (outer sizes).
#[derive(Clone, Copy, Debug, Default)]
struct Contribution {
    start: usize,
    end: usize,
    minimum: f64,
    min_content: f64,
    max_content: f64,
}

/// A placement in one axis: lines `start..end`, or auto with a span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Span {
    Definite(i64, i64),
    Auto(i64),
}

#[derive(Clone, Copy, Debug)]
struct Item {
    node: usize,
    /// Column and row track ranges, after placement.
    columns: (usize, usize),
    rows: (usize, usize),
    justify: Alignment,
    align: Alignment,
    padding: Edges,
    border: Edges,
    margin: Edges,
    auto_margin: [bool; 4],
    /// Content width of the latest layout request.
    width: f64,
    /// Content height reported by the latest layout.
    natural: f64,
    /// Definite height of the latest layout request, if any.
    laid: Option<f64>,
    height: f64,
    x: f64,
    y: f64,
    /// The grid area's size, the item's containing block.
    area: (f64, f64),
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
}

/// One grid container's layout state.
pub(super) struct GridRun {
    container: usize,
    direction: Direction,
    width: f64,
    definite_height: Option<f64>,
    column_gap: f64,
    row_gap: f64,
    columns: Vec<Track>,
    rows: Vec<Track>,
    items: Vec<Item>,
    justify_content: Alignment,
    align_content: Alignment,
    /// Track start positions (from the inline/block start) plus the end
    /// of the last track, after content alignment.
    column_pos: Vec<f64>,
    row_pos: Vec<f64>,
    phase: Phase,
    queue: Vec<Request>,
    current: Option<usize>,
}

fn grid_value(style: &ComputedStyle, p: P) -> Result<&GridValue, FlowError> {
    match style.get(p) {
        V::Grid(v) => Ok(v),
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

/// A track breadth against a percentage basis (`None`: indefinite, so
/// percentages behave as `auto`).
fn breadth(b: &TrackBreadth, basis: Option<f64>) -> Result<Breadth, FlowError> {
    Ok(match b {
        TrackBreadth::Length(v) => match basis {
            Some(basis) => Breadth::Fixed(lp(v, basis)?.max(0.0)),
            None => match intrinsic::fixed(v)? {
                Some(n) => Breadth::Fixed(n.max(0.0)),
                None => Breadth::Auto,
            },
        },
        TrackBreadth::Flex(f) => Breadth::Flex(*f),
        TrackBreadth::MinContent => Breadth::MinContent,
        TrackBreadth::MaxContent => Breadth::MaxContent,
        TrackBreadth::Auto => Breadth::Auto,
    })
}

fn track(size: &TrackSize, basis: Option<f64>) -> Result<Track, FlowError> {
    let (min, max) = size.functions();
    let mut min = breadth(&min, basis)?;
    if matches!(min, Breadth::Flex(_)) {
        min = Breadth::Auto;
    }
    let max = breadth(&max, basis)?;
    let fit = match size {
        TrackSize::FitContent(v) => match basis {
            Some(b) => Some(lp(v, b)?.max(0.0)),
            None => intrinsic::fixed(v)?.map(|n| n.max(0.0)),
        },
        _ => None,
    };
    Ok(Track {
        min,
        max,
        fit,
        base: 0.0,
        limit: 0.0,
        collapsed: false,
    })
}

/// One axis's explicit grid: track sizes and the names of each line.
#[derive(Default)]
struct Explicit {
    sizes: Vec<TrackSize>,
    names: Vec<Vec<String>>,
    /// Track range of the `auto-fit` repetition, if any.
    auto_fit: Option<(usize, usize)>,
}

impl Explicit {
    fn add_names(&mut self, names: &[String]) {
        if let Some(last) = self.names.last_mut() {
            last.extend(names.iter().cloned());
        }
    }
    fn add_track(&mut self, size: &TrackSize) {
        self.sizes.push(size.clone());
        self.names.push(Vec::new());
    }
}

/// The breadth that decides how many auto repetitions fit (§7.2.3.2): the
/// max function if fixed, else the min function if fixed.
fn repeat_breadth(size: &TrackSize, basis: f64) -> Result<f64, FlowError> {
    let (min, max) = size.functions();
    let fixed = |b: &TrackBreadth| -> Result<Option<f64>, FlowError> {
        Ok(match b {
            TrackBreadth::Length(v) => Some(lp(v, basis)?.max(0.0)),
            _ => None,
        })
    };
    let max = fixed(&max)?;
    let min = fixed(&min)?;
    Ok(match (min, max) {
        (Some(min), Some(max)) => max.max(min),
        (_, Some(max)) => max,
        (Some(min), None) => min,
        (None, None) => 0.0,
    })
}

/// The space an auto repetition is counted against (§7.2.3.2): a definite
/// size or maximum the repetitions must fit in, else a minimum they fill.
#[derive(Clone, Copy, Debug, Default)]
struct RepeatSpace {
    fit: Option<f64>,
    fill: Option<f64>,
}

fn expand(
    template: &GridTemplate,
    repeat: RepeatSpace,
    gap: f64,
    work: &mut Work<'_>,
) -> Result<Explicit, FlowError> {
    let available = repeat.fit.or(repeat.fill);
    let mut out = Explicit {
        names: vec![Vec::new()],
        ..Explicit::default()
    };
    let GridTemplate::Tracks(list) = template else {
        return Ok(out);
    };
    // Room the non-repeated tracks leave for an auto repetition.
    let mut fixed_total = 0.0;
    let mut fixed_count = 0usize;
    if let Some(basis) = available {
        for c in list {
            match c {
                TrackComponent::Size(s) => {
                    fixed_total += repeat_breadth(s, basis)?;
                    fixed_count += 1;
                }
                TrackComponent::Repeat(RepeatCount::Count(n), inner) => {
                    for c in inner {
                        if let TrackComponent::Size(s) = c {
                            fixed_total += repeat_breadth(s, basis)? * f64::from(*n);
                            fixed_count = fixed_count.saturating_add(*n as usize);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    for c in list {
        work.charge(1)?;
        match c {
            TrackComponent::Names(names) => out.add_names(names),
            TrackComponent::Size(s) => out.add_track(s),
            TrackComponent::Repeat(count, inner) => {
                let sizes: Vec<&TrackSize> = inner
                    .iter()
                    .filter_map(|c| match c {
                        TrackComponent::Size(s) => Some(s),
                        _ => None,
                    })
                    .collect();
                let times = match count {
                    RepeatCount::Count(n) => i64::from(*n),
                    RepeatCount::AutoFill | RepeatCount::AutoFit => match available {
                        Some(space) => {
                            let mut one = 0.0;
                            for s in &sizes {
                                one += repeat_breadth(s, space)?;
                            }
                            let per = sizes.len() as f64;
                            let one_with_gaps = one + gap * per;
                            let free = space - fixed_total - gap * fixed_count as f64;
                            if one_with_gaps <= 0.0 {
                                1
                            } else {
                                // n repetitions need n·one + (n·per + fixed − 1)
                                // gaps: the most that fit, or the fewest that
                                // fill a minimum.
                                let ratio = (free + gap) / one_with_gaps;
                                let n = if repeat.fit.is_some() {
                                    ratio.floor()
                                } else {
                                    ratio.ceil()
                                };
                                if n.is_finite() {
                                    (n as i64).clamp(1, MAX_TRACKS)
                                } else {
                                    1
                                }
                            }
                        }
                        None => 1,
                    },
                };
                let first = out.sizes.len();
                for _ in 0..times {
                    if out.sizes.len() as i64 >= MAX_TRACKS {
                        break;
                    }
                    work.charge(inner.len())?;
                    for c in inner {
                        match c {
                            TrackComponent::Names(names) => out.add_names(names),
                            TrackComponent::Size(s) => out.add_track(s),
                            TrackComponent::Repeat(..) => return Err(FlowError::InvalidSnapshot),
                        }
                    }
                }
                if *count == RepeatCount::AutoFit {
                    out.auto_fit = Some((first, out.sizes.len()));
                }
            }
        }
        if out.sizes.len() as i64 >= MAX_TRACKS {
            break;
        }
    }
    Ok(out)
}

/// Line names of one axis, by explicit line index.
struct Lines<'n> {
    names: &'n [Vec<String>],
    /// Explicit tracks in this axis.
    count: i64,
}

impl Lines<'_> {
    fn has(&self, line: i64, name: &str) -> bool {
        usize::try_from(line)
            .ok()
            .and_then(|l| self.names.get(l))
            .is_some_and(|names| names.iter().any(|n| n == name))
    }

    /// The `n`th line named `name` from the start (`n > 0`) or the end
    /// (`n < 0`); implicit lines past the explicit grid all carry the name.
    fn nth(&self, name: &str, n: i64, work: &mut Work<'_>) -> Result<i64, FlowError> {
        let last = self.count;
        if n > 0 {
            let mut seen = 0;
            for line in 0..=last {
                work.charge(1)?;
                if self.has(line, name) {
                    seen += 1;
                    if seen == n {
                        return Ok(line);
                    }
                }
            }
            Ok(last + (n - seen))
        } else {
            let mut seen = 0;
            for line in (0..=last).rev() {
                work.charge(1)?;
                if self.has(line, name) {
                    seen += 1;
                    if seen == -n {
                        return Ok(line);
                    }
                }
            }
            Ok(-(-n - seen))
        }
    }

    /// Search from `from` in direction `step` for the `n`th line named
    /// `name` (span searches, §8.3.1).
    fn search(
        &self,
        from: i64,
        step: i64,
        name: &str,
        n: i64,
        work: &mut Work<'_>,
    ) -> Result<i64, FlowError> {
        let mut seen = 0;
        let mut line = from;
        loop {
            work.charge(1)?;
            line += step;
            if line < 0 || line > self.count {
                // Past the explicit grid every implicit line has the name.
                return Ok(line + step * (n - seen - 1));
            }
            if self.has(line, name) {
                seen += 1;
                if seen == n {
                    return Ok(line);
                }
            }
        }
    }

    /// A non-span line (§8.3.1), or `None` for `auto`/spans.
    fn definite(
        &self,
        value: &GridLine,
        end: bool,
        work: &mut Work<'_>,
    ) -> Result<Option<i64>, FlowError> {
        let GridLine::Line {
            span: false,
            integer,
            name,
        } = value
        else {
            return Ok(None);
        };
        Ok(Some(match (integer, name) {
            (Some(n), None) => {
                let n = i64::from(*n);
                if n > 0 { n - 1 } else { self.count + 1 + n }
            }
            (Some(n), Some(name)) => self.nth(name, i64::from(*n), work)?,
            (None, Some(name)) => {
                let suffixed = format!("{name}{}", if end { "-end" } else { "-start" });
                let mut found = None;
                for line in 0..=self.count {
                    work.charge(1)?;
                    if self.has(line, &suffixed) {
                        found = Some(line);
                        break;
                    }
                }
                match found {
                    Some(l) => l,
                    None => self.nth(name, 1, work)?,
                }
            }
            (None, None) => return Err(FlowError::InvalidSnapshot),
        }))
    }

    /// Resolve `grid-*-start`/`-end` into a placement (§8.3).
    fn resolve(
        &self,
        start: &GridLine,
        end: &GridLine,
        work: &mut Work<'_>,
    ) -> Result<Span, FlowError> {
        let span_of = |l: &GridLine| match l {
            GridLine::Line {
                span: true,
                integer,
                name,
            } => Some((i64::from(integer.unwrap_or(1)), name.clone())),
            _ => None,
        };
        let s = self.definite(start, false, work)?;
        let e = self.definite(end, true, work)?;
        Ok(match (s, e) {
            (Some(s), Some(e)) => {
                let (s, e) = if s > e { (e, s) } else { (s, e) };
                Span::Definite(s, if e == s { s + 1 } else { e })
            }
            (Some(s), None) => match span_of(end) {
                Some((n, None)) => Span::Definite(s, s + n),
                Some((n, Some(name))) => Span::Definite(s, self.search(s, 1, &name, n, work)?),
                None => Span::Definite(s, s + 1),
            },
            (None, Some(e)) => match span_of(start) {
                Some((n, None)) => Span::Definite(e - n, e),
                Some((n, Some(name))) => Span::Definite(self.search(e, -1, &name, n, work)?, e),
                None => Span::Definite(e - 1, e),
            },
            // A named span with no definite line is span 1; two spans
            // ignore the end span.
            (None, None) => match span_of(start).or_else(|| span_of(end)) {
                Some((n, None)) => Span::Auto(n),
                _ => Span::Auto(1),
            },
        })
    }
}

/// Whether one axis of `overflow` makes a scroll container (the automatic
/// minimum size is zero in that axis, §6.6).
fn scrolls(o: rustic_css::Overflow) -> bool {
    matches!(
        o,
        rustic_css::Overflow::Hidden | rustic_css::Overflow::Scroll | rustic_css::Overflow::Auto
    )
}

/// Clamp a line into the supported range.
fn clamp_line(n: i64) -> i64 {
    n.clamp(-MAX_TRACKS, MAX_TRACKS)
}

/// A placed area: (column lines, row lines), end exclusive.
type Area = ((i64, i64), (i64, i64));

/// One intrinsic sizing step (§11.5.1): whether it grows growth limits,
/// which tracks it affects, and the contribution it accommodates.
type SizingStep<'a> = (
    bool,
    &'a dyn Fn(&Track) -> bool,
    &'a dyn Fn(&Contribution) -> f64,
);

/// Occupied areas, as (columns, rows) line ranges.
struct Occupancy {
    areas: Vec<Area>,
}

impl Occupancy {
    fn free(
        &self,
        columns: (i64, i64),
        rows: (i64, i64),
        work: &mut Work<'_>,
    ) -> Result<bool, FlowError> {
        work.charge(self.areas.len())?;
        Ok(!self
            .areas
            .iter()
            .any(|(c, r)| c.0 < columns.1 && columns.0 < c.1 && r.0 < rows.1 && rows.0 < r.1))
    }
}

/// The automatic placement algorithm (§8.5) over (column, row) spans;
/// returns definite line ranges, possibly negative.
fn place(
    spans: &[(Span, Span)],
    column_flow: bool,
    dense: bool,
    explicit: (i64, i64),
    work: &mut Work<'_>,
) -> Result<Vec<Area>, FlowError> {
    // Work in (minor, major) terms: the minor axis is the one the cursor
    // sweeps (columns for row flow), the major axis grows.
    let flip = |(c, r): (Span, Span)| if column_flow { (r, c) } else { (c, r) };
    let items: Vec<(Span, Span)> = spans.iter().map(|&s| flip(s)).collect();
    let mut out: Vec<Option<Area>> = vec![None; items.len()];
    let mut occupied = Occupancy { areas: Vec::new() };
    let minor_explicit = if column_flow { explicit.1 } else { explicit.0 };
    // Minor-axis extent: the explicit grid, definite minor placements and
    // the widest auto span.
    let (mut minor_start, mut minor_end) = (0_i64, minor_explicit.max(0));
    for (minor, _) in &items {
        work.charge(1)?;
        match *minor {
            Span::Definite(s, e) => {
                minor_start = minor_start.min(s);
                minor_end = minor_end.max(e);
            }
            Span::Auto(n) => minor_end = minor_end.max(minor_start + n),
        }
    }
    // 1. Items definite in both axes.
    for (i, (minor, major)) in items.iter().enumerate() {
        if let (Span::Definite(a, b), Span::Definite(c, d)) = (*minor, *major) {
            out[i] = Some(((a, b), (c, d)));
            occupied.areas.push(((a, b), (c, d)));
        }
    }
    // 2. Items locked to a major-axis track.
    let mut cursors: Vec<(i64, i64)> = Vec::new();
    for (i, (minor, major)) in items.iter().enumerate() {
        let (Span::Auto(n), Span::Definite(c, d)) = (*minor, *major) else {
            continue;
        };
        let cursor = cursors.iter().find(|(row, _)| *row == c).map(|&(_, x)| x);
        let mut x = if dense {
            minor_start
        } else {
            cursor.unwrap_or(minor_start)
        };
        loop {
            work.charge(1)?;
            if occupied.free((x, x + n), (c, d), work)? || x > MAX_TRACKS {
                break;
            }
            x += 1;
        }
        minor_end = minor_end.max(x + n);
        out[i] = Some(((x, x + n), (c, d)));
        occupied.areas.push(((x, x + n), (c, d)));
        match cursors.iter_mut().find(|(row, _)| *row == c) {
            Some(entry) => entry.1 = x + n,
            None => cursors.push((c, x + n)),
        }
    }
    // 3. The rest, with a cursor.
    let (mut cx, mut cy) = (minor_start, 0_i64);
    let major_start = items
        .iter()
        .filter_map(|(_, major)| match major {
            Span::Definite(s, _) => Some(*s),
            Span::Auto(_) => None,
        })
        .min()
        .unwrap_or(0)
        .min(0);
    for (i, (minor, major)) in items.iter().enumerate() {
        if out[i].is_some() {
            continue;
        }
        let rows = match *major {
            Span::Auto(n) => n,
            Span::Definite(..) => continue,
        };
        if dense {
            cx = minor_start;
            cy = major_start;
        }
        let placed = match *minor {
            Span::Definite(a, b) => {
                // Definite minor position: advance the major cursor past it.
                if a < cx && !dense {
                    cy += 1;
                }
                loop {
                    work.charge(1)?;
                    if occupied.free((a, b), (cy, cy + rows), work)? || cy > MAX_TRACKS {
                        break;
                    }
                    cy += 1;
                }
                ((a, b), (cy, cy + rows))
            }
            Span::Auto(n) => loop {
                work.charge(1)?;
                if cx + n > minor_end {
                    cx = minor_start;
                    cy += 1;
                    continue;
                }
                if occupied.free((cx, cx + n), (cy, cy + rows), work)? || cy > MAX_TRACKS {
                    break ((cx, cx + n), (cy, cy + rows));
                }
                cx += 1;
            },
        };
        cx = placed.0.1;
        occupied.areas.push(placed);
        out[i] = Some(placed);
    }
    out.into_iter()
        .map(|p| {
            let (minor, major) = p.ok_or(FlowError::InvalidSnapshot)?;
            Ok(if column_flow {
                (major, minor)
            } else {
                (minor, major)
            })
        })
        .collect()
}

/// "Distribute extra space" (§11.5.1) for one span group and one sizing
/// step; returns the planned increase per track.
fn distribute_group(
    tracks: &[Track],
    group: &[(usize, usize, f64)],
    gap: f64,
    of_limit: bool,
    affected: &dyn Fn(&Track) -> bool,
    beyond: &dyn Fn(&Track) -> bool,
    work: &mut Work<'_>,
) -> Result<Vec<Option<f64>>, FlowError> {
    let mut planned: Vec<Option<f64>> = vec![None; tracks.len()];
    for &(start, end, size) in group {
        work.charge(end - start)?;
        let current = |t: &Track| {
            if of_limit && t.limit.is_finite() {
                t.limit
            } else {
                t.base
            }
        };
        let spanned = &tracks[start..end];
        let used: f64 =
            spanned.iter().map(current).sum::<f64>() + gap * (end - start).saturating_sub(1) as f64;
        let mut extra = (size - used).max(0.0);
        let chosen: Vec<usize> = (start..end).filter(|&t| affected(&tracks[t])).collect();
        // Affected tracks are touched even by a zero increase, which makes
        // an infinite growth limit finite.
        for &t in &chosen {
            planned[t] = Some(planned[t].unwrap_or(0.0));
        }
        if extra <= 0.0 || chosen.is_empty() {
            continue;
        }
        let mut increase = vec![0.0_f64; chosen.len()];
        // Up to each track's limit first.
        let room = |t: &Track| -> f64 {
            if of_limit {
                match t.fit {
                    Some(fit) => (fit.max(t.base) - current(t)).max(0.0),
                    None => f64::INFINITY,
                }
            } else {
                (t.limit - t.base).max(0.0)
            }
        };
        for _ in 0..=chosen.len() {
            let open: Vec<usize> = (0..chosen.len())
                .filter(|&j| increase[j] < room(&tracks[chosen[j]]))
                .collect();
            if open.is_empty() || extra <= 1e-9 {
                break;
            }
            let share = extra / open.len() as f64;
            for j in open {
                let t = &tracks[chosen[j]];
                let add = share.min(room(t) - increase[j]);
                increase[j] += add;
                extra -= add;
            }
        }
        // Then past the limits.
        if extra > 1e-9 {
            let mut past: Vec<usize> = (0..chosen.len())
                .filter(|&j| beyond(&tracks[chosen[j]]))
                .collect();
            if past.is_empty() && !of_limit {
                past = (0..chosen.len()).collect();
            }
            if !past.is_empty() {
                let share = extra / past.len() as f64;
                for j in past {
                    increase[j] += share;
                }
            }
        }
        for (j, &t) in chosen.iter().enumerate() {
            planned[t] = Some(planned[t].unwrap_or(0.0).max(increase[j]));
        }
    }
    Ok(planned)
}

/// The track sizing algorithm (§11.3) for one axis.
fn size_tracks(
    tracks: &mut [Track],
    gap: f64,
    available: Available,
    items: &[Contribution],
    work: &mut Work<'_>,
) -> Result<(), FlowError> {
    // §11.4 Initialize.
    for t in tracks.iter_mut() {
        t.base = match t.min {
            Breadth::Fixed(n) => n,
            _ => 0.0,
        };
        t.limit = match t.max {
            Breadth::Fixed(n) => n,
            _ => f64::INFINITY,
        };
        if t.limit < t.base {
            t.limit = t.base;
        }
        if t.collapsed {
            t.base = 0.0;
            t.limit = 0.0;
        }
    }
    let crosses_flex =
        |c: &Contribution, tracks: &[Track]| tracks[c.start..c.end].iter().any(Track::flexible);
    // Under a min-content constraint `auto` minimums use min-content
    // contributions, under a max-content constraint max-content ones
    // (limited contributions, §11.5 step 2).
    // Limited contributions are capped by fixed max sizing functions (or
    // `fit-content()` arguments) of every spanned track.
    let fixed_max: Vec<Option<f64>> = tracks
        .iter()
        .map(|t| match (t.max, t.fit) {
            (_, Some(fit)) => Some(fit),
            (Breadth::Fixed(f), _) => Some(f),
            _ => None,
        })
        .collect();
    let limited = |c: &Contribution, size: f64| -> f64 {
        let caps: Option<f64> = fixed_max[c.start..c.end].iter().copied().sum();
        match caps {
            Some(cap) => {
                let gaps = gap * (c.end - c.start).saturating_sub(1) as f64;
                size.min(cap + gaps).max(c.minimum)
            }
            None => size.max(c.minimum),
        }
    };
    let auto_min = |c: &Contribution| match available {
        Available::MinContent => limited(c, c.min_content),
        Available::MaxContent => limited(c, c.max_content),
        Available::Definite(_) => c.minimum,
    };
    // §11.5 Resolve intrinsic track sizes, by span count, items not
    // crossing flexible tracks first.
    let mut spans: Vec<usize> = items
        .iter()
        .filter(|c| !crosses_flex(c, tracks))
        .map(|c| c.end - c.start)
        .collect();
    spans.sort_unstable();
    spans.dedup();
    for span in spans {
        let group: Vec<&Contribution> = items
            .iter()
            .filter(|c| c.end - c.start == span && !crosses_flex(c, tracks))
            .collect();
        let steps: [SizingStep<'_>; 5] = [
            (false, &|t: &Track| t.min.intrinsic(), &auto_min),
            (
                false,
                &|t: &Track| matches!(t.min, Breadth::MinContent | Breadth::MaxContent),
                &|c: &Contribution| c.min_content,
            ),
            (
                false,
                &|t: &Track| {
                    t.min == Breadth::MaxContent
                        || (t.min == Breadth::Auto && available == Available::MaxContent)
                },
                // `auto` minimums take limited contributions under a
                // max-content constraint.
                &|c: &Contribution| {
                    if available == Available::MaxContent {
                        limited(c, c.max_content)
                    } else {
                        c.max_content
                    }
                },
            ),
            (true, &|t: &Track| t.intrinsic_max(), &|c: &Contribution| {
                c.min_content
            }),
            (
                true,
                &|t: &Track| {
                    matches!(t.max, Breadth::MaxContent | Breadth::Auto) || t.fit.is_some()
                },
                &|c: &Contribution| c.max_content,
            ),
        ];
        for (of_limit, affected, size) in steps {
            let sizes: Vec<(usize, usize, f64)> =
                group.iter().map(|c| (c.start, c.end, size(c))).collect();
            let planned = distribute_group(
                tracks,
                &sizes,
                gap,
                of_limit,
                affected,
                &|t: &Track| t.intrinsic_max(),
                work,
            )?;
            for (t, add) in tracks.iter_mut().zip(planned) {
                let Some(add) = add else { continue };
                if of_limit {
                    let from = if t.limit.is_finite() { t.limit } else { t.base };
                    t.limit = from + add;
                } else {
                    t.base += add;
                    if t.limit < t.base {
                        t.limit = t.base;
                    }
                }
            }
        }
    }
    // Items crossing flexible tracks: their minimums go to flexible tracks
    // with intrinsic minimums, in proportion to the flex factors.
    let flexed: Vec<&Contribution> = items.iter().filter(|c| crosses_flex(c, tracks)).collect();
    for c in flexed {
        work.charge(c.end - c.start)?;
        let spanned = &tracks[c.start..c.end];
        let used: f64 = spanned.iter().map(|t| t.base).sum::<f64>()
            + gap * (c.end - c.start).saturating_sub(1) as f64;
        let extra = (auto_min(c) - used).max(0.0);
        let flex: Vec<usize> = (c.start..c.end)
            .filter(|&t| tracks[t].flexible() && tracks[t].min.intrinsic())
            .collect();
        if extra <= 0.0 || flex.is_empty() {
            continue;
        }
        let factors: f64 = flex.iter().map(|&t| tracks[t].factor()).sum();
        for &t in &flex {
            let share = if factors > 0.0 {
                extra * tracks[t].factor() / factors
            } else {
                extra / flex.len() as f64
            };
            tracks[t].base += share;
        }
    }
    for t in tracks.iter_mut() {
        if !t.limit.is_finite() || t.limit < t.base {
            t.limit = t.base;
        }
    }
    let gaps = gap
        * tracks
            .iter()
            .filter(|t| !t.collapsed)
            .count()
            .saturating_sub(1) as f64;
    let total = |tracks: &[Track]| tracks.iter().map(|t| t.base).sum::<f64>() + gaps;
    // §11.6 Maximize tracks.
    match available {
        Available::Definite(space) => {
            let mut free = space - total(tracks);
            for _ in 0..=tracks.len() {
                let open: Vec<usize> = (0..tracks.len())
                    .filter(|&t| tracks[t].base < tracks[t].limit)
                    .collect();
                if free <= 1e-9 || open.is_empty() {
                    break;
                }
                let share = free / open.len() as f64;
                for t in open {
                    let add = share.min(tracks[t].limit - tracks[t].base);
                    tracks[t].base += add;
                    free -= add;
                }
            }
        }
        Available::MaxContent => {
            for t in tracks.iter_mut() {
                t.base = t.limit;
            }
        }
        Available::MinContent => {}
    }
    // §11.7 Expand flexible tracks.
    if tracks.iter().any(Track::flexible) {
        let fr = match available {
            Available::MinContent => 0.0,
            Available::Definite(space) => {
                let all: Vec<usize> = (0..tracks.len()).collect();
                find_fr(tracks, &all, space - gaps)
            }
            Available::MaxContent => {
                let mut fr = 0.0_f64;
                for t in tracks.iter().filter(|t| t.flexible()) {
                    let f = t.factor();
                    fr = fr.max(if f > 1.0 { t.base / f } else { t.base });
                }
                for c in items.iter().filter(|c| crosses_flex(c, tracks)) {
                    work.charge(1)?;
                    let range: Vec<usize> = (c.start..c.end).collect();
                    let space = c.max_content - gap * range.len().saturating_sub(1) as f64;
                    fr = fr.max(find_fr(tracks, &range, space));
                }
                fr
            }
        };
        for t in tracks.iter_mut().filter(|t| t.flexible()) {
            t.base = t.base.max(fr * t.factor());
        }
    }
    Ok(())
}

/// "Find the size of an fr" (§11.7.1) over `range` filling `space`.
fn find_fr(tracks: &[Track], range: &[usize], space: f64) -> f64 {
    let mut inflexible: Vec<bool> = range.iter().map(|&t| !tracks[t].flexible()).collect();
    for _ in 0..=range.len() {
        let leftover = space
            - range
                .iter()
                .zip(&inflexible)
                .filter(|(_, inf)| **inf)
                .map(|(&t, _)| tracks[t].base)
                .sum::<f64>();
        let factors: f64 = range
            .iter()
            .zip(&inflexible)
            .filter(|(_, inf)| !**inf)
            .map(|(&t, _)| tracks[t].factor())
            .sum::<f64>()
            .max(1.0);
        let hypothetical = (leftover / factors).max(0.0);
        let mut changed = false;
        for (j, &t) in range.iter().enumerate() {
            if !inflexible[j] && hypothetical * tracks[t].factor() < tracks[t].base {
                inflexible[j] = true;
                changed = true;
            }
        }
        if !changed {
            return hypothetical;
        }
    }
    0.0
}

/// §11.8: give positive free space to `auto`-max tracks.
fn stretch_auto(tracks: &mut [Track], free: f64) {
    let autos: Vec<usize> = (0..tracks.len())
        .filter(|&t| tracks[t].max == Breadth::Auto && !tracks[t].collapsed)
        .collect();
    if free > 0.0 && !autos.is_empty() {
        let share = free / autos.len() as f64;
        for t in autos {
            tracks[t].base += share;
        }
    }
}

/// Offset of the first track and extra space between tracks for
/// `justify-content`/`align-content` (CSS Box Alignment 3 §5.3).
fn distribute(a: Alignment, free: f64, count: usize, rtl_left: bool) -> (f64, f64) {
    if free < 0.0 && a.overflow == Some(OverflowPosition::Safe) {
        return (0.0, 0.0);
    }
    let n = count as f64;
    match a.keyword {
        K::Center => (free / 2.0, 0.0),
        K::End | K::FlexEnd | K::SelfEnd | K::FlowEnd => (free, 0.0),
        K::Right if !rtl_left => (free, 0.0),
        K::Left if rtl_left => (free, 0.0),
        K::SpaceBetween if count > 1 && free > 0.0 => (0.0, free / (n - 1.0)),
        K::SpaceAround if free > 0.0 && count > 0 => (free / n / 2.0, free / n),
        K::SpaceEvenly if free > 0.0 => (free / (n + 1.0), free / (n + 1.0)),
        _ => (0.0, 0.0),
    }
}

/// Track start positions given sizes, gaps and content alignment.
fn positions(tracks: &[Track], gap: f64, size: f64, a: Alignment, rtl_left: bool) -> Vec<f64> {
    let visible = tracks.iter().filter(|t| !t.collapsed).count();
    let used = tracks.iter().map(|t| t.base).sum::<f64>() + gap * visible.saturating_sub(1) as f64;
    let (offset, between) = distribute(a, size - used, visible, rtl_left);
    let mut out = Vec::with_capacity(tracks.len() + 1);
    let mut pos = offset;
    let mut first = true;
    for t in tracks {
        if !t.collapsed {
            if !first {
                pos += gap + between;
            }
            first = false;
        }
        out.push(pos);
        pos += t.base;
    }
    out.push(pos);
    out
}

/// The size of a grid area from aligned track positions: the start of
/// its first track to the end of its last.
fn area_from(pos: &[f64], tracks: &[Track], range: (usize, usize)) -> f64 {
    let last = range.1 - 1;
    (pos[last] + tracks[last].base - pos[range.0]).max(0.0)
}

/// Content alignment with `normal`/`stretch` falling back to `start`.
fn content(a: Alignment) -> Alignment {
    if matches!(a.keyword, K::Normal | K::Stretch) {
        Alignment::new(K::Start)
    } else {
        a
    }
}

/// The span of `range`'s tracks, including gutters between them.
fn area_size(tracks: &[Track], range: (usize, usize), gap: f64) -> f64 {
    let spanned = &tracks[range.0..range.1];
    let visible = spanned.iter().filter(|t| !t.collapsed).count();
    spanned.iter().map(|t| t.base).sum::<f64>() + gap * visible.saturating_sub(1) as f64
}

/// Resolve `normal`/`auto`/`legacy` self alignment: replaced boxes start,
/// everything else stretches.
fn self_alignment(a: Alignment, replaced: bool) -> Alignment {
    match a.keyword {
        K::Normal | K::Auto | K::Legacy if replaced => Alignment::new(K::Start),
        K::Normal | K::Auto | K::Legacy => Alignment::new(K::Stretch),
        K::LegacyLeft => Alignment::new(K::Left),
        K::LegacyRight => Alignment::new(K::Right),
        K::LegacyCenter => Alignment::new(K::Center),
        // Baseline self-alignment falls back to start (TD-209).
        K::Baseline | K::LastBaseline => Alignment::new(K::Start),
        _ => a,
    }
}

/// Offset of a box in its area for a self-alignment value.
fn self_offset(a: Alignment, free: f64) -> f64 {
    if free < 0.0 && a.overflow == Some(OverflowPosition::Safe) {
        return 0.0;
    }
    match a.keyword {
        K::Center => free / 2.0,
        K::End | K::FlexEnd | K::SelfEnd | K::FlowEnd => free,
        // Offsets run from the area's physical left edge.
        K::Right => free,
        _ => 0.0,
    }
}

impl Engine<'_, '_> {
    /// Read the container's placement inputs and place its in-flow items.
    /// `width` and `height` are the content box's definite sizes, if any.
    #[allow(
        clippy::too_many_arguments,
        reason = "both axes' sizes and repeat spaces"
    )]
    fn grid_setup(
        &mut self,
        c: usize,
        width: Option<f64>,
        height: Option<f64>,
        column_space: RepeatSpace,
        row_space: RepeatSpace,
        queue_absolute: bool,
    ) -> Result<GridRun, FlowError> {
        let style = self.nodes[c].style;
        let direction = self.nodes[c].direction;
        let column_gap = gap(style, P::ColumnGap, width)?;
        let row_gap = gap(style, P::RowGap, height)?;
        let template = |p| -> Result<GridTemplate, FlowError> {
            match grid_value(style, p)? {
                GridValue::Template(t) => Ok(t.clone()),
                _ => Err(FlowError::InvalidSnapshot),
            }
        };
        let mut columns = expand(
            &template(P::GridTemplateColumns)?,
            column_space,
            column_gap,
            &mut self.work,
        )?;
        let mut rows = expand(
            &template(P::GridTemplateRows)?,
            row_space,
            row_gap,
            &mut self.work,
        )?;
        let auto_sizes = |p| -> Result<Vec<TrackSize>, FlowError> {
            match grid_value(style, p)? {
                GridValue::AutoTracks(list) if !list.is_empty() => Ok(list.clone()),
                _ => Err(FlowError::InvalidSnapshot),
            }
        };
        let auto_columns = auto_sizes(P::GridAutoColumns)?;
        let auto_rows = auto_sizes(P::GridAutoRows)?;
        // Areas extend the explicit grid (tracks sized by `grid-auto-*`,
        // §7.3) and name their edge lines.
        if let GridValue::Areas(Some(areas)) = grid_value(style, P::GridTemplateAreas)? {
            let from = columns.sizes.len();
            while columns.sizes.len() < areas.columns() {
                let size = auto_columns[(columns.sizes.len() - from) % auto_columns.len()].clone();
                columns.add_track(&size);
            }
            let from = rows.sizes.len();
            while rows.sizes.len() < areas.cells.len() {
                let size = auto_rows[(rows.sizes.len() - from) % auto_rows.len()].clone();
                rows.add_track(&size);
            }
            for area in &areas.areas {
                self.work.charge(1)?;
                let named = |lines: &mut Explicit, at: usize, suffix: &str| {
                    if let Some(names) = lines.names.get_mut(at) {
                        names.push(format!("{}{suffix}", area.name));
                    }
                };
                named(&mut columns, area.columns.0, "-start");
                named(&mut columns, area.columns.1, "-end");
                named(&mut rows, area.rows.0, "-start");
                named(&mut rows, area.rows.1, "-end");
            }
        }
        let (flow_column, dense) = match grid_value(style, P::GridAutoFlow)? {
            GridValue::AutoFlow(f) => (f.column, f.dense),
            _ => return Err(FlowError::InvalidSnapshot),
        };
        let column_lines = Lines {
            names: &columns.names,
            count: columns.sizes.len() as i64,
        };
        let row_lines = Lines {
            names: &rows.names,
            count: rows.sizes.len() as i64,
        };
        let justify_items = alignment(style, P::JustifyItems)?;
        let align_items = alignment(style, P::AlignItems)?;
        let end = self.nodes[c].end;
        let mut items: Vec<(i32, Item)> = Vec::new();
        let mut spans = Vec::new();
        let mut i = c + 1;
        while i < end {
            self.work.charge(1)?;
            let child = &self.nodes[i];
            let next = child.end;
            if matches!(child.scheme, Scheme::Absolute | Scheme::Fixed) {
                if queue_absolute {
                    // Static position: the content box's start corner
                    // (grid-area containing blocks are TD-209).
                    let x = match direction {
                        Direction::Ltr => 0.0,
                        Direction::Rtl => width.unwrap_or(0.0),
                    };
                    self.nodes[i].static_pos = (x, 0.0);
                    self.push_absolute(i);
                }
                i = next;
                continue;
            }
            let s = child.style;
            let line = |p| -> Result<GridLine, FlowError> {
                match grid_value(s, p)? {
                    GridValue::Line(l) => Ok(l.clone()),
                    _ => Err(FlowError::InvalidSnapshot),
                }
            };
            let column = column_lines.resolve(
                &line(P::GridColumnStart)?,
                &line(P::GridColumnEnd)?,
                &mut self.work,
            )?;
            let row = row_lines.resolve(
                &line(P::GridRowStart)?,
                &line(P::GridRowEnd)?,
                &mut self.work,
            )?;
            let clamp = |s: Span| match s {
                Span::Definite(a, b) => {
                    let a = clamp_line(a);
                    Span::Definite(a, clamp_line(b).max(a + 1))
                }
                Span::Auto(n) => Span::Auto(n.clamp(1, MAX_TRACKS)),
            };
            spans.push((clamp(column), clamp(row)));
            let replaced = child.natural.is_some();
            let mut justify = alignment(s, P::JustifySelf)?;
            if justify.keyword == K::Auto {
                justify = justify_items;
            }
            let mut align = alignment(s, P::AlignSelf)?;
            if align.keyword == K::Auto {
                align = align_items;
            }
            let order = match s.get(P::Order) {
                V::Integer(n) => *n,
                _ => return Err(FlowError::InvalidSnapshot),
            };
            // `self-start`/`self-end` in the inline axis follow the item's
            // own direction; the block axis is shared (horizontal-tb only).
            let same_direction = child.direction == direction;
            let resolve_self = |mut a: Alignment, inline: bool| {
                if matches!(a.keyword, K::SelfStart | K::SelfEnd) {
                    let start = (a.keyword == K::SelfStart) == (!inline || same_direction);
                    a.keyword = if start { K::Start } else { K::End };
                }
                a
            };
            let justify = resolve_self(justify, true);
            let align = resolve_self(align, false);
            items.push((
                order,
                Item {
                    node: i,
                    columns: (0, 0),
                    rows: (0, 0),
                    justify: self_alignment(justify, replaced),
                    align: self_alignment(align, replaced),
                    padding: Edges::default(),
                    border: Edges::default(),
                    margin: Edges::default(),
                    auto_margin: [false; 4],
                    width: 0.0,
                    natural: 0.0,
                    laid: None,
                    height: 0.0,
                    x: 0.0,
                    y: 0.0,
                    area: (0.0, 0.0),
                },
            ));
            i = next;
        }
        // `order`-modified document order (a stable sort).
        type Keyed = (i32, (Item, (Span, Span)));
        let mut keyed: Vec<Keyed> = items
            .into_iter()
            .zip(spans)
            .map(|((o, item), s)| (o, (item, s)))
            .collect();
        keyed.sort_by_key(|(order, _)| *order);
        let (mut items, spans): (Vec<Item>, Vec<(Span, Span)>) =
            keyed.into_iter().map(|(_, pair)| pair).unzip();
        let explicit = (columns.sizes.len() as i64, rows.sizes.len() as i64);
        let placed = place(&spans, flow_column, dense, explicit, &mut self.work)?;
        // The implicit grid: shift so the first line is 0.
        let (mut c0, mut c1, mut r0, mut r1) = (0_i64, explicit.0, 0_i64, explicit.1);
        for ((a, b), (c, d)) in &placed {
            c0 = c0.min(*a);
            c1 = c1.max(*b);
            r0 = r0.min(*c);
            r1 = r1.max(*d);
        }
        let c1 = c1.min(c0 + MAX_TRACKS);
        let r1 = r1.min(r0 + MAX_TRACKS);
        let build = |explicit: &Explicit,
                     auto_list: &[TrackSize],
                     from: i64,
                     to: i64,
                     basis: Option<f64>,
                     work: &mut Work<'_>|
         -> Result<Vec<Track>, FlowError> {
            let mut out = Vec::new();
            let n = explicit.sizes.len() as i64;
            let len = auto_list.len() as i64;
            for line in from..to {
                work.charge(1)?;
                let size = if (0..n).contains(&line) {
                    &explicit.sizes[line as usize]
                } else if line >= n {
                    &auto_list[((line - n) % len) as usize]
                } else {
                    // Before the explicit grid the pattern runs backwards.
                    &auto_list[(len - 1 - ((-line - 1) % len)) as usize]
                };
                out.push(track(size, basis)?);
            }
            Ok(out)
        };
        let mut column_tracks = build(&columns, &auto_columns, c0, c1, width, &mut self.work)?;
        let mut row_tracks = build(&rows, &auto_rows, r0, r1, height, &mut self.work)?;
        // Ranges past the track limit clamp into the last track.
        let range =
            |start: i64, end: i64, from: i64, to: i64| -> Result<(usize, usize), FlowError> {
                if to <= from {
                    return Err(FlowError::InvalidSnapshot);
                }
                let s = start.clamp(from, to - 1);
                let e = end.clamp(s + 1, to);
                Ok(((s - from) as usize, (e - from) as usize))
            };
        for (item, ((a, b), (c, d))) in items.iter_mut().zip(&placed) {
            item.columns = range(*a, *b, c0, c1)?;
            item.rows = range(*c, *d, r0, r1)?;
        }
        // Empty auto-fit tracks collapse.
        for (explicit, tracks, origin, columns_axis) in [
            (&columns, &mut column_tracks, c0, true),
            (&rows, &mut row_tracks, r0, false),
        ] {
            if let Some((first, last)) = explicit.auto_fit {
                for t in first..last {
                    let index = (t as i64 - origin) as usize;
                    let used = items.iter().any(|item| {
                        let range = if columns_axis {
                            item.columns
                        } else {
                            item.rows
                        };
                        (range.0..range.1).contains(&index)
                    });
                    if !used {
                        if let Some(track) = tracks.get_mut(index) {
                            // A collapsed track is a fixed 0px track.
                            track.collapsed = true;
                            track.min = Breadth::Fixed(0.0);
                            track.max = Breadth::Fixed(0.0);
                            track.fit = None;
                        }
                    }
                }
            }
        }
        Ok(GridRun {
            container: c,
            direction,
            width: width.unwrap_or(0.0),
            definite_height: height,
            column_gap,
            row_gap,
            columns: column_tracks,
            rows: row_tracks,
            items,
            justify_content: alignment(style, P::JustifyContent)?,
            align_content: alignment(style, P::AlignContent)?,
            column_pos: Vec::new(),
            row_pos: Vec::new(),
            phase: Phase::Measure,
            queue: Vec::new(),
            current: None,
        })
    }

    /// An item's inline-axis contributions from its intrinsic sizes.
    fn column_contribution(
        &self,
        item: &Item,
        tracks: &[Track],
    ) -> Result<Contribution, FlowError> {
        let n = &self.nodes[item.node];
        let style = n.style;
        let outer = intrinsic::outer_contribution(style, n.intrinsic)?;
        // Automatic minimum size (§6.6): the content-based minimum unless
        // `min-width` is set or the item scrolls, clamped by the fixed
        // max sizes of the tracks it spans.
        // §6.6: with a preferred size that is not `auto` (and does not
        // depend on the grid area) the minimum contribution is the
        // min-content contribution.
        let preferred = Sizing::read(style, P::Width)?;
        let definite_width = match preferred {
            Sizing::Initial => false,
            Sizing::Length(v) => !intrinsic::has_percentage(v),
            _ => true,
        };
        let minimum = match Sizing::read(style, P::MinWidth)? {
            _ if definite_width => outer.min,
            Sizing::Initial if !scrolls(n.overflow.0) => {
                let mut m = outer.min;
                let spanned = &tracks[item.columns.0..item.columns.1];
                if spanned.iter().all(|t| matches!(t.max, Breadth::Fixed(_))) {
                    let fixed: f64 = spanned
                        .iter()
                        .map(|t| match t.max {
                            Breadth::Fixed(f) => f,
                            _ => 0.0,
                        })
                        .sum();
                    m = m.min(fixed);
                }
                m
            }
            Sizing::Initial => {
                intrinsic::fixed_extras(style)?
                    + intrinsic::fixed_margin(style, P::MarginLeft)?
                    + intrinsic::fixed_margin(style, P::MarginRight)?
            }
            _ => outer.min,
        };
        Ok(Contribution {
            start: item.columns.0,
            end: item.columns.1,
            minimum,
            min_content: outer.min,
            max_content: outer.max,
        })
    }

    /// Intrinsic inline sizes of a grid container (§11.1 under min- and
    /// max-content constraints).
    pub(super) fn grid_intrinsic(&mut self, i: usize) -> Result<Intrinsic, FlowError> {
        // Repetitions count against fixed `max-width`/`min-width` here.
        let style = self.nodes[i].style;
        let extras = intrinsic::fixed_extras(style)?;
        let r = Resolver::new(style, None, extras, Intrinsic::default());
        let (_, minimum, maximum) = r.widths(0.0)?;
        let columns = RepeatSpace {
            fit: maximum,
            fill: (minimum > 0.0).then_some(minimum),
        };
        let mut run = self.grid_setup(i, None, None, columns, RepeatSpace::default(), false)?;
        let contributions: Vec<Contribution> = run
            .items
            .iter()
            .map(|item| self.column_contribution(item, &run.columns))
            .collect::<Result<_, _>>()?;
        let mut sizes = [0.0; 2];
        for (slot, available) in [Available::MinContent, Available::MaxContent]
            .into_iter()
            .enumerate()
        {
            let mut tracks = run.columns.clone();
            size_tracks(
                &mut tracks,
                run.column_gap,
                available,
                &contributions,
                &mut self.work,
            )?;
            sizes[slot] = area_size(&tracks, (0, tracks.len()), run.column_gap);
        }
        run.items.clear();
        Ok(Intrinsic {
            min: sizes[0],
            max: sizes[1].max(sizes[0]),
        })
    }

    /// Start laying out grid container `c`, whose width and height
    /// constraint are already set. Returns its index in `self.grids`.
    pub(super) fn start_grid(&mut self, c: usize) -> Result<usize, FlowError> {
        let width = self.nodes[c].measure.content_width.get();
        let height = self.nodes[c].constraint()?.definite().map(CssPx::get);
        let constraint = self.nodes[c].constraint()?;
        let rows = match height {
            Some(h) => RepeatSpace {
                fit: Some(h),
                fill: None,
            },
            None => RepeatSpace {
                fit: constraint.maximum().map(CssPx::get),
                fill: Some(constraint.minimum().get()).filter(|&m| m > 0.0),
            },
        };
        let columns = RepeatSpace {
            fit: Some(width),
            fill: None,
        };
        let mut run = self.grid_setup(c, Some(width), height, columns, rows, true)?;
        let contributions: Vec<Contribution> = run
            .items
            .iter()
            .map(|item| self.column_contribution(item, &run.columns))
            .collect::<Result<_, _>>()?;
        size_tracks(
            &mut run.columns,
            run.column_gap,
            Available::Definite(width),
            &contributions,
            &mut self.work,
        )?;
        let free = width - area_size(&run.columns, (0, run.columns.len()), run.column_gap);
        if matches!(run.justify_content.keyword, K::Normal | K::Stretch) {
            stretch_auto(&mut run.columns, free);
        }
        let rtl = run.direction == Direction::Rtl;
        run.column_pos = positions(
            &run.columns,
            run.column_gap,
            width,
            content(run.justify_content),
            rtl,
        );
        // Lay every item out at its area width.
        let mut queue = Vec::with_capacity(run.items.len());
        for idx in (0..run.items.len()).rev() {
            self.work.charge(1)?;
            let area = area_from(&run.column_pos, &run.columns, run.items[idx].columns);
            let w = self.item_width(&mut run.items[idx], area)?;
            let n = &self.nodes[run.items[idx].node];
            let extras = run.items[idx].vertical_extras();
            // A definite height that needs no grid area (fixed lengths).
            let height = HeightConstraint::new(n.style, None, CssPx::new(extras)?)?
                .definite()
                .map(CssPx::get);
            queue.push(Request {
                item: idx,
                width: w,
                height,
            });
        }
        run.queue = queue;
        self.grids.push(run);
        Ok(self.grids.len() - 1)
    }

    /// Resolve an item's edges against its area width and return its
    /// content width (stretched, specified or fit-content).
    fn item_width(&self, item: &mut Item, area: f64) -> Result<f64, FlowError> {
        let n = &self.nodes[item.node];
        let s = n.style;
        let (padding, border) = box_edges(s, area)?;
        let side = |p| auto(s, p, area);
        let (t, r, b, l) = (
            side(P::MarginTop)?,
            side(P::MarginRight)?,
            side(P::MarginBottom)?,
            side(P::MarginLeft)?,
        );
        let px = |m: Option<f64>| CssPx::new(m.unwrap_or(0.0));
        item.padding = padding;
        item.border = border;
        item.margin = Edges {
            top: px(t)?,
            right: px(r)?,
            bottom: px(b)?,
            left: px(l)?,
        };
        item.auto_margin = [t.is_none(), r.is_none(), b.is_none(), l.is_none()];
        let extras = item.horizontal_extras();
        let available = area - item.margin.left.get() - item.margin.right.get() - extras;
        let resolver = Resolver::new(s, Some(area), extras, n.intrinsic);
        let (specified, minimum, maximum) = resolver.widths(available)?;
        let stretch =
            item.justify.keyword == K::Stretch && !item.auto_margin[1] && !item.auto_margin[3];
        let tentative = match specified {
            Some(w) => w,
            None => match n.natural {
                Some(natural) => replaced::used_size(s, natural, Some(area), None)?.0,
                None if stretch => available.max(0.0),
                None => n.intrinsic.fit(available),
            },
        };
        Ok(maximum
            .map_or(tentative, |m| tentative.min(m))
            .max(minimum)
            .max(0.0))
    }

    /// Advance grid container `k`: push the next item frame and return
    /// `None`, or finish and return the container's natural content height.
    pub(super) fn grid_step(
        &mut self,
        frames: &mut Vec<Frame>,
        bfcs: &mut Vec<Bfc>,
        k: usize,
    ) -> Result<Option<f64>, FlowError> {
        loop {
            self.work.charge(1)?;
            if let Some(req) = self.grids[k].queue.pop() {
                let item = self.grids[k].items[req.item];
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
                self.grids[k].items[req.item].width = req.width;
                self.grids[k].items[req.item].laid = req.height;
                if let Some(natural) = n.natural {
                    // A replaced item has no content to lay out.
                    let style = n.style;
                    let (_, h) = replaced::used_size(style, natural, Some(req.width), None)?;
                    let content = match (req.height, natural.ratio()) {
                        (Some(h), _) => h,
                        (None, Some(r)) if Sizing::read(style, P::Height)?.length().is_none() => {
                            req.width / r
                        }
                        _ => h,
                    };
                    self.grids[k].items[req.item].natural = content;
                    continue;
                }
                self.grids[k].current = Some(req.item);
                frames.push(self.root_frame(item.node)?);
                bfcs.push(Bfc::default());
                return Ok(None);
            }
            match self.grids[k].phase {
                Phase::Measure => self.grid_rows(k)?,
                Phase::Final => {
                    let natural = self.finish_grid(k)?;
                    if self.grids.len() != k + 1 {
                        return Err(FlowError::InvalidSnapshot);
                    }
                    self.grids.pop();
                    return Ok(Some(natural));
                }
            }
        }
    }

    /// Record the natural content height of the item just laid out.
    pub(super) fn grid_item_done(
        &mut self,
        k: usize,
        node: usize,
        natural: f64,
    ) -> Result<(), FlowError> {
        let run = &mut self.grids[k];
        let item = run.current.take().ok_or(FlowError::InvalidSnapshot)?;
        if run.items[item].node != node {
            return Err(FlowError::InvalidSnapshot);
        }
        run.items[item].natural = natural;
        Ok(())
    }

    /// Size the rows from the items' heights, then queue items whose final
    /// height differs from the measured one.
    fn grid_rows(&mut self, k: usize) -> Result<(), FlowError> {
        let count = self.grids[k].items.len();
        let mut contributions = Vec::with_capacity(count);
        for idx in 0..count {
            self.work.charge(1)?;
            let item = self.grids[k].items[idx];
            let n = &self.nodes[item.node];
            let extras = item.vertical_extras();
            let constraint = HeightConstraint::new(n.style, None, CssPx::new(extras)?)?;
            let used = match item.laid {
                Some(h) => h,
                None => constraint.used(CssPx::new(item.natural.max(0.0))?).get(),
            };
            let margins = item.margin.top.get() + item.margin.bottom.get();
            let outer = used + extras + margins;
            let minimum = match Sizing::read(n.style, P::MinHeight)? {
                Sizing::Initial if scrolls(n.overflow.1) => extras + margins,
                _ => outer,
            };
            contributions.push(Contribution {
                start: item.rows.0,
                end: item.rows.1,
                minimum,
                min_content: outer,
                max_content: outer,
            });
        }
        let container = self.grids[k].container;
        let constraint = self.nodes[container].constraint()?;
        let run = &mut self.grids[k];
        let available = match run.definite_height {
            Some(h) => Available::Definite(h),
            None => Available::MaxContent,
        };
        let initial = run.rows.clone();
        size_tracks(
            &mut run.rows,
            run.row_gap,
            available,
            &contributions,
            &mut self.work,
        )?;
        // §11.7: indefinite flexible rows that break `min-height` or
        // `max-height` are sized again against that bound.
        if available == Available::MaxContent && run.rows.iter().any(Track::flexible) {
            let total = area_size(&run.rows, (0, run.rows.len()), run.row_gap);
            let bound = constraint.used(CssPx::new(total.max(0.0))?).get();
            if (bound - total).abs() > 1e-9 {
                run.rows = initial;
                size_tracks(
                    &mut run.rows,
                    run.row_gap,
                    Available::Definite(bound),
                    &contributions,
                    &mut self.work,
                )?;
            }
        }
        let natural = area_size(&run.rows, (0, run.rows.len()), run.row_gap);
        let height = constraint.used(CssPx::new(natural.max(0.0))?).get();
        if matches!(run.align_content.keyword, K::Normal | K::Stretch) {
            stretch_auto(&mut run.rows, height - natural);
        }
        run.row_pos = positions(
            &run.rows,
            run.row_gap,
            height,
            content(run.align_content),
            false,
        );
        // Final heights.
        let mut queue = Vec::new();
        for idx in (0..count).rev() {
            self.work.charge(1)?;
            let run = &self.grids[k];
            let item = run.items[idx];
            let area = area_from(&run.row_pos, &run.rows, item.rows);
            let n = &self.nodes[item.node];
            let extras = item.vertical_extras();
            let against_area = HeightConstraint::new(
                n.style,
                Some(CssPx::new(area.max(0.0))?),
                CssPx::new(extras)?,
            )?;
            let margins = item.margin.top.get() + item.margin.bottom.get();
            let stretch = item.align.keyword == K::Stretch
                && !item.auto_margin[0]
                && !item.auto_margin[2]
                && against_area.specified().is_none();
            let (height, definite) = if let Some(h) = against_area.definite() {
                (h.get(), true)
            } else if stretch {
                let target = (area - margins - extras).max(0.0);
                (against_area.used(CssPx::new(target)?).get(), true)
            } else {
                (
                    against_area.used(CssPx::new(item.natural.max(0.0))?).get(),
                    false,
                )
            };
            self.grids[k].items[idx].height = height;
            let unchanged = item.laid == Some(height)
                || (item.laid.is_none() && !definite && height == item.natural)
                || (item.laid.is_none()
                    && height == item.natural
                    && self.grids[k].definite_height.is_none());
            if unchanged || n.natural.is_some() {
                continue;
            }
            queue.push(Request {
                item: idx,
                width: item.width,
                height: Some(height),
            });
        }
        let run = &mut self.grids[k];
        run.queue = queue;
        run.phase = Phase::Final;
        Ok(())
    }

    /// Track alignment and final item geometry; returns the container's
    /// natural content height.
    fn finish_grid(&mut self, k: usize) -> Result<f64, FlowError> {
        let run = &mut self.grids[k];
        let natural = area_size(&run.rows, (0, run.rows.len()), run.row_gap);
        let rtl = run.direction == Direction::Rtl;
        // Columns run from the inline start; `left`/`right` are physical.
        let xs = run.column_pos.clone();
        let ys = run.row_pos.clone();
        let width = run.width;
        let direction = run.direction;
        let columns = run.columns.clone();
        let rows = run.rows.clone();
        let mut items = run.items.clone();
        for item in &mut items {
            self.work.charge(1)?;
            let area_w = area_from(&xs, &columns, item.columns);
            let area_h = area_from(&ys, &rows, item.rows);
            let area_x = xs[item.columns.0];
            let area_y = ys[item.rows.0];
            // Inline axis: auto margins absorb free space, then justify-self.
            let outer_w = item.width
                + item.horizontal_extras()
                + item.margin.left.get()
                + item.margin.right.get();
            let free_w = area_w - outer_w;
            let (ml, mr) = (item.auto_margin[3], item.auto_margin[1]);
            let mut offset_x = if (ml || mr) && free_w > 0.0 {
                let share = if ml && mr { free_w / 2.0 } else { free_w };
                if ml {
                    item.margin.left = CssPx::new(share)?;
                }
                if mr {
                    item.margin.right = CssPx::new(share)?;
                }
                0.0
            } else {
                self_offset(item.justify, free_w)
            };
            // Self alignment in RTL measures from the right edge.
            if rtl && !(ml || mr) {
                offset_x = match item.justify.keyword {
                    K::Left | K::Right => offset_x,
                    _ => free_w - offset_x,
                };
            }
            // Block axis.
            let outer_h = item.height
                + item.vertical_extras()
                + item.margin.top.get()
                + item.margin.bottom.get();
            let free_h = area_h - outer_h;
            let (mt, mb) = (item.auto_margin[0], item.auto_margin[2]);
            let offset_y = if (mt || mb) && free_h > 0.0 {
                let share = if mt && mb { free_h / 2.0 } else { free_h };
                if mt {
                    item.margin.top = CssPx::new(share)?;
                }
                if mb {
                    item.margin.bottom = CssPx::new(share)?;
                }
                0.0
            } else {
                self_offset(item.align, free_h)
            };
            // Columns are laid out from the inline start; mirror in RTL.
            let x = if rtl {
                width - area_x - area_w + offset_x
            } else {
                area_x + offset_x
            };
            item.x = x;
            item.y = area_y + offset_y;
            item.area = (area_w, area_h);
        }
        for item in &items {
            self.work.charge(1)?;
            let n = &mut self.nodes[item.node];
            n.measure = BlockMeasure {
                content_width: CssPx::new(item.width.max(0.0))?,
                margin: item.margin,
                padding: item.padding,
                border: item.border,
            };
            n.content_height = CssPx::new(item.height.max(0.0))?;
            n.border_height = n.content_height.checked_add(n.vertical_extras()?)?;
            n.pos = (
                item.x + item.margin.left.get(),
                item.y + item.margin.top.get(),
            );
            n.top_resolved = true;
            if n.relative {
                // Percentages resolve against the grid area.
                n.rel = position::relative_offset(
                    n.style,
                    CssPx::new(item.area.0)?,
                    Some(CssPx::new(item.area.1)?),
                    direction,
                )?;
            }
        }
        self.grids[k].items = items;
        Ok(natural)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustic_base::CancellationToken;

    fn tracks(list: &[(Breadth, Breadth)]) -> Vec<Track> {
        list.iter()
            .map(|&(min, max)| Track {
                min,
                max,
                fit: None,
                base: 0.0,
                limit: 0.0,
                collapsed: false,
            })
            .collect()
    }

    fn sized(
        list: &[(Breadth, Breadth)],
        gap: f64,
        available: Available,
        items: &[Contribution],
    ) -> Vec<f64> {
        let cancel = CancellationToken::default();
        let mut work = Work {
            spent: 0,
            limit: 1_000_000,
            cancellation: &cancel,
        };
        let mut t = tracks(list);
        size_tracks(&mut t, gap, available, items, &mut work).unwrap();
        t.iter().map(|t| t.base).collect()
    }

    fn item(start: usize, end: usize, min: f64, max: f64) -> Contribution {
        Contribution {
            start,
            end,
            minimum: min,
            min_content: min,
            max_content: max,
        }
    }

    use Breadth::{Auto, Fixed, Flex, MaxContent, MinContent};

    #[test]
    fn fixed_and_flexible_tracks_share_definite_space() {
        // 100px 1fr 2fr in 400px with 10px gaps: 280px of fr space.
        let sizes = sized(
            &[
                (Fixed(100.0), Fixed(100.0)),
                (Auto, Flex(1.0)),
                (Auto, Flex(2.0)),
            ],
            10.0,
            Available::Definite(400.0),
            &[],
        );
        assert_eq!(sizes, [100.0, 280.0 / 3.0, 560.0 / 3.0]);
        // A flexible track whose content needs more than its share is
        // treated as inflexible.
        let sizes = sized(
            &[(Auto, Flex(1.0)), (Auto, Flex(1.0))],
            0.0,
            Available::Definite(100.0),
            &[item(0, 1, 80.0, 80.0)],
        );
        assert_eq!(sizes, [80.0, 20.0]);
    }

    #[test]
    fn intrinsic_tracks_grow_to_contributions() {
        // auto tracks: min contribution then max-content, then free space.
        let sizes = sized(
            &[
                (Auto, Auto),
                (MinContent, MinContent),
                (MaxContent, MaxContent),
            ],
            0.0,
            Available::Definite(1000.0),
            &[
                item(0, 1, 10.0, 50.0),
                item(1, 2, 20.0, 60.0),
                item(2, 3, 30.0, 70.0),
            ],
        );
        assert_eq!(sizes, [50.0, 20.0, 70.0]);
        // Under a min-content constraint nothing is maximized.
        let sizes = sized(
            &[(Auto, Auto)],
            0.0,
            Available::MinContent,
            &[item(0, 1, 10.0, 50.0)],
        );
        assert_eq!(sizes, [10.0]);
        // A spanning item distributes its excess equally.
        let sizes = sized(
            &[(Auto, Auto), (Auto, Auto)],
            10.0,
            Available::MaxContent,
            &[item(0, 1, 20.0, 20.0), item(0, 2, 70.0, 70.0)],
        );
        assert_eq!(sizes, [20.0, 40.0]);
    }

    #[test]
    fn indefinite_flexible_tracks_use_their_contributions() {
        // 1fr 2fr with max-content items 30px and 100px: the fr size is
        // max(30/1, 100/2) = 50.
        let sizes = sized(
            &[(Auto, Flex(1.0)), (Auto, Flex(2.0))],
            0.0,
            Available::MaxContent,
            &[item(0, 1, 0.0, 30.0), item(1, 2, 0.0, 100.0)],
        );
        assert_eq!(sizes, [50.0, 100.0]);
        assert_eq!(
            sized(&[(Auto, Flex(1.0))], 0.0, Available::MinContent, &[]),
            [0.0]
        );
    }

    #[test]
    fn auto_placement_fills_rows_and_respects_locked_items() {
        let cancel = CancellationToken::default();
        let mut work = Work {
            spent: 0,
            limit: 1_000_000,
            cancellation: &cancel,
        };
        let a = |n| Span::Auto(n);
        let d = |s, e| Span::Definite(s, e);
        // Two explicit columns; the second item spans both.
        let placed = place(
            &[(a(1), a(1)), (a(2), a(1)), (a(1), a(1)), (d(1, 2), d(0, 1))],
            false,
            false,
            (2, 0),
            &mut work,
        )
        .unwrap();
        assert_eq!(
            placed,
            [
                ((0, 1), (0, 1)),
                ((0, 2), (1, 2)),
                ((0, 1), (2, 3)),
                ((1, 2), (0, 1)),
            ]
        );
        // Dense packing back-fills the hole.
        let placed = place(
            &[(a(1), a(1)), (a(2), a(1)), (a(1), a(1))],
            false,
            true,
            (2, 0),
            &mut work,
        )
        .unwrap();
        assert_eq!(
            placed,
            [((0, 1), (0, 1)), ((0, 2), (1, 2)), ((1, 2), (0, 1))]
        );
        // Column flow fills columns.
        let placed = place(
            &[(a(1), a(1)), (a(1), a(1))],
            true,
            false,
            (0, 1),
            &mut work,
        )
        .unwrap();
        assert_eq!(placed, [((0, 1), (0, 1)), ((1, 2), (0, 1))]);
    }
}
