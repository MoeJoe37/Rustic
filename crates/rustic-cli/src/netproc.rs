//! The network process (M7, ISO-07; ADR 0061).
//!
//! In a split run the broker keeps every network *decision* (the origin
//! grants, CSP, mixed content, CORS, cookies, the HTTP cache, ORB) but no
//! longer runs exchanges itself: each request its policy grants is handed,
//! with the granted address, to a `rustic __network` child that opens the
//! socket, runs TLS, parses HTTP/1.1 framing and undoes content codings —
//! the code that reads hostile bytes from servers. Its replies are
//! revalidated here: heads go through [`ResponseHead::from_parts`] and
//! [`Headers::append`], body bytes may not exceed the credit the consumer
//! has returned ([`rustic_net::remote`]), events for unknown exchanges or
//! out of order and unknown error codes end the process.
//!
//! A network process that crashes, hangs up or breaks the protocol fails
//! only the exchanges it was running (with
//! [`TransportError::NetworkProcess`]); the next request starts a fresh
//! process, at most [`MAX_STARTS`] times per run, so a process that dies
//! on every request cannot loop. Nothing it held survives into the new
//! one: the broker keeps the cookies and cache, the child only sockets.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Stdout, Write};
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

use rustic_base::EscapedText;
use rustic_ipc::network::{ExchangeLimits, Head, NetworkCommand, NetworkEvent, Open};
use rustic_ipc::{Channel, IpcError, Limits, Receiver, Role, Sender};
use rustic_net::remote::{
    Connector, ExchangeHooks, FeedError, RemoteFeed, error_code, error_from_code, remote_exchange,
};
use rustic_net::tls::TlsConfig;
use rustic_net::{
    Exchange, Headers, HttpVersion, NetEvent, NetworkPolicy, Request, ResponseHead, Transport,
    TransportError, TransportLimits,
};
use rustic_url::Url;

/// The hidden subcommand that runs the network process.
pub const NETWORK_COMMAND: &str = "__network";

/// Network processes started per run, the first included.
pub const MAX_STARTS: u32 = 4;

