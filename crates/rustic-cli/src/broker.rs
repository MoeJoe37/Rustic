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
    BrokerMessage, DocumentGrant, FetchResponse, RendererMessage, ResourceBody, ResourceFailure,
    Start, StorageArea, StorageOp, StorageValue,
};
use rustic_ipc::{Channel, IpcError, Limits, Role};
use rustic_style::FetchError;
use rustic_url::Url;

use crate::loader::{self, LoadError, LoaderLimits};
use crate::network::{NetworkSession, ResourceKind, Session};
use crate::stylesheets::LocalStylesheetFetcher;

static SINGLE_PROCESS: AtomicBool = AtomicBool::new(false);
/// Whether renderers are confined. On by default where Rustic has a
/// sandbox (Linux, Windows); `--no-sandbox` or `RUSTIC_SANDBOX=0` turns it
/// off, `--sandbox` or `RUSTIC_SANDBOX=1` demands it (an error where none
/// exists).
static SANDBOX: AtomicBool = AtomicBool::new(false);

/// Whether renderers (and the network process) are confined in this run.
/// (Windows does not confine the network process yet, TD-176.)
#[cfg(not(windows))]
pub(crate) fn sandboxed() -> bool {
    SANDBOX.load(Ordering::Relaxed)
}

/// Whether this platform has a renderer sandbox (ADR 0058, ADR 0060).
pub const SANDBOX_AVAILABLE: bool = cfg!(any(target_os = "linux", windows));
static TIMEOUT_SECONDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(120);

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
    "RUSTIC_TEST_ESCAPE_ABSTRACT",
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
                | "render"
                | "hit-test"
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

/// Run `command` with its document in a renderer. Returns the exit status.
pub fn run(command: &str, args: &[OsString]) -> u8 {
    let Some((limits, path, output)) = document_argument(command, args) else {
        // A usage error: let the command report it; nothing is loaded.
        let mut engine = rustic_engine::Engine::new(1);
        let mut all = vec![OsString::from(command)];
        all.extend_from_slice(args);
        return crate::run_command(&mut engine, &all).unwrap_or(2);
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
                "rustic {command}: network URLs need --allow-origin ORIGIN=ADDRESS for each \
                 origin; nothing was sent"
            );
            return 2;
        }
        let url = path.to_str().unwrap_or_default();
        let navigated = match crate::network::navigate(
            url,
            limits.max_file_bytes,
            CancellationToken::default(),
        ) {
            Ok(navigated) => navigated,
            Err(error) => return fail(LoadError::Network(error)),
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
                return 2;
            }
        };
        let (canonical, bytes) = match loader::read_granted_file(&local, limits) {
            Ok(read) => read,
            Err(error) => return fail(error),
        };
        let Some(url) = crate::stylesheets::file_url(&canonical) else {
            return fail(LoadError::NotLocalPath);
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
    let mut renderer_args: Vec<String> = Vec::new();
    if let Some(locale) = crate::network::configured_locale() {
        renderer_args.extend(["--locale".to_owned(), locale.to_owned()]);
    }
    renderer_args.extend(args.iter().map(|arg| arg.to_string_lossy().into_owned()));
    let factor = (command == "render")
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
    let status = serve(command, start, &grant, limits, output.as_ref(), &context);
    if let Grant::Network(session) = &grant {
        if let Err(error) = session.finish() {
            eprintln!("rustic {command}: profile: {error}");
            return 1;
        }
    }
    status
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
            let (limits, path) = crate::parse_dump_args(args).ok()?;
            Some((limits, path, None))
        }
    }
}

/// A renderer process: unconfined, or in the Windows AppContainer.
enum Process {
    Child(Child),
    /// A bubblewrap renderer and its filter file, which lives until the
    /// launcher has opened (and removed) it or the process is gone.
    #[cfg(not(windows))]
    Sandboxed {
        child: Child,
        _filter: SeccompFile,
    },
    #[cfg(windows)]
    Confined(rustic_sandbox::Confined),
}

impl Process {
    fn id(&self) -> u32 {
        match self {
            Self::Child(child) => child.id(),
            #[cfg(not(windows))]
            Self::Sandboxed { child, .. } => child.id(),
            #[cfg(windows)]
            Self::Confined(confined) => confined.id(),
        }
    }

    fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        match self {
            Self::Child(child) => child.wait(),
            #[cfg(not(windows))]
            Self::Sandboxed { child, .. } => child.wait(),
            #[cfg(windows)]
            Self::Confined(confined) => confined.wait(),
        }
    }

    fn kill(&mut self) -> io::Result<()> {
        match self {
            Self::Child(child) => child.kill(),
            #[cfg(not(windows))]
            Self::Sandboxed { child, .. } => child.kill(),
            #[cfg(windows)]
            Self::Confined(confined) => confined.kill(),
        }
    }
}

/// The renderer's standard error, whichever way it was started.
pub(crate) enum RendererStderr {
    Pipe(std::process::ChildStderr),
    #[cfg(windows)]
    File(std::fs::File),
}

