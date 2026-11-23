//! The winit/softbuffer window behind [`crate::run`] (desktop targets).

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use crate::accessibility;
use crate::{
    Content, Fallback, Ime, Input, Key, Lifecycle, Modifiers, Response, ScrollInput, Step,
    WindowConfig, fallback, present, strip_rows, usable_scale,
};

/// How often pending content work is checked.
const POLL: Duration = Duration::from_millis(15);
/// How long a scripted resize, minimize or restore waits for the window
/// system to report it.
const WINDOW_WAIT: Duration = Duration::from_secs(3);

/// What a script step waits for before the next frame.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Wait {
    /// A `Resized` event.
    Resize(Instant),
    /// The window becoming hidden.
    Hidden(Instant),
    /// The window becoming visible.
    Visible(Instant),
}

impl Wait {
    fn deadline(self) -> Instant {
        match self {
            Self::Resize(at) | Self::Hidden(at) | Self::Visible(at) => at,
        }
    }
}

struct Shell<C: Content> {
    config: WindowConfig,
    content: C,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    /// Settled frames presented.
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
    modifiers: Modifiers,
    /// Whether the window system reports the window fully occluded.
    occluded: bool,
    /// Whether the window is shown (not minimized, occluded or empty).
    visible: bool,
    /// The input method's state as last set, and whether a composition
    /// is in progress (text from key presses is then not sent).
    ime_allowed: bool,
    composing: bool,
    title: Option<String>,
    /// The next script step to run, and what the current one waits for.
    step: usize,
    wait: Option<Wait>,
    /// Whether the current step needs a settled frame before the next.
    awaiting_frame: bool,
}

/// The modifier state winit reports, as the shell's own.
fn modifiers_of(state: ModifiersState) -> Modifiers {
    Modifiers {
        ctrl: state.control_key(),
        shift: state.shift_key(),
        alt: state.alt_key(),
        meta: state.super_key(),
    }
}

/// The shell's key for a winit logical key.
fn key_of(key: &WinitKey) -> Option<Key> {
    Some(match key {
        WinitKey::Named(named) => match named {
            NamedKey::Enter => Key::Enter,
            NamedKey::Escape => Key::Escape,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::Tab => Key::Tab,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Space => Key::Space,
            NamedKey::F1 => Key::Function(1),
            NamedKey::F2 => Key::Function(2),
            NamedKey::F3 => Key::Function(3),
            NamedKey::F4 => Key::Function(4),
            NamedKey::F5 => Key::Function(5),
            NamedKey::F6 => Key::Function(6),
            NamedKey::F7 => Key::Function(7),
            NamedKey::F8 => Key::Function(8),
            NamedKey::F9 => Key::Function(9),
            NamedKey::F10 => Key::Function(10),
            NamedKey::F11 => Key::Function(11),
            NamedKey::F12 => Key::Function(12),
            _ => return None,
        },
        WinitKey::Character(text) => {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(' '), None) => Key::Space,
                (Some(c), None) => Key::Character(c.to_lowercase().next().unwrap_or(c)),
                _ => return None,
            }
        }
        _ => return None,
    })
}

/// Text a key press typed: printable characters only.
fn typed(text: &str) -> Option<String> {
    let text: String = text.chars().filter(|c| !c.is_control()).collect();
    (!text.is_empty()).then_some(text)
}

