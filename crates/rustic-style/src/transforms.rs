//! Transform interpolation (CSS Transforms 1 §9, CSS Transforms 2 §§10,
//! 15–16; editor's drafts reviewed 2026-11-25): matching primitives
//! interpolate as numbers and lengths, everything else through matrix
//! decomposition (translation, scale, skew, perspective and a quaternion,
//! slerped).
//!
//! Matrices here are indexed `m[column][row]`, the layout of the
//! specification's pseudo-code and of `matrix3d()`'s arguments. Trigonometry
//! uses the platform's `f64` functions, which may differ in the last bits
//! between platforms (TD-201); tests compare with tolerances.

use rustic_css::{Calc, IndividualTransform, LengthPercentage, TransformFunction, TransformList};

type Matrix = [[f64; 4]; 4];

const IDENTITY: Matrix = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn lp(a: &LengthPercentage, b: &LengthPercentage, t: f64) -> LengthPercentage {
    if a == b {
        a.clone()
    } else {
        Calc::mix(a, 1.0 - t, b, t)
    }
}

/// `a × b` for column-major matrices (`a` applied after `b`).
fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [[0.0; 4]; 4];
    for (c, column) in out.iter_mut().enumerate() {
        for (r, cell) in column.iter_mut().enumerate() {
            *cell = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

fn from_css(values: &[f64; 16]) -> Matrix {
    let mut m = [[0.0; 4]; 4];
    for (c, column) in m.iter_mut().enumerate() {
        column.copy_from_slice(&values[c * 4..c * 4 + 4]);
    }
    m
}

fn px(value: &LengthPercentage) -> Option<f64> {
    match value {
        LengthPercentage::Length(l) => l.to_px(),
        LengthPercentage::Percentage(_) | LengthPercentage::Calc(_) => None,
    }
}

/// The matrix of one function, or `None` for a translation with a
/// percentage (which needs the reference box).
fn function_matrix(function: &TransformFunction) -> Option<Matrix> {
    let mut m = IDENTITY;
    match function {
        TransformFunction::Matrix([a, b, c, d, e, f]) => {
            m[0][0] = *a;
            m[0][1] = *b;
            m[1][0] = *c;
            m[1][1] = *d;
            m[3][0] = *e;
            m[3][1] = *f;
        }
        TransformFunction::Matrix3d(values) => m = from_css(values),
        TransformFunction::Translate(x, y, z) => {
            m[3][0] = px(x)?;
            m[3][1] = px(y)?;
            m[3][2] = px(z)?;
        }
        TransformFunction::Scale(x, y, z) => {
            m[0][0] = *x;
            m[1][1] = *y;
            m[2][2] = *z;
        }
        TransformFunction::Rotate { x, y, z, angle } => {
            let length = (x * x + y * y + z * z).sqrt();
            if !(length > 0.0 && length.is_finite()) {
                return Some(IDENTITY);
            }
            let (x, y, z) = (x / length, y / length, z / length);
            let half = angle.to_radians() / 2.0;
            let (sc, sq) = (half.sin() * half.cos(), half.sin() * half.sin());
            m = from_css(&[
                1.0 - 2.0 * (y * y + z * z) * sq,
                2.0 * (x * y * sq + z * sc),
                2.0 * (x * z * sq - y * sc),
                0.0,
                2.0 * (x * y * sq - z * sc),
                1.0 - 2.0 * (x * x + z * z) * sq,
                2.0 * (y * z * sq + x * sc),
                0.0,
                2.0 * (x * z * sq + y * sc),
                2.0 * (y * z * sq - x * sc),
                1.0 - 2.0 * (x * x + y * y) * sq,
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
            ]);
        }
        TransformFunction::Skew(ax, ay) => {
            m[1][0] = ax.to_radians().tan();
            m[0][1] = ay.to_radians().tan();
        }
        TransformFunction::Perspective(d) => {
            if let Some(d) = d {
                m[2][3] = -1.0 / px(d)?.max(1.0);
            }
        }
    }
    Some(m)
}

fn list_matrix(functions: &[TransformFunction]) -> Option<Matrix> {
    functions
        .iter()
        .try_fold(IDENTITY, |m, f| Some(multiply(&m, &function_matrix(f)?)))
}

/// A decomposed 3D matrix (CSS Transforms 2 §15.2.1).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Decomposed {
    translate: [f64; 3],
    scale: [f64; 3],
    skew: [f64; 3],
    perspective: [f64; 4],
    quaternion: [f64; 4],
}

fn determinant3(m: [[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

fn minor(m: &Matrix, row: usize, col: usize) -> f64 {
    let mut out = [[0.0; 3]; 3];
    let mut oi = 0;
    for (i, mi) in m.iter().enumerate() {
        if i == row {
            continue;
        }
        let mut oj = 0;
        for (j, v) in mi.iter().enumerate() {
            if j == col {
                continue;
            }
            out[oi][oj] = *v;
            oj += 1;
        }
        oi += 1;
    }
    determinant3(out)
}

fn determinant(m: &Matrix) -> f64 {
    (0..4)
        .map(|j| {
            let sign = if j % 2 == 0 { 1.0 } else { -1.0 };
            sign * m[0][j] * minor(m, 0, j)
        })
        .sum()
}

fn inverse(m: &Matrix) -> Option<Matrix> {
    let det = determinant(m);
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let mut out = [[0.0; 4]; 4];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            let sign = if (i + j) % 2 == 0 { 1.0 } else { -1.0 };
            // Adjugate: transpose of the cofactor matrix.
            *cell = sign * minor(m, j, i) / det;
        }
    }
    Some(out)
}

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn length3(a: [f64; 3]) -> f64 {
    dot3(a, a).sqrt()
}

fn normalize3(a: [f64; 3]) -> [f64; 3] {
    let l = length3(a);
    if l == 0.0 { a } else { a.map(|v| v / l) }
}

fn combine3(a: [f64; 3], b: [f64; 3], wa: f64, wb: f64) -> [f64; 3] {
    [
        a[0] * wa + b[0] * wb,
        a[1] * wa + b[1] * wb,
        a[2] * wa + b[2] * wb,
    ]
}

fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn decompose(matrix: &Matrix) -> Option<Decomposed> {
    let mut m = *matrix;
    if m[3][3] == 0.0 {
        return None;
    }
    let w = m[3][3];
    for column in &mut m {
        for v in column.iter_mut() {
            *v /= w;
        }
    }
    let mut perspective_matrix = m;
    for column in perspective_matrix.iter_mut().take(3) {
        column[3] = 0.0;
    }
    perspective_matrix[3][3] = 1.0;
    if determinant(&perspective_matrix) == 0.0 {
        return None;
    }
    let perspective = if m[0][3] != 0.0 || m[1][3] != 0.0 || m[2][3] != 0.0 {
        let rhs = [m[0][3], m[1][3], m[2][3], m[3][3]];
        let inverse = inverse(&perspective_matrix)?;
        // rhs × transpose(inverse), as a row vector.
        let mut out = [0.0; 4];
        for (i, o) in out.iter_mut().enumerate() {
            *o = (0..4).map(|k| rhs[k] * inverse[i][k]).sum();
        }
        out
    } else {
        [0.0, 0.0, 0.0, 1.0]
    };
    let translate = [m[3][0], m[3][1], m[3][2]];
    let mut row = [
        [m[0][0], m[0][1], m[0][2]],
        [m[1][0], m[1][1], m[1][2]],
        [m[2][0], m[2][1], m[2][2]],
    ];
    let mut scale = [0.0; 3];
    let mut skew = [0.0; 3];
    scale[0] = length3(row[0]);
    row[0] = normalize3(row[0]);
    skew[0] = dot3(row[0], row[1]);
    row[1] = combine3(row[1], row[0], 1.0, -skew[0]);
    scale[1] = length3(row[1]);
    row[1] = normalize3(row[1]);
    if scale[1] != 0.0 {
        skew[0] /= scale[1];
    }
    skew[1] = dot3(row[0], row[2]);
    row[2] = combine3(row[2], row[0], 1.0, -skew[1]);
    skew[2] = dot3(row[1], row[2]);
    row[2] = combine3(row[2], row[1], 1.0, -skew[2]);
    scale[2] = length3(row[2]);
    row[2] = normalize3(row[2]);
    if scale[2] != 0.0 {
        skew[1] /= scale[2];
        skew[2] /= scale[2];
    }
    if dot3(row[0], cross3(row[1], row[2])) < 0.0 {
        for i in 0..3 {
            scale[i] = -scale[i];
            row[i] = row[i].map(|v| -v);
        }
    }
    // The rotation's quaternion. The specification's square-root form
    // loses the relative signs of half-turns, so this uses Shepperd's
    // method (largest diagonal term first), with `w >= 0` as there.
    // `row[c][r]` is the rotation matrix's entry at row r, column c.
    let r = |rr: usize, c: usize| row[c][rr];
    let trace = r(0, 0) + r(1, 1) + r(2, 2);
    let mut q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            (r(2, 1) - r(1, 2)) / s,
            (r(0, 2) - r(2, 0)) / s,
            (r(1, 0) - r(0, 1)) / s,
            0.25 * s,
        ]
    } else if r(0, 0) > r(1, 1) && r(0, 0) > r(2, 2) {
        let s = (1.0 + r(0, 0) - r(1, 1) - r(2, 2)).sqrt() * 2.0;
        [
            0.25 * s,
            (r(0, 1) + r(1, 0)) / s,
            (r(0, 2) + r(2, 0)) / s,
            (r(2, 1) - r(1, 2)) / s,
        ]
    } else if r(1, 1) > r(2, 2) {
        let s = (1.0 + r(1, 1) - r(0, 0) - r(2, 2)).sqrt() * 2.0;
        [
            (r(0, 1) + r(1, 0)) / s,
            0.25 * s,
            (r(1, 2) + r(2, 1)) / s,
            (r(0, 2) - r(2, 0)) / s,
        ]
    } else {
        let s = (1.0 + r(2, 2) - r(0, 0) - r(1, 1)).sqrt() * 2.0;
        [
            (r(0, 2) + r(2, 0)) / s,
            (r(1, 2) + r(2, 1)) / s,
            0.25 * s,
            (r(1, 0) - r(0, 1)) / s,
        ]
    };
    if q[3] < 0.0 {
        q = q.map(|v| -v);
    }
    Some(Decomposed {
        translate,
        scale,
        skew,
        perspective,
        quaternion: q,
    })
}

