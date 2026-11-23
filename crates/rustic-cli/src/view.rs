//! `rustic view`: the browser window (UI-01 on UI-02, ADR 0065, ADR 0074).
//!
//! [`crate::browser::Browser`] holds the browser's state; this module is
//! the window's [`rustic_shell::Content`]: it draws the strip, drives the
//! active tab's page and turns what the page reports into browser state.
//! The page is a long-lived renderer ([`broker::live`], TD-182, ADR 0069)
//! held by one worker thread, so the window keeps answering (Stop, typing,
//! switching tabs) while a page loads: the page loads once, then frames,
//! clicks and keys go to the same renderer, which keeps its scripts and
//! timers between frames. Only the active tab has a page; switching tabs
//! loads the other tab's page again (TD-196). Results of loads the user
//! stopped or navigated away from are dropped by generation.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::Instant;

use rustic_base::EscapedText;
use rustic_ipc::protocol::{
    AssistiveRequest, FrameAsk, MODIFIER_ALT, MODIFIER_CONTROL, MODIFIER_META, MODIFIER_SHIFT,
};
use rustic_shell::{Input, Key, Lifecycle, Modifiers, Painted, Response};

use crate::browser::{Browser, Focus, Load, Target};
use crate::{broker, chrome};

/// The environment variable a test sets to drive the window through
/// `rustic_shell::Script` steps.
pub const SCRIPT_VARIABLE: &str = "RUSTIC_TEST_WINDOW_SCRIPT";
/// The directory a scripted test captures presented frames into.
pub const CAPTURE_VARIABLE: &str = "RUSTIC_TEST_WINDOW_CAPTURE";

/// What one page frame shows: everything that changes its pixels.
#[derive(Clone, Debug, PartialEq)]
struct FrameKey {
    tab: u64,
    generation: u64,
    /// Page area in CSS pixels.
    css: (u32, u32),
    scale: f64,
    zoom: f64,
    scroll: (f64, f64),
    find: Option<(String, usize)>,
    /// Bumped when the page handled input or asked for a frame (timers,
    /// animation): the same view, new pixels.
    serial: u64,
}

/// Work for the page thread.
enum Job {
    /// Load a tab's page (closing the previous one).
    Open {
        tab: u64,
        generation: u64,
        args: Vec<OsString>,
    },
    /// Close the page (an empty tab).
    Close,
    /// Paint a frame, then send the page's accessibility tree when an
    /// assistive technology is listening (`tree`).
    Frame {
        key: FrameKey,
        ask: FrameAsk,
        tree: bool,
    },
    /// A primary click on the page.
    Click { x: f64, y: f64, modifiers: u8 },
    /// A key press and release on the page.
    Key {
        key: String,
        code: String,
        modifiers: u8,
    },
    /// An assistive technology's request for one page node (ADR 0073).
    Assist {
        node: u64,
        request: AssistiveRequest,
    },
}

/// What the page thread reports.
enum Event {
    /// The page committed and is live.
    Opened {
        tab: u64,
        generation: u64,
        committed: Option<rustic_url::Url>,
    },
    /// The page could not be loaded.
    Failed {
        tab: u64,
        generation: u64,
        status: u8,
        failure: Option<broker::Failure>,
    },
    /// A frame, or why there is none.
    Frame {
        key: FrameKey,
        frame: Result<broker::LiveFrame, String>,
        notes: Vec<String>,
        /// The page's accessibility tree after the frame, when asked.
        tree: Option<Option<rustic_a11y::AxTree>>,
    },
    /// The page ran its handlers for an input.
    Handled,
}

/// The page thread's inbox, with jobs looked at but not yet taken.
struct Inbox {
    jobs: Receiver<Job>,
    held: VecDeque<Job>,
}

impl Inbox {
    /// The next job, `None` when the window is gone. A frame request
    /// followed by another one is skipped: only the newest view is painted.
    fn next(&mut self) -> Option<Job> {
        loop {
            while let Ok(job) = self.jobs.try_recv() {
                self.held.push_back(job);
            }
            let job = match self.held.pop_front() {
                Some(job) => job,
                None => self.jobs.recv().ok()?,
            };
            let superseded = matches!(job, Job::Frame { .. })
                && self
                    .held
                    .iter()
                    .any(|later| matches!(later, Job::Frame { .. }));
            if !superseded {
                return Some(job);
            }
        }
    }
}