/// Largest frame on the network channel: a body piece is at most one
/// window, a head at most `max_head_bytes`; 16 MiB covers both defaults
/// many times over.
const NETWORK_FRAME_BYTES: usize = 16 * 1024 * 1024;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Each guarded value is replaced or updated in one step, so a panic
    // elsewhere cannot leave it half-written.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn channel_limits() -> Limits {
    Limits {
        max_frame_bytes: NETWORK_FRAME_BYTES,
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn wire_limits(limits: &TransportLimits) -> ExchangeLimits {
    let size = |value: usize| u64::try_from(value).unwrap_or(u64::MAX);
    ExchangeLimits {
        max_head_bytes: size(limits.max_head_bytes),
        max_header_count: size(limits.max_header_count),
        max_body_bytes: limits.max_body_bytes,
        max_chunk_line_bytes: size(limits.max_chunk_line_bytes),
        max_trailer_bytes: size(limits.max_trailer_bytes),
        max_interim_responses: size(limits.max_interim_responses),
        window: size(limits.buffer_window),
        read_chunk: size(limits.read_chunk),
        connect_ms: millis(limits.connect_timeout),
        idle_ms: millis(limits.idle_timeout),
        total_ms: limits.total_timeout.map(millis),
        poll_ms: millis(limits.poll_interval),
    }
}

fn transport_limits(limits: &ExchangeLimits) -> Option<TransportLimits> {
    let size = |value: u64| usize::try_from(value).ok();
    let limits = TransportLimits {
        max_head_bytes: size(limits.max_head_bytes)?,
        max_header_count: size(limits.max_header_count)?,
        max_body_bytes: limits.max_body_bytes,
        max_chunk_line_bytes: size(limits.max_chunk_line_bytes)?,
        max_trailer_bytes: size(limits.max_trailer_bytes)?,
        max_interim_responses: size(limits.max_interim_responses)?,
        // The window bounds what this process holds for one exchange.
        buffer_window: size(limits.window).filter(|&window| window <= NETWORK_FRAME_BYTES / 2)?,
        read_chunk: size(limits.read_chunk)?,
        connect_timeout: Duration::from_millis(limits.connect_ms),
        idle_timeout: Duration::from_millis(limits.idle_ms),
        total_timeout: limits.total_ms.map(Duration::from_millis),
        poll_interval: Duration::from_millis(limits.poll_ms),
    };
    limits.validate().ok()?;
    Some(limits)
}

// ---------------------------------------------------------------------
// Broker side
// ---------------------------------------------------------------------

/// The broker's handle on its network process, started on first use.
#[derive(Debug)]
pub struct NetworkProcess {
    ca_pem: Option<Vec<u8>>,
    link: Mutex<Option<Arc<Link>>>,
    starts: AtomicU32,
}

static PROCESS: OnceLock<Arc<NetworkProcess>> = OnceLock::new();

/// The run's network process (created on first call, started on the first
/// request). `ca_pem` replaces the platform trust store in the child.
pub fn shared(ca_pem: Option<&[u8]>) -> Arc<NetworkProcess> {
    Arc::clone(PROCESS.get_or_init(|| {
        Arc::new(NetworkProcess {
            ca_pem: ca_pem.map(<[u8]>::to_vec),
            link: Mutex::new(None),
            starts: AtomicU32::new(0),
        })
    }))
}

impl NetworkProcess {
    /// The live link, starting a process if there is none or the last one
    /// died.
    fn link(&self) -> Result<Arc<Link>, TransportError> {
        let mut slot = lock(&self.link);
        if let Some(link) = slot.as_ref() {
            if !link.dead.load(Ordering::Acquire) {
                return Ok(Arc::clone(link));
            }
        }
        *slot = None;
        if self.starts.fetch_add(1, Ordering::Relaxed) >= MAX_STARTS {
            self.starts.store(MAX_STARTS, Ordering::Relaxed);
            return Err(TransportError::NetworkProcess);
        }
        let link = Link::start(self.ca_pem.clone()).map_err(|error| {
            eprintln!("rustic: network process: {error}");
            TransportError::NetworkProcess
        })?;
        *slot = Some(Arc::clone(&link));
        Ok(link)
    }
}

impl Connector for NetworkProcess {
    fn start(
        &self,
        request: Request,
        address: SocketAddr,
        limits: &TransportLimits,
    ) -> Result<Exchange, TransportError> {
        let link = self.link()?;
        let id = link.next_id.fetch_add(1, Ordering::Relaxed);
        let hooks = Arc::new(Hooks {
            link: Arc::downgrade(&link),
            id,
        });
        let (feed, exchange) = remote_exchange(limits.buffer_window, hooks);
        lock(&link.exchanges).insert(id, Arc::new(Mutex::new(feed)));
        let open = Open {
            id,
            method: request.method().to_owned(),
            url: request.url().serialize(),
            headers: request
                .headers()
                .iter()
                .map(|(name, value)| (name.to_owned(), value.to_vec()))
                .collect(),
            body: request.body().to_vec(),
            address: address.to_string(),
            limits: wire_limits(limits),
        };
        if link.send(&NetworkCommand::Open(Box::new(open))).is_err() {
            link.fail_all("the network process stopped");
        }
        Ok(exchange)
    }
}

type Slot = Arc<Mutex<RemoteFeed>>;

/// One running network process.
struct Link {
    child: Mutex<crate::broker::Process>,
    /// The bubblewrap filter file, kept until the launcher removed it.
    #[cfg(not(windows))]
    _filter: Option<crate::broker::SeccompFile>,
    sender: Mutex<Sender<Box<dyn Write + Send>>>,
    exchanges: Mutex<HashMap<u64, Slot>>,
    next_id: AtomicU64,
    dead: AtomicBool,
    max_header_count: usize,
}

impl fmt::Debug for Link {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Link")
            .field("dead", &self.dead)
            .finish_non_exhaustive()
    }
}

type Pipes = (
    Child,
    Box<dyn Read + Send>,
    Box<dyn Write + Send>,
    Box<dyn Read + Send>,
);

/// A started network process and its standard output, input and error.
#[cfg(windows)]
type ProcessPipes = (
    crate::broker::Process,
    Box<dyn Read + Send>,
    Box<dyn Write + Send>,
    Box<dyn Read + Send>,
);

