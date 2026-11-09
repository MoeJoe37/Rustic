//! `loca` + `glyf` (OpenType §glyf): TrueType quadratic outlines.
//!
//! Composite glyphs are flattened with an explicit work stack rather than
//! recursion; depth, component count and total points are bounded by
//! [`FontLimits`], which also stops reference cycles.

use crate::FontLimits;
use crate::error::{FontError, Tag};
use crate::reader::{Reader, size};

const TAG: Tag = Tag::GLYF;

fn invalid(reason: &'static str) -> FontError {
    FontError::Invalid { table: TAG, reason }
}

/// One outline point in font units. Off-curve points are quadratic control
/// points; two consecutive off-curve points imply an on-curve midpoint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutlinePoint {
    /// Horizontal position, in font units.
    pub x: f64,
    /// Vertical position, in font units (y grows upward).
    pub y: f64,
    /// Whether the point lies on the curve.
    pub on_curve: bool,
}

/// A glyph outline: closed contours in font units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outline {
    contours: Vec<Vec<OutlinePoint>>,
}

/// An axis-aligned bounding box in font units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    /// Minimum x.
    pub x_min: f64,
    /// Minimum y.
    pub y_min: f64,
    /// Maximum x.
    pub x_max: f64,
    /// Maximum y.
    pub y_max: f64,
}

impl Outline {
    /// The contours, each a closed sequence of points.
    pub fn contours(&self) -> &[Vec<OutlinePoint>] {
        &self.contours
    }

    /// Whether the glyph has no contours (for example a space).
    pub fn is_empty(&self) -> bool {
        self.contours.is_empty()
    }

    /// The bounds of every point (control points included), or `None` for
    /// an empty outline.
    pub fn bounds(&self) -> Option<Bounds> {
        let mut points = self.contours.iter().flatten();
        let first = points.next()?;
        let mut b = Bounds {
            x_min: first.x,
            y_min: first.y,
            x_max: first.x,
            y_max: first.y,
        };
        for p in points {
            b.x_min = b.x_min.min(p.x);
            b.y_min = b.y_min.min(p.y);
            b.x_max = b.x_max.max(p.x);
            b.y_max = b.y_max.max(p.y);
        }
        Some(b)
    }
}

/// The validated location table.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Loca {
    long: bool,
    glyphs: u16,
}

impl Loca {
    pub(crate) fn parse(loca: &[u8], long: bool, glyphs: u16) -> Result<Self, FontError> {
        let entry = if long { 4 } else { 2 };
        if loca.len() < (usize::from(glyphs) + 1) * entry {
            return Err(FontError::Truncated(Tag::LOCA));
        }
        Ok(Self { long, glyphs })
    }

    /// The byte range of glyph `id` in `glyf`, checked against its length.
    pub(crate) fn range(
        &self,
        loca: &[u8],
        glyf_len: usize,
        id: u16,
    ) -> Result<(usize, usize), FontError> {
        if id >= self.glyphs {
            return Err(FontError::GlyphOutOfRange(u32::from(id)));
        }
        let i = usize::from(id);
        let (start, end) = if self.long {
            let mut r = Reader::at(loca, i * 4, Tag::LOCA)?;
            (size(r.u32()?), size(r.u32()?))
        } else {
            let mut r = Reader::at(loca, i * 2, Tag::LOCA)?;
            (usize::from(r.u16()?) * 2, usize::from(r.u16()?) * 2)
        };
        if start > end {
            return Err(FontError::Invalid {
                table: Tag::LOCA,
                reason: "glyph offsets decrease",
            });
        }
        if end > glyf_len {
            return Err(FontError::TableOutOfBounds(TAG));
        }
        Ok((start, end))
    }
}