/// How a live page's run ended.
enum Next {
    Open(Job),
    Close,
    Quit,
}

/// Run the page thread until the window is gone.
fn page_thread(mut inbox: Inbox, events: &Sender<Event>) {
    let mut next = None;
    loop {
        let job = match next.take() {
            Some(job) => job,
            None => match inbox.next() {
                Some(job) => job,
                None => return,
            },
        };
        let Job::Open {
            tab,
            generation,
            args,
        } = job
        else {
            // Input for a page that is not open: nothing to do.
            continue;
        };
        let ran = broker::live(&args, |page| {
            let _ = events.send(Event::Opened {
                tab,
                generation,
                committed: broker::committed_url(),
            });
            loop {
                let Some(job) = inbox.next() else {
                    return Next::Quit;
                };
                let event = match job {
                    open @ Job::Open { .. } => return Next::Open(open),
                    Job::Close => return Next::Close,
                    Job::Frame { key, ask, tree } => {
                        let frame = page.frame(ask);
                        let tree =
                            (tree && frame.is_ok()).then(|| page.accessibility().ok().flatten());
                        Event::Frame {
                            key,
                            frame,
                            notes: page.take_notes(),
                            tree,
                        }
                    }
                    Job::Assist { node, request } => {
                        if let Err(error) = page.assistive(node, request) {
                            eprintln!("rustic view: {}", EscapedText(&error));
                        }
                        Event::Handled
                    }
                    Job::Click { x, y, modifiers } => {
                        if let Err(error) = page.click(x, y, modifiers) {
                            eprintln!("rustic view: {}", EscapedText(&error));
                        }
                        Event::Handled
                    }
                    Job::Key {
                        key,
                        code,
                        modifiers,
                    } => {
                        let pressed = page
                            .key(true, &key, &code, false, modifiers)
                            .and_then(|()| page.key(false, &key, &code, false, modifiers));
                        if let Err(error) = pressed {
                            eprintln!("rustic view: {}", EscapedText(&error));
                        }
                        Event::Handled
                    }
                };
                if events.send(event).is_err() {
                    return Next::Quit;
                }
            }
        });
        match ran {
            Ok(Next::Open(open)) => next = Some(open),
            Ok(Next::Close) => {}
            Ok(Next::Quit) => return,
            Err(status) => {
                let failed = Event::Failed {
                    tab,
                    generation,
                    status,
                    failure: broker::last_failure(),
                };
                if events.send(failed).is_err() {
                    return;
                }
            }
        }
    }
}

/// The page thread's handles.
struct Worker {
    jobs: Sender<Job>,
    events: Receiver<Event>,
}

impl Worker {
    fn start() -> Result<Self, String> {
        let (jobs, inbox) = channel();
        let (outbox, events) = channel();
        std::thread::Builder::new()
            .name("rustic-view-page".into())
            .spawn(move || {
                page_thread(
                    Inbox {
                        jobs: inbox,
                        held: VecDeque::new(),
                    },
                    &outbox,
                );
            })
            .map_err(|error| format!("cannot start the page thread: {error}"))?;
        Ok(Self { jobs, events })
    }
}

/// Modifier bits for the page.
fn modifier_bits(modifiers: Modifiers) -> u8 {
    let mut bits = 0;
    for (on, bit) in [
        (modifiers.ctrl, MODIFIER_CONTROL),
        (modifiers.alt, MODIFIER_ALT),
        (modifiers.shift, MODIFIER_SHIFT),
        (modifiers.meta, MODIFIER_META),
    ] {
        if on {
            bits |= bit;
        }
    }
    bits
}