impl<C: Content> Shell<C> {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: String) {
        self.error.get_or_insert(error);
        event_loop.exit();
    }

    fn log(&self, line: &str) {
        if self.config.script.is_some() {
            println!("window: {line}");
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn scale(&self) -> f64 {
        usable_scale(self.window.as_ref().map_or(1.0, |w| w.scale_factor()))
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        if !self.visible {
            return;
        }
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
        let scale = usable_scale(window.scale_factor());
        let top = strip_rows(self.content.chrome_height(), scale, size.height);
        let painted = match self
            .content
            .paint((size.width, (size.height - top).max(1)), scale)
        {
            Ok(painted) => painted,
            Err(error) => return self.fail(event_loop, error),
        };
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
        let capture = match (&self.config.capture, painted.settled) {
            (Some(dir), true) => {
                crate::buffer_surface(&buffer, size.width, size.height).map(|surface| {
                    (
                        dir.join(format!("frame-{}.png", self.presented + 1)),
                        surface,
                    )
                })
            }
            _ => None,
        };
        if let Err(error) = buffer.present() {
            let message = format!("cannot present the frame: {error}");
            return self.fail(event_loop, message);
        }
        self.sync_window_state();
        if self.accessible {
            self.update_accessibility();
        }
        if !painted.settled {
            return;
        }
        self.presented += 1;
        self.log(&format!(
            "frame {}x{} scale {scale} strip {}",
            size.width, size.height, self.strip
        ));
        if self.config.script.is_some() {
            if let Some(state) = self.content.describe() {
                self.log(&format!("state {state}"));
            }
        }
        if let Some((path, surface)) = capture {
            let written = rustic_raster::encode_png(&surface, usize::MAX)
                .map_err(|error| format!("{error:?}"))
                .and_then(|png| std::fs::write(&path, png).map_err(|error| error.to_string()));
            if let Err(error) = written {
                let message = format!("cannot write {}: {error}", path.display());
                return self.fail(event_loop, message);
            }
        }
        if self.config.script.is_some() {
            self.awaiting_frame = false;
            return self.advance(event_loop);
        }
        match self.config.exit_after {
            Some(frames) if self.presented >= frames => event_loop.exit(),
            // A counted run presents its frames without waiting for the
            // window system to ask (a bare X server never does).
            Some(_) => self.request_redraw(),
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

    /// Bring the title and the input method in line with the content.
    fn sync_window_state(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let title = self.content.title();
        if title.is_some() && title != self.title {
            window.set_title(title.as_deref().unwrap_or_default());
            self.title = title;
        }
        let entry = self.content.text_entry();
        if entry.is_some() != self.ime_allowed {
            self.ime_allowed = entry.is_some();
            window.set_ime_allowed(self.ime_allowed);
        }
        if let Some((x, y, w, h)) = entry {
            let finite = |v: f64| if v.is_finite() { v.max(0.0) } else { 0.0 };
            window.set_ime_cursor_area(
                LogicalPosition::new(finite(x), finite(y)),
                LogicalSize::new(finite(w).max(1.0), finite(h).max(1.0)),
            );
        }
    }

    /// Run the next script step once the previous one is done.
    fn advance(&mut self, event_loop: &ActiveEventLoop) {
        let Some(script) = self.config.script.clone() else {
            return;
        };
        while self.wait.is_none() && !self.awaiting_frame {
            let Some(step) = script.steps().get(self.step).cloned() else {
                self.log("done");
                return event_loop.exit();
            };
            self.step += 1;
            self.log(&format!("step {step:?}"));
            let Some(window) = self.window.clone() else {
                return;
            };
            let now = Instant::now();
            match step {
                Step::Resize(w, h) => {
                    match window.request_inner_size(LogicalSize::new(w, h)) {
                        // Applied at once (Wayland): no event follows.
                        Some(size) => {
                            let scale = self.scale();
                            let _ = self.content.lifecycle(Lifecycle::Resized {
                                size: (size.width, size.height),
                                scale,
                            });
                            self.log(&format!(
                                "resized {}x{} scale {scale}",
                                size.width, size.height
                            ));
                            self.awaiting_frame = true;
                            window.request_redraw();
                        }
                        None => self.wait = Some(Wait::Resize(now + WINDOW_WAIT)),
                    }
                }
                Step::Minimize => {
                    window.set_minimized(true);
                    if self.visible {
                        self.wait = Some(Wait::Hidden(now + WINDOW_WAIT));
                    }
                }
                Step::Restore => {
                    window.set_minimized(false);
                    window.focus_window();
                    if self.visible {
                        self.awaiting_frame = true;
                        window.request_redraw();
                    } else {
                        self.wait = Some(Wait::Visible(now + WINDOW_WAIT));
                    }
                }
                Step::Frame => {
                    self.awaiting_frame = true;
                    window.request_redraw();
                }
                Step::Click(x, y) => {
                    let scale = self.scale();
                    self.cursor = (x * scale, y * scale);
                    self.click(event_loop);
                    self.awaiting_frame = true;
                    self.request_redraw();
                }
                Step::Input(input) => {
                    self.deliver(event_loop, &input);
                    self.awaiting_frame = true;
                    self.request_redraw();
                }
            }
            if !self.visible && self.awaiting_frame {
                // A hidden window presents nothing; the step is done.
                self.awaiting_frame = false;
            }
        }
    }

    /// The window system answered (or did not answer in time) a step's
    /// request.
    fn finish_wait(&mut self, event_loop: &ActiveEventLoop, outcome: &str) {
        if self.wait.take().is_some() {
            self.log(outcome);
            if self.visible {
                self.awaiting_frame = true;
                self.request_redraw();
            }
            self.advance(event_loop);
        }
    }

    fn set_visible(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        let visible = !self.occluded
            && size.width > 0
            && size.height > 0
            && window.is_minimized() != Some(true);
        if visible == self.visible {
            return;
        }
        self.visible = visible;
        self.log(if visible { "visible" } else { "hidden" });
        if self.content.lifecycle(Lifecycle::Visible(visible)) || visible {
            self.request_redraw();
        }
        match (self.wait, visible) {
            (Some(Wait::Hidden(_)), false) => self.finish_wait(event_loop, "minimized"),
            (Some(Wait::Visible(_)), true) => self.finish_wait(event_loop, "restored"),
            _ => {}
        }
    }

    /// Hand one input to the content and apply the shell's defaults to
    /// what it leaves.
    fn deliver(&mut self, event_loop: &ActiveEventLoop, input: &Input) {
        let response = self.content.input(input);
        self.respond(event_loop, response);
        if response.handled {
            return;
        }
        if let Input::Key(key, modifiers) = input {
            match fallback(key, *modifiers) {
                Some(Fallback::Close) => event_loop.exit(),
                Some(Fallback::Scroll(scroll)) => {
                    let response = self.content.input(&Input::Scroll(scroll));
                    self.respond(event_loop, response);
                }
                None => {}
            }
        }
    }

    fn respond(&mut self, event_loop: &ActiveEventLoop, response: Response) {
        if response.close {
            event_loop.exit();
        }
        if response.redraw {
            self.request_redraw();
        }
        self.sync_window_state();
    }

    fn click(&mut self, event_loop: &ActiveEventLoop) {
        // Only real clicks inside the strip reach it; the page never sees
        // them and cannot synthesize them.
        let (x, y) = self.cursor;
        let scale = self.scale();
        let input = if (0.0..f64::from(self.strip)).contains(&y) {
            Input::ChromeClick(x / scale, y / scale)
        } else {
            Input::PageClick(
                x / scale,
                (y - f64::from(self.strip)) / scale,
                self.modifiers,
            )
        };
        self.deliver(event_loop, &input);
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
        let size = window.inner_size();
        let scale = usable_scale(window.scale_factor());
        self.window = Some(Rc::clone(&window));
        self.visible = size.width > 0 && size.height > 0;
        let _ = self.content.lifecycle(Lifecycle::Resized {
            size: (size.width, size.height),
            scale,
        });
        self.log(&format!(
            "opened {}x{} scale {scale}",
            size.width, size.height
        ));
        if self.config.script.is_some() {
            // The first step waits for the first settled frame.
            self.awaiting_frame = true;
        }
        window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if let (Some(window), Some(adapter)) = (&self.window, &mut self.adapter) {
            adapter.process_event(window, &event);
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                let Some(window) = &self.window else {
                    return;
                };
                let size = window.inner_size();
                let scale = usable_scale(window.scale_factor());
                let _ = self.content.lifecycle(Lifecycle::Resized {
                    size: (size.width, size.height),
                    scale,
                });
                self.set_visible(event_loop);
                self.request_redraw();
                if matches!(self.wait, Some(Wait::Resize(_))) {
                    let outcome = format!("resized {}x{} scale {scale}", size.width, size.height);
                    self.finish_wait(event_loop, &outcome);
                }
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                self.set_visible(event_loop);
            }
            WindowEvent::Focused(focused) => {
                if self.content.lifecycle(Lifecycle::Focused(focused)) {
                    self.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::ModifiersChanged(state) => self.modifiers = modifiers_of(state.state()),
            WindowEvent::CursorMoved { position, .. } => self.cursor = (position.x, position.y),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.click(event_loop),
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        ScrollInput::Lines(f64::from(x), f64::from(y))
                    }
                    MouseScrollDelta::PixelDelta(p) => ScrollInput::Pixels(p.x, p.y),
                };
                self.deliver(event_loop, &Input::Scroll(scroll));
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        text,
                        ..
                    },
                ..
            } => {
                if let Some(key) = key_of(&logical_key) {
                    self.deliver(event_loop, &Input::Key(key, self.modifiers));
                }
                // Typed text goes to the content as text (the address
                // field, an editable element); shortcuts type nothing.
                if let Some(text) = text.as_deref().and_then(typed) {
                    if self.modifiers.plain() && !self.composing {
                        self.deliver(event_loop, &Input::Text(text));
                    }
                }
            }
            WindowEvent::Ime(ime) => {
                let ime = match ime {
                    winit::event::Ime::Enabled => Ime::Enabled,
                    winit::event::Ime::Preedit(text, cursor) => {
                        self.composing = !text.is_empty();
                        Ime::Preedit(text, cursor)
                    }
                    winit::event::Ime::Commit(text) => {
                        self.composing = false;
                        Ime::Commit(text)
                    }
                    winit::event::Ime::Disabled => {
                        self.composing = false;
                        Ime::Disabled
                    }
                };
                self.deliver(event_loop, &Input::Ime(ime));
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(wait) = self.wait {
            if Instant::now() >= wait.deadline() {
                let outcome = match wait {
                    Wait::Resize(_) => "resize not reported",
                    Wait::Hidden(_) => "minimize not reported",
                    Wait::Visible(_) => "restore not reported",
                };
                self.finish_wait(event_loop, outcome);
            }
        }
        if self.content.busy() && self.content.poll() {
            self.request_redraw();
        }
        let mut wake = self.wait.map(Wait::deadline);
        if self.content.busy() {
            let poll = Instant::now() + POLL;
            wake = Some(wake.map_or(poll, |at| at.min(poll)));
        }
        event_loop.set_control_flow(wake.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}

/// Open the window and run its event loop on this thread until it closes.
pub fn run(config: WindowConfig, content: impl Content) -> Result<(), String> {
    // winit panics when a display exists but a library it loads at run time
    // (libxkbcommon-x11) does not: that is a missing window system too,
    // reported like the others instead of as a crash.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let built =
        std::panic::catch_unwind(|| EventLoop::<accesskit_winit::Event>::with_user_event().build());
    std::panic::set_hook(hook);
    let event_loop = match built {
        Ok(Ok(event_loop)) => event_loop,
        Ok(Err(error)) => return Err(format!("cannot reach the window system: {error}")),
        Err(panic) => {
            let reason = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("the window library failed to start");
            return Err(format!("cannot reach the window system: {reason}"));
        }
    };
    let mut shell = Shell {
        adapter: None,
        proxy: event_loop.create_proxy(),
        accessible: false,
        tree: None,
        config,
        content,
        window: None,
        surface: None,
        presented: 0,
        error: None,
        cursor: (0.0, 0.0),
        strip: 0,
        modifiers: Modifiers::default(),
        occluded: false,
        visible: false,
        ime_allowed: false,
        composing: false,
        title: None,
        step: 0,
        wait: None,
        awaiting_frame: false,
    };
    event_loop
        .run_app(&mut shell)
        .map_err(|error| format!("the window system failed: {error}"))?;
    shell.error.map_or(Ok(()), Err)
}
