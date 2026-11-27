//! Multi-column layout (CSS Multicol 1) by measuring and moving.
//!
//! The box tree gives every multi-column container an anonymous block
//! (its *column wrapper*) holding its content. Layout then runs up to
//! three times:
//!
//! 1. as is, to learn each container's content width, which gives the
//!    used column count and width (§3.4);
//! 2. with each wrapper as wide as one column, to learn where the content
//!    can break: between line boxes and between monolithic blocks;
//! 3. with each wrapper also as tall as the columns: the balanced height
//!    (the shortest that fits the content into the count, §7) or, for a
//!    container with a definite height, that height (overflow columns
//!    continue in the inline direction).
//!
//! The content of the last pass is then moved into place column by
//! column: whatever starts in column `k` moves right by `k` column pitches
//! and up by the column's start. Containers nested in a column wrapper
//! lay out as single columns, `column-span`, column rules and fragmented
//! box decorations (a block split between columns keeps its whole
//! background in its first column) are not supported (TD-221).

use std::sync::Arc;

use rustic_css::{
    Gap, LengthPercentage, LengthPercentageOrAuto, PropertyId as P, PropertyValue as V, Size,
    ZIndex,
};
use rustic_style::ComputedStyle;

use super::{BlockLayout, FlowError};
use crate::{BoxKind, BoxTree};

/// Whether any box is a column wrapper.
pub(super) fn needed(tree: &BoxTree) -> bool {
    tree.boxes().iter().any(|b| b.is_column_wrapper())
}

/// The used geometry of one multi-column container.
#[derive(Clone, Debug)]
struct Columns {
    /// The wrapper's box index.
    wrapper: usize,
    width: f64,
    gap: f64,
    count: usize,
    /// Fixed column height from a definite container height.
    fixed: Option<f64>,
}

const EPSILON: f64 = 1e-6;

/// Lay out `tree` (which has column wrappers) with `inner`, the layout
/// without multi-column support.
pub(super) fn layout(
    tree: &BoxTree,
    inner: &dyn Fn(&BoxTree) -> Result<BlockLayout, FlowError>,
) -> Result<BlockLayout, FlowError> {
    let boxes = tree.boxes();
    // Only outermost wrappers make columns.
    let wrappers: Vec<usize> = (0..boxes.len())
        .filter(|&i| boxes[i].is_column_wrapper())
        .filter(|&i| {
            let mut p = boxes[i].parent();
            while let Some(q) = p {
                if boxes[q].is_column_wrapper() {
                    return false;
                }
                p = boxes[q].parent();
            }
            true
        })
        .collect();
    let first = inner(tree)?;
    let mut columns = Vec::new();
    for &w in &wrappers {
        let Some(container) = boxes[w].parent() else {
            continue;
        };
        let Some(fragment) = first.fragments.iter().find(|f| f.source_box == container) else {
            continue;
        };
        let style = boxes[container].style().ok_or(FlowError::InvalidSnapshot)?;
        let available = fragment.content.size().width().get();
        let definite = matches!(
            style.get(P::Height),
            V::Size(Size::LengthPercentage(LengthPercentage::Length(_)))
        );
        let (count, width, gap) = used_columns(style, available);
        columns.push(Columns {
            wrapper: w,
            width,
            gap,
            count,
            fixed: definite.then(|| fragment.content.size().height().get()),
        });
    }
    if columns.is_empty() {
        return Ok(first);
    }
    let measured = inner(&with_wrappers(tree, &columns, &[])?)?;
    let mut heights = Vec::with_capacity(columns.len());
    for c in &columns {
        let pieces = pieces(&measured, c.wrapper);
        let total = wrapper_height(&measured, c.wrapper);
        heights.push(match c.fixed {
            Some(h) => h.max(0.0),
            None => balance(&pieces, total, c.count),
        });
    }
    let mut out = inner(&with_wrappers(tree, &columns, &heights)?)?;
    for (c, &height) in columns.iter().zip(&heights) {
        columnize(&mut out, c, height)?;
    }
    crate::overflow::reclip(out.fragments_mut());
    Ok(out)
}

