//! The bounded hand-off between an exchange's network thread and its
//! consumer. Body bytes count against a window; the producer waits while
//! the window is full, which stops socket reads and lets TCP flow control
//! push back on the peer.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::NetEvent;

struct State {
    queue: VecDeque<NetEvent>,
    /// Body bytes in `queue`.
    buffered: usize,
    /// Highest `buffered` value reached, for tests and diagnostics.
    peak: usize,
    consumer_gone: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    window: usize,
    /// Told about every event pushed, for a consumer that waits on many
    /// pipes at once.
    signal: Mutex<Option<Arc<Signal>>>,
}

/// A wake-up shared by several pipes: a counter bumped on every event
/// any of them receives.
#[derive(Default)]
pub struct Signal {
    count: Mutex<u64>,
    bumped: Condvar,
}

impl Signal {
    /// The number of events seen so far.
    pub fn count(&self) -> u64 {
        *self.count.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn bump(&self) {
        *self.count.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        self.bumped.notify_all();
    }

    /// Wait until the count passes `seen` or `deadline` passes; whether
    /// it moved.
    pub fn wait(&self, seen: u64, deadline: Instant) -> bool {
        let mut count = self.count.lock().unwrap_or_else(PoisonError::into_inner);
        while *count == seen {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            count = self
                .bumped
                .wait_timeout(count, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

impl std::fmt::Debug for Signal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Signal")
            .field("count", &self.count())
            .finish()
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock cannot leave `State` inconsistent:
        // every update is a single push or pop with its byte count.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub(crate) fn pipe(window: usize) -> (Producer, Consumer) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            queue: VecDeque::new(),
            buffered: 0,
            peak: 0,
            consumer_gone: false,
        }),
        changed: Condvar::new(),
        window,
        signal: Mutex::new(None),
    });
    (Producer(Arc::clone(&shared)), Consumer(shared))
}

/// Why the producer stopped waiting.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Wait {
    /// The consumer was dropped; nothing will read further events.
    ConsumerGone,
    /// `stop` returned true while the window was full.
    Stopped,
}

pub(crate) struct Producer(Arc<Shared>);

impl Producer {
    /// Queue an event. A body event waits while the window is full; one body
    /// event is always accepted into an empty window, so a chunk larger
    /// than the window still progresses. `stop` is polled every `tick`.
    pub(crate) fn push(
        &self,
        event: NetEvent,
        tick: Duration,
        mut stop: impl FnMut() -> bool,
    ) -> Result<(), Wait> {
        let size = match &event {
            NetEvent::Body(bytes) => bytes.len(),
            _ => 0,
        };
        let mut state = self.0.lock();
        loop {
            if state.consumer_gone {
                return Err(Wait::ConsumerGone);
            }
            if size == 0 || state.buffered == 0 || state.buffered + size <= self.0.window {
                break;
            }
            if stop() {
                return Err(Wait::Stopped);
            }
            state = self
                .0
                .changed
                .wait_timeout(state, tick)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        state.buffered += size;
        state.peak = state.peak.max(state.buffered);
        state.queue.push_back(event);
        drop(state);
        self.0.changed.notify_all();
        if let Some(signal) = &*self.0.signal.lock().unwrap_or_else(PoisonError::into_inner) {
            signal.bump();
        }
        Ok(())
    }

    /// Body bytes queued now.
    pub(crate) fn buffered(&self) -> (usize, usize) {
        let state = self.0.lock();
        (state.buffered, state.peak)
    }

    /// Whether the consumer is gone.
    pub(crate) fn consumer_gone(&self) -> bool {
        self.0.lock().consumer_gone
    }
}

pub(crate) struct Consumer(Arc<Shared>);

impl Consumer {
    /// The next event, waiting until `deadline` (or forever with `None`).
    pub(crate) fn next(&self, deadline: Option<Instant>) -> Option<NetEvent> {
        let mut state = self.0.lock();
        loop {
            if let Some(event) = state.queue.pop_front() {
                if let NetEvent::Body(bytes) = &event {
                    state.buffered -= bytes.len();
                }
                drop(state);
                self.0.changed.notify_all();
                return Some(event);
            }
            let wait = match deadline {
                None => Duration::from_secs(3600),
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return None;
                    }
                    deadline - now
                }
            };
            state = self
                .0
                .changed
                .wait_timeout(state, wait)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Bump `signal` on every event from now on; an event already queued
    /// bumps it at once, so a waiter never misses it.
    pub(crate) fn attach(&self, signal: Arc<Signal>) {
        let pending = !self.0.lock().queue.is_empty();
        *self.0.signal.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&signal));
        if pending {
            signal.bump();
        }
    }

    /// Body bytes queued now and the highest number ever queued.
    pub(crate) fn buffered(&self) -> (usize, usize) {
        let state = self.0.lock();
        (state.buffered, state.peak)
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.consumer_gone = true;
        state.queue.clear();
        state.buffered = 0;
        drop(state);
        self.0.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_millis(1);

    fn body(size: usize) -> NetEvent {
        NetEvent::Body(vec![0; size])
    }

    #[test]
    fn window_blocks_body_bytes_but_not_control_events() {
        let (producer, consumer) = pipe(10);
        producer.push(body(30), TICK, || true).unwrap();
        assert_eq!(producer.push(body(1), TICK, || true), Err(Wait::Stopped));
        producer.push(NetEvent::Finished, TICK, || true).unwrap();
        assert_eq!(consumer.buffered(), (30, 30));
        assert!(matches!(consumer.next(None), Some(NetEvent::Body(b)) if b.len() == 30));
        producer.push(body(6), TICK, || true).unwrap();
        producer.push(body(4), TICK, || true).unwrap();
        assert_eq!(producer.push(body(1), TICK, || true), Err(Wait::Stopped));
        assert!(matches!(consumer.next(None), Some(NetEvent::Finished)));
        assert_eq!(consumer.buffered(), (10, 30));
    }

    #[test]
    fn a_waiting_producer_resumes_when_the_consumer_reads() {
        let (producer, consumer) = pipe(4);
        producer.push(body(4), TICK, || false).unwrap();
        let thread = std::thread::spawn(move || {
            producer.push(body(4), TICK, || false).unwrap();
            producer
        });
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(consumer.buffered().0, 4);
        assert!(consumer.next(None).is_some());
        let producer = thread.join().unwrap();
        assert!(consumer.next(None).is_some());
        assert_eq!(consumer.buffered(), (0, 4));
        drop(consumer);
        assert!(producer.consumer_gone());
        assert_eq!(
            producer.push(NetEvent::Finished, TICK, || false),
            Err(Wait::ConsumerGone)
        );
    }

    #[test]
    fn next_honors_its_deadline() {
        let (_producer, consumer) = pipe(4);
        let start = Instant::now();
        assert!(
            consumer
                .next(Some(start + Duration::from_millis(15)))
                .is_none()
        );
        assert!(start.elapsed() >= Duration::from_millis(15));
    }
}
