//! Deterministic CPU reference rasterizer: exact-area antialiasing and
//! source-over compositing into a checked premultiplied RGBA8 surface.
//!
//! Coverage is the exact area of each pixel inside the shape (signed-area
//! accumulation, nonzero winding clamped to one), computed in `f64` with no
//! platform math calls, so output bytes are identical on every target.

use rustic_base::CancellationToken;
use rustic_layout::Clip;
use rustic_layout::{CssRect, DeviceScale};
use rustic_paint::{
    ClipNode, DisplayCommand, DisplayItem, DisplayList, Homography, Image, Path, PathPoint,
    RoundedRect, Shape, Srgba,
};
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
        // Stepping can stray an ulp outside the clipped range; a negative
        // x would floor to column -1 and shift the edge a whole pixel.
        let x_next = (x + dxdy * dy).max(0.0);
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

/// Largest distance a flattened quadratic may stray from the curve, in
/// device pixels.
const FLATNESS: f64 = 0.05;
/// Most line segments one quadratic becomes.
const MAX_SEGMENTS: u32 = 128;

/// Append the flattening of the quadratic `p0`–`c`–`p1` (excluding `p0`).
fn quadratic(out: &mut Vec<Point>, p0: Point, c: Point, p1: Point) {
    // The curve deviates from its chord by at most |p0 - 2c + p1| / 4, and
    // splitting into n parts divides that by n².
    let dx = p0.0 - 2.0 * c.0 + p1.0;
    let dy = p0.1 - 2.0 * c.1 + p1.1;
    let deviation = (dx * dx + dy * dy).sqrt() / 4.0;
    let n = (deviation / FLATNESS).sqrt().ceil();
    let n = if n.is_finite() && n >= 1.0 {
        (n as u32).min(MAX_SEGMENTS)
    } else {
        1
    };
    for k in 1..=n {
        let t = f64::from(k) / f64::from(n);
        let u = 1.0 - t;
        out.push((
            u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p1.0,
            u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p1.1,
        ));
    }
}

/// Flatten one TrueType-style contour (quadratic, implied on-curve
/// midpoints between consecutive control points) into a polygon.
fn flatten(contour: &[PathPoint], scale: f64) -> Vec<Point> {
    let points: Vec<(Point, bool)> = contour
        .iter()
        .map(|p| ((p.x * scale, p.y * scale), p.on_curve))
        .collect();
    let n = points.len();
    let mid = |a: Point, b: Point| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    // Start on an on-curve point, or the midpoint of the first two controls.
    let (start, first) = match points.iter().position(|p| p.1) {
        Some(i) => (points[i].0, i + 1),
        None => (mid(points[0].0, points[n - 1].0), 0),
    };
    let mut out = vec![start];
    let mut current = start;
    let mut control: Option<Point> = None;
    for k in 0..n {
        let (p, on_curve) = points[(first + k) % n];
        match (on_curve, control) {
            (true, None) => {
                out.push(p);
                current = p;
            }
            (true, Some(c)) => {
                quadratic(&mut out, current, c, p);
                current = p;
                control = None;
            }
            (false, None) => control = Some(p),
            (false, Some(c)) => {
                let m = mid(c, p);
                quadratic(&mut out, current, c, m);
                current = m;
                control = Some(p);
            }
        }
    }
    if let Some(c) = control {
        quadratic(&mut out, current, c, start);
    }
    out
}

fn contours(shape: &Shape, paths: &[Path], rounded: &[RoundedRect], scale: f64) -> Vec<Vec<Point>> {
    match *shape {
        // Images are drawn by `draw_image`, not filled.
        Shape::Image { .. } => Vec::new(),
        Shape::Path(index) => paths.get(index).map_or_else(Vec::new, |path| {
            path.contours()
                .iter()
                .filter(|c| !c.is_empty())
                .map(|c| flatten(c, scale))
                .collect()
        }),
        Shape::Rect(r) => vec![rect_points(r, scale, false)],
        Shape::Ring { outer, inner } => {
            vec![
                rect_points(outer, scale, false),
                rect_points(inner, scale, true),
            ]
        }
        Shape::Rounded(r) => rounded
            .get(r)
            .map_or_else(Vec::new, |r| vec![flatten(&r.contour(), scale)]),
        Shape::RoundedRing { outer, inner } => match (rounded.get(outer), rounded.get(inner)) {
            (Some(outer), Some(inner)) => {
                let mut hole = flatten(&inner.contour(), scale);
                hole.reverse();
                vec![flatten(&outer.contour(), scale), hole]
            }
            _ => Vec::new(),
        },
        Shape::Quad(points) => vec![
            points
                .iter()
                .map(|p| (p.x.get() * scale, p.y.get() * scale))
                .collect(),
        ],
    }
}