/// Windows: the child in the `Rustic.Network` AppContainer (outbound
/// Internet and certificate verification only, no loopback, files or
/// processes), in a kill-on-close job of one process (TD-176).
#[cfg(windows)]
fn spawn_confined(exe: &std::path::Path) -> io::Result<ProcessPipes> {
    let command = std::ffi::OsStr::new(NETWORK_COMMAND);
    let (confined, pipes) = rustic_sandbox::spawn_app_container(
        rustic_sandbox::Container::Network,
        exe,
        &[command],
        &crate::broker::sandbox_env(),
    )?;
    Ok((
        crate::broker::Process::Confined(confined),
        Box::new(pipes.stdout),
        Box::new(pipes.stdin),
        Box::new(pipes.stderr),
    ))
}

/// Start the child unconfined.
fn spawn_plain(exe: &std::path::Path) -> io::Result<Pipes> {
    let mut child = Command::new(exe)
        .arg(NETWORK_COMMAND)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    Ok((child, Box::new(stdout), Box::new(stdin), Box::new(stderr)))
}

/// macOS: the child with a cleared environment, `/` as its working
/// directory and `RUSTIC_SEATBELT=network`, which makes it apply the
/// Seatbelt network profile (ADR 0063,
/// `rustic_sandbox::seatbelt::network_profile`) to itself before it reads
/// from the broker.
#[cfg(not(windows))]
fn spawn_seatbelt(exe: &std::path::Path) -> io::Result<Pipes> {
    let mut child = Command::new(exe)
        .arg(NETWORK_COMMAND)
        .env_clear()
        .envs(crate::broker::sandbox_env())
        .env(crate::broker::SEATBELT_VARIABLE, "network")
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    Ok((child, Box::new(stdout), Box::new(stdin), Box::new(stderr)))
}

