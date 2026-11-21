//! Overflow clipping and scroll-container geometry (CSS Overflow 3 §2–3).
//!
//! Layout runs at scroll position zero; `crate::scroll` applies other
//! positions afterwards. No scrollbar gutter is reserved (as with overlay
//! scrollbars). Clips follow the
//! containing-block chain, so an absolutely positioned box escapes the overflow
//! of ancestors between it and its containing block, and fixed boxes escape all.

use rustic_css::{Direction, Overflow};
use std::fmt;

use crate::flow::Work;
use crate::{BlockFragment, CssPoint, CssPx, CssRect, CssSize, FlowError};

/// A half-open clip interval along one axis, in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisRange {
    /// Inclusive start coordinate.
    pub start: CssPx,
    /// Exclusive end coordinate; an empty interval has `end <= start`.
    pub end: CssPx,
}

/// The clip applied to a box; `None` on an axis means unclipped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clip {
    /// Horizontal clip interval.
    pub x: Option<AxisRange>,
    /// Vertical clip interval.
    pub y: Option<AxisRange>,
}

impl Clip {
    /// No clipping on either axis.
    pub const NONE: Self = Self { x: None, y: None };

    /// Whether neither axis is clipped.
    pub const fn is_none(self) -> bool {
        self.x.is_none() && self.y.is_none()
    }

    /// Whether a point survives the clip.
    pub fn contains(self, point: CssPoint) -> bool {
        let inside = |r: Option<AxisRange>, v: CssPx| r.is_none_or(|r| r.start <= v && v < r.end);
        inside(self.x, point.x) && inside(self.y, point.y)
    }

    fn intersect(self, other: Self) -> Self {
        let both = |a: Option<AxisRange>, b: Option<AxisRange>| match (a, b) {
            (Some(a), Some(b)) => Some(AxisRange {
                start: if a.start > b.start { a.start } else { b.start },
                end: if a.end < b.end { a.end } else { b.end },
            }),
            (a, None) => a,
            (None, b) => b,
        };
        Self {
            x: both(self.x, other.x),
            y: both(self.y, other.y),
        }
    }
}

impl fmt::Display for Clip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let axis = |r: Option<AxisRange>| {
            r.map_or_else(
                || "*".into(),
                |r| format!("{}..{}", r.start.get(), r.end.get()),
            )
        };
        write!(f, "({},{})", axis(self.x), axis(self.y))
    }
}

fn scroll_container(o: (Overflow, Overflow)) -> bool {
    [o.0, o.1]
        .iter()
        .any(|o| matches!(o, Overflow::Hidden | Overflow::Scroll | Overflow::Auto))
}

