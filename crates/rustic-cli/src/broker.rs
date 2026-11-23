//! The broker side of a split run (ISO-02, ADR 0056).
//!
//! Document commands run in a renderer child process unless
//! `--single-process` is given. The broker keeps all authority: it reads
//! the granted file (or navigates to the granted URL), owns the network
//! session, cookie jar and profile, and holds the font bytes. The renderer
//! gets the document bytes and its policies, and every later request is
//! checked here against what this broker granted, whatever the renderer
//! claims: a local document's resources must lie inside its directory, a
//! network document's go through its session's CSP, mixed-content, CORS
//! and MIME checks. A renderer that dies or breaks the protocol ends the
//! command with an error; nothing it half-produced is written.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use rustic_base::{CancellationToken, EscapedText};
use rustic_engine::{StorageHost, StorageKind};
use rustic_ipc::protocol::{
    BrokerMessage, DocumentGrant, FetchResponse, FrameAsk, PageInput, RendererMessage,
    ResourceBody, ResourceFailure, Start, StorageArea, StorageOp, StorageValue,
};
use rustic_ipc::{Channel, IpcError, Limits, Role};
use rustic_style::FetchError;
use rustic_url::Url;

use crate::loader::{self, LoadError, LoaderLimits};
use crate::network::{NetworkSession, ResourceKind, Session};
use crate::stylesheets::LocalStylesheetFetcher;

static SINGLE_PROCESS: AtomicBool = AtomicBool::new(false);
/// Whether renderers are confined. On by default where Rustic has a
/// sandbox (Linux, Windows, macOS); `--no-sandbox` or `RUSTIC_SANDBOX=0` turns it
/// off, `--sandbox` or `RUSTIC_SANDBOX=1` demands it (an error where none
/// exists).
static SANDBOX: AtomicBool = AtomicBool::new(false);

/// Whether renderers (and the network process) are confined in this run.
/// (On Windows the network process is confined only when no loopback
/// fixture origin is granted: an AppContainer cannot connect to loopback,
/// TD-176.)
/// How long a renderer may run (`--renderer-timeout`); a GPU process gets
/// the same for one composition.
pub(crate) fn renderer_timeout() -> std::time::Duration {
    std::time::Duration::from_secs(TIMEOUT_SECONDS.load(Ordering::Relaxed))
}

pub(crate) fn sandboxed() -> bool {
    SANDBOX.load(Ordering::Relaxed)
}

/// Whether renderers are confined by default here: the verified
/// sandboxes (ADR 0058 Linux, ADR 0060 Windows, ADR 0063 macOS, the last
/// verified on native macOS x64 and ARM64 on 2026-11-22).
pub const SANDBOX_AVAILABLE: bool = cfg!(any(target_os = "linux", windows, target_os = "macos"));
static TIMEOUT_SECONDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(120);

/// Tells a macOS child (`renderer` or `network`) to confine itself with
/// that Seatbelt profile before reading from the broker (ADR 0063).
#[cfg(not(windows))]
pub(crate) const SEATBELT_VARIABLE: &str = "RUSTIC_SEATBELT";

/// In a child process started with [`SEATBELT_VARIABLE`], apply that
/// Seatbelt profile to this process. Called first thing in `main`, before
/// any input from the broker; an error must end the process.
#[cfg(target_os = "macos")]
pub fn confine_self(kind: &std::ffi::OsStr) -> Result<(), String> {
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|error| format!("cannot find the executable: {error}"))?;
    // The GPU process holds no authority and gets the renderer's profile
    // until its backend needs driver access (ADR 0068).
    let profile = if kind == "renderer" || kind == "gpu" {
        rustic_sandbox::seatbelt::renderer_profile(&exe)
    } else if kind == "network" {
        rustic_sandbox::seatbelt::network_profile(&exe)
    } else {
        return Err("unknown sandbox profile".into());
    }
    .map_err(|error| error.to_string())?;
    rustic_sandbox::enter(&profile)
}

/// Environment variables a sandboxed renderer may keep (test hooks and
/// backtraces; nothing that names a user, a home or a display).
const SANDBOX_ENV: &[&str] = &[
    "RUST_BACKTRACE",
    "RUSTIC_TEST_RENDERER_CRASH",
    "RUSTIC_TEST_RENDERER_HANG",
    "RUSTIC_TEST_COMPROMISED_RENDERER",
    "RUSTIC_TEST_ESCAPE_ADDRESS",
    "RUSTIC_TEST_ESCAPE_FILE",
    "RUSTIC_TEST_ESCAPE_HOME",
    "RUSTIC_TEST_ESCAPE_DIR",
    "RUSTIC_TEST_NETWORK_PROCESS",
    "RUSTIC_TEST_GPU_PROCESS",
    "RUSTIC_TEST_LIVE_RENDERER",
    "RUSTIC_TEST_GPU_DEVICE",
    "RUSTIC_GPU",
    "RUSTIC_TEST_ESCAPE_ABSTRACT",
    "RUSTIC_TEST_ESCAPE_UNIX",
    "RUSTIC_TEST_FAULT_URL",
    "RUSTIC_TEST_FAULT",
];

/// Remove the process options (`--single-process`, `--sandbox`,
/// `--no-sandbox`, `--renderer-timeout`) from `args`.
pub fn configure(args: Vec<OsString>) -> Result<Vec<OsString>, String> {
    let mut output = Vec::new();
    let mut args = args.into_iter();
    let mut choice = match std::env::var_os("RUSTIC_SANDBOX") {
        Some(value) if value == "1" => Some(true),
        Some(value) if value == "0" => Some(false),
        Some(_) => return Err("RUSTIC_SANDBOX must be 0 or 1".into()),
        None => None,
    };
    while let Some(arg) = args.next() {
        if arg == "--" {
            output.push(arg);
            output.extend(args);
            break;
        }
        if arg == "--single-process" {
            SINGLE_PROCESS.store(true, Ordering::Relaxed);
        } else if arg == "--sandbox" {
            choice = Some(true);
        } else if arg == "--no-sandbox" {
            choice = Some(false);
        } else if arg == "--renderer-timeout" {
            let seconds = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|&seconds| (1..=86_400).contains(&seconds))
                .ok_or("--renderer-timeout requires seconds from 1 to 86400")?;
            TIMEOUT_SECONDS.store(seconds, Ordering::Relaxed);
        } else {
            output.push(arg);
        }
    }
    let single = SINGLE_PROCESS.load(Ordering::Relaxed);
    if choice == Some(true) && single {
        return Err(
            "--sandbox confines the renderer process; it cannot be used with \
                    --single-process"
                .into(),
        );
    }
    // `--single-process` is an explicit choice of no renderer process, so
    // there is nothing to confine.
    SANDBOX.store(
        !single && choice.unwrap_or(SANDBOX_AVAILABLE),
        Ordering::Relaxed,
    );
    Ok(output)
}

/// Whether `--single-process` was given.
pub fn single_process() -> bool {
    SINGLE_PROCESS.load(Ordering::Relaxed)
}

/// Whether a command runs its document in a renderer.
pub fn splits(command: &OsString) -> bool {
    matches!(
        command.to_str(),
        Some(
            "dump-dom"
                | "dump-style"
                | "dump-boxes"
                | "dump-formatting"
                | "dump-layout"
                | "dump-paint"
                | "dump-a11y"
                | "render"
                | "hit-test"
                | "reftest"
                | "automate"
        )
    )
}

/// Where the renderer's document and resources come from.
#[derive(Clone)]
enum Grant {
    Local {
        root: PathBuf,
    },
    Network(Rc<NetworkSession>),
    /// Nothing beyond the document (`data:` and sandboxed `srcdoc`
    /// frames of local documents).
    Nothing,
}

/// Frames nested below the top-level document at most (ADR 0062).
const MAX_FRAME_DEPTH: usize = 3;
/// Frame documents one command loads at most, over the whole frame tree.
const MAX_FRAMES_PER_COMMAND: usize = 32;

/// Where a renderer's document sits in the frame tree (ADR 0062).
#[derive(Clone)]
struct FrameContext {
    /// 0 for the top-level document.
    depth: usize,
    /// Frame documents this command may still load (shared by the tree).
    budget: Rc<std::cell::Cell<usize>>,
    /// Device pixels per CSS pixel of the render, or `None` for commands
    /// that do not paint (they load no frames).
    factor: Option<f64>,
    /// The top-level renderer's arguments (limits, scripts, locale).
    args: Vec<String>,
    /// The document, without its bytes: URL, policies, storage.
    document: DocumentGrant,
    /// The document's origin and its ancestors' origins (parent first).
    origin: rustic_url::Origin,
    ancestors: Vec<rustic_url::Origin>,
    /// Whether the document's renderer may run scripts.
    scripts: bool,
    /// Popups this command may still open, or `None` where this document
    /// may open none (frames and popups themselves).
    popups: Option<Rc<std::cell::Cell<usize>>>,
}

/// Popups one command opens at most (ADR 0057 §3).
const MAX_POPUPS_PER_COMMAND: usize = 4;

