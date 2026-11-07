use std::cell::Cell;
use std::fmt;
use std::time::{Duration, Instant};

/// Monotonic elapsed time relative to the clock's own origin, not civil time.
pub trait Clock {
    /// Read elapsed time without changing the clock.
    fn now(&self) -> Duration;
}

/// A production monotonic clock whose origin is construction time.
#[derive(Debug)]
pub struct SystemClock {
    origin: Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// An agent-local deterministic clock advanced explicitly by a test or host.
///
/// It is deliberately not shared through locks. Separate agents can receive
/// their own clocks and explicit scheduler messages when that subsystem exists.
#[derive(Debug, Default)]
pub struct ManualClock {
    elapsed: Cell<Duration>,
}

impl ManualClock {
    /// Advance without sleeping; an overflow leaves the time unchanged.
    pub fn advance(&self, amount: Duration) -> Result<(), ClockOverflow> {
        let elapsed = self
            .elapsed
            .get()
            .checked_add(amount)
            .ok_or(ClockOverflow)?;
        self.elapsed.set(elapsed);
        Ok(())
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        self.elapsed.get()
    }
}

/// An attempted clock advance exceeded the duration representation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockOverflow;

impl fmt::Display for ClockOverflow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("monotonic clock range exceeded")
    }
}

impl std::error::Error for ClockOverflow {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advances_deterministically_and_preserves_time_on_overflow() {
        let clock = ManualClock::default();
        assert_eq!(clock.now(), Duration::ZERO);
        clock.advance(Duration::from_millis(25)).unwrap();
        assert_eq!(clock.now(), Duration::from_millis(25));
        assert_eq!(clock.advance(Duration::MAX), Err(ClockOverflow));
        assert_eq!(clock.now(), Duration::from_millis(25));
    }
}
