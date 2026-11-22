//! Rustic's desktop host (UI-02, ADR 0065): one native window, its event
//! loop, and presentation of CPU-rendered surfaces.
//!
//! The shell owns no web semantics. It asks a [`Content`] for a surface at
//! the window's size, device scale and scroll position, and shows it with
//! softbuffer. It turns wheel and keyboard input into scroll requests and
//! closes on the window's close button or Escape. winit and softbuffer
//! stay behind this module: the engine crates never see them, and the
//! window, display connection and buffers live only on the thread that runs
//! [`run`].

use rustic_raster::Surface;

// Android has no desktop window: its host is an activity (ADR 0059), so
// winit and softbuffer are not built there and `run` reports that.
#[cfg(not(target_os = "android"))]
mod window;
#[cfg(not(target_os = "android"))]
pub use window::run;

/// Android: no desktop window exists; the mobile host owns presentation.
#[cfg(target_os = "android")]
pub fn run(_config: WindowConfig, _content: impl Content) -> Result<(), String> {
    Err("cannot reach the window system: Android has no desktop window".into())
}

/// CSS pixels one wheel line or arrow key scrolls.
pub const LINE: f64 = 40.0;

/// The window to open.
#[derive(Clone, Debug)]
pub struct WindowConfig {
    /// The title.
    pub title: String,
    /// The initial inner size in logical (CSS) pixels.
    pub size: (u32, u32),
    /// Close after presenting this many frames (tests and smoke runs).
    pub exit_after: Option<u32>,
}

/// One painted frame: its pixels and the scroll position it shows.
#[derive(Debug)]
pub struct Painted {
    /// Premultiplied RGBA at device pixels.
    pub surface: Surface,
    /// The viewport scroll position actually used (after clamping), in
    /// CSS pixels.
    pub scroll: (f64, f64),
}

/// What fills the window.
pub trait Content {
    /// Paint the page for a window of `size` device pixels at `scale`
    /// device pixels per CSS pixel, scrolled to `scroll` CSS pixels.
    fn paint(
        &mut self,
        size: (u32, u32),
        scale: f64,
        scroll: (f64, f64),
    ) -> Result<Painted, String>;
}

/// Write `surface` into a window buffer of `width × height` 0RGB pixels,
/// over white: premultiplied color plus white times the missing alpha.
/// Pixels the surface does not cover are white.
pub fn present_over_white(surface: &Surface, width: u32, height: u32, buffer: &mut [u32]) {
    let pixels = surface.premultiplied();
    for y in 0..height {
        for x in 0..width {
            let target = (y * width + x) as usize;
            let Some(slot) = buffer.get_mut(target) else {
                return;
            };
            *slot = if x < surface.width() && y < surface.height() {
                let i = ((y * surface.width() + x) * 4) as usize;
                let white = 255 - u32::from(pixels[i + 3]);
                let channel = |c: u8| (u32::from(c) + white).min(255);
                (channel(pixels[i]) << 16) | (channel(pixels[i + 1]) << 8) | channel(pixels[i + 2])
            } else {
                0x00FF_FFFF
            };
        }
    }
}

/// The scroll position after one input, before the content clamps it.
pub fn scrolled_by(input: &ScrollInput, scroll: (f64, f64), scale: f64, page: f64) -> (f64, f64) {
    let (x, y) = scroll;
    let next = match *input {
        ScrollInput::Lines(dx, dy) => (x - dx * LINE, y - dy * LINE),
        ScrollInput::Pixels(dx, dy) => (x - dx / scale, y - dy / scale),
        ScrollInput::Down => (x, y + LINE),
        ScrollInput::Up => (x, y - LINE),
        ScrollInput::PageDown => (x, y + page * 0.875),
        ScrollInput::PageUp => (x, y - page * 0.875),
        ScrollInput::Home => (x, 0.0),
    };
    (next.0.max(0.0), next.1.max(0.0))
}

/// A scroll request from the user.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollInput {
    /// Wheel lines (positive scrolls up/left, as winit reports).
    Lines(f64, f64),
    /// Touchpad pixels (device pixels, positive up/left).
    Pixels(f64, f64),
    /// Arrow down.
    Down,
    /// Arrow up.
    Up,
    /// Page down or space.
    PageDown,
    /// Page up.
    PageUp,
    /// Home.
    Home,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surfaces_present_over_white_and_uncovered_pixels_stay_white() {
        let surface = Surface::from_premultiplied(
            2,
            1,
            vec![255, 0, 0, 255, 0, 0, 64, 128],
            rustic_raster::RasterLimits::default(),
        )
        .unwrap();
        let mut buffer = vec![0; 3 * 2];
        present_over_white(&surface, 3, 2, &mut buffer);
        // Opaque red; half-transparent blue over white; then white.
        assert_eq!(buffer[0], 0x00FF_0000);
        assert_eq!(buffer[1], 0x007F_7FBF);
        assert_eq!(&buffer[2..], &[0x00FF_FFFF; 4]);
        // A smaller buffer than the window never panics.
        present_over_white(&surface, 3, 2, &mut [0; 2]);
    }

    #[test]
    fn scroll_inputs_move_and_stop_at_the_origin() {
        let at = (0.0, 100.0);
        assert_eq!(
            scrolled_by(&ScrollInput::Lines(0.0, -1.0), at, 2.0, 600.0),
            (0.0, 140.0)
        );
        assert_eq!(
            scrolled_by(&ScrollInput::Pixels(0.0, -20.0), at, 2.0, 600.0),
            (0.0, 110.0)
        );
        assert_eq!(
            scrolled_by(&ScrollInput::PageDown, at, 1.0, 600.0),
            (0.0, 625.0)
        );
        assert_eq!(
            scrolled_by(&ScrollInput::PageUp, at, 1.0, 600.0),
            (0.0, 0.0)
        );
        assert_eq!(scrolled_by(&ScrollInput::Up, at, 1.0, 600.0), (0.0, 60.0));
        assert_eq!(scrolled_by(&ScrollInput::Home, at, 1.0, 600.0), (0.0, 0.0));
    }
}
