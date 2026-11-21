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
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustic_base::CancellationToken;

use crate::pipe::{Producer, Wait, pipe};
use crate::{Exchange, NetEvent, Request, TransportError, TransportLimits};

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