/// The shape of an item as contours in its local CSS pixels, before any
/// transform.
fn local_contours(shape: &Shape, list: &DisplayList) -> Vec<Vec<PathPoint>> {
    let on = |x: f64, y: f64| PathPoint {
        x,
        y,
        on_curve: true,
    };
    let rect = |r: CssRect, reverse: bool| {
        let (x0, y0) = (r.origin().x.get(), r.origin().y.get());
        let (x1, y1) = (x0 + r.size().width().get(), y0 + r.size().height().get());
        let mut points = vec![on(x0, y0), on(x1, y0), on(x1, y1), on(x0, y1)];
        if reverse {
            points.reverse();
        }
        points
    };
    match *shape {
        Shape::Image { .. } => Vec::new(),
        Shape::Path(index) => list.paths().get(index).map_or_else(Vec::new, |path| {
            path.contours()
                .iter()
                .filter(|c| !c.is_empty())
                .cloned()
                .collect()
        }),
        Shape::Rect(r) => vec![rect(r, false)],
        Shape::Ring { outer, inner } => vec![rect(outer, false), rect(inner, true)],
        Shape::Rounded(r) => list
            .rounded()
            .get(r)
            .map_or_else(Vec::new, |r| vec![r.contour()]),
        Shape::RoundedRing { outer, inner } => {
            match (list.rounded().get(outer), list.rounded().get(inner)) {
                (Some(outer), Some(inner)) => {
                    let mut hole = inner.contour();
                    hole.reverse();
                    vec![outer.contour(), hole]
                }
                _ => Vec::new(),
            }
        }
        Shape::Quad(points) => vec![points.iter().map(|p| on(p.x.get(), p.y.get())).collect()],
    }
}

/// Smallest homogeneous w kept when projecting: geometry at or behind the
/// viewer's plane is cut off (CSS Transforms 2 §6.1).
const W_MIN: f64 = 1e-6;

/// Keep the part of a closed polygon where `a·x + b·y + c >= 0`.
fn clip_half_plane(points: &[Point], (a, b, c): (f64, f64, f64)) -> Vec<Point> {
    let value = |p: Point| a * p.0 + b * p.1 + c;
    if points.iter().all(|&p| value(p) >= 0.0) {
        return points.to_vec();
    }
    let mut out = Vec::with_capacity(points.len() + 2);
    let Some(&last) = points.last() else {
        return out;
    };
    let mut previous = last;
    for &current in points {
        let (vp, vc) = (value(previous), value(current));
        let cross = || {
            let t = vp / (vp - vc);
            (
                previous.0 + (current.0 - previous.0) * t,
                previous.1 + (current.1 - previous.1) * t,
            )
        };
        match (vp >= 0.0, vc >= 0.0) {
            (true, true) => out.push(current),
            (true, false) => out.push(cross()),
            (false, true) => {
                out.push(cross());
                out.push(current);
            }
            (false, false) => {}
        }
        previous = current;
    }
    out
}

/// The most device pixels one local CSS pixel stretches to near the points
/// of `contour` under `map`, for flattening curves before projecting them.
fn stretch(map: &Homography, contour: &[PathPoint]) -> f64 {
    let h = &map.0;
    let mut most: f64 = 0.0;
    for p in contour {
        let w = map.w(p.x, p.y);
        if w <= W_MIN {
            continue;
        }
        let Some((x, y)) = map.apply(p.x, p.y) else {
            continue;
        };
        // The Jacobian of the projection at p.
        let j = [
            (h[0] - x * h[6]) / w,
            (h[1] - x * h[7]) / w,
            (h[3] - y * h[6]) / w,
            (h[4] - y * h[7]) / w,
        ];
        let norm = j.iter().map(|v| v * v).sum::<f64>().sqrt();
        if norm.is_finite() {
            most = most.max(norm);
        }
    }
    if most > 0.0 {
        most.clamp(1e-3, 1e6)
    } else {
        1.0
    }
}

