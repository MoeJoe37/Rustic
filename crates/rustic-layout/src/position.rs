//! Relative offsets (CSS 2.2 §9.4.3), float widths (§10.3.5) and the
//! absolutely positioned width/height equations (§10.3.7, §10.6.4).

use rustic_css::{
    BoxSizing, Direction, LengthPercentageOrAuto, PropertyId as P, PropertyValue as V,
};
use rustic_style::ComputedStyle;

use crate::intrinsic::{Intrinsic, Resolver, Sizing, fixed};
use crate::sizing::{auto, box_edges, lp};
use crate::{BlockMeasure, CssPoint, CssPx, Edges, MeasureError};

fn border_box(style: &ComputedStyle) -> bool {
    matches!(style.get(P::BoxSizing), V::BoxSizing(BoxSizing::BorderBox))
}

/// Run a solver with the tentative size, then max, then min (CSS 2.2 §10.4/§10.7).
fn constrain<T>(
    specified: Option<f64>,
    minimum: f64,
    maximum: Option<f64>,
    mut solve: impl FnMut(Option<f64>) -> Result<(f64, T), MeasureError>,
) -> Result<(f64, T), MeasureError> {
    let mut result = solve(specified)?;
    if let Some(max) = maximum.filter(|&max| result.0 > max) {
        result = solve(Some(max))?;
    }
    if result.0 < minimum {
        result = solve(Some(minimum))?;
    }
    Ok(result)
}

/// Width of a floating, non-replaced box (CSS 2.2 §10.3.5).
pub(crate) fn measure_float(
    style: &ComputedStyle,
    containing_width: CssPx,
    intrinsic: Intrinsic,
) -> Result<BlockMeasure, MeasureError> {
    let basis = containing_width.get();
    let (padding, border) = box_edges(style, basis)?;
    let extras = padding.left.get() + padding.right.get() + border.left.get() + border.right.get();
    let ml = auto(style, P::MarginLeft, basis)?.unwrap_or(0.0);
    let mr = auto(style, P::MarginRight, basis)?.unwrap_or(0.0);
    let resolver = Resolver::new(style, Some(basis), extras, intrinsic);
    let (specified, minimum, maximum) = resolver.widths(basis - ml - mr - extras)?;
    let (width, ()) = constrain(specified, minimum, maximum, |w| {
        Ok((
            w.unwrap_or_else(|| intrinsic.fit(basis - ml - mr - extras)),
            (),
        ))
    })?;
    Ok(BlockMeasure {
        content_width: CssPx::new(width.max(0.0))?,
        padding,
        border,
        margin: Edges {
            top: CssPx::new(auto(style, P::MarginTop, basis)?.unwrap_or(0.0))?,
            right: CssPx::new(mr)?,
            bottom: CssPx::new(auto(style, P::MarginBottom, basis)?.unwrap_or(0.0))?,
            left: CssPx::new(ml)?,
        },
    })
}

/// Relative offset; percentages of an indefinite containing height are auto.
pub(crate) fn relative_offset(
    style: &ComputedStyle,
    width: CssPx,
    height: Option<CssPx>,
    direction: Direction,
) -> Result<CssPoint, MeasureError> {
    let vertical = |p: P| -> Result<Option<f64>, MeasureError> {
        match style.get(p) {
            V::LengthPercentageOrAuto(LengthPercentageOrAuto::Auto) => Ok(None),
            V::LengthPercentageOrAuto(LengthPercentageOrAuto::LengthPercentage(v)) => {
                match height {
                    Some(h) => lp(v, h.get()).map(Some),
                    None => fixed(v),
                }
            }
            _ => Err(MeasureError::InvalidComputedValue),
        }
    };
    let left = auto(style, P::Left, width.get())?;
    let right = auto(style, P::Right, width.get())?;
    let dx = match (left, right, direction) {
        (None, None, _) => 0.0,
        (Some(l), None, _) | (Some(l), Some(_), Direction::Ltr) => l,
        (None, Some(r), _) | (Some(_), Some(r), Direction::Rtl) => -r,
    };
    let dy = match (vertical(P::Top)?, vertical(P::Bottom)?) {
        (Some(t), _) => t,
        (None, Some(b)) => -b,
        (None, None) => 0.0,
    };
    Ok(CssPoint {
        x: CssPx::new(dx)?,
        y: CssPx::new(dy)?,
    })
}

