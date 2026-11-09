//! Rounded rectangles for `border-radius` (CSS Backgrounds 3 §5).

use rustic_css::{CornerRadius, LengthPercentage, LengthUnit};
use rustic_layout::CssRect;

use crate::display::PathPoint;

/// The horizontal and vertical radii of one elliptical corner, in CSS
/// pixels; a corner is square when either is zero.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Corner {
    /// Horizontal radius.
    pub x: f64,
    /// Vertical radius.
    pub y: f64,
}

impl Corner {
    const fn is_square(self) -> bool {
        self.x <= 0.0 || self.y <= 0.0
    }
}

/// A rectangle with elliptical corners: top-left, top-right, bottom-right,
/// bottom-left. Adjacent radii never exceed their side (they are scaled
/// down as CSS Backgrounds 3 §5.5 describes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoundedRect {
    /// The bounding rectangle.
    pub rect: CssRect,
    /// Corner radii in the order top-left, top-right, bottom-right,
    /// bottom-left.
    pub corners: [Corner; 4],
}

/// `(cos, sin)` of `k·π/16` for `k` in `0..=8`, as constants so that no
/// platform math library is involved.
const ARC: [(f64, f64); 9] = [
    (1.0, 0.0),
    (0.980_785_280_403_230_4, 0.195_090_322_016_128_25),
    (0.923_879_532_511_286_7, 0.382_683_432_365_089_8),
    (0.831_469_612_302_545_2, 0.555_570_233_019_602_2),
    (
        std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2,
    ),
    (0.555_570_233_019_602_2, 0.831_469_612_302_545_2),
    (0.382_683_432_365_089_8, 0.923_879_532_511_286_7),
    (0.195_090_322_016_128_25, 0.980_785_280_403_230_4),
    (0.0, 1.0),
];
/// `1 / cos(π/16)`.
const SEC_PI_16: f64 = 1.019_591_158_208_318_4;

impl RoundedRect {
    /// The rectangle's edges as (left, top, right, bottom).
    pub fn edges(&self) -> (f64, f64, f64, f64) {
        let o = self.rect.origin();
        let s = self.rect.size();
        (
            o.x.get(),
            o.y.get(),
            o.x.get() + s.width().get(),
            o.y.get() + s.height().get(),
        )
    }

    /// Whether every corner is square.
    pub fn is_rect(&self) -> bool {
        self.corners.iter().all(|c| c.is_square())
    }

