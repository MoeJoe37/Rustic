//! `clip-path` basic shapes resolved against a reference box (CSS Masking 1
//! §5, CSS Shapes 1 §3).

use rustic_css::{BasicShape, CornerRadius, LengthPercentage, ShapeRadius};
use rustic_layout::{CssPoint, CssPx, CssRect, CssSize};

use crate::rounded::{Corner, RoundedRect, resolve, used_corners};

/// A basic shape in CSS pixels.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Resolved {
    /// A rectangle, possibly rounded (an ellipse is a fully rounded one).
    Rounded(RoundedRect),
    /// A polygon with the nonzero fill rule.
    Polygon(Vec<(f64, f64)>),
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Option<CssRect> {
    CssRect::new(
        CssPoint {
            x: CssPx::new(x).ok()?,
            y: CssPx::new(y).ok()?,
        },
        CssSize::new(w.max(0.0), h.max(0.0)).ok()?,
    )
    .ok()
}

fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64) -> Option<RoundedRect> {
    let (rx, ry) = (rx.max(0.0), ry.max(0.0));
    Some(RoundedRect {
        rect: rect(cx - rx, cy - ry, 2.0 * rx, 2.0 * ry)?,
        corners: [Corner { x: rx, y: ry }; 4],
    })
}

/// Resolve `shape` in the reference box `(x, y, w, h)`; `None` when a value
/// cannot resolve to a finite length.
pub(crate) fn resolve_shape(
    shape: &BasicShape,
    (x, y, w, h): (f64, f64, f64, f64),
) -> Option<Resolved> {
    let length = |lp: &LengthPercentage, basis: f64| -> Option<f64> {
        // `resolve` clamps at zero; offsets and coordinates may be negative.
        let px = match lp {
            LengthPercentage::Length(l) => l.value,
            LengthPercentage::Percentage(p) => p / 100.0 * basis,
            LengthPercentage::Calc(calc) => calc.resolve(|l| l.value, Some(basis))?,
        };
        px.is_finite().then_some(px)
    };
    let center = |c: &Option<rustic_css::PositionValue>| -> Option<(f64, f64)> {
        match c {
            None => Some((x + w / 2.0, y + h / 2.0)),
            Some(p) => Some((
                x + length(&p.x.to_offset(), w)?,
                y + length(&p.y.to_offset(), h)?,
            )),
        }
    };
    Some(match shape {
        BasicShape::Inset { offsets, radii } => {
            let [t, r, b, l] = offsets;
            let (mut t, mut r, mut b, mut l) =
                (length(t, h)?, length(r, w)?, length(b, h)?, length(l, w)?);
            // Offsets that overlap shrink proportionally to meet.
            if l + r > w && l + r > 0.0 {
                let f = w / (l + r);
                (l, r) = (l * f, r * f);
            }
            if t + b > h && t + b > 0.0 {
                let f = h / (t + b);
                (t, b) = (t * f, b * f);
            }
            let inner = rect(x + l, y + t, w - l - r, h - t - b)?;
            let radii: [&CornerRadius; 4] = [&radii[0], &radii[1], &radii[2], &radii[3]];
            Resolved::Rounded(used_corners(radii, inner)?)
        }
        BasicShape::Circle { radius, center: c } => {
            let (cx, cy) = center(c)?;
            let sides = [
                (cx - x).abs(),
                (x + w - cx).abs(),
                (cy - y).abs(),
                (y + h - cy).abs(),
            ];
            let r = match radius {
                ShapeRadius::ClosestSide => sides.into_iter().fold(f64::INFINITY, f64::min),
                ShapeRadius::FarthestSide => sides.into_iter().fold(0.0, f64::max),
                ShapeRadius::Length(lp) => resolve(lp, ((w * w + h * h) / 2.0).sqrt())?,
            };
            Resolved::Rounded(ellipse(cx, cy, r, r)?)
        }
        BasicShape::Ellipse { rx, ry, center: c } => {
            let (cx, cy) = center(c)?;
            let axis = |radius: &ShapeRadius, lo: f64, hi: f64, basis: f64| match radius {
                ShapeRadius::ClosestSide => Some(lo.abs().min(hi.abs())),
                ShapeRadius::FarthestSide => Some(lo.abs().max(hi.abs())),
                ShapeRadius::Length(lp) => resolve(lp, basis),
            };
            let rx = axis(rx, cx - x, x + w - cx, w)?;
            let ry = axis(ry, cy - y, y + h - cy, h)?;
            Resolved::Rounded(ellipse(cx, cy, rx, ry)?)
        }
        BasicShape::Polygon { points, .. } => Resolved::Polygon(
            points
                .iter()
                .map(|(px, py)| Some((x + length(px, w)?, y + length(py, h)?)))
                .collect::<Option<_>>()?,
        ),
    })
}

/// Whether `(px, py)` is inside `polygon` by the nonzero rule.
pub(crate) fn polygon_contains(polygon: &[(f64, f64)], px: f64, py: f64) -> bool {
    let mut winding = 0i32;
    for i in 0..polygon.len() {
        let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
        let side = (b.0 - a.0) * (py - a.1) - (px - a.0) * (b.1 - a.1);
        if a.1 <= py {
            if b.1 > py && side > 0.0 {
                winding += 1;
            }
        } else if b.1 <= py && side < 0.0 {
            winding -= 1;
        }
    }
    winding != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustic_css::{Length, PositionComponent, PositionValue};

    fn px(v: f64) -> LengthPercentage {
        LengthPercentage::Length(Length::px(v))
    }

    #[test]
    fn shapes_resolve_in_the_reference_box() {
        let reference = (10.0, 20.0, 100.0, 50.0);
        let circle = BasicShape::Circle {
            radius: ShapeRadius::ClosestSide,
            center: Some(PositionValue {
                x: PositionComponent::Offset(px(30.0)),
                y: PositionComponent::Offset(LengthPercentage::Percentage(50.0)),
            }),
        };
        let Some(Resolved::Rounded(r)) = resolve_shape(&circle, reference) else {
            panic!("ellipse expected");
        };
        // Center (40, 45); the closest side is 25px away (top and bottom).
        assert_eq!(r.edges(), (15.0, 20.0, 65.0, 70.0));
        assert_eq!(r.corners[0], Corner { x: 25.0, y: 25.0 });
        let inset = BasicShape::Inset {
            offsets: [px(10.0), px(80.0), px(10.0), px(80.0)],
            radii: [
                CornerRadius::ZERO,
                CornerRadius::ZERO,
                CornerRadius::ZERO,
                CornerRadius::ZERO,
            ],
        };
        let Some(Resolved::Rounded(r)) = resolve_shape(&inset, reference) else {
            panic!("rect expected");
        };
        // 160px of horizontal offsets in a 100px box meet at its middle.
        assert_eq!(r.edges(), (60.0, 30.0, 60.0, 60.0));
        let triangle = [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)];
        assert!(polygon_contains(&triangle, 2.0, 2.0));
        assert!(!polygon_contains(&triangle, 8.0, 8.0));
    }
}
