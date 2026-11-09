//! Deterministic CPU reference rasterizer: exact-area antialiasing and
//! source-over compositing into a checked premultiplied RGBA8 surface.
//!
//! Coverage is the exact area of each pixel inside the shape (signed-area
//! accumulation, nonzero winding clamped to one), computed in `f64` with no
//! platform math calls, so output bytes are identical on every target.

use rustic_base::CancellationToken;
use rustic_layout::Clip;
use rustic_layout::{CssRect, DeviceScale};
use rustic_paint::{DisplayCommand, DisplayItem, DisplayList, Shape, Srgba};
use std::{error::Error, fmt};

/// Surface and work budgets for one rasterization.
#[derive(Clone, Copy, Debug)]
pub struct RasterLimits {
    /// Maximum surface width in device pixels.
    pub max_width: u32,
    /// Maximum surface height in device pixels.
    pub max_height: u32,
    /// Maximum surface bytes (4 per pixel), counting the surface and every
    /// opacity group buffer alive at the same time.
    pub max_bytes: usize,
    /// Maximum pixel visits summed over every item's clipped bounds.
    pub max_work: u64,
}

impl Default for RasterLimits {
    fn default() -> Self {
        Self {
            max_width: 16_384,
            max_height: 16_384,
            max_bytes: 256 * 1024 * 1024,
            max_work: 1 << 30,
        }
    }
}

/// An atomic rasterization failure; no partial surface is returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterError {
    /// Zero, nonfinite or over-budget surface dimensions.
    SurfaceSize,
    /// Pixel visits exceeded [`RasterLimits::max_work`].
    WorkLimit,
    /// Nested opacity groups would exceed [`RasterLimits::max_bytes`].
    MemoryLimit,
    /// Unbalanced group markers in the display list.
    InvalidList,
    /// Cooperative cancellation.
    Cancelled,
}

impl fmt::Display for RasterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SurfaceSize => "surface size is zero or exceeds its budget",
            Self::WorkLimit => "rasterization work limit exceeded",
            Self::MemoryLimit => "opacity groups exceed the surface memory budget",
            Self::InvalidList => "display list has unbalanced groups",
            Self::Cancelled => "rasterization cancelled",
        })
    }
}

impl Error for RasterError {}

/// A premultiplied RGBA8 pixel surface, rows top to bottom.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Surface {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Surface {
    /// Allocate a transparent surface after checking every budget.
    pub fn new(width: u32, height: u32, limits: RasterLimits) -> Result<Self, RasterError> {
        if width == 0 || height == 0 || width > limits.max_width || height > limits.max_height {
            return Err(RasterError::SurfaceSize);
        }
        let bytes = usize::try_from(width)
            .ok()
            .zip(usize::try_from(height).ok())
            .and_then(|(w, h)| w.checked_mul(h)?.checked_mul(4))
            .filter(|&n| n <= limits.max_bytes)
            .ok_or(RasterError::SurfaceSize)?;
        Ok(Self {
            width,
            height,
            pixels: vec![0; bytes],
        })
    }

    /// Width in device pixels.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in device pixels.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Premultiplied RGBA bytes, row-major with no padding.
    pub fn premultiplied(&self) -> &[u8] {
        &self.pixels
    }

    /// One pixel as straight-alpha sRGB, or `None` outside the surface.
    pub fn pixel(&self, x: u32, y: u32) -> Option<Srgba> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let p = &self.pixels[i..i + 4];
        Some(unpremultiply([p[0], p[1], p[2], p[3]]))
    }

    #[cfg(test)]
    pub(crate) fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    fn fill_opaque(&mut self, color: Srgba) {
        for p in self.pixels.chunks_exact_mut(4) {
            p.copy_from_slice(&[color.r, color.g, color.b, 255]);
        }
    }
}