    /// Whether `(x, y)` lies inside, with half-open rectangle edges.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let (l, t, r, b) = self.edges();
        if !(l <= x && x < r && t <= y && y < b) {
            return false;
        }
        let [tl, tr, br, bl] = self.corners;
        // (corner, center, whether the point is in that corner's box)
        let checks = [
            (tl, l + tl.x, t + tl.y, x < l + tl.x && y < t + tl.y),
            (tr, r - tr.x, t + tr.y, x > r - tr.x && y < t + tr.y),
            (br, r - br.x, b - br.y, x > r - br.x && y > b - br.y),
            (bl, l + bl.x, b - bl.y, x < l + bl.x && y > b - bl.y),
        ];
        checks.iter().all(|&(c, cx, cy, inside_box)| {
            if c.is_square() || !inside_box {
                return true;
            }
            let (dx, dy) = ((x - cx) / c.x, (y - cy) / c.y);
            dx * dx + dy * dy <= 1.0
        })
    }

    /// The outline as one clockwise contour (y down) of quadratic curves in
    /// TrueType form (see [`PathPoint`]): each elliptical corner is four
    /// quadratics, which stray less than 0.02% of the radius from the
    /// ellipse. Only constants and arithmetic are used, so every platform
    /// produces the same points.
    pub fn contour(&self) -> Vec<PathPoint> {
        let (l, t, r, b) = self.edges();
        let [tl, tr, br, bl] = self.corners;
        let mut out = Vec::with_capacity(36);
        let on = |x, y| PathPoint {
            x,
            y,
            on_curve: true,
        };
        // (corner, center, start offset a, end offset b): the arc is
        // center + a·cos θ + b·sin θ for θ from 0 to π/2.
        let corners = [
            (tr, (r - tr.x, t + tr.y), (0.0, -tr.y), (tr.x, 0.0), (r, t)),
            (br, (r - br.x, b - br.y), (br.x, 0.0), (0.0, br.y), (r, b)),
            (bl, (l + bl.x, b - bl.y), (0.0, bl.y), (-bl.x, 0.0), (l, b)),
            (tl, (l + tl.x, t + tl.y), (-tl.x, 0.0), (0.0, -tl.y), (l, t)),
        ];
        for (corner, (cx, cy), a, b, square) in corners {
            if corner.is_square() {
                out.push(on(square.0, square.1));
                continue;
            }
            let at = |(c, s): (f64, f64), k: f64| PathPoint {
                x: cx + k * (a.0 * c + b.0 * s),
                y: cy + k * (a.1 * c + b.1 * s),
                on_curve: k == 1.0,
            };
            for (i, &angle) in ARC.iter().enumerate() {
                // Even entries are on the arc; odd ones are the controls,
                // pushed out by sec(π/16) to the tangent intersection.
                out.push(at(angle, if i % 2 == 0 { 1.0 } else { SEC_PI_16 }));
            }
        }
        out
    }

    /// [`RoundedRect::contour`] as a polygon in CSS pixels, each quadratic
    /// split into 16 chords.
    pub(crate) fn polygon(&self) -> Vec<(f64, f64)> {
        let contour = self.contour();
        let mut out = Vec::with_capacity(contour.len() * 8);
        let mut i = 0;
        while i < contour.len() {
            let p = contour[i];
            if p.on_curve {
                out.push((p.x, p.y));
                i += 1;
                continue;
            }
            // `contour` alternates on, off, on within a corner.
            let (Some(&start), Some(&end)) = (out.last(), contour.get(i + 1)) else {
                break;
            };
            for k in 1..16 {
                let t = f64::from(k) / 16.0;
                let u = 1.0 - t;
                out.push((
                    u * u * start.0 + 2.0 * u * t * p.x + t * t * end.x,
                    u * u * start.1 + 2.0 * u * t * p.y + t * t * end.y,
                ));
            }
            i += 1;
        }
        out
    }

    /// The curve inside this one at `inner`, whose edges lie the given
    /// distances inward (CSS Backgrounds 3 §5.2): each radius shrinks by
    /// the adjacent distance, stopping at zero.
    pub(crate) fn inset(&self, inner: CssRect) -> Self {
        let (l, t, r, b) = self.edges();
        let o = inner.origin();
        let s = inner.size();
        let (il, it) = (o.x.get(), o.y.get());
        let (ir, ib) = (il + s.width().get(), it + s.height().get());
        let (left, top, right, bottom) = (il - l, it - t, r - ir, b - ib);
        let shrink = |c: Corner, dx: f64, dy: f64| Corner {
            x: (c.x - dx).max(0.0),
            y: (c.y - dy).max(0.0),
        };
        let [tl, tr, br, bl] = self.corners;
        Self {
            rect: inner,
            corners: [
                shrink(tl, left, top),
                shrink(tr, right, top),
                shrink(br, right, bottom),
                shrink(bl, left, bottom),
            ],
        }
    }
}

/// Resolve one computed `<length-percentage>` radius against `basis`.
pub(crate) fn resolve(value: &LengthPercentage, basis: f64) -> Option<f64> {
    let px = match value {
        LengthPercentage::Length(l) if l.unit == LengthUnit::Px => l.value,
        LengthPercentage::Length(_) => return None,
        LengthPercentage::Percentage(p) => p / 100.0 * basis,
        LengthPercentage::Calc(calc) => calc.resolve(|l| l.value, Some(basis))?,
    };
    px.is_finite().then_some(px.max(0.0))
}

