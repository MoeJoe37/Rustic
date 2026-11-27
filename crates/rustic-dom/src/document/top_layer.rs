//! The top layer (CSS Positioned Layout 4 §2, HTML §6.2): modal dialogs and
//! showing popovers, in the order they were added. Selectors read it for
//! `:modal` and `:popover-open`; removing an element from the tree takes it
//! and its descendants out (HTML's removing steps for dialogs and popovers).

use rustic_base::NodeId;

use super::Document;
use crate::DomError;

/// Why an element is in the top layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TopLayer {
    /// A dialog opened with `showModal()`.
    Modal,
    /// A popover in the showing state.
    Popover,
}

impl Document {
    /// Add `element` to the end of the top layer (moving it there if present).
    pub fn top_layer_add(&mut self, element: NodeId, kind: TopLayer) -> Result<(), DomError> {
        self.element(element)?;
        self.top_layer.retain(|&(id, _)| id != element);
        self.top_layer
            .try_reserve(1)
            .map_err(|_| DomError::AllocationFailed)?;
        self.top_layer.push((element, kind));
        self.note_mutation();
        Ok(())
    }

    /// Remove `element` from the top layer; whether it was there.
    pub fn top_layer_remove(&mut self, element: NodeId) -> bool {
        let before = self.top_layer.len();
        self.top_layer.retain(|&(id, _)| id != element);
        let removed = self.top_layer.len() != before;
        if removed {
            self.note_mutation();
        }
        removed
    }

    /// Why `element` is in the top layer, if it is.
    pub fn top_layer_state(&self, element: NodeId) -> Option<TopLayer> {
        self.top_layer
            .iter()
            .find(|&&(id, _)| id == element)
            .map(|&(_, kind)| kind)
    }

    /// The top layer, bottom first.
    pub fn top_layer(&self) -> &[(NodeId, TopLayer)] {
        &self.top_layer
    }

    /// Drop entries inside `removed` before it leaves the tree.
    pub(super) fn top_layer_before_remove(&mut self, removed: NodeId) {
        if self.top_layer.is_empty() {
            return;
        }
        let inside: Vec<NodeId> = self
            .top_layer
            .iter()
            .map(|&(id, _)| id)
            .filter(|&id| self.is_inclusive_descendant(id, removed))
            .collect();
        if !inside.is_empty() {
            self.top_layer.retain(|(id, _)| !inside.contains(id));
        }
    }
}