pub(crate) fn unpremultiply(p: [u8; 4]) -> Srgba {
    let [r, g, b, a] = p;
    if a == 0 {
        return Srgba {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        };
    }
    let channel = |c: u8| {
        let v = (u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a);
        v.min(255) as u8
    };
    Srgba {
        r: channel(r),
        g: channel(g),
        b: channel(b),
        a,
    }
}

type Point = (f64, f64);

/// Device-space clip rectangle as (left, top, right, bottom).
type Bounds = (f64, f64, f64, f64);

/// Sutherland–Hodgman clip of one closed contour to `bounds`.
fn clip(contour: &[Point], bounds: Bounds) -> Vec<Point> {
    let mut points = contour.to_vec();
    let (x0, y0, x1, y1) = bounds;
    // (axis, bound, keep_greater)
    for (axis, bound, greater) in [(0, x0, true), (0, x1, false), (1, y0, true), (1, y1, false)] {
        if points.is_empty() {
            break;
        }
        let inside = |p: Point| {
            let v = if axis == 0 { p.0 } else { p.1 };
            if greater { v >= bound } else { v <= bound }
        };
        let cross = |a: Point, b: Point| {
            let (av, bv) = if axis == 0 { (a.0, b.0) } else { (a.1, b.1) };
            let t = (bound - av) / (bv - av);
            let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            if axis == 0 { (bound, y) } else { (x, bound) }
        };
        let input = std::mem::take(&mut points);
        let mut previous = input[input.len() - 1];
        for &current in &input {
            match (inside(previous), inside(current)) {
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

/// Add one edge's signed area to a `stride`-wide accumulation buffer.
/// Every coordinate is already clipped to `[0,w]×[0,h]`.
fn accumulate_edge(acc: &mut [f64], stride: usize, h: usize, p0: Point, p1: Point) {
    if p0.1 == p1.1 {
        return;
    }
    let (dir, p0, p1) = if p0.1 < p1.1 {
        (1.0, p0, p1)
    } else {
        (-1.0, p1, p0)
    };
    let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
    let mut x = p0.0;
    let first = p0.1.floor().max(0.0) as usize;
    let last = (p1.1.ceil() as usize).min(h);
    for row in first..last {
        let top = (row as f64).max(p0.1);
        let bottom = ((row + 1) as f64).min(p1.1);
        let dy = bottom - top;
        if dy <= 0.0 {
            continue;
        }
        let x_next = x + dxdy * dy;
        let d = dy * dir;
        let (xa, xb) = if x < x_next { (x, x_next) } else { (x_next, x) };
        let line = &mut acc[row * stride..(row + 1) * stride];
        let xa_floor = xa.floor();
        let ia = xa_floor as usize;
        let ib = xb.ceil() as usize;
        if ib <= ia + 1 {
            // The edge stays within one pixel column on this row.
            let mid = 0.5 * (x + x_next) - xa_floor;
            line[ia] += d - d * mid;
            line[ia + 1] += d * mid;
        } else {
            let s = 1.0 / (xb - xa);
            let fa = xa - xa_floor;
            let a0 = 0.5 * s * (1.0 - fa) * (1.0 - fa);
            let fb = xb - ib as f64 + 1.0;
            let am = 0.5 * s * fb * fb;
            line[ia] += d * a0;
            if ib == ia + 2 {
                line[ia + 1] += d * (1.0 - a0 - am);
            } else {
                let a1 = s * (1.5 - fa);
                line[ia + 1] += d * (a1 - a0);
                for cell in &mut line[ia + 2..ib - 1] {
                    *cell += d * s;
                }
                let a2 = a1 + (ib - ia - 3) as f64 * s;
                line[ib - 1] += d * (1.0 - a2 - am);
            }
            line[ib] += d * am;
        }
        x = x_next;
    }
}

fn rect_points(r: CssRect, scale: f64, reverse: bool) -> Vec<Point> {
    let o = r.origin();
    let (x0, y0) = (o.x.get() * scale, o.y.get() * scale);
    let x1 = (o.x.get() + r.size().width().get()) * scale;
    let y1 = (o.y.get() + r.size().height().get()) * scale;
    let mut points = vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    if reverse {
        points.reverse();
    }
    points
}

fn contours(shape: &Shape, scale: f64) -> Vec<Vec<Point>> {
    match *shape {
        Shape::Rect(r) => vec![rect_points(r, scale, false)],
        Shape::Ring { outer, inner } => {
            vec![
                rect_points(outer, scale, false),
                rect_points(inner, scale, true),
            ]
        }
        Shape::Quad(points) => vec![
            points
                .iter()
                .map(|p| (p.x.get() * scale, p.y.get() * scale))
                .collect(),
        ],
    }
}

struct Work<'a> {
    used: u64,
    limit: u64,
    cancellation: &'a CancellationToken,
}

impl Work<'_> {
    fn charge(&mut self, n: u64) -> Result<(), RasterError> {
        self.cancellation
            .check()
            .map_err(|_| RasterError::Cancelled)?;
        self.used = self
            .used
            .checked_add(n)
            .filter(|&u| u <= self.limit)
            .ok_or(RasterError::WorkLimit)?;
        Ok(())
    }
}

/// The device-space area an item may touch: the surface intersected with its
/// overflow clip. `None` when nothing remains.
fn item_bounds(clip_rect: Clip, w: f64, h: f64, scale: f64) -> Option<Bounds> {
    let mut b: Bounds = (0.0, 0.0, w, h);
    if let Some(r) = clip_rect.x {
        b.0 = b.0.max(r.start.get() * scale);
        b.2 = b.2.min(r.end.get() * scale);
    }
    if let Some(r) = clip_rect.y {
        b.1 = b.1.max(r.start.get() * scale);
        b.3 = b.3.min(r.end.get() * scale);
    }
    (b.0 < b.2 && b.1 < b.3).then_some(b)
}

fn fill(
    pixels: &mut [u8],
    (width, height): (u32, u32),
    item: &DisplayItem,
    scale: f64,
    work: &mut Work<'_>,
) -> Result<(), RasterError> {
    let (w, h) = (f64::from(width), f64::from(height));
    let Some(limit) = item_bounds(item.clip, w, h, scale) else {
        return Ok(());
    };
    let color = item.color;
    let clipped: Vec<Vec<Point>> = contours(&item.shape, scale)
        .iter()
        .map(|c| clip(c, limit))
        .filter(|c| c.len() >= 3)
        .collect();
    let mut bounds = (w, h, 0.0_f64, 0.0_f64);
    for p in clipped.iter().flatten() {
        bounds = (
            bounds.0.min(p.0),
            bounds.1.min(p.1),
            bounds.2.max(p.0),
            bounds.3.max(p.1),
        );
    }
    if bounds.0 >= bounds.2 || bounds.1 >= bounds.3 {
        return Ok(());
    }
    // Work in a local buffer covering only the item's pixel bounds.
    let bx = bounds.0.floor();
    let by = bounds.1.floor();
    let bw = (bounds.2.ceil() - bx) as usize;
    let bh = (bounds.3.ceil() - by) as usize;
    work.charge((bw as u64 + 2) * bh as u64)?;
    let stride = bw + 2;
    let mut acc = vec![0.0_f64; stride * bh];
    for contour in &clipped {
        let mut previous = contour[contour.len() - 1];
        for &current in contour {
            accumulate_edge(
                &mut acc,
                stride,
                bh,
                (previous.0 - bx, previous.1 - by),
                (current.0 - bx, current.1 - by),
            );
            previous = current;
        }
    }
    let alpha = f64::from(color.a) / 255.0;
    let premultiplied = [
        f64::from(color.r) * alpha,
        f64::from(color.g) * alpha,
        f64::from(color.b) * alpha,
        f64::from(color.a),
    ];
    let width = width as usize;
    let (ox, oy) = (bx as usize, by as usize);
    for row in 0..bh {
        work.cancellation
            .check()
            .map_err(|_| RasterError::Cancelled)?;
        let mut sum = 0.0;
        let start = ((oy + row) * width + ox) * 4;
        for col in 0..bw {
            sum += acc[row * stride + col];
            let coverage = sum.abs().min(1.0);
            if coverage <= 0.0 {
                continue;
            }
            let i = start + col * 4;
            let keep = 1.0 - alpha * coverage;
            for (dst, &src) in pixels[i..i + 4].iter_mut().zip(&premultiplied) {
                let v = src * coverage + f64::from(*dst) * keep;
                *dst = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Ok(())
}

/// Composite a premultiplied group onto `dst` source-over at `opacity`.
fn composite(dst: &mut [u8], group: &[u8], opacity: f64) {
    for (d, g) in dst.chunks_exact_mut(4).zip(group.chunks_exact(4)) {
        if g[3] == 0 {
            continue;
        }
        let keep = 1.0 - f64::from(g[3]) * opacity / 255.0;
        for c in 0..4 {
            let v = f64::from(g[c]) * opacity + f64::from(d[c]) * keep;
            d[c] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// Rasterize `list` at `scale` device pixels per CSS pixel.
///
/// The surface is the viewport size times `scale`, rounded up, cleared to the
/// list's opaque canvas color. Items are composited source-over in order with
/// exact-area antialiasing; edges are not snapped to device pixels.
pub fn rasterize(
    list: &DisplayList,
    scale: DeviceScale,
    limits: RasterLimits,
    cancellation: &CancellationToken,
) -> Result<Surface, RasterError> {
    cancellation.check().map_err(|_| RasterError::Cancelled)?;
    let device = |css: f64| -> Result<u32, RasterError> {
        let v = rustic_layout::CssPx::new(css)
            .map_err(|_| RasterError::SurfaceSize)?
            .to_device(scale)
            .get()
            .ceil();
        if v.is_finite() && v >= 1.0 && v <= f64::from(u32::MAX) {
            Ok(v as u32)
        } else {
            Err(RasterError::SurfaceSize)
        }
    };
    let viewport = list.viewport();
    let width = device(viewport.width().get())?;
    let height = device(viewport.height().get())?;
    let mut surface = Surface::new(width, height, limits)?;
    let mut work = Work {
        used: 0,
        limit: limits.max_work,
        cancellation,
    };
    work.charge(u64::from(width) * u64::from(height))?;
    surface.fill_opaque(list.canvas());
    let factor = rustic_layout::CssPx::new(1.0)
        .map_err(|_| RasterError::SurfaceSize)?
        .to_device(scale)
        .get();
    let size = (surface.width, surface.height);
    let bytes = surface.pixels.len();
    // Open opacity groups, innermost last, with their opacity.
    let mut groups: Vec<(Vec<u8>, f64)> = Vec::new();
    for command in list.commands() {
        match command {
            DisplayCommand::Fill(item) => {
                let target = match groups.last_mut() {
                    Some((pixels, _)) => pixels,
                    None => &mut surface.pixels,
                };
                fill(target, size, item, factor, &mut work)?;
            }
            DisplayCommand::BeginGroup { opacity, .. } => {
                // The surface, every open group and the new one.
                if bytes
                    .checked_mul(groups.len() + 2)
                    .is_none_or(|n| n > limits.max_bytes)
                {
                    return Err(RasterError::MemoryLimit);
                }
                work.charge(u64::from(size.0) * u64::from(size.1))?;
                groups.push((vec![0; bytes], *opacity));
            }
            DisplayCommand::EndGroup => {
                let (group, opacity) = groups.pop().ok_or(RasterError::InvalidList)?;
                work.charge(u64::from(size.0) * u64::from(size.1))?;
                let target = match groups.last_mut() {
                    Some((pixels, _)) => pixels,
                    None => &mut surface.pixels,
                };
                composite(target, &group, opacity);
            }
        }
    }
    if !groups.is_empty() {
        return Err(RasterError::InvalidList);
    }
    Ok(surface)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coverage(contour: &[Point], w: usize, h: usize) -> Vec<f64> {
        let stride = w + 2;
        let mut acc = vec![0.0; stride * h];
        let clipped = clip(contour, (0.0, 0.0, w as f64, h as f64));
        let mut previous = clipped[clipped.len() - 1];
        for &current in &clipped {
            accumulate_edge(&mut acc, stride, h, previous, current);
            previous = current;
        }
        let mut out = Vec::new();
        for row in 0..h {
            let mut sum = 0.0;
            for col in 0..w {
                sum += acc[row * stride + col];
                out.push(sum.abs().min(1.0));
            }
        }
        out
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    #[test]
    fn pixel_aligned_rectangles_cover_exactly_whole_pixels() {
        let c = coverage(&[(1.0, 1.0), (3.0, 1.0), (3.0, 2.0), (1.0, 2.0)], 4, 3);
        let expected = [0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0];
        for (got, want) in c.iter().zip(expected) {
            assert!(close(*got, f64::from(want)), "{c:?}");
        }
    }

    #[test]
    fn fractional_edges_cover_their_exact_area() {
        let c = coverage(&[(0.25, 0.5), (2.75, 0.5), (2.75, 1.0), (0.25, 1.0)], 3, 1);
        assert!(
            close(c[0], 0.375) && close(c[1], 0.5) && close(c[2], 0.375),
            "{c:?}"
        );
    }

    #[test]
    fn diagonal_edges_split_pixels_by_area_and_sum_to_the_polygon_area() {
        // Right triangle with legs of 4 pixels: total area 8.
        let c = coverage(&[(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)], 4, 4);
        assert!(close(c.iter().sum::<f64>(), 8.0), "{c:?}");
        assert!(close(c[0], 1.0) && close(c[3], 0.5) && close(c[15], 0.0));
        // Winding direction does not matter.
        let r = coverage(&[(0.0, 4.0), (4.0, 0.0), (0.0, 0.0)], 4, 4);
        assert_eq!(c, r);
    }

    #[test]
    fn shapes_outside_or_straddling_the_surface_are_clipped() {
        let c = coverage(&[(-5.0, -5.0), (2.0, -5.0), (2.0, 1.5), (-5.0, 1.5)], 3, 2);
        let expected = [1.0, 1.0, 0.0, 0.5, 0.5, 0.0];
        for (got, want) in c.iter().zip(expected) {
            assert!(close(*got, want), "{c:?}");
        }
        assert!(clip(&[(5.0, 5.0), (6.0, 5.0), (6.0, 6.0)], (0.0, 0.0, 3.0, 2.0)).is_empty());
    }

    #[test]
    fn unpremultiply_rounds_and_handles_transparency() {
        assert_eq!(unpremultiply([0, 0, 0, 0]).a, 0);
        assert_eq!(
            unpremultiply([64, 0, 128, 128]),
            Srgba {
                r: 128,
                g: 0,
                b: 255,
                a: 128
            }
        );
    }

    #[test]
    fn surface_budgets_are_checked_before_allocation() {
        let limits = RasterLimits {
            max_width: 10,
            max_height: 10,
            max_bytes: 400,
            max_work: 1,
        };
        assert!(Surface::new(10, 10, limits).is_ok());
        assert_eq!(Surface::new(0, 1, limits), Err(RasterError::SurfaceSize));
        assert_eq!(Surface::new(11, 1, limits), Err(RasterError::SurfaceSize));
        let small = RasterLimits {
            max_bytes: 399,
            ..limits
        };
        assert_eq!(Surface::new(10, 10, small), Err(RasterError::SurfaceSize));
        assert_eq!(
            Surface::new(
                u32::MAX,
                u32::MAX,
                RasterLimits {
                    max_width: u32::MAX,
                    max_height: u32::MAX,
                    max_bytes: usize::MAX,
                    max_work: 1,
                }
            ),
            Err(RasterError::SurfaceSize)
        );
    }
}