/// The used corners of a border box from computed radii (top-left,
/// top-right, bottom-right, bottom-left): percentages against the box's
/// width and height, then all scaled down together until no side's two
/// radii overlap (CSS Backgrounds 3 §5.5). `None` for an invalid value.
pub(crate) fn used_corners(radii: [&CornerRadius; 4], border: CssRect) -> Option<RoundedRect> {
    let (w, h) = (border.size().width().get(), border.size().height().get());
    let mut corners = [Corner::default(); 4];
    for (corner, radius) in corners.iter_mut().zip(radii) {
        *corner = Corner {
            x: resolve(&radius.horizontal, w)?,
            y: resolve(&radius.vertical, h)?,
        };
    }
    let [tl, tr, br, bl] = corners;
    let ratio = |side: f64, a: f64, b: f64| if a + b > 0.0 { side / (a + b) } else { 1.0 };
    let f = [
        ratio(w, tl.x, tr.x),
        ratio(h, tr.y, br.y),
        ratio(w, br.x, bl.x),
        ratio(h, bl.y, tl.y),
    ]
    .into_iter()
    .fold(1.0_f64, f64::min);
    if f < 1.0 {
        for c in &mut corners {
            c.x *= f;
            c.y *= f;
        }
    }
    // A corner with one zero radius is square.
    for c in &mut corners {
        if c.is_square() {
            *c = Corner::default();
        }
    }
    Some(RoundedRect {
        rect: border,
        corners,
    })
}

/// Sutherland–Hodgman clip of one closed contour to a convex polygon given
/// clockwise (y down). Winding inside the polygon is preserved.
pub(crate) fn clip_convex(contour: &[(f64, f64)], polygon: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut points = contour.to_vec();
    for i in 0..polygon.len() {
        if points.is_empty() {
            break;
        }
        let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
        // Clockwise in y-down space: the inside is where the cross is >= 0.
        let side = |p: (f64, f64)| (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
        let cross = |p: (f64, f64), q: (f64, f64)| {
            let (sp, sq) = (side(p), side(q));
            let t = sp / (sp - sq);
            (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t)
        };
        let input = std::mem::take(&mut points);
        let mut previous = input[input.len() - 1];
        for &current in &input {
            match (side(previous) >= 0.0, side(current) >= 0.0) {
                (true, true) => points.push(current),
                (true, false) => points.push(cross(previous, current)),
                (false, true) => {
                    points.push(cross(previous, current));
                    points.push(current);
                }
                (false, false) => {}
            }
            previous = current;
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustic_layout::{CssPoint, CssPx, CssSize};

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

    fn area(points: &[(f64, f64)]) -> f64 {
        let n = points.len();
        (0..n)
            .map(|i| {
                let (a, b) = (points[i], points[(i + 1) % n]);
                a.0 * b.1 - b.0 * a.1
            })
            .sum::<f64>()
            / 2.0
    }

    #[test]
    fn overlapping_radii_scale_down_together() {
        let px = |v| CornerRadius {
            horizontal: LengthPercentage::Length(rustic_css::Length::px(v)),
            vertical: LengthPercentage::Percentage(50.0),
        };
        let (a, b) = (px(80.0), px(20.0));
        let used = used_corners([&a, &a, &b, &b], rect(0.0, 0.0, 100.0, 40.0)).unwrap();
        // The top side needs 160px in 100px: everything scales by 5/8.
        assert_eq!(used.corners[0], Corner { x: 50.0, y: 12.5 });
        assert_eq!(used.corners[2], Corner { x: 12.5, y: 12.5 });
    }

    #[test]
    fn corners_stay_on_the_ellipse_and_containment_matches() {
        let r = RoundedRect {
            rect: rect(0.0, 0.0, 100.0, 60.0),
            corners: [Corner { x: 30.0, y: 30.0 }; 4],
        };
        let points = r.polygon();
        let exact = 100.0 * 60.0 - (4.0 - std::f64::consts::PI) * 900.0;
        // Clockwise in y-down space has a positive shoelace area.
        assert!((area(&points) - exact).abs() < 0.5, "{}", area(&points));
        // Every flattened point lies on the circle to within 0.02%.
        for &(x, y) in &points {
            let (cx, cy) = (x.clamp(30.0, 70.0), 30.0);
            let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            assert!((d - 30.0).abs() <= 0.006, "{x},{y}");
        }
        assert!(r.contains(50.0, 0.0));
        assert!(!r.contains(1.0, 1.0));
        assert!(r.contains(9.0, 9.0));
        assert!(!r.contains(100.0, 30.0));
        let inner = r.inset(rect(10.0, 40.0, 80.0, 10.0));
        assert_eq!(inner.corners[0], Corner { x: 20.0, y: 0.0 });
    }

    #[test]
    fn convex_clipping_keeps_the_area_inside() {
        let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let triangle = [(0.0, 0.0), (10.0, 0.0), (5.0, 5.0)];
        assert!((area(&clip_convex(&square, &triangle)) - 25.0).abs() < 1e-9);
    }
}
