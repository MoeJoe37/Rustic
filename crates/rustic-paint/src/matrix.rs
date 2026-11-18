//! Transform matrices (CSS Transforms 1 and 2) built with plain arithmetic,
//! so every platform computes the same values: no platform math library is
//! called for trigonometry.

use rustic_css::{PropertyId as P, PropertyValue as V, TransformFunction};
use rustic_style::ComputedStyle;

use crate::clip::length;

/// Why a box's transform could not be used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TransformError {
    /// A computed value of the wrong type.
    Invalid,
}

/// The perspective a containing block gives its transformed children:
/// `perspective(d)` about `perspective-origin` in its border box, or `None`.
pub(crate) fn perspective_of(
    style: &ComputedStyle,
    (x, y, w, h): (f64, f64, f64, f64),
) -> Result<Option<Matrix4>, TransformError> {
    let V::Perspective(d) = style.get(P::Perspective) else {
        return Err(TransformError::Invalid);
    };
    let Some(d) = d else {
        return Ok(None);
    };
    let V::PerspectiveOrigin(origin) = style.get(P::PerspectiveOrigin) else {
        return Err(TransformError::Invalid);
    };
    let d = length(d, 0.0).ok_or(TransformError::Invalid)?;
    let ox = x + length(&origin.x.to_offset(), w).ok_or(TransformError::Invalid)?;
    let oy = y + length(&origin.y.to_offset(), h).ok_or(TransformError::Invalid)?;
    Ok(Some(
        Matrix4::translate(ox, oy, 0.0)
            .multiply(&Matrix4::perspective(Some(d)))
            .multiply(&Matrix4::translate(-ox, -oy, 0.0)),
    ))
}

/// A box's transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum BoxTransform {
    /// No transform.
    None,
    /// A length or matrix that cannot be resolved to a finite value.
    Degenerate,
    /// The box's matrix in layout coordinates, perspective included.
    Matrix(Matrix4),
}

/// The transform of a box with border box `(x, y, w, h)` in layout
/// coordinates (CSS Transforms 2 §6): `translate`, `rotate`, `scale` and
/// then `transform`, about `transform-origin`, after the containing block's
/// `perspective`.
pub(crate) fn box_transform(
    style: &ComputedStyle,
    (x, y, w, h): (f64, f64, f64, f64),
    perspective: Option<Matrix4>,
) -> Result<BoxTransform, TransformError> {
    let V::Transform(list) = style.get(P::Transform) else {
        return Err(TransformError::Invalid);
    };
    let mut functions = Vec::with_capacity(list.0.len() + 3);
    for property in [P::Translate, P::Rotate, P::Scale] {
        match style.get(property) {
            V::IndividualTransform(t) => functions.extend(t.0.iter()),
            _ => return Err(TransformError::Invalid),
        }
    }
    functions.extend(list.0.iter());
    if functions.is_empty() {
        return Ok(BoxTransform::None);
    }
    let V::TransformOrigin(origin, z) = style.get(P::TransformOrigin) else {
        return Err(TransformError::Invalid);
    };
    let matrix = || -> Option<Matrix4> {
        let ox = x + length(&origin.x.to_offset(), w)?;
        let oy = y + length(&origin.y.to_offset(), h)?;
        let oz = length(z, 0.0)?;
        let mut m = perspective
            .unwrap_or(Matrix4::IDENTITY)
            .multiply(&Matrix4::translate(ox, oy, oz));
        for f in &functions {
            m = m.multiply(&Matrix4::function(f, &length, (w, h))?);
        }
        let m = m.multiply(&Matrix4::translate(-ox, -oy, -oz));
        let finite = m.0.iter().flatten().all(|v| v.is_finite());
        finite.then_some(m)
    };
    Ok(matrix().map_or(BoxTransform::Degenerate, BoxTransform::Matrix))
}

/// A 4×4 matrix in row-major order acting on column vectors `(x, y, z, w)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Matrix4(pub(crate) [[f64; 4]; 4]);