/// The DOM `key` and `code` for a key the browser left to the page.
fn dom_key(key: &Key, modifiers: Modifiers) -> Option<(String, String)> {
    let named = |name: &str| Some((name.to_owned(), name.to_owned()));
    match key {
        Key::Enter => named("Enter"),
        Key::Escape => named("Escape"),
        Key::Backspace => named("Backspace"),
        Key::Delete => named("Delete"),
        Key::Tab => named("Tab"),
        Key::Left => named("ArrowLeft"),
        Key::Right => named("ArrowRight"),
        Key::Up => named("ArrowUp"),
        Key::Down => named("ArrowDown"),
        Key::Home => named("Home"),
        Key::End => named("End"),
        Key::PageUp => named("PageUp"),
        Key::PageDown => named("PageDown"),
        Key::Space => Some((" ".to_owned(), "Space".to_owned())),
        Key::Function(n) => named(&format!("F{n}")),
        Key::Character(c) => {
            let shown = if modifiers.shift {
                c.to_uppercase().collect()
            } else {
                c.to_string()
            };
            let code = if c.is_ascii_alphabetic() {
                format!("Key{}", c.to_ascii_uppercase())
            } else if c.is_ascii_digit() {
                format!("Digit{c}")
            } else {
                String::new()
            };
            Some((shown, code))
        }
    }
}

/// A frame shown in a tab.
struct Shown {
    key: FrameKey,
    surface: rustic_raster::Surface,
}

/// The window's content.
pub struct ViewPage {
    browser: Browser,
    /// Document options for every page (the path removed).
    options: Vec<OsString>,
    worker: Worker,
    /// The page the worker holds: tab and generation.
    open: Option<(u64, u64)>,
    /// Whether that page committed and takes frames.
    live: bool,
    /// The frame request on the worker, if any.
    pending: Option<FrameKey>,
    /// Inputs the page is still handling.
    handling: u32,
    /// The viewport position the page last reported (user scrolls are sent
    /// only when the entry differs from it).
    reported: (f64, f64),
    /// When the page wants its next frame.
    next_frame: Option<Instant>,
    /// When the last frame was asked for (the page clock's step).
    last_ask: Option<Instant>,
    serial: u64,
    shown: HashMap<u64, Shown>,
    /// The last strip drawn: its document, width and scale.
    strip: Option<(String, u32, u64, rustic_raster::Surface)>,
    /// The page area's height in CSS pixels at the last frame.
    page_height: f64,
    scale: f64,
    /// The strip's width in CSS pixels at the last frame (for clicks).
    strip_width: f64,
    /// Whether the window is shown (hidden windows ask for no frames).
    visible: bool,
    /// The active page's accessibility tree from its last frame, in CSS
    /// pixels of the page area (ADR 0073).
    tree: Option<rustic_a11y::AxTree>,
    /// An assistive technology asked for the tree: frames fetch it.
    accessible: bool,
    /// It asked before any tree arrived: paint a frame to fetch one.
    tree_wanted: bool,
}

impl ViewPage {
    fn key(&self, css: (u32, u32), scale: f64) -> FrameKey {
        let tab = self.browser.tab();
        FrameKey {
            tab: tab.id,
            generation: tab.generation,
            css,
            scale,
            zoom: tab.zoom,
            scroll: tab.entry().scroll,
            find: tab
                .find
                .as_ref()
                .filter(|find| !find.field.text().is_empty())
                .map(|find| (find.field.text().to_owned(), find.index)),
            serial: self.serial,
        }
    }

    /// Make sure the worker holds the active tab's current page.
    fn ensure_open(&mut self) {
        let tab = self.browser.tab();
        let wanted = (tab.id, tab.generation);
        if self.open == Some(wanted) || tab.load == Load::Stopped {
            return;
        }
        let target = match &tab.entry().target {
            Target::Blank => {
                self.tree = None;
                if self.open.take().is_some() {
                    let _ = self.worker.jobs.send(Job::Close);
                }
                return;
            }
            Target::File(path) => path.clone().into_os_string(),
            Target::Url(url) => OsString::from(url),
        };
        let mut args = self.options.clone();
        args.extend(["--output".into(), "view.png".into(), "--".into(), target]);
        let job = Job::Open {
            tab: wanted.0,
            generation: wanted.1,
            args,
        };
        if self.worker.jobs.send(job).is_ok() {
            self.open = Some(wanted);
            self.live = false;
            self.tree = None;
            self.pending = None;
            self.handling = 0;
            self.reported = (0.0, 0.0);
            self.next_frame = None;
            self.last_ask = None;
            // A page loaded again (switching back to a tab, back and
            // forward) starts where its entry was scrolled.
            self.browser.tab_mut().load = Load::Loading;
        }
    }