/// The arguments for a frame's renderer: the parent's limits and locale,
/// scripts only if allowed, and the frame's own view.
fn frame_args(
    parent: &[String],
    (width, height): (u32, u32),
    factor: f64,
    scripts: bool,
) -> Vec<String> {
    let mut args = child_args(parent, scripts);
    args.extend([
        "--viewport".to_owned(),
        format!("{width}x{height}"),
        "--scale".to_owned(),
        format!("{factor}"),
        "--output".to_owned(),
        "frame.png".to_owned(),
        "--".to_owned(),
        "frame".to_owned(),
    ]);
    args
}

/// The parent's limits and locale for another document's renderer, and
/// `--run-scripts` only if `scripts`: no view, output or input options.
fn child_args(parent: &[String], scripts: bool) -> Vec<String> {
    const VIEW: &[&str] = &[
        "--viewport",
        "--scale",
        "--zoom",
        "--output",
        "--at",
        "--click",
        "--scroll",
        "--scroll-box",
        "--frames",
        "--devtools-allow",
    ];
    let mut args = Vec::new();
    let mut iter = parent.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--" => break,
            "--run-scripts" => {
                if scripts {
                    args.push(arg.clone());
                }
            }
            option if VIEW.contains(&option) => {
                iter.next();
            }
            option if option.starts_with("--") => {
                args.push(arg.clone());
                if let Some(value) = iter.next() {
                    args.push(value.clone());
                }
            }
            _ => {}
        }
    }
    args
}

/// A composited page and the renderer's standard output.
type Captured = (rustic_raster::Surface, Vec<u8>);

thread_local! {
    /// `Some` while [`render_surface`] runs: `serve` leaves the page here
    /// instead of writing it.
    static CAPTURE: std::cell::RefCell<Option<Option<Captured>>> =
        const { std::cell::RefCell::new(None) };
    /// The URL of the document the last command committed (after
    /// redirects), decided here, never reported by the renderer: the
    /// window's trusted address display (TD-173).
    static COMMITTED: std::cell::RefCell<Option<Url>> = const { std::cell::RefCell::new(None) };
}

/// The URL of the document the last brokered command committed, for the
/// trusted address display. The broker decides it; a renderer cannot.
pub fn committed_url() -> Option<Url> {
    COMMITTED.with(|committed| committed.borrow().clone())
}

/// Run `render` with `args` (which must name an `--output`, unused) in a
/// renderer and return the composited page and the renderer's standard
/// output (`--report-scroll`), for the window host (`view`, ADR 0065).
pub fn render_surface(args: &[OsString]) -> Result<Captured, u8> {
    CAPTURE.with(|slot| *slot.borrow_mut() = Some(None));
    let status = run("render", args);
    let captured = CAPTURE.with(|slot| slot.borrow_mut().take()).flatten();
    match captured {
        Some(page) if status == 0 => Ok(page),
        _ => Err(if status == 0 { 1 } else { status }),
    }
}

/// A document command ready for its renderer: the grant, the job and the
/// frame tree's context.
struct Prepared {
    grant: Grant,
    start: Start,
    limits: LoaderLimits,
    output: Option<OsString>,
    context: FrameContext,
}

/// Run `command` with its document in a renderer. Returns the exit status.
pub fn run(command: &str, args: &[OsString]) -> u8 {
    if args.iter().any(|arg| arg == "--live") {
        eprintln!("rustic {command}: --live is how `drive` and `view` start a live page");
        return 2;
    }
    let prepared = match prepare(command, args) {
        Ok(prepared) => prepared,
        Err(status) => return status,
    };
    let status = serve(
        command,
        prepared.start,
        &prepared.grant,
        prepared.limits,
        prepared.output.as_ref(),
        &prepared.context,
    );
    if let Grant::Network(session) = &prepared.grant {
        if let Err(error) = session.finish() {
            eprintln!("rustic {command}: profile: {error}");
            return 1;
        }
    }
    status
}

/// Decide `command`'s document and grant: read the granted file or
/// navigate to the granted URL. `Err` carries the exit status after the
/// error was reported.
fn prepare(command: &str, args: &[OsString]) -> Result<Prepared, u8> {
    COMMITTED.with(|committed| committed.borrow_mut().take());
    let Some((limits, path, output)) = document_argument(command, args) else {
        // A usage error: let the command report it; nothing is loaded.
        let mut engine = rustic_engine::Engine::new(1);
        let mut all = vec![OsString::from(command)];
        all.extend_from_slice(args);
        return Err(crate::run_command(&mut engine, &all).unwrap_or(2));
    };
    let shown = EscapedText(&Path::new(&path).to_string_lossy()).to_string();
    let fail = |error: LoadError| -> u8 {
        if matches!(error, LoadError::InvalidLimits) {
            eprintln!("rustic {command}: {error}");
            2
        } else {
            eprintln!("rustic {command}: {shown}: {error}");
            1
        }
    };
    let (grant, document) = if crate::network::is_network_argument(&path) {
        if !crate::network::has_grants() {
            eprintln!(
                "rustic {command}: network URLs need --allow-origin ORIGIN[=ADDRESS] for each \
                 origin, or --allow-remote; nothing was sent"
            );
            return Err(2);
        }
        let url = path.to_str().unwrap_or_default();
        let navigated = match crate::network::navigate(
            url,
            limits.max_file_bytes,
            CancellationToken::default(),
        ) {
            Ok(navigated) => navigated,
            Err(error) => return Err(fail(LoadError::Network(error))),
        };
        let session = Rc::clone(&navigated.session);
        let document = DocumentGrant {
            url: session.document_url().serialize(),
            bytes: navigated.bytes,
            transport_charset: navigated.transport_encoding.map(|e| e.name().to_owned()),
            local: false,
            csp: navigated.policy.csp.clone(),
            csp_report_only: navigated.policy.csp_report_only.clone(),
            referrer_policy: navigated.policy.referrer_policy.as_str().to_owned(),
            storage: !session.document_url().origin().is_opaque(),
        };
        (Grant::Network(session), document)
    } else {
        let local = match loader::local_path(&path) {
            Ok(local) => local,
            Err(error) => {
                eprintln!("rustic {command}: {error}");
                return Err(2);
            }
        };
        let (canonical, bytes) = match loader::read_granted_file(&local, limits) {
            Ok(read) => read,
            Err(error) => return Err(fail(error)),
        };
        let Some(url) = crate::stylesheets::file_url(&canonical) else {
            return Err(fail(LoadError::NotLocalPath));
        };
        let root = canonical
            .parent()
            .expect("a canonical regular file has a parent")
            .to_owned();
        let document = DocumentGrant {
            url: url.serialize(),
            bytes,
            local: true,
            ..DocumentGrant::default()
        };
        (Grant::Local { root }, document)
    };
    COMMITTED.with(|committed| *committed.borrow_mut() = Url::parse(&document.url, None).ok());
    let mut renderer_args: Vec<String> = Vec::new();
    if let Some(locale) = crate::network::configured_locale() {
        renderer_args.extend(["--locale".to_owned(), locale.to_owned()]);
    }
    renderer_args.extend(args.iter().map(|arg| arg.to_string_lossy().into_owned()));
    let factor = matches!(command, "render" | "reftest")
        .then(|| crate::parse_render_args(args).ok())
        .flatten()
        .and_then(|parsed| {
            rustic_layout::CssPx::new(1.0)
                .ok()
                .map(|one| one.to_device(parsed.view.scale()).get())
        });
    let origin = match &grant {
        Grant::Network(session) => session.origin().clone(),
        _ => rustic_url::Origin::opaque(),
    };
    let context = FrameContext {
        depth: 0,
        budget: Rc::new(std::cell::Cell::new(MAX_FRAMES_PER_COMMAND)),
        factor,
        scripts: limits.run_scripts,
        args: renderer_args.clone(),
        document: DocumentGrant {
            bytes: Vec::new(),
            ..document.clone()
        },
        origin,
        ancestors: Vec::new(),
        popups: Some(Rc::new(std::cell::Cell::new(MAX_POPUPS_PER_COMMAND))),
    };
    let start = Start {
        command: command.to_owned(),
        args: renderer_args,
        document,
        fonts: crate::fonts::granted_bytes().to_vec(),
    };
    Ok(Prepared {
        grant,
        start,
        limits,
        output,
        context,
    })
}

/// Times a live page's renderer is replaced after it died, hung or broke
/// the protocol, before the page gives up (TD-182).
pub const MAX_LIVE_RESTARTS: u32 = 3;
/// Longest a live page may take to answer input or a frame once loaded
/// (the load itself gets the renderer timeout).
const LIVE_ANSWER_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// One frame of a live page, composited and checked by the broker.
#[derive(Debug)]
pub struct LiveFrame {
    /// The page at the asked size and scale.
    pub surface: rustic_raster::Surface,
    /// The viewport position painted, in CSS pixels (finite, at least 0).
    pub scroll: (f64, f64),
    /// When the page wants another frame (timers, animation frames).
    pub next_frame: Option<std::time::Duration>,
}

/// A page loaded once in a long-lived renderer (TD-182): input and frame
/// requests go to the same renderer, which keeps the document, its
/// scripts and timers between frames. The broker keeps every authority:
/// the renderer's requests are answered from the page's grant as in a
/// one-shot run, and every frame it returns is checked. A renderer that
/// dies, hangs or breaks the protocol is replaced by a fresh one that
/// loads the page again (at most [`MAX_LIVE_RESTARTS`] times).
pub struct LivePage<'a> {
    prepared: &'a Prepared,
    running: Option<LiveRenderer<'a>>,
    restarts: u32,
    /// Notes for the user about replaced renderers, taken by the host.
    notes: Vec<String>,
}