fn slerp(a: [f64; 4], b: [f64; 4], t: f64) -> [f64; 4] {
    let product = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).clamp(-1.0, 1.0);
    if product.abs() == 1.0 {
        return a;
    }
    let theta = product.acos();
    let w = (t * theta).sin() / (1.0 - product * product).sqrt();
    let k = (t * theta).cos() - product * w;
    [
        a[0] * k + b[0] * w,
        a[1] * k + b[1] * w,
        a[2] * k + b[2] * w,
        a[3] * k + b[3] * w,
    ]
}

fn recompose(d: &Decomposed) -> Matrix {
    let mut m = IDENTITY;
    for (i, column) in m.iter_mut().enumerate() {
        column[3] = d.perspective[i];
    }
    let offsets: [f64; 4] = std::array::from_fn(|i| (0..3).map(|j| d.translate[j] * m[j][i]).sum());
    for (cell, offset) in m[3].iter_mut().zip(offsets) {
        *cell += offset;
    }
    let [x, y, z, w] = d.quaternion;
    let mut rotation = IDENTITY;
    rotation[0][0] = 1.0 - 2.0 * (y * y + z * z);
    // The specification's pseudo-code lists the rotation row by row; in
    // column-major indexing that is the transpose.
    rotation[1][0] = 2.0 * (x * y - z * w);
    rotation[2][0] = 2.0 * (x * z + y * w);
    rotation[0][1] = 2.0 * (x * y + z * w);
    rotation[1][1] = 1.0 - 2.0 * (x * x + z * z);
    rotation[2][1] = 2.0 * (y * z - x * w);
    rotation[0][2] = 2.0 * (x * z - y * w);
    rotation[1][2] = 2.0 * (y * z + x * w);
    rotation[2][2] = 1.0 - 2.0 * (x * x + y * y);
    m = multiply(&m, &rotation);
    let mut temp = IDENTITY;
    if d.skew[2] != 0.0 {
        temp[2][1] = d.skew[2];
        m = multiply(&m, &temp);
    }
    if d.skew[1] != 0.0 {
        temp[2][1] = 0.0;
        temp[2][0] = d.skew[1];
        m = multiply(&m, &temp);
    }
    if d.skew[0] != 0.0 {
        temp[2][0] = 0.0;
        temp[1][0] = d.skew[0];
        m = multiply(&m, &temp);
    }
    for (i, column) in m.iter_mut().enumerate().take(3) {
        for v in column.iter_mut() {
            *v *= d.scale[i];
        }
    }
    m
}

