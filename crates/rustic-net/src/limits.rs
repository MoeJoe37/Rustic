use std::fmt;
use std::time::Duration;

/// Bounds for one HTTP exchange.
///
/// Every value is checked by [`TransportLimits::validate`] when a
/// [`Transport`](crate::Transport) is built, so an exchange never runs with a
/// zero window or a zero polling interval.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransportLimits {
    /// Bytes of status line plus header section, including interim responses.
    /// Chromium's limit (`kMaxHeaderBufSize`) is 256 KiB.
    pub max_head_bytes: usize,
    /// Header field lines in one response head, after unfolding.
    pub max_header_count: usize,
    /// Decoded body bytes delivered for one response.
    pub max_body_bytes: u64,
    /// Bytes of one chunk-size line, including chunk extensions.
    pub max_chunk_line_bytes: usize,
    /// Bytes of the trailer section of a chunked body.
    pub max_trailer_bytes: usize,
    /// Interim (1xx) responses skipped before the final one.
    pub max_interim_responses: usize,
    /// Body bytes buffered for the consumer before the transport stops
    /// reading the socket (backpressure). TCP flow control then slows the
    /// peer.
    pub buffer_window: usize,
    /// Size of one socket read.
    pub read_chunk: usize,
    /// Time allowed to establish the connection.
    pub connect_timeout: Duration,
    /// Time allowed without any socket progress while the transport is
    /// waiting on the peer. Time spent waiting for the consumer does not
    /// count.
    pub idle_timeout: Duration,
    /// Time allowed for the whole exchange, if any.
    pub total_timeout: Option<Duration>,
    /// Longest blocking step between cancellation and deadline checks.
    pub poll_interval: Duration,
}

impl Default for TransportLimits {
    fn default() -> Self {
        Self {
            max_head_bytes: 256 * 1024,
            max_header_count: 512,
            max_body_bytes: 256 * 1024 * 1024,
            max_chunk_line_bytes: 4096,
            max_trailer_bytes: 16 * 1024,
            max_interim_responses: 16,
            buffer_window: 256 * 1024,
            read_chunk: 16 * 1024,
            connect_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(60),
            total_timeout: None,
            poll_interval: Duration::from_millis(10),
        }
    }
}

impl TransportLimits {
    /// Reject limits that would make an exchange unable to progress.
    pub fn validate(&self) -> Result<(), LimitsError> {
        let invalid = [
            (self.max_head_bytes < 16, "max_head_bytes"),
            (self.max_chunk_line_bytes < 3, "max_chunk_line_bytes"),
            (self.buffer_window == 0, "buffer_window"),
            (self.read_chunk == 0, "read_chunk"),
            (self.connect_timeout.is_zero(), "connect_timeout"),
            (self.idle_timeout.is_zero(), "idle_timeout"),
            (self.poll_interval.is_zero(), "poll_interval"),
            (
                self.total_timeout.is_some_and(|total| total.is_zero()),
                "total_timeout",
            ),
        ];
        match invalid.iter().find(|(bad, _)| *bad) {
            Some((_, field)) => Err(LimitsError { field }),
            None => Ok(()),
        }
    }
}

/// A [`TransportLimits`] field cannot make progress.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LimitsError {
    field: &'static str,
}

impl LimitsError {
    /// The rejected field.
    pub fn field(&self) -> &'static str {
        self.field
    }
}

impl fmt::Display for LimitsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "transport limit `{}` is too small", self.field)
    }
}

impl std::error::Error for LimitsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate_and_each_degenerate_field_is_named() {
        TransportLimits::default().validate().unwrap();
        type Mutation = fn(&mut TransportLimits);
        let cases: [(Mutation, &str); 5] = [
            (|l| l.buffer_window = 0, "buffer_window"),
            (|l| l.read_chunk = 0, "read_chunk"),
            (|l| l.poll_interval = Duration::ZERO, "poll_interval"),
            (|l| l.total_timeout = Some(Duration::ZERO), "total_timeout"),
            (|l| l.max_head_bytes = 15, "max_head_bytes"),
        ];
        for (mutate, field) in cases {
            let mut limits = TransportLimits::default();
            mutate(&mut limits);
            assert_eq!(limits.validate().unwrap_err().field(), field);
        }
    }
}
