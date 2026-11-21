//! Deterministic CPU reference rasterizer: exact-area antialiasing and
//! source-over compositing into a checked premultiplied RGBA8 surface.
//!
//! Coverage is the exact area of each pixel inside the shape (signed-area
//! accumulation, nonzero winding clamped to one), computed in `f64` with no
//! platform math calls, so output bytes are identical on every target.

use crate::damage::Culling;
use rustic_base::CancellationToken;
use rustic_layout::Clip;
use rustic_layout::{CssRect, DeviceScale};
use rustic_paint::{
    BlendMode, ClipNode, DisplayCommand, DisplayItem, DisplayList, FilterOp, Homography, Image,
    Path, PathPoint, RoundedRect, Shape, Srgba,
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
        let surface = rustic_base::SurfaceLimits {
            max_width: limits.max_width,
            max_height: limits.max_height,
            max_bytes: limits.max_bytes,
        };
        let bytes = rustic_base::SurfaceAllocation::check(width, height, 4, surface)
            .map_err(|_| RasterError::SurfaceSize)?
            .bytes();
        Ok(Self {
            width,
            height,
            pixels: vec![0; bytes],
        })
    }

    /// A surface from premultiplied RGBA bytes another process produced:
    /// `None` unless the size passes every budget, `pixels` holds exactly
    /// four bytes per pixel and no color channel exceeds its alpha.
    pub fn from_premultiplied(
        width: u32,
        height: u32,
        pixels: Vec<u8>,
        limits: RasterLimits,
    ) -> Option<Self> {
        let expected = Self::new(width, height, limits).ok()?.pixels.len();
        let premultiplied = pixels
            .chunks_exact(4)
            .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]);
        (pixels.len() == expected && premultiplied).then_some(Self {
            width,
            height,
            pixels,
        })
    }

    /// Composite `frame` into the `width × height` rectangle at `x, y`:
    /// each pixel gains `coverage / 255` of the frame pixel at the same
    /// offset (the frame's edge pixel past its size), saturating. Fails
    /// without changing anything when the rectangle leaves the surface or
    /// `coverage` is not one byte per pixel. This is the arithmetic of
    /// ADR 0062: the embedder painted the frame opaque black, and
    /// `coverage` is how much an opaque white frame would have added.
    pub fn add_frame(
        &mut self,
        frame: &Surface,
        (x, y, width, height): (u32, u32, u32, u32),
        coverage: &[u8],
    ) -> Result<(), RasterError> {
        let fits = |start: u32, extent: u32, limit: u32| {
            start.checked_add(extent).is_some_and(|end| end <= limit)
        };
        let count = usize::try_from(u64::from(width) * u64::from(height))
            .map_err(|_| RasterError::SurfaceSize)?;
        if !fits(x, width, self.width) || !fits(y, height, self.height) || coverage.len() != count {
            return Err(RasterError::SurfaceSize);
        }
        if frame.width == 0 || frame.height == 0 {
            return Ok(());
        }
        for row in 0..height {
            let source_row = row.min(frame.height - 1) as usize;
            for column in 0..width {
                let weight = u32::from(coverage[(row * width + column) as usize]);
                if weight == 0 {
                    continue;
                }
                let source_column = column.min(frame.width - 1) as usize;
                let source = (source_row * frame.width as usize + source_column) * 4;
                let target =
                    (((y + row) as usize) * self.width as usize + (x + column) as usize) * 4;
                let alpha = u32::from(self.pixels[target + 3]);
                for channel in 0..3 {
                    let added = (weight * u32::from(frame.pixels[source + channel]) + 127) / 255;
                    let value = (u32::from(self.pixels[target + channel]) + added).min(alpha);
                    self.pixels[target + channel] = value as u8;
                }
            }
        }
        Ok(())
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

    pub(crate) fn pixels_mut_internal(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    #[cfg(test)]
    pub(crate) fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    /// Clear to the canvas color: opaque for a page, fully transparent for
    /// an overlay layer (ADR 0064).
    fn fill_canvas(&mut self, color: Srgba) {
        let pixel = if color.a == 0 {
            [0, 0, 0, 0]
        } else {
            [color.r, color.g, color.b, 255]
        };
        for p in self.pixels.chunks_exact_mut(4) {
            p.copy_from_slice(&pixel);
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
pub(crate) type Bounds = (f64, f64, f64, f64);

/// The device pixels being rasterized: a rectangle of the full surface,
/// in its coordinates. Every buffer covers the frame; geometry stays in
/// surface coordinates, so a pixel's value does not depend on the frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Frame {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Frame {
    fn bounds(self) -> Bounds {
        (
            self.x as f64,
            self.y as f64,
            (self.x + self.w) as f64,
            (self.y + self.h) as f64,
        )
    }

    /// The byte offset of surface pixel `(x, y)`, which must be inside.
    fn offset(self, x: usize, y: usize) -> usize {
        ((y - self.y) * self.w + (x - self.x)) * 4
    }
}

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
        // Images and shadows are drawn by their own functions.
        Shape::Image { .. } | Shape::Shadow(_) => Vec::new(),
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
        Shape::Image { .. } | Shape::Shadow(_) => Vec::new(),
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
pub(crate) fn stretch(map: &Homography, contour: &[PathPoint]) -> f64 {
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
pub(crate) fn project(
    contour: &[PathPoint],
    map: &Homography,
    bounds: Option<Bounds>,
) -> Vec<Point> {
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
pub(crate) fn local_bounds(clip: Clip) -> Bounds {
    const FAR: f64 = 1e9;
    let (x0, x1) = clip.x.map_or((-FAR, FAR), |r| (r.start.get(), r.end.get()));
    let (y0, y1) = clip.y.map_or((-FAR, FAR), |r| (r.start.get(), r.end.get()));
    (x0, y0, x1, y1)
}

/// The device pixel window covering `contours`, within `limit`.
fn window(contours: &[Vec<Point>], limit: Bounds) -> Option<(f64, f64, usize, usize)> {
    let mut b = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for p in contours.iter().filter(|c| c.len() >= 3).flatten() {
        b = (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1));
    }
    let (x0, y0) = (b.0.max(limit.0).floor(), b.1.max(limit.1).floor());
    let (x1, y1) = (b.2.min(limit.2).ceil(), b.3.min(limit.3).ceil());
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
    frame: Frame,
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
    let Some((bx, by, bw, bh)) = window(&shape, frame.bounds()) else {
        return Ok(());
    };
    let cover = coverage(&shape, (bx, by), (bw, bh), work)?;
    let nodes = chain(list.clips(), item.rounded_clip)?;
    let mask = clip_mask_mapped(&nodes, map, (bx, by), (bw, bh), work)?;
    blend_color(
        pixels,
        frame,
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
    frame: Frame,
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
        let start = frame.offset(ox, oy + row);
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
    frame: Frame,
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
    let Some((bx, by, bw, bh)) = window(&shape, frame.bounds()) else {
        return Ok(());
    };
    let cover = coverage(&shape, (bx, by), (bw, bh), work)?;
    let nodes = chain(list.clips(), item.rounded_clip)?;
    let mask = clip_mask_mapped(&nodes, map, (bx, by), (bw, bh), work)?;
    let (sx, sy) = (
        f64::from(image.width()) / rw,
        f64::from(image.height()) / rh,
    );
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
            let i = frame.offset(px, py);
            for (dst, s) in pixels[i..i + 4].iter_mut().zip(src) {
                let value = s * coverage + f64::from(*dst) * keep;
                *dst = value.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Ok(())
}

/// `e^-x` for `x >= 0` in plain arithmetic (halving, a Taylor series and
/// squaring), so every platform computes the same value.
fn exp_neg(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 || x > 745.0 {
        return if x < 0.0 { 1.0 } else { 0.0 };
    }
    let mut halvings = 0;
    let mut r = x;
    while r > 0.5 {
        r /= 2.0;
        halvings += 1;
    }
    // e^-r for r in [0, 0.5]: the series' 16th term is below 1e-17.
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..=16 {
        term *= -r / f64::from(k);
        sum += term;
    }
    for _ in 0..halvings {
        sum *= sum;
    }
    sum
}

/// The box size of the three-box approximation for `sigma` (Filter
/// Effects 1 §15.17).
fn box_size(sigma: f64) -> isize {
    (sigma * 3.0 * (2.0 * std::f64::consts::PI).sqrt() / 4.0 + 0.5).floor() as isize
}

/// How far a blur of `sigma` device pixels reaches, including how far the
/// clamped edges of a finite buffer can disturb it.
pub(crate) fn blur_reach(sigma: f64) -> usize {
    if sigma.is_nan() || sigma <= 0.001 {
        return 0;
    }
    let exact = (3.0 * sigma).ceil();
    let boxes = if sigma >= 2.0 {
        2.0 * box_size(sigma) as f64
    } else {
        0.0
    };
    let reach = exact.max(boxes) + 2.0;
    if reach < 1e9 {
        reach as usize
    } else {
        1_000_000_000
    }
}

/// Fixed-point scale for box sums: integer sums are exact, so a value does
/// not depend on where a buffer starts.
const BOX_ONE: f64 = 4_294_967_296.0;

/// Replace each value of `line` by the mean of `line[x + lo ..= x + hi]`,
/// edge values extending past the ends, summing in fixed point.
fn box_pass(line: &mut [f64], lo: isize, hi: isize, prefix: &mut Vec<i64>) {
    let n = line.len() as isize;
    prefix.clear();
    prefix.push(0);
    let mut sum = 0i64;
    let fixed: Vec<i64> = line.iter().map(|&v| (v * BOX_ONE).round() as i64).collect();
    for &v in &fixed {
        sum = sum.saturating_add(v);
        prefix.push(sum);
    }
    let (first, last) = (fixed[0], fixed[fixed.len() - 1]);
    let width = (hi - lo + 1) as f64;
    let out: Vec<f64> = (0..n)
        .map(|x| {
            let (a, b) = (x + lo, x + hi);
            // Clamped parts beyond each end repeat the edge value.
            let before = (-a).max(0).min(b - a + 1) as i64 * first;
            let after = (b - (n - 1)).max(0).min(b - a + 1) as i64 * last;
            let (ca, cb) = (a.clamp(0, n), (b + 1).clamp(0, n));
            let inside = if cb > ca {
                prefix[cb as usize] - prefix[ca as usize]
            } else {
                0
            };
            (before + inside + after) as f64 / BOX_ONE / width
        })
        .collect();
    line.copy_from_slice(&out);
}

/// Blur one line by a Gaussian of standard deviation `sigma` pixels:
/// exactly below two pixels, else by three box blurs as Filter Effects 1
/// §15.17 (`feGaussianBlur`) allows.
fn blur_line(line: &mut [f64], sigma: f64, kernel: &[f64], prefix: &mut Vec<i64>) {
    if line.is_empty() {
        return;
    }
    if sigma >= 2.0 {
        let d = box_size(sigma);
        if d % 2 == 1 {
            let h = (d - 1) / 2;
            for _ in 0..3 {
                box_pass(line, -h, h, prefix);
            }
        } else {
            let h = d / 2;
            box_pass(line, -h, h - 1, prefix);
            box_pass(line, -h + 1, h, prefix);
            box_pass(line, -h, h, prefix);
        }
        return;
    }
    let r = kernel.len() / 2;
    let n = line.len();
    let input = line.to_vec();
    for (x, out) in line.iter_mut().enumerate() {
        let mut sum = 0.0;
        for (k, w) in kernel.iter().enumerate() {
            let i = (x + k).saturating_sub(r).min(n - 1);
            sum += w * input[i];
        }
        *out = sum;
    }
}

/// The normalized Gaussian kernel for `sigma` below two pixels.
fn gaussian_kernel(sigma: f64) -> Vec<f64> {
    if sigma >= 2.0 {
        return Vec::new();
    }
    let r = (3.0 * sigma).ceil() as i32;
    let weights: Vec<f64> = (-r..=r)
        .map(|k| exp_neg(f64::from(k * k) / (2.0 * sigma * sigma)))
        .collect();
    let total: f64 = weights.iter().sum();
    weights.into_iter().map(|w| w / total).collect()
}

/// Blur a `w`×`h` row-major buffer in place, rows then columns.
fn blur(
    values: &mut [f64],
    (w, h): (usize, usize),
    sigma: f64,
    work: &mut Work<'_>,
) -> Result<(), RasterError> {
    if sigma.is_nan() || sigma <= 0.001 {
        return Ok(());
    }
    let kernel = gaussian_kernel(sigma);
    let passes = if kernel.is_empty() { 3 } else { kernel.len() } as u64;
    work.charge(2 * passes * (w * h) as u64)?;
    let mut prefix = Vec::new();
    for row in values.chunks_exact_mut(w) {
        blur_line(row, sigma, &kernel, &mut prefix);
    }
    let mut column = vec![0.0; h];
    for x in 0..w {
        work.cancellation
            .check()
            .map_err(|_| RasterError::Cancelled)?;
        for (y, c) in column.iter_mut().enumerate() {
            *c = values[y * w + x];
        }
        blur_line(&mut column, sigma, &kernel, &mut prefix);
        for (y, c) in column.iter().enumerate() {
            values[y * w + x] = *c;
        }
    }
    Ok(())
}

/// Paint a box shadow item. `map` is the local-to-device map inside a
/// transform (the blur then uses the map's stretch at the shape), else
/// coordinates scale by `scale`.
fn shadow(
    pixels: &mut [u8],
    frame: Frame,
    item: &DisplayItem,
    list: &DisplayList,
    map: Option<&Homography>,
    scale: f64,
    work: &mut Work<'_>,
) -> Result<(), RasterError> {
    let Shape::Shadow(index) = item.shape else {
        return Ok(());
    };
    let s = list.shadows().get(index).ok_or(RasterError::InvalidList)?;
    let device = |r: &RoundedRect| match map {
        None => flatten(&r.contour(), scale),
        Some(m) => project(&r.contour(), m, None),
    };
    let sigma = s.sigma
        * match map {
            None => scale,
            Some(m) => stretch(m, &s.clip.contour()),
        };
    let reach = blur_reach(sigma) as f64;
    let mut limit = frame.bounds();
    if map.is_none() {
        let Some(b) = item_bounds(item.clip, limit, scale) else {
            return Ok(());
        };
        limit = b;
    }
    let shape = s.shape.as_ref().map(device);
    let clip = device(&s.clip);
    work.charge(clip.len() as u64 + shape.as_ref().map_or(0, |c| c.len() as u64))?;
    // The pixels the shadow may reach.
    let reaching = if s.inset {
        window(std::slice::from_ref(&clip), limit)
    } else {
        shape.as_ref().and_then(|c| {
            let mut b = (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            );
            for p in c {
                b = (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1));
            }
            let grown = (b.0 - reach, b.1 - reach, b.2 + reach, b.3 + reach);
            let corners = vec![
                (grown.0, grown.1),
                (grown.2, grown.1),
                (grown.2, grown.3),
                (grown.0, grown.3),
            ];
            window(&[corners], limit)
        })
    };
    let Some((tx, ty, tw, th)) = reaching else {
        return Ok(());
    };
    let Some(target) = intersect((tx, ty, tx + tw as f64, ty + th as f64), limit) else {
        return Ok(());
    };
    let (tx, ty) = (target.0.floor(), target.1.floor());
    let (tw, th) = (
        (target.2.ceil() - tx) as usize,
        (target.3.ceil() - ty) as usize,
    );
    // Blur over the target grown by the blur's reach, so values flowing in
    // from outside it are right.
    let pad = reach as usize;
    let (ex, ey) = (tx - reach, ty - reach);
    let (ew, eh) = (tw + 2 * pad, th + 2 * pad);
    let mut base = match &shape {
        Some(c) => coverage(std::slice::from_ref(c), (ex, ey), (ew, eh), work)?,
        None => vec![0.0; ew * eh],
    };
    if s.inset {
        for v in &mut base {
            *v = 1.0 - *v;
        }
    }
    blur(&mut base, (ew, eh), sigma, work)?;
    let edge = coverage(&[clip], (tx, ty), (tw, th), work)?;
    let mut cover = vec![0.0; tw * th];
    for row in 0..th {
        for col in 0..tw {
            let blurred = base[(row + pad) * ew + col + pad];
            let e = edge[row * tw + col];
            cover[row * tw + col] = blurred * if s.inset { e } else { 1.0 - e };
        }
    }
    let nodes = chain(list.clips(), item.rounded_clip)?;
    let mut mask = match map {
        None => clip_mask(&nodes, (tx, ty), (tw, th), scale, work)?,
        Some(m) => clip_mask_mapped(&nodes, m, (tx, ty), (tw, th), work)?,
    };
    if let (Some(m), false) = (map, item.clip.is_none()) {
        let (x0, y0, x1, y1) = local_bounds(item.clip);
        let on = |x, y| PathPoint {
            x,
            y,
            on_curve: true,
        };
        let area = project(&[on(x0, y0), on(x1, y0), on(x1, y1), on(x0, y1)], m, None);
        let axis = coverage(&[area], (tx, ty), (tw, th), work)?;
        mask = Some(match mask {
            None => axis,
            Some(mut v) => {
                for (a, b) in v.iter_mut().zip(axis) {
                    *a *= b;
                }
                v
            }
        });
    }
    blend_color(
        pixels,
        frame,
        (tx, ty, tw, th),
        &cover,
        mask.as_deref(),
        item.color,
        work,
    )
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
fn item_bounds(clip_rect: Clip, frame: Bounds, scale: f64) -> Option<Bounds> {
    let mut b = frame;
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
pub(crate) fn chain(
    clips: &[ClipNode],
    first: Option<usize>,
) -> Result<Vec<&ClipNode>, RasterError> {
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
    frame: Frame,
    item: &DisplayItem,
    list: &DisplayList,
    scale: f64,
    work: &mut Work<'_>,
) -> Result<(), RasterError> {
    let Shape::Image { rect, image } = item.shape else {
        return Ok(());
    };
    let image = list.images().get(image).ok_or(RasterError::InvalidList)?;
    let Some(mut limit) = item_bounds(item.clip, frame.bounds(), scale) else {
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
            let i = frame.offset(px, py);
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
    frame: Frame,
    item: &DisplayItem,
    list: &DisplayList,
    scale: f64,
    work: &mut Work<'_>,
) -> Result<(), RasterError> {
    let Some(mut limit) = item_bounds(item.clip, frame.bounds(), scale) else {
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
    let mut bounds = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
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
        frame,
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

/// The blended color `B(cb, cs)` of one pixel's straight-alpha channels in
/// 0–1 (Compositing and Blending 1 §5), for every mode but `normal` and
/// `plus-lighter`.
fn blend_rgb(mode: BlendMode, cb: [f64; 3], cs: [f64; 3]) -> [f64; 3] {
    let separable =
        |f: &dyn Fn(f64, f64) -> f64| [f(cb[0], cs[0]), f(cb[1], cs[1]), f(cb[2], cs[2])];
    let multiply = |b: f64, s: f64| b * s;
    let screen = |b: f64, s: f64| b + s - b * s;
    let hard_light = |b: f64, s: f64| {
        if s <= 0.5 {
            multiply(b, 2.0 * s)
        } else {
            screen(b, 2.0 * s - 1.0)
        }
    };
    let lum = |c: [f64; 3]| 0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2];
    let clip_color = |c: [f64; 3]| {
        let l = lum(c);
        let n = c[0].min(c[1]).min(c[2]);
        let x = c[0].max(c[1]).max(c[2]);
        let mut c = c;
        if n < 0.0 && l - n != 0.0 {
            c = c.map(|v| l + (v - l) * l / (l - n));
        }
        if x > 1.0 && x - l != 0.0 {
            c = c.map(|v| l + (v - l) * (1.0 - l) / (x - l));
        }
        c
    };
    let set_lum = |c: [f64; 3], l: f64| {
        let d = l - lum(c);
        clip_color(c.map(|v| v + d))
    };
    let sat = |c: [f64; 3]| c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
    let set_sat = |c: [f64; 3], s: f64| {
        let (n, x) = (c[0].min(c[1]).min(c[2]), c[0].max(c[1]).max(c[2]));
        if x > n {
            c.map(|v| (v - n) * s / (x - n))
        } else {
            [0.0; 3]
        }
    };
    match mode {
        BlendMode::Normal | BlendMode::PlusLighter => cs,
        BlendMode::Multiply => separable(&multiply),
        BlendMode::Screen => separable(&screen),
        BlendMode::Overlay => separable(&|b, s| hard_light(s, b)),
        BlendMode::Darken => separable(&f64::min),
        BlendMode::Lighten => separable(&f64::max),
        BlendMode::ColorDodge => separable(&|b, s| {
            if b == 0.0 {
                0.0
            } else if s >= 1.0 {
                1.0
            } else {
                (b / (1.0 - s)).min(1.0)
            }
        }),
        BlendMode::ColorBurn => separable(&|b, s| {
            if b >= 1.0 {
                1.0
            } else if s <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - b) / s).min(1.0)
            }
        }),
        BlendMode::HardLight => separable(&hard_light),
        BlendMode::SoftLight => separable(&|b, s| {
            if s <= 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                let d = if b <= 0.25 {
                    ((16.0 * b - 12.0) * b + 4.0) * b
                } else {
                    b.sqrt()
                };
                b + (2.0 * s - 1.0) * (d - b)
            }
        }),
        BlendMode::Difference => separable(&|b, s| (b - s).abs()),
        BlendMode::Exclusion => separable(&|b, s| b + s - 2.0 * b * s),
        BlendMode::Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        BlendMode::Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        BlendMode::Color => set_lum(cs, lum(cb)),
        BlendMode::Luminosity => set_lum(cb, lum(cs)),
    }
}

/// Composite a premultiplied group onto `dst` at `opacity` with a blend
/// mode: `co = cs·αs·(1 − αb) + cb·αb·(1 − αs) + αs·αb·B(cb, cs)`, or for
/// `plus-lighter` the clamped sum (Compositing and Blending 1 §9.1).
fn composite_blend(dst: &mut [u8], group: &[u8], opacity: f64, mode: BlendMode) {
    if mode == BlendMode::Normal {
        composite(dst, group, opacity);
        return;
    }
    for (d, g) in dst.chunks_exact_mut(4).zip(group.chunks_exact(4)) {
        if g[3] == 0 {
            continue;
        }
        let a_s = f64::from(g[3]) / 255.0 * opacity;
        let a_b = f64::from(d[3]) / 255.0;
        if mode == BlendMode::PlusLighter {
            for c in 0..4 {
                let v = f64::from(g[c]) * opacity + f64::from(d[c]);
                d[c] = v.round().clamp(0.0, 255.0) as u8;
            }
            continue;
        }
        let straight = |p: &[u8], c: usize| {
            if p[3] == 0 {
                0.0
            } else {
                f64::from(p[c]) / f64::from(p[3])
            }
        };
        let cs = [straight(g, 0), straight(g, 1), straight(g, 2)].map(|v| v.min(1.0));
        let cb = [straight(d, 0), straight(d, 1), straight(d, 2)].map(|v| v.min(1.0));
        let mixed = blend_rgb(mode, cb, cs);
        let a_o = a_s + a_b * (1.0 - a_s);
        for c in 0..3 {
            let v = cs[c] * a_s * (1.0 - a_b) + cb[c] * a_b * (1.0 - a_s) + a_s * a_b * mixed[c];
            d[c] = (v * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        d[3] = (a_o * 255.0).round().clamp(0.0, 255.0) as u8;
    }
}

/// Apply filter operations in order to a premultiplied `w`×`h` layer,
/// `scale` device pixels per CSS pixel.
fn apply_filter(
    pixels: &mut [u8],
    (w, h): (usize, usize),
    ops: &[FilterOp],
    scale: f64,
    work: &mut Work<'_>,
) -> Result<(), RasterError> {
    for op in ops {
        work.charge((w * h) as u64)?;
        match *op {
            FilterOp::Blur(sigma) => {
                let mut channels: Vec<Vec<f64>> = (0..4)
                    .map(|c| {
                        pixels
                            .iter()
                            .skip(c)
                            .step_by(4)
                            .map(|&v| f64::from(v))
                            .collect()
                    })
                    .collect();
                for channel in &mut channels {
                    blur(channel, (w, h), sigma * scale, work)?;
                }
                for (i, p) in pixels.chunks_exact_mut(4).enumerate() {
                    for c in 0..4 {
                        p[c] = channels[c][i].round().clamp(0.0, 255.0) as u8;
                    }
                }
                // Blurring premultiplied channels separately can leave a
                // color above its alpha by rounding.
                for p in pixels.chunks_exact_mut(4) {
                    for c in 0..3 {
                        p[c] = p[c].min(p[3]);
                    }
                }
            }
            FilterOp::Color(m) => {
                for p in pixels.chunks_exact_mut(4) {
                    let a = f64::from(p[3]) / 255.0;
                    let straight = |v: u8| {
                        if a > 0.0 {
                            f64::from(v) / 255.0 / a
                        } else {
                            0.0
                        }
                    };
                    let input = [straight(p[0]), straight(p[1]), straight(p[2]), a];
                    let mut out = [0.0_f64; 4];
                    for (r, o) in out.iter_mut().enumerate() {
                        let row = &m[r * 5..r * 5 + 5];
                        *o = (row[0] * input[0]
                            + row[1] * input[1]
                            + row[2] * input[2]
                            + row[3] * input[3]
                            + row[4])
                            .clamp(0.0, 1.0);
                    }
                    for c in 0..3 {
                        p[c] = (out[c] * out[3] * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                    p[3] = (out[3] * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            }
            FilterOp::DropShadow {
                dx,
                dy,
                sigma,
                color,
            } => {
                let mut alpha: Vec<f64> = pixels
                    .chunks_exact(4)
                    .map(|p| f64::from(p[3]) / 255.0)
                    .collect();
                blur(&mut alpha, (w, h), sigma * scale, work)?;
                let (dx, dy) = (dx * scale, dy * scale);
                // Shift by bilinear sampling at (x − dx, y − dy).
                let at = |x: isize, y: isize| {
                    if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
                        0.0
                    } else {
                        alpha[y as usize * w + x as usize]
                    }
                };
                let (fx, fy) = (dx.floor(), dy.floor());
                let (tx, ty) = (dx - fx, dy - fy);
                let ca = f64::from(color.a) / 255.0;
                let tint = [color.r, color.g, color.b].map(|v| f64::from(v) * ca);
                for y in 0..h {
                    for x in 0..w {
                        let (sx, sy) = (x as isize - fx as isize, y as isize - fy as isize);
                        let shadow = at(sx, sy) * (1.0 - tx) * (1.0 - ty)
                            + at(sx - 1, sy) * tx * (1.0 - ty)
                            + at(sx, sy - 1) * (1.0 - tx) * ty
                            + at(sx - 1, sy - 1) * tx * ty;
                        if shadow <= 0.0 {
                            continue;
                        }
                        let p = &mut pixels[(y * w + x) * 4..(y * w + x) * 4 + 4];
                        // The layer over its shadow.
                        let keep = 1.0 - f64::from(p[3]) / 255.0;
                        for c in 0..3 {
                            let v = f64::from(p[c]) + tint[c] * shadow * keep;
                            p[c] = v.round().clamp(0.0, 255.0) as u8;
                        }
                        let v = f64::from(p[3]) + 255.0 * ca * shadow * keep;
                        p[3] = v.round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
    }
    Ok(())
}

/// An open layer: an opacity group, a `clip-path` layer with the map its
/// shape is drawn with, or a transform with the clips around it.
#[derive(Clone, Copy)]
enum Layer {
    Group {
        opacity: f64,
        blend: BlendMode,
        filter: Option<usize>,
        /// Device pixels per CSS pixel where the group opened.
        scale: f64,
    },
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

/// A rectangle of device pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceRect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

impl DeviceRect {
    /// The smallest rectangle containing both.
    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = (self.x + self.width).max(other.x + other.width);
        let bottom = (self.y + self.height).max(other.y + other.height);
        Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }
}

/// The surface size `list` rasterizes to at `scale`: the viewport times
/// `scale`, rounded up past a millionth of a pixel (so float error in a
/// zoomed viewport, the window over the zoom, does not add a pixel).
pub fn surface_size(list: &DisplayList, scale: DeviceScale) -> Result<(u32, u32), RasterError> {
    let device = |css: f64| -> Result<u32, RasterError> {
        let v = rustic_layout::CssPx::new(css)
            .map_err(|_| RasterError::SurfaceSize)?
            .to_device(scale)
            .get();
        let v = (v - 1e-6).ceil();
        if v.is_finite() && v >= 1.0 && v <= f64::from(u32::MAX) {
            Ok(v as u32)
        } else {
            Err(RasterError::SurfaceSize)
        }
    };
    let viewport = list.viewport();
    Ok((
        device(viewport.width().get())?,
        device(viewport.height().get())?,
    ))
}

/// Device pixels per CSS pixel.
pub(crate) fn device_factor(scale: DeviceScale) -> Result<f64, RasterError> {
    Ok(rustic_layout::CssPx::new(1.0)
        .map_err(|_| RasterError::SurfaceSize)?
        .to_device(scale)
        .get())
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
    let (width, height) = surface_size(list, scale)?;
    let all = DeviceRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    rasterize_region(list, scale, all, limits, cancellation)
}

/// How far, in device pixels, filters can move color across the surface:
/// the largest sum of blur reaches and drop shadow offsets over nested
/// filter groups. Rasterizing a region needs this much context around it.
pub(crate) fn effect_margin(list: &DisplayList, factor: f64) -> Result<usize, RasterError> {
    let mut maps: Vec<Option<Homography>> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut total = 0usize;
    let mut most = 0usize;
    for command in list.commands() {
        match command {
            DisplayCommand::BeginTransform { transform, .. } => {
                let node = list
                    .transforms()
                    .get(*transform)
                    .ok_or(RasterError::InvalidList)?;
                let outer = maps
                    .iter()
                    .rev()
                    .flatten()
                    .next()
                    .copied()
                    .unwrap_or_else(|| Homography::scale(factor));
                maps.push(Some(outer.then_after(&node.matrix)));
            }
            DisplayCommand::EndTransform => {
                maps.pop();
            }
            DisplayCommand::BeginGroup { filter, .. } => {
                let scale = group_scale(maps.iter().rev().flatten().next(), factor);
                let reach = match filter {
                    None => 0,
                    Some(f) => list
                        .filters()
                        .get(*f)
                        .ok_or(RasterError::InvalidList)?
                        .iter()
                        .map(|op| filter_reach(op, scale))
                        .fold(0usize, usize::saturating_add),
                };
                open.push(reach);
                total = total.saturating_add(reach);
                most = most.max(total);
            }
            DisplayCommand::EndGroup => {
                total -= open.pop().ok_or(RasterError::InvalidList)?;
            }
            _ => {}
        }
    }
    Ok(most)
}

/// Device pixels per CSS pixel for a group opened under `map`.
pub(crate) fn group_scale(map: Option<&Homography>, factor: f64) -> f64 {
    map.map_or(factor, |m| {
        stretch(
            m,
            &[PathPoint {
                x: 0.0,
                y: 0.0,
                on_curve: true,
            }],
        )
    })
}

/// How far one filter operation moves color, in device pixels.
pub(crate) fn filter_reach(op: &FilterOp, scale: f64) -> usize {
    let px = |v: f64| {
        let v = (v.abs() * scale).ceil();
        if v.is_finite() && v < 1e9 {
            v as usize
        } else {
            1_000_000_000
        }
    };
    match *op {
        FilterOp::Blur(sigma) => blur_reach(sigma * scale),
        FilterOp::DropShadow { dx, dy, sigma, .. } => blur_reach(sigma * scale)
            .saturating_add(px(dx))
            .saturating_add(px(dy)),
        FilterOp::Color(_) => 0,
    }
}

/// The side of the square device-pixel tiles the surface is rasterized in.
/// Tiles are the unit of work: a tile's pixels depend only on the display
/// list and the tile, so any set of tiles can be repainted alone and match
/// the full surface exactly.
pub const TILE_SIZE: u32 = 256;

/// Rasterize the pixels of `region` of the surface [`rasterize`] would
/// produce, into a surface of the region's size. The surface is painted in
/// `TILE_SIZE` tiles on a grid from its origin; this paints only the
/// tiles that meet `region`, each with enough context around it for every
/// filter (see `effect_margin`), so every pixel equals the full surface's.
pub fn rasterize_region(
    list: &DisplayList,
    scale: DeviceScale,
    region: DeviceRect,
    limits: RasterLimits,
    cancellation: &CancellationToken,
) -> Result<Surface, RasterError> {
    paint_region(list, scale, region, limits, true, cancellation)
}

/// Rasterize like [`rasterize`] without skipping any command by its
/// bounds: every tile runs every command. This is the reference that
/// skipping must match exactly, kept for tests.
pub fn rasterize_reference(
    list: &DisplayList,
    scale: DeviceScale,
    limits: RasterLimits,
    cancellation: &CancellationToken,
) -> Result<Surface, RasterError> {
    let (width, height) = surface_size(list, scale)?;
    let all = DeviceRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    paint_region(list, scale, all, limits, false, cancellation)
}

fn paint_region(
    list: &DisplayList,
    scale: DeviceScale,
    region: DeviceRect,
    limits: RasterLimits,
    cull: bool,
    cancellation: &CancellationToken,
) -> Result<Surface, RasterError> {
    cancellation.check().map_err(|_| RasterError::Cancelled)?;
    let (width, height) = surface_size(list, scale)?;
    if region.width == 0
        || region.height == 0
        || region.x.checked_add(region.width).is_none_or(|r| r > width)
        || region
            .y
            .checked_add(region.height)
            .is_none_or(|b| b > height)
    {
        return Err(RasterError::SurfaceSize);
    }
    let factor = device_factor(scale)?;
    let margin = effect_margin(list, factor)?;
    let culling = if cull {
        Some(Culling::new(list, factor)?)
    } else {
        None
    };
    let mut out = Surface::new(region.width, region.height, limits)?;
    let mut work = Work {
        used: 0,
        limit: limits.max_work,
        cancellation,
    };
    let (rx0, ry0) = (region.x as usize, region.y as usize);
    let (rx1, ry1) = (rx0 + region.width as usize, ry0 + region.height as usize);
    let tile = TILE_SIZE as usize;
    let grow = |start: usize, end: usize, limit: u32| {
        let lo = start.saturating_sub(margin);
        let hi = end.saturating_add(margin).min(limit as usize);
        (lo, hi - lo)
    };
    for ty in (ry0 / tile)..ry1.div_ceil(tile) {
        for tx in (rx0 / tile)..rx1.div_ceil(tile) {
            let (x0, y0) = (tx * tile, ty * tile);
            let (x1, y1) = (
                (x0 + tile).min(width as usize),
                (y0 + tile).min(height as usize),
            );
            let (fx, fw) = grow(x0, x1, width);
            let (fy, fh) = grow(y0, y1, height);
            let frame = Frame {
                x: fx,
                y: fy,
                w: fw,
                h: fh,
            };
            let painted = render(list, factor, frame, limits, culling.as_ref(), &mut work)?;
            // Copy the part of the tile inside the region.
            let (cx0, cy0) = (x0.max(rx0), y0.max(ry0));
            let (cx1, cy1) = (x1.min(rx1), y1.min(ry1));
            let row = (cx1 - cx0) * 4;
            let into = Frame {
                x: rx0,
                y: ry0,
                w: region.width as usize,
                h: region.height as usize,
            };
            for y in cy0..cy1 {
                let from = frame.offset(cx0, y);
                let to = into.offset(cx0, y);
                out.pixels[to..to + row].copy_from_slice(&painted.pixels[from..from + row]);
            }
        }
    }
    Ok(out)
}

/// Repaint `regions` of `surface`, a surface [`rasterize`] produced at
/// `scale`, from `list`: afterwards each region holds exactly the pixels
/// `rasterize(list, scale, …)` would. With the regions [`damage`] reports
/// for the list the surface was painted from, the whole surface then
/// equals a full rasterization of `list`.
///
/// [`damage`]: crate::damage
pub fn repaint(
    surface: &mut Surface,
    list: &DisplayList,
    scale: DeviceScale,
    regions: &[DeviceRect],
    limits: RasterLimits,
    cancellation: &CancellationToken,
) -> Result<(), RasterError> {
    if surface_size(list, scale)? != (surface.width, surface.height) {
        return Err(RasterError::SurfaceSize);
    }
    let into = Frame {
        x: 0,
        y: 0,
        w: surface.width as usize,
        h: surface.height as usize,
    };
    for &region in regions {
        let painted = rasterize_region(list, scale, region, limits, cancellation)?;
        let from = Frame {
            x: region.x as usize,
            y: region.y as usize,
            w: region.width as usize,
            h: region.height as usize,
        };
        let row = from.w * 4;
        for y in from.y..from.y + from.h {
            let a = from.offset(from.x, y);
            let b = into.offset(from.x, y);
            surface.pixels[b..b + row].copy_from_slice(&painted.pixels[a..a + row]);
        }
    }
    Ok(())
}

/// Rasterize `frame` of the surface into a surface of the frame's size.
fn render(
    list: &DisplayList,
    factor: f64,
    frame: Frame,
    limits: RasterLimits,
    culling: Option<&Culling>,
    work: &mut Work<'_>,
) -> Result<Surface, RasterError> {
    let to_u32 = |v: usize| u32::try_from(v).map_err(|_| RasterError::SurfaceSize);
    let mut surface = Surface::new(to_u32(frame.w)?, to_u32(frame.h)?, limits)?;
    work.charge(frame.w as u64 * frame.h as u64)?;
    surface.fill_canvas(list.canvas());
    let size = (surface.width, surface.height);
    let bytes = surface.pixels.len();
    // Open layers, innermost last; a transform without clips has no buffer.
    let mut groups: Vec<(Option<Vec<u8>>, Layer)> = Vec::new();
    // Local-to-device maps of the open transforms, innermost last.
    let mut maps: Vec<Homography> = Vec::new();
    let window_size = (frame.w, frame.h);
    let origin = (frame.x as f64, frame.y as f64);
    let commands = list.commands();
    let mut next = 0;
    while let Some(command) = commands.get(next) {
        let index = next;
        next += 1;
        // Skip a fill or a whole layer that cannot change the frame.
        if let (
            Some(culling),
            DisplayCommand::Fill(_)
            | DisplayCommand::BeginGroup { .. }
            | DisplayCommand::BeginClip { .. }
            | DisplayCommand::BeginTransform { .. },
        ) = (culling, command)
        {
            if let Some(after) = culling.skip(index, frame.bounds()) {
                next = after;
                continue;
            }
        }
        match command {
            DisplayCommand::Fill(item) => {
                let pixels = target(&mut groups, &mut surface.pixels);
                if matches!(item.shape, Shape::Shadow(_)) {
                    shadow(pixels, frame, item, list, maps.last(), factor, work)?;
                    continue;
                }
                let image = matches!(item.shape, Shape::Image { .. });
                match (maps.last(), image) {
                    (None, true) => draw_image(pixels, frame, item, list, factor, work)?,
                    (None, false) => fill(pixels, frame, item, list, factor, work)?,
                    (Some(map), true) => {
                        draw_image_mapped(pixels, frame, item, list, map, work)?;
                    }
                    (Some(map), false) => fill_mapped(pixels, frame, item, list, map, work)?,
                }
            }
            DisplayCommand::BeginGroup { .. }
            | DisplayCommand::BeginClip { .. }
            | DisplayCommand::BeginTransform { .. } => {
                let layer = match command {
                    DisplayCommand::BeginGroup {
                        opacity,
                        blend,
                        filter,
                        ..
                    } => Layer::Group {
                        opacity: *opacity,
                        blend: *blend,
                        filter: *filter,
                        scale: group_scale(maps.last(), factor),
                    },
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
                    (
                        DisplayCommand::EndGroup,
                        Layer::Group {
                            opacity,
                            blend,
                            filter,
                            scale,
                        },
                    ) => {
                        let mut group = group.ok_or(RasterError::InvalidList)?;
                        work.charge(u64::from(size.0) * u64::from(size.1))?;
                        if let Some(f) = filter {
                            let ops = list.filters().get(f).ok_or(RasterError::InvalidList)?;
                            apply_filter(&mut group, window_size, ops, scale, work)?;
                        }
                        composite_blend(
                            target(&mut groups, &mut surface.pixels),
                            &group,
                            opacity,
                            blend,
                        );
                        continue;
                    }
                    (DisplayCommand::EndClip, Layer::Clip(shape, None)) => {
                        let contours = contours(&shape, list.paths(), list.rounded(), factor);
                        work.charge(contours.iter().map(|c| c.len() as u64).sum())?;
                        coverage(&contours, origin, window_size, work)?
                    }
                    (DisplayCommand::EndClip, Layer::Clip(shape, Some(map))) => {
                        let local = local_contours(&shape, list);
                        let contours: Vec<Vec<Point>> =
                            local.iter().map(|c| project(c, &map, None)).collect();
                        work.charge(contours.iter().map(|c| c.len() as u64).sum())?;
                        coverage(&contours, origin, window_size, work)?
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
                        let mut mask = coverage(&[area], origin, window_size, work)?;
                        let nodes = chain(list.clips(), rounded_clip)?;
                        if let Some(rounded) =
                            clip_mask_mapped(&nodes, &outer, origin, window_size, work)?
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

    #[test]
    fn foreign_surfaces_are_checked_and_frames_add_by_coverage() {
        let limits = RasterLimits::default();
        assert!(Surface::from_premultiplied(1, 1, vec![0, 0, 0, 255], limits).is_some());
        // Wrong length, color above alpha, empty or over-budget sizes.
        assert!(Surface::from_premultiplied(1, 1, vec![0; 3], limits).is_none());
        assert!(Surface::from_premultiplied(1, 1, vec![9, 0, 0, 8], limits).is_none());
        assert!(Surface::from_premultiplied(0, 1, Vec::new(), limits).is_none());
        assert!(Surface::from_premultiplied(u32::MAX, 1, Vec::new(), limits).is_none());
        // A 2x1 page whose right pixel is a black frame seen through 50%
        // opacity over white: half the frame's color shows through.
        let mut page =
            Surface::from_premultiplied(2, 1, vec![255, 255, 255, 255, 128, 128, 128, 255], limits)
                .unwrap();
        let frame = Surface::from_premultiplied(1, 1, vec![0, 0, 255, 255], limits).unwrap();
        let before = page.clone();
        assert_eq!(
            page.add_frame(&frame, (1, 0, 2, 1), &[255, 255]),
            Err(RasterError::SurfaceSize)
        );
        assert_eq!(
            page.add_frame(&frame, (1, 0, 1, 1), &[]),
            Err(RasterError::SurfaceSize)
        );
        assert_eq!(
            page.add_frame(&frame, (u32::MAX, 0, 2, 1), &[0, 0]),
            Err(RasterError::SurfaceSize)
        );
        assert_eq!(page, before);
        page.add_frame(&frame, (1, 0, 1, 1), &[127]).unwrap();
        assert_eq!(
            page.premultiplied(),
            &[255, 255, 255, 255, 128, 128, 255, 255]
        );
        // Color never exceeds alpha, and the frame's edge pixel extends.
        let mut faint =
            Surface::from_premultiplied(2, 1, vec![0, 0, 0, 100, 0, 0, 0, 100], limits).unwrap();
        faint.add_frame(&frame, (0, 0, 2, 1), &[255, 255]).unwrap();
        assert_eq!(faint.premultiplied(), &[0, 0, 100, 100, 0, 0, 100, 100]);
    }

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
    fn gaussian_blur_is_deterministic_conserves_mass_and_is_symmetric() {
        for x in [0.0, 0.1, 0.5, 1.0, 2.5, 10.0, 100.0] {
            let e = exp_neg(x);
            assert!(
                (e - (-x).exp()).abs() <= 1e-13 * (-x).exp().max(1e-300),
                "{x}: {e}"
            );
        }
        assert_eq!(exp_neg(1000.0), 0.0);
        let cancel = CancellationToken::default();
        let mut work = Work {
            used: 0,
            limit: u64::MAX,
            cancellation: &cancel,
        };
        for sigma in [0.6, 1.5, 2.0, 3.3, 8.0] {
            // A one-pixel impulse in the middle of a wide line.
            let (w, h) = (101, 1);
            let mut line = vec![0.0; w];
            line[50] = 1.0;
            blur(&mut line, (w, h), sigma, &mut work).unwrap();
            let total: f64 = line.iter().sum();
            assert!((total - 1.0).abs() < 1e-8, "{sigma}: {total}");
            for k in 1..40 {
                assert!((line[50 - k] - line[50 + k]).abs() < 1e-9, "{sigma} {k}");
                assert!(line[50 + k] <= line[50 + k - 1] + 1e-9, "{sigma} {k}");
            }
            // The spread matches the standard deviation within a few percent.
            let variance: f64 = line
                .iter()
                .enumerate()
                .map(|(i, v)| v * (i as f64 - 50.0).powi(2))
                .sum();
            let ratio = variance.sqrt() / sigma;
            assert!((0.9..1.1).contains(&ratio), "{sigma}: {ratio}");
        }
        // A step blurs to one half at the edge.
        let mut step: Vec<f64> = (0..60).map(|i| if i < 30 { 1.0 } else { 0.0 }).collect();
        blur(&mut step, (60, 1), 4.0, &mut work).unwrap();
        assert!((step[29] + step[30] - 1.0).abs() < 1e-8);
        assert!(step[0] > 0.999 && step[59] < 0.001);
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