struct LiveRenderer<'a> {
    process: Process,
    channel: Channel<Box<dyn Read + Send>, Box<dyn Write + Send>>,
    relay: Option<std::thread::JoinHandle<()>>,
    broker: Broker<'a>,
    /// Whether the renderer answered once (finished loading).
    loaded: bool,
}

impl Drop for LiveRenderer<'_> {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        if let Some(relay) = self.relay.take() {
            let _ = relay.join();
        }
    }
}

/// Load `args` (`render` arguments: the document and its limits; the
/// window gives the view) as a live page and call `f` with it. The page's
/// scripts run (a live page is a browsing page). `Err` is the exit status
/// after the problem was reported.
pub fn live<R>(args: &[OsString], f: impl FnOnce(&mut LivePage<'_>) -> R) -> Result<R, u8> {
    if single_process() {
        eprintln!("rustic: a live page runs in a renderer process; not with --single-process");
        return Err(2);
    }
    let mut all: Vec<OsString> = vec!["--live".into()];
    if !args.iter().any(|arg| arg == "--run-scripts") {
        all.push("--run-scripts".into());
    }
    all.extend_from_slice(args);
    if !all.iter().any(|arg| arg == "--output") {
        all.extend(["--output".into(), "live.png".into()]);
    }
    let prepared = prepare("render", &all)?;
    let mut page = LivePage {
        prepared: &prepared,
        running: None,
        restarts: 0,
        notes: Vec::new(),
    };
    let result = f(&mut page);
    drop(page);
    if let Grant::Network(session) = &prepared.grant {
        if let Err(error) = session.finish() {
            eprintln!("rustic: profile: {error}");
            return Err(1);
        }
    }
    Ok(result)
}

/// A live renderer's answer to one [`PageInput`].
enum Answer {
    Painted(rustic_ipc::protocol::Painted),
    Handled,
    Accessibility(Vec<u8>),
}

impl<'a> LivePage<'a> {
    /// How many times the renderer was replaced.
    pub fn restarts(&self) -> u32 {
        self.restarts
    }

    /// Take the notes about replaced renderers since the last call.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// Deliver a primary click at window CSS pixels (100% zoom) from the
    /// page area's top-left corner.
    pub fn click(&mut self, x: f64, y: f64, modifiers: u8) -> Result<(), String> {
        if !(x.is_finite() && y.is_finite()) {
            return Ok(());
        }
        self.exchange(&PageInput::Click { x, y, modifiers })
            .map(drop)
    }

    /// Deliver a key press or release (DOM `key` and `code` values; longer
    /// than 64 bytes are dropped).
    pub fn key(
        &mut self,
        down: bool,
        key: &str,
        code: &str,
        repeat: bool,
        modifiers: u8,
    ) -> Result<(), String> {
        let limit = rustic_ipc::protocol::MAX_KEY_BYTES;
        if key.len() > limit || code.len() > limit {
            return Ok(());
        }
        let input = PageInput::Key {
            down,
            key: key.to_owned(),
            code: code.to_owned(),
            repeat,
            modifiers,
        };
        self.exchange(&input).map(drop)
    }

    /// Deliver an IME composition step to the focused element (ADR 0070);
    /// over-long text is dropped.
    pub fn composition(
        &mut self,
        phase: rustic_ipc::protocol::CompositionPhase,
        text: &str,
    ) -> Result<(), String> {
        if text.len() > rustic_ipc::protocol::MAX_INPUT_TEXT_BYTES {
            return Ok(());
        }
        let input = PageInput::Composition {
            phase,
            text: text.to_owned(),
        };
        self.exchange(&input).map(drop)
    }

    /// Deliver an assistive technology's request for accessibility node
    /// `node` (ADR 0070).
    pub fn assistive(
        &mut self,
        node: u64,
        request: rustic_ipc::protocol::AssistiveRequest,
    ) -> Result<(), String> {
        if let rustic_ipc::protocol::AssistiveRequest::SetValue(text) = &request {
            if text.len() > rustic_ipc::protocol::MAX_INPUT_TEXT_BYTES {
                return Ok(());
            }
        }
        self.exchange(&PageInput::Assistive { node, request })
            .map(drop)
    }

    /// The page's accessibility tree now, decoded as hostile input
    /// (ADR 0070); `None` when the renderer sent none or an invalid one.
    pub fn accessibility(&mut self) -> Result<Option<rustic_a11y::AxTree>, String> {
        Ok(match self.exchange(&PageInput::Accessibility)? {
            Answer::Accessibility(bytes) => {
                rustic_a11y::wire::decode(&bytes, rustic_a11y::TreeLimits::default()).ok()
            }
            _ => None,
        })
    }

    /// Advance the page's clock by `ask.elapsed_us`, run what is due and
    /// paint it at the asked size, scale, zoom and scroll position.
    pub fn frame(&mut self, ask: FrameAsk) -> Result<LiveFrame, String> {
        let Answer::Painted(painted) = self.exchange(&PageInput::Frame(ask))? else {
            return Err("the page sent no frame".into());
        };
        let surface = self
            .running
            .as_ref()
            .and_then(|running| running.broker.composite(painted.surface))
            .ok_or("the page's renderer sent an invalid frame")?;
        let (x, y) = painted.scroll;
        let scroll = if x.is_finite() && y.is_finite() {
            (x.max(0.0), y.max(0.0))
        } else {
            (0.0, 0.0)
        };
        Ok(LiveFrame {
            surface,
            scroll,
            next_frame: painted.next_frame_us.map(std::time::Duration::from_micros),
        })
    }

    /// Send `input` and serve the renderer's requests until it answers,
    /// replacing a renderer that fails (the input then goes to the fresh
    /// one). A frame request is answered with its frame.
    fn exchange(&mut self, input: &PageInput) -> Result<Answer, String> {
        loop {
            if self.running.is_none() {
                self.running = Some(self.start()?);
            }
            let running = self.running.as_mut().expect("started above");
            match Self::serve_one(running, input, self.prepared.limits) {
                Ok(answer) => return Ok(answer),
                Err(why) => {
                    self.running = None;
                    if self.restarts >= MAX_LIVE_RESTARTS {
                        return Err(format!(
                            "the page's renderer {why}; it was replaced {MAX_LIVE_RESTARTS} \
                             times and the page was closed"
                        ));
                    }
                    self.restarts += 1;
                    self.notes.push(format!(
                        "the page's renderer {why}; the page was reloaded in a new renderer"
                    ));
                }
            }
        }
    }

    fn start(&self) -> Result<LiveRenderer<'a>, String> {
        let prepared = self.prepared;
        let (process, stdout, stdin, relay) = spawn().map_err(|error| {
            if SANDBOX.load(Ordering::Relaxed)
                && cfg!(not(any(windows, target_os = "macos")))
                && error.kind() == io::ErrorKind::NotFound
            {
                "--sandbox needs bubblewrap (bwrap) on PATH; the renderer was not started \
                 without its sandbox"
                    .to_owned()
            } else {
                format!("cannot start the renderer process: {error}")
            }
        })?;
        let mut process = process;
        let channel = match Channel::connect(stdout, stdin, Role::Broker, Limits::default()) {
            Ok(channel) => channel,
            Err(error) => {
                let _ = process.kill();
                let status = process
                    .wait()
                    .map_or_else(|error| error.to_string(), |status| status.to_string());
                let _ = relay.join();
                return Err(format!(
                    "the renderer could not start ({error}, {status}); nothing ran unsandboxed"
                ));
            }
        };
        let mut running = LiveRenderer {
            process,
            channel,
            relay: Some(relay),
            broker: Broker::new(&prepared.grant, prepared.limits, &prepared.context),
            loaded: false,
        };
        running
            .channel
            .send(&BrokerMessage::Start(prepared.start.clone()))
            .map_err(|error| format!("cannot reach the renderer: {error}"))?;
        Ok(running)
    }

    fn serve_one(
        running: &mut LiveRenderer<'_>,
        input: &PageInput,
        limits: LoaderLimits,
    ) -> Result<Answer, String> {
        let limit = if running.loaded {
            renderer_timeout().min(LIVE_ANSWER_LIMIT)
        } else {
            renderer_timeout()
        };
        let kill = running.process.killer();
        let (answered, watch) = std::sync::mpsc::channel::<()>();
        let timed_out = std::sync::Arc::new(AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&timed_out);
        let watchdog = std::thread::spawn(move || {
            if watch.recv_timeout(limit) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
                flag.store(true, Ordering::SeqCst);
                kill();
            }
        });
        let frame = matches!(input, PageInput::Frame(_));
        let tree = matches!(input, PageInput::Accessibility);
        let result = (|| -> Result<Answer, IpcError> {
            running.channel.send(&BrokerMessage::Page(input.clone()))?;
            loop {
                match running.channel.recv::<RendererMessage>()? {
                    RendererMessage::Diagnostic(text) => eprintln!("{}", EscapedText(&text)),
                    RendererMessage::Painted(painted) if frame => {
                        let bytes = painted.surface.pixels.len();
                        if bytes > limits.max_dump_bytes.max(256 << 20) {
                            return Err(IpcError::Malformed(rustic_ipc::DecodeError::OutOfRange));
                        }
                        return Ok(Answer::Painted(painted));
                    }
                    RendererMessage::Accessibility(bytes) if tree => {
                        return Ok(Answer::Accessibility(bytes));
                    }
                    RendererMessage::Handled if !frame && !tree => return Ok(Answer::Handled),
                    RendererMessage::Painted(_)
                    | RendererMessage::Accessibility(_)
                    | RendererMessage::Handled
                    | RendererMessage::Surface(_)
                    | RendererMessage::Done { .. } => {
                        return Err(IpcError::Malformed(rustic_ipc::DecodeError::OutOfRange));
                    }
                    request => {
                        let reply = running.broker.handle(request);
                        running.channel.send(&reply)?;
                    }
                }
            }
        })();
        let _ = answered.send(());
        let _ = watchdog.join();
        match result {
            Ok(answer) => {
                running.loaded = true;
                Ok(answer)
            }
            Err(_) if timed_out.load(Ordering::SeqCst) => {
                Err(format!("did not answer within {} s", limit.as_secs()))
            }
            Err(IpcError::Closed) => {
                let _ = running.process.kill();
                let status = running
                    .process
                    .wait()
                    .map_or_else(|error| error.to_string(), |status| status.to_string());
                Err(format!("ended ({status})"))
            }
            Err(error) => Err(format!("broke the IPC protocol ({error})")),
        }
    }
}

