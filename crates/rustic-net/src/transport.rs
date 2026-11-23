use std::cell::Cell;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustic_base::CancellationToken;

use crate::coding::ContentDecoder;
use crate::pipe::{Consumer, Producer, Wait, pipe};
use crate::remote::{Connector, ExchangeHooks};
use crate::response::{BodyDecoder, HeadParser, ProtocolError, ResponseHead, framing};
use crate::tls::{CertificateError, TlsConfig, TlsFailure, classify};
use crate::{LimitsError, NetworkPolicy, PolicyError, Request, Resolver, TransportLimits};

/// Starts HTTP/1.1 exchanges with origins the [`NetworkPolicy`] allows.
///
/// Each exchange runs on its own thread with blocking sockets and short
/// timeouts, checking cancellation and deadlines between steps (ADR 0052).
/// The page-visible order of results is decided by the consumer, which
/// drains [`Exchange`] events from its own task source.
#[derive(Clone, Debug)]
pub struct Transport {
    policy: Arc<NetworkPolicy>,
    limits: TransportLimits,
    tls: Arc<std::sync::OnceLock<Option<TlsConfig>>>,
    /// Where exchanges run when not on this process's threads (a network
    /// process, ISO-07).
    connector: Option<Arc<dyn Connector>>,
    /// Resolves the hosts of origins the policy lets resolve.
    resolver: ResolverHandle,
}

/// The resolver a transport uses (the system resolver by default).
#[derive(Clone)]
struct ResolverHandle(Arc<Resolver>);

impl std::ops::Deref for ResolverHandle {
    type Target = Resolver;
    fn deref(&self) -> &Resolver {
        &*self.0
    }
}

impl fmt::Debug for ResolverHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Resolver")
    }
}

impl Transport {
    /// A transport over a fixed policy. The limits are validated.
    pub fn new(policy: NetworkPolicy, limits: TransportLimits) -> Result<Self, LimitsError> {
        limits.validate()?;
        Ok(Self {
            policy: Arc::new(policy),
            limits,
            tls: Arc::new(std::sync::OnceLock::new()),
            connector: None,
            resolver: ResolverHandle(Arc::new(crate::system_resolver)),
        })
    }

    /// Resolve host names with `resolver` instead of the system resolver
    /// (tests). Its results pass the same public-address check.
    pub fn with_resolver(mut self, resolver: Arc<Resolver>) -> Self {
        self.resolver = ResolverHandle(resolver);
        self
    }

    /// Run every exchange through `connector` (a network process) instead
    /// of on this process's threads. The policy is still checked here,
    /// before anything is handed over; the connector receives only the
    /// address this policy granted.
    pub fn with_connector(mut self, connector: Arc<dyn Connector>) -> Self {
        self.connector = Some(connector);
        self
    }

    /// Verify `https` certificates with `tls` instead of the platform
    /// verifier (enterprise roots or a test CA). Verification itself can
    /// never be turned off.
    pub fn with_tls(mut self, tls: TlsConfig) -> Self {
        self.tls = Arc::new(std::sync::OnceLock::from(Some(tls)));
        self
    }

    /// The TLS configuration, creating the platform one on first use.
    fn tls(&self) -> Option<TlsConfig> {
        self.tls.get_or_init(|| TlsConfig::platform().ok()).clone()
    }

    /// The limits every exchange uses.
    pub fn limits(&self) -> &TransportLimits {
        &self.limits
    }