fn is_2d(m: &Matrix) -> bool {
    m[0][2] == 0.0
        && m[0][3] == 0.0
        && m[1][2] == 0.0
        && m[1][3] == 0.0
        && m[2][0] == 0.0
        && m[2][1] == 0.0
        && m[2][2] == 1.0
        && m[2][3] == 0.0
        && m[3][2] == 0.0
        && m[3][3] == 1.0
}

fn to_function(m: &Matrix) -> TransformFunction {
    // Clean the noise of the round trip so a 2D result stays 2D.
    let mut m = *m;
    for column in &mut m {
        for v in column.iter_mut() {
            if v.abs() < 1e-12 {
                *v = 0.0;
            }
        }
    }
    for i in [2usize, 3] {
        if (m[i][i] - 1.0).abs() < 1e-12 {
            m[i][i] = 1.0;
        }
    }
    if is_2d(&m) {
        TransformFunction::Matrix([m[0][0], m[0][1], m[1][0], m[1][1], m[3][0], m[3][1]])
    } else {
        let mut values = [0.0; 16];
        for c in 0..4 {
            values[c * 4..c * 4 + 4].copy_from_slice(&m[c]);
        }
        TransformFunction::Matrix3d(values)
    }
}

/// Interpolate two matrices through decomposition; `None` when one cannot
/// be decomposed (the caller animates discretely).
fn interpolate_matrices(a: &Matrix, b: &Matrix, t: f64) -> Option<TransformFunction> {
    let (da, db) = (decompose(a)?, decompose(b)?);
    let mix3 = |x: [f64; 3], y: [f64; 3]| {
        [
            lerp(x[0], y[0], t),
            lerp(x[1], y[1], t),
            lerp(x[2], y[2], t),
        ]
    };
    let d = Decomposed {
        translate: mix3(da.translate, db.translate),
        scale: mix3(da.scale, db.scale),
        skew: mix3(da.skew, db.skew),
        perspective: [
            lerp(da.perspective[0], db.perspective[0], t),
            lerp(da.perspective[1], db.perspective[1], t),
            lerp(da.perspective[2], db.perspective[2], t),
            lerp(da.perspective[3], db.perspective[3], t),
        ],
        quaternion: slerp(da.quaternion, db.quaternion, t),
    };
    Some(to_function(&recompose(&d)))
}