/// The limits, document argument and `--output` of a document command, or
/// `None` on a usage error.
fn document_argument(
    command: &str,
    args: &[OsString],
) -> Option<(LoaderLimits, OsString, Option<OsString>)> {
    match command {
        "render" => {
            let parsed = crate::parse_render_args(args).ok()?;
            parsed.output.as_ref()?;
            Some((parsed.limits, parsed.path, parsed.output))
        }
        "reftest" => {
            // References come from the broker inside the grant (TD-171).
            let parsed = crate::parse_render_args(args).ok()?;
            if parsed.output.is_some() {
                return None;
            }
            Some((parsed.limits, parsed.path, None))
        }
        "hit-test" => {
            let mut rest = Vec::new();
            let mut iter = args.iter();
            let mut points = 0;
            while let Some(arg) = iter.next() {
                if arg == "--" {
                    rest.push(arg.clone());
                    rest.extend(iter.by_ref().cloned());
                } else if arg == "--at" {
                    iter.next()?;
                    points += 1;
                } else {
                    rest.push(arg.clone());
                }
            }
            if points == 0 {
                return None;
            }
            let (limits, path) = crate::parse_dump_args(&rest).ok()?;
            Some((limits, path, None))
        }
        _ => {
            let (mut limits, path) = crate::parse_dump_args(args).ok()?;
            // `automate` always runs scripts (ADR 0069).
            limits.run_scripts |= command == "automate";
            Some((limits, path, None))
        }
    }
}

/// A renderer process: unconfined (or self-confined by Seatbelt on macOS), in
/// bubblewrap, or in the Windows AppContainer. The network process uses
/// it too.
pub(crate) enum Process {
    Child(Child),
    /// A bubblewrap renderer and its filter file, which lives until the
    /// launcher has opened (and removed) it or the process is gone.
    #[cfg(not(any(windows, target_os = "macos")))]
    Sandboxed {
        child: Child,
        _filter: SeccompFile,
    },
    #[cfg(windows)]
    Confined(rustic_sandbox::Confined),
    /// A renderer the host started (Android's isolated service); stopping
    /// it closes its socket.
    Hosted(std::sync::Arc<dyn Fn() + Send + Sync>),
}

impl Process {
    /// What the watchdog calls to stop this process from another thread.
    pub(crate) fn killer(&self) -> Box<dyn FnOnce() + Send> {
        match self {
            Self::Hosted(stop) => {
                let stop = std::sync::Arc::clone(stop);
                Box::new(move || stop())
            }
            other => {
                let pid = other.id();
                Box::new(move || kill_process(pid))
            }
        }
    }

    fn id(&self) -> u32 {
        match self {
            Self::Child(child) => child.id(),
            #[cfg(not(any(windows, target_os = "macos")))]
            Self::Sandboxed { child, .. } => child.id(),
            #[cfg(windows)]
            Self::Confined(confined) => confined.id(),
            Self::Hosted(_) => 0,
        }
    }

    pub(crate) fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        match self {
            Self::Child(child) => child.wait(),
            #[cfg(not(any(windows, target_os = "macos")))]
            Self::Sandboxed { child, .. } => child.wait(),
            #[cfg(windows)]
            Self::Confined(confined) => confined.wait(),
            Self::Hosted(_) => Err(io::Error::other("the renderer service ended")),
        }
    }

    pub(crate) fn kill(&mut self) -> io::Result<()> {
        match self {
            Self::Child(child) => child.kill(),
            #[cfg(not(any(windows, target_os = "macos")))]
            Self::Sandboxed { child, .. } => child.kill(),
            #[cfg(windows)]
            Self::Confined(confined) => confined.kill(),
            Self::Hosted(stop) => {
                stop();
                Ok(())
            }
        }
    }
}

/// The renderer's standard error, whichever way it was started.
pub(crate) enum RendererStderr {
    Pipe(std::process::ChildStderr),
    /// A hosted renderer's diagnostics stay with its host.
    Nothing,
    #[cfg(windows)]
    File(std::fs::File),
}

impl Read for RendererStderr {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Pipe(pipe) => pipe.read(buffer),
            Self::Nothing => Ok(0),
            #[cfg(windows)]
            Self::File(file) => file.read(buffer),
        }
    }
}

/// A renderer a host started outside this process tree: the Android
/// isolated renderer service (ADR 0059), connected over a socket the host
/// handed over.
pub struct HostedRenderer {
    /// The renderer's end of the stream, read side.
    pub reader: Box<dyn Read + Send>,
    /// The renderer's end of the stream, write side.
    pub writer: Box<dyn Write + Send>,
    /// Stops the renderer (closes the socket; the host unbinds the
    /// service). Called by the watchdog and on protocol errors.
    pub stop: std::sync::Arc<dyn Fn() + Send + Sync>,
}

thread_local! {
    /// The renderer the next command on this thread uses instead of
    /// starting a process (one per command: a second renderer, such as a
    /// frame's, is refused).
    static HOSTED: std::cell::RefCell<Option<HostedRenderer>> =
        const { std::cell::RefCell::new(None) };
    /// `Some` while [`run_hosted`] runs: command output collects here.
    static OUTPUT: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
}

/// Run document `command` with `args` against `renderer`, which the host
/// started (the Android isolated service), and return the exit status and
/// what the command would have written to standard output.
pub fn run_hosted(command: &str, args: &[OsString], renderer: HostedRenderer) -> (u8, Vec<u8>) {
    HOSTED.with(|hosted| *hosted.borrow_mut() = Some(renderer));
    OUTPUT.with(|output| *output.borrow_mut() = Some(Vec::new()));
    let status = run(command, args);
    HOSTED.with(|hosted| hosted.borrow_mut().take());
    let output = OUTPUT.with(|output| output.borrow_mut().take().unwrap_or_default());
    (status, output)
}

/// Command output: to standard output, or to the host's buffer.
fn write_output(bytes: &[u8]) -> io::Result<()> {
    let collected = OUTPUT.with(|output| {
        output
            .borrow_mut()
            .as_mut()
            .map(|buffer| buffer.extend_from_slice(bytes))
            .is_some()
    });
    if collected {
        return Ok(());
    }
    let mut stdout = io::stdout().lock();
    stdout.write_all(bytes)?;
    stdout.flush()
}

type Started = (
    Process,
    Box<dyn Read + Send>,
    Box<dyn Write + Send>,
    RendererStderr,
);

type Spawned = (
    Process,
    Box<dyn Read + Send>,
    Box<dyn Write + Send>,
    std::thread::JoinHandle<()>,
);

/// The sandbox environment: only the allowed variables (test hooks and
/// backtraces; nothing that names a user, a home or a display).
pub(crate) fn sandbox_env() -> Vec<(OsString, OsString)> {
    SANDBOX_ENV
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (OsString::from(name), value)))
        .collect()
}

fn pipes(child: &mut Child) -> (Box<dyn Read + Send>, Box<dyn Write + Send>, RendererStderr) {
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let stderr = RendererStderr::Pipe(child.stderr.take().expect("piped"));
    (Box::new(stdout), Box::new(stdin), stderr)
}

/// Start the renderer: the executable itself, or, under the sandbox,
/// that executable confined by the operating system (ADR 0058 on Linux,
/// ADR 0060 on Windows). A sandbox that cannot be set up is an error.
fn spawn_process() -> io::Result<Started> {
    if let Some(hosted) = HOSTED.with(|hosted| hosted.borrow_mut().take()) {
        return Ok((
            Process::Hosted(hosted.stop),
            hosted.reader,
            hosted.writer,
            RendererStderr::Nothing,
        ));
    }
    if OUTPUT.with(|output| output.borrow().is_some()) {
        return Err(io::Error::other(
            "the host provides one renderer per command",
        ));
    }
    let exe = std::env::current_exe()?;
    if SANDBOX.load(Ordering::Relaxed) {
        return spawn_confined(&exe);
    }
    let mut child = Command::new(exe)
        .arg(crate::renderer::RENDERER_COMMAND)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let (stdout, stdin, stderr) = pipes(&mut child);
    Ok((Process::Child(child), stdout, stdin, stderr))
}