    /// Start an exchange. Policy failures are reported here, before any
    /// socket is opened; everything later arrives as
    /// [`NetEvent::Failed`].
    ///
    /// `https` verifies the server certificate (ADR 0051); a failure is
    /// [`TransportError::Certificate`] and is never retried over `http`.
    pub fn start(&self, request: Request) -> Result<Exchange, TransportError> {
        let address = self
            .policy
            .address(request.url(), &*self.resolver)
            .map_err(|error| match error {
                PolicyError::NotResolved => TransportError::NameNotResolved,
                PolicyError::NotPublic => TransportError::NotPublic,
                _ => TransportError::Blocked,
            })?;
        if let Some(connector) = &self.connector {
            return connector.start(request, address, &self.limits);
        }
        let tls = if request.url().scheme() == "https" {
            Some(self.tls().ok_or(TransportError::TlsUnavailable)?)
        } else {
            None
        };
        let cancel = CancellationToken::default();
        let (producer, consumer) = pipe(self.limits.buffer_window);
        let run = Run {
            tls,
            limits: self.limits.clone(),
            cancel: cancel.clone(),
            producer,
            deadline: self
                .limits
                .total_timeout
                .map(|total| Instant::now() + total),
        };
        std::thread::Builder::new()
            .name("rustic-net exchange".to_owned())
            .spawn(move || run.run(&request, address))
            .map_err(|error| TransportError::Io(error.kind()))?;
        Ok(Exchange {
            cancel,
            consumer,
            ended: Cell::new(false),
            hooks: None,
        })
    }
}

/// What an exchange reports, in order: one [`NetEvent::Response`], any
/// number of [`NetEvent::Body`] pieces, then [`NetEvent::Finished`]; or a
/// [`NetEvent::Failed`] at any point.
#[derive(Debug, Eq, PartialEq)]
pub enum NetEvent {
    /// The final response head (interim 1xx responses are skipped).
    Response(ResponseHead),
    /// Decoded body bytes, never empty.
    Body(Vec<u8>),
    /// The body is complete.
    Finished,
    /// The exchange stopped. No further events follow.
    Failed(TransportError),
}

/// One running request. Dropping it cancels the request.
pub struct Exchange {
    pub(crate) cancel: CancellationToken,
    pub(crate) consumer: Consumer,
    pub(crate) ended: Cell<bool>,
    /// Told about consumption and cancellation when the exchange runs in
    /// another process.
    pub(crate) hooks: Option<Arc<dyn ExchangeHooks>>,
}

impl Exchange {
    /// Request cancellation. The network thread stops at its next check
    /// (within one `poll_interval` once connected) and reports
    /// [`TransportError::Cancelled`] unless it already finished.
    pub fn cancel(&self) {
        self.cancel.cancel();
        if let Some(hooks) = &self.hooks {
            if !self.ended.get() {
                hooks.cancelled();
            }
        }
    }

    /// The next event, waiting at most `timeout` (`None` waits until one
    /// arrives). Returns `None` on timeout and after the final event.
    pub fn next_event(&self, timeout: Option<Duration>) -> Option<NetEvent> {
        if self.ended.get() {
            return None;
        }
        let event = self
            .consumer
            .next(timeout.map(|timeout| Instant::now() + timeout))?;
        match &event {
            NetEvent::Finished | NetEvent::Failed(_) => self.ended.set(true),
            NetEvent::Body(bytes) => {
                if let Some(hooks) = &self.hooks {
                    hooks.consumed(bytes.len());
                }
            }
            NetEvent::Response(_) => {}
        }
        Some(event)
    }

    /// Bump `signal` whenever this exchange receives an event, so one
    /// thread can wait on many exchanges ([`crate::fetch::FetchController`]).
    pub fn attach(&self, signal: Arc<crate::pipe::Signal>) {
        self.consumer.attach(signal);
    }

    /// An exchange that delivers `body` and finishes, for a response
    /// served from the HTTP cache (its head is returned separately).
    pub(crate) fn from_body(body: Vec<u8>) -> Self {
        let (producer, consumer) = pipe(body.len().max(1));
        // An empty window accepts one body event, and control events are
        // never held back, so neither push waits.
        if !body.is_empty() {
            let _ = producer.push(NetEvent::Body(body), Duration::ZERO, || true);
        }
        let _ = producer.push(NetEvent::Finished, Duration::ZERO, || true);
        Self {
            cancel: CancellationToken::default(),
            consumer,
            ended: Cell::new(false),
            hooks: None,
        }
    }

    /// Body bytes buffered now, and the most ever buffered.
    pub fn buffered_bytes(&self) -> (usize, usize) {
        self.consumer.buffered()
    }