/// Flatten a local contour, keep what lies inside `bounds` (local CSS
/// pixels) and in front of the viewer, and map it to device pixels.
/// Polygon clipping commutes with the projection, which maps lines to
/// lines; curves are flattened finely enough for the local stretch.
fn project(contour: &[PathPoint], map: &Homography, bounds: Option<Bounds>) -> Vec<Point> {
    let k = stretch(map, contour);
    let mut points: Vec<Point> = flatten(contour, k)
        .into_iter()
        .map(|(x, y)| (x / k, y / k))
        .collect();
    if let Some(b) = bounds {
        points = clip(&points, b);
    }
    let h = &map.0;
    clip_half_plane(&points, (h[6], h[7], h[8] - W_MIN))
        .into_iter()
        .filter_map(|(x, y)| map.apply(x, y))
        .filter(|p| p.0.is_finite() && p.1.is_finite())
        .collect()
}

/// An axis clip as local bounds, unbounded axes reaching far beyond any
/// layout.
fn local_bounds(clip: Clip) -> Bounds {
    const FAR: f64 = 1e9;
    let (x0, x1) = clip.x.map_or((-FAR, FAR), |r| (r.start.get(), r.end.get()));
    let (y0, y1) = clip.y.map_or((-FAR, FAR), |r| (r.start.get(), r.end.get()));
    (x0, y0, x1, y1)
}

/// The device pixel window covering `contours`, within the surface.
fn window(contours: &[Vec<Point>], (w, h): (f64, f64)) -> Option<(f64, f64, usize, usize)> {
    let mut b = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for p in contours.iter().filter(|c| c.len() >= 3).flatten() {
        b = (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1));
    }
    let (x0, y0) = (b.0.max(0.0).floor(), b.1.max(0.0).floor());
    let (x1, y1) = (b.2.min(w).ceil(), b.3.min(h).ceil());
    (x0 < x1 && y0 < y1).then_some((x0, y0, (x1 - x0) as usize, (y1 - y0) as usize))
}

/// Like [`clip_mask`], for rounded clips in the local coordinates of `map`.
fn clip_mask_mapped(
    nodes: &[&ClipNode],
    map: &Homography,
    origin: (f64, f64),
    size: (usize, usize),
    work: &mut Work<'_>,
) -> Result<Option<Vec<f64>>, RasterError> {
    let mut mask: Option<Vec<f64>> = None;
    for node in nodes {
        let contour = project(&node.shape.contour(), map, None);
        work.charge(contour.len() as u64)?;
        let cover = coverage(&[contour], origin, size, work)?;
        mask = Some(match mask {
            None => cover,
            Some(mut m) => {
                for (a, b) in m.iter_mut().zip(cover) {
                    *a *= b;
                }
                m
            }
        });
    }
    Ok(mask)
}

/// Fill an item whose coordinates `map` takes to device pixels.
fn fill_mapped(
    pixels: &mut [u8],
    (width, height): (u32, u32),
    item: &DisplayItem,
    list: &DisplayList,
    map: &Homography,
    work: &mut Work<'_>,
) -> Result<(), RasterError> {
    let local = local_contours(&item.shape, list);
    work.charge(local.iter().map(|c| c.len() as u64).sum())?;
    let bounds = local_bounds(item.clip);
    let shape: Vec<Vec<Point>> = local
        .iter()
        .map(|c| project(c, map, Some(bounds)))
        .collect();
    work.charge(shape.iter().map(|c| c.len() as u64).sum())?;
    let Some((bx, by, bw, bh)) = window(&shape, (f64::from(width), f64::from(height))) else {
        return Ok(());
    };
    let cover = coverage(&shape, (bx, by), (bw, bh), work)?;
    let nodes = chain(list.clips(), item.rounded_clip)?;
    let mask = clip_mask_mapped(&nodes, map, (bx, by), (bw, bh), work)?;
    blend_color(
        pixels,
        width as usize,
        (bx, by, bw, bh),
        &cover,
        mask.as_deref(),
        item.color,
        work,
    )
}

