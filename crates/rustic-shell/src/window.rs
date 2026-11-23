//! The winit/softbuffer window behind [`crate::run`] (desktop targets).

use std::num::NonZeroU32;
use std::rc::Rc;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::accessibility;
use crate::{Content, ScrollInput, WindowConfig, present, scrolled_by};

struct Shell<C: Content> {
    config: WindowConfig,
    content: C,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    scroll: (f64, f64),
    presented: u32,
    error: Option<String>,
    /// The cursor, in device pixels.
    cursor: (f64, f64),
    /// The trusted strip's height in device pixels at the last frame.
    strip: u32,
    /// The platform accessibility bridge (ADR 0073).
    adapter: Option<accesskit_winit::Adapter>,
    proxy: EventLoopProxy<accesskit_winit::Event>,
    /// An assistive technology is listening: send the tree each frame.
    accessible: bool,
    /// The page tree last sent, for mapping actions back.
    tree: Option<rustic_a11y::AxTree>,
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
        // The page gets the rows below the trusted strip; the strip is
        // drawn after the page, so it shows the document just committed.
        let scale = window.scale_factor();
        let top = ((self.content.chrome_height() * scale).ceil().max(0.0) as u32).min(size.height);
        let painted = match self.content.paint(
            (size.width, (size.height - top).max(1)),
            window.scale_factor(),
            self.scroll,
        ) {
            Ok(painted) => painted,
            Err(error) => return self.fail(event_loop, error),
        };
        self.scroll = painted.scroll;
        let chrome = match self.content.chrome(size.width, scale) {
            Ok(chrome) => chrome,
            Err(error) => return self.fail(event_loop, error),
        };
        let mut buffer = match surface.buffer_mut() {
            Ok(buffer) => buffer,
            Err(error) => {
                let message = format!("cannot map the window buffer: {error}");
                return self.fail(event_loop, message);
            }
        };
        self.strip = present(
            &painted.surface,
            chrome.as_ref(),
            size.width,
            size.height,
            &mut buffer,
        );
        if let Err(error) = buffer.present() {
            let message = format!("cannot present the frame: {error}");
            return self.fail(event_loop, message);
        }
        self.presented += 1;
        let window = Rc::clone(window);
        if self.accessible {
            self.update_accessibility();
        }
        match self.config.exit_after {
            Some(frames) if self.presented >= frames => event_loop.exit(),
            // A counted run presents its frames without waiting for the
            // window system to ask (a bare X server never does).
            Some(_) => window.request_redraw(),
            None => {}
        }
    }

    /// Send the page's current tree to the platform bridge.
    fn update_accessibility(&mut self) {
        let (Some(window), Some(adapter)) = (&self.window, &mut self.adapter) else {
            return;
        };
        self.tree = self.content.accessibility();
        let size = window.inner_size();
        let scale = window.scale_factor();
        let top = f64::from(self.strip) / scale;
        let title = &self.config.title;
        let tree = self.tree.as_ref();
        adapter.update_if_active(|| {
            accessibility::tree_update(tree, title, (size.width, size.height), scale, top)
        });
    }

    fn scroll(&mut self, input: ScrollInput) {
        let Some(window) = &self.window else {
            return;
        };
        let page = f64::from(window.inner_size().height.saturating_sub(self.strip))
            / window.scale_factor();
        let next = scrolled_by(&input, self.scroll, window.scale_factor(), page);
        if next != self.scroll {
            self.scroll = next;
            window.request_redraw();
        }
    }
}

impl<C: Content> ApplicationHandler<accesskit_winit::Event> for Shell<C> {
    fn user_event(&mut self, _: &ActiveEventLoop, event: accesskit_winit::Event) {
        match event.window_event {
            accesskit_winit::WindowEvent::InitialTreeRequested => {
                self.accessible = true;
                self.update_accessibility();
            }
            accesskit_winit::WindowEvent::ActionRequested(request) => {
                let Some(tree) = &self.tree else {
                    return;
                };
                if let Some((node, action)) = accessibility::page_action(tree, &request) {
                    if self.content.accessibility_action(node, action) {
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                }
            }
            accesskit_winit::WindowEvent::AccessibilityDeactivated => self.accessible = false,
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // The accessibility adapter must exist before the window is first
        // shown, so it opens hidden.
        let attributes = Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(LogicalSize::new(self.config.size.0, self.config.size.1))
            .with_visible(false);
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Rc::new(window),
            Err(error) => return self.fail(event_loop, format!("cannot open a window: {error}")),
        };
        self.adapter = Some(accesskit_winit::Adapter::with_event_loop_proxy(
            event_loop,
            &window,
            self.proxy.clone(),
        ));
        window.set_visible(true);
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
        if let (Some(window), Some(adapter)) = (&self.window, &mut self.adapter) {
            adapter.process_event(window, &event);
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::CursorMoved { position, .. } => self.cursor = (position.x, position.y),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // Only real clicks inside the strip reach it; the page never
                // sees them and cannot synthesize them.
                let (x, y) = self.cursor;
                if let Some(window) = &self.window {
                    let scale = window.scale_factor();
                    if (0.0..f64::from(self.strip)).contains(&y)
                        && self.content.chrome_click(x / scale, y / scale)
                    {
                        window.request_redraw();
                    }
                }
            }
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
                Key::Named(NamedKey::Escape) => {
                    if self.content.chrome_escape() {
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    } else {
                        event_loop.exit();
                    }
                }
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
    let event_loop = EventLoop::<accesskit_winit::Event>::with_user_event()
        .build()
        .map_err(|error| format!("cannot reach the window system: {error}"))?;
    let mut shell = Shell {
        adapter: None,
        proxy: event_loop.create_proxy(),
        accessible: false,
        tree: None,
        config,
        content,
        window: None,
        surface: None,
        scroll: (0.0, 0.0),
        presented: 0,
        error: None,
        cursor: (0.0, 0.0),
        strip: 0,
    };
    event_loop
        .run_app(&mut shell)
        .map_err(|error| format!("the window system failed: {error}"))?;
    shell.error.map_or(Ok(()), Err)
}
