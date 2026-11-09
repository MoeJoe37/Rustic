//! Sticky positioning (CSS Positioned Layout 3 §3.4, reviewed 2026-11-09).
//!
//! A sticky box is laid out in normal flow. Its offset depends on where its
//! nearest scrollport is scrolled to, so layout records the inputs as a
//! [`StickyConstraint`] and applies the offset for the initial scroll
//! position; an embedder that scrolls asks the constraint for the offset at
//! another scroll position.

use crate::{CssPoint, CssPx, CssRect, GeometryError};

/// One axis of a sticky box's geometry, all in document coordinates.
struct Axis {
    /// Border-box start and end before the sticky offset.
    start: f64,
    end: f64,
    /// Margins outside them.
    margin_start: f64,
    margin_end: f64,
    /// The containing block's content-box start and end.
    cb_start: f64,
    cb_end: f64,
    /// Sticky view rectangle edges, for non-`auto` insets.
    view_start: Option<f64>,
    view_end: Option<f64>,
}

impl Axis {
    /// The shift that keeps the box inside the view rectangle while its
    /// margin box stays inside the containing block. The start inset wins
    /// when both apply (the box is larger than the view).
    fn offset(&self) -> f64 {
        let from_start = match self.view_start {
            Some(v) if self.start < v => {
                let room = self.cb_end - (self.end + self.margin_end);
                (v - self.start).min(room).max(0.0)
            }
            _ => 0.0,
        };
        let from_end = match self.view_end {
            Some(v) if self.end > v => {
                let room = self.cb_start - (self.start - self.margin_start);
                (v - self.end).max(room).min(0.0)
            }
            _ => 0.0,
        };
        if from_start > 0.0 {
            from_start
        } else {
            from_end
        }
    }
}

/// Everything needed to compute a sticky box's offset at any scroll position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StickyConstraint {
    pub(crate) scroll_container: Option<usize>,
    pub(crate) scrollport: CssRect,
    /// Used `top`, `right`, `bottom`, `left` (`None` for `auto`).
    pub(crate) insets: [Option<f64>; 4],
    pub(crate) border: CssRect,
    /// Margins: top, right, bottom, left.
    pub(crate) margin: [f64; 4],
    pub(crate) containing: CssRect,
}

fn edges(r: CssRect) -> (f64, f64, f64, f64) {
    let (x, y) = (r.origin().x.get(), r.origin().y.get());
    (
        x,
        y,
        x + r.size().width().get(),
        y + r.size().height().get(),
    )
}

impl StickyConstraint {
    /// The fragment index of the nearest scroll container; `None` is the
    /// viewport.
    pub const fn scroll_container(self) -> Option<usize> {
        self.scroll_container
    }
    /// The scrollport (padding box, no scrollbars exist) at scroll offset
    /// zero, in document coordinates.
    pub const fn scrollport(self) -> CssRect {
        self.scrollport
    }
    /// Used `top`, `right`, `bottom` and `left` insets; `None` is `auto`.
    /// Percentages resolve against the scrollport size.
    pub const fn insets(self) -> [Option<f64>; 4] {
        self.insets
    }
    /// The border box in normal flow, before any sticky offset.
    pub const fn static_border(self) -> CssRect {
        self.border
    }
    /// The containing block (its content box) the margin box stays inside.
    pub const fn containing_block(self) -> CssRect {
        self.containing
    }