/// Windows: the executable in the renderer AppContainer with no
/// capabilities, in a kill-on-close job of one process (ADR 0060).
#[cfg(windows)]
fn spawn_confined(exe: &Path) -> io::Result<Started> {
    let command = std::ffi::OsStr::new(crate::renderer::RENDERER_COMMAND);
    let (confined, pipes) = rustic_sandbox::spawn_app_container(
        rustic_sandbox::Container::Renderer,
        exe,
        &[command],
        &sandbox_env(),
    )?;
    Ok((
        Process::Confined(confined),
        Box::new(pipes.stdout),
        Box::new(pipes.stdin),
        RendererStderr::File(pipes.stderr),
    ))
}

/// macOS (ADR 0063): the executable with a cleared environment
/// and `RUSTIC_SEATBELT=renderer`, which makes it apply the deny-by-default
/// Seatbelt profile of `rustic_sandbox::seatbelt` to itself before it
/// reads anything from the broker (a failure ends it before the
/// handshake, which the broker reports as a sandbox failure). A renderer
/// has no parent-death signal here; it ends when its IPC pipe closes, and
/// the watchdog stops a hung one.
#[cfg(target_os = "macos")]
fn spawn_confined(exe: &Path) -> io::Result<Started> {
    let mut child = Command::new(exe)
        .arg(crate::renderer::RENDERER_COMMAND)
        .env_clear()
        .envs(sandbox_env())
        .env(SEATBELT_VARIABLE, "renderer")
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let (stdout, stdin, stderr) = pipes(&mut child);
    Ok((Process::Child(child), stdout, stdin, stderr))
}

/// Linux: the executable inside bubblewrap with every namespace unshared
/// (no network), a filesystem of the read-only system libraries and the
/// executable only, no capabilities, a new session (no terminal
/// injection), a cleared environment, death with the broker, and the
/// seccomp-bpf filter of `rustic_sandbox::seccomp` (ADR 0058). `/bin/sh`
/// opens the filter as descriptor 3 for `bwrap --seccomp` (std cannot pass
/// a descriptor without unsafe code) and removes the file first.
#[cfg(not(any(windows, target_os = "macos")))]
fn spawn_confined(exe: &Path) -> io::Result<Started> {
    let (child, filter, stdout, stdin, stderr) = spawn_bubblewrap(
        exe,
        crate::renderer::RENDERER_COMMAND,
        Confinement::Renderer,
    )?;
    Ok((
        Process::Sandboxed {
            child,
            _filter: filter,
        },
        stdout,
        stdin,
        stderr,
    ))
}

/// A confined child started by [`spawn_bubblewrap`], with its pipes.
#[cfg(not(windows))]
pub(crate) type Bubblewrapped = (
    Child,
    SeccompFile,
    Box<dyn Read + Send>,
    Box<dyn Write + Send>,
    RendererStderr,
);

/// Start `exe command` under bubblewrap and seccomp. A renderer
/// (`network == false`) gets every namespace unshared and the renderer
/// filter; the network process (ADR 0061, TD-176) keeps the network
/// namespace, read-only trust-store directories and the network filter,
/// which admits only Internet-family client sockets.
#[cfg(not(windows))]
/// Which child a bubblewrap sandbox confines (its seccomp filter and binds).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Confinement {
    /// A renderer: no files beyond `/usr`, no sockets, no devices.
    Renderer,
    /// The network process: Internet client sockets and the trust store.
    Network,
    /// The GPU process (ADR 0071): the renderer's rules plus anonymous
    /// memory files, the GPU device nodes and the sysfs entries drivers
    /// read to identify them.
    Gpu,
}

pub(crate) fn spawn_bubblewrap(
    exe: &Path,
    command_name: &str,
    confinement: Confinement,
) -> io::Result<Bubblewrapped> {
    let network = confinement == Confinement::Network;
    if !cfg!(target_os = "linux") {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "--sandbox is implemented on Linux, Windows and (proposed, ADR 0063) macOS only",
        ));
    }
    let arch = rustic_sandbox::seccomp::Arch::current().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "the renderer sandbox has no system-call table for this CPU architecture; \
             run with --no-sandbox to accept an unconfined renderer",
        )
    })?;
    let exe = exe.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the executable path is not UTF-8",
        )
    })?;
    let program = match confinement {
        Confinement::Renderer => rustic_sandbox::seccomp::renderer_filter(arch),
        Confinement::Network => rustic_sandbox::seccomp::network_filter(arch),
        Confinement::Gpu => rustic_sandbox::seccomp::gpu_filter(arch),
    };
    let filter = SeccompFile::create(&rustic_sandbox::seccomp::to_bytes(&program))?;
    let mut command = Command::new("/bin/sh");
    command.args([
        "-c",
        "exec 3<\"$0\" && rm -f -- \"$0\" && exec bwrap --seccomp 3 \"$@\"",
    ]);
    command.arg(&filter.0);
    command.arg("--unshare-all");
    if network {
        command.arg("--share-net");
    }
    command.args([
        "--die-with-parent",
        "--new-session",
        "--cap-drop",
        "ALL",
        "--clearenv",
        "--ro-bind",
        "/usr",
        "/usr",
        "--symlink",
        "usr/lib",
        "/lib",
        "--symlink",
        "usr/lib64",
        "/lib64",
        "--ro-bind",
        exe,
        "/renderer",
        "--chdir",
        "/",
    ]);
    if network {
        // The platform verifier reads the system trust store; nothing
        // else under /etc (no resolver configuration: there is no DNS,
        // TD-157).
        for path in ["/etc/ssl", "/etc/pki", "/etc/ca-certificates"] {
            command.args(["--ro-bind-try", path, path]);
        }
    }
    if confinement == Confinement::Gpu {
        // Render nodes and the PCI identity drivers match against; Vulkan
        // ICD manifests live under /usr (already bound) or /etc/vulkan.
        command.args(["--dev-bind-try", "/dev/dri", "/dev/dri"]);
        for path in [
            "/sys/dev/char",
            "/sys/devices",
            "/sys/bus/pci",
            "/etc/vulkan",
        ] {
            command.args(["--ro-bind-try", path, path]);
        }
    }
    for (name, value) in sandbox_env() {
        command.arg("--setenv").arg(name).arg(value);
    }
    command.args(["/renderer", command_name]);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let (stdout, stdin, stderr) = pipes(&mut child);
    Ok((child, filter, stdout, stdin, stderr))
}

/// The filter program in a new private file, removed on drop unless the
/// launcher has removed it already.
#[cfg(not(windows))]
pub(crate) struct SeccompFile(PathBuf);

#[cfg(not(windows))]
impl SeccompFile {
    fn create(bytes: &[u8]) -> io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "rustic-seccomp-{}-{}.bpf",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let created = Self(path);
        file.write_all(bytes)?;
        Ok(created)
    }
}

#[cfg(not(windows))]
impl Drop for SeccompFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn spawn() -> io::Result<Spawned> {
    let (process, stdout, stdin, stderr) = spawn_process()?;
    // Relay the renderer's diagnostics with control characters escaped, so
    // a compromised renderer cannot drive the user's terminal.
    let relay = std::thread::spawn(move || {
        let mut lines = BufReader::new(stderr);
        let mut line = Vec::new();
        while lines.read_until(b'\n', &mut line).unwrap_or(0) > 0 {
            let text = String::from_utf8_lossy(&line);
            let trimmed = text.strip_suffix('\n').unwrap_or(&text);
            eprintln!("{}", EscapedText(trimmed));
            line.clear();
        }
    });
    Ok((process, stdout, stdin, relay))
}

/// How a renderer ended, when it did not finish normally.
enum Ended {
    /// It could not be started.
    NotStarted(io::Error),
    /// The watchdog stopped it.
    TimedOut(u64),
    /// The sandboxed renderer died before the handshake.
    SandboxFailed(String),
    /// It died before finishing.
    Died(String),
    /// It broke the protocol and was stopped.
    Broke(IpcError),
}

/// What a renderer that finished produced.
struct Finished {
    status: u8,
    output: Vec<u8>,
    /// `render`: the page with its frames composited (one per frame with
    /// `--frames`).
    surfaces: Vec<rustic_raster::Surface>,
}

