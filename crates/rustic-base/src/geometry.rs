//! Unit-typed geometry and checked surface allocation.
//!
//! Lengths, points, sizes and rectangles carry a unit marker ([`Css`] or
//! [`Device`]), so a CSS coordinate cannot be mixed with a device pixel by
//! accident: the only way between units is an explicit [`Scale`]. Every
//! value is finite and within ±[`MAX_COORDINATE`], checked on construction
//! and after arithmetic, and sizes are never negative.
//!
//! [`SurfaceAllocation`] decides whether a pixel surface may be allocated:
//! nonzero dimensions within per-axis limits and a byte total, computed
//! with checked integer arithmetic, within a byte budget. It is the one
//! place where layout geometry becomes a memory request.

use std::{error::Error, fmt, marker::PhantomData};

/// Largest absolute coordinate in any unit.
pub const MAX_COORDINATE: f64 = 1_000_000_000.0;

/// CSS pixels (the layout coordinate space).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Css;

/// Device pixels (a raster surface's coordinate space).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Device;

/// Nonfinite, negative-sized or out-of-range geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GeometryError;

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("geometry is nonfinite, negative-sized or out of range")
    }
}

impl Error for GeometryError {}

/// A finite length in unit `U`.
#[derive(Debug)]
pub struct Length<U> {
    value: f64,
    unit: PhantomData<U>,
}

// Manual impls: derives would require `U` itself to implement the traits.
impl<U> Clone for Length<U> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<U> Copy for Length<U> {}
impl<U> PartialEq for Length<U> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl<U> PartialOrd for Length<U> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.value.partial_cmp(&other.value)
    }
}

impl<U> Length<U> {
    /// Zero.
    pub const ZERO: Self = Self {
        value: 0.0,
        unit: PhantomData,
    };

    /// Reject nonfinite or out-of-range values; normalize negative zero.
    pub fn new(value: f64) -> Result<Self, GeometryError> {
        if !value.is_finite() || value.abs() > MAX_COORDINATE {
            return Err(GeometryError);
        }
        Ok(Self {
            value: if value == 0.0 { 0.0 } else { value },
            unit: PhantomData,
        })
    }

    /// The value in unit `U`.
    pub const fn get(self) -> f64 {
        self.value
    }

    /// Checked sum.
    pub fn checked_add(self, other: Self) -> Result<Self, GeometryError> {
        Self::new(self.value + other.value)
    }

    /// Checked difference.
    pub fn checked_sub(self, other: Self) -> Result<Self, GeometryError> {
        Self::new(self.value - other.value)
    }
}

/// A point in unit `U`.
#[derive(Debug)]
pub struct Point2<U> {
    /// Horizontal coordinate.
    pub x: Length<U>,
    /// Vertical coordinate.
    pub y: Length<U>,
}

impl<U> Clone for Point2<U> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<U> Copy for Point2<U> {}
impl<U> PartialEq for Point2<U> {
    fn eq(&self, other: &Self) -> bool {
        self.x == other.x && self.y == other.y
    }
}

impl<U> Point2<U> {
    /// A checked point.
    pub fn new(x: f64, y: f64) -> Result<Self, GeometryError> {
        Ok(Self {
            x: Length::new(x)?,
            y: Length::new(y)?,
        })
    }
}

/// Nonnegative dimensions in unit `U`.
#[derive(Debug)]
pub struct Size2<U> {
    width: Length<U>,
    height: Length<U>,
}

impl<U> Clone for Size2<U> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<U> Copy for Size2<U> {}
impl<U> PartialEq for Size2<U> {
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width && self.height == other.height
    }
}

impl<U> Size2<U> {
    /// Reject negative as well as invalid dimensions.
    pub fn new(width: f64, height: f64) -> Result<Self, GeometryError> {
        if width < 0.0 || height < 0.0 {
            return Err(GeometryError);
        }
        Ok(Self {
            width: Length::new(width)?,
            height: Length::new(height)?,
        })
    }
    /// Width.
    pub const fn width(self) -> Length<U> {
        self.width
    }
    /// Height.
    pub const fn height(self) -> Length<U> {
        self.height
    }
}

/// A rectangle in unit `U` whose far edges are also in range.
#[derive(Debug)]
pub struct Rect<U> {
    origin: Point2<U>,
    size: Size2<U>,
}

impl<U> Clone for Rect<U> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<U> Copy for Rect<U> {}
impl<U> PartialEq for Rect<U> {
    fn eq(&self, other: &Self) -> bool {
        self.origin == other.origin && self.size == other.size
    }
}

impl<U> Rect<U> {
    /// Check the far edges before retaining the rectangle.
    pub fn new(origin: Point2<U>, size: Size2<U>) -> Result<Self, GeometryError> {
        origin.x.checked_add(size.width)?;
        origin.y.checked_add(size.height)?;
        Ok(Self { origin, size })
    }
    /// Top-left corner.
    pub const fn origin(self) -> Point2<U> {
        self.origin
    }
    /// Dimensions.
    pub const fn size(self) -> Size2<U> {
        self.size
    }
    /// Right edge.
    pub fn max_x(self) -> f64 {
        self.origin.x.get() + self.size.width.get()
    }
    /// Bottom edge.
    pub fn max_y(self) -> f64 {
        self.origin.y.get() + self.size.height.get()
    }
}

