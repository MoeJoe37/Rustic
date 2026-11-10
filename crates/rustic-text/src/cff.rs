//! Unhinted CFF/CFF2 adapter; cubic conversion is iterative and bounded.
use crate::{FontError, FontLimits, GlyphId, Outline, OutlinePoint, Tag};
use skrifa::{
    FontRef, MetadataProvider,
    instance::{LocationRef, Size},
    outline::{DrawSettings, OutlinePen},
};
type Pt = (f64, f64);
fn mid(a: Pt, b: Pt) -> Pt {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
}
struct Pen {
    out: Outline,
    points: usize,
    limit: usize,
    failed: bool,
    current: Pt,
}
impl Pen {
    fn point(&mut self, p: Pt, on: bool) {
        if self.failed {
            return;
        }
        if self.points >= self.limit || !p.0.is_finite() || !p.1.is_finite() {
            self.failed = true;
            return;
        }
        if let Some(c) = self.out.contours.last_mut() {
            c.push(OutlinePoint {
                x: p.0,
                y: p.1,
                on_curve: on,
            });
            self.points += 1;
        }
        self.current = p;
    }
}
impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.failed {
            return;
        }
        self.out.contours.push(Vec::new());
        self.point((f64::from(x), f64::from(y)), true);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.point((f64::from(x), f64::from(y)), true);
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.point((f64::from(cx), f64::from(cy)), false);
        self.point((f64::from(x), f64::from(y)), true);
    }
    fn curve_to(&mut self, cx: f32, cy: f32, dx: f32, dy: f32, x: f32, y: f32) {
        let mut todo = vec![(
            self.current,
            (f64::from(cx), f64::from(cy)),
            (f64::from(dx), f64::from(dy)),
            (f64::from(x), f64::from(y)),
            0,
        )];
        while let Some((a, b, c, d, depth)) = todo.pop() {
            if self.failed {
                return;
            }
            let flat = (3.0 * b.0 - 2.0 * a.0 - d.0)
                .abs()
                .max((3.0 * c.0 - a.0 - 2.0 * d.0).abs())
                .max((3.0 * b.1 - 2.0 * a.1 - d.1).abs())
                .max((3.0 * c.1 - a.1 - 2.0 * d.1).abs())
                <= 0.125;
            if flat {
                self.point(d, true);
                continue;
            }
            if depth >= 20 {
                self.failed = true;
                return;
            }
            let ab = mid(a, b);
            let bc = mid(b, c);
            let cd = mid(c, d);
            let abc = mid(ab, bc);
            let bcd = mid(bc, cd);
            let m = mid(abc, bcd);
            todo.push((m, bcd, cd, d, depth + 1));
            todo.push((a, ab, abc, m, depth + 1));
        }
    }
    fn close(&mut self) {}
}
pub(crate) fn outline(
    bytes: &[u8],
    glyph: GlyphId,
    limits: &FontLimits,
) -> Result<Outline, FontError> {
    let bad = || FontError::Invalid {
        table: Tag(*b"CFF "),
        reason: "invalid CFF outline",
    };
    let font = FontRef::new(bytes).map_err(|_| bad())?;
    let outlines = font.outline_glyphs();
    let g = outlines
        .get(skrifa::GlyphId::new(u32::from(glyph.0)))
        .ok_or_else(bad)?;
    let mut pen = Pen {
        out: Outline::default(),
        points: 0,
        limit: limits.max_glyph_points,
        failed: false,
        current: (0.0, 0.0),
    };
    g.draw(
        DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
        &mut pen,
    )
    .map_err(|_| bad())?;
    if pen.failed {
        return Err(FontError::LimitExceeded("CFF outline points"));
    }
    Ok(pen.out)
}
