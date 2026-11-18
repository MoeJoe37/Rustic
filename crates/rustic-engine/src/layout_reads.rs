//! Document-local revisions and synchronous layout snapshot reuse.

use std::collections::HashMap;
use std::fmt::Debug;

use rustic_bindings::page::LayoutReadError;
use rustic_dom::{Document, NodeId};
use rustic_html::DocumentMode;

/// The maximum number of dirty layout snapshots a script turn can build.
pub const MAX_LAYOUT_FLUSHES_PER_TURN: usize = 8;

/// Bounded layout inputs supplied by a host, using its normal renderer's pipeline.
pub trait LayoutSource: Debug {
    /// Build the widths of the rendered elements from the current DOM. Missing
    /// elements have no associated box and read as zero.
    fn measure_widths(
        &self,
        document: &Document,
        mode: DocumentMode,
    ) -> Result<HashMap<NodeId, i32>, LayoutReadError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LayoutRevision {
    pub(crate) dom: u64,
    pub(crate) inputs: u64,
}

type Widths = Result<HashMap<NodeId, i32>, LayoutReadError>;

#[derive(Debug, Default)]
pub(crate) struct LayoutReads {
    source: Option<Box<dyn LayoutSource>>,
    cached: Option<(LayoutRevision, Widths)>,
    inputs: u64,
    turn_flushes: usize,
    total_flushes: u64,
}

impl LayoutReads {
    pub(crate) fn set_source(&mut self, source: Box<dyn LayoutSource>) {
        self.source = Some(source);
        self.inputs_changed();
    }

    pub(crate) fn inputs_changed(&mut self) {
        self.inputs = self.inputs.saturating_add(1);
        self.cached = None;
    }

    pub(crate) fn revision(&self, document: &Document) -> Option<LayoutRevision> {
        (self.inputs != u64::MAX).then_some(LayoutRevision {
            dom: document.mutation_revision()?,
            inputs: self.inputs,
        })
    }

    pub(crate) fn begin_turn(&mut self) {
        self.turn_flushes = 0;
    }

    pub(crate) fn take_flush_count(&mut self) -> u64 {
        std::mem::take(&mut self.total_flushes)
    }

    pub(crate) fn clear(&mut self) {
        self.source = None;
        self.cached = None;
    }

    pub(crate) fn width(
        &mut self,
        document: &Document,
        mode: DocumentMode,
        node: NodeId,
    ) -> Result<i32, LayoutReadError> {
        let Some(source) = &self.source else {
            return Ok(0);
        };
        let revision = self.revision(document);
        if let Some((cached, widths)) = &self.cached {
            if Some(*cached) == revision {
                return width_in(widths, node);
            }
        }
        if self.turn_flushes == MAX_LAYOUT_FLUSHES_PER_TURN {
            return Err(LayoutReadError::Limit);
        }
        self.turn_flushes += 1;
        self.total_flushes = self.total_flushes.saturating_add(1);
        let widths = source.measure_widths(document, mode);
        let result = width_in(&widths, node);
        self.cached = revision.map(|revision| (revision, widths));
        result
    }
}

fn width_in(widths: &Widths, node: NodeId) -> Result<i32, LayoutReadError> {
    widths
        .as_ref()
        .map(|widths| widths.get(&node).copied().unwrap_or(0))
        .map_err(|&error| error)
}
