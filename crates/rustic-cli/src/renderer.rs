//! The renderer process (ISO-02, ADR 0056).
//!
//! `rustic __renderer` is started by the broker with its standard input
//! and output as the IPC channel. It receives one `Start` with the command
//! and the granted document bytes, runs the same command code as a
//! single-process run, and asks the broker for every subresource,
//! `fetch()` and Web Storage operation. It opens no files and no sockets
//! itself; its standard error is relayed (escaped) by the broker.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, StdinLock, StdoutLock, Write};
use std::path::Path;
use std::rc::Rc;

use rustic_engine::{Engine, QuotaExceeded, StorageHost, StorageKind};
use rustic_ipc::protocol::{
    BrokerMessage, FetchRequest, FrameRequest, RendererMessage, ResourceFailure, StorageArea,
    StorageOp, StorageValue, Surface,
};
use rustic_ipc::{Channel, IpcError, Limits, Role};
use rustic_net::csp::CspList;
use rustic_net::navigation::PolicyContainer;
use rustic_net::referrer::ReferrerPolicy;
use rustic_style::FetchError;
use rustic_url::Url;

use crate::network::{ResourceKind, Session, SessionDocument};

/// The hidden subcommand that runs a renderer.
pub const RENDERER_COMMAND: &str = "__renderer";

type Link = Channel<StdinLock<'static>, StdoutLock<'static>>;

thread_local! {
    /// Command output, collected instead of written (renderer mode only).
    static OUTPUT: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
    /// The document the broker granted, consumed by the command's load.
    static GRANTED: RefCell<Option<SessionDocument>> = const { RefCell::new(None) };
    /// The broker, which loads this document's frames (renderer mode only).
    static FRAMES: RefCell<Option<Rc<BrokerLink>>> = const { RefCell::new(None) };
    /// The rendered surface, sent before `Done` (`render` only).
    static SURFACE: RefCell<Option<Surface>> = const { RefCell::new(None) };
}

/// Where `render` sends frame requests: the broker, in a renderer.
pub(crate) fn frame_host() -> Option<Rc<dyn crate::loader::FrameHost>> {
    FRAMES.with(|frames| {
        frames
            .borrow()
            .as_ref()
            .map(|link| Rc::clone(link) as Rc<dyn crate::loader::FrameHost>)
    })
}

/// Hand the rendered surface to the broker, which composites the frames
/// and writes the PNG.
pub(crate) fn emit_surface(surface: Surface) {
    SURFACE.with(|slot| *slot.borrow_mut() = Some(surface));
}

impl crate::loader::FrameHost for BrokerLink {
    fn open(&self, request: FrameRequest) -> Option<u32> {
        match self
            .request(|id| RendererMessage::Frame { id, request })
            .unwrap_or_else(|error| lost(&error))
        {
            BrokerMessage::Frame {
                result: Ok(handle), ..
            } => Some(handle),
            _ => None,
        }
    }
}