/// Run one renderer to the end: start it (confined when the sandbox is
/// on), arm the watchdog, send `start` and answer its requests from
/// `grant` until it is done.
fn run_renderer(
    start: Start,
    grant: &Grant,
    limits: LoaderLimits,
    context: &FrameContext,
    automation: Option<rustic_devtools::Grants>,
) -> Result<Finished, Ended> {
    let (mut child, stdout, stdin, relay) = spawn().map_err(Ended::NotStarted)?;
    // Watchdog: a renderer that does not finish in time is killed, and the
    // pipe closing ends the wait below (security.recovery).
    let seconds = TIMEOUT_SECONDS.load(Ordering::Relaxed);
    let kill = child.killer();
    let (finished, watch) = std::sync::mpsc::channel::<()>();
    let timed_out = std::sync::Arc::new(AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&timed_out);
    let watchdog = std::thread::spawn(move || {
        if watch.recv_timeout(std::time::Duration::from_secs(seconds))
            == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        {
            flag.store(true, Ordering::SeqCst);
            kill();
        }
    });
    let handshake_done = AtomicBool::new(false);
    let result = (|| -> Result<Finished, IpcError> {
        let mut channel = Channel::connect(stdout, stdin, Role::Broker, Limits::default())?;
        handshake_done.store(true, Ordering::SeqCst);
        channel.send(&BrokerMessage::Start(start))?;
        let mut broker = Broker::new(grant, limits, context);
        let mut surfaces: Vec<rustic_raster::Surface> = Vec::new();
        let mut bytes = 0usize;
        let refused = || IpcError::Malformed(rustic_ipc::DecodeError::OutOfRange);
        loop {
            match channel.recv::<RendererMessage>()? {
                RendererMessage::Done { status, output } => {
                    return Ok(Finished {
                        status,
                        output,
                        surfaces,
                    });
                }
                RendererMessage::Diagnostic(text) => eprintln!("{}", EscapedText(&text)),
                // An automation session (ADR 0069): only the top-level
                // renderer of `automate` has one. Its lines reach the
                // client only when well-formed, and it receives only the
                // commands this broker admitted for the session's grants.
                RendererMessage::AutomationOutput(line) => {
                    if automation.is_none() || !rustic_devtools::protocol::valid_output(&line) {
                        return Err(refused());
                    }
                    crate::devtools::write_stdout_line(&line);
                }
                RendererMessage::AutomationRead { id } => {
                    let Some(grants) = automation else {
                        return Err(refused());
                    };
                    let line = crate::devtools::next_admitted(grants, &timed_out);
                    channel.send(&BrokerMessage::AutomationCommand { id, line })?;
                }
                RendererMessage::Surface(sent) => {
                    // One surface, or one per frame within the output budget.
                    if context.factor.is_none() || surfaces.len() >= crate::loader::MAX_FRAMES {
                        return Err(refused());
                    }
                    let surface = broker.composite(sent).ok_or_else(refused)?;
                    bytes = bytes.saturating_add(surface.premultiplied().len());
                    if !surfaces.is_empty() && bytes > limits.max_dump_bytes {
                        return Err(refused());
                    }
                    surfaces.push(surface);
                }
                request => {
                    let reply = broker.handle(request);
                    channel.send(&reply)?;
                }
            }
        }
    })();
    let _ = finished.send(());
    let _ = watchdog.join();
    match result {
        Ok(finished) => {
            let _ = child.wait();
            let _ = relay.join();
            Ok(finished)
        }
        Err(IpcError::Closed) if timed_out.load(Ordering::SeqCst) => {
            let _ = child.wait();
            let _ = relay.join();
            Err(Ended::TimedOut(seconds))
        }
        Err(IpcError::Closed) => {
            let status = child.wait();
            let _ = relay.join();
            let status = status.map_or_else(|error| error.to_string(), |status| status.to_string());
            if SANDBOX.load(Ordering::Relaxed) && !handshake_done.load(Ordering::SeqCst) {
                return Err(Ended::SandboxFailed(status));
            }
            Err(Ended::Died(status))
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = relay.join();
            Err(Ended::Broke(error))
        }
    }
}

fn serve(
    command: &str,
    start: Start,
    grant: &Grant,
    limits: LoaderLimits,
    output: Option<&OsString>,
    context: &FrameContext,
) -> u8 {
    // Only `automate`'s own renderer gets the session's command stream.
    let automation = (command == "automate")
        .then(|| limits.devtools.unwrap_or(rustic_devtools::Grants::DEFAULT));
    let finished = match run_renderer(start, grant, limits, context, automation) {
        Ok(finished) => finished,
        Err(Ended::NotStarted(error)) => {
            if SANDBOX.load(Ordering::Relaxed)
                && cfg!(not(any(windows, target_os = "macos")))
                && error.kind() == io::ErrorKind::NotFound
            {
                eprintln!(
                    "rustic {command}: --sandbox needs bubblewrap (bwrap) on PATH; the renderer \
                     was not started without its sandbox"
                );
            } else {
                eprintln!("rustic {command}: cannot start the renderer process: {error}");
            }
            return 1;
        }
        Err(Ended::TimedOut(seconds)) => {
            eprintln!(
                "rustic {command}: the renderer did not finish within {seconds} s and was \
                 stopped; no output was written"
            );
            return 1;
        }
        Err(Ended::SandboxFailed(status)) => {
            eprintln!(
                "rustic {command}: the sandboxed renderer could not start ({status}); nothing \
                 ran unsandboxed"
            );
            return 1;
        }
        Err(Ended::Died(status)) => {
            eprintln!(
                "rustic {command}: the renderer process ended before finishing ({status}); no \
                 output was written"
            );
            return 1;
        }
        Err(Ended::Broke(error)) => {
            eprintln!(
                "rustic {command}: the renderer broke the IPC protocol ({error}) and was stopped"
            );
            return 1;
        }
    };
    if finished.status != 0 {
        // A failing reftest still prints its verdict lines.
        if command == "reftest" && finished.status == 1 {
            // A broken pipe changes nothing: the status is already a failure.
            let _ = write_output(&finished.output);
        }
        return finished.status;
    }
    let mut surfaces = finished.surfaces;
    if surfaces.len() > 1 {
        // `render --frames`: numbered files, then the frame statistics.
        let Some(output) = output else {
            return 1;
        };
        for (index, surface) in surfaces.iter().enumerate() {
            let name = crate::numbered(output, index);
            let written = rustic_raster::encode_png(surface, limits.max_dump_bytes)
                .map_err(|error| error.to_string())
                .and_then(|png| std::fs::write(&name, png).map_err(|error| error.to_string()));
            if let Err(error) = written {
                eprintln!(
                    "rustic {command}: {}: {error}",
                    EscapedText(&Path::new(&name).to_string_lossy())
                );
                return 1;
            }
        }
        return u8::from(write_output(&finished.output).is_err());
    }
    if command == "render" && surfaces.len() == 1 && CAPTURE.with(|slot| slot.borrow().is_some()) {
        let page = surfaces.pop().expect("one surface");
        CAPTURE.with(|slot| *slot.borrow_mut() = Some(Some((page, finished.output))));
        return 0;
    }
    // With an image, the renderer's standard output is its report
    // (`--report-scroll`), printed after the file is written.
    let mut report = Vec::new();
    let bytes = match surfaces.pop() {
        // `render`: this broker encodes the composited page.
        Some(surface) => match rustic_raster::encode_png(&surface, limits.max_dump_bytes) {
            Ok(png) => {
                report = finished.output;
                png
            }
            Err(error) => {
                eprintln!("rustic {command}: {error}");
                return 1;
            }
        },
        None if command == "render" => {
            eprintln!("rustic {command}: the renderer sent no image; no output was written");
            return 1;
        }
        None => finished.output,
    };
    match output {
        Some(path) => {
            if let Err(error) = std::fs::write(path, bytes) {
                eprintln!(
                    "rustic {command}: {}: {error}",
                    EscapedText(&Path::new(path).to_string_lossy())
                );
                return 1;
            }
            // The renderer is untrusted: only well-formed report lines
            // reach the terminal.
            let lines: String = String::from_utf8_lossy(&report)
                .lines()
                .filter(|line| {
                    line.strip_prefix("scroll ").is_some_and(|at| {
                        !at.is_empty()
                            && at
                                .bytes()
                                .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b',' | b'-'))
                    })
                })
                .map(|line| format!("{line}\n"))
                .collect();
            u8::from(write_output(lines.as_bytes()).is_err())
        }
        None => {
            if write_output(&bytes).is_err() {
                return 1;
            }
            0
        }
    }
}

/// Answers a renderer's requests from this broker's grant.
pub(crate) struct Broker<'a> {
    grant: &'a Grant,
    limits: LoaderLimits,
    local: Option<LocalStylesheetFetcher>,
    storage: Option<Option<Rc<dyn StorageHost>>>,
    context: &'a FrameContext,
    /// The composited pixels of the frames this renderer opened, by handle.
    frames: Vec<rustic_raster::Surface>,
}

impl<'a> Broker<'a> {
    fn new(grant: &'a Grant, limits: LoaderLimits, context: &'a FrameContext) -> Self {
        let local = match grant {
            Grant::Local { root } => LocalStylesheetFetcher::new(
                root.clone(),
                limits.read_chunk_bytes,
                CancellationToken::default(),
            ),
            Grant::Network(_) | Grant::Nothing => None,
        };
        Self {
            grant,
            limits,
            local,
            storage: None,
            context,
            frames: Vec::new(),
        }
    }

