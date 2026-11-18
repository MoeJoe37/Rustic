//! CSSOM View rectangles from the same transforms and fragments as paint.

use std::collections::HashMap;

use rustic_base::CancellationToken;
use rustic_dom::NodeId;
use rustic_layout::{BlockLayout, BoxTree, CssRect};

use crate::display::PaintError;
use crate::matrix::{BoxTransform, Homography, box_transform, perspective_of};

/// Map CSSOM View elements to `[x, y, width, height]` in CSS pixels.
/// An unprojectable rectangle is recorded as `None`.
pub type ClientRects = HashMap<NodeId, Option<[f64; 4]>>;

/// Compute transformed client bounding rectangles for elements with fragments.
/// Clipping and visibility affect painting independently of fragment geometry.
pub fn client_rects(
    tree: &BoxTree,
    layout: &BlockLayout,
    cancellation: &CancellationToken,
) -> Result<ClientRects, PaintError> {
    let boxes = tree.boxes();
    let fragments = layout.fragments();
    let mut transforms = Vec::<Option<Homography>>::with_capacity(fragments.len());
    let mut rects = HashMap::new();
    let style = |index: usize| {
        boxes
            .get(index)
            .and_then(|source| source.style())
            .ok_or(PaintError::InvalidInput)
    };
    for fragment in fragments {
        cancellation.check().map_err(|_| PaintError::Cancelled)?;
        let perspective = fragment
            .containing_block()
            .map(|index| {
                let containing = fragments.get(index).ok_or(PaintError::InvalidInput)?;
                perspective_of(
                    style(containing.source_box())?,
                    coordinates(containing.border()),
                )
                .map_err(|_| PaintError::InvalidInput)
            })
            .transpose()?
            .flatten();
        let own = box_transform(
            style(fragment.source_box())?,
            coordinates(fragment.border()),
            perspective,
        )
        .map_err(|_| PaintError::InvalidInput)?;
        let parent = match fragment.parent() {
            Some(index) => *transforms.get(index).ok_or(PaintError::InvalidInput)?,
            None => Some(Homography::IDENTITY),
        };
        let transform = match own {
            BoxTransform::None => parent,
            BoxTransform::Matrix(matrix) => parent.map(|parent| parent.then_after(&matrix.plane())),
            BoxTransform::Degenerate => None,
        };
        include(&mut rects, fragment.node(), fragment.border(), transform);
        transforms.push(transform);
    }
    for fragment in layout.inlines() {
        cancellation.check().map_err(|_| PaintError::Cancelled)?;
        let block = layout
            .lines()
            .get(fragment.line())
            .ok_or(PaintError::InvalidInput)?
            .block();
        let transform = *transforms.get(block).ok_or(PaintError::InvalidInput)?;
        include(&mut rects, fragment.node(), fragment.border(), transform);
    }
    Ok(rects
        .into_iter()
        .map(|(node, group)| (node, group.map(BoundingGroup::finish)))
        .collect())
}

fn coordinates(rect: CssRect) -> (f64, f64, f64, f64) {
    (
        rect.origin().x.get(),
        rect.origin().y.get(),
        rect.size().width().get(),
        rect.size().height().get(),
    )
}

#[derive(Clone, Copy)]
struct BoundingGroup {
    first: [f64; 4],
    bounds: Option<[f64; 4]>,
    has_area: bool,
}

impl BoundingGroup {
    fn new(first: [f64; 4]) -> Self {
        Self {
            first,
            bounds: None,
            has_area: false,
        }
    }

    fn include(mut self, next: [f64; 4]) -> Self {
        self.has_area |= next[2] != 0.0 && next[3] != 0.0;
        if next[2] != 0.0 || next[3] != 0.0 {
            self.bounds = Some(match self.bounds {
                Some(first) => {
                    let x = first[0].min(next[0]);
                    let y = first[1].min(next[1]);
                    let right = (first[0] + first[2]).max(next[0] + next[2]);
                    let bottom = (first[1] + first[3]).max(next[1] + next[3]);
                    [x, y, right - x, bottom - y]
                }
                None => next,
            });
        }
        self
    }

    fn finish(self) -> [f64; 4] {
        if self.has_area {
            self.bounds
                .expect("nonempty rectangle contributes to bounds")
        } else {
            self.first
        }
    }
}

fn include(
    rects: &mut HashMap<NodeId, Option<BoundingGroup>>,
    node: NodeId,
    rect: CssRect,
    transform: Option<Homography>,
) {
    let rect = transform.and_then(|transform| project(transform, rect));
    let existing = rects
        .entry(node)
        .or_insert_with(|| rect.map(BoundingGroup::new));
    *existing = existing.zip(rect).map(|(group, next)| group.include(next));
}

fn project(transform: Homography, rect: CssRect) -> Option<[f64; 4]> {
    let (x, y, w, h) = coordinates(rect);
    let (mut left, mut top) = transform.apply(x, y)?;
    if !left.is_finite() || !top.is_finite() {
        return None;
    }
    let (mut right, mut bottom) = (left, top);
    for (x, y) in [(x + w, y), (x + w, y + h), (x, y + h)] {
        let (x, y) = transform.apply(x, y)?;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }
    let transformed = [left, top, right - left, bottom - top];
    transformed
        .iter()
        .all(|coord| coord.is_finite())
        .then_some(transformed)
}