    fn request(&mut self, key: FrameKey) {
        let now = Instant::now();
        let elapsed = self
            .last_ask
            .map_or(0, |at| now.saturating_duration_since(at).as_micros());
        let ask = FrameAsk {
            width: key.css.0,
            height: key.css.1,
            scale: key.scale,
            zoom: key.zoom,
            scroll: (key.scroll != self.reported).then_some(key.scroll),
            elapsed_us: u64::try_from(elapsed).unwrap_or(u64::MAX),
            find: key
                .find
                .as_ref()
                .map(|(text, index)| (text.clone(), u32::try_from(*index).unwrap_or(u32::MAX))),
        };
        if self
            .worker
            .jobs
            .send(Job::Frame {
                key: key.clone(),
                ask,
                tree: self.accessible,
            })
            .is_ok()
        {
            self.pending = Some(key);
            self.last_ask = Some(now);
        }
    }

    fn message(
        &self,
        heading: &str,
        detail: &str,
        size: (u32, u32),
        scale: f64,
    ) -> Result<rustic_raster::Surface, String> {
        let css_height = ((f64::from(size.1) / scale).ceil() as u32).max(1);
        chrome::paint_trusted(
            chrome::message_document(heading, detail),
            size.0,
            css_height,
            scale,
        )
    }

    /// Take one report of the page thread into the browser's state.
    fn take(&mut self, event: Event) {
        match event {
            Event::Opened {
                tab,
                generation,
                committed,
            } => {
                if self.open != Some((tab, generation)) {
                    return;
                }
                self.live = true;
                // A page loaded again needs its own first frame.
                self.serial += 1;
                if let Some(tab) = self.browser.tab_by_id(tab) {
                    if tab.generation == generation {
                        tab.committed = committed;
                    }
                }
            }
            Event::Failed {
                tab,
                generation,
                status,
                failure,
            } => {
                if self.open == Some((tab, generation)) {
                    self.open = None;
                    self.live = false;
                    self.tree = None;
                }
                let current = self
                    .browser
                    .tab_by_id(tab)
                    .is_some_and(|t| t.generation == generation);
                if !current {
                    return;
                }
                match failure {
                    Some(broker::Failure::Download { name, saved }) => {
                        let notice = match saved {
                            Ok(path) => format!("Downloaded {name} to {path}"),
                            Err(why) if name.is_empty() => format!("Download failed: {why}"),
                            Err(why) => format!("Download of {name} refused: {why}"),
                        };
                        self.browser.download_finished(tab, notice);
                    }
                    Some(broker::Failure::Load(why)) => {
                        if let Some(tab) = self.browser.tab_by_id(tab) {
                            tab.load = Load::Failed(why);
                        }
                    }
                    None => {
                        if let Some(tab) = self.browser.tab_by_id(tab) {
                            tab.load =
                                Load::Failed(format!("the page did not load (status {status})"));
                        }
                    }
                }
            }
            Event::Handled => {
                self.handling = self.handling.saturating_sub(1);
                self.serial += 1;
            }
            Event::Frame {
                key,
                frame,
                notes,
                tree,
            } => {
                if self.pending.as_ref() == Some(&key) {
                    self.pending = None;
                }
                if let Some(tree) = tree {
                    if self.open == Some((key.tab, key.generation)) {
                        self.tree = tree;
                    }
                }
                for note in notes {
                    eprintln!("rustic view: {}", EscapedText(&note));
                }
                let page_height = self.page_height;
                let Some(tab) = self.browser.tab_by_id(key.tab) else {
                    return;
                };
                if key.generation != tab.generation {
                    return;
                }
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(why) => {
                        tab.load = Load::Failed(why);
                        return;
                    }
                };
                let mut key = key;
                // The page clamped (or a script moved) the position; keep it
                // unless the user scrolled again meanwhile.
                if tab.entry().scroll == key.scroll {
                    tab.entry_mut().scroll = frame.scroll;
                }
                key.scroll = frame.scroll;
                self.reported = frame.scroll;
                if let Some(find) = &mut tab.find {
                    let asked = key
                        .find
                        .as_ref()
                        .map(|(text, index)| (text.as_str(), *index));
                    if asked == Some((find.field.text(), find.index)) {
                        find.result = frame.found.map(|(count, active, top, bottom)| {
                            (count as usize, active as usize, top, bottom)
                        });
                        // Bring the active match into view.
                        if let Some((count, _, top, bottom)) = find.result {
                            let scroll = tab.entry().scroll;
                            if count > 0 && (top < scroll.1 || bottom > scroll.1 + page_height) {
                                tab.entry_mut().scroll.1 =
                                    (top - page_height / 3.0).max(0.0).floor();
                            }
                        }
                    }
                }
                if tab.load == Load::Loading {
                    tab.load = Load::Idle;
                }
                self.next_frame = frame.next_frame.map(|delay| Instant::now() + delay);
                self.shown.insert(
                    key.tab,
                    Shown {
                        key,
                        surface: frame.surface,
                    },
                );
            }
        }
    }

    /// Hand a click or key the browser left to the page.
    fn send_to_page(&mut self, job: Job) {
        if self.live && self.worker.jobs.send(job).is_ok() {
            self.handling += 1;
        }
    }
}

