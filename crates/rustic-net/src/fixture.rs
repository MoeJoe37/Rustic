//! An in-process HTTP/1.1 server for tests (ADR 0053).
//!
//! It binds `127.0.0.1` on an ephemeral port only, answers each connection
//! on its own thread with a scripted [`Reply`], and records every request it
//! reads. It never contacts another host, so tests that use it run offline
//! and under emulated tiers. It is deliberately simple: it is a test
//! instrument, not a server implementation, and it trusts its own scripts.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustic_url::Url;

/// Requests larger than this are answered with 431 and recorded truncated.
const MAX_REQUEST_HEAD: usize = 64 * 1024;
/// How long the server waits for a request before closing the connection.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const TICK: Duration = Duration::from_millis(5);

/// One scripted step of a reply.
#[derive(Clone, Debug)]
enum Action {
    Send(Vec<u8>),
    Pause(Duration),
    Hold,
}

/// What the server does on a connection after reading the request: a
/// sequence of writes and pauses, then closing the connection.
#[derive(Clone, Debug, Default)]
pub struct Reply {
    actions: Vec<Action>,
}

impl Reply {
    /// A reply that closes the connection without writing anything.
    pub fn new() -> Self {
        Self::default()
    }

    /// A complete response with a `Content-Length` and the given fields.
    pub fn response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Self {
        let mut bytes = format!(
            "HTTP/1.1 {status} {}\r\n",
            if status == 200 { "OK" } else { "Fixture" }
        )
        .into_bytes();
        for (name, value) in headers {
            bytes.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        bytes.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
        bytes.extend_from_slice(body);
        Self::new().send(bytes)
    }

    /// Write these bytes verbatim.
    pub fn send(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.actions.push(Action::Send(bytes.into()));
        self
    }

    /// Write `bytes` in pieces of `piece` bytes with `interval` between them.
    pub fn drip(mut self, bytes: &[u8], piece: usize, interval: Duration) -> Self {
        for chunk in bytes.chunks(piece.max(1)) {
            self.actions.push(Action::Send(chunk.to_vec()));
            self.actions.push(Action::Pause(interval));
        }
        self
    }

    /// Wait before the next step.
    pub fn pause(mut self, duration: Duration) -> Self {
        self.actions.push(Action::Pause(duration));
        self
    }

    /// Keep the connection open, writing nothing more, until the client
    /// closes it or the server stops.
    pub fn hold(mut self) -> Self {
        self.actions.push(Action::Hold);
        self
    }
}

/// A request as the fixture server read it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedRequest {
    /// The method.
    pub method: String,
    /// The request-target as sent.
    pub target: String,
    /// Header fields in order, names as sent.
    pub headers: Vec<(String, String)>,
    /// The body, read by its `Content-Length`.
    pub body: Vec<u8>,
}

impl RecordedRequest {
    /// The first value of a header field, compared case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

struct Shared {
    routes: HashMap<String, Reply>,
    stop: AtomicBool,
    log: Mutex<Vec<RecordedRequest>>,
    connections: AtomicUsize,
}

/// A running loopback fixture server; dropping it stops it.
pub struct FixtureServer {
    address: SocketAddr,
    shared: Arc<Shared>,
    acceptor: Option<JoinHandle<()>>,
}

impl FixtureServer {
    /// Bind `127.0.0.1:0` and serve `routes`, keyed by request-target
    /// (an exact match first, then the path without its query). Unknown
    /// targets get a 404.
    pub fn start<'a>(routes: impl IntoIterator<Item = (&'a str, Reply)>) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let shared = Arc::new(Shared {
            routes: routes
                .into_iter()
                .map(|(target, reply)| (target.to_owned(), reply))
                .collect(),
            stop: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            connections: AtomicUsize::new(0),
        });
        let accept_shared = Arc::clone(&shared);
        let acceptor = std::thread::Builder::new()
            .name("rustic-net fixture".to_owned())
            .spawn(move || accept_loop(&listener, &accept_shared))?;
        Ok(Self {
            address,
            shared,
            acceptor: Some(acceptor),
        })
    }

    /// The bound loopback address.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// `http://127.0.0.1:<port>` plus `path`, parsed.
    pub fn url(&self, path: &str) -> Url {
        Url::parse(&format!("http://{}{path}", self.address), None).expect("fixture URLs are valid")
    }

    /// Requests read so far, in arrival order.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.shared
            .log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Connections accepted so far.
    pub fn connections(&self) -> usize {
        self.shared.connections.load(Ordering::SeqCst)
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // Wake the blocking accept so the thread sees the flag.
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_secs(1));
        if let Some(acceptor) = self.acceptor.take() {
            let _ = acceptor.join();
        }
    }
}

fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>) {
    for stream in listener.incoming() {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(stream) = stream else { continue };
        shared.connections.fetch_add(1, Ordering::SeqCst);
        let shared = Arc::clone(shared);
        let _ = std::thread::Builder::new()
            .name("rustic-net fixture connection".to_owned())
            .spawn(move || serve(stream, &shared));
    }
}

fn serve(mut stream: TcpStream, shared: &Shared) {
    let _ = stream.set_nodelay(true);
    let Some(request) = read_request(&mut stream, shared) else {
        return;
    };
    let path = request.target.split('?').next().unwrap_or_default();
    let reply = shared
        .routes
        .get(&request.target)
        .or_else(|| shared.routes.get(path))
        .cloned()
        .unwrap_or_else(|| Reply::response(404, &[], b"not found"));
    shared
        .log
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(request);
    for action in reply.actions {
        let stopped = || shared.stop.load(Ordering::SeqCst);
        match action {
            Action::Send(bytes) => {
                if stream
                    .write_all(&bytes)
                    .and_then(|()| stream.flush())
                    .is_err()
                {
                    return;
                }
            }
            Action::Pause(duration) => {
                let until = Instant::now() + duration;
                while Instant::now() < until && !stopped() {
                    std::thread::sleep(TICK.min(until.saturating_duration_since(Instant::now())));
                }
            }
            Action::Hold => {
                let _ = stream.set_read_timeout(Some(TICK));
                let mut byte = [0; 1];
                while !stopped() {
                    match stream.read(&mut byte) {
                        Ok(0) => return,
                        Ok(_) => {}
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                            ) => {}
                        Err(_) => return,
                    }
                }
            }
        }
        if stopped() {
            return;
        }
    }
}

fn read_request(stream: &mut TcpStream, shared: &Shared) -> Option<RecordedRequest> {
    stream.set_read_timeout(Some(TICK)).ok()?;
    let started = Instant::now();
    let mut data = Vec::new();
    let mut buffer = [0; 4096];
    let head_end = loop {
        if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
        if data.len() > MAX_REQUEST_HEAD {
            let _ = stream.write_all(&Reply::response(431, &[], b"").actions_bytes());
            return None;
        }
        if shared.stop.load(Ordering::SeqCst) || started.elapsed() > REQUEST_TIMEOUT {
            return None;
        }
        match stream.read(&mut buffer) {
            Ok(0) => return None,
            Ok(read) => data.extend_from_slice(&buffer[..read]),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return None,
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end - 4]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_owned();
    let target = request_line.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_owned(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let mut body = data[head_end..].to_vec();
    while body.len() < length {
        if shared.stop.load(Ordering::SeqCst) || started.elapsed() > REQUEST_TIMEOUT {
            return None;
        }
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => body.extend_from_slice(&buffer[..read]),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return None,
        }
    }
    body.truncate(length);
    Some(RecordedRequest {
        method,
        target,
        headers,
        body,
    })
}

impl Reply {
    fn actions_bytes(&self) -> Vec<u8> {
        self.actions
            .iter()
            .filter_map(|action| match action {
                Action::Send(bytes) => Some(bytes.as_slice()),
                _ => None,
            })
            .flatten()
            .copied()
            .collect()
    }
}
