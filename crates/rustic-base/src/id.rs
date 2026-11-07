use std::fmt;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DOCUMENT_ID: AtomicU64 = AtomicU64::new(1);

/// A process-local document identity that is never reused, including after drop.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DocumentId(NonZeroU64);

impl DocumentId {
    /// Allocate a unique identity, failing permanently if the counter is exhausted.
    pub fn allocate() -> Result<Self, IdExhausted> {
        allocate_from(&NEXT_DOCUMENT_ID)
    }
}

fn allocate_from(counter: &AtomicU64) -> Result<DocumentId, IdExhausted> {
    // Relaxed is sufficient for uniqueness; IDs do not publish other memory.
    let mut value = counter.load(Ordering::Relaxed);
    loop {
        let next = value.checked_add(1).ok_or(IdExhausted)?;
        match counter.compare_exchange_weak(value, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return NonZeroU64::new(value).map(DocumentId).ok_or(IdExhausted),
            Err(actual) => value = actual,
        }
    }
}

/// A node's document, storage slot, and slot generation.
///
/// The owning document must validate all three fields on every lookup. Constructing
/// an ID does not create a node, keep one alive, or grant access to a document.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId {
    document: DocumentId,
    slot: u32,
    generation: NonZeroU64,
}

impl NodeId {
    /// Construct a storage identity; the owner remains responsible for validation.
    pub const fn from_parts(document: DocumentId, slot: u32, generation: NonZeroU64) -> Self {
        Self {
            document,
            slot,
            generation,
        }
    }

    /// The document that owns this storage identity.
    pub const fn document(self) -> DocumentId {
        self.document
    }

    /// Storage slot; this is not valid without checking document and generation.
    pub const fn slot(self) -> u32 {
        self.slot
    }

    /// Generation of the slot when this identity was created.
    pub const fn generation(self) -> NonZeroU64 {
        self.generation
    }
}

/// The process-local document identity space has been exhausted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IdExhausted;

impl fmt::Display for IdExhausted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("document identity space exhausted")
    }
}

impl std::error::Error for IdExhausted {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhaustion_does_not_wrap_or_reuse_an_identity() {
        let counter = AtomicU64::new(u64::MAX - 1);
        assert_eq!(allocate_from(&counter).unwrap().0.get(), u64::MAX - 1);
        assert_eq!(allocate_from(&counter), Err(IdExhausted));
        assert_eq!(allocate_from(&counter), Err(IdExhausted));
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    }

    #[test]
    fn allocation_is_unique_across_threads() {
        let threads: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    (0..128)
                        .map(|_| DocumentId::allocate().unwrap())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let ids: std::collections::HashSet<_> = threads
            .into_iter()
            .flat_map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(ids.len(), 1024);
    }
}
