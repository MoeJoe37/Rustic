//! Unit-specific, checked reference geometry. No implicit device rounding.

use std::{error::Error, fmt};

/// Maximum absolute CSS coordinate accepted by this reference implementation
/// (the same bound as [`rustic_base::MAX_COORDINATE`]).
pub const MAX_CSS_PX: f64 = rustic_base::MAX_COORDINATE;

/// A finite CSS pixel coordinate, bounded before geometry arithmetic.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct CssPx(f64);

impl CssPx {
    /// Zero CSS pixels.
    pub const ZERO: Self = Self(0.0);

    /// Reject nonfinite or out-of-budget coordinates; normalize negative zero.
    pub fn new(value: f64) -> Result<Self, GeometryError> {
        if !value.is_finite() || value.abs() > MAX_CSS_PX {
            return Err(GeometryError);
        }
        Ok(Self(if value == 0.0 { 0.0 } else { value }))
    }

    /// The value in CSS pixels.
    pub const fn get(self) -> f64 {
        self.0
    }

    /// Add without overflowing the coordinate budget.
    pub fn checked_add(self, other: Self) -> Result<Self, GeometryError> {
        Self::new(self.0 + other.0)
    }

    /// Subtract without overflowing the coordinate budget.
    pub fn checked_sub(self, other: Self) -> Result<Self, GeometryError> {
        Self::new(self.0 - other.0)
    }

    /// Convert to device pixels, retaining the fractional coordinate.
    pub fn to_device(self, scale: DeviceScale) -> DevicePx {
        // Both operands are checked and their product is at most 64e9.
        DevicePx(self.0 * scale.0)
    }
}

/// A device coordinate. Surface allocation and raster rounding are later work.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DevicePx(f64);

impl DevicePx {
    /// The fractional device-pixel coordinate.
    pub const fn get(self) -> f64 {
        self.0
    }
}

/// Positive device pixels per CSS pixel, bounded at 64 for this prototype.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceScale(f64);

impl DeviceScale {
    /// The same factor as a unit-typed base scale from CSS to device pixels.
    pub fn to_typed(
        self,
    ) -> Result<rustic_base::Scale<rustic_base::Css, rustic_base::Device>, GeometryError> {
        rustic_base::Scale::new(self.0).map_err(|_| GeometryError)
    }

    /// Validate an embedder's scale independently of page dimensions.
    pub fn new(value: f64) -> Result<Self, GeometryError> {
        if !value.is_finite() || value <= 0.0 || value > 64.0 {
            return Err(GeometryError);
        }
        Ok(Self(value))
    }
}

/// A point in CSS pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CssPoint {
    /// Horizontal coordinate.
    pub x: CssPx,
    /// Vertical coordinate.
    pub y: CssPx,
}

/// Nonnegative CSS dimensions.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CssSize {
    width: CssPx,
    height: CssPx,
}

impl CssSize {
    /// Reject negative dimensions as well as invalid coordinates.
    pub fn new(width: f64, height: f64) -> Result<Self, GeometryError> {
        if width < 0.0 || height < 0.0 {
            return Err(GeometryError);
        }
        Ok(Self {
            width: CssPx::new(width)?,
            height: CssPx::new(height)?,
        })
    }

    /// Width in CSS pixels.
    pub const fn width(self) -> CssPx {
        self.width
    }

    /// Height in CSS pixels.
    pub const fn height(self) -> CssPx {
        self.height
    }
}

/// A CSS rectangle with all four edges inside the coordinate budget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CssRect {
    origin: CssPoint,
    size: CssSize,
}

impl CssRect {
    /// The same rectangle as unit-typed base geometry. Both share one
    /// coordinate bound, so this fails only for values built unchecked.
    pub fn to_typed(self) -> Result<rustic_base::Rect<rustic_base::Css>, GeometryError> {
        let point = rustic_base::Point2::new(self.origin.x.get(), self.origin.y.get());
        let size = rustic_base::Size2::new(self.size.width.get(), self.size.height.get());
        point
            .and_then(|p| rustic_base::Rect::new(p, size?))
            .map_err(|_| GeometryError)
    }

    /// Validate far edges before retaining the rectangle.
    pub fn new(origin: CssPoint, size: CssSize) -> Result<Self, GeometryError> {
        origin.x.checked_add(size.width)?;
        origin.y.checked_add(size.height)?;
        Ok(Self { origin, size })
    }

    /// Top-left corner in CSS pixels.
    pub const fn origin(self) -> CssPoint {
        self.origin
    }

    /// Nonnegative CSS dimensions.
    pub const fn size(self) -> CssSize {
        self.size
    }
}

/// Nonfinite, negative-size or out-of-budget geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GeometryError;

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("geometry is nonfinite, negative-sized or exceeds the coordinate budget")
    }
}

impl Error for GeometryError {}
