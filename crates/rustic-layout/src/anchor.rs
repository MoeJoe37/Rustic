//! Anchor positioning (CSS Anchor Positioning Level 1, editor's draft
//! reviewed 2026-11-25) for absolutely positioned boxes: `anchor()` insets,
//! `anchor-size()` sizes and `position-area`.
//!
//! The flow engine finds the anchor box and passes its border box, in the
//! containing block's padding-box space, to [`resolve`], which returns a
//! copy of the positioned box's style with the anchor functions replaced by
//! pixel lengths and, for `position-area`, the grid region that becomes the
//! containing block. Limits are TD-216.

use rustic_css::{
    AnchorFunction, AnchorSide, AnchorSizeKeyword, AnchorValue, AreaSpan, Direction, Length,
    LengthPercentage, LengthPercentageOrAuto, MaxSize, PositionArea, PropertyId as P,
    PropertyValue as V, Size,
};
use rustic_style::ComputedStyle;

/// A rectangle in a containing block's padding-box space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Alignment of a box inside its `position-area` region in one axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AreaAlign {
    Start,
    Center,
    End,
}

/// The `position-area` region that replaces the containing block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Region {
    pub rect: Rect,
    /// Center the box in the axis after layout (both insets were `auto`).
    pub center_x: bool,
    pub center_y: bool,
}

/// Whether `style` uses anchor positioning at all.
pub(crate) fn uses_anchors(style: &ComputedStyle) -> bool {
    style.has_anchor_functions() || area(style).is_some()
}

/// Whether the box with `style` is an anchor named `name`.
pub(crate) fn is_named(style: &ComputedStyle, name: &str) -> bool {
    matches!(style.get(P::AnchorName), V::Anchor(AnchorValue::Names(names)) if names.iter().any(|n| &**n == name))
}

const ANCHORED: [P; 10] = [
    P::Top,
    P::Right,
    P::Bottom,
    P::Left,
    P::Width,
    P::Height,
    P::MinWidth,
    P::MinHeight,
    P::MaxWidth,
    P::MaxHeight,
];

fn default_anchor(style: &ComputedStyle) -> Option<&str> {
    match style.get(P::PositionAnchor) {
        V::Anchor(AnchorValue::Default(Some(name))) => Some(name),
        _ => None,
    }
}

fn area(style: &ComputedStyle) -> Option<PositionArea> {
    match style.get(P::PositionArea) {
        V::Anchor(AnchorValue::Area(area)) => *area,
        _ => None,
    }
}

/// Containing block inputs for resolution.
pub(crate) struct Containing {
    pub width: f64,
    pub height: f64,
    pub direction: Direction,
}

/// Substitute the anchor functions of `style` (`lookup` gives the border
/// box of a named anchor) and apply `position-area`. Returns `None` when
/// nothing changes.
pub(crate) fn resolve(
    style: &ComputedStyle,
    cb: &Containing,
    mut lookup: impl FnMut(&str) -> Option<Rect>,
) -> Option<(ComputedStyle, Option<Region>)> {
    if !uses_anchors(style) {
        return None;
    }
    let mut out = style.clone();
    let default = default_anchor(style);
    let self_direction = match style.get(P::Direction) {
        V::Direction(d) => *d,
        _ => Direction::Ltr,
    };
    for property in ANCHORED {
        let Some(function) = style.anchor_function(property) else {
            continue;
        };
        let Some(rect) = function
            .name()
            .map(|n| &**n)
            .or(default)
            .and_then(&mut lookup)
        else {
            continue;
        };
        let px = match function {
            AnchorFunction::Edge { side, .. } => edge(property, *side, rect, cb, self_direction)
                .map(|px| match property {
                    P::Top | P::Left => px,
                    P::Bottom => cb.height - px,
                    _ => cb.width - px,
                }),
            AnchorFunction::Size { size, .. } => Some(match size {
                Some(AnchorSizeKeyword::Width)
                | Some(AnchorSizeKeyword::Inline)
                | Some(AnchorSizeKeyword::SelfInline) => rect.width,
                Some(_) => rect.height,
                None if matches!(property, P::Width | P::MinWidth | P::MaxWidth) => rect.width,
                None if matches!(property, P::Height | P::MinHeight | P::MaxHeight) => rect.height,
                // anchor-size() in an inset uses that inset's axis.
                None if matches!(property, P::Left | P::Right) => rect.width,
                None => rect.height,
            }),
        };
        let Some(px) = px.filter(|px| px.is_finite()) else {
            continue;
        };
        let lp = LengthPercentage::Length(Length::px(px));
        let value = match property {
            P::Top | P::Right | P::Bottom | P::Left => {
                V::LengthPercentageOrAuto(LengthPercentageOrAuto::LengthPercentage(lp))
            }
            P::MaxWidth | P::MaxHeight => V::MaxSize(MaxSize::LengthPercentage(
                LengthPercentage::Length(Length::px(px.max(0.0))),
            )),
            _ => V::Size(Size::LengthPercentage(LengthPercentage::Length(
                Length::px(px.max(0.0)),
            ))),
        };
        out.set(property, value);
    }
    let region = match (area(style), default.and_then(&mut lookup)) {
        (Some(area), Some(anchor)) => Some(apply_area(&mut out, area, anchor, cb)),
        _ => None,
    };
    Some((out, region))
}

