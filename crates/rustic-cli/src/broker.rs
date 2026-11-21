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
enum Grant {
    Local { root: PathBuf },
    Network(Rc<NetworkSession>),
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
    let start = Start {
        command: command.to_owned(),
        args: renderer_args,
        document,
        fonts: crate::fonts::granted_bytes().to_vec(),
    };
    let status = serve(command, start, &grant, limits, output.as_ref());
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
enum RendererStderr {
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
    let filter = SeccompFile::create(&rustic_sandbox::seccomp::to_bytes(
        &rustic_sandbox::seccomp::renderer_filter(arch),
    ))?;
    let mut command = Command::new("/bin/sh");
    command.args([
        "-c",
        "exec 3<\"$0\" && rm -f -- \"$0\" && exec bwrap --seccomp 3 \"$@\"",
    ]);
    command.arg(&filter.0);
    command.args([
        "--unshare-all",
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
    for (name, value) in sandbox_env() {
        command.arg("--setenv").arg(name).arg(value);
    }
    command.args(["/renderer", crate::renderer::RENDERER_COMMAND]);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let (stdout, stdin, stderr) = pipes(&mut child);
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

/// The filter program in a new private file, removed on drop unless the
/// launcher has removed it already.
#[cfg(not(windows))]
struct SeccompFile(PathBuf);

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

fn serve(
    command: &str,
    start: Start,
    grant: &Grant,
    limits: LoaderLimits,
    output: Option<&OsString>,
) -> u8 {
    let (mut child, stdout, stdin, relay) = match spawn() {
        Ok(spawned) => spawned,
        Err(error) => {
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
    };
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
    let result = (|| -> Result<(u8, Vec<u8>), IpcError> {
        let mut channel = Channel::connect(stdout, stdin, Role::Broker, Limits::default())?;
        handshake_done.store(true, Ordering::SeqCst);
        channel.send(&BrokerMessage::Start(start))?;
        let mut broker = Broker::new(grant, limits);
        loop {
            match channel.recv::<RendererMessage>()? {
                RendererMessage::Done { status, output } => return Ok((status, output)),
                RendererMessage::Diagnostic(text) => eprintln!("{}", EscapedText(&text)),
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
        Ok((status, bytes)) => {
            let _ = child.wait();
            let _ = relay.join();
            if status != 0 {
                return status;
            }
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
        Err(IpcError::Closed) if timed_out.load(Ordering::SeqCst) => {
            let _ = child.wait();
            let _ = relay.join();
            eprintln!(
                "rustic {command}: the renderer did not finish within {seconds} s and was \
                 stopped; no output was written"
            );
            1
        }
        Err(IpcError::Closed) => {
            let status = child.wait();
            let _ = relay.join();
            if SANDBOX.load(Ordering::Relaxed) && !handshake_done.load(Ordering::SeqCst) {
                eprintln!(
                    "rustic {command}: the sandboxed renderer could not start ({}); nothing \
                     ran unsandboxed",
                    status.map_or_else(|error| error.to_string(), |status| status.to_string())
                );
                return 1;
            }
            eprintln!(
                "rustic {command}: the renderer process ended before finishing ({}); no output \
                 was written",
                status.map_or_else(|error| error.to_string(), |status| status.to_string())
            );
            1
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = relay.join();
            eprintln!(
                "rustic {command}: the renderer broke the IPC protocol ({error}) and was stopped"
            );
            1
        }
    }
}

/// Answers a renderer's requests from this broker's grant.
pub(crate) struct Broker<'a> {
    grant: &'a Grant,
    limits: LoaderLimits,
    local: Option<LocalStylesheetFetcher>,
    storage: Option<Option<Rc<dyn StorageHost>>>,
}

impl<'a> Broker<'a> {
    fn new(grant: &'a Grant, limits: LoaderLimits) -> Self {
        let local = match grant {
            Grant::Local { root } => LocalStylesheetFetcher::new(
                root.clone(),
                limits.read_chunk_bytes,
                CancellationToken::default(),
            ),
            Grant::Network(_) => None,
        };
        Self {
            grant,
            limits,
            local,
            storage: None,
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
            // The loop handles Diagnostic and Done; anything else is a
            // request this broker refuses.
            RendererMessage::Diagnostic(_) | RendererMessage::Done { .. } => {
                BrokerMessage::Storage {
                    id: u32::MAX,
                    value: StorageValue::Refused,
                }
            }
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
        };
        let charset = match self.grant {
            Grant::Network(session) => session.charset_of(&url),
            Grant::Local { .. } => None,
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