/// The used column count, width and gap for `available` content width
/// (CSS Multicol 1 §3.4).
fn used_columns(style: &ComputedStyle, available: f64) -> (usize, f64, f64) {
    let font_size = style.font_size_px();
    let gap = match style.get(P::ColumnGap) {
        V::Gap(Gap::LengthPercentage(LengthPercentage::Length(l))) => l.to_px().unwrap_or(0.0),
        V::Gap(Gap::LengthPercentage(LengthPercentage::Percentage(p))) => available * p / 100.0,
        _ => font_size,
    }
    .max(0.0);
    let count = match style.get(P::ColumnCount) {
        V::ZIndex(ZIndex::Integer(n)) => Some((*n).max(1) as usize),
        _ => None,
    };
    let width = match style.get(P::ColumnWidth) {
        V::LengthPercentageOrAuto(LengthPercentageOrAuto::LengthPercentage(
            LengthPercentage::Length(l),
        )) => l.to_px().map(|w| w.max(1.0)),
        _ => None,
    };
    let available = available.max(0.0);
    let fit = |w: f64| (((available + gap) / (w + gap)).floor() as usize).max(1);
    let count = match (count, width) {
        (Some(n), None) => n,
        (None, Some(w)) => fit(w),
        (Some(n), Some(w)) => n.min(fit(w)),
        (None, None) => 1,
    }
    .min(1000);
    let width = ((available + gap) / count as f64 - gap).max(0.0);
    (count, width, gap)
}

/// `tree` with each wrapper as wide as its columns and, when `heights`
/// is given, as tall as them.
fn with_wrappers(
    tree: &BoxTree,
    columns: &[Columns],
    heights: &[f64],
) -> Result<BoxTree, FlowError> {
    let mut boxes = tree.boxes().to_vec();
    for (index, c) in columns.iter().enumerate() {
        let b = &boxes[c.wrapper];
        let mut style = b.style().ok_or(FlowError::InvalidSnapshot)?.clone();
        style.set(P::Width, V::Size(Size::LengthPercentage(px(c.width))));
        if let Some(&h) = heights.get(index) {
            style.set(P::Height, V::Size(Size::LengthPercentage(px(h))));
        }
        let derived = b.derived(
            b.parent(),
            Some(Arc::new(style)),
            BoxKind::Element(rustic_css::Display::Block),
            false,
        );
        boxes[c.wrapper] = derived;
    }
    Ok(BoxTree::from_boxes(boxes))
}

fn px(v: f64) -> LengthPercentage {
    LengthPercentage::Length(rustic_css::Length::px(v))
}

/// The wrapper's fragment index in `layout`.
fn wrapper_fragment(layout: &BlockLayout, wrapper: usize) -> Option<usize> {
    layout
        .fragments
        .iter()
        .position(|f| f.source_box == wrapper)
}

fn wrapper_height(layout: &BlockLayout, wrapper: usize) -> f64 {
    wrapper_fragment(layout, wrapper)
        .map_or(0.0, |i| layout.fragments[i].content.size().height().get())
}

/// Whether fragment `i` is a descendant of fragment `root`.
fn inside(layout: &BlockLayout, mut i: usize, root: usize) -> bool {
    while let Some(p) = layout.fragments[i].parent {
        if p == root {
            return true;
        }
        i = p;
    }
    false
}

