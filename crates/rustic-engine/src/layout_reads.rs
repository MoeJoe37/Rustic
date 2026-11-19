//! Document-local revisions and synchronous layout snapshot reuse.

use std::collections::HashMap;
use std::fmt::Debug;

use rustic_bindings::page::{ComputedStyleSnapshot, LayoutReadError};
use rustic_dom::{Document, NodeId};
use rustic_html::DocumentMode;

/// The maximum number of dirty layout snapshots a script turn can build.
pub const MAX_LAYOUT_FLUSHES_PER_TURN: usize = 8;

/// Bounded layout inputs supplied by a host, using its normal renderer's pipeline.
pub trait LayoutSource: Debug {
    /// Build the geometry of rendered elements from the current DOM.
    /// Missing elements have no associated box and read as zero.
    fn measure_layout(
        &self,
        document: &Document,
        mode: DocumentMode,
    ) -> Result<LayoutGeometry, LayoutReadError>;
}

/// Geometry captured at one DOM and rendering-input revision.
#[derive(Debug, Default)]
pub struct LayoutGeometry {
    /// Rounded untransformed border-fragment widths.
    pub offset_widths: HashMap<NodeId, i32>,
    /// Transformed client bounds. `None` values describe unprojectable geometry.
    pub client_rects: HashMap<NodeId, Option<[f64; 4]>>,
    /// CSSOM values for every styled element, including elements without boxes.
    pub computed_styles: HashMap<NodeId, ComputedStyleSnapshot>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LayoutRevision {
    pub(crate) dom: u64,
    pub(crate) inputs: u64,
}

type Geometry = Result<LayoutGeometry, LayoutReadError>;

#[derive(Debug, Default)]
pub(crate) struct LayoutReads {
    source: Option<Box<dyn LayoutSource>>,
    cached: Option<(LayoutRevision, Geometry)>,
    inputs: u64,
    turn_flushes: usize,
    total_flushes: u64,
}

impl LayoutReads {
    pub(crate) fn has_source(&self) -> bool {
        self.source.is_some()
    }

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

    pub(crate) fn read<T>(
        &mut self,
        document: &Document,
        mode: DocumentMode,
        empty: T,
        read: impl FnOnce(&LayoutGeometry) -> Result<T, LayoutReadError>,
    ) -> Result<T, LayoutReadError> {
        let Some(source) = &self.source else {
            return Ok(empty);
        };
        let revision = self.revision(document);
        if let Some((cached, geometry)) = &self.cached {
            if Some(*cached) == revision {
                return geometry.as_ref().map_err(|&error| error).and_then(read);
            }
        }
        if self.turn_flushes == MAX_LAYOUT_FLUSHES_PER_TURN {
            return Err(LayoutReadError::Limit);
        }
        self.turn_flushes += 1;
        self.total_flushes = self.total_flushes.saturating_add(1);
        let geometry = source.measure_layout(document, mode);
        let result = geometry.as_ref().map_err(|&error| error).and_then(read);
        self.cached = revision.map(|revision| (revision, geometry));
        result
    }
}
