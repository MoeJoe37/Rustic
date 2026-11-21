//! Exchanges that run in another process (ISO-07).
//!
//! A [`Connector`] given to [`Transport::with_connector`](crate::Transport::with_connector)
//! receives each request after the [`NetworkPolicy`](crate::NetworkPolicy)
//! has granted it, together with the granted address, and returns an
//! [`Exchange`] fed by a [`RemoteFeed`]. The process that runs the socket,
//! TLS, HTTP framing and content decoding is treated as a hostile peer:
//! response heads are revalidated by [`ResponseHead::from_parts`](crate::ResponseHead::from_parts),
//! body bytes may never exceed the credit the consumer has returned
//! through [`ExchangeHooks::consumed`] (so a peer that ignores flow
//! control is detected instead of growing memory), and events after the
//! final one are refused.

use std::cell::Cell;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustic_base::CancellationToken;

use crate::pipe::{Producer, Wait, pipe};
use crate::tls::CertificateError;
use crate::{
    Exchange, NetEvent, ProtocolError, Request, TimeoutKind, TransportError, TransportLimits,
};

/// Runs granted requests somewhere else, typically a network process.
pub trait Connector: Send + Sync + fmt::Debug {
    /// Start `request` against `address`, which the policy granted for
    /// its origin, under `limits`.
    fn start(
        &self,
        request: Request,
        address: SocketAddr,
        limits: &TransportLimits,
    ) -> Result<Exchange, TransportError>;
}

/// What a remote exchange tells its connector.
pub trait ExchangeHooks: Send + Sync {
    /// The consumer took `bytes` body bytes: the peer may send that many
    /// more.
    fn consumed(&self, bytes: usize);
    /// The consumer cancelled or dropped the exchange before its final
    /// event. May be called more than once.
    fn cancelled(&self);
}

/// Why a [`RemoteFeed`] refused an event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedError {
    /// Body bytes beyond the window the consumer has granted.
    CreditExceeded,
    /// An event out of order: a body before the head, a second head, or
    /// anything after the final event.
    OutOfOrder,
    /// The consumer is gone; drop the feed.
    ConsumerGone,
}

impl fmt::Display for FeedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CreditExceeded => "body bytes beyond the granted window",
            Self::OutOfOrder => "network event out of order",
            Self::ConsumerGone => "the exchange was dropped",
        })
    }
}

impl std::error::Error for FeedError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stage {
    Head,
    Body,
    Ended,
}

/// The producing end of a remote [`Exchange`]. Pushing never waits.
pub struct RemoteFeed {
    producer: Producer,
    window: usize,
    stage: Cell<Stage>,
}

impl fmt::Debug for RemoteFeed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteFeed")
            .field("window", &self.window)
            .field("stage", &self.stage.get())
            .finish_non_exhaustive()
    }
}

impl RemoteFeed {
    /// Queue the next event from the peer.
    ///
    /// A body event must fit the window next to what the consumer has not
    /// yet taken, and arrive between the head and the final event. On an
    /// error nothing is queued; the caller should end the exchange with
    /// [`RemoteFeed::fail`] and treat the peer as broken.
    pub fn push(&self, event: NetEvent) -> Result<(), FeedError> {
        let stage = self.stage.get();
        let next = match (&event, stage) {
            (NetEvent::Response(_), Stage::Head) => Stage::Body,
            (NetEvent::Body(bytes), Stage::Body) => {
                if bytes.is_empty() || bytes.len() > self.window {
                    return Err(FeedError::CreditExceeded);
                }
                Stage::Body
            }
            (NetEvent::Finished, Stage::Body)
            | (NetEvent::Failed(_), Stage::Head | Stage::Body) => Stage::Ended,
            _ => return Err(FeedError::OutOfOrder),
        };
        if let NetEvent::Body(bytes) = &event {
            let (buffered, _) = self.producer.buffered();
            if buffered + bytes.len() > self.window {
                return Err(FeedError::CreditExceeded);
            }
        }
        // The window check above means a body push never waits; control
        // events never wait.
        match self.producer.push(event, Duration::ZERO, || true) {
            Ok(()) => {
                self.stage.set(next);
                Ok(())
            }
            Err(Wait::ConsumerGone) => Err(FeedError::ConsumerGone),
            Err(Wait::Stopped) => Err(FeedError::CreditExceeded),
        }
    }

    /// End the exchange with `error` unless it has already ended.
    pub fn fail(&self, error: TransportError) {
        if self.stage.get() != Stage::Ended {
            self.stage.set(Stage::Ended);
            let _ = self
                .producer
                .push(NetEvent::Failed(error), Duration::ZERO, || true);
        }
    }

    /// Whether the final event has been queued.
    pub fn ended(&self) -> bool {
        self.stage.get() == Stage::Ended
    }
}