impl rustic_shell::Content for ViewPage {
    fn paint(&mut self, size: (u32, u32), scale: f64) -> Result<Painted, String> {
        let scale = rustic_shell::usable_scale(scale);
        self.scale = scale;
        let css = |device: u32| ((f64::from(device) / scale).round() as u32).clamp(1, 16_384);
        let css_size = (css(size.0), css(size.1));
        self.page_height = f64::from(css_size.1);
        self.ensure_open();
        let tab = self.browser.tab();
        match (&tab.load, &tab.entry().target) {
            (Load::Failed(why), target) => {
                let detail = format!("{}\n\n{why}", target.label());
                return Ok(Painted {
                    surface: self.message("This page could not be shown", &detail, size, scale)?,
                    settled: true,
                });
            }
            (_, Target::Blank) => {
                return Ok(Painted {
                    surface: self.message(
                        "New tab",
                        "Type an address (https://...) or a file path above and press Enter.",
                        size,
                        scale,
                    )?,
                    settled: true,
                });
            }
            _ => {}
        }
        let key = self.key(css_size, scale);
        let current = self
            .shown
            .get(&key.tab)
            .is_some_and(|shown| shown.key == key);
        let stopped = tab.load == Load::Stopped;
        if self.live && !current && !stopped && self.pending.is_none() {
            self.request(key.clone());
        }
        let settled = stopped
            || (current
                && self.pending.is_none()
                && self.handling == 0
                && self.browser.tab().load == Load::Idle);
        let surface = match self.shown.get(&key.tab) {
            Some(shown) => shown.surface.clone(),
            None if stopped => {
                let label = self.browser.tab().entry().target.label();
                self.message("Stopped", &label, size, scale)?
            }
            None => {
                rustic_raster::Surface::new(size.0, size.1, rustic_raster::RasterLimits::default())
                    .map_err(|_| "cannot size the page".to_owned())?
            }
        };
        Ok(Painted { surface, settled })
    }

    fn chrome_height(&self) -> f64 {
        f64::from(self.browser.with_strip(chrome::strip_height))
    }

    fn chrome(&mut self, width: u32, scale: f64) -> Result<Option<rustic_raster::Surface>, String> {
        let scale = rustic_shell::usable_scale(scale);
        self.strip_width = (f64::from(width) / scale).floor();
        let css_width = (self.strip_width as u32).max(1);
        let html = self
            .browser
            .with_strip(|strip| chrome::document(strip, css_width));
        if let Some((cached, w, s, surface)) = &self.strip {
            if *cached == html && *w == width && *s == scale.to_bits() {
                return Ok(Some(surface.clone()));
            }
        }
        let height = self.browser.with_strip(chrome::strip_height);
        let surface = chrome::paint_trusted(html.clone(), width, height, scale)?;
        self.strip = Some((html, width, scale.to_bits(), surface.clone()));
        Ok(Some(surface))
    }