fn normalized_axis(x: f64, y: f64, z: f64) -> Option<[f64; 3]> {
    let l = (x * x + y * y + z * z).sqrt();
    (l > 0.0 && l.is_finite()).then(|| [x / l, y / l, z / l])
}

fn quaternion(axis: [f64; 3], degrees: f64) -> [f64; 4] {
    let half = degrees.to_radians() / 2.0;
    let s = half.sin();
    [axis[0] * s, axis[1] * s, axis[2] * s, half.cos()]
}

/// Interpolate two functions that share a primitive (CSS Transforms 2
/// §16); `None` when they do not.
pub(crate) fn interpolate_function(
    a: &TransformFunction,
    b: &TransformFunction,
    t: f64,
) -> Option<TransformFunction> {
    use TransformFunction as F;
    Some(match (a, b) {
        (F::Translate(x1, y1, z1), F::Translate(x2, y2, z2)) => {
            F::Translate(lp(x1, x2, t), lp(y1, y2, t), lp(z1, z2, t))
        }
        (F::Scale(x1, y1, z1), F::Scale(x2, y2, z2)) => {
            F::Scale(lerp(*x1, *x2, t), lerp(*y1, *y2, t), lerp(*z1, *z2, t))
        }
        (F::Skew(x1, y1), F::Skew(x2, y2)) => F::Skew(lerp(*x1, *x2, t), lerp(*y1, *y2, t)),
        (
            F::Rotate {
                x: x1,
                y: y1,
                z: z1,
                angle: a1,
            },
            F::Rotate {
                x: x2,
                y: y2,
                z: z2,
                angle: a2,
            },
        ) => {
            let (n1, n2) = (
                normalized_axis(*x1, *y1, *z1),
                normalized_axis(*x2, *y2, *z2),
            );
            match (n1, n2) {
                (Some(p), Some(q))
                    if (p[0] - q[0]).abs() < 1e-12
                        && (p[1] - q[1]).abs() < 1e-12
                        && (p[2] - q[2]).abs() < 1e-12 =>
                {
                    F::Rotate {
                        x: *x1,
                        y: *y1,
                        z: *z1,
                        angle: lerp(*a1, *a2, t),
                    }
                }
                (Some(p), Some(q)) => {
                    // Different axes: slerp the rotations' quaternions.
                    let r = slerp(quaternion(p, *a1), quaternion(q, *a2), t);
                    let w = r[3].clamp(-1.0, 1.0);
                    let angle = 2.0 * w.acos();
                    let s = (1.0 - w * w).sqrt();
                    if s < 1e-12 {
                        F::Rotate {
                            x: 0.0,
                            y: 0.0,
                            z: 1.0,
                            angle: 0.0,
                        }
                    } else {
                        F::Rotate {
                            x: r[0] / s,
                            y: r[1] / s,
                            z: r[2] / s,
                            angle: angle.to_degrees(),
                        }
                    }
                }
                // A zero axis is no rotation: interpolate the angle about
                // the other axis.
                (None, Some(_)) => F::Rotate {
                    x: *x2,
                    y: *y2,
                    z: *z2,
                    angle: lerp(0.0, *a2, t),
                },
                (Some(_), None) | (None, None) => F::Rotate {
                    x: *x1,
                    y: *y1,
                    z: *z1,
                    angle: lerp(*a1, 0.0, t),
                },
            }
        }
        (F::Perspective(Some(d1)), F::Perspective(Some(d2))) => F::Perspective(Some(lp(d1, d2, t))),
        (F::Perspective(None), F::Perspective(None)) => F::Perspective(None),
        (F::Matrix(_) | F::Matrix3d(_), F::Matrix(_) | F::Matrix3d(_)) => {
            interpolate_matrices(&function_matrix(a)?, &function_matrix(b)?, t)?
        }
        _ => return None,
    })
}