/// Write command output: to standard output, or, in a renderer, to the
/// buffer the broker receives.
pub fn emit_stdout(bytes: &[u8]) -> io::Result<()> {
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

/// Write an output file (`render --output`); a renderer hands the bytes to
/// the broker, which owns the file system.
pub fn emit_file(path: &OsString, bytes: Vec<u8>) -> io::Result<()> {
    let collected = OUTPUT.with(|output| {
        output
            .borrow_mut()
            .as_mut()
            .map(|buffer| buffer.extend_from_slice(&bytes))
            .is_some()
    });
    if collected {
        return Ok(());
    }
    std::fs::write(Path::new(path), bytes)
}

/// The granted document, once (renderer mode only).
pub fn take_granted() -> Option<SessionDocument> {
    GRANTED.with(|granted| granted.borrow_mut().take())
}

/// Run as a renderer. Returns the process exit status.
pub fn run() -> u8 {
    let stdin = io::stdin().lock();
    let stdout = io::stdout().lock();
    let mut channel = match Channel::connect(stdin, stdout, Role::Renderer, Limits::default()) {
        Ok(channel) => channel,
        Err(error) => {
            eprintln!("rustic renderer: {error}");
            return 1;
        }
    };
    let start = match channel.recv::<BrokerMessage>() {
        Ok(BrokerMessage::Start(start)) => start,
        Ok(_) => {
            eprintln!("rustic renderer: the broker did not start with a job");
            return 1;
        }
        Err(error) => {
            eprintln!("rustic renderer: {error}");
            return 1;
        }
    };
    // Test hooks. They only change what this untrusted process does, which
    // the broker must survive anyway; they grant nothing.
    if std::env::var_os("RUSTIC_TEST_RENDERER_CRASH").is_some() {
        panic!("simulated renderer crash");
    }
    if std::env::var_os("RUSTIC_TEST_RENDERER_HANG").is_some() {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
    if let Some(mode) = std::env::var_os("RUSTIC_TEST_COMPROMISED_RENDERER") {
        return compromised(channel, &start.document.url, &mode);
    }
    // A fault in only the renderers whose document URL contains
    // RUSTIC_TEST_FAULT_URL (a frame's, ADR 0062).
    let fault = std::env::var("RUSTIC_TEST_FAULT_URL")
        .ok()
        .filter(|part| start.document.url.contains(part.as_str()))
        .and_then(|_| std::env::var("RUSTIC_TEST_FAULT").ok());
    match fault.as_deref() {
        Some("crash") => panic!("simulated frame renderer crash"),
        Some("probe") => probe(&mut channel, &start.document.url),
        _ => {}
    }
    if let Err(error) = crate::fonts::configure_bytes(start.fonts) {
        eprintln!("rustic renderer: fonts: {error}");
        return 1;
    }
    let args: Vec<OsString> = start.args.into_iter().map(OsString::from).collect();
    // Only `--locale` remains of the run-wide options.
    let args = match crate::network::configure(args) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("rustic renderer: {error}");
            return 2;
        }
    };
    let document = start.document;
    let Ok(url) = Url::parse(&document.url, None) else {
        eprintln!("rustic renderer: the granted document URL is invalid");
        return 1;
    };
    let policy = PolicyContainer {
        referrer_policy: ReferrerPolicy::from_token(&document.referrer_policy).unwrap_or_default(),
        csp: document.csp,
        csp_report_only: document.csp_report_only,
    };
    let link = Rc::new(BrokerLink {
        channel: RefCell::new(channel),
        next_id: Cell::new(0),
    });
    let session = Rc::new(BrokeredSession {
        link: Rc::clone(&link),
        url,
        csp: CspList::from_container(&policy),
        local: document.local,
        storage: document.storage,
        charsets: RefCell::new(HashMap::new()),
    });
    let transport_encoding = document
        .transport_charset
        .as_deref()
        .and_then(rustic_encoding::Encoding::for_label);
    GRANTED.with(|granted| {
        *granted.borrow_mut() = Some(SessionDocument {
            session,
            bytes: document.bytes,
            transport_encoding,
        })
    });
    OUTPUT.with(|output| *output.borrow_mut() = Some(Vec::new()));
    FRAMES.with(|frames| *frames.borrow_mut() = Some(Rc::clone(&link)));
    let mut engine = Engine::new(1);
    let command = std::iter::once(OsString::from(&start.command))
        .chain(args)
        .collect::<Vec<_>>();
    let status = crate::run_command(&mut engine, &command).unwrap_or(2);
    let output = OUTPUT.with(|output| output.borrow_mut().take().unwrap_or_default());
    FRAMES.with(|frames| frames.borrow_mut().take());
    if let Some(mut surface) = SURFACE.with(|slot| slot.borrow_mut().take()) {
        if fault.as_deref() == Some("forge") {
            // Claim a frame this renderer was never given.
            surface.layers.push(rustic_ipc::protocol::FrameLayer {
                handle: 99,
                width: 1,
                height: 1,
                coverage: vec![255],
                ..rustic_ipc::protocol::FrameLayer::default()
            });
        }
        if let Err(error) = link
            .channel
            .borrow_mut()
            .send(&RendererMessage::Surface(surface))
        {
            eprintln!("rustic renderer: {error}");
            return 1;
        }
    }
    let done = RendererMessage::Done { status, output };
    if let Err(error) = link.channel.borrow_mut().send(&done) {
        eprintln!("rustic renderer: {error}");
        return 1;
    }
    0
}