    fn input(&mut self, input: &Input) -> Response {
        if let Input::Scroll(scroll) = input {
            if self.browser.focus() == Focus::Page
                && self.browser.scroll(scroll, self.scale, self.page_height)
            {
                return Response::REDRAW;
            }
            return Response::HANDLED;
        }
        let (response, _) = self.browser.input(input, self.strip_width);
        if response.handled || self.browser.focus() != Focus::Page {
            return response;
        }
        // What the browser left goes to the page (TD-182 input routing).
        match input {
            Input::PageClick(x, y, modifiers) => {
                self.send_to_page(Job::Click {
                    x: *x,
                    y: *y,
                    modifiers: modifier_bits(*modifiers),
                });
            }
            Input::Key(key, modifiers) => {
                if let Some((key, code)) = dom_key(key, *modifiers) {
                    self.send_to_page(Job::Key {
                        key,
                        code,
                        modifiers: modifier_bits(*modifiers),
                    });
                }
            }
            _ => {}
        }
        response
    }

    fn lifecycle(&mut self, event: Lifecycle) -> bool {
        match event {
            Lifecycle::Resized { scale, .. } => {
                self.scale = rustic_shell::usable_scale(scale);
                true
            }
            // A hidden window asks for no frames: timers keep their
            // deadlines and run on the next frame after it is shown.
            Lifecycle::Visible(visible) => {
                self.visible = visible;
                visible
            }
            Lifecycle::Focused(_) => false,
        }
    }

    fn busy(&self) -> bool {
        self.open.is_some() && (!self.live || self.pending.is_some() || self.handling > 0)
            || (self.visible && self.next_frame.is_some())
            || (self.tree_wanted && self.live)
    }

    fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.worker.events.try_recv() {
                Ok(event) => {
                    self.take(event);
                    changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.open = None;
                    break;
                }
            }
        }
        // The first tree comes with a new frame of the page.
        if self.tree_wanted && self.live {
            self.tree_wanted = false;
            self.serial += 1;
            changed = true;
        }
        if self.visible && self.next_frame.is_some_and(|at| Instant::now() >= at) {
            self.next_frame = None;
            self.serial += 1;
            changed = true;
        }
        changed
    }

    fn title(&self) -> Option<String> {
        let title = self.browser.with_strip(|strip| {
            strip
                .tabs
                .get(strip.active)
                .map(|tab| tab.title.clone())
                .unwrap_or_default()
        });
        Some(format!("{title} — Rustic"))
    }

    fn text_entry(&self) -> Option<(f64, f64, f64, f64)> {
        self.browser.text_entry(self.strip_width)
    }

    fn describe(&self) -> Option<String> {
        Some(self.browser.describe())
    }

    fn accessibility(&mut self) -> Option<rustic_a11y::AxTree> {
        if !self.accessible {
            self.accessible = true;
            self.tree_wanted = self.tree.is_none();
        }
        let tab = self.browser.tab();
        let failed = matches!(tab.load, Load::Failed(_));
        if failed || matches!(tab.entry().target, Target::Blank) {
            return None;
        }
        self.tree.clone()
    }

    fn accessibility_action(
        &mut self,
        node: rustic_a11y::AxId,
        action: rustic_a11y::ActionRequest,
    ) -> bool {
        let request = match action {
            rustic_a11y::ActionRequest::Focus => AssistiveRequest::Focus,
            rustic_a11y::ActionRequest::Click => AssistiveRequest::Click,
            rustic_a11y::ActionRequest::SetValue(text) => AssistiveRequest::SetValue(text),
            rustic_a11y::ActionRequest::SetSelection(anchor, focus) => {
                AssistiveRequest::SetSelection { anchor, focus }
            }
            // The page scrolls itself into view on focus; a bare request
            // has no page input yet.
            rustic_a11y::ActionRequest::ScrollIntoView => return false,
        };
        let live = self.live;
        self.send_to_page(Job::Assist {
            node: node.0,
            request,
        });
        live
    }
}