/// An affine transform `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Clone, Copy, Debug)]
struct Transform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Transform {
    const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// `self ∘ inner`: apply `inner`, then `self`.
    fn then(&self, inner: &Self) -> Self {
        let (e, f) = self.apply(inner.e, inner.f);
        Self {
            a: self.a * inner.a + self.c * inner.b,
            b: self.b * inner.a + self.d * inner.b,
            c: self.a * inner.c + self.c * inner.d,
            d: self.b * inner.c + self.d * inner.d,
            e,
            f,
        }
    }
}

/// The glyph header's bounding box (yMax is used for x-height and cap
/// height when OS/2 lacks them), or `None` for an empty glyph.
pub(crate) fn header_bounds(glyph: &[u8]) -> Result<Option<[i16; 4]>, FontError> {
    if glyph.is_empty() {
        return Ok(None);
    }
    let mut r = Reader::new(glyph, TAG);
    r.i16()?;
    let b = [r.i16()?, r.i16()?, r.i16()?, r.i16()?];
    if b[0] > b[2] || b[1] > b[3] {
        return Err(invalid("glyph bounding box is inverted"));
    }
    Ok(Some(b))
}

// Simple glyph flags.
const ON_CURVE: u8 = 0x01;
const X_SHORT: u8 = 0x02;
const Y_SHORT: u8 = 0x04;
const REPEAT: u8 = 0x08;
const X_SAME_OR_POSITIVE: u8 = 0x10;
const Y_SAME_OR_POSITIVE: u8 = 0x20;

// Composite glyph flags.
const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const ARGS_ARE_XY_VALUES: u16 = 0x0002;
const WE_HAVE_A_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;
const SCALED_COMPONENT_OFFSET: u16 = 0x0800;
const UNSCALED_COMPONENT_OFFSET: u16 = 0x1000;

fn f2dot14(r: &mut Reader<'_>) -> Result<f64, FontError> {
    Ok(f64::from(r.i16()?) / 16384.0)
}

/// Bounded totals for one outline request.
struct Budget<'l> {
    limits: &'l FontLimits,
    points: usize,
    components: usize,
}

/// Append the simple glyph's contours, transformed, to `out`.
fn simple(
    glyph: &[u8],
    contours: usize,
    transform: &Transform,
    budget: &mut Budget<'_>,
    out: &mut Vec<Vec<OutlinePoint>>,
) -> Result<(), FontError> {
    let mut r = Reader::at(glyph, 10, TAG)?;
    let mut ends = Vec::with_capacity(contours.min(glyph.len() / 2));
    let mut previous: Option<u16> = None;
    for _ in 0..contours {
        let end = r.u16()?;
        if previous.is_some_and(|p| end <= p) {
            return Err(invalid("contour end points are not increasing"));
        }
        previous = Some(end);
        ends.push(usize::from(end));
    }
    let points = previous.map_or(0, |p| usize::from(p) + 1);
    budget.points = budget
        .points
        .checked_add(points)
        .filter(|&n| n <= budget.limits.max_glyph_points)
        .ok_or(FontError::LimitExceeded("outline points"))?;
    let instructions = usize::from(r.u16()?);
    r.skip(instructions)?;

    let mut flags = Vec::with_capacity(points);
    while flags.len() < points {
        let flag = r.u8()?;
        flags.push(flag);
        if flag & REPEAT != 0 {
            let count = usize::from(r.u8()?);
            if flags.len() + count > points {
                return Err(invalid("flag repeat runs past the last point"));
            }
            flags.extend(std::iter::repeat_n(flag, count));
        }
    }
    let mut xs = Vec::with_capacity(points);
    let mut value = 0i32;
    for &flag in &flags {
        value += coordinate(&mut r, flag, X_SHORT, X_SAME_OR_POSITIVE)?;
        xs.push(value);
    }
    let mut start = 0;
    let mut value = 0i32;
    let mut contour = Vec::new();
    let mut end_iter = ends.iter().copied();
    let mut next_end = end_iter.next();
    for (i, (&flag, &x)) in flags.iter().zip(&xs).enumerate() {
        value += coordinate(&mut r, flag, Y_SHORT, Y_SAME_OR_POSITIVE)?;
        let (x, y) = transform.apply(f64::from(x), f64::from(value));
        contour.push(OutlinePoint {
            x,
            y,
            on_curve: flag & ON_CURVE != 0,
        });
        if Some(i) == next_end {
            out.push(std::mem::take(&mut contour));
            start = i + 1;
            next_end = end_iter.next();
        }
    }
    debug_assert_eq!(start, points);
    Ok(())
}

