//! Scroll positions applied to a finished layout (CSS Overflow 3 §2.1,
//! CSSOM View "scroll position"; ADR 0064).
//!
//! Layout is computed once at scroll position zero. Scrolling the viewport
//! or a scroll container moves its contents by the negated position: every
//! fragment's shift is its containing block's shift, minus that block's
//! scroll position when it is a scroll container, starting from the
//! viewport's position at the initial containing block. Fixed boxes do not
//! move with the viewport. Sticky boxes take the offset their constraint
//! gives at their scroll container's new position. Clips are then
//! recomputed over the shifted padding boxes. None of this lays anything
//! out again, so a scroll costs one pass over the fragments.

use std::collections::HashMap;

use crate::flow::{BlockLayout, FlowError, PositionScheme};
use crate::{CssPoint, CssPx, CssRect};

/// Scroll positions: how far the viewport and scroll containers are
/// scrolled, in CSS pixels (positive towards the end edges).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScrollPositions {
    viewport: (f64, f64),
    boxes: HashMap<usize, (f64, f64)>,
}

impl ScrollPositions {
    /// Every position at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// The viewport's requested position.
    pub fn set_viewport(&mut self, x: f64, y: f64) {
        self.viewport = (x, y);
    }

    /// Scroll container `fragment`'s requested position.
    pub fn set_box(&mut self, fragment: usize, x: f64, y: f64) {
        self.boxes.insert(fragment, (x, y));
    }

    /// The viewport's requested position.
    pub fn viewport(&self) -> (f64, f64) {
        self.viewport
    }

    /// Scroll container `fragment`'s requested position.
    pub fn of_box(&self, fragment: usize) -> (f64, f64) {
        self.boxes.get(&fragment).copied().unwrap_or((0.0, 0.0))
    }
}

/// The range a scroll position can take: `min..=max` on each axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollRange {
    /// Smallest position (negative only for leftward overflow in RTL).
    pub min: (f64, f64),
    /// Largest position.
    pub max: (f64, f64),
}

impl ScrollRange {
    /// `position` clamped into the range; non-finite values become zero.
    pub fn clamp(self, (x, y): (f64, f64)) -> (f64, f64) {
        let axis = |value: f64, min: f64, max: f64| {
            if value.is_finite() {
                value.clamp(min, max.max(min))
            } else {
                0.0
            }
        };
        (
            axis(x, self.min.0, self.max.0),
            axis(y, self.min.1, self.max.1),
        )
    }
}

/// The range for a scrollport `port` over scrollable overflow `area`.
fn range(port: CssRect, area: CssRect) -> ScrollRange {
    let (px, py) = (port.origin().x.get(), port.origin().y.get());
    let (pw, ph) = (port.size().width().get(), port.size().height().get());
    let (ax, ay) = (area.origin().x.get(), area.origin().y.get());
    let (aw, ah) = (area.size().width().get(), area.size().height().get());
    ScrollRange {
        min: ((ax - px).min(0.0), (ay - py).min(0.0)),
        max: (
            ((ax + aw) - (px + pw)).max(0.0),
            ((ay + ah) - (py + ph)).max(0.0),
        ),
    }
}

fn moved(rect: CssRect, dx: f64, dy: f64) -> Result<CssRect, FlowError> {
    Ok(CssRect::new(
        CssPoint {
            x: CssPx::new(rect.origin().x.get() + dx)?,
            y: CssPx::new(rect.origin().y.get() + dy)?,
        },
        rect.size(),
    )?)
}

impl BlockLayout {
    /// The viewport's scroll range: its scrollable overflow over the
    /// initial containing block of `viewport` size.
    pub fn viewport_scroll_range(&self, viewport: crate::CssSize) -> ScrollRange {
        let origin = CssPoint {
            x: CssPx::ZERO,
            y: CssPx::ZERO,
        };
        let port = CssRect::new(origin, viewport).unwrap_or(self.viewport_scrollable_overflow());
        range(port, self.viewport_scrollable_overflow())
    }

    /// Fragment `index`'s scroll range, if it is a scroll container.
    pub fn scroll_range(&self, index: usize) -> Option<ScrollRange> {
        let fragment = self.fragments().get(index)?;
        let area = fragment.scrollable_overflow()?;
        Some(range(fragment.padding(), area))
    }