    /// The sticky offset when the scroll container is scrolled by `scroll`
    /// (the scrollport moves by it over the document).
    pub fn offset(self, scroll: CssPoint) -> Result<CssPoint, GeometryError> {
        let (bx0, by0, bx1, by1) = edges(self.border);
        let (cx0, cy0, cx1, cy1) = edges(self.containing);
        let (px0, py0, px1, py1) = edges(self.scrollport);
        let (sx, sy) = (scroll.x.get(), scroll.y.get());
        let [top, right, bottom, left] = self.insets;
        let [mt, mr, mb, ml] = self.margin;
        let vertical = Axis {
            start: by0,
            end: by1,
            margin_start: mt,
            margin_end: mb,
            cb_start: cy0,
            cb_end: cy1,
            view_start: top.map(|t| py0 + sy + t),
            view_end: bottom.map(|b| py1 + sy - b),
        };
        let horizontal = Axis {
            start: bx0,
            end: bx1,
            margin_start: ml,
            margin_end: mr,
            cb_start: cx0,
            cb_end: cx1,
            view_start: left.map(|l| px0 + sx + l),
            view_end: right.map(|r| px1 + sx - r),
        };
        Ok(CssPoint {
            x: CssPx::new(horizontal.offset())?,
            y: CssPx::new(vertical.offset())?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CssSize;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> CssRect {
        CssRect::new(
            CssPoint {
                x: CssPx::new(x).unwrap(),
                y: CssPx::new(y).unwrap(),
            },
            CssSize::new(w, h).unwrap(),
        )
        .unwrap()
    }

    fn at(x: f64, y: f64) -> CssPoint {
        CssPoint {
            x: CssPx::new(x).unwrap(),
            y: CssPx::new(y).unwrap(),
        }
    }

    /// An independent per-scroll-position model: try every integer shift
    /// and keep the one the specification's constraints select.
    fn model(c: &StickyConstraint, sy: f64) -> f64 {
        let (_, by0, _, by1) = edges(c.border);
        let (_, cy0, _, cy1) = edges(c.containing);
        let (_, py0, _, py1) = edges(c.scrollport);
        let [top, _, bottom, _] = c.insets;
        let [mt, _, mb, _] = c.margin;
        let allowed = |d: f64| by0 - mt + d >= cy0 - 1e-9 && by1 + mb + d <= cy1 + 1e-9;
        if let Some(t) = top {
            let v = py0 + sy + t;
            if by0 < v {
                // The largest allowed shift towards `v`, at most `v - by0`.
                let mut best = 0.0;
                let mut d = 0.0;
                while d <= v - by0 + 1e-9 {
                    if allowed(d) {
                        best = d;
                    }
                    d += 1.0;
                }
                if best > 0.0 {
                    return best;
                }
            }
        }
        if let Some(b) = bottom {
            let v = py1 + sy - b;
            if by1 > v {
                let mut best = 0.0;
                let mut d = 0.0;
                while d >= v - by1 - 1e-9 {
                    if allowed(d) {
                        best = d;
                    }
                    d -= 1.0;
                }
                return best;
            }
        }
        0.0
    }

    #[test]
    fn offsets_match_an_independent_scan_over_scroll_positions() {
        // A 20px box at y=100 in a containing block 50..300, in a 200px
        // scrollport, for several inset combinations.
        for insets in [
            [Some(10.0), None, None, None],
            [None, None, Some(10.0), None],
            [Some(0.0), None, Some(0.0), None],
            [Some(30.0), None, Some(40.0), None],
        ] {
            let c = StickyConstraint {
                scroll_container: None,
                scrollport: rect(0.0, 0.0, 100.0, 200.0),
                insets,
                border: rect(0.0, 100.0, 100.0, 20.0),
                margin: [5.0, 0.0, 3.0, 0.0],
                containing: rect(0.0, 50.0, 100.0, 250.0),
            };
            for sy in (-100..=400).step_by(1) {
                let sy = f64::from(sy);
                let got = c.offset(at(0.0, sy)).unwrap().y.get();
                assert_eq!(got, model(&c, sy), "{insets:?} at {sy}");
            }
        }
    }

    #[test]
    fn horizontal_insets_and_auto_sides_do_not_move_the_box() {
        let c = StickyConstraint {
            scroll_container: Some(3),
            scrollport: rect(10.0, 0.0, 100.0, 100.0),
            insets: [None, None, None, Some(5.0)],
            border: rect(20.0, 0.0, 10.0, 10.0),
            margin: [0.0; 4],
            containing: rect(10.0, 0.0, 300.0, 100.0),
        };
        assert_eq!(c.offset(at(0.0, 0.0)).unwrap(), at(0.0, 0.0));
        // Scrolled 40px right: the view's left edge is at 55.
        assert_eq!(c.offset(at(40.0, 50.0)).unwrap(), at(35.0, 0.0));
        // Never past the containing block's end (310 - 10 = 300).
        assert_eq!(c.offset(at(400.0, 0.0)).unwrap(), at(280.0, 0.0));
    }
}