/// A compromised frame renderer's probe: ask for its parent's file and
/// another site's resource, and report the answers as diagnostics.
fn probe(channel: &mut Link, document: &str) {
    let base = Url::parse(document, None).ok();
    let parent = base
        .as_ref()
        .and_then(|base| Url::parse("../inside.txt", Some(base)).ok())
        .map_or_else(String::new, |url| url.serialize());
    for (index, (label, url)) in [
        ("parent-file", parent),
        ("other-site", "http://a.test/".into()),
    ]
    .into_iter()
    .enumerate()
    {
        let asked = channel.send(&RendererMessage::Resource {
            id: 1_000 + index as u32,
            url,
            kind: rustic_ipc::protocol::ResourceKind::Image,
            max_bytes: u64::MAX,
        });
        let answer = match asked.and_then(|()| channel.recv::<BrokerMessage>()) {
            Ok(BrokerMessage::Resource {
                result: Ok(body), ..
            }) => format!("read {} bytes", body.bytes.len()),
            Ok(BrokerMessage::Resource {
                result: Err(error), ..
            }) => format!("{error:?}"),
            _ => "unexpected reply".to_owned(),
        };
        let _ = channel.send(&RendererMessage::Diagnostic(format!(
            "frame probe {label}: {answer}"
        )));
    }
}