/// The identity function with `like`'s primitive.
fn identity_of(like: &TransformFunction) -> TransformFunction {
    use TransformFunction as F;
    let zero = LengthPercentage::ZERO;
    match like {
        F::Translate(..) => F::Translate(zero.clone(), zero.clone(), zero),
        F::Scale(..) => F::Scale(1.0, 1.0, 1.0),
        F::Rotate { x, y, z, .. } => F::Rotate {
            x: *x,
            y: *y,
            z: *z,
            angle: 0.0,
        },
        F::Skew(..) => F::Skew(0.0, 0.0),
        F::Perspective(_) => F::Perspective(None),
        F::Matrix(_) => F::Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        F::Matrix3d(_) => F::Matrix3d([
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]),
    }
}

/// Interpolate two transform lists (CSS Transforms 2 §10): the shorter
/// list is padded with identity functions; leading pairs that share a
/// primitive interpolate function by function and the rest as one matrix.
pub(crate) fn interpolate_lists(
    a: &TransformList,
    b: &TransformList,
    t: f64,
) -> Option<TransformList> {
    let len = a.0.len().max(b.0.len());
    let pad = |list: &TransformList, other: &TransformList| -> Vec<TransformFunction> {
        let mut out = list.0.clone();
        for f in other.0.iter().skip(out.len()) {
            out.push(identity_of(f));
        }
        out
    };
    let (fa, fb) = (pad(a, b), pad(b, a));
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        match interpolate_function(&fa[i], &fb[i], t) {
            Some(f) => out.push(f),
            None => {
                let (ma, mb) = (list_matrix(&fa[i..])?, list_matrix(&fb[i..])?);
                out.push(interpolate_matrices(&ma, &mb, t)?);
                break;
            }
        }
    }
    Some(TransformList(out))
}