/// `view`: the browser window (UI-01, UI-02).
pub fn view_command(args: &[OsString]) -> u8 {
    let usage = |message: &str| {
        eprintln!("rustic view: {message}\n\n{}", crate::USAGE);
        2
    };
    if broker::single_process() {
        return usage(
            "view shows pages from renderer processes; it cannot run with --single-process",
        );
    }
    let mut size = (1024, 768);
    let mut exit_after = None;
    let mut rest = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--") => {
                rest.push(arg.clone());
                rest.extend(iter.by_ref().cloned());
            }
            Some("--window") => {
                let parsed = iter
                    .next()
                    .and_then(|v| v.to_str())
                    .and_then(|v| v.split_once('x'))
                    .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
                    .filter(|&(w, h)| (64..=8192).contains(&w) && (64..=8192).contains(&h));
                match parsed {
                    Some(parsed) => size = parsed,
                    None => return usage("--window requires WxH with each side from 64 to 8192"),
                }
            }
            Some("--exit-after") => {
                match iter
                    .next()
                    .and_then(|v| v.to_str())
                    .and_then(|v| v.parse::<u32>().ok())
                {
                    Some(frames) if frames > 0 => exit_after = Some(frames),
                    _ => return usage("--exit-after requires a positive frame count"),
                }
            }
            Some(
                "--viewport" | "--scale" | "--zoom" | "--output" | "--frames" | "--find"
                | "--find-index" | "--scroll" | "--report-scroll",
            ) => {
                return usage("the window sets the view; use --window WxH");
            }
            _ => rest.push(arg.clone()),
        }
    }
    let Some(path) = rest.pop() else {
        return usage("missing file path or address");
    };
    if rest.last().is_some_and(|arg| arg == "--") {
        rest.pop();
    }
    let script = match std::env::var(SCRIPT_VARIABLE) {
        Ok(text) => match rustic_shell::Script::parse(&text) {
            Ok(script) => Some(script),
            Err(error) => {
                eprintln!("rustic view: {error}");
                return 2;
            }
        },
        Err(_) => None,
    };
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let first = if crate::network::is_network_argument(&path) {
        Target::Url(path.to_string_lossy().into_owned())
    } else if path == "about:blank" {
        Target::Blank
    } else {
        Target::File(PathBuf::from(&path))
    };
    let title = format!("{} — Rustic", Path::new(&path).to_string_lossy());
    let worker = match Worker::start() {
        Ok(worker) => worker,
        Err(error) => {
            eprintln!("rustic view: {error}");
            return 1;
        }
    };
    let page = ViewPage {
        browser: Browser::new(first, crate::storage::label(), base),
        options: rest,
        worker,
        open: None,
        live: false,
        pending: None,
        handling: 0,
        reported: (0.0, 0.0),
        next_frame: None,
        last_ask: None,
        serial: 0,
        shown: HashMap::new(),
        strip: None,
        page_height: f64::from(size.1),
        scale: 1.0,
        strip_width: f64::from(size.0),
        visible: true,
        tree: None,
        accessible: false,
        tree_wanted: false,
    };
    let config = rustic_shell::WindowConfig {
        title,
        size,
        exit_after,
        script,
        capture: std::env::var_os(CAPTURE_VARIABLE).map(PathBuf::from),
    };
    match rustic_shell::run(config, page) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("rustic view: {}", EscapedText(&error));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_reach_the_page_as_dom_keys() {
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            dom_key(&Key::Character('a'), shift),
            Some(("A".into(), "KeyA".into()))
        );
        assert_eq!(
            dom_key(&Key::Space, Modifiers::default()),
            Some((" ".into(), "Space".into()))
        );
        assert_eq!(
            dom_key(&Key::Left, Modifiers::default()),
            Some(("ArrowLeft".into(), "ArrowLeft".into()))
        );
        assert_eq!(modifier_bits(shift), MODIFIER_SHIFT);
    }
}