/// Behave like a compromised renderer: ask for what the grant forbids and
/// report the broker's answers as the command output. `garbage` instead
/// sends a frame the protocol does not have.
fn compromised(mut channel: Link, document: &str, mode: &std::ffi::OsStr) -> u8 {
    if mode == "escape" {
        return escape(channel);
    }
    if mode == "garbage" {
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(&[3, 0, 0, 0, 0xEE, 0xEE, 0xEE]);
        let _ = stdout.flush();
        std::thread::sleep(std::time::Duration::from_secs(5));
        return 1;
    }
    let base = Url::parse(document, None).ok();
    let sibling = |path: &str| {
        base.as_ref()
            .and_then(|base| Url::parse(path, Some(base)).ok())
            .map_or_else(String::new, |url| url.serialize())
    };
    let requests = [
        ("system-file", "file:///etc/passwd".to_owned()),
        ("parent-directory", sibling("../outside.txt")),
        ("sibling", sibling("inside.txt")),
        ("unlisted-origin", "http://c.test/secret".to_owned()),
        ("other-scheme", "ftp://a.test/x".to_owned()),
    ];
    let mut report = String::new();
    for (index, (label, url)) in requests.iter().enumerate() {
        let sent = channel.send(&RendererMessage::Resource {
            id: index as u32,
            url: url.clone(),
            kind: rustic_ipc::protocol::ResourceKind::Image,
            max_bytes: u64::MAX,
        });
        let answer = match sent.and_then(|()| channel.recv::<BrokerMessage>()) {
            Ok(BrokerMessage::Resource {
                result: Ok(body), ..
            }) => {
                format!("read {} bytes", body.bytes.len())
            }
            Ok(BrokerMessage::Resource {
                result: Err(error), ..
            }) => format!("{error:?}"),
            Ok(_) => "unexpected reply".to_owned(),
            Err(error) => format!("channel: {error}"),
        };
        report.push_str(&format!("{label}: {answer}\n"));
    }
    let fetch = RendererMessage::Fetch {
        id: 100,
        request: FetchRequest {
            method: "GET".into(),
            url: "http://c.test/secret".into(),
            mode: "no-cors".into(),
            credentials: "include".into(),
            redirect: "follow".into(),
            ..FetchRequest::default()
        },
    };
    let answer = match channel
        .send(&fetch)
        .and_then(|()| channel.recv::<BrokerMessage>())
    {
        Ok(BrokerMessage::Fetch { result: None, .. }) => "network error".to_owned(),
        Ok(BrokerMessage::Fetch {
            result: Some(response),
            ..
        }) => {
            format!("status {}", response.status)
        }
        _ => "unexpected reply".to_owned(),
    };
    report.push_str(&format!("fetch-unlisted-origin: {answer}\n"));
    let storage = RendererMessage::Storage {
        id: 101,
        area: StorageArea::Local,
        op: StorageOp::Keys,
    };
    let answer = match channel
        .send(&storage)
        .and_then(|()| channel.recv::<BrokerMessage>())
    {
        Ok(BrokerMessage::Storage { value, .. }) => format!("{value:?}"),
        _ => "unexpected reply".to_owned(),
    };
    report.push_str(&format!("storage: {answer}\n"));
    let done = RendererMessage::Done {
        status: 0,
        output: report.into_bytes(),
    };
    match channel.send(&done) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// Try to act directly on the operating system, as code running in a
/// compromised renderer would, and report what worked. Under `--sandbox`
/// every attempt must fail.
fn escape(mut channel: Link) -> u8 {
    let mut report = String::new();
    let mut attempt = |label: &str, result: Result<String, String>| {
        report.push_str(&match result {
            Ok(detail) => format!("{label}: allowed ({detail})\n"),
            Err(error) => format!("{label}: denied ({error})\n"),
        });
    };
    // Targets come from the test (the sandbox clears the environment
    // except these hooks); the defaults are the usual Unix ones.
    let target = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    attempt(
        "read-user-file",
        std::fs::read(target("RUSTIC_TEST_ESCAPE_FILE", "/etc/passwd"))
            .map(|bytes| format!("{} bytes", bytes.len()))
            .map_err(|error| error.kind().to_string()),
    );
    let home = target(
        "RUSTIC_TEST_ESCAPE_HOME",
        &std::env::var("HOME").unwrap_or_else(|_| "/home".to_owned()),
    );
    attempt(
        "list-home",
        std::fs::read_dir(&home)
            .map(|entries| format!("{} entries", entries.count()))
            .map_err(|error| error.kind().to_string()),
    );
    let probe = std::path::Path::new(&target("RUSTIC_TEST_ESCAPE_DIR", "/tmp"))
        .join(format!("rustic-escape-probe-{}", std::process::id()));
    attempt(
        "write-temp",
        std::fs::write(&probe, b"x")
            .map(|()| {
                let _ = std::fs::remove_file(&probe);
                "wrote".to_owned()
            })
            .map_err(|error| error.kind().to_string()),
    );
    let address = std::env::var("RUSTIC_TEST_ESCAPE_ADDRESS").unwrap_or_default();
    attempt(
        "connect-loopback",
        address
            .parse::<std::net::SocketAddr>()
            .map_err(|error| error.to_string())
            .and_then(|address| {
                // In the Windows LPAC sandbox Winsock cannot start and std
                // panics; that is the denial, not a crash of the probe.
                std::panic::set_hook(Box::new(|_| {}));
                let connected = std::panic::catch_unwind(|| {
                    std::net::TcpStream::connect_timeout(
                        &address,
                        std::time::Duration::from_secs(2),
                    )
                });
                let _ = std::panic::take_hook();
                match connected {
                    Ok(Ok(_)) => Ok("connected".to_owned()),
                    Ok(Err(error)) => Err(error.kind().to_string()),
                    Err(_) => Err("sockets unavailable".to_owned()),
                }
            }),
    );
    attempt(
        "spawn-shell",
        if cfg!(windows) {
            let mut shell = std::process::Command::new(
                std::path::Path::new(&target("SystemRoot", r"C:\Windows"))
                    .join(r"System32\cmd.exe"),
            );
            shell.args(["/c", "exit", "0"]);
            shell
        } else {
            let mut shell = std::process::Command::new("/bin/sh");
            shell.args(["-c", "true"]);
            shell
        }
        .status()
        .map(|status| status.to_string())
        .map_err(|error| error.kind().to_string()),
    );
    let done = RendererMessage::Done {
        status: 0,
        output: report.into_bytes(),
    };
    match channel.send(&done) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// The channel to the broker and the request counter.
struct BrokerLink {
    channel: RefCell<Link>,
    next_id: Cell<u32>,
}

impl BrokerLink {
    /// Send a request and wait for its reply. Any protocol failure ends
    /// the renderer: a broker that misbehaves cannot be trusted further.
    fn request(
        &self,
        build: impl FnOnce(u32) -> RendererMessage,
    ) -> Result<BrokerMessage, IpcError> {
        let id = self.next_id.get();
        self.next_id.set(id.wrapping_add(1));
        let mut channel = self.channel.borrow_mut();
        channel.send(&build(id))?;
        let reply = channel.recv::<BrokerMessage>()?;
        let reply_id = match &reply {
            BrokerMessage::Resource { id, .. }
            | BrokerMessage::Fetch { id, .. }
            | BrokerMessage::Storage { id, .. }
            | BrokerMessage::Frame { id, .. } => Some(*id),
            BrokerMessage::Start(_) => None,
        };
        if reply_id != Some(id) {
            eprintln!("rustic renderer: the broker answered out of order");
            std::process::exit(1);
        }
        Ok(reply)
    }
}

fn lost(error: &IpcError) -> ! {
    eprintln!("rustic renderer: lost the broker: {error}");
    std::process::exit(1);
}

/// Loads through the broker. Decisions about inline content use the
/// document's CSP here; every request is checked again by the broker.
struct BrokeredSession {
    link: Rc<BrokerLink>,
    url: Url,
    csp: CspList,
    local: bool,
    storage: bool,
    charsets: RefCell<HashMap<String, String>>,
}

impl std::fmt::Debug for BrokeredSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BrokeredSession")
            .field("local", &self.local)
            .finish_non_exhaustive()
    }
}

