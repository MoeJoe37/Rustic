//! Host rendering opportunities and document-local frame order.

use std::time::Duration;

use rustic_base::DocumentId;

use crate::ScriptStop;
use crate::layout_reads::LayoutRevision;

/// Ticket issued for one rendering turn of a document.
///
/// Hosts may carry it with the rendering output and validate it before presentation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderFrame {
    owner: DocumentId,
    sequence: u64,
    timestamp: Duration,
}

impl RenderFrame {
    /// Sequence within the owning document, in rendering-turn order.
    pub fn sequence(self) -> u64 {
        self.sequence
    }

    /// Time of the selected opportunity, relative to the document's host clock.
    pub fn timestamp(self) -> Duration {
        self.timestamp
    }
}

pub(crate) struct Rendering {
    owner: DocumentId,
    sequence: u64,
    pending: Option<Duration>,
    outstanding: Option<(RenderFrame, LayoutRevision)>,
    preferred: bool,
}

impl Rendering {
    pub(crate) fn new(owner: DocumentId) -> Self {
        Self {
            owner,
            sequence: 0,
            pending: None,
            outstanding: None,
            preferred: false,
        }
    }

    pub(crate) fn offer(&mut self, timestamp: Duration) {
        self.pending = Some(timestamp);
    }

    pub(crate) fn has_priority(&self) -> bool {
        self.preferred && self.pending.is_some()
    }

    pub(crate) fn after_script(&mut self) {
        self.preferred = true;
    }

    pub(crate) fn begin(&mut self) -> Result<Option<RenderFrame>, ScriptStop> {
        let Some(timestamp) = self.pending else {
            return Ok(None);
        };
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(ScriptStop::FrameIdsExhausted)?;
        let frame = RenderFrame {
            owner: self.owner,
            sequence,
            timestamp,
        };
        self.sequence = sequence;
        self.pending = None;
        self.outstanding = None;
        self.preferred = false;
        Ok(Some(frame))
    }

    pub(crate) fn record(&mut self, frame: RenderFrame, revision: Option<LayoutRevision>) {
        self.outstanding = revision.map(|revision| (frame, revision));
    }

    pub(crate) fn accept(&mut self, frame: RenderFrame, revision: Option<LayoutRevision>) -> bool {
        if let Some((outstanding, rendered_revision)) = self.outstanding {
            if outstanding == frame {
                self.outstanding = None;
                return revision == Some(rendered_revision);
            }
        }
        false
    }

    pub(crate) fn clear(&mut self) {
        self.pending = None;
        self.outstanding = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_sequence_exhaustion_preserves_existing_frame() {
        let mut rendering = Rendering::new(DocumentId::allocate().unwrap());
        rendering.sequence = u64::MAX - 1;
        rendering.offer(Duration::ZERO);
        let last = rendering.begin().unwrap().unwrap();
        assert_eq!(last.sequence(), u64::MAX);
        let revision = Some(LayoutRevision { dom: 0, inputs: 0 });
        rendering.record(last, revision);

        rendering.offer(Duration::from_nanos(1));
        assert_eq!(rendering.begin(), Err(ScriptStop::FrameIdsExhausted));
        assert!(rendering.accept(last, revision));
    }
}