impl Link {
    fn start(ca_pem: Option<Vec<u8>>) -> io::Result<Arc<Self>> {
        let exe = std::env::current_exe()?;
        // Under the sandbox the network process is confined like a
        // renderer, but keeps Internet sockets (TD-176): bubblewrap and a
        // seccomp filter on Linux, the Seatbelt network profile on macOS
        // (both on by default).
        // A setup failure is an error, never an unconfined start. On
        // Windows it runs in the `Rustic.Network` AppContainer unless a
        // loopback fixture origin is granted, which no AppContainer can
        // reach (TD-176); remote origins are always served confined.
        #[cfg(not(windows))]
        let (child, stdout, stdin, stderr, filter) =
            if crate::broker::sandboxed() && cfg!(target_os = "macos") {
                let (child, stdout, stdin, stderr) = spawn_seatbelt(&exe)?;
                (child, stdout, stdin, stderr, None)
            } else if crate::broker::sandboxed() && cfg!(target_os = "linux") {
                let (child, filter, stdout, stdin, stderr) = crate::broker::spawn_bubblewrap(
                    &exe,
                    NETWORK_COMMAND,
                    crate::broker::Confinement::Network,
                )?;
                let stderr: Box<dyn Read + Send> = Box::new(stderr);
                (child, stdout, stdin, stderr, Some(filter))
            } else {
                let (child, stdout, stdin, stderr) = spawn_plain(&exe)?;
                (child, stdout, stdin, stderr, None)
            };
        #[cfg(not(windows))]
        let child = crate::broker::Process::Child(child);
        #[cfg(windows)]
        let (child, stdout, stdin, stderr) =
            if crate::broker::sandboxed() && !crate::network::has_loopback_grants() {
                spawn_confined(&exe)?
            } else {
                let (child, stdout, stdin, stderr) = spawn_plain(&exe)?;
                (crate::broker::Process::Child(child), stdout, stdin, stderr)
            };
        let mut child = child;
        // Relay the child's diagnostics escaped: it is untrusted and must
        // not write control sequences to the user's terminal.
        std::thread::Builder::new()
            .name("rustic network stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
                    let line = String::from_utf8_lossy(&line);
                    eprintln!("{}", EscapedText(line.trim_end()));
                }
            })?;
        let handshake =
            Channel::connect_to(stdout, stdin, Role::Broker, Role::Network, channel_limits());
        let channel = match handshake {
            Ok(channel) => channel,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::other(error.to_string()));
            }
        };
        let (receiver, mut sender) = channel.split();
        if let Err(error) = sender.send(&NetworkCommand::Configure { ca_pem }) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(error.to_string()));
        }
        let link = Arc::new(Self {
            child: Mutex::new(child),
            #[cfg(not(windows))]
            _filter: filter,
            sender: Mutex::new(sender),
            exchanges: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            dead: AtomicBool::new(false),
            max_header_count: TransportLimits::default().max_header_count,
        });
        let dispatcher = Arc::clone(&link);
        std::thread::Builder::new()
            .name("rustic network dispatch".into())
            .spawn(move || dispatcher.dispatch(receiver))?;
        Ok(link)
    }

    fn send(&self, command: &NetworkCommand) -> Result<(), IpcError> {
        if self.dead.load(Ordering::Acquire) {
            return Err(IpcError::Closed);
        }
        lock(&self.sender).send(command)
    }

    /// Read events until the channel ends or the peer misbehaves.
    fn dispatch(self: Arc<Self>, mut receiver: Receiver<Box<dyn Read + Send>>) {
        let reason = loop {
            match receiver.recv::<NetworkEvent>() {
                Ok(event) => {
                    if let Err(reason) = self.deliver(event) {
                        break reason;
                    }
                }
                Err(IpcError::Closed) => break "the network process stopped".to_owned(),
                Err(error) => break format!("the network process broke the protocol: {error}"),
            }
        };
        self.fail_all(&reason);
    }

    /// Route one event to its exchange. An error names a protocol
    /// violation, which ends the process.
    fn deliver(&self, event: NetworkEvent) -> Result<(), String> {
        let (id, event) = match event {
            NetworkEvent::Head(head) => {
                let Head {
                    id,
                    http11,
                    status,
                    reason,
                    headers,
                } = *head;
                if headers.len() > self.max_header_count {
                    return Err("the network process sent too many header fields".into());
                }
                let mut list = Headers::new();
                for (name, value) in &headers {
                    list.append(name, value)
                        .map_err(|_| "the network process sent an invalid header field")?;
                }
                let version = if http11 {
                    HttpVersion::Http11
                } else {
                    HttpVersion::Http10
                };
                let head = ResponseHead::from_parts(version, status, reason, list)
                    .map_err(|_| "the network process sent an invalid status line")?;
                (id, NetEvent::Response(head))
            }
            NetworkEvent::Body { id, bytes } => (id, NetEvent::Body(bytes)),
            NetworkEvent::Finished { id } => (id, NetEvent::Finished),
            NetworkEvent::Failed { id, code, detail } => {
                let error = error_from_code(code, detail)
                    .ok_or("the network process sent an unknown error")?;
                (id, NetEvent::Failed(error))
            }
        };
        if id == 0 || id >= self.next_id.load(Ordering::Relaxed) {
            return Err("the network process answered an exchange it was never given".into());
        }
        // An exchange the consumer already cancelled or dropped: ignore
        // the rest of what the process says about it.
        let Some(slot) = lock(&self.exchanges).get(&id).cloned() else {
            return Ok(());
        };
        let feed = lock(&slot);
        match feed.push(event) {
            Ok(()) => {
                if feed.ended() {
                    lock(&self.exchanges).remove(&id);
                }
                Ok(())
            }
            Err(FeedError::ConsumerGone) => {
                drop(feed);
                lock(&self.exchanges).remove(&id);
                let _ = self.send(&NetworkCommand::Cancel { id });
                Ok(())
            }
            Err(error) => Err(format!("the network process broke the protocol: {error}")),
        }
    }

    /// End the process and fail every exchange it was running.
    fn fail_all(&self, reason: &str) {
        if self.dead.swap(true, Ordering::AcqRel) {
            return;
        }
        let pending: Vec<Slot> = lock(&self.exchanges)
            .drain()
            .map(|(_, slot)| slot)
            .collect();
        if !pending.is_empty() || reason != "the network process stopped" {
            eprintln!("rustic: {reason}; {} request(s) failed", pending.len());
        }
        for slot in pending {
            lock(&slot).fail(TransportError::NetworkProcess);
        }
        let mut child = lock(&self.child);
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        let child = self.child.get_mut().unwrap_or_else(PoisonError::into_inner);
        let _ = child.kill();
        let _ = child.wait();
    }
}

