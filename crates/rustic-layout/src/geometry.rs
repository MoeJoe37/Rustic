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

    /// Device pixels per CSS pixel.
    pub const fn get(self) -> f64 {
        self.0
    }
}

/// The window a page is laid out and painted in: its size in pixels at
/// 100% zoom (CSS pixels before zoom, as `window.innerWidth` reports at
/// 100%), the device pixel ratio and the page zoom.
///
/// Page zoom scales CSS pixels: the layout viewport is the window divided
/// by the zoom, and the device scale is the ratio times the zoom, so a
/// zoomed page reflows into a narrower viewport and paints larger. Resizing
/// or zooming means laying the page out again with [`View::viewport`],
/// computing styles against it first (viewport units and media queries
/// depend on it), and painting at [`View::scale`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    window: CssSize,
    ratio: f64,
    zoom: f64,
}

impl View {
    /// The smallest page zoom accepted (25%).
    pub const MIN_ZOOM: f64 = 0.25;
    /// The largest page zoom accepted (500%).
    pub const MAX_ZOOM: f64 = 5.0;

    /// A view of a `width` × `height` window at `ratio` device pixels per
    /// pixel and page `zoom`. Fails unless the window is at least one pixel
    /// each way, the zoom is within [`Self::MIN_ZOOM`]..=[`Self::MAX_ZOOM`]
    /// and `ratio × zoom` is a valid [`DeviceScale`].
    pub fn new(width: f64, height: f64, ratio: f64, zoom: f64) -> Result<Self, GeometryError> {
        if !(width >= 1.0 && height >= 1.0) || !(Self::MIN_ZOOM..=Self::MAX_ZOOM).contains(&zoom) {
            return Err(GeometryError);
        }
        DeviceScale::new(ratio)?;
        DeviceScale::new(ratio * zoom)?;
        CssSize::new(width / zoom, height / zoom)?;
        Ok(Self {
            window: CssSize::new(width, height)?,
            ratio,
            zoom,
        })
    }

    /// The same view with the window resized.
    pub fn resized(self, width: f64, height: f64) -> Result<Self, GeometryError> {
        Self::new(width, height, self.ratio, self.zoom)
    }

    /// The same view at another page zoom.
    pub fn zoomed(self, zoom: f64) -> Result<Self, GeometryError> {
        Self::new(
            self.window.width.get(),
            self.window.height.get(),
            self.ratio,
            zoom,
        )
    }

    /// The window size at 100% zoom.
    pub const fn window(self) -> CssSize {
        self.window
    }

    /// The page zoom.
    pub const fn zoom(self) -> f64 {
        self.zoom
    }

    /// The layout viewport in CSS pixels: the window over the zoom.
    pub fn viewport(self) -> CssSize {
        CssSize {
            width: CssPx(self.window.width.get() / self.zoom),
            height: CssPx(self.window.height.get() / self.zoom),
        }
    }

    /// Device pixels per CSS pixel: the device pixel ratio times the zoom.
    pub fn scale(self) -> DeviceScale {
        DeviceScale(self.ratio * self.zoom)
    }

    /// The CSS point under a window point given in device pixels.
    pub fn to_css(self, device_x: f64, device_y: f64) -> Result<CssPoint, GeometryError> {
        let scale = self.ratio * self.zoom;
        Ok(CssPoint {
            x: CssPx::new(device_x / scale)?,
            y: CssPx::new(device_y / scale)?,
        })
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
