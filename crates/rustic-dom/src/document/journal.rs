//! An opt-in log of tree mutations, the input to DOM mutation observers.
//!
//! The DOM's "queue a mutation record" steps need the target, the moved
//! nodes and their siblings, or the previous value, at the moment of change.
//! A document records exactly those facts in [`MutationEntry`] values while
//! its journal is enabled. It knows nothing about observers: the binding layer
//! drains the journal after each operation and decides who is interested.
//!
//! Every entry is fully allocated before the tree changes, so an allocation
//! failure leaves both the tree and the journal untouched.

use rustic_base::NodeId;

use super::Document;
use crate::{DomError, DomString, QualifiedName};

/// One low-level mutation, in the order the DOM specification queues records.
///
/// Node IDs embed their document. A cross-document move records nodes of
/// both documents in one entry list, in the destination's journal when it is
/// enabled.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationEntry {
    /// A `childList` record: nodes were inserted into or removed from `target`.
    ChildList {
        /// The parent whose children changed.
        target: NodeId,
        /// Whether the parent was connected at the time of the change.
        connected: bool,
        /// Inserted nodes, in tree order.
        added: Vec<NodeId>,
        /// Removed nodes, in their former tree order.
        removed: Vec<NodeId>,
        /// The sibling before the change point.
        previous: Option<NodeId>,
        /// The sibling after the change point.
        next: Option<NodeId>,
    },
    /// An `attributes` record: an attribute was set, replaced or removed.
    Attribute {
        /// The element.
        target: NodeId,
        /// The attribute's expanded name.
        name: QualifiedName,
        /// The value before the change; `None` for a new attribute.
        old_value: Option<DomString>,
        /// The value after the change, or none for a removed attribute.
        new_value: Option<DomString>,
    },
    /// A `characterData` record: a character-data value was replaced.
    CharacterData {
        /// The text, comment, CDATA section or processing instruction.
        target: NodeId,
        /// The data before the change.
        old_value: DomString,
    },
}

impl MutationEntry {
    /// The node the DOM queues this record for.
    pub fn target(&self) -> NodeId {
        match self {
            Self::ChildList { target, .. }
            | Self::Attribute { target, .. }
            | Self::CharacterData { target, .. } => *target,
        }
    }
}

impl Document {
    /// Start or stop recording [`MutationEntry`] values. Stopping drops any
    /// entries not yet taken. A disabled journal costs one branch per edit.
    pub fn set_mutation_journal(&mut self, enabled: bool) {
        match (enabled, self.journal.is_some()) {
            (true, false) => self.journal = Some(Vec::new()),
            (false, true) => self.journal = None,
            _ => {}
        }
    }

    /// Whether mutations are being recorded.
    pub fn mutation_journal_enabled(&self) -> bool {
        self.journal.is_some()
    }

    /// Whether recorded entries are waiting to be taken.
    pub fn has_mutation_entries(&self) -> bool {
        self.journal.as_ref().is_some_and(|log| !log.is_empty())
    }

    /// Take every recorded entry, oldest first, leaving the journal enabled.
    pub fn take_mutation_entries(&mut self) -> Vec<MutationEntry> {
        self.journal
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Reserve room for `count` entries; `false` when recording is off.
    pub(super) fn reserve_journal(&mut self, count: usize) -> Result<bool, DomError> {
        match &mut self.journal {
            Some(log) => log
                .try_reserve(count)
                .map(|()| true)
                .map_err(|_| DomError::AllocationFailed),
            None => Ok(false),
        }
    }

    /// Append an entry whose slot [`Self::reserve_journal`] reserved.
    pub(super) fn push_reserved(&mut self, entry: MutationEntry) {
        if let Some(log) = &mut self.journal {
            debug_assert!(log.len() < log.capacity(), "journal slot reserved");
            log.push(entry);
        }
    }

    /// A previous character-data value, copied only when recording.
    pub(super) fn journal_old_data(&self, id: NodeId) -> Result<Option<DomString>, DomError> {
        if self.journal.is_none() {
            return Ok(None);
        }
        copy_string(self.character_data(id)?).map(Some)
    }
}

/// A fallible copy of a node list.
pub(super) fn nodes(ids: &[NodeId]) -> Result<Vec<NodeId>, DomError> {
    let mut out = Vec::new();
    out.try_reserve_exact(ids.len())
        .map_err(|_| DomError::AllocationFailed)?;
    out.extend_from_slice(ids);
    Ok(out)
}

/// A fallible copy of a string value.
pub(super) fn copy_string(value: &DomString) -> Result<DomString, DomError> {
    let mut units = Vec::new();
    units
        .try_reserve_exact(value.as_units().len())
        .map_err(|_| DomError::AllocationFailed)?;
    units.extend_from_slice(value.as_units());
    Ok(DomString::from_units(units))
}