struct Hooks {
    link: Weak<Link>,
    id: u64,
}

impl ExchangeHooks for Hooks {
    fn consumed(&self, bytes: usize) {
        if let Some(link) = self.link.upgrade() {
            let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
            let _ = link.send(&NetworkCommand::Credit { id: self.id, bytes });
        }
    }

    fn cancelled(&self) {
        let Some(link) = self.link.upgrade() else {
            return;
        };
        let slot = lock(&link.exchanges).remove(&self.id);
        if let Some(slot) = slot {
            lock(&slot).fail(TransportError::Cancelled);
            let _ = link.send(&NetworkCommand::Cancel { id: self.id });
        }
    }
}

/// A transport whose exchanges run in the shared network process.
pub fn transport(
    policy: NetworkPolicy,
    limits: TransportLimits,
    ca_pem: Option<&[u8]>,
) -> Result<Transport, String> {
    Transport::new(policy, limits)
        .map(|transport| transport.with_connector(shared(ca_pem)))
        .map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------
// Network process side
// ---------------------------------------------------------------------

/// What the child knows about one running exchange.
struct Running {
    /// Body bytes it may still send.
    credit: Mutex<u64>,
    changed: Condvar,
    cancelled: AtomicBool,
}

type Out = Arc<Mutex<Sender<Stdout>>>;

/// Run as the network process. Returns the exit status.
pub fn run() -> u8 {
    let channel = match Channel::connect_to(
        io::stdin(),
        io::stdout(),
        Role::Network,
        Role::Broker,
        channel_limits(),
    ) {
        Ok(channel) => channel,
        Err(error) => {
            eprintln!("rustic network process: {error}");
            return 1;
        }
    };
    let (mut receiver, sender) = channel.split();
    let out: Out = Arc::new(Mutex::new(sender));
    let tls = match receiver.recv::<NetworkCommand>() {
        Ok(NetworkCommand::Configure { ca_pem: Some(pem) }) => {
            match TlsConfig::with_roots_pem(&pem) {
                Ok(tls) => Some(tls),
                Err(error) => {
                    eprintln!("rustic network process: certificate authority: {error}");
                    return 1;
                }
            }
        }
        Ok(NetworkCommand::Configure { ca_pem: None }) => None,
        Ok(_) => {
            eprintln!("rustic network process: the broker did not configure the process first");
            return 1;
        }
        Err(error) => {
            eprintln!("rustic network process: {error}");
            return 1;
        }
    };
    let test_mode = std::env::var("RUSTIC_TEST_NETWORK_PROCESS").ok();
    let running: Arc<Mutex<HashMap<u64, Arc<Running>>>> = Arc::default();
    loop {
        let command = match receiver.recv::<NetworkCommand>() {
            Ok(command) => command,
            // The broker exited or hung up: nothing is left to do.
            Err(IpcError::Closed) => return 0,
            Err(error) => {
                eprintln!("rustic network process: {error}");
                return 1;
            }
        };
        match command {
            NetworkCommand::Configure { .. } => {
                eprintln!("rustic network process: configured twice");
                return 1;
            }
            NetworkCommand::Open(open) => {
                if let Some(mode) = test_mode.as_deref() {
                    if misbehave(mode, &out, open.id, &open.url) {
                        continue;
                    }
                }
                let id = open.id;
                let entry = Arc::new(Running {
                    credit: Mutex::new(0),
                    changed: Condvar::new(),
                    cancelled: AtomicBool::new(false),
                });
                if lock(&running).insert(id, Arc::clone(&entry)).is_some() {
                    eprintln!("rustic network process: exchange {id} opened twice");
                    return 1;
                }
                let exchange_out = Arc::clone(&out);
                let tls = tls.clone();
                let table = Arc::clone(&running);
                let spawned = std::thread::Builder::new()
                    .name("rustic network exchange".into())
                    .spawn(move || {
                        let failure = exchange(*open, tls, &entry, &exchange_out).err();
                        if let Some(error) = failure {
                            let (code, detail) = error_code(error);
                            let _ = lock(&exchange_out).send(&NetworkEvent::Failed {
                                id,
                                code,
                                detail,
                            });
                        }
                        lock(&table).remove(&id);
                    });
                if spawned.is_err() {
                    lock(&running).remove(&id);
                    let (code, detail) = error_code(TransportError::Io(io::ErrorKind::Other));
                    let _ = lock(&out).send(&NetworkEvent::Failed { id, code, detail });
                }
            }
            NetworkCommand::Cancel { id } => {
                if let Some(entry) = lock(&running).get(&id) {
                    entry.cancelled.store(true, Ordering::Release);
                    entry.changed.notify_all();
                }
            }
            NetworkCommand::Credit { id, bytes } => {
                if let Some(entry) = lock(&running).get(&id) {
                    let mut credit = lock(&entry.credit);
                    *credit = credit.saturating_add(bytes);
                    entry.changed.notify_all();
                }
            }
        }
    }
}

/// Run one exchange and stream its events. `Err` is the failure to report.
fn exchange(
    open: Open,
    tls: Option<TlsConfig>,
    entry: &Running,
    out: &Out,
) -> Result<(), TransportError> {
    let id = open.id;
    let limits = transport_limits(&open.limits).ok_or(TransportError::Blocked)?;
    let url = Url::parse(&open.url, None).map_err(|_| TransportError::Blocked)?;
    let address: SocketAddr = open.address.parse().map_err(|_| TransportError::Blocked)?;
    // The broker's grant is re-checked here as defence in depth: a
    // loopback fixture address, or a public one the broker resolved
    // (TD-157); never a private or other special-purpose address. This
    // process never resolves names.
    let mut policy = NetworkPolicy::deny_all();
    if address.ip().is_loopback() {
        policy.allow(&url, address)
    } else {
        policy.allow_public(&url, address)
    }
    .map_err(|_| TransportError::Blocked)?;
    let mut request = Request::new(&open.method, url).map_err(|_| TransportError::Blocked)?;
    for (name, value) in &open.headers {
        request
            .header(name, value)
            .map_err(|_| TransportError::Blocked)?;
    }
    request.set_body(open.body);
    let window = u64::try_from(limits.buffer_window).unwrap_or(u64::MAX);
    *lock(&entry.credit) = window;
    let poll = limits.poll_interval;
    let mut transport = Transport::new(policy, limits).map_err(|_| TransportError::Blocked)?;
    if let Some(tls) = tls {
        transport = transport.with_tls(tls);
    }
    let running = transport.start(request)?;
    let send = |event: &NetworkEvent| {
        lock(out)
            .send(event)
            .map_err(|_| TransportError::NetworkProcess)
    };
    loop {
        if entry.cancelled.load(Ordering::Acquire) {
            running.cancel();
        }
        let Some(event) = running.next_event(Some(poll)) else {
            continue;
        };
        match event {
            NetEvent::Response(head) => send(&NetworkEvent::Head(Box::new(Head {
                id,
                http11: head.version() == HttpVersion::Http11,
                status: head.status(),
                reason: head.reason().to_vec(),
                headers: head
                    .headers()
                    .iter()
                    .map(|(name, value)| (name.to_owned(), value.to_vec()))
                    .collect(),
            })))?,
            NetEvent::Body(bytes) => {
                let piece = usize::try_from(window).unwrap_or(usize::MAX).max(1);
                for chunk in bytes.chunks(piece) {
                    wait_for_credit(entry, chunk.len(), poll, &running)?;
                    send(&NetworkEvent::Body {
                        id,
                        bytes: chunk.to_vec(),
                    })?;
                }
            }
            NetEvent::Finished => {
                send(&NetworkEvent::Finished { id })?;
                return Ok(());
            }
            NetEvent::Failed(error) => return Err(error),
        }
    }
}

/// Wait until `bytes` of credit are available and take them.
fn wait_for_credit(
    entry: &Running,
    bytes: usize,
    poll: Duration,
    running: &Exchange,
) -> Result<(), TransportError> {
    let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
    let mut credit = lock(&entry.credit);
    while *credit < bytes {
        if entry.cancelled.load(Ordering::Acquire) {
            running.cancel();
            return Err(TransportError::Cancelled);
        }
        credit = entry
            .changed
            .wait_timeout(credit, poll)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
    *credit -= bytes;
    Ok(())
}

/// Test hooks for a broken or compromised network process. They change
/// only what this untrusted process does, which the broker must survive
/// anyway. Returns whether the request was consumed.
fn misbehave(mode: &str, out: &Out, id: u64, url: &str) -> bool {
    let send = |event: NetworkEvent| {
        let _ = lock(out).send(&event);
    };
    let head = |status| {
        NetworkEvent::Head(Box::new(Head {
            id,
            http11: true,
            status,
            reason: b"OK".to_vec(),
            headers: vec![("Content-Type".into(), b"text/html".to_vec())],
        }))
    };
    match mode {
        "escape" => {
            escape_probes();
            false
        }
        // Probe, then fail the request without connecting anywhere (a
        // remote grant in a test that must not use the Internet).
        "escape-only" => {
            escape_probes();
            let (code, detail) = rustic_net::remote::error_code(TransportError::Blocked);
            send(NetworkEvent::Failed { id, code, detail });
            true
        }
        "crash" => panic!("simulated network process crash"),
        _ if mode
            .strip_prefix("crash-on:")
            .is_some_and(|path| url.ends_with(path)) =>
        {
            panic!("simulated network process crash")
        }
        "credit-overrun" => {
            send(head(200));
            send(NetworkEvent::Body {
                id,
                bytes: vec![b'a'; 64 * 1024 * 1024 / 8],
            });
            true
        }
        "bad-status" => {
            send(head(101));
            true
        }
        "foreign-exchange" => {
            send(NetworkEvent::Finished { id: id + 1000 });
            true
        }
        "unknown-error" => {
            send(NetworkEvent::Failed {
                id,
                code: 999,
                detail: 0,
            });
            true
        }
        _ => false,
    }
}

/// Try what a compromised network process would (TD-176) and report
/// each attempt on standard error as `network-escape NAME: allowed` or
/// `denied`. Connecting to loopback TCP is its job on Linux and macOS
/// and is not probed there; on Windows it is (the container serves remote
/// origins only).
fn escape_probes() {
    let report = |name: &str, allowed: bool| {
        eprintln!(
            "network-escape {name}: {}",
            if allowed { "allowed" } else { "denied" }
        );
    };
    let var = |name| std::env::var_os(name).unwrap_or_default();
    report(
        "read-user-file",
        std::fs::read(var("RUSTIC_TEST_ESCAPE_FILE")).is_ok(),
    );
    report(
        "list-home",
        std::fs::read_dir(var("RUSTIC_TEST_ESCAPE_DIR")).is_ok(),
    );
    let shell = if cfg!(windows) {
        Command::new("cmd.exe").args(["/c", "exit 0"]).status()
    } else {
        Command::new("/bin/sh").args(["-c", "exit 0"]).status()
    };
    report("spawn-shell", shell.is_ok_and(|status| status.success()));
    // Windows: loopback is out of the network container's reach (TD-176).
    #[cfg(windows)]
    {
        let reached = var("RUSTIC_TEST_ESCAPE_ADDRESS")
            .to_str()
            .and_then(|address| address.parse::<std::net::SocketAddr>().ok())
            .is_some_and(|address| {
                std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_ok()
            });
        report("connect-loopback", reached);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::linux::net::SocketAddrExt;
        let name = var("RUSTIC_TEST_ESCAPE_ABSTRACT");
        let reached = std::os::unix::net::SocketAddr::from_abstract_name(name.as_encoded_bytes())
            .and_then(|address| std::os::unix::net::UnixStream::connect_addr(&address))
            .is_ok();
        report("connect-abstract-socket", reached);
    }
    #[cfg(target_os = "macos")]
    {
        let reached =
            std::os::unix::net::UnixStream::connect(var("RUSTIC_TEST_ESCAPE_UNIX")).is_ok();
        report("connect-unix-socket", reached);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_survive_the_wire_and_bad_ones_are_refused() {
        let limits = TransportLimits {
            total_timeout: Some(Duration::from_millis(1500)),
            ..TransportLimits::default()
        };
        assert_eq!(transport_limits(&wire_limits(&limits)), Some(limits));
        let mut wire = wire_limits(&TransportLimits::default());
        wire.window = 0;
        assert_eq!(transport_limits(&wire), None, "a zero window is invalid");
        wire.window = u64::MAX;
        assert_eq!(
            transport_limits(&wire),
            None,
            "a window beyond the frame bound"
        );
    }
}