/// A new remote exchange with a body window of `window` bytes (at least
/// one), reporting to `hooks`.
pub fn remote_exchange(window: usize, hooks: Arc<dyn ExchangeHooks>) -> (RemoteFeed, Exchange) {
    let window = window.max(1);
    let (producer, consumer) = pipe(window);
    let feed = RemoteFeed {
        producer,
        window,
        stage: Cell::new(Stage::Head),
    };
    let exchange = Exchange {
        cancel: CancellationToken::default(),
        consumer,
        ended: Cell::new(false),
        hooks: Some(hooks),
    };
    (feed, exchange)
}

/// I/O error kinds that cross a process boundary by index; anything
/// else becomes `Other`.
const IO_KINDS: [io::ErrorKind; 14] = [
    io::ErrorKind::Other,
    io::ErrorKind::NotFound,
    io::ErrorKind::PermissionDenied,
    io::ErrorKind::ConnectionRefused,
    io::ErrorKind::ConnectionReset,
    io::ErrorKind::ConnectionAborted,
    io::ErrorKind::NotConnected,
    io::ErrorKind::AddrInUse,
    io::ErrorKind::AddrNotAvailable,
    io::ErrorKind::BrokenPipe,
    io::ErrorKind::TimedOut,
    io::ErrorKind::UnexpectedEof,
    io::ErrorKind::Interrupted,
    io::ErrorKind::WouldBlock,
];

const CERTIFICATE_ERRORS: [CertificateError; 6] = [
    CertificateError::UnknownIssuer,
    CertificateError::Expired,
    CertificateError::NotValidForName,
    CertificateError::Revoked,
    CertificateError::Invalid,
    CertificateError::Other,
];

const PROTOCOL_ERRORS: [ProtocolError; 13] = [
    ProtocolError::InvalidStatusLine,
    ProtocolError::InvalidHeader,
    ProtocolError::HeadTooLarge,
    ProtocolError::TooManyInterimResponses,
    ProtocolError::UnexpectedUpgrade,
    ProtocolError::InvalidContentLength,
    ProtocolError::UnsupportedTransferCoding,
    ProtocolError::InvalidChunk,
    ProtocolError::InvalidTrailer,
    ProtocolError::BodyTooLarge,
    ProtocolError::InvalidContentCoding,
    ProtocolError::Truncated,
    ProtocolError::Empty,
];

fn index_of<T: PartialEq>(table: &[T], value: &T) -> u16 {
    table
        .iter()
        .position(|entry| entry == value)
        .and_then(|index| u16::try_from(index).ok())
        .unwrap_or(0)
}

/// A transport error as two numbers, for [`error_from_code`] in another
/// process.
pub fn error_code(error: TransportError) -> (u16, u16) {
    match error {
        TransportError::Blocked => (1, 0),
        TransportError::TlsUnavailable => (2, 0),
        TransportError::Certificate(kind) => (3, index_of(&CERTIFICATE_ERRORS, &kind)),
        TransportError::Tls => (4, 0),
        TransportError::Connect(kind) => (5, index_of(&IO_KINDS, &kind)),
        TransportError::ConnectTimeout => (6, 0),
        TransportError::Io(kind) => (7, index_of(&IO_KINDS, &kind)),
        TransportError::Timeout(TimeoutKind::Idle) => (8, 0),
        TransportError::Timeout(TimeoutKind::Total) => (8, 1),
        TransportError::Cancelled => (9, 0),
        TransportError::Protocol(kind) => (10, index_of(&PROTOCOL_ERRORS, &kind)),
        TransportError::NetworkProcess => (11, 0),
    }
}