/// The coordinate of an anchor edge along the axis of `property`.
fn edge(
    property: P,
    side: AnchorSide,
    rect: Rect,
    cb: &Containing,
    self_direction: Direction,
) -> Option<f64> {
    let vertical = matches!(property, P::Top | P::Bottom);
    let (start, size) = if vertical {
        (rect.y, rect.height)
    } else {
        (rect.x, rect.width)
    };
    let end = start + size;
    // Inline-axis logical sides follow the direction (block axis is
    // always top to bottom: TD-216).
    let flipped = |direction: Direction| !vertical && direction == Direction::Rtl;
    let at_start = matches!(property, P::Top | P::Left);
    Some(match side {
        AnchorSide::Top | AnchorSide::Bottom if !vertical => return None,
        AnchorSide::Left | AnchorSide::Right if vertical => return None,
        AnchorSide::Top | AnchorSide::Left => start,
        AnchorSide::Bottom | AnchorSide::Right => end,
        AnchorSide::Start => {
            if flipped(cb.direction) {
                end
            } else {
                start
            }
        }
        AnchorSide::End => {
            if flipped(cb.direction) {
                start
            } else {
                end
            }
        }
        AnchorSide::SelfStart => {
            if flipped(self_direction) {
                end
            } else {
                start
            }
        }
        AnchorSide::SelfEnd => {
            if flipped(self_direction) {
                start
            } else {
                end
            }
        }
        AnchorSide::Center => start + size / 2.0,
        AnchorSide::Inside => {
            if at_start {
                start
            } else {
                end
            }
        }
        AnchorSide::Outside => {
            if at_start {
                end
            } else {
                start
            }
        }
        AnchorSide::Percentage(p) => {
            if flipped(cb.direction) {
                end - size * p / 100.0
            } else {
                start + size * p / 100.0
            }
        }
    })
}

fn span(cells: AreaSpan, anchor_start: f64, anchor_end: f64, total: f64) -> (f64, f64) {
    let a0 = anchor_start.clamp(0.0, total.max(0.0));
    let a1 = anchor_end.clamp(a0, total.max(a0));
    let from = if cells.start {
        0.0
    } else if cells.center {
        a0
    } else {
        a1
    };
    let to = if cells.end {
        total.max(a1)
    } else if cells.center {
        a1
    } else {
        a0
    };
    (from, to.max(from))
}

fn align(cells: AreaSpan) -> AreaAlign {
    match (cells.start, cells.end) {
        (true, false) => AreaAlign::End,
        (false, true) => AreaAlign::Start,
        _ => AreaAlign::Center,
    }
}

/// Set auto insets for the region's alignment and return the region.
fn apply_area(
    style: &mut ComputedStyle,
    area: PositionArea,
    anchor: Rect,
    cb: &Containing,
) -> Region {
    let (x0, x1) = span(area.x, anchor.x, anchor.x + anchor.width, cb.width);
    let (y0, y1) = span(area.y, anchor.y, anchor.y + anchor.height, cb.height);
    let zero = V::LengthPercentageOrAuto(LengthPercentageOrAuto::LengthPercentage(
        LengthPercentage::ZERO,
    ));
    let auto = |s: &ComputedStyle, p: P| {
        matches!(
            s.get(p),
            V::LengthPercentageOrAuto(LengthPercentageOrAuto::Auto)
        )
    };
    let mut center = [false; 2];
    for (axis, (cells, (start, end))) in
        [(area.x, (P::Left, P::Right)), (area.y, (P::Top, P::Bottom))]
            .into_iter()
            .enumerate()
    {
        if !(auto(style, start) && auto(style, end)) {
            continue;
        }
        match align(cells) {
            AreaAlign::End => style.set(end, zero.clone()),
            AreaAlign::Start => style.set(start, zero.clone()),
            AreaAlign::Center => {
                style.set(start, zero.clone());
                center[axis] = true;
            }
        }
    }
    Region {
        rect: Rect {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        },
        center_x: center[0],
        center_y: center[1],
    }
}
