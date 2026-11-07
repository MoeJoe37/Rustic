use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A shared, one-way cancellation signal; clones observe the same state.
///
/// Work must call [`Self::check`] at bounded checkpoints. This does not interrupt
/// blocked calls, provide an execution timeout, or publish other shared state.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// Signal cancellation permanently for this token and all its clones.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Stop a cooperative work checkpoint if cancellation was requested.
    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Cooperative work was cancelled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("operation cancelled")
    }
}

impl std::error::Error for Cancelled {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_cancellation_across_threads_but_other_tokens_do_not() {
        let token = CancellationToken::default();
        let other = CancellationToken::default();
        token.check().unwrap();
        let clone = token.clone();
        std::thread::spawn(move || clone.cancel()).join().unwrap();
        assert_eq!(token.check(), Err(Cancelled));
        token.cancel();
        assert_eq!(token.check(), Err(Cancelled));
        other.check().unwrap();
    }
}
