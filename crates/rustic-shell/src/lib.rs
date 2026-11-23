//! Rustic's desktop host (UI-02, ADR 0065): one native window, its event
//! loop, and presentation of CPU-rendered surfaces.
//!
//! The shell owns no web semantics. It asks a [`Content`] for a surface at
//! the window's size, device scale and scroll position, and shows it with
//! softbuffer. It turns wheel and keyboard input into scroll requests and
//! closes on the window's close button or Escape. A [`Content`] may also
//! give a trusted strip (the address display and permission prompts) that
//! the shell keeps above the page: page pixels never reach its rows, and
//! clicks there go to the strip, never to the page. winit and softbuffer
//! stay behind this module: the engine crates never see them, and the
//! window, display connection and buffers live only on the thread that runs
//! [`run`].

use rustic_raster::Surface;

// Android and iOS have no desktop window: their hosts are an activity and
// a BrowserEngineKit app (ADR 0059), so winit and softbuffer are not built
// there and `run` reports that.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod accessibility;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod window;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use window::run;

/// Android and iOS: no desktop window exists; the mobile host owns
/// presentation.
#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn run(_config: WindowConfig, _content: impl Content) -> Result<(), String> {
    Err("cannot reach the window system: mobile platforms have no desktop window".into())
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

    /// The trusted strip's height in CSS pixels (0 for none).
    fn chrome_height(&self) -> f64 {
        0.0
    }

    /// The trusted strip above the page (address display, permission
    /// prompt) at `width` device pixels, drawn by the host, never by the
    /// page. `None` leaves the whole window to the page.
    fn chrome(&mut self, _width: u32, _scale: f64) -> Result<Option<Surface>, String> {
        Ok(None)
    }

    /// A primary click inside the strip at (`x`, `y`) CSS pixels from its
    /// top-left corner; returns whether the strip changed.
    fn chrome_click(&mut self, _x: f64, _y: f64) -> bool {
        false
    }

    /// Escape: returns whether the strip used it (closing a prompt) rather
    /// than the window closing.
    fn chrome_escape(&mut self) -> bool {
        false
    }

    /// The page's accessibility tree as last painted, in CSS pixels from
    /// the page area's top-left corner (ADR 0073). Asked after each frame
    /// while an assistive technology is listening; `None` shows an empty
    /// window to it.
    fn accessibility(&mut self) -> Option<rustic_a11y::AxTree> {
        None
    }

    /// An assistive technology asked page node `node` to do `action`;
    /// returns whether the page may have changed (the window repaints).
    fn accessibility_action(
        &mut self,
        _node: rustic_a11y::AxId,
        _action: rustic_a11y::ActionRequest,
    ) -> bool {
        false
    }
}

/// Compose a window buffer of `width × height` 0RGB pixels: the trusted
/// `chrome` strip in its rows at the top, then `page` below it. Page pixels
/// never reach the strip's rows, whatever the page's size (a page cannot
/// paint over the address display or a prompt). Returns the strip's
/// height in device pixels.
pub fn present(
    page: &Surface,
    chrome: Option<&Surface>,
    width: u32,
    height: u32,
    buffer: &mut [u32],
) -> u32 {
    let top = chrome.map_or(0, |chrome| chrome.height().min(height));
    let start = (top as usize)
        .saturating_mul(width as usize)
        .min(buffer.len());
    present_over_white(page, width, height - top, &mut buffer[start..]);
    if let Some(chrome) = chrome {
        present_over_white(chrome, width, top, &mut buffer[..start]);
    }
    top
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
    fn page_pixels_never_reach_the_trusted_strip() {
        let limits = rustic_raster::RasterLimits::default();
        // A page as tall as the whole window, all black.
        let page = Surface::from_premultiplied(4, 4, [0, 0, 0, 255].repeat(16), limits).unwrap();
        let chrome = Surface::from_premultiplied(4, 1, [0, 0, 255, 255].repeat(4), limits).unwrap();
        let mut buffer = vec![0x00AB_CDEF; 4 * 4];
        let top = present(&page, Some(&chrome), 4, 4, &mut buffer);
        assert_eq!(top, 1);
        assert_eq!(&buffer[..4], &[0x0000_00FF; 4]);
        assert_eq!(&buffer[4..], &[0; 12]);
        // A strip taller than the window takes it all; no page shows.
        let tall = Surface::from_premultiplied(4, 9, [0, 0, 255, 255].repeat(36), limits).unwrap();
        assert_eq!(present(&page, Some(&tall), 4, 4, &mut buffer), 4);
        assert_eq!(&buffer[..], &[0x0000_00FF; 16]);
        // No strip: the page has the window.
        assert_eq!(present(&page, None, 4, 4, &mut buffer), 0);
        assert_eq!(&buffer[..], &[0; 16]);
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
