//! Rustic's desktop host (UI-02, ADR 0065, ADR 0069): one native window,
//! its event loop, presentation of CPU-rendered surfaces, and the host's
//! side of input, IME and window lifecycle.
//!
//! The shell owns no web semantics. It asks a [`Content`] for a surface at
//! the window's size and device scale and shows it with softbuffer. It
//! turns the platform's keyboard, text, IME, pointer and wheel events into
//! [`Input`] and its resize, scale, focus and visibility changes into
//! [`Lifecycle`] events, and hands both to the content; what the content
//! leaves unhandled falls back to scrolling (wheel, arrows, Page Up/Down,
//! space, Home, End) and closing (Escape). A [`Content`] may also give a
//! trusted strip (the browser's chrome: tabs, toolbar, address, prompts)
//! that the shell keeps above the page: page pixels never reach its rows,
//! and clicks there go to the strip, never to the page. winit and
//! softbuffer stay behind this module: the engine crates never see them,
//! and the window, display connection and buffers live only on the thread
//! that runs [`run`].
//!
//! A hidden window (minimized, fully occluded, or zero-sized) presents
//! nothing; the content is told so it can stop work it would only throw
//! away. [`Script`] drives a real window through resizes, minimize and
//! restore, key presses, text and IME composition for tests on every
//! desktop platform, using the same handlers as the platform's events.

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

mod script;
pub use script::{Script, Step};

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
    /// Close after presenting this many settled frames (tests and smoke
    /// runs).
    pub exit_after: Option<u32>,
    /// Drive the window through these steps, then close (tests).
    pub script: Option<Script>,
    /// Write each settled frame of a scripted run, as presented, to
    /// `frame-N.png` in this directory (tests).
    pub capture: Option<std::path::PathBuf>,
}

/// One painted frame.
#[derive(Debug)]
pub struct Painted {
    /// Premultiplied RGBA at device pixels.
    pub surface: Surface,
    /// Whether this frame shows what the content was asked for (no load or
    /// repaint still pending). Only settled frames count toward
    /// [`WindowConfig::exit_after`] and let a [`Script`] continue.
    pub settled: bool,
}

/// Keyboard modifiers held during an input.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers {
    /// Control.
    pub ctrl: bool,
    /// Shift.
    pub shift: bool,
    /// Alt (Option on macOS).
    pub alt: bool,
    /// The logo key (Command on macOS, Windows key elsewhere).
    pub meta: bool,
}

impl Modifiers {
    /// The platform's shortcut modifier: Command on macOS, Control
    /// elsewhere.
    pub fn command(self) -> bool {
        if cfg!(target_os = "macos") {
            self.meta
        } else {
            self.ctrl
        }
    }

    /// No modifier at all, or only Shift.
    pub fn plain(self) -> bool {
        !self.ctrl && !self.alt && !self.meta
    }
}

/// A key the host reports, independent of the window library.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Key {
    /// Enter or Return.
    Enter,
    /// Escape.
    Escape,
    /// Backspace.
    Backspace,
    /// Delete (forward).
    Delete,
    /// Tab.
    Tab,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// The space bar.
    Space,
    /// A function key, F1 to F24.
    Function(u8),
    /// A key producing this character (lowercase for letters; text input
    /// arrives separately as [`Input::Text`]).
    Character(char),
}

/// An IME composition event (host-level plumbing; the page's composition
/// events are the accessibility and editing workstream's).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ime {
    /// The input method became active for the window.
    Enabled,
    /// The composition so far, with the cursor's byte range in it, if the
    /// input method shows one. An empty text ends the composition.
    Preedit(String, Option<(usize, usize)>),
    /// The input method committed this text.
    Commit(String),
    /// The input method became inactive.
    Disabled,
}

/// One user input, in the order the platform reported it.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// A key press (auto-repeat included).
    Key(Key, Modifiers),
    /// Text a key press produced, outside an IME composition.
    Text(String),
    /// IME composition.
    Ime(Ime),
    /// A primary click inside the trusted strip, in CSS pixels from its
    /// top-left corner.
    ChromeClick(f64, f64),
    /// A primary click on the page, in CSS pixels from the page area's
    /// top-left corner (below the strip).
    PageClick(f64, f64, Modifiers),
    /// A scroll request (wheel, touchpad, or a scrolling key the content
    /// did not use).
    Scroll(ScrollInput),
}

