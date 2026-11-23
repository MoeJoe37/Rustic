//! Float exclusions inside one block formatting context (CSS 2.2 §9.5).
//!
//! Coordinates are the formatting context's own (its root's content origin).
//! Every query charges the caller's work budget by the areas it scans.
//!
//! Float tops never decrease (rule 6), so the areas are sorted by top, and a
//! float whose bottom is at or above the latest float top can never meet a
//! query that starts at or below that top. Such queries (every placement and
//! almost every line or block that follows) scan only the *live* floats that
//! straddle the latest top, which are the floats side by side on one row,
//! instead of every earlier float (TD-44).

use crate::FlowError;
use crate::flow::Work;

/// A physical float side.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Side {
    Left,
    Right,
}

/// One placed float's margin box.
#[derive(Clone, Copy, Debug)]
struct Area {
    side: Side,
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}

impl Area {
    /// Negative-height margin boxes exclude nothing; a zero-height query is a point.
    fn overlaps(self, top: f64, bottom: f64) -> bool {
        if self.bottom <= self.top {
            return false;
        }
        if bottom > top {
            self.top < bottom && top < self.bottom
        } else {
            self.top <= top && top < self.bottom
        }
    }
}

/// The free horizontal band next to floats for a vertical range.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Band {
    pub left: f64,
    pub right: f64,
    pub intruded: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Exclusions {
    /// Every placed float, in placement order and so by non-decreasing top.
    areas: Vec<Area>,
    last_top: Option<f64>,
    /// Indexes of the areas whose bottom is below `last_top`.
    live: Vec<usize>,
}

impl Exclusions {
    pub(crate) fn is_empty(&self) -> bool {
        self.areas.is_empty()
    }

    /// The areas that can overlap `[top, bottom)`, a superset of the overlap
    /// set, after charging `work` for scanning them.
    fn candidates(
        &self,
        top: f64,
        bottom: f64,
        work: &mut Work,
    ) -> Result<impl Iterator<Item = Area> + '_, FlowError> {
        let live = self.last_top.is_some_and(|floor| top >= floor);
        let end = if live {
            0
        } else {
            // Tops are sorted: no later area can start inside the range.
            let reach = bottom.max(top);
            self.areas.partition_point(|a| a.top <= reach)
        };
        work.charge(if live { self.live.len() } else { end })?;
        let live_areas = self
            .live
            .iter()
            .take(if live { usize::MAX } else { 0 })
            .map(|&i| self.areas[i]);
        Ok(live_areas
            .chain(self.areas[..end].iter().copied())
            .filter(move |a| a.overlaps(top, bottom)))
    }

    /// Free band in `[left, right]` for `[top, bottom)`.
    pub(crate) fn band(
        &self,
        top: f64,
        bottom: f64,
        left: f64,
        right: f64,
        work: &mut Work,
    ) -> Result<Band, FlowError> {
        let (mut l, mut r) = (left, right);
        for a in self.candidates(top, bottom, work)? {
            match a.side {
                Side::Left => l = l.max(a.right),
                Side::Right => r = r.min(a.left),
            }
        }
        Ok(Band {
            left: l,
            right: r,
            intruded: l > left || r < right,
        })
    }

    /// The nearest float bottom below `top` among floats overlapping the range.
    pub(crate) fn next_bottom(
        &self,
        top: f64,
        bottom: f64,
        work: &mut Work,
    ) -> Result<Option<f64>, FlowError> {
        Ok(self
            .candidates(top, bottom, work)?
            .filter(|a| a.bottom > top)
            .map(|a| a.bottom)
            .reduce(f64::min))
    }

    /// Place a float margin box: as high, then as far to its side, as possible.
    ///
    /// The top is at least `min_top` and no higher than any earlier float
    /// (rule 6). A band that floats narrow below the box width moves it down
    /// to the next float bottom; a box wider than an unintruded containing
    /// block stays at its side's containing edge (rules 1–3, 7–9).
    #[allow(clippy::too_many_arguments, reason = "plain geometric inputs")]
    pub(crate) fn place(
        &mut self,
        side: Side,
        width: f64,
        height: f64,
        left: f64,
        right: f64,
        min_top: f64,
        work: &mut Work,
    ) -> Result<(f64, f64), FlowError> {
        let mut y = self.last_top.map_or(min_top, |t| t.max(min_top));
        let query = height.max(0.0);
        loop {
            let band = self.band(y, y + query, left, right, work)?;
            if !band.intruded || band.right - band.left >= width {
                let x = match side {
                    Side::Left => band.left,
                    Side::Right => band.right - width,
                };
                self.last_top = Some(y);
                let areas = &self.areas;
                work.charge(self.live.len())?;
                self.live.retain(|&i| areas[i].bottom > y);
                if height > 0.0 {
                    self.live.push(self.areas.len());
                }
                self.areas.push(Area {
                    side,
                    left: x,
                    right: x + width,
                    top: y,
                    bottom: y + height,
                });
                return Ok((x, y));
            }
            // An intruded band always has an overlapping float ending below y.
            y = self
                .next_bottom(y, y + query, work)?
                .ok_or(FlowError::InvalidSnapshot)?;
        }
    }

    /// The lowest bottom outer edge of floats on the selected sides.
    pub(crate) fn clearance(&self, left: bool, right: bool) -> Option<f64> {
        self.areas
            .iter()
            .filter(|a| match a.side {
                Side::Left => left,
                Side::Right => right,
            })
            .map(|a| a.bottom)
            .reduce(f64::max)
    }

    /// The lowest float bottom (for auto heights of formatting-context roots).
    pub(crate) fn lowest(&self) -> Option<f64> {
        self.clearance(true, true)
    }
}