/// A positive factor from unit `S` to unit `D` (device pixels per CSS
/// pixel for `Scale<Css, Device>`), bounded at 64.
#[derive(Debug)]
pub struct Scale<S, D> {
    factor: f64,
    units: PhantomData<(S, D)>,
}

impl<S, D> Clone for Scale<S, D> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<S, D> Copy for Scale<S, D> {}
impl<S, D> PartialEq for Scale<S, D> {
    fn eq(&self, other: &Self) -> bool {
        self.factor == other.factor
    }
}

impl<S, D> Scale<S, D> {
    /// Accept a finite factor in `(0, 64]`.
    pub fn new(factor: f64) -> Result<Self, GeometryError> {
        if !factor.is_finite() || factor <= 0.0 || factor > 64.0 {
            return Err(GeometryError);
        }
        Ok(Self {
            factor,
            units: PhantomData,
        })
    }
    /// The factor.
    pub const fn get(self) -> f64 {
        self.factor
    }
    /// Convert a length, checking the result's range.
    pub fn length(self, l: Length<S>) -> Result<Length<D>, GeometryError> {
        Length::new(l.get() * self.factor)
    }
    /// Convert a rectangle, checking every edge.
    pub fn rect(self, r: Rect<S>) -> Result<Rect<D>, GeometryError> {
        Rect::new(
            Point2 {
                x: self.length(r.origin.x)?,
                y: self.length(r.origin.y)?,
            },
            Size2::new(
                r.size.width.get() * self.factor,
                r.size.height.get() * self.factor,
            )?,
        )
    }
}

/// Limits for one pixel surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceLimits {
    /// Largest width in device pixels.
    pub max_width: u32,
    /// Largest height in device pixels.
    pub max_height: u32,
    /// Largest total byte size.
    pub max_bytes: usize,
}

/// Why a surface may not be allocated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceError {
    /// A zero dimension or bytes per pixel.
    Empty,
    /// A dimension above its per-axis limit.
    Dimension,
    /// The byte total overflows or exceeds the budget.
    Bytes,
    /// The device rectangle is invalid.
    Geometry,
}

impl fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "surface has a zero dimension",
            Self::Dimension => "surface dimension exceeds its limit",
            Self::Bytes => "surface byte size overflows or exceeds its budget",
            Self::Geometry => "surface rectangle is invalid",
        })
    }
}

impl Error for SurfaceError {}

/// A checked surface request: dimensions and the exact byte count.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceAllocation {
    width: u32,
    height: u32,
    bytes: usize,
}

impl SurfaceAllocation {
    /// Check `width × height × bytes_per_pixel` against `limits` with
    /// checked integer arithmetic; no memory is allocated here.
    pub fn check(
        width: u32,
        height: u32,
        bytes_per_pixel: usize,
        limits: SurfaceLimits,
    ) -> Result<Self, SurfaceError> {
        if width == 0 || height == 0 || bytes_per_pixel == 0 {
            return Err(SurfaceError::Empty);
        }
        if width > limits.max_width || height > limits.max_height {
            return Err(SurfaceError::Dimension);
        }
        let bytes = usize::try_from(width)
            .ok()
            .zip(usize::try_from(height).ok())
            .and_then(|(w, h)| w.checked_mul(h)?.checked_mul(bytes_per_pixel))
            .filter(|&n| n <= limits.max_bytes)
            .ok_or(SurfaceError::Bytes)?;
        Ok(Self {
            width,
            height,
            bytes,
        })
    }

    /// The surface covering a device rectangle: its edges rounded outwards
    /// to whole pixels (a rectangle starting at a fractional coordinate
    /// still covers every pixel it touches).
    pub fn covering(
        rect: Rect<Device>,
        bytes_per_pixel: usize,
        limits: SurfaceLimits,
    ) -> Result<Self, SurfaceError> {
        let x0 = rect.origin().x.get().floor();
        let y0 = rect.origin().y.get().floor();
        let (x1, y1) = (rect.max_x().ceil(), rect.max_y().ceil());
        let dimension = |n: f64| -> Result<u32, SurfaceError> {
            if !(0.0..=f64::from(u32::MAX)).contains(&n) {
                return Err(SurfaceError::Geometry);
            }
            // In range and integral after floor/ceil, so exact.
            Ok(n as u32)
        };
        Self::check(
            dimension(x1 - x0)?,
            dimension(y1 - y0)?,
            bytes_per_pixel,
            limits,
        )
    }

    /// Width in device pixels.
    pub const fn width(self) -> u32 {
        self.width
    }
    /// Height in device pixels.
    pub const fn height(self) -> u32 {
        self.height
    }
    /// Exact byte size.
    pub const fn bytes(self) -> usize {
        self.bytes
    }
}
