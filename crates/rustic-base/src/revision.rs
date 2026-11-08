use crate::SourceSpan;
use std::fmt;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SOURCE_ID: AtomicU64 = AtomicU64::new(1);

/// A process-local identity for one logical source, such as a document's input.
///
/// It is never reused, including after the source is dropped. It is a label for
/// matching diagnostics to buffers, not an authorization capability.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceId(NonZeroU64);

impl SourceId {
    /// Allocate a unique identity, failing permanently if the counter is exhausted.
    pub fn allocate() -> Result<Self, RevisionExhausted> {
        let mut value = NEXT_SOURCE_ID.load(Ordering::Relaxed);
        loop {
            let next = value.checked_add(1).ok_or(RevisionExhausted)?;
            // Relaxed is sufficient for uniqueness; IDs do not publish other memory.
            match NEXT_SOURCE_ID.compare_exchange_weak(
                value,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return NonZeroU64::new(value).map(Self).ok_or(RevisionExhausted),
                Err(actual) => value = actual,
            }
        }
    }

    /// The numeric label, for deterministic diagnostic output only.
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// One immutable revision of a source.
///
/// Any change to a source's text must produce a new revision with
/// [`Self::next`]. Spans recorded against an older revision are then rejected
/// instead of being silently applied to different text.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceRevision {
    source: SourceId,
    revision: u64,
}

impl SourceRevision {
    /// The first revision (zero) of `source`.
    pub const fn initial(source: SourceId) -> Self {
        Self {
            source,
            revision: 0,
        }
    }

    /// The following revision of the same source; it never wraps.
    pub const fn next(self) -> Result<Self, RevisionExhausted> {
        match self.revision.checked_add(1) {
            Some(revision) => Ok(Self {
                source: self.source,
                revision,
            }),
            None => Err(RevisionExhausted),
        }
    }

    /// The source this revision belongs to.
    pub const fn source(self) -> SourceId {
        self.source
    }

    /// Zero-based revision number within the source.
    pub const fn number(self) -> u64 {
        self.revision
    }
}

/// A byte span tied to the source revision it was measured in.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceLocation {
    revision: SourceRevision,
    span: SourceSpan,
}

impl SourceLocation {
    /// Record `span` as measured in `revision`; bounds are checked on resolution.
    pub const fn new(revision: SourceRevision, span: SourceSpan) -> Self {
        Self { revision, span }
    }

    /// The revision the span was measured in.
    pub const fn revision(self) -> SourceRevision {
        self.revision
    }

    /// The byte span in that revision.
    pub const fn span(self) -> SourceSpan {
        self.span
    }

    /// Slice `text`, which the caller states is `current`'s full text.
    ///
    /// Fails for another source, another revision, an out-of-bounds span, or a
    /// span that splits a UTF-8 character. Failure never returns partial text.
    pub fn resolve(self, current: SourceRevision, text: &str) -> Result<&str, LocationError> {
        if self.revision.source != current.source {
            return Err(LocationError::OtherSource);
        }
        if self.revision.revision != current.revision {
            return Err(LocationError::StaleRevision);
        }
        self.span.slice_str(text).ok_or(LocationError::OutOfBounds)
    }
}

/// A source location could not be resolved against the supplied text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocationError {
    /// The location belongs to a different source.
    OtherSource,
    /// The location was measured in an earlier or later revision.
    StaleRevision,
    /// The span exceeds the text or splits a UTF-8 character.
    OutOfBounds,
}

impl fmt::Display for LocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::OtherSource => "source location belongs to another source",
            Self::StaleRevision => "source location belongs to another revision",
            Self::OutOfBounds => "source location is outside the text or splits a character",
        })
    }
}

impl std::error::Error for LocationError {}

/// A source identity or revision counter would overflow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RevisionExhausted;

impl fmt::Display for RevisionExhausted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("source identity or revision space exhausted")
    }
}

impl std::error::Error for RevisionExhausted {}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize, end: usize) -> SourceSpan {
        SourceSpan::new(start, end).unwrap()
    }

    #[test]
    fn source_ids_are_unique_and_revisions_are_ordered() {
        let first = SourceId::allocate().unwrap();
        let second = SourceId::allocate().unwrap();
        assert_ne!(first, second);
        let initial = SourceRevision::initial(first);
        let next = initial.next().unwrap();
        assert_eq!(initial.number(), 0);
        assert_eq!(next.number(), 1);
        assert_eq!(next.source(), first);
        assert!(initial < next);
    }

    #[test]
    fn revision_counter_never_wraps() {
        let source = SourceId::allocate().unwrap();
        let last = SourceRevision {
            source,
            revision: u64::MAX,
        };
        assert_eq!(last.next(), Err(RevisionExhausted));
    }

    #[test]
    fn stale_or_foreign_locations_are_rejected() {
        let source = SourceId::allocate().unwrap();
        let other = SourceId::allocate().unwrap();
        let old = SourceRevision::initial(source);
        let new = old.next().unwrap();
        let location = SourceLocation::new(old, span(1, 3));
        assert_eq!(location.resolve(old, "aé!"), Ok("é"));
        assert_eq!(
            location.resolve(new, "aé!"),
            Err(LocationError::StaleRevision)
        );
        assert_eq!(
            location.resolve(SourceRevision::initial(other), "aé!"),
            Err(LocationError::OtherSource)
        );
        let split = SourceLocation::new(old, span(1, 2));
        assert_eq!(split.resolve(old, "aé!"), Err(LocationError::OutOfBounds));
        let past_end = SourceLocation::new(old, span(0, 9));
        assert_eq!(past_end.resolve(old, "ab"), Err(LocationError::OutOfBounds));
    }
}