    /// Wait for the whole response.
    pub fn collect(self) -> Result<(ResponseHead, Vec<u8>), TransportError> {
        let mut head = None;
        let mut body = Vec::new();
        while let Some(event) = self.next_event(None) {
            match event {
                NetEvent::Response(response) => head = Some(response),
                NetEvent::Body(bytes) => body.extend_from_slice(&bytes),
                NetEvent::Finished => break,
                NetEvent::Failed(error) => return Err(error),
            }
        }
        head.map(|head| (head, body))
            .ok_or(TransportError::Protocol(ProtocolError::Truncated))
    }
}

impl Drop for Exchange {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl fmt::Debug for Exchange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Exchange")
            .field("ended", &self.ended.get())
            .finish_non_exhaustive()
    }
}

/// Which time limit ended an exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeoutKind {
    /// No socket progress for `idle_timeout`.
    Idle,
    /// The whole exchange exceeded `total_timeout`.
    Total,
}

/// Why an exchange did not complete. Errors carry no URL or content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportError {
    /// The origin is not in the [`NetworkPolicy`].
    Blocked,
    /// No TLS configuration is available for `https` (the platform
    /// verifier could not be created and none was given).
    TlsUnavailable,
    /// The server certificate was refused. There is no override.
    Certificate(CertificateError),
    /// The TLS handshake or record layer failed for another reason.
    Tls,
    /// The connection could not be established.
    Connect(io::ErrorKind),
    /// Connecting took longer than `connect_timeout`.
    ConnectTimeout,
    /// A socket operation failed after connecting.
    Io(io::ErrorKind),
    /// A time limit was reached.
    Timeout(TimeoutKind),
    /// [`Exchange::cancel`] was called or the exchange was dropped.
    Cancelled,
    /// The response violated HTTP/1.1 or a [`TransportLimits`] bound.
    Protocol(ProtocolError),
    /// The network process that ran the exchange stopped, broke the
    /// protocol or could not be started (ISO-07). The request is not
    /// retried.
    NetworkProcess,
    /// The host name of an allowed origin did not resolve.
    NameNotResolved,
    /// The host resolved only to loopback, private or other non-public
    /// addresses, which a resolved origin never reaches.
    NotPublic,
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blocked => formatter.write_str("origin not allowed by the network policy"),
            Self::TlsUnavailable => formatter.write_str("no TLS configuration is available"),
            Self::Certificate(error) => error.fmt(formatter),
            Self::Tls => formatter.write_str("TLS handshake failed"),
            Self::Connect(kind) => write!(formatter, "connection failed: {kind}"),
            Self::ConnectTimeout => formatter.write_str("connection timed out"),
            Self::Io(kind) => write!(formatter, "network error: {kind}"),
            Self::Timeout(TimeoutKind::Idle) => {
                formatter.write_str("no network progress before the idle timeout")
            }
            Self::Timeout(TimeoutKind::Total) => {
                formatter.write_str("request exceeded its total timeout")
            }
            Self::Cancelled => formatter.write_str("request cancelled"),
            Self::Protocol(error) => error.fmt(formatter),
            Self::NetworkProcess => formatter.write_str("the network process stopped"),
            Self::NameNotResolved => formatter.write_str("the host name did not resolve"),
            Self::NotPublic => formatter.write_str(
                "the host resolved only to local or private addresses, which remote origins never reach",
            ),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<ProtocolError> for TransportError {
    fn from(error: ProtocolError) -> Self {
        Self::Protocol(error)
    }
}

/// The network thread's state for one exchange.
struct Run {
    tls: Option<TlsConfig>,
    limits: TransportLimits,
    cancel: CancellationToken,
    producer: Producer,
    deadline: Option<Instant>,
}

impl Run {
    fn run(self, request: &Request, address: SocketAddr) {
        let end = match self.exchange(request, address) {
            Ok(()) => NetEvent::Finished,
            Err(error) => NetEvent::Failed(error),
        };
        // Control events never wait for the window; a gone consumer is fine.
        let _ = self.producer.push(end, self.limits.poll_interval, || true);
    }