/// One delta coordinate; deltas are at most ±32,767 per point and at most
/// 65,535 points per glyph, so the running `i32` sum cannot overflow.
fn coordinate(r: &mut Reader<'_>, flag: u8, short: u8, same: u8) -> Result<i32, FontError> {
    Ok(if flag & short != 0 {
        let v = i32::from(r.u8()?);
        if flag & same != 0 { v } else { -v }
    } else if flag & same != 0 {
        0
    } else {
        i32::from(r.i16()?)
    })
}

/// Parse glyph `id`'s outline, flattening composite glyphs.
pub(crate) fn outline(
    loca: &Loca,
    loca_data: &[u8],
    glyf: &[u8],
    id: u16,
    limits: &FontLimits,
) -> Result<Outline, FontError> {
    let mut budget = Budget {
        limits,
        points: 0,
        components: 0,
    };
    let mut contours = Vec::new();
    // (glyph, transform, depth); popped in source order.
    let mut stack = vec![(id, Transform::IDENTITY, 0usize)];
    while let Some((id, transform, depth)) = stack.pop() {
        let (start, end) = loca.range(loca_data, glyf.len(), id)?;
        let glyph = &glyf[start..end];
        if glyph.is_empty() {
            continue;
        }
        header_bounds(glyph)?;
        let count = Reader::new(glyph, TAG).i16()?;
        if let Ok(count) = usize::try_from(count) {
            simple(glyph, count, &transform, &mut budget, &mut contours)?;
            continue;
        }
        if depth >= limits.max_component_depth {
            return Err(FontError::LimitExceeded("composite glyph depth"));
        }
        let mut r = Reader::at(glyph, 10, TAG)?;
        let mut children = Vec::new();
        loop {
            budget.components += 1;
            if budget.components > limits.max_components {
                return Err(FontError::LimitExceeded("composite glyph components"));
            }
            let flags = r.u16()?;
            let child = r.u16()?;
            if flags & ARGS_ARE_XY_VALUES == 0 {
                return Err(FontError::Unsupported("composite point matching"));
            }
            let (dx, dy) = if flags & ARG_1_AND_2_ARE_WORDS != 0 {
                (f64::from(r.i16()?), f64::from(r.i16()?))
            } else {
                (f64::from(r.i8()?), f64::from(r.i8()?))
            };
            let (mut a, mut b, mut c, mut d) = (1.0, 0.0, 0.0, 1.0);
            if flags & WE_HAVE_A_SCALE != 0 {
                a = f2dot14(&mut r)?;
                d = a;
            } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
                a = f2dot14(&mut r)?;
                d = f2dot14(&mut r)?;
            } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
                a = f2dot14(&mut r)?;
                b = f2dot14(&mut r)?;
                c = f2dot14(&mut r)?;
                d = f2dot14(&mut r)?;
            }
            // OpenType default: offsets are not scaled, unless the font
            // explicitly asks for Apple's scaled-offset behavior.
            let scaled =
                flags & SCALED_COMPONENT_OFFSET != 0 && flags & UNSCALED_COMPONENT_OFFSET == 0;
            let (e, f) = if scaled {
                (a * dx + c * dy, b * dx + d * dy)
            } else {
                (dx, dy)
            };
            let local = Transform { a, b, c, d, e, f };
            children.push((child, transform.then(&local), depth + 1));
            if flags & MORE_COMPONENTS == 0 {
                break;
            }
        }
        // Trailing instructions are ignored (no hinting).
        stack.extend(children.into_iter().rev());
    }
    Ok(Outline { contours })
}