impl Matrix4 {
    pub(crate) const IDENTITY: Self = Self([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);

    pub(crate) fn multiply(&self, other: &Self) -> Self {
        let mut out = [[0.0; 4]; 4];
        for (r, row) in out.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = (0..4).map(|k| self.0[r][k] * other.0[k][c]).sum();
            }
        }
        Self(out)
    }

    pub(crate) fn translate(x: f64, y: f64, z: f64) -> Self {
        let mut m = Self::IDENTITY;
        m.0[0][3] = x;
        m.0[1][3] = y;
        m.0[2][3] = z;
        m
    }

    fn scale(x: f64, y: f64, z: f64) -> Self {
        let mut m = Self::IDENTITY;
        m.0[0][0] = x;
        m.0[1][1] = y;
        m.0[2][2] = z;
        m
    }

    /// `perspective(d)`: `None` or a distance of at most zero is the
    /// identity (CSS Transforms 2 clamps distances below 1px to 1px).
    pub(crate) fn perspective(d: Option<f64>) -> Self {
        let mut m = Self::IDENTITY;
        if let Some(d) = d {
            m.0[3][2] = -1.0 / d.max(1.0);
        }
        m
    }

    /// Rotation by `angle` degrees about `(x, y, z)` (CSS Transforms 2
    /// §13.1); a zero axis is no rotation.
    fn rotate(x: f64, y: f64, z: f64, angle: f64) -> Self {
        let length = (x * x + y * y + z * z).sqrt();
        if length.is_nan() || length <= 0.0 || !length.is_finite() {
            return Self::IDENTITY;
        }
        let (x, y, z) = (x / length, y / length, z / length);
        let (s, c) = sin_cos_degrees(angle);
        let t = 1.0 - c;
        Self([
            [t * x * x + c, t * x * y - s * z, t * x * z + s * y, 0.0],
            [t * x * y + s * z, t * y * y + c, t * y * z - s * x, 0.0],
            [t * x * z - s * y, t * y * z + s * x, t * z * z + c, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    fn skew(ax: f64, ay: f64) -> Self {
        let mut m = Self::IDENTITY;
        m.0[0][1] = tan_degrees(ax);
        m.0[1][0] = tan_degrees(ay);
        m
    }

    /// The matrix of one function, with `px(x, y, z)` resolving the
    /// translation's lengths (x and y against the reference box) and
    /// `length` a perspective distance; `None` if one does not resolve.
    pub(crate) fn function(
        function: &TransformFunction,
        resolve: &dyn Fn(&rustic_css::LengthPercentage, f64) -> Option<f64>,
        (width, height): (f64, f64),
    ) -> Option<Self> {
        Some(match function {
            TransformFunction::Matrix([a, b, c, d, e, f]) => Self([
                [*a, *c, 0.0, *e],
                [*b, *d, 0.0, *f],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ]),
            TransformFunction::Matrix3d(m) => {
                // Column-major in CSS.
                let mut out = [[0.0; 4]; 4];
                for (c, column) in m.chunks_exact(4).enumerate() {
                    for (r, v) in column.iter().enumerate() {
                        out[r][c] = *v;
                    }
                }
                Self(out)
            }
            TransformFunction::Translate(x, y, z) => {
                Self::translate(resolve(x, width)?, resolve(y, height)?, resolve(z, 0.0)?)
            }
            TransformFunction::Scale(x, y, z) => Self::scale(*x, *y, *z),
            TransformFunction::Rotate { x, y, z, angle } => Self::rotate(*x, *y, *z, *angle),
            TransformFunction::Skew(x, y) => Self::skew(*x, *y),
            TransformFunction::Perspective(d) => Self::perspective(match d {
                Some(d) => Some(resolve(d, 0.0)?),
                None => None,
            }),
        })
    }

    /// Whether the back of the plane z = 0 faces the viewer: the z component
    /// of the transformed normal is negative (CSS Transforms 2 §5.1), read
    /// from the inverse's cofactor. A singular matrix faces the viewer.
    pub(crate) fn back_facing(&self) -> bool {
        // inverse[2][2] = cofactor(2, 2) / det: the sign of the minor
        // without row 2 and column 2, over the determinant.
        let m = &self.0;
        let minor = |rows: [usize; 3], cols: [usize; 3]| {
            let a = |r: usize, c: usize| m[rows[r]][cols[c]];
            a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1))
                - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
                + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0))
        };
        let cofactor = minor([0, 1, 3], [0, 1, 3]);
        let det: f64 = (0..4)
            .map(|c| {
                let cols: Vec<usize> = (0..4).filter(|&k| k != c).collect();
                let sign = if c % 2 == 0 { 1.0 } else { -1.0 };
                sign * m[0][c] * minor([1, 2, 3], [cols[0], cols[1], cols[2]])
            })
            .sum();
        det != 0.0 && cofactor / det < 0.0
    }

    /// The homography this matrix applies to the plane z = 0: rows x, y and
    /// w of columns x, y and the translation.
    pub(crate) fn plane(&self) -> Homography {
        let m = &self.0;
        Homography([
            m[0][0], m[0][1], m[0][3], m[1][0], m[1][1], m[1][3], m[3][0], m[3][1], m[3][3],
        ])
    }
}