impl Session for BrokeredSession {
    fn document_url(&self) -> &Url {
        &self.url
    }

    fn csp(&self) -> &CspList {
        &self.csp
    }

    fn is_local(&self) -> bool {
        self.local
    }

    fn fetch(
        &self,
        url: &Url,
        kind: &ResourceKind,
        max_bytes: usize,
    ) -> Result<Vec<u8>, FetchError> {
        let kind = match kind {
            ResourceKind::Stylesheet => rustic_ipc::protocol::ResourceKind::Stylesheet,
            ResourceKind::Image => rustic_ipc::protocol::ResourceKind::Image,
            ResourceKind::Script {
                nonce,
                parser_inserted,
                module,
            } => rustic_ipc::protocol::ResourceKind::Script {
                nonce: nonce.clone(),
                parser_inserted: *parser_inserted,
                module: *module,
            },
        };
        let reply = self
            .link
            .request(|id| RendererMessage::Resource {
                id,
                url: url.serialize(),
                kind,
                max_bytes: max_bytes as u64,
            })
            .unwrap_or_else(|error| lost(&error));
        let BrokerMessage::Resource { result, .. } = reply else {
            return Err(FetchError::Failed);
        };
        match result {
            Ok(body) => {
                if body.bytes.len() > max_bytes {
                    return Err(FetchError::TooLarge);
                }
                if let Some(charset) = body.charset {
                    self.charsets.borrow_mut().insert(url.serialize(), charset);
                }
                Ok(body.bytes)
            }
            Err(ResourceFailure::Blocked) => Err(FetchError::Blocked),
            Err(ResourceFailure::NotFound) => Err(FetchError::NotFound),
            Err(ResourceFailure::TooLarge) => Err(FetchError::TooLarge),
            Err(ResourceFailure::Failed) => Err(FetchError::Failed),
            Err(ResourceFailure::Cancelled) => Err(FetchError::Cancelled),
        }
    }