/// Accumulate two lists (Web Animations `accumulate`): matching primitives
/// accumulate per function; otherwise `None` (the effect value replaces).
pub(crate) fn accumulate_lists(a: &TransformList, b: &TransformList) -> Option<TransformList> {
    if a.0.len() != b.0.len() {
        return None;
    }
    a.0.iter()
        .zip(&b.0)
        .map(|(x, y)| accumulate_function(x, y))
        .collect::<Option<Vec<_>>>()
        .map(TransformList)
}

fn accumulate_function(a: &TransformFunction, b: &TransformFunction) -> Option<TransformFunction> {
    use TransformFunction as F;
    let zero = LengthPercentage::ZERO;
    let add = |x: &LengthPercentage, y: &LengthPercentage| Calc::mix(x, 1.0, y, 1.0);
    let _ = zero;
    Some(match (a, b) {
        (F::Translate(x1, y1, z1), F::Translate(x2, y2, z2)) => {
            F::Translate(add(x1, x2), add(y1, y2), add(z1, z2))
        }
        (F::Scale(x1, y1, z1), F::Scale(x2, y2, z2)) => {
            F::Scale(x1 + x2 - 1.0, y1 + y2 - 1.0, z1 + z2 - 1.0)
        }
        (F::Skew(x1, y1), F::Skew(x2, y2)) => F::Skew(x1 + x2, y1 + y2),
        (
            F::Rotate { x, y, z, angle: a1 },
            F::Rotate {
                x: x2,
                y: y2,
                z: z2,
                angle: a2,
            },
        ) if normalized_axis(*x, *y, *z) == normalized_axis(*x2, *y2, *z2) => F::Rotate {
            x: *x,
            y: *y,
            z: *z,
            angle: a1 + a2,
        },
        _ => return None,
    })
}

/// `translate`, `rotate` and `scale` under `add`: translations sum,
/// rotations about one axis sum, scales multiply.
pub(crate) fn accumulate_individual(
    a: &IndividualTransform,
    b: &IndividualTransform,
) -> Option<IndividualTransform> {
    use TransformFunction as F;
    Some(IndividualTransform(match (&a.0, &b.0) {
        (None, other) | (other, None) => other.clone(),
        (Some(F::Scale(x1, y1, z1)), Some(F::Scale(x2, y2, z2))) => {
            Some(F::Scale(x1 * x2, y1 * y2, z1 * z2))
        }
        (Some(x), Some(y)) => Some(accumulate_function(x, y)?),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: &Matrix, b: &Matrix) -> bool {
        a.iter()
            .flatten()
            .zip(b.iter().flatten())
            .all(|(x, y)| (x - y).abs() < 1e-9)
    }

    #[test]
    fn decomposition_round_trips() {
        let cases = [
            TransformFunction::Rotate {
                x: 0.0,
                y: 0.0,
                z: 1.0,
                angle: 30.0,
            },
            TransformFunction::Rotate {
                x: 1.0,
                y: 2.0,
                z: 3.0,
                angle: 75.0,
            },
            TransformFunction::Skew(20.0, 10.0),
            TransformFunction::Scale(2.0, -3.0, 0.5),
            TransformFunction::Matrix([1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            TransformFunction::Perspective(Some(LengthPercentage::Length(rustic_css::Length::px(
                400.0,
            )))),
        ];
        for f in cases {
            let m = function_matrix(&f).unwrap();
            let d = decompose(&m).unwrap();
            assert!(
                near(&recompose(&d), &m),
                "{f:?}: {:?} vs {m:?}",
                recompose(&d)
            );
        }
        let m = list_matrix(&[
            TransformFunction::Translate(
                LengthPercentage::Length(rustic_css::Length::px(10.0)),
                LengthPercentage::Length(rustic_css::Length::px(-4.0)),
                LengthPercentage::ZERO,
            ),
            TransformFunction::Rotate {
                x: 0.0,
                y: 1.0,
                z: 0.0,
                angle: 40.0,
            },
            TransformFunction::Skew(5.0, 0.0),
            TransformFunction::Scale(1.5, 1.5, 1.0),
        ])
        .unwrap();
        let d = decompose(&m).unwrap();
        assert!(near(&recompose(&d), &m));
    }
}