    /// Check a renderer's surface and composite its frame layers into it.
    /// `None` is a protocol violation: a wrong size, pixels that are not
    /// premultiplied, a layer outside the surface or a handle this broker
    /// never gave out.
    ///
    /// The pixels are composited by the GPU process (ADR 0068), which sees
    /// only the checked layers and the frames they name, renumbered so
    /// handle `n` is the `n`th frame sent; the broker's handles stay here.
    fn composite(&self, surface: rustic_ipc::protocol::Surface) -> Option<rustic_raster::Surface> {
        let page = rustic_raster::Surface::from_premultiplied(
            surface.width,
            surface.height,
            surface.pixels,
            rustic_raster::RasterLimits::default(),
        )?;
        let fits = |start: u32, extent: u32, limit: u32| {
            start.checked_add(extent).is_some_and(|end| end <= limit)
        };
        let mut frames: Vec<&rustic_raster::Surface> = Vec::new();
        let mut handles: Vec<u32> = Vec::new();
        let mut layers = Vec::with_capacity(surface.layers.len());
        for mut layer in surface.layers {
            let frame = self.frames.get(usize::try_from(layer.handle).ok()?)?;
            let count = u64::from(layer.width) * u64::from(layer.height);
            if !fits(layer.x, layer.width, page.width())
                || !fits(layer.y, layer.height, page.height())
                || u64::try_from(layer.coverage.len()).ok()? != count
            {
                return None;
            }
            let index = match handles.iter().position(|&handle| handle == layer.handle) {
                Some(index) => index,
                None => {
                    handles.push(layer.handle);
                    frames.push(frame);
                    handles.len() - 1
                }
            };
            layer.handle = u32::try_from(index).ok()?;
            layers.push(layer);
        }
        crate::gpuproc::composite(&page, &layers, &frames)
    }

    /// Load and render an `iframe` in a renderer of its own (ADR 0062):
    /// decide its document and grant from this document's, render it at
    /// its size and keep its pixels for this renderer's layers.
    fn open_frame(
        &mut self,
        request: rustic_ipc::protocol::FrameRequest,
    ) -> Result<u32, ResourceFailure> {
        let context = self.context;
        let factor = context.factor.ok_or(ResourceFailure::Blocked)?;
        if context.depth >= MAX_FRAME_DEPTH
            || context.budget.get() == 0
            || self.frames.len() >= rustic_ipc::protocol::MAX_FRAMES
            || !(1..=16_384).contains(&request.width)
            || !(1..=16_384).contains(&request.height)
        {
            return Err(ResourceFailure::Blocked);
        }
        context.budget.set(context.budget.get() - 1);
        let device = |css: u32| (f64::from(css) * factor - 1e-6).ceil().max(1.0) as u32;
        let (width, height) = (device(request.width), device(request.height));
        let container = rustic_net::navigation::FrameContainer {
            ancestors: Vec::new(),
            top_level_site: rustic_net::site::PublicSuffixList::bundled().site(&context.origin),
            same_site_ancestors: false,
            sandbox: request.sandbox.clone(),
        };
        let scripts = context.scripts && container.allows_scripts();
        let opaque = container.sandboxes_origin();
        let shown = EscapedText(&request.url).to_string();
        let (grant, document) = match self.frame_document(&request, container) {
            Ok(Some(found)) => found,
            // about:blank: an empty white document needs no renderer.
            Ok(None) => {
                let white = vec![255; (width as usize) * (height as usize) * 4];
                let surface = rustic_raster::Surface::from_premultiplied(
                    width,
                    height,
                    white,
                    rustic_raster::RasterLimits::default(),
                )
                .ok_or(ResourceFailure::TooLarge)?;
                self.frames.push(surface);
                return Ok((self.frames.len() - 1) as u32);
            }
            Err(error) => {
                eprintln!("rustic: frame {shown}: {error}");
                return Err(ResourceFailure::Blocked);
            }
        };
        let document = DocumentGrant {
            storage: document.storage && !opaque,
            ..document
        };
        let child_origin = match &grant {
            Grant::Network(session) => session.origin().clone(),
            _ => rustic_url::Origin::opaque(),
        };
        let mut ancestors = vec![context.origin.clone()];
        ancestors.extend(context.ancestors.iter().cloned());
        let child = FrameContext {
            depth: context.depth + 1,
            budget: Rc::clone(&context.budget),
            factor: Some(factor),
            args: context.args.clone(),
            document: DocumentGrant {
                bytes: Vec::new(),
                ..document.clone()
            },
            origin: child_origin,
            ancestors,
            scripts,
            popups: None,
        };
        let start = Start {
            command: "render".to_owned(),
            args: frame_args(
                &context.args,
                (request.width, request.height),
                factor,
                scripts,
            ),
            document,
            fonts: crate::fonts::granted_bytes().to_vec(),
        };
        // A frame renderer that fails costs only its frame (it paints
        // white); the page renders on.
        let surface = match run_renderer(start, &grant, self.limits, &child, None) {
            Ok(Finished {
                status: 0,
                mut surfaces,
                ..
            }) if surfaces.len() == 1 => surfaces.pop().expect("one surface"),
            Ok(_) => {
                eprintln!("rustic: frame {shown}: the frame did not render");
                return Err(ResourceFailure::Failed);
            }
            Err(ended) => {
                let why = match ended {
                    Ended::NotStarted(error) => format!("cannot start its renderer: {error}"),
                    Ended::TimedOut(seconds) => {
                        format!("its renderer did not finish within {seconds} s")
                    }
                    Ended::SandboxFailed(status) | Ended::Died(status) => {
                        format!("its renderer ended before finishing ({status})")
                    }
                    Ended::Broke(error) => format!("its renderer broke the IPC protocol ({error})"),
                };
                eprintln!("rustic: frame {shown}: {why}");
                return Err(ResourceFailure::Failed);
            }
        };
        if (surface.width(), surface.height()) != (width, height) {
            eprintln!("rustic: frame {shown}: the frame rendered at the wrong size");
            return Err(ResourceFailure::Failed);
        }
        self.frames.push(surface);
        Ok((self.frames.len() - 1) as u32)
    }

    /// Open a popup (ADR 0057 §3): a top-level navigation initiated by this
    /// document, in a renderer of its own with its own grant. The opener
    /// relationship survives only without `noopener` and when COOP needs
    /// no browsing context group switch; the decision and the popup's
    /// process key are reported. The popup's output is not shown (there
    /// is no window to show it in, TD-178).
    fn open_popup(&mut self, url: &str, noopener: bool, noreferrer: bool) -> bool {
        let context = self.context;
        let shown = EscapedText(url).to_string();
        let Some(budget) = context.popups.as_ref().filter(|budget| budget.get() > 0) else {
            eprintln!("rustic: popup {shown}: refused (no popups from here, or too many)");
            return false;
        };
        let Grant::Network(session) = self.grant else {
            eprintln!("rustic: popup {shown}: refused (popups open from network documents only)");
            return false;
        };
        let Ok(url) = Url::parse(url, None) else {
            return false;
        };
        budget.set(budget.get() - 1);
        let navigated = match session.open_popup(&url, noreferrer, self.limits.max_file_bytes) {
            Ok(navigated) => navigated,
            Err(error) => {
                eprintln!("rustic: popup {shown}: {error}");
                return false;
            }
        };
        let popup = Rc::clone(&navigated.session);
        let opener = session.isolation();
        // The popup's initial about:blank inherits the opener's COOP.
        let switch = rustic_net::isolation::requires_group_switch(
            true,
            opener.opener,
            session.origin(),
            popup.isolation().opener,
            popup.origin(),
        );
        let relation = match (noopener, switch) {
            (true, _) => "opener severed (noopener)",
            (false, true) => "opener severed (COOP: new browsing context group)",
            (false, false) => "opener kept (same browsing context group)",
        };
        let mut unique = 0;
        let key = match rustic_net::isolation::process_key(
            popup.document_url(),
            &popup.isolation(),
            &rustic_net::site::PublicSuffixList::bundled(),
            &mut unique,
        ) {
            rustic_net::isolation::ProcessKey::Site(site) => format!("site {site}"),
            rustic_net::isolation::ProcessKey::Origin(origin) => format!("origin {origin}"),
            rustic_net::isolation::ProcessKey::Unique(_) => "unique".to_owned(),
        };
        eprintln!(
            "rustic: popup {}: opened in a renderer of its own ({key}); {relation}",
            EscapedText(&popup.document_url().serialize())
        );
        let document = DocumentGrant {
            url: popup.document_url().serialize(),
            bytes: navigated.bytes,
            transport_charset: navigated.transport_encoding.map(|e| e.name().to_owned()),
            local: false,
            csp: navigated.policy.csp.clone(),
            csp_report_only: navigated.policy.csp_report_only.clone(),
            referrer_policy: navigated.policy.referrer_policy.as_str().to_owned(),
            storage: !popup.origin().is_opaque(),
        };
        let child = FrameContext {
            depth: 0,
            budget: Rc::clone(&context.budget),
            factor: None,
            args: context.args.clone(),
            document: DocumentGrant {
                bytes: Vec::new(),
                ..document.clone()
            },
            origin: popup.origin().clone(),
            ancestors: Vec::new(),
            scripts: context.scripts,
            popups: None,
        };
        let mut args = child_args(&context.args, context.scripts);
        args.extend(["--".to_owned(), "popup".to_owned()]);
        let start = Start {
            command: "dump-dom".to_owned(),
            args,
            document,
            fonts: crate::fonts::granted_bytes().to_vec(),
        };
        match run_renderer(start, &Grant::Network(popup), self.limits, &child, None) {
            Ok(Finished { status: 0, .. }) => true,
            _ => {
                eprintln!("rustic: popup {shown}: its renderer did not finish");
                false
            }
        }
    }