/// One axis of the absolutely positioned constraint equation.
struct Axis {
    start: Option<f64>,
    end: Option<f64>,
    margin_start: Option<f64>,
    margin_end: Option<f64>,
    containing: f64,
    extras: f64,
    /// Static margin-edge coordinate, measured from the containing start edge.
    static_edge: f64,
    /// Whether the static position sets `start` (else `end`).
    static_start: bool,
    /// Over-constrained cases drop `end` (else `start`).
    keep_start: bool,
    /// Negative equal auto margins zero the start margin (horizontal only).
    horizontal: bool,
}

/// Used start offset, content size and margins along one axis.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Solved {
    pub start: f64,
    pub size: f64,
    pub margin_start: f64,
    pub margin_end: f64,
}

impl Axis {
    fn solve(&self, size: Option<f64>, auto_size: impl Fn(f64) -> f64) -> (f64, Solved) {
        let cb = self.containing;
        let rest = |s: f64, w: f64, ms: f64, me: f64| cb - s - w - ms - me - self.extras;
        let (mut s, mut e) = (self.start, self.end);
        let done = |start: f64, size: f64, ms: f64, me: f64| {
            (
                size,
                Solved {
                    start,
                    size,
                    margin_start: ms,
                    margin_end: me,
                },
            )
        };
        if s.is_none() && e.is_none() && size.is_none() {
            if self.static_start {
                s = Some(self.static_edge);
            } else {
                e = Some(cb - self.static_edge);
            }
        }
        match (s, size, e) {
            (Some(s), Some(w), Some(e)) => {
                let remaining = cb - s - w - e - self.extras;
                let (ms, me) = match (self.margin_start, self.margin_end) {
                    (None, None) if self.horizontal && remaining < 0.0 => {
                        if self.keep_start {
                            (0.0, remaining)
                        } else {
                            (remaining, 0.0)
                        }
                    }
                    (None, None) => (remaining / 2.0, remaining / 2.0),
                    (None, Some(me)) => (remaining - me, me),
                    (Some(ms), None) => (ms, remaining - ms),
                    (Some(ms), Some(me)) => {
                        // Over-constrained: the dropped offset absorbs the rest.
                        return if self.keep_start {
                            done(s, w, ms, me)
                        } else {
                            done(rest(0.0, w, ms, me) - e, w, ms, me)
                        };
                    }
                };
                done(s, w, ms, me)
            }
            (start, size, end) => {
                let ms = self.margin_start.unwrap_or(0.0);
                let me = self.margin_end.unwrap_or(0.0);
                match (start, size, end) {
                    (None, None, Some(e)) => {
                        let w = auto_size(rest(0.0, 0.0, ms, me) - e);
                        done(rest(0.0, w, ms, me) - e, w, ms, me)
                    }
                    (None, Some(w), None) => {
                        if self.static_start {
                            done(self.static_edge, w, ms, me)
                        } else {
                            let e = cb - self.static_edge;
                            done(rest(0.0, w, ms, me) - e, w, ms, me)
                        }
                    }
                    (Some(s), None, None) => done(s, auto_size(rest(s, 0.0, ms, me)), ms, me),
                    (None, Some(w), Some(e)) => done(rest(0.0, w, ms, me) - e, w, ms, me),
                    (Some(s), None, Some(e)) => done(s, rest(s, 0.0, ms, me) - e, ms, me),
                    (Some(s), Some(w), None) => done(s, w, ms, me),
                    // All three auto was rewritten through the static position.
                    (None, None, None) | (Some(_), Some(_), Some(_)) => done(0.0, 0.0, ms, me),
                }
            }
        }
    }
}

/// Inputs shared by both axes of an absolutely positioned box.
pub(crate) struct AbsoluteContext {
    /// Containing block padding-box size.
    pub width: f64,
    pub height: f64,
    /// Static position of the hypothetical margin edge, relative to the
    /// containing padding box: the left edge for an LTR static-position
    /// parent, the right edge for RTL; and the top edge.
    pub static_x: f64,
    pub static_y: f64,
    pub static_direction: Direction,
    pub containing_direction: Direction,
}

/// Horizontal result plus edges for an absolutely positioned box.
pub(crate) struct AbsoluteWidth {
    pub left: f64,
    pub measure: BlockMeasure,
}