/// A box's own clip for its contents: its padding box on non-visible axes.
fn own_clip(f: &BlockFragment) -> Result<Clip, FlowError> {
    let p = f.padding();
    let range = |start: CssPx, size: CssPx| -> Result<AxisRange, FlowError> {
        Ok(AxisRange {
            start,
            end: start.checked_add(size)?,
        })
    };
    let (ox, oy) = f.overflow();
    Ok(Clip {
        x: (ox != Overflow::Visible)
            .then(|| range(p.origin().x, p.size().width()))
            .transpose()?,
        y: (oy != Overflow::Visible)
            .then(|| range(p.origin().y, p.size().height()))
            .transpose()?,
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Viewport,
    Box(usize),
    Discard,
}

/// Running union as (left, top, right, bottom).
type Area = (f64, f64, f64, f64);

fn union(a: Area, b: Area) -> Area {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

fn area(r: CssRect) -> Area {
    let (x, y) = (r.origin().x.get(), r.origin().y.get());
    (
        x,
        y,
        x + r.size().width().get(),
        y + r.size().height().get(),
    )
}

/// Restrict an area to a clip; `None` if nothing remains.
fn restrict(a: Area, clip: Clip) -> Option<Area> {
    let mut a = a;
    if let Some(r) = clip.x {
        a.0 = a.0.max(r.start.get());
        a.2 = a.2.min(r.end.get());
    }
    if let Some(r) = clip.y {
        a.1 = a.1.max(r.start.get());
        a.3 = a.3.min(r.end.get());
    }
    (a.0 < a.2 && a.1 < a.3).then_some(a)
}

/// Clamp to the reachable region (no scrolling before the scroll origin).
fn reachable(a: Area, padding: Area, direction: Direction) -> Result<CssRect, FlowError> {
    let top = a.1.max(padding.1);
    let (left, right) = match direction {
        Direction::Ltr => (a.0.max(padding.0), a.2),
        Direction::Rtl => (a.0, a.2.min(padding.2)),
    };
    Ok(CssRect::new(
        CssPoint {
            x: CssPx::new(left)?,
            y: CssPx::new(top)?,
        },
        CssSize::new((right - left).max(0.0), (a.3 - top).max(0.0))?,
    )?)
}

/// Recompute every fragment's clip from its containing block's clip and
/// padding box, as [`assign`] does, after fragments moved (scrolling).
pub(crate) fn reclip(fragments: &mut [BlockFragment]) {
    reclip_from(fragments, None);
}

/// Like [`reclip`], but boxes whose containing block is `root` start
/// unclipped: neither `root` nor its ancestors clip `root`'s contents
/// (a scroll container's layer, which the compositor clips instead).
pub(crate) fn reclip_from(fragments: &mut [BlockFragment], root: Option<usize>) {
    let mut own: Vec<Clip> = Vec::with_capacity(fragments.len());
    for i in 0..fragments.len() {
        let f = fragments[i];
        let effective = match f.containing_block() {
            _ if f.scheme() == crate::PositionScheme::Fixed => Clip::NONE,
            Some(c) if Some(c) == root => Clip::NONE,
            Some(c) if c < i => fragments[c].clip().intersect(own[c]),
            _ => Clip::NONE,
        };
        fragments[i].set_clip(effective);
        own.push(own_clip(&f).unwrap_or(Clip::NONE));
    }
}

/// Assign effective clips and scrollable overflow; returns the viewport's.
///
/// `containing[i]` is box `i`'s containing-block fragment (`None` for the
/// initial containing block or the viewport); preorder guarantees it precedes `i`.
pub(crate) fn assign(
    fragments: &mut [BlockFragment],
    containing: &[Option<usize>],
    fixed: &[bool],
    directions: &[Direction],
    viewport: CssSize,
    root_direction: Direction,
    work: &mut Work,
) -> Result<CssRect, FlowError> {
    let n = fragments.len();
    // Per box: own clip, clip of contributions through it, and their target.
    let mut own = Vec::with_capacity(n);
    let mut through: Vec<Clip> = Vec::with_capacity(n);
    let mut target: Vec<Target> = Vec::with_capacity(n);
    let mut areas: Vec<Option<Area>> = vec![None; n];
    let mut viewport_area = (0.0, 0.0, viewport.width().get(), viewport.height().get());
    for i in 0..n {
        work.charge(1)?;
        let f = fragments[i];
        let clip = own_clip(&f)?;
        let (effective, restriction, to) = match containing[i] {
            Some(c) => (fragments[c].clip().intersect(own[c]), through[c], target[c]),
            None if fixed[i] => (Clip::NONE, Clip::NONE, Target::Discard),
            None => (Clip::NONE, Clip::NONE, Target::Viewport),
        };
        fragments[i].set_clip(effective);
        if let Some(a) = restrict(area(f.border()), restriction) {
            match to {
                Target::Viewport => viewport_area = union(viewport_area, a),
                Target::Box(s) => {
                    areas[s] = Some(areas[s].map_or(a, |b| union(a, b)));
                }
                Target::Discard => {}
            }
        }
        own.push(clip);
        if scroll_container(f.overflow()) {
            through.push(Clip::NONE);
            target.push(Target::Box(i));
        } else {
            through.push(restriction.intersect(clip));
            target.push(to);
        }
    }
    for i in 0..n {
        work.charge(1)?;
        if scroll_container(fragments[i].overflow()) {
            let padding = area(fragments[i].padding());
            let total = areas[i].map_or(padding, |a| union(a, padding));
            fragments[i].set_scrollable(reachable(total, padding, directions[i])?);
        }
    }
    let icb = (0.0, 0.0, viewport.width().get(), viewport.height().get());
    reachable(viewport_area, icb, root_direction)
}
