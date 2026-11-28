//! Document-local revisions and synchronous layout snapshot reuse.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Debug;
use std::rc::Rc;

use rustic_bindings::page::{BoxSizes, ComputedStyleSnapshot, LayoutReadError};
use rustic_dom::{Document, NodeId};
use rustic_html::DocumentMode;
use rustic_style::AnimationController;

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

    /// [`Self::measure_layout`] with the document's running animations
    /// applied to computed styles (and advanced by any style change, as
    /// a style flush does). The default ignores them.
    fn measure_layout_animated(
        &self,
        document: &Document,
        mode: DocumentMode,
        animations: &RefCell<AnimationController>,
    ) -> Result<LayoutGeometry, LayoutReadError> {
        let _ = animations;
        self.measure_layout(document, mode)
    }
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
    /// Content and border box sizes of elements with block or atomic boxes,
    /// for ResizeObserver. Elements without an entry report zero.
    pub box_sizes: HashMap<NodeId, BoxSizes>,
    /// Scroll containers' position ranges `[min x, min y, max x, max y]`
    /// (ADR 0064). Elements without an entry do not scroll.
    pub scroll_ranges: HashMap<NodeId, [f64; 4]>,
    /// The viewport's position range, like [`Self::scroll_ranges`].
    pub viewport_scroll_range: [f64; 4],
    /// The viewport's size in CSS pixels.
    pub viewport_size: [f64; 2],
    /// For each element with a box: whether the viewport's position moves
    /// it (not for fixed boxes), and the scroll containers on its
    /// containing-block chain whose positions move it.
    pub scroll_chains: HashMap<NodeId, (bool, Vec<NodeId>)>,
    /// The current request of each `img` element the layout's image load
    /// requested (HTML §4.8.4.1). Elements without an entry have none.
    pub images: HashMap<NodeId, rustic_bindings::page::ImageState>,
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
    animations: Option<Rc<RefCell<AnimationController>>>,
    cached: Option<(LayoutRevision, Geometry)>,
    inputs: u64,
    turn_flushes: usize,
    total_flushes: u64,
}

impl LayoutReads {
    pub(crate) fn with_animations(animations: Rc<RefCell<AnimationController>>) -> Self {
        Self {
            animations: Some(animations),
            ..Self::default()
        }
    }

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
        let geometry = match &self.animations {
            Some(animations) => source.measure_layout_animated(document, mode, animations),
            None => source.measure_layout(document, mode),
        };
        let result = geometry.as_ref().map_err(|&error| error).and_then(read);
        self.cached = revision.map(|revision| (revision, geometry));
        result
    }
}