/// A window lifecycle change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lifecycle {
    /// The window's inner size in device pixels or its device scale
    /// changed (a resize, a move to another display, a scaling change).
    Resized {
        /// Inner size in device pixels.
        size: (u32, u32),
        /// Device pixels per CSS pixel (fractional on scaled displays).
        scale: f64,
    },
    /// The window became hidden (minimized, fully occluded or zero-sized:
    /// `false`) or visible again (`true`).
    Visible(bool),
    /// The window gained (`true`) or lost keyboard focus.
    Focused(bool),
}

/// What the content did with an input.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Response {
    /// The input was used; the shell applies no default action.
    pub handled: bool,
    /// The window needs a new frame.
    pub redraw: bool,
    /// Close the window.
    pub close: bool,
}

impl Response {
    /// Used, and the window needs a new frame.
    pub const REDRAW: Self = Self {
        handled: true,
        redraw: true,
        close: false,
    };
    /// Used, nothing to repaint.
    pub const HANDLED: Self = Self {
        handled: true,
        redraw: false,
        close: false,
    };
    /// Not used: the shell applies its default action.
    pub const UNHANDLED: Self = Self {
        handled: false,
        redraw: false,
        close: false,
    };
}

/// What fills the window.
pub trait Content {
    /// Paint the page for a page area of `size` device pixels at `scale`
    /// device pixels per CSS pixel.
    fn paint(&mut self, size: (u32, u32), scale: f64) -> Result<Painted, String>;

    /// The trusted strip's height in CSS pixels (0 for none).
    fn chrome_height(&self) -> f64 {
        0.0
    }

    /// The trusted strip above the page at `width` device pixels, drawn by
    /// the host, never by the page. `None` leaves the whole window to the
    /// page.
    fn chrome(&mut self, _width: u32, _scale: f64) -> Result<Option<Surface>, String> {
        Ok(None)
    }

    /// One user input. Unhandled keys fall back to scrolling (arrows, Page
    /// Up/Down, space, Home, End, which arrive again as [`Input::Scroll`])
    /// and closing (Escape).
    fn input(&mut self, _input: &Input) -> Response {
        Response::UNHANDLED
    }

    /// A lifecycle change; returns whether the window needs a new frame.
    fn lifecycle(&mut self, _event: Lifecycle) -> bool {
        false
    }