/// The unbreakable pieces of a wrapper's content, as (top, bottom)
/// relative to its content box, in order: its line boxes and the blocks
/// that hold no lines or blocks of their own.
fn pieces(layout: &BlockLayout, wrapper: usize) -> Vec<(f64, f64)> {
    let Some(root) = wrapper_fragment(layout, wrapper) else {
        return Vec::new();
    };
    let top = layout.fragments[root].content.origin().y.get();
    let mut has_children = vec![false; layout.fragments.len()];
    for f in &layout.fragments {
        if let Some(p) = f.parent {
            has_children[p] = true;
        }
    }
    for line in &layout.lines {
        has_children[line.block()] = true;
    }
    let mut out: Vec<(f64, f64)> = Vec::new();
    for line in &layout.lines {
        let b = line.block();
        if b == root || inside(layout, b, root) {
            let r = line.rect();
            let y = r.origin().y.get() - top;
            out.push((y, y + r.size().height().get()));
        }
    }
    for (i, f) in layout.fragments.iter().enumerate() {
        if !has_children[i] && f.line.is_none() && inside(layout, i, root) {
            let y = f.border.origin().y.get() - top;
            out.push((y, y + f.border.size().height().get()));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// Where each column starts for columns of `height`: a piece that would
/// cross a column's end starts the next column (unless it already starts
/// its column).
fn starts(pieces: &[(f64, f64)], height: f64) -> Vec<f64> {
    let mut starts = vec![0.0];
    for &(top, bottom) in pieces {
        let start = *starts.last().unwrap_or(&0.0);
        if bottom > start + height + EPSILON && top > start + EPSILON {
            // A piece wholly past the end may leave a gap: the column
            // ends there and the next one starts at the piece.
            starts.push(top);
        }
    }
    starts
}

/// The balanced column height: the shortest that fits the pieces into
/// `count` columns (or the whole content, when nothing can).
fn balance(pieces: &[(f64, f64)], total: f64, count: usize) -> f64 {
    if count <= 1 || pieces.is_empty() {
        return total;
    }
    let tallest = pieces.iter().map(|p| p.1 - p.0).fold(0.0, f64::max);
    let mut low = (total / count as f64).max(tallest);
    let mut high = total;
    if starts(pieces, low).len() <= count {
        return low;
    }
    for _ in 0..40 {
        let mid = (low + high) / 2.0;
        if starts(pieces, mid).len() <= count {
            high = mid;
        } else {
            low = mid;
        }
        if high - low < 0.01 {
            break;
        }
    }
    // The tight end of the last column: the bottom of its last piece.
    let start = starts(pieces, high);
    let fitted = start
        .iter()
        .enumerate()
        .map(|(k, &s)| {
            let end = start.get(k + 1).copied().unwrap_or(f64::INFINITY);
            pieces
                .iter()
                .filter(|p| p.0 >= s - EPSILON && p.0 < end - EPSILON)
                .map(|p| p.1 - s)
                .fold(0.0, f64::max)
        })
        .fold(0.0, f64::max);
    fitted.max(low.min(high)).min(high)
}

/// Move the wrapper's content into its columns.
fn columnize(layout: &mut BlockLayout, c: &Columns, height: f64) -> Result<(), FlowError> {
    let Some(root) = wrapper_fragment(layout, c.wrapper) else {
        return Ok(());
    };
    let pieces = pieces(layout, c.wrapper);
    let start = starts(&pieces, height);
    if start.len() <= 1 {
        return Ok(());
    }
    let top = layout.fragments[root].content.origin().y.get();
    let pitch = c.width + c.gap;
    let column = |y: f64| {
        let local = y - top;
        start
            .iter()
            .rposition(|&s| s <= local + EPSILON)
            .unwrap_or(0)
    };
    let shift = |k: usize| (pitch * k as f64, -start[k]);
    // Lines first: atomic inlines and texts follow their line.
    let line_column: Vec<Option<usize>> = layout
        .lines
        .iter()
        .map(|line| {
            let b = line.block();
            (b == root || inside(layout, b, root)).then(|| column(line.rect().origin().y.get()))
        })
        .collect();
    let n = layout.fragments.len();
    let mut fragment_column: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        if !inside(layout, i, root) {
            continue;
        }
        let f = &layout.fragments[i];
        // A fragment inside an atomic inline moves with it.
        let parent = f
            .parent
            .and_then(|p| (p != root).then(|| fragment_column[p]).flatten());
        let own = match f.line {
            Some(l) => line_column.get(l).copied().flatten(),
            None => None,
        };
        let atomic_ancestor = {
            let mut p = f.parent;
            let mut found = None;
            while let Some(q) = p {
                if q == root {
                    break;
                }
                if layout.fragments[q].line.is_some() {
                    found = fragment_column[q];
                    break;
                }
                p = layout.fragments[q].parent;
            }
            found
        };
        fragment_column[i] = Some(
            own.or(atomic_ancestor)
                .unwrap_or_else(|| column(f.border.origin().y.get())),
        );
        let _ = parent;
    }
    for (i, k) in fragment_column.iter().enumerate() {
        if let Some(k) = *k {
            let (dx, dy) = shift(k);
            layout.fragments[i].translate(dx, dy)?;
        }
    }
    for (l, k) in line_column.iter().enumerate() {
        if let Some(k) = *k {
            let (dx, dy) = shift(k);
            layout.lines[l].translate(dx, dy)?;
        }
    }
    for text in &mut layout.texts {
        if let Some(Some(k)) = line_column.get(text.line()) {
            let (dx, dy) = shift(*k);
            text.translate(dx, dy)?;
        }
    }
    for inline in &mut layout.inlines {
        if let Some(Some(k)) = line_column.get(inline.line()) {
            let (dx, dy) = shift(*k);
            inline.translate(dx, dy)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balancing_finds_the_shortest_height_that_fits() {
        // Six 10px lines in three columns: 20px each.
        let lines: Vec<(f64, f64)> = (0..6)
            .map(|i| (i as f64 * 10.0, i as f64 * 10.0 + 10.0))
            .collect();
        let h = balance(&lines, 60.0, 3);
        assert!((h - 20.0).abs() < 0.02, "{h}");
        assert_eq!(starts(&lines, h).len(), 3);
        // A tall piece sets the minimum.
        let tall = [(0.0, 50.0), (50.0, 60.0)];
        assert!((balance(&tall, 60.0, 2) - 50.0).abs() < 0.02);
    }

    #[test]
    fn a_piece_crossing_the_end_starts_the_next_column() {
        let pieces = [(0.0, 10.0), (12.0, 22.0), (25.0, 35.0)];
        assert_eq!(starts(&pieces, 25.0), [0.0, 25.0]);
        assert_eq!(starts(&pieces, 10.0), [0.0, 12.0, 25.0]);
    }
}