/// The transport error [`error_code`] encoded, or `None` for numbers it
/// never produces (a broken peer).
pub fn error_from_code(code: u16, detail: u16) -> Option<TransportError> {
    let detail = usize::from(detail);
    let plain = |error| (detail == 0).then_some(error);
    match code {
        1 => plain(TransportError::Blocked),
        2 => plain(TransportError::TlsUnavailable),
        3 => CERTIFICATE_ERRORS
            .get(detail)
            .map(|&kind| TransportError::Certificate(kind)),
        4 => plain(TransportError::Tls),
        5 => IO_KINDS
            .get(detail)
            .map(|&kind| TransportError::Connect(kind)),
        6 => plain(TransportError::ConnectTimeout),
        7 => IO_KINDS.get(detail).map(|&kind| TransportError::Io(kind)),
        8 => match detail {
            0 => Some(TransportError::Timeout(TimeoutKind::Idle)),
            1 => Some(TransportError::Timeout(TimeoutKind::Total)),
            _ => None,
        },
        9 => plain(TransportError::Cancelled),
        10 => PROTOCOL_ERRORS
            .get(detail)
            .map(|&kind| TransportError::Protocol(kind)),
        11 => plain(TransportError::NetworkProcess),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::{Headers, HttpVersion, ResponseHead};

    #[derive(Default)]
    struct Recorder {
        consumed: Mutex<usize>,
        cancelled: Mutex<usize>,
    }

    impl ExchangeHooks for Recorder {
        fn consumed(&self, bytes: usize) {
            *self.consumed.lock().unwrap() += bytes;
        }
        fn cancelled(&self) {
            *self.cancelled.lock().unwrap() += 1;
        }
    }

    fn head() -> ResponseHead {
        ResponseHead::from_parts(HttpVersion::Http11, 200, b"OK".to_vec(), Headers::new()).unwrap()
    }

    #[test]
    fn events_flow_in_order_and_consumption_is_reported() {
        let hooks = Arc::new(Recorder::default());
        let (feed, exchange) = remote_exchange(8, hooks.clone());
        feed.push(NetEvent::Response(head())).unwrap();
        feed.push(NetEvent::Body(b"abcd".to_vec())).unwrap();
        feed.push(NetEvent::Body(b"efgh".to_vec())).unwrap();
        assert_eq!(
            feed.push(NetEvent::Body(b"i".to_vec())),
            Err(FeedError::CreditExceeded),
            "a ninth byte overruns the window"
        );
        assert!(
            matches!(exchange.next_event(None), Some(NetEvent::Response(h)) if h.status() == 200)
        );
        assert_eq!(
            exchange.next_event(None),
            Some(NetEvent::Body(b"abcd".to_vec()))
        );
        assert_eq!(*hooks.consumed.lock().unwrap(), 4);
        feed.push(NetEvent::Body(b"ijkl".to_vec())).unwrap();
        feed.push(NetEvent::Finished).unwrap();
        assert!(feed.ended());
        assert_eq!(
            exchange.next_event(None),
            Some(NetEvent::Body(b"efgh".to_vec()))
        );
        assert_eq!(
            exchange.next_event(None),
            Some(NetEvent::Body(b"ijkl".to_vec()))
        );
        assert_eq!(exchange.next_event(None), Some(NetEvent::Finished));
        assert_eq!(*hooks.consumed.lock().unwrap(), 12);
        assert_eq!(
            *hooks.cancelled.lock().unwrap(),
            0,
            "a finished exchange is not cancelled"
        );
    }

    #[test]
    fn out_of_order_events_are_refused() {
        let (feed, _exchange) = remote_exchange(8, Arc::new(Recorder::default()));
        assert_eq!(
            feed.push(NetEvent::Body(b"x".to_vec())),
            Err(FeedError::OutOfOrder)
        );
        assert_eq!(feed.push(NetEvent::Finished), Err(FeedError::OutOfOrder));
        feed.push(NetEvent::Response(head())).unwrap();
        assert_eq!(
            feed.push(NetEvent::Response(head())),
            Err(FeedError::OutOfOrder)
        );
        assert_eq!(
            feed.push(NetEvent::Body(Vec::new())),
            Err(FeedError::CreditExceeded)
        );
        feed.push(NetEvent::Failed(TransportError::Tls)).unwrap();
        assert_eq!(feed.push(NetEvent::Finished), Err(FeedError::OutOfOrder));
        feed.fail(TransportError::NetworkProcess);
        assert!(feed.ended());
    }

    #[test]
    fn dropping_an_unfinished_exchange_reports_cancellation() {
        let hooks = Arc::new(Recorder::default());
        let (feed, exchange) = remote_exchange(8, hooks.clone());
        feed.push(NetEvent::Response(head())).unwrap();
        drop(exchange);
        assert!(*hooks.cancelled.lock().unwrap() >= 1);
        assert_eq!(feed.push(NetEvent::Finished), Err(FeedError::ConsumerGone));
    }

    #[test]
    fn every_transport_error_survives_its_code() {
        let mut errors = vec![
            TransportError::Blocked,
            TransportError::TlsUnavailable,
            TransportError::Tls,
            TransportError::ConnectTimeout,
            TransportError::Timeout(TimeoutKind::Idle),
            TransportError::Timeout(TimeoutKind::Total),
            TransportError::Cancelled,
            TransportError::NetworkProcess,
        ];
        errors.extend(CERTIFICATE_ERRORS.map(TransportError::Certificate));
        errors.extend(IO_KINDS.map(TransportError::Connect));
        errors.extend(IO_KINDS.map(TransportError::Io));
        errors.extend(PROTOCOL_ERRORS.map(TransportError::Protocol));
        for error in errors {
            let (code, detail) = error_code(error);
            assert_eq!(error_from_code(code, detail), Some(error));
        }
        assert_eq!(
            error_code(TransportError::Io(io::ErrorKind::OutOfMemory)),
            error_code(TransportError::Io(io::ErrorKind::Other)),
            "unlisted I/O kinds travel as Other"
        );
        for (code, detail) in [(0, 0), (1, 1), (3, 6), (8, 2), (12, 0), (u16::MAX, 0)] {
            assert_eq!(error_from_code(code, detail), None);
        }
    }

    #[test]
    fn a_failed_feed_ends_the_exchange() {
        let (feed, exchange) = remote_exchange(8, Arc::new(Recorder::default()));
        feed.fail(TransportError::NetworkProcess);
        assert_eq!(
            exchange.next_event(Some(Duration::from_secs(1))),
            Some(NetEvent::Failed(TransportError::NetworkProcess))
        );
        assert_eq!(exchange.next_event(Some(Duration::ZERO)), None);
    }
}