    /// This layout, which must be unscrolled (as computed), with
    /// `positions` applied: each clamped to its range, and ignored for
    /// fragments that are not scroll containers. Scrollable-overflow
    /// rectangles move with their container; sticky constraints and the
    /// viewport's scrollable overflow keep their unscrolled geometry.
    pub fn scrolled(
        &self,
        viewport: crate::CssSize,
        positions: &ScrollPositions,
    ) -> Result<Self, FlowError> {
        let fragments = self.fragments();
        let view = self
            .viewport_scroll_range(viewport)
            .clamp(positions.viewport());
        let mut scroll = Vec::with_capacity(fragments.len());
        for index in 0..fragments.len() {
            scroll.push(
                self.scroll_range(index)
                    .map_or((0.0, 0.0), |r| r.clamp(positions.of_box(index))),
            );
        }
        // Shift of each block fragment, and of the contents it holds
        // (its own scroll position subtracted).
        let mut shift: Vec<(f64, f64)> = Vec::with_capacity(fragments.len());
        let mut inner: Vec<(f64, f64)> = Vec::with_capacity(fragments.len());
        for (index, fragment) in fragments.iter().enumerate() {
            let mut base = match fragment.containing_block() {
                _ if fragment.scheme() == PositionScheme::Fixed => (0.0, 0.0),
                Some(c) if c < index => inner[c],
                Some(_) => return Err(FlowError::InvalidSnapshot),
                None => (-view.0, -view.1),
            };
            if let Some(constraint) = fragment.sticky() {
                let at = match constraint.scroll_container() {
                    Some(c) => scroll.get(c).copied().unwrap_or((0.0, 0.0)),
                    None => view,
                };
                let offset = constraint.offset(CssPoint {
                    x: CssPx::new(at.0)?,
                    y: CssPx::new(at.1)?,
                })?;
                let applied = fragment.relative_offset();
                base.0 += offset.x.get() - applied.x.get();
                base.1 += offset.y.get() - applied.y.get();
            }
            shift.push(base);
            inner.push((base.0 - scroll[index].0, base.1 - scroll[index].1));
        }
        let mut out = self.clone();
        for (index, fragment) in out.fragments_mut().iter_mut().enumerate() {
            let (dx, dy) = shift[index];
            fragment.translate(dx, dy)?;
        }
        let line_shift: Vec<(f64, f64)> = out
            .lines()
            .iter()
            .map(|line| inner.get(line.block()).copied().unwrap_or((0.0, 0.0)))
            .collect();
        for (line, &(dx, dy)) in out.lines_mut().iter_mut().zip(&line_shift) {
            line.translate(dx, dy)?;
        }
        for text in out.texts_mut() {
            let (dx, dy) = line_shift.get(text.line()).copied().unwrap_or((0.0, 0.0));
            text.translate(dx, dy)?;
        }
        for inline in out.inlines_mut() {
            let (dx, dy) = line_shift.get(inline.line()).copied().unwrap_or((0.0, 0.0));
            inline.translate(dx, dy)?;
        }
        crate::overflow::reclip(out.fragments_mut());
        Ok(out)
    }
}

/// Move `rect` by `(dx, dy)`, as fragments do when they scroll.
pub(crate) fn translate_rect(rect: CssRect, dx: f64, dy: f64) -> Result<CssRect, FlowError> {
    moved(rect, dx, dy)
}

fn shifted(value: CssPx, by: f64) -> Result<CssPx, FlowError> {
    Ok(CssPx::new(value.get() + by)?)
}

impl crate::LineFragment {
    pub(crate) fn translate(&mut self, dx: f64, dy: f64) -> Result<(), FlowError> {
        self.rect = translate_rect(self.rect, dx, dy)?;
        self.baseline = shifted(self.baseline, dy)?;
        Ok(())
    }
}

impl crate::TextFragment {
    pub(crate) fn translate(&mut self, dx: f64, dy: f64) -> Result<(), FlowError> {
        self.rect = translate_rect(self.rect, dx, dy)?;
        self.baseline = shifted(self.baseline, dy)?;
        for glyph in &mut self.glyphs {
            glyph.x += dx;
            glyph.y += dy;
        }
        Ok(())
    }
}

impl crate::InlineFragment {
    pub(crate) fn translate(&mut self, dx: f64, dy: f64) -> Result<(), FlowError> {
        self.border = translate_rect(self.border, dx, dy)?;
        self.padding = translate_rect(self.padding, dx, dy)?;
        self.content = translate_rect(self.content, dx, dy)?;
        Ok(())
    }
}

impl crate::BlockFragment {
    pub(crate) fn translate(&mut self, dx: f64, dy: f64) -> Result<(), FlowError> {
        if dx == 0.0 && dy == 0.0 {
            return Ok(());
        }
        self.content = translate_rect(self.content, dx, dy)?;
        self.padding = translate_rect(self.padding, dx, dy)?;
        self.border = translate_rect(self.border, dx, dy)?;
        if let Some(area) = self.scrollable {
            self.scrollable = Some(translate_rect(area, dx, dy)?);
        }
        Ok(())
    }
}