pub(crate) fn absolute_width(
    style: &ComputedStyle,
    cx: &AbsoluteContext,
    intrinsic: Intrinsic,
) -> Result<AbsoluteWidth, MeasureError> {
    let basis = cx.width;
    let (padding, border) = box_edges(style, basis)?;
    let extras = padding.left.get() + padding.right.get() + border.left.get() + border.right.get();
    let axis = Axis {
        start: auto(style, P::Left, basis)?,
        end: auto(style, P::Right, basis)?,
        margin_start: auto(style, P::MarginLeft, basis)?,
        margin_end: auto(style, P::MarginRight, basis)?,
        containing: basis,
        extras,
        static_edge: cx.static_x,
        static_start: cx.static_direction == Direction::Ltr,
        keep_start: cx.containing_direction == Direction::Ltr,
        horizontal: true,
    };
    // `fit-content` sizes into the space the non-auto offsets and margins leave.
    let available = basis
        - axis.start.unwrap_or(0.0)
        - axis.end.unwrap_or(0.0)
        - axis.margin_start.unwrap_or(0.0)
        - axis.margin_end.unwrap_or(0.0)
        - extras;
    let resolver = Resolver::new(style, Some(basis), extras, intrinsic);
    let (specified, minimum, maximum) = resolver.widths(available)?;
    let (width, solved) = constrain(specified, minimum, maximum, |w| {
        Ok(axis.solve(w, |available| intrinsic.fit(available)))
    })?;
    Ok(AbsoluteWidth {
        left: solved.start,
        measure: BlockMeasure {
            content_width: CssPx::new(width.max(0.0))?,
            padding,
            border,
            margin: Edges {
                top: CssPx::ZERO,
                right: CssPx::new(solved.margin_end)?,
                bottom: CssPx::ZERO,
                left: CssPx::new(solved.margin_start)?,
            },
        },
    })
}

/// A block-axis size against a definite basis; keywords act as the initial value.
fn block_size(style: &ComputedStyle, property: P, basis: f64) -> Result<Option<f64>, MeasureError> {
    Sizing::read(style, property)?
        .length()
        .map(|v| lp(v, basis))
        .transpose()
}

/// Vertical inputs of an absolutely positioned box, before content layout.
pub(crate) struct AbsoluteHeight {
    axis: Axis,
    specified: Option<f64>,
    minimum: f64,
    maximum: Option<f64>,
}

impl AbsoluteHeight {
    pub(crate) fn new(
        style: &ComputedStyle,
        cx: &AbsoluteContext,
        padding: Edges,
        border: Edges,
    ) -> Result<Self, MeasureError> {
        let extras =
            padding.top.get() + padding.bottom.get() + border.top.get() + border.bottom.get();
        let h = cx.height;
        let adjust = |n: f64| {
            if border_box(style) {
                (n.max(0.0) - extras).max(0.0)
            } else {
                n.max(0.0)
            }
        };
        Ok(Self {
            axis: Axis {
                start: auto(style, P::Top, h)?,
                end: auto(style, P::Bottom, h)?,
                margin_start: auto(style, P::MarginTop, cx.width)?,
                margin_end: auto(style, P::MarginBottom, cx.width)?,
                containing: h,
                extras,
                static_edge: cx.static_y,
                static_start: true,
                keep_start: true,
                horizontal: false,
            },
            specified: block_size(style, P::Height, h)?.map(adjust),
            minimum: block_size(style, P::MinHeight, h)?.map_or(0.0, adjust),
            maximum: block_size(style, P::MaxHeight, h)?.map(adjust),
        })
    }

    fn clamp(&self, n: f64) -> f64 {
        self.maximum.map_or(n, |m| n.min(m)).max(self.minimum)
    }

    /// The content height children may resolve percentages against, if it
    /// does not depend on the content (specified, or both offsets non-auto).
    pub(crate) fn definite(&self) -> Option<f64> {
        if let Some(h) = self.specified {
            return Some(self.clamp(h));
        }
        let a = &self.axis;
        match (a.start, a.end) {
            (Some(s), Some(e)) => Some(self.clamp(
                a.containing
                    - s
                    - e
                    - a.margin_start.unwrap_or(0.0)
                    - a.margin_end.unwrap_or(0.0)
                    - a.extras,
            )),
            _ => None,
        }
    }

    /// Solve with the laid-out content height for `height:auto` cases.
    pub(crate) fn solve(&self, content: f64) -> Result<Solved, MeasureError> {
        let (_, solved) = constrain(self.specified, self.minimum, self.maximum, |h| {
            Ok(self.axis.solve(h, |_| content))
        })?;
        Ok(solved)
    }
}