    /// The frame's document and the grant its renderer gets; `None` for
    /// `about:blank`. A local document's frames are files in its
    /// directory (the frame's grant is the frame's own directory) or
    /// `data:`; a network document's are navigations through its session.
    fn frame_document(
        &self,
        request: &rustic_ipc::protocol::FrameRequest,
        mut container: rustic_net::navigation::FrameContainer,
    ) -> Result<Option<(Grant, DocumentGrant)>, String> {
        let context = self.context;
        if request.url == "about:blank" {
            return Ok(None);
        }
        let opaque = container.sandboxes_origin();
        if request.url == "about:srcdoc" {
            let srcdoc = request
                .srcdoc
                .clone()
                .ok_or("about:srcdoc without srcdoc")?;
            // The parent's origin and policies, unless sandboxed.
            let grant = if opaque {
                Grant::Nothing
            } else {
                self.grant.clone()
            };
            return Ok(Some((
                grant,
                DocumentGrant {
                    bytes: srcdoc.into_bytes(),
                    transport_charset: Some("utf-8".to_owned()),
                    ..context.document.clone()
                },
            )));
        }
        let url = Url::parse(&request.url, None).map_err(|_| "invalid frame URL")?;
        let max = self.limits.max_file_bytes;
        match self.grant {
            Grant::Local { root } => {
                if url.scheme() == "data" {
                    let data =
                        rustic_net::data_url::process_data_url(&url).ok_or("invalid data: URL")?;
                    if data.mime.essence() != "text/html" {
                        return Err("a data: frame must be text/html".into());
                    }
                    return Ok(Some((
                        Grant::Nothing,
                        DocumentGrant {
                            url: url.serialize(),
                            bytes: data.body,
                            transport_charset: data.mime.charset().map(|e| e.name().to_owned()),
                            ..DocumentGrant::default()
                        },
                    )));
                }
                let path = crate::reftest::resolve(&request.url, None, Some(root))
                    .map_err(|why| format!("blocked: {why}"))?;
                let (canonical, bytes) = loader::read_granted_file(&path, self.limits)
                    .map_err(|error| error.to_string())?;
                let file = crate::stylesheets::file_url(&canonical).ok_or("not a local file")?;
                let directory = canonical
                    .parent()
                    .ok_or("the frame file has no directory")?
                    .to_owned();
                Ok(Some((
                    Grant::Local { root: directory },
                    DocumentGrant {
                        url: file.serialize(),
                        bytes,
                        local: true,
                        ..DocumentGrant::default()
                    },
                )))
            }
            Grant::Network(session) => {
                let (top_level_site, same_so_far) = session.frame_context();
                let suffixes = rustic_net::site::PublicSuffixList::bundled();
                container.same_site_ancestors =
                    same_so_far && suffixes.site(session.origin()) == top_level_site;
                container.top_level_site = top_level_site;
                container.ancestors = std::iter::once(context.origin.clone())
                    .chain(context.ancestors.iter().cloned())
                    .collect();
                let navigated = session
                    .open_frame(&url, container, max)
                    .map_err(|error| error.to_string())?;
                let frame = Rc::clone(&navigated.session);
                let document = DocumentGrant {
                    url: frame.document_url().serialize(),
                    bytes: navigated.bytes,
                    transport_charset: navigated.transport_encoding.map(|e| e.name().to_owned()),
                    local: false,
                    csp: navigated.policy.csp.clone(),
                    csp_report_only: navigated.policy.csp_report_only.clone(),
                    referrer_policy: navigated.policy.referrer_policy.as_str().to_owned(),
                    storage: !frame.origin().is_opaque(),
                };
                Ok(Some((Grant::Network(frame), document)))
            }
            Grant::Nothing => Err("this document cannot load frames".into()),
        }
    }

    /// The largest body any resource of this command may have.
    fn cap(&self) -> u64 {
        let limits = self.limits;
        limits
            .max_file_bytes
            .max(limits.stylesheets.max_total_bytes)
            .max(limits.images.max_file_bytes) as u64
    }

    fn handle(&mut self, request: RendererMessage) -> BrokerMessage {
        match request {
            RendererMessage::Resource {
                id,
                url,
                kind,
                max_bytes,
            } => BrokerMessage::Resource {
                id,
                result: self.resource(&url, kind, max_bytes),
            },
            RendererMessage::Fetch { id, request } => BrokerMessage::Fetch {
                id,
                result: self.fetch(request),
            },
            RendererMessage::Storage { id, area, op } => BrokerMessage::Storage {
                id,
                value: self.storage(area, op),
            },
            RendererMessage::Frame { id, request } => BrokerMessage::Frame {
                id,
                result: self.open_frame(request),
            },
            RendererMessage::Popup {
                id,
                url,
                noopener,
                noreferrer,
            } => BrokerMessage::Popup {
                id,
                opened: self.open_popup(&url, noopener || noreferrer, noreferrer),
            },
            // The loop handles Diagnostic, Surface and Done; anything else
            // is a request this broker refuses.
            RendererMessage::Diagnostic(_)
            | RendererMessage::Surface(_)
            | RendererMessage::Painted(_)
            | RendererMessage::Handled
            | RendererMessage::Accessibility(_)
            | RendererMessage::AutomationOutput(_)
            | RendererMessage::AutomationRead { .. }
            | RendererMessage::Done { .. } => BrokerMessage::Storage {
                id: u32::MAX,
                value: StorageValue::Refused,
            },
        }
    }

    fn resource(
        &self,
        url: &str,
        kind: rustic_ipc::protocol::ResourceKind,
        max_bytes: u64,
    ) -> Result<ResourceBody, ResourceFailure> {
        let Ok(url) = Url::parse(url, None) else {
            return Err(ResourceFailure::Blocked);
        };
        // The renderer's budget never raises this broker's own.
        let max = usize::try_from(max_bytes.min(self.cap())).unwrap_or(usize::MAX);
        let kind = match kind {
            rustic_ipc::protocol::ResourceKind::Stylesheet => ResourceKind::Stylesheet,
            rustic_ipc::protocol::ResourceKind::Image => ResourceKind::Image,
            rustic_ipc::protocol::ResourceKind::Script {
                nonce,
                parser_inserted,
                module,
            } => ResourceKind::Script {
                nonce,
                parser_inserted,
                module,
            },
        };
        let result = match self.grant {
            // Only files inside the document's directory, after symbolic
            // links resolve: the same rule a single-process run applies.
            Grant::Local { .. } => match &self.local {
                Some(fetcher) => fetcher.fetch_bytes(&url, max),
                None => Err(FetchError::Blocked),
            },
            Grant::Network(session) => session.fetch(&url, &kind, max),
            Grant::Nothing => Err(FetchError::Blocked),
        };
        let charset = match self.grant {
            Grant::Network(session) => session.charset_of(&url),
            Grant::Local { .. } | Grant::Nothing => None,
        };
        result
            .map(|bytes| ResourceBody { bytes, charset })
            .map_err(|error| match error {
                FetchError::Blocked => ResourceFailure::Blocked,
                FetchError::NotFound => ResourceFailure::NotFound,
                FetchError::TooLarge => ResourceFailure::TooLarge,
                FetchError::Failed => ResourceFailure::Failed,
                FetchError::Cancelled => ResourceFailure::Cancelled,
            })
    }

    fn fetch(&self, request: rustic_ipc::protocol::FetchRequest) -> Option<FetchResponse> {
        // Local documents have no network.
        let Grant::Network(session) = self.grant else {
            return None;
        };
        let url = Url::parse(&request.url, None).ok()?;
        let response = session
            .script_fetch(rustic_engine::ScriptRequest {
                method: request.method,
                url,
                headers: request.headers,
                body: request.body,
                mode: request.mode,
                credentials: request.credentials,
                redirect: request.redirect,
                referrer_policy: request.referrer_policy,
            })
            .ok()?;
        Some(FetchResponse {
            kind: response.kind.to_owned(),
            url: response.url.map(|url| url.serialize()),
            redirected: response.redirected,
            status: response.status,
            status_text: response.status_text,
            headers: response.headers,
            body: response.body,
        })
    }

    fn storage(&mut self, area: StorageArea, op: StorageOp) -> StorageValue {
        let Grant::Network(session) = self.grant else {
            return StorageValue::Refused;
        };
        if self.storage.is_none() {
            self.storage = Some(session.storage_host().ok().flatten());
        }
        let Some(Some(host)) = &self.storage else {
            return StorageValue::Refused;
        };
        let kind = match area {
            StorageArea::Local => StorageKind::Local,
            StorageArea::Session => StorageKind::Session,
        };
        match op {
            StorageOp::Length => StorageValue::Count(host.length(kind)),
            StorageOp::Key(index) => StorageValue::Units(host.key(kind, index)),
            StorageOp::Get(key) => StorageValue::Units(host.get(kind, &key)),
            StorageOp::Set(key, value) => match host.set(kind, &key, &value) {
                Ok(()) => StorageValue::Done,
                Err(_) => StorageValue::QuotaExceeded,
            },
            StorageOp::Remove(key) => {
                host.remove(kind, &key);
                StorageValue::Done
            }
            StorageOp::Clear => {
                host.clear(kind);
                StorageValue::Done
            }
            StorageOp::Keys => {
                let mut keys = host.keys(kind);
                keys.truncate(rustic_ipc::protocol::MAX_STORAGE_KEYS);
                StorageValue::Keys(keys)
            }
        }
    }
}

/// Stop a renderer by process id from the watchdog thread (the broker's
/// thread owns its `Child`). The `kill` utility needs no unsafe code.
fn kill_process(pid: u32) {
    if cfg!(unix) {
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    } else {
        let _ = Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