impl Read for RendererStderr {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Pipe(pipe) => pipe.read(buffer),
            #[cfg(windows)]
            Self::File(file) => file.read(buffer),
        }
    }
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
fn sandbox_env() -> Vec<(OsString, OsString)> {
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
    let (confined, pipes) = rustic_sandbox::spawn_app_container(exe, &[command], &sandbox_env())?;
    Ok((
        Process::Confined(confined),
        Box::new(pipes.stdout),
        Box::new(pipes.stdin),
        RendererStderr::File(pipes.stderr),
    ))
}

/// Linux: the executable inside bubblewrap with every namespace unshared
/// (no network), a filesystem of the read-only system libraries and the
/// executable only, no capabilities, a new session (no terminal
/// injection), a cleared environment, death with the broker, and the
/// seccomp-bpf filter of `rustic_sandbox::seccomp` (ADR 0058). `/bin/sh`
/// opens the filter as descriptor 3 for `bwrap --seccomp` (std cannot pass
/// a descriptor without unsafe code) and removes the file first.
#[cfg(not(windows))]
fn spawn_confined(exe: &Path) -> io::Result<Started> {
    let (child, filter, stdout, stdin, stderr) =
        spawn_bubblewrap(exe, crate::renderer::RENDERER_COMMAND, false)?;
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
pub(crate) fn spawn_bubblewrap(
    exe: &Path,
    command_name: &str,
    network: bool,
) -> io::Result<Bubblewrapped> {
    if !cfg!(target_os = "linux") {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "--sandbox is implemented on Linux and Windows only (macOS: TD-174)",
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
    let program = if network {
        rustic_sandbox::seccomp::network_filter(arch)
    } else {
        rustic_sandbox::seccomp::renderer_filter(arch)
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
    /// `render`: the page with its frames composited.
    surface: Option<rustic_raster::Surface>,
}

/// Run one renderer to the end: start it (confined when the sandbox is
/// on), arm the watchdog, send `start` and answer its requests from
/// `grant` until it is done.
fn run_renderer(
    start: Start,
    grant: &Grant,
    limits: LoaderLimits,
    context: &FrameContext,
) -> Result<Finished, Ended> {
    let (mut child, stdout, stdin, relay) = spawn().map_err(Ended::NotStarted)?;
    // Watchdog: a renderer that does not finish in time is killed, and the
    // pipe closing ends the wait below (security.recovery).
    let seconds = TIMEOUT_SECONDS.load(Ordering::Relaxed);
    let pid = child.id();
    let (finished, watch) = std::sync::mpsc::channel::<()>();
    let timed_out = std::sync::Arc::new(AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&timed_out);
    let watchdog = std::thread::spawn(move || {
        if watch.recv_timeout(std::time::Duration::from_secs(seconds))
            == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        {
            flag.store(true, Ordering::SeqCst);
            kill_process(pid);
        }
    });
    let handshake_done = AtomicBool::new(false);
    let result = (|| -> Result<Finished, IpcError> {
        let mut channel = Channel::connect(stdout, stdin, Role::Broker, Limits::default())?;
        handshake_done.store(true, Ordering::SeqCst);
        channel.send(&BrokerMessage::Start(start))?;
        let mut broker = Broker::new(grant, limits, context);
        let mut surface = None;
        loop {
            match channel.recv::<RendererMessage>()? {
                RendererMessage::Done { status, output } => {
                    return Ok(Finished {
                        status,
                        output,
                        surface,
                    });
                }
                RendererMessage::Diagnostic(text) => eprintln!("{}", EscapedText(&text)),
                RendererMessage::Surface(sent) => {
                    if surface.is_some() || context.factor.is_none() {
                        return Err(IpcError::Malformed(rustic_ipc::DecodeError::OutOfRange));
                    }
                    surface = Some(
                        broker
                            .composite(sent)
                            .ok_or(IpcError::Malformed(rustic_ipc::DecodeError::OutOfRange))?,
                    );
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
    let finished = match run_renderer(start, grant, limits, context) {
        Ok(finished) => finished,
        Err(Ended::NotStarted(error)) => {
            if SANDBOX.load(Ordering::Relaxed) && error.kind() == io::ErrorKind::NotFound {
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
        return finished.status;
    }
    let bytes = match finished.surface {
        // `render`: this broker encodes the composited page.
        Some(surface) => match rustic_raster::encode_png(&surface, limits.max_dump_bytes) {
            Ok(png) => png,
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
            0
        }
        None => {
            let mut stdout = io::stdout().lock();
            if stdout
                .write_all(&bytes)
                .and_then(|()| stdout.flush())
                .is_err()
            {
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
    fn composite(&self, surface: rustic_ipc::protocol::Surface) -> Option<rustic_raster::Surface> {
        let mut page = rustic_raster::Surface::from_premultiplied(
            surface.width,
            surface.height,
            surface.pixels,
            rustic_raster::RasterLimits::default(),
        )?;
        for layer in &surface.layers {
            let frame = self.frames.get(usize::try_from(layer.handle).ok()?)?;
            page.add_frame(
                frame,
                (layer.x, layer.y, layer.width, layer.height),
                &layer.coverage,
            )
            .ok()?;
        }
        Some(page)
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
        let surface = match run_renderer(start, &grant, self.limits, &child) {
            Ok(Finished {
                status: 0,
                surface: Some(surface),
                ..
            }) => surface,
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
        match run_renderer(start, &Grant::Network(popup), self.limits, &child) {
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
