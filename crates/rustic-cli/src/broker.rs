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
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
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

/// Remove `--single-process` from `args`.
pub fn configure(args: Vec<OsString>) -> Result<Vec<OsString>, String> {
    let mut output = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            output.push(arg);
            output.extend(args);
            break;
        }
        if arg == "--single-process" {
            SINGLE_PROCESS.store(true, Ordering::Relaxed);
        } else {
            output.push(arg);
        }
    }
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

fn spawn() -> io::Result<(Child, ChildStdout, ChildStdin, std::thread::JoinHandle<()>)> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg(crate::renderer::RENDERER_COMMAND)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
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
    Ok((child, stdout, stdin, relay))
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
            eprintln!("rustic {command}: cannot start the renderer process: {error}");
            return 1;
        }
    };
    let result = (|| -> Result<(u8, Vec<u8>), IpcError> {
        let mut channel = Channel::connect(stdout, stdin, Role::Broker, Limits::default())?;
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
        Err(IpcError::Closed) => {
            let status = child.wait();
            let _ = relay.join();
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