    fn exchange(&self, request: &Request, address: SocketAddr) -> Result<(), TransportError> {
        self.check_stopped()?;
        let mut connect_for = self.limits.connect_timeout;
        if let Some(deadline) = self.deadline {
            connect_for = connect_for.min(deadline.saturating_duration_since(Instant::now()));
            if connect_for.is_zero() {
                return Err(TransportError::Timeout(TimeoutKind::Total));
            }
        }
        let mut stream = TcpStream::connect_timeout(&address, connect_for).map_err(|error| {
            if matches!(
                error.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ) {
                if self.past_deadline() {
                    TransportError::Timeout(TimeoutKind::Total)
                } else {
                    TransportError::ConnectTimeout
                }
            } else {
                TransportError::Connect(error.kind())
            }
        })?;
        let tick = self.limits.poll_interval;
        let setup = |error: io::Error| TransportError::Io(error.kind());
        stream.set_nodelay(true).map_err(setup)?;
        stream.set_read_timeout(Some(tick)).map_err(setup)?;
        stream.set_write_timeout(Some(tick)).map_err(setup)?;
        let result = match &self.tls {
            None => self.over(&mut stream, request),
            Some(tls) => {
                let connection = tls
                    .connect(request.url())
                    .map_err(TransportError::Certificate)?;
                let mut secure = rustls::StreamOwned::new(connection, stream);
                let result = self
                    .handshake(&mut secure)
                    .and_then(|()| self.over(&mut secure, request));
                secure.conn.send_close_notify();
                let _ = secure.flush();
                stream = secure.sock;
                result
            }
        };
        let _ = stream.shutdown(Shutdown::Both);
        result
    }

