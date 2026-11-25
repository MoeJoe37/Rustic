//! Scroll and view progress timelines (Scroll-driven Animations, editor's
//! draft reviewed 2026-11-25): measuring each timeline the animation
//! controller uses from layout geometry and scroll positions, once per
//! rendering opportunity before animations update.
//!
//! Positions come from unscrolled client rectangles. The scrollport is
//! taken as the scroll container's border box and logical axes resolve as
//! `horizontal-tb` (TD-217).

use rustic_css::{LengthPercentageOrAuto, ScrollAxis};
use rustic_dom::NodeId;
use rustic_style::{ScrollTimelineState, ScrollerRef, TimelineSource, ViewGeometry, length_px};

use crate::layout_reads::LayoutGeometry;
use crate::scripting::ScrollState;

/// The coordinate index (0 for x, 1 for y) of an axis.
fn axis_index(axis: ScrollAxis) -> usize {
    match axis {
        ScrollAxis::Block | ScrollAxis::Y => 1,
        ScrollAxis::Inline | ScrollAxis::X => 0,
    }
}

/// The nearest scroll container of `node` that moves it: `Some(None)` for
/// the viewport.
fn nearest(geometry: &LayoutGeometry, node: NodeId) -> Option<Option<NodeId>> {
    match geometry.scroll_chains.get(&node) {
        Some((_, containers)) if !containers.is_empty() => Some(containers.last().copied()),
        Some((true, _)) => Some(None),
        _ => None,
    }
}

fn rect(geometry: &LayoutGeometry, node: NodeId) -> Option<[f64; 4]> {
    geometry.client_rects.get(&node).copied().flatten()
}

/// The state of `source` now.
pub(crate) fn measure(
    geometry: &LayoutGeometry,
    scroll: &ScrollState,
    source: &TimelineSource,
) -> ScrollTimelineState {
    measure_active(geometry, scroll, source).unwrap_or_default()
}

fn measure_active(
    geometry: &LayoutGeometry,
    scroll: &ScrollState,
    source: &TimelineSource,
) -> Option<ScrollTimelineState> {
    let (scroller, axis) = match source {
        TimelineSource::Scroll { scroller, axis } => (
            match scroller {
                ScrollerRef::Element(node) => Some(*node),
                ScrollerRef::Root => None,
                ScrollerRef::NearestTo(node) => nearest(geometry, *node)?,
            },
            *axis,
        ),
        TimelineSource::View { subject, axis, .. } => (nearest(geometry, *subject)?, *axis),
    };
    let i = axis_index(axis);
    let (range, offset, origin, port) = match scroller {
        Some(node) => {
            let range = *geometry.scroll_ranges.get(&node)?;
            let offset = scroll.elements.get(&node).copied().unwrap_or_default();
            let r = rect(geometry, node)?;
            (range, offset, r[i], r[i + 2])
        }
        None => (
            geometry.viewport_scroll_range,
            scroll.viewport,
            0.0,
            geometry.viewport_size[i],
        ),
    };
    let (min, max) = (range[i], range[i + 2]);
    let offset = if i == 0 { offset.0 } else { offset.1 };
    match source {
        TimelineSource::Scroll { .. } => (max > min).then_some(ScrollTimelineState {
            offset: Some(offset),
            start: min,
            end: max,
            view: None,
        }),
        TimelineSource::View { subject, inset, .. } => {
            let r = rect(geometry, *subject)?;
            let px = |side: &LengthPercentageOrAuto| match side {
                LengthPercentageOrAuto::Auto => 0.0,
                LengthPercentageOrAuto::LengthPercentage(lp) => length_px(lp, port),
            };
            let (inset_start, inset_end) = (px(&inset.0), px(&inset.1));
            let subject_start = r[i] - origin - inset_start;
            let subject_size = r[i + 2];
            let port = port - inset_start - inset_end;
            let start = subject_start - port;
            let end = subject_start + subject_size;
            (end > start).then_some(ScrollTimelineState {
                offset: Some(offset),
                start,
                end,
                view: Some(ViewGeometry {
                    subject_start,
                    subject_size,
                    port,
                }),
            })
        }
    }
}