    fn charset_of(&self, url: &Url) -> Option<String> {
        self.charsets.borrow().get(&url.serialize()).cloned()
    }

    fn script_fetch(
        &self,
        request: rustic_engine::ScriptRequest,
    ) -> Result<rustic_engine::ScriptResponse, rustic_engine::NetworkFailure> {
        let wire = FetchRequest {
            method: request.method,
            url: request.url.serialize(),
            headers: request.headers,
            body: request.body,
            mode: request.mode,
            credentials: request.credentials,
            redirect: request.redirect,
            referrer_policy: request.referrer_policy,
        };
        let reply = self
            .link
            .request(|id| RendererMessage::Fetch { id, request: wire })
            .unwrap_or_else(|error| lost(&error));
        let BrokerMessage::Fetch {
            result: Some(response),
            ..
        } = reply
        else {
            return Err(rustic_engine::NetworkFailure);
        };
        let kind = match response.kind.as_str() {
            "basic" => "basic",
            "cors" => "cors",
            "opaqueredirect" => "opaqueredirect",
            _ => "opaque",
        };
        Ok(rustic_engine::ScriptResponse {
            kind,
            url: response.url.and_then(|url| Url::parse(&url, None).ok()),
            redirected: response.redirected,
            status: response.status,
            status_text: response.status_text,
            headers: response.headers,
            body: response.body,
        })
    }

    fn storage_host(&self) -> Result<Option<Rc<dyn StorageHost>>, String> {
        Ok(self.storage.then(|| {
            Rc::new(BrokeredStorage {
                link: Rc::clone(&self.link),
            }) as Rc<dyn StorageHost>
        }))
    }

    fn finish(&self) -> Result<(), String> {
        // The broker persists cookies and the profile after `Done`.
        Ok(())
    }
}

/// Web Storage through the broker, which owns the profile.
#[derive(Debug)]
struct BrokeredStorage {
    link: Rc<BrokerLink>,
}

impl std::fmt::Debug for BrokerLink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("BrokerLink").finish_non_exhaustive()
    }
}

impl BrokeredStorage {
    fn ask(&self, kind: StorageKind, op: StorageOp) -> StorageValue {
        let area = match kind {
            StorageKind::Local => StorageArea::Local,
            StorageKind::Session => StorageArea::Session,
        };
        match self
            .link
            .request(|id| RendererMessage::Storage { id, area, op })
            .unwrap_or_else(|error| lost(&error))
        {
            BrokerMessage::Storage { value, .. } => value,
            _ => StorageValue::Refused,
        }
    }
}

impl StorageHost for BrokeredStorage {
    fn length(&self, kind: StorageKind) -> u32 {
        match self.ask(kind, StorageOp::Length) {
            StorageValue::Count(count) => count,
            _ => 0,
        }
    }

    fn key(&self, kind: StorageKind, index: u32) -> Option<Vec<u16>> {
        match self.ask(kind, StorageOp::Key(index)) {
            StorageValue::Units(units) => units,
            _ => None,
        }
    }

    fn get(&self, kind: StorageKind, key: &[u16]) -> Option<Vec<u16>> {
        match self.ask(kind, StorageOp::Get(key.to_vec())) {
            StorageValue::Units(units) => units,
            _ => None,
        }
    }

    fn set(&self, kind: StorageKind, key: &[u16], value: &[u16]) -> Result<(), QuotaExceeded> {
        match self.ask(kind, StorageOp::Set(key.to_vec(), value.to_vec())) {
            StorageValue::Done => Ok(()),
            _ => Err(QuotaExceeded),
        }
    }

    fn remove(&self, kind: StorageKind, key: &[u16]) {
        self.ask(kind, StorageOp::Remove(key.to_vec()));
    }

    fn clear(&self, kind: StorageKind) {
        self.ask(kind, StorageOp::Clear);
    }

    fn keys(&self, kind: StorageKind) -> Vec<Vec<u16>> {
        match self.ask(kind, StorageOp::Keys) {
            StorageValue::Keys(keys) => keys,
            _ => Vec::new(),
        }
    }
}