/// A 3×3 projective map of the plane, row-major: `(x, y)` maps to
/// `((h0 x + h1 y + h2) / w, (h3 x + h4 y + h5) / w)` with
/// `w = h6 x + h7 y + h8`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography(pub [f64; 9]);

impl Homography {
    /// The identity map.
    pub const IDENTITY: Self = Self([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

    /// `self` after `other`: maps `p` to `self(other(p))`.
    pub fn then_after(&self, other: &Self) -> Self {
        let (a, b) = (&self.0, &other.0);
        let mut out = [0.0; 9];
        for r in 0..3 {
            for c in 0..3 {
                out[r * 3 + c] = (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum();
            }
        }
        Self(out)
    }

    /// A uniform scale (CSS pixels to device pixels).
    pub fn scale(factor: f64) -> Self {
        Self([factor, 0.0, 0.0, 0.0, factor, 0.0, 0.0, 0.0, 1.0])
    }

    /// Whether this is the identity.
    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// Whether w is the same everywhere (an affine map).
    pub fn is_affine(&self) -> bool {
        self.0[6] == 0.0 && self.0[7] == 0.0
    }

    /// The homogeneous w at `(x, y)`.
    pub fn w(&self, x: f64, y: f64) -> f64 {
        self.0[6] * x + self.0[7] * y + self.0[8]
    }

    /// Map a point; `None` where w is not positive (behind the viewer).
    pub fn apply(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let h = &self.0;
        let w = self.w(x, y);
        (w > 0.0).then(|| {
            (
                (h[0] * x + h[1] * y + h[2]) / w,
                (h[3] * x + h[4] * y + h[5]) / w,
            )
        })
    }

    /// The inverse map, if the matrix is invertible.
    pub fn inverse(&self) -> Option<Self> {
        let m = &self.0;
        let cof = [
            m[4] * m[8] - m[5] * m[7],
            m[5] * m[6] - m[3] * m[8],
            m[3] * m[7] - m[4] * m[6],
            m[2] * m[7] - m[1] * m[8],
            m[0] * m[8] - m[2] * m[6],
            m[1] * m[6] - m[0] * m[7],
            m[1] * m[5] - m[2] * m[4],
            m[2] * m[3] - m[0] * m[5],
            m[0] * m[4] - m[1] * m[3],
        ];
        let det = m[0] * cof[0] + m[1] * cof[1] + m[2] * cof[2];
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        // The adjugate is the transposed cofactor matrix.
        let out = [
            cof[0], cof[3], cof[6], cof[1], cof[4], cof[7], cof[2], cof[5], cof[8],
        ]
        .map(|v| v / det);
        out.iter().all(|v| v.is_finite()).then_some(Self(out))
    }

    /// Map a point through the inverse of `self` onto the plane: the local
    /// point that `self` maps to `(x, y)`.
    pub fn unapply(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let inverse = self.inverse()?;
        let local = inverse.apply_any(x, y)?;
        // The local point must itself be in front of the viewer.
        (self.w(local.0, local.1) > 0.0).then_some(local)
    }

    /// Map a point allowing either sign of w (for inverses, whose w is a
    /// scaled reciprocal).
    fn apply_any(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let h = &self.0;
        let w = self.w(x, y);
        (w != 0.0).then(|| {
            (
                (h[0] * x + h[1] * y + h[2]) / w,
                (h[3] * x + h[4] * y + h[5]) / w,
            )
        })
    }
}

/// `(sin, cos)` of an angle in degrees, from exact reduction to
/// `[0°, 45°]` and Taylor series in plain arithmetic, accurate to a few
/// ulps.
pub(crate) fn sin_cos_degrees(degrees: f64) -> (f64, f64) {
    if !degrees.is_finite() {
        return (0.0, 1.0);
    }
    // `%` on f64 is exact.
    let mut d = degrees % 360.0;
    if d < 0.0 {
        d += 360.0;
    }
    // Quadrant and octant symmetries.
    let quadrant = (d / 90.0).floor();
    let mut r = d - quadrant * 90.0;
    let swap = r > 45.0;
    if swap {
        r = 90.0 - r;
    }
    let x = r * (std::f64::consts::PI / 180.0);
    let x2 = x * x;
    // Horner forms of the Taylor series up to x^17 and x^16; |x| <= π/4
    // keeps the truncation error below 1e-17.
    let sin = x
        * (1.0
            + x2 * (-1.0 / 6.0
                + x2 * (1.0 / 120.0
                    + x2 * (-1.0 / 5040.0
                        + x2 * (1.0 / 362_880.0
                            + x2 * (-1.0 / 39_916_800.0
                                + x2 * (1.0 / 6_227_020_800.0
                                    + x2 * (-1.0 / 1_307_674_368_000.0
                                        + x2 / 355_687_428_096_000.0))))))));
    let cos = 1.0
        + x2 * (-0.5
            + x2 * (1.0 / 24.0
                + x2 * (-1.0 / 720.0
                    + x2 * (1.0 / 40_320.0
                        + x2 * (-1.0 / 3_628_800.0
                            + x2 * (1.0 / 479_001_600.0
                                + x2 * (-1.0 / 87_178_291_200.0 + x2 / 20_922_789_888_000.0)))))));
    let (s, c) = if swap { (cos, sin) } else { (sin, cos) };
    match quadrant as u8 {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

/// `tan` of an angle in degrees; 90° (mod 180°) gives a very large value.
fn tan_degrees(degrees: f64) -> f64 {
    let (s, c) = sin_cos_degrees(degrees);
    if c == 0.0 {
        f64::MAX.copysign(s)
    } else {
        s / c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigonometry_matches_known_values() {
        for (d, s, c) in [
            (0.0, 0.0, 1.0),
            (30.0, 0.5, 0.866_025_403_784_438_6),
            (
                45.0,
                std::f64::consts::FRAC_1_SQRT_2,
                std::f64::consts::FRAC_1_SQRT_2,
            ),
            (90.0, 1.0, 0.0),
            (180.0, 0.0, -1.0),
            (-90.0, -1.0, 0.0),
            (
                765.0,
                std::f64::consts::FRAC_1_SQRT_2,
                std::f64::consts::FRAC_1_SQRT_2,
            ),
        ] {
            let (gs, gc) = sin_cos_degrees(d);
            assert!(
                (gs - s).abs() < 1e-15 && (gc - c).abs() < 1e-15,
                "{d}: {gs} {gc}"
            );
        }
        // Against the platform's functions over a sweep (tolerance only).
        for k in -720..720 {
            let d = f64::from(k) * 0.37;
            let (s, c) = sin_cos_degrees(d);
            let r = d.to_radians();
            assert!(
                (s - r.sin()).abs() < 1e-14 && (c - r.cos()).abs() < 1e-14,
                "{d}"
            );
        }
    }

    #[test]
    fn homographies_compose_and_invert() {
        let rotate = Matrix4::rotate(0.0, 0.0, 1.0, 90.0).plane();
        let moved = Matrix4::translate(10.0, 0.0, 0.0)
            .plane()
            .then_after(&rotate);
        let (x, y) = moved.apply(1.0, 0.0).unwrap();
        assert!((x - 10.0).abs() < 1e-12 && (y - 1.0).abs() < 1e-12);
        let (u, v) = moved.unapply(x, y).unwrap();
        assert!((u - 1.0).abs() < 1e-12 && v.abs() < 1e-12);
        // Perspective: a point pushed toward the viewer grows.
        let near = Matrix4::perspective(Some(100.0))
            .multiply(&Matrix4::translate(0.0, 0.0, 50.0))
            .plane();
        let (x, _) = near.apply(10.0, 0.0).unwrap();
        assert!((x - 20.0).abs() < 1e-12);
        assert!(!Matrix4::rotate(0.0, 1.0, 0.0, 30.0).back_facing());
        assert!(Matrix4::rotate(0.0, 1.0, 0.0, 150.0).back_facing());
        assert!(!Matrix4::scale(-1.0, 1.0, 1.0).back_facing());
    }
}