    /// Complete the TLS handshake, honouring cancellation and timeouts.
    /// End of stream during the handshake is a TLS failure, and so is a
    /// reset: Windows resets a connection its peer closed with our
    /// ClientHello unread, where Unix reports end of stream.
    fn handshake(
        &self,
        stream: &mut rustls::StreamOwned<rustls::ClientConnection, TcpStream>,
    ) -> Result<(), TransportError> {
        let mut last_progress = Instant::now();
        while stream.conn.is_handshaking() {
            match stream.conn.complete_io(&mut stream.sock) {
                Ok((0, 0)) => return Err(TransportError::Tls),
                Ok(_) => last_progress = Instant::now(),
                Err(error) if waits(&error) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::UnexpectedEof
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::ConnectionAborted
                    ) =>
                {
                    return Err(TransportError::Tls);
                }
                Err(error) => return Err(io_error(error)),
            }
            self.check_idle(last_progress)?;
        }
        Ok(())
    }

    fn over(
        &self,
        stream: &mut (impl Read + Write),
        request: &Request,
    ) -> Result<(), TransportError> {
        let io = io_error;
        self.check_stopped()?;
        self.write_all(stream, &request.serialize())?;

        let mut head_parser = HeadParser::new(&self.limits);
        let mut body: Option<BodyDecoder> = None;
        let mut content: Option<ContentDecoder> = None;
        let mut received_any = false;
        let mut buffer = vec![0; self.limits.read_chunk];
        let mut decoded = Vec::new();
        let mut last_progress = Instant::now();
        loop {
            let read = match stream.read(&mut buffer) {
                Ok(read) => read,
                Err(error) if waits(&error) => {
                    self.check_idle(last_progress)?;
                    continue;
                }
                // A TLS peer that closes without close_notify: treated as
                // end of stream, as browsers do; framing still detects
                // truncated length-delimited and chunked bodies.
                Err(error)
                    if self.tls.is_some() && error.kind() == io::ErrorKind::UnexpectedEof =>
                {
                    0
                }
                Err(error) => return Err(io(error)),
            };
            if read == 0 {
                return match &mut body {
                    Some(decoder) => {
                        decoder.finish()?;
                        self.finish_content(content.as_mut())
                    }
                    None if received_any => Err(ProtocolError::Truncated.into()),
                    None => Err(ProtocolError::Empty.into()),
                };
            }
            received_any = true;
            last_progress = Instant::now();
            let input = &buffer[..read];
            decoded.clear();
            match &mut body {
                Some(decoder) => decoder.feed(input, &mut decoded)?,
                None => {
                    if let Some((head, rest)) = head_parser.feed(input)? {
                        let body_framing = framing(&head, request.is_head())?;
                        if body_framing != crate::response::Framing::Empty {
                            content = ContentDecoder::for_response(
                                head.headers(),
                                self.limits.max_body_bytes,
                            );
                        }
                        let mut decoder = BodyDecoder::new(body_framing, &self.limits);
                        self.emit(NetEvent::Response(head))?;
                        decoder.feed(&rest, &mut decoded)?;
                        body = Some(decoder);
                    }
                }
            }
            if let Some(content) = &mut content {
                let wire = std::mem::take(&mut decoded);
                content.feed(&wire, &mut decoded)?;
            }
            if !decoded.is_empty() {
                self.emit(NetEvent::Body(std::mem::take(&mut decoded)))?;
                // Waiting for the consumer is not idleness of the peer.
                last_progress = Instant::now();
            }
            self.check_stopped()?;
            if body.as_ref().is_some_and(BodyDecoder::is_done) {
                return self.finish_content(content.as_mut());
            }
        }
    }

    /// Flush the content decoder at the end of the body.
    fn finish_content(&self, content: Option<&mut ContentDecoder>) -> Result<(), TransportError> {
        let Some(content) = content else {
            return Ok(());
        };
        let mut rest = Vec::new();
        content.finish(&mut rest)?;
        for piece in rest.chunks(self.limits.read_chunk.max(1)) {
            self.emit(NetEvent::Body(piece.to_vec()))?;
        }
        Ok(())
    }

    fn write_all(&self, stream: &mut impl Write, bytes: &[u8]) -> Result<(), TransportError> {
        let mut written = 0;
        let mut last_progress = Instant::now();
        while written < bytes.len() {
            match stream.write(&bytes[written..]) {
                Ok(0) => return Err(TransportError::Io(io::ErrorKind::WriteZero)),
                Ok(count) => {
                    written += count;
                    last_progress = Instant::now();
                }
                Err(error) if waits(&error) => {}
                Err(error) => return Err(io_error(error)),
            }
            self.check_idle(last_progress)?;
        }
        loop {
            match stream.flush() {
                Ok(()) => return Ok(()),
                Err(error) if waits(&error) => self.check_idle(last_progress)?,
                Err(error) => return Err(io_error(error)),
            }
        }
    }

    fn emit(&self, event: NetEvent) -> Result<(), TransportError> {
        self.producer
            .push(event, self.limits.poll_interval, || {
                self.cancel.is_cancelled() || self.past_deadline()
            })
            .map_err(|wait| match wait {
                Wait::ConsumerGone => TransportError::Cancelled,
                Wait::Stopped => self
                    .check_stopped()
                    .err()
                    .unwrap_or(TransportError::Cancelled),
            })
    }

    fn past_deadline(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn check_stopped(&self) -> Result<(), TransportError> {
        if self.cancel.is_cancelled() || self.producer.consumer_gone() {
            Err(TransportError::Cancelled)
        } else if self.past_deadline() {
            Err(TransportError::Timeout(TimeoutKind::Total))
        } else {
            Ok(())
        }
    }

    fn check_idle(&self, last_progress: Instant) -> Result<(), TransportError> {
        self.check_stopped()?;
        if last_progress.elapsed() >= self.limits.idle_timeout {
            return Err(TransportError::Timeout(TimeoutKind::Idle));
        }
        Ok(())
    }
}

/// Map a socket or TLS error.
fn io_error(error: io::Error) -> TransportError {
    match classify(&error) {
        Some(TlsFailure::Certificate(reason)) => TransportError::Certificate(reason),
        Some(TlsFailure::Handshake) => TransportError::Tls,
        None => TransportError::Io(error.kind()),
    }
}

/// A socket timeout or interruption: check limits and try again.
fn waits(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}