    /// Whether work is pending that [`Content::poll`] should check (a load
    /// or a repaint off this thread).
    fn busy(&self) -> bool {
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

    /// Check pending work; returns whether the window needs a new frame.
    fn poll(&mut self) -> bool {
        false
    }

    /// The window title, if the content has one.
    fn title(&self) -> Option<String> {
        None
    }

    /// Whether text entry is active (the address field or an editable
    /// page element), so the input method should be enabled, and where
    /// its caret is (left, top, width, height in CSS pixels of the
    /// window) for the candidate window.
    fn text_entry(&self) -> Option<(f64, f64, f64, f64)> {
        None
    }

    /// A one-line description of the content's state, logged after each
    /// settled frame of a scripted run (tests).
    fn describe(&self) -> Option<String> {
        None
    }
}

/// A window buffer of 0RGB pixels as an opaque surface (scripted
/// captures).
pub fn buffer_surface(buffer: &[u32], width: u32, height: u32) -> Option<Surface> {
    let mut pixels = Vec::with_capacity(buffer.len().saturating_mul(4));
    for &pixel in buffer
        .iter()
        .take((width as usize).saturating_mul(height as usize))
    {
        pixels.extend_from_slice(&[(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8, 255]);
    }
    Surface::from_premultiplied(
        width,
        height,
        pixels,
        rustic_raster::RasterLimits::default(),
    )
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
/// `page` is the page area's height in CSS pixels.
pub fn scrolled_by(input: &ScrollInput, scroll: (f64, f64), scale: f64, page: f64) -> (f64, f64) {
    let (x, y) = scroll;
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let next = match *input {
        ScrollInput::Lines(dx, dy) => (x - dx * LINE, y - dy * LINE),
        ScrollInput::Pixels(dx, dy) => (x - dx / scale, y - dy / scale),
        ScrollInput::Down => (x, y + LINE),
        ScrollInput::Up => (x, y - LINE),
        ScrollInput::PageDown => (x, y + page * 0.875),
        ScrollInput::PageUp => (x, y - page * 0.875),
        ScrollInput::Home => (x, 0.0),
        // The content clamps to the real end.
        ScrollInput::End => (x, f64::MAX / 4.0),
    };
    let finite = |v: f64| if v.is_finite() { v.max(0.0) } else { 0.0 };
    (finite(next.0), finite(next.1))
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
    /// End.
    End,
}

/// The default action for a key the content left unhandled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fallback {
    /// Scroll the page.
    Scroll(ScrollInput),
    /// Close the window.
    Close,
}

/// The shell's default action for an unhandled key, if any.
pub fn fallback(key: &Key, modifiers: Modifiers) -> Option<Fallback> {
    if !modifiers.plain() {
        return None;
    }
    Some(match key {
        Key::Escape => Fallback::Close,
        Key::Down => Fallback::Scroll(ScrollInput::Down),
        Key::Up => Fallback::Scroll(ScrollInput::Up),
        Key::PageDown => Fallback::Scroll(ScrollInput::PageDown),
        Key::Space if modifiers.shift => Fallback::Scroll(ScrollInput::PageUp),
        Key::Space => Fallback::Scroll(ScrollInput::PageDown),
        Key::PageUp => Fallback::Scroll(ScrollInput::PageUp),
        Key::Home => Fallback::Scroll(ScrollInput::Home),
        Key::End => Fallback::Scroll(ScrollInput::End),
        _ => return None,
    })
}

/// The device scale a window reports, made usable: finite and within
/// 0.25 to 8 (1 otherwise).
pub fn usable_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale.clamp(0.25, 8.0)
    } else {
        1.0
    }
}

/// The trusted strip's height in device pixels for `css` CSS pixels at
/// `scale`, within a window `height` device pixels tall. Rounded up so
/// fractional scales never leave a page row under the strip's last line.
pub fn strip_rows(css: f64, scale: f64, height: u32) -> u32 {
    let rows = (css.max(0.0) * usable_scale(scale)).ceil();
    if rows >= f64::from(height) {
        height
    } else {
        rows as u32
    }
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
        assert!(scrolled_by(&ScrollInput::End, at, 1.0, 600.0).1 > 1e300);
        // A broken scale or delta never yields NaN.
        assert_eq!(
            scrolled_by(&ScrollInput::Pixels(f64::NAN, -20.0), at, 0.0, 600.0),
            (0.0, 120.0)
        );
    }

    #[test]
    fn unhandled_keys_scroll_or_close_only_without_shortcut_modifiers() {
        let plain = Modifiers::default();
        let shift = Modifiers {
            shift: true,
            ..plain
        };
        let ctrl = Modifiers {
            ctrl: true,
            ..plain
        };
        assert_eq!(fallback(&Key::Escape, plain), Some(Fallback::Close));
        assert_eq!(
            fallback(&Key::Space, plain),
            Some(Fallback::Scroll(ScrollInput::PageDown))
        );
        assert_eq!(
            fallback(&Key::Space, shift),
            Some(Fallback::Scroll(ScrollInput::PageUp))
        );
        assert_eq!(fallback(&Key::Down, ctrl), None);
        assert_eq!(fallback(&Key::Character('a'), plain), None);
    }

    #[test]
    fn strip_rows_round_up_at_fractional_scales_and_fit_the_window() {
        assert_eq!(strip_rows(28.0, 1.0, 600), 28);
        assert_eq!(strip_rows(28.0, 1.25, 600), 35);
        // 28 × 1.5 = 42 exactly; 29 × 1.5 = 43.5 rounds up.
        assert_eq!(strip_rows(28.0, 1.5, 600), 42);
        assert_eq!(strip_rows(29.0, 1.5, 600), 44);
        assert_eq!(strip_rows(500.0, 2.0, 600), 600);
        assert_eq!(strip_rows(28.0, f64::NAN, 600), 28);
        assert_eq!(usable_scale(100.0), 8.0);
    }
}
