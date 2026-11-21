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

use std::num::NonZeroU32;
use std::rc::Rc;

use rustic_raster::Surface;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

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

struct Shell<C: Content> {
    config: WindowConfig,
    content: C,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    scroll: (f64, f64),
    presented: u32,
    error: Option<String>,
}

impl<C: Content> Shell<C> {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: String) {
        self.error.get_or_insert(error);
        event_loop.exit();
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return;
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        if let Err(error) = surface.resize(width, height) {
            let message = format!("cannot size the window buffer: {error}");
            return self.fail(event_loop, message);
        }
        let painted = match self.content.paint(
            (size.width, size.height),
            window.scale_factor(),
            self.scroll,
        ) {
            Ok(painted) => painted,
            Err(error) => return self.fail(event_loop, error),
        };
        self.scroll = painted.scroll;
        let mut buffer = match surface.buffer_mut() {
            Ok(buffer) => buffer,
            Err(error) => {
                let message = format!("cannot map the window buffer: {error}");
                return self.fail(event_loop, message);
            }
        };
        present_over_white(&painted.surface, size.width, size.height, &mut buffer);
        if let Err(error) = buffer.present() {
            let message = format!("cannot present the frame: {error}");
            return self.fail(event_loop, message);
        }
        self.presented += 1;
        if self
            .config
            .exit_after
            .is_some_and(|frames| self.presented >= frames)
        {
            event_loop.exit();
        }
    }

    fn scroll(&mut self, input: ScrollInput) {
        let Some(window) = &self.window else {
            return;
        };
        let page = f64::from(window.inner_size().height) / window.scale_factor();
        let next = scrolled_by(&input, self.scroll, window.scale_factor(), page);
        if next != self.scroll {
            self.scroll = next;
            window.request_redraw();
        }
    }
}

impl<C: Content> ApplicationHandler for Shell<C> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(LogicalSize::new(self.config.size.0, self.config.size.1));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Rc::new(window),
            Err(error) => return self.fail(event_loop, format!("cannot open a window: {error}")),
        };
        let surface = softbuffer::Context::new(Rc::clone(&window))
            .and_then(|context| softbuffer::Surface::new(&context, Rc::clone(&window)));
        match surface {
            Ok(surface) => self.surface = Some(surface),
            Err(error) => {
                return self.fail(event_loop, format!("cannot draw into the window: {error}"));
            }
        }
        window.request_redraw();
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::MouseWheel { delta, .. } => self.scroll(match delta {
                MouseScrollDelta::LineDelta(x, y) => ScrollInput::Lines(f64::from(x), f64::from(y)),
                MouseScrollDelta::PixelDelta(p) => ScrollInput::Pixels(p.x, p.y),
            }),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => match logical_key {
                Key::Named(NamedKey::Escape) => event_loop.exit(),
                Key::Named(NamedKey::ArrowDown) => self.scroll(ScrollInput::Down),
                Key::Named(NamedKey::ArrowUp) => self.scroll(ScrollInput::Up),
                Key::Named(NamedKey::PageDown | NamedKey::Space) => {
                    self.scroll(ScrollInput::PageDown);
                }
                Key::Named(NamedKey::PageUp) => self.scroll(ScrollInput::PageUp),
                Key::Named(NamedKey::Home) => self.scroll(ScrollInput::Home),
                _ => {}
            },
            _ => {}
        }
    }
}

/// Open the window and run its event loop on this thread until it closes.
pub fn run(config: WindowConfig, content: impl Content) -> Result<(), String> {
    let event_loop =
        EventLoop::new().map_err(|error| format!("cannot reach the window system: {error}"))?;
    let mut shell = Shell {
        config,
        content,
        window: None,
        surface: None,
        scroll: (0.0, 0.0),
        presented: 0,
        error: None,
    };
    event_loop
        .run_app(&mut shell)
        .map_err(|error| format!("the window system failed: {error}"))?;
    shell.error.map_or(Ok(()), Err)
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