/// Composite `color` over a window of the surface, weighted per pixel by
/// `cover` and `mask`.
fn blend_color(
    pixels: &mut [u8],
    width: usize,
    (bx, by, bw, bh): (f64, f64, usize, usize),
    cover: &[f64],
    mask: Option<&[f64]>,
    color: Srgba,
    work: &Work<'_>,
) -> Result<(), RasterError> {
    let alpha = f64::from(color.a) / 255.0;
    let premultiplied = [
        f64::from(color.r) * alpha,
        f64::from(color.g) * alpha,
        f64::from(color.b) * alpha,
        f64::from(color.a),
    ];
    let (ox, oy) = (bx as usize, by as usize);
    for row in 0..bh {
        work.cancellation
            .check()
            .map_err(|_| RasterError::Cancelled)?;
        let start = ((oy + row) * width + ox) * 4;
        for col in 0..bw {
            let k = row * bw + col;
            let coverage = cover[k] * mask.map_or(1.0, |m| m[k]);
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

/// Draw an image item whose coordinates `map` takes to device pixels: each
/// device pixel samples the image bilinearly at the local point its center
/// comes from, weighted by how much of the pixel the mapped destination
/// (within the clips) covers.
fn draw_image_mapped(
    pixels: &mut [u8],
    (width, height): (u32, u32),
    item: &DisplayItem,
    list: &DisplayList,
    map: &Homography,
    work: &mut Work<'_>,
) -> Result<(), RasterError> {
    let Shape::Image { rect, image } = item.shape else {
        return Ok(());
    };
    let image = list.images().get(image).ok_or(RasterError::InvalidList)?;
    let Some(inverse) = map.inverse() else {
        return Ok(());
    };
    let (x0, y0) = (rect.origin().x.get(), rect.origin().y.get());
    let (rw, rh) = (rect.size().width().get(), rect.size().height().get());
    if !(rw > 0.0 && rh > 0.0) {
        return Ok(());
    }
    let local = local_contours(&Shape::Rect(rect), list);
    let shape: Vec<Vec<Point>> = local
        .iter()
        .map(|c| project(c, map, Some(local_bounds(item.clip))))
        .collect();
    let Some((bx, by, bw, bh)) = window(&shape, (f64::from(width), f64::from(height))) else {
        return Ok(());
    };
    let cover = coverage(&shape, (bx, by), (bw, bh), work)?;
    let nodes = chain(list.clips(), item.rounded_clip)?;
    let mask = clip_mask_mapped(&nodes, map, (bx, by), (bw, bh), work)?;
    let (sx, sy) = (
        f64::from(image.width()) / rw,
        f64::from(image.height()) / rh,
    );
    let stride = width as usize;
    let (ox, oy) = (bx as usize, by as usize);
    for row in 0..bh {
        work.cancellation
            .check()
            .map_err(|_| RasterError::Cancelled)?;
        for col in 0..bw {
            let k = row * bw + col;
            let coverage = cover[k] * mask.as_ref().map_or(1.0, |m| m[k]);
            if coverage <= 0.0 {
                continue;
            }
            let (px, py) = (ox + col, oy + row);
            let center = (px as f64 + 0.5, py as f64 + 0.5);
            let w = inverse.w(center.0, center.1);
            if w == 0.0 {
                continue;
            }
            let h = &inverse.0;
            let lx = (h[0] * center.0 + h[1] * center.1 + h[2]) / w;
            let ly = (h[3] * center.0 + h[4] * center.1 + h[5]) / w;
            let src = sample(image, (lx - x0) * sx, (ly - y0) * sy);
            let keep = 1.0 - src[3] / 255.0 * coverage;
            let i = (py * stride + px) * 4;
            for (dst, s) in pixels[i..i + 4].iter_mut().zip(src) {
                let value = s * coverage + f64::from(*dst) * keep;
                *dst = value.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Ok(())
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

/// Device bounds of a rounded rectangle.
fn rounded_bounds(r: &RoundedRect, scale: f64) -> Bounds {
    let (l, t, rr, b) = r.edges();
    (l * scale, t * scale, rr * scale, b * scale)
}

fn intersect(a: Bounds, b: Bounds) -> Option<Bounds> {
    let r = (a.0.max(b.0), a.1.max(b.1), a.2.min(b.2), a.3.min(b.3));
    (r.0 < r.2 && r.1 < r.3).then_some(r)
}

/// The rounded clips of an item, innermost first. Each node's parent must
/// come before it, so the walk ends.
fn chain(clips: &[ClipNode], first: Option<usize>) -> Result<Vec<&ClipNode>, RasterError> {
    let mut out = Vec::new();
    let mut next = first;
    while let Some(i) = next {
        let node = clips.get(i).ok_or(RasterError::InvalidList)?;
        if node.parent.is_some_and(|p| p >= i) {
            return Err(RasterError::InvalidList);
        }
        out.push(node);
        next = node.parent;
    }
    Ok(out)
}

/// Exact-area coverage, clamped to one, of `contours` (device space) over
/// the `bw`×`bh` pixel window at `(bx, by)`, row-major.
fn coverage(
    contours: &[Vec<Point>],
    (bx, by): (f64, f64),
    (bw, bh): (usize, usize),
    work: &mut Work<'_>,
) -> Result<Vec<f64>, RasterError> {
    work.charge((bw as u64 + 2) * bh as u64)?;
    let stride = bw + 2;
    let mut acc = vec![0.0_f64; stride * bh];
    let window = (bx, by, bx + bw as f64, by + bh as f64);
    for contour in contours {
        let contour = clip(contour, window);
        if contour.len() < 3 {
            continue;
        }
        let mut previous = contour[contour.len() - 1];
        for &current in &contour {
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
    let mut out = vec![0.0; bw * bh];
    for row in 0..bh {
        let mut sum = 0.0;
        for col in 0..bw {
            sum += acc[row * stride + col];
            out[row * bw + col] = sum.abs().min(1.0);
        }
    }
    Ok(out)
}

/// The product of the coverage of every rounded clip in `nodes` over a
/// pixel window, or `None` when there are none.
fn clip_mask(
    nodes: &[&ClipNode],
    origin: (f64, f64),
    size: (usize, usize),
    scale: f64,
    work: &mut Work<'_>,
) -> Result<Option<Vec<f64>>, RasterError> {
    let mut mask: Option<Vec<f64>> = None;
    for node in nodes {
        let contour = flatten(&node.shape.contour(), scale);
        work.charge(contour.len() as u64)?;
        let cover = coverage(&[contour], origin, size, work)?;
        mask = Some(match mask {
            None => cover,
            Some(mut m) => {
                for (a, b) in m.iter_mut().zip(cover) {
                    *a *= b;
                }
                m
            }
        });
    }
    Ok(mask)
}

/// Bilinear sample of `image` at image-space `(u, v)` (pixel centers at
/// half-integers), clamped to the edges, as premultiplied RGBA in 0–255.
fn sample(image: &Image, u: f64, v: f64) -> [f64; 4] {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let clamp = |t: f64, n: usize| t.clamp(0.0, (n - 1) as f64);
    let (x, y) = (clamp(u - 0.5, w), clamp(v - 0.5, h));
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f64, y - y0 as f64);
    let pixels = image.pixels();
    let texel = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        let a = f64::from(pixels[i + 3]);
        [
            f64::from(pixels[i]) * a / 255.0,
            f64::from(pixels[i + 1]) * a / 255.0,
            f64::from(pixels[i + 2]) * a / 255.0,
            a,
        ]
    };
    let (p00, p10, p01, p11) = (texel(x0, y0), texel(x1, y0), texel(x0, y1), texel(x1, y1));
    let mut out = [0.0; 4];
    for c in 0..4 {
        let top = p00[c] + (p10[c] - p00[c]) * fx;
        let bottom = p01[c] + (p11[c] - p01[c]) * fx;
        out[c] = top + (bottom - top) * fy;
    }
    out
}

/// Draw an image item: each device pixel samples the image bilinearly at
/// its center and composites source-over, weighted by how much of the pixel
/// the destination rectangle (within the clip) covers. When the
/// destination maps image pixels one-to-one onto device pixels, this copies
/// them exactly.
fn draw_image(
    pixels: &mut [u8],
    (width, height): (u32, u32),
    item: &DisplayItem,
    list: &DisplayList,
    scale: f64,
    work: &mut Work<'_>,
) -> Result<(), RasterError> {
    let Shape::Image { rect, image } = item.shape else {
        return Ok(());
    };
    let image = list.images().get(image).ok_or(RasterError::InvalidList)?;
    let (w, h) = (f64::from(width), f64::from(height));
    let Some(mut limit) = item_bounds(item.clip, w, h, scale) else {
        return Ok(());
    };
    let nodes = chain(list.clips(), item.rounded_clip)?;
    for node in &nodes {
        let Some(b) = intersect(limit, rounded_bounds(&node.shape, scale)) else {
            return Ok(());
        };
        limit = b;
    }
    let o = rect.origin();
    let (x0, y0) = (o.x.get() * scale, o.y.get() * scale);
    let (x1, y1) = (
        (o.x.get() + rect.size().width().get()) * scale,
        (o.y.get() + rect.size().height().get()) * scale,
    );
    if !(x1 > x0 && y1 > y0) {
        return Ok(());
    }
    let (cx0, cy0, cx1, cy1) = (
        x0.max(limit.0),
        y0.max(limit.1),
        x1.min(limit.2),
        y1.min(limit.3),
    );
    if !(cx0 < cx1 && cy0 < cy1) {
        return Ok(());
    }
    let (px0, py0) = (cx0.floor() as usize, cy0.floor() as usize);
    let (px1, py1) = (cx1.ceil() as usize, cy1.ceil() as usize);
    work.charge((px1 - px0) as u64 * (py1 - py0) as u64)?;
    let window = (px1 - px0, py1 - py0);
    let mask = clip_mask(&nodes, (px0 as f64, py0 as f64), window, scale, work)?;
    let (sx, sy) = (
        f64::from(image.width()) / (x1 - x0),
        f64::from(image.height()) / (y1 - y0),
    );
    let stride = width as usize;
    let overlap = |p: usize, lo: f64, hi: f64| (hi.min(p as f64 + 1.0) - lo.max(p as f64)).max(0.0);
    for py in py0..py1 {
        work.cancellation
            .check()
            .map_err(|_| RasterError::Cancelled)?;
        let cover_y = overlap(py, cy0, cy1);
        let v = (py as f64 + 0.5 - y0) * sy;
        for px in px0..px1 {
            let masked = mask
                .as_ref()
                .map_or(1.0, |m| m[(py - py0) * window.0 + (px - px0)]);
            let coverage = overlap(px, cx0, cx1) * cover_y * masked;
            if coverage <= 0.0 {
                continue;
            }
            let src = sample(image, (px as f64 + 0.5 - x0) * sx, v);
            let keep = 1.0 - src[3] / 255.0 * coverage;
            let i = (py * stride + px) * 4;
            for (dst, s) in pixels[i..i + 4].iter_mut().zip(src) {
                let value = s * coverage + f64::from(*dst) * keep;
                *dst = value.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Ok(())
}

fn fill(
    pixels: &mut [u8],
    (width, height): (u32, u32),
    item: &DisplayItem,
    list: &DisplayList,
    scale: f64,
    work: &mut Work<'_>,
) -> Result<(), RasterError> {
    let (w, h) = (f64::from(width), f64::from(height));
    let Some(mut limit) = item_bounds(item.clip, w, h, scale) else {
        return Ok(());
    };
    let nodes = chain(list.clips(), item.rounded_clip)?;
    for node in &nodes {
        let Some(b) = intersect(limit, rounded_bounds(&node.shape, scale)) else {
            return Ok(());
        };
        limit = b;
    }
    let color = item.color;
    let shape = contours(&item.shape, list.paths(), list.rounded(), scale);
    if matches!(
        item.shape,
        Shape::Path(_) | Shape::Rounded(_) | Shape::RoundedRing { .. }
    ) {
        // Flattened points; plain box shapes are a few points each.
        work.charge(shape.iter().map(|c| c.len() as u64).sum())?;
    }
    let clipped: Vec<Vec<Point>> = shape
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
    // Work in a local window covering only the item's pixel bounds.
    let bx = bounds.0.floor();
    let by = bounds.1.floor();
    let bw = (bounds.2.ceil() - bx) as usize;
    let bh = (bounds.3.ceil() - by) as usize;
    let cover = coverage(&clipped, (bx, by), (bw, bh), work)?;
    let mask = clip_mask(&nodes, (bx, by), (bw, bh), scale, work)?;
    blend_color(
        pixels,
        width as usize,
        (bx, by, bw, bh),
        &cover,
        mask.as_deref(),
        color,
        work,
    )
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

/// An open layer: an opacity group, a `clip-path` layer with the map its
/// shape is drawn with, or a transform with the clips around it.
#[derive(Clone, Copy)]
enum Layer {
    Opacity(f64),
    Clip(Shape, Option<Homography>),
    Transform {
        clip: Clip,
        rounded_clip: Option<usize>,
    },
}

/// The pixels painting goes to: the innermost open layer with a buffer, or
/// the surface.
fn target<'a>(groups: &'a mut [(Option<Vec<u8>>, Layer)], surface: &'a mut [u8]) -> &'a mut [u8] {
    match groups
        .iter_mut()
        .rev()
        .find_map(|(pixels, _)| pixels.as_mut())
    {
        Some(pixels) => pixels,
        None => surface,
    }
}

/// Composite a premultiplied layer onto `dst` source-over, each pixel
/// weighted by `mask` (one coverage value per pixel).
fn composite_masked(dst: &mut [u8], layer: &[u8], mask: &[f64]) {
    for ((d, g), &m) in dst.chunks_exact_mut(4).zip(layer.chunks_exact(4)).zip(mask) {
        if g[3] == 0 || m <= 0.0 {
            continue;
        }
        let keep = 1.0 - f64::from(g[3]) * m / 255.0;
        for c in 0..4 {
            let v = f64::from(g[c]) * m + f64::from(d[c]) * keep;
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
    // Open layers, innermost last; a transform without clips has no buffer.
    let mut groups: Vec<(Option<Vec<u8>>, Layer)> = Vec::new();
    // Local-to-device maps of the open transforms, innermost last.
    let mut maps: Vec<Homography> = Vec::new();
    let window_size = (size.0 as usize, size.1 as usize);
    for command in list.commands() {
        match command {
            DisplayCommand::Fill(item) => {
                let pixels = target(&mut groups, &mut surface.pixels);
                let image = matches!(item.shape, Shape::Image { .. });
                match (maps.last(), image) {
                    (None, true) => draw_image(pixels, size, item, list, factor, &mut work)?,
                    (None, false) => fill(pixels, size, item, list, factor, &mut work)?,
                    (Some(map), true) => {
                        draw_image_mapped(pixels, size, item, list, map, &mut work)?;
                    }
                    (Some(map), false) => fill_mapped(pixels, size, item, list, map, &mut work)?,
                }
            }
            DisplayCommand::BeginGroup { .. }
            | DisplayCommand::BeginClip { .. }
            | DisplayCommand::BeginTransform { .. } => {
                let layer = match command {
                    DisplayCommand::BeginGroup { opacity, .. } => Layer::Opacity(*opacity),
                    DisplayCommand::BeginClip { shape, .. } => {
                        Layer::Clip(*shape, maps.last().copied())
                    }
                    DisplayCommand::BeginTransform { transform, .. } => {
                        let node = list
                            .transforms()
                            .get(*transform)
                            .ok_or(RasterError::InvalidList)?;
                        let outer = maps
                            .last()
                            .copied()
                            .unwrap_or_else(|| Homography::scale(factor));
                        maps.push(outer.then_after(&node.matrix));
                        Layer::Transform {
                            clip: node.clip,
                            rounded_clip: node.rounded_clip,
                        }
                    }
                    _ => return Err(RasterError::InvalidList),
                };
                let buffered = match layer {
                    Layer::Transform {
                        clip, rounded_clip, ..
                    } => !clip.is_none() || rounded_clip.is_some(),
                    _ => true,
                };
                if !buffered {
                    groups.push((None, layer));
                    continue;
                }
                // The surface, every open layer and the new one.
                if bytes
                    .checked_mul(groups.len() + 2)
                    .is_none_or(|n| n > limits.max_bytes)
                {
                    return Err(RasterError::MemoryLimit);
                }
                work.charge(u64::from(size.0) * u64::from(size.1))?;
                groups.push((Some(vec![0; bytes]), layer));
            }
            DisplayCommand::EndGroup | DisplayCommand::EndClip | DisplayCommand::EndTransform => {
                let (group, layer) = groups.pop().ok_or(RasterError::InvalidList)?;
                let mask = match (command, layer) {
                    (DisplayCommand::EndGroup, Layer::Opacity(opacity)) => {
                        let group = group.ok_or(RasterError::InvalidList)?;
                        work.charge(u64::from(size.0) * u64::from(size.1))?;
                        composite(target(&mut groups, &mut surface.pixels), &group, opacity);
                        continue;
                    }
                    (DisplayCommand::EndClip, Layer::Clip(shape, None)) => {
                        let contours = contours(&shape, list.paths(), list.rounded(), factor);
                        work.charge(contours.iter().map(|c| c.len() as u64).sum())?;
                        coverage(&contours, (0.0, 0.0), window_size, &mut work)?
                    }
                    (DisplayCommand::EndClip, Layer::Clip(shape, Some(map))) => {
                        let local = local_contours(&shape, list);
                        let contours: Vec<Vec<Point>> =
                            local.iter().map(|c| project(c, &map, None)).collect();
                        work.charge(contours.iter().map(|c| c.len() as u64).sum())?;
                        coverage(&contours, (0.0, 0.0), window_size, &mut work)?
                    }
                    (
                        DisplayCommand::EndTransform,
                        Layer::Transform {
                            clip, rounded_clip, ..
                        },
                    ) => {
                        maps.pop().ok_or(RasterError::InvalidList)?;
                        if group.is_none() {
                            continue;
                        }
                        // The clips around the transformed box, in the
                        // enclosing coordinates.
                        let outer = maps
                            .last()
                            .copied()
                            .unwrap_or_else(|| Homography::scale(factor));
                        let (x0, y0, x1, y1) = local_bounds(clip);
                        let on = |x, y| PathPoint {
                            x,
                            y,
                            on_curve: true,
                        };
                        let rect = [on(x0, y0), on(x1, y0), on(x1, y1), on(x0, y1)];
                        let area = project(&rect, &outer, None);
                        let mut mask = coverage(&[area], (0.0, 0.0), window_size, &mut work)?;
                        let nodes = chain(list.clips(), rounded_clip)?;
                        if let Some(rounded) =
                            clip_mask_mapped(&nodes, &outer, (0.0, 0.0), window_size, &mut work)?
                        {
                            for (a, b) in mask.iter_mut().zip(rounded) {
                                *a *= b;
                            }
                        }
                        mask
                    }
                    _ => return Err(RasterError::InvalidList),
                };
                let group = group.ok_or(RasterError::InvalidList)?;
                work.charge(u64::from(size.0) * u64::from(size.1))?;
                composite_masked(target(&mut groups, &mut surface.pixels), &group, &mask);
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

    fn polygon_area(points: &[Point]) -> f64 {
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
    fn quadratic_contours_flatten_within_tolerance() {
        let p = |x: f64, y: f64, on_curve| PathPoint { x, y, on_curve };
        // Four quadratic arcs with corner controls, as in the test fonts'
        // `0`: the exact area is 5/6 of the bounding rectangle.
        let oval = [
            p(20.0, 0.0, true),
            p(0.0, 0.0, false),
            p(0.0, 30.0, true),
            p(0.0, 60.0, false),
            p(20.0, 60.0, true),
            p(40.0, 60.0, false),
            p(40.0, 30.0, true),
            p(40.0, 0.0, false),
        ];
        for scale in [1.0, 4.0, 64.0] {
            let polygon = flatten(&oval, scale);
            let exact = 5.0 / 6.0 * 40.0 * 60.0 * scale * scale;
            let area = polygon_area(&polygon).abs();
            // Chords lie inside the convex arcs, so the error is negative;
            // within the flatness it is under a band along the perimeter
            // (200 units), and the segment cap keeps huge glyphs within 0.05%.
            let band = FLATNESS * 200.0 * scale;
            assert!(
                area <= exact && exact - area <= band.max(exact * 5e-4),
                "{scale}: {area}"
            );
            assert!(polygon.len() <= 4 * MAX_SEGMENTS as usize + 1);
        }
        // Only control points: the start is the implied midpoint, and every
        // arc still closes on itself.
        let controls = [
            p(0.0, 0.0, false),
            p(10.0, 0.0, false),
            p(10.0, 10.0, false),
            p(0.0, 10.0, false),
        ];
        let polygon = flatten(&controls, 1.0);
        assert_eq!(polygon[0], (0.0, 5.0));
        assert_eq!(*polygon.last().unwrap(), (0.0, 5.0));
        // A lone on-curve point and a straight contour stay as they are.
        assert_eq!(flatten(&[p(1.0, 2.0, true)], 2.0), [(2.0, 4.0), (2.0, 4.0)]);
        let square = [
            p(0.0, 0.0, true),
            p(0.0, 1.0, true),
            p(1.0, 1.0, true),
            p(1.0, 0.0, true),
        ];
        assert_eq!(polygon_area(&flatten(&square, 3.0)).abs(), 9.0);
    }

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
    fn coverage_does_not_depend_on_the_window_origin() {
        // A steep edge ending exactly on the window's left edge: stepping
        // along it once produced x = -1e-16 and shifted it a column.
        let contour = vec![
            (25.0, 35.0),
            (25.190_301_168_721_78, 33.048_729_582_188_98),
            (40.0, 30.0),
            (40.0, 40.0),
        ];
        let cancel = CancellationToken::default();
        let mut work = Work {
            used: 0,
            limit: u64::MAX,
            cancellation: &cancel,
        };
        let wide = super::coverage(
            std::slice::from_ref(&contour),
            (0.0, 0.0),
            (50, 50),
            &mut work,
        )
        .unwrap();
        let tight = super::coverage(&[contour], (25.0, 30.0), (15, 10), &mut work).unwrap();
        for row in 0..10 {
            for col in 0..15 {
                let (a, b) = (wide[(row + 30) * 50 + col + 25], tight[row * 15 + col]);
                assert!(close(a, b), "({col},{row}): {a} vs {b}");
            }
        }
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
