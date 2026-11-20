use std::cell::Cell;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustic_base::CancellationToken;

use crate::pipe::{Consumer, Producer, Wait, pipe};
use crate::response::{BodyDecoder, HeadParser, ProtocolError, ResponseHead, framing};
use crate::{LimitsError, NetworkPolicy, Request, TransportLimits};

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
}

impl Transport {
    /// A transport over a fixed policy. The limits are validated.
    pub fn new(policy: NetworkPolicy, limits: TransportLimits) -> Result<Self, LimitsError> {
        limits.validate()?;
        Ok(Self {
            policy: Arc::new(policy),
            limits,
        })
    }

    /// The limits every exchange uses.
    pub fn limits(&self) -> &TransportLimits {
        &self.limits
    }

    /// Start an exchange. Policy and scheme failures are reported here,
    /// before any socket is opened; everything later arrives as a
    /// [`NetEvent::Failed`].
    ///
    /// `https` fails with [`TransportError::TlsUnavailable`] until the
    /// TLS adapter of ADR 0051 lands. It is never retried over `http`.
    pub fn start(&self, request: Request) -> Result<Exchange, TransportError> {
        let address = self
            .policy
            .route(request.url())
            .ok_or(TransportError::Blocked)?;
        if request.url().scheme() == "https" {
            return Err(TransportError::TlsUnavailable);
        }
        let cancel = CancellationToken::default();
        let (producer, consumer) = pipe(self.limits.buffer_window);
        let run = Run {
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
    cancel: CancellationToken,
    consumer: Consumer,
    ended: Cell<bool>,
}

impl Exchange {
    /// Request cancellation. The network thread stops at its next check
    /// (within one `poll_interval` once connected) and reports
    /// [`TransportError::Cancelled`] unless it already finished.
    pub fn cancel(&self) {
        self.cancel.cancel();
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
        if matches!(event, NetEvent::Finished | NetEvent::Failed(_)) {
            self.ended.set(true);
        }
        Some(event)
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
        self.cancel.cancel();
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
    /// `https` needs the TLS adapter, which is not built yet (ADR 0051).
    TlsUnavailable,
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
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blocked => formatter.write_str("origin not allowed by the network policy"),
            Self::TlsUnavailable => formatter.write_str("https is not available yet"),
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
        let result = self.over(&mut stream, request);
        let _ = stream.shutdown(Shutdown::Both);
        result
    }

    fn over(&self, stream: &mut TcpStream, request: &Request) -> Result<(), TransportError> {
        let tick = self.limits.poll_interval;
        let io = |error: io::Error| TransportError::Io(error.kind());
        stream.set_nodelay(true).map_err(io)?;
        stream.set_read_timeout(Some(tick)).map_err(io)?;
        stream.set_write_timeout(Some(tick)).map_err(io)?;
        self.check_stopped()?;
        self.write_all(stream, &request.serialize())?;

        let mut head_parser = HeadParser::new(&self.limits);
        let mut body: Option<BodyDecoder> = None;
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
                Err(error) => return Err(io(error)),
            };
            if read == 0 {
                return match &mut body {
                    Some(decoder) => {
                        decoder.finish()?;
                        Ok(())
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
                        let mut decoder =
                            BodyDecoder::new(framing(&head, request.is_head())?, &self.limits);
                        self.emit(NetEvent::Response(head))?;
                        decoder.feed(&rest, &mut decoded)?;
                        body = Some(decoder);
                    }
                }
            }
            if !decoded.is_empty() {
                self.emit(NetEvent::Body(std::mem::take(&mut decoded)))?;
                // Waiting for the consumer is not idleness of the peer.
                last_progress = Instant::now();
            }
            self.check_stopped()?;
            if body.as_ref().is_some_and(BodyDecoder::is_done) {
                return Ok(());
            }
        }
    }

    fn write_all(&self, stream: &mut TcpStream, bytes: &[u8]) -> Result<(), TransportError> {
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
                Err(error) => return Err(TransportError::Io(error.kind())),
            }
            self.check_idle(last_progress)?;
        }
        Ok(())
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

/// A socket timeout or interruption: check limits and try again.
fn waits(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}
