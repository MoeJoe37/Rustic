use rustic_base::NodeId;

use super::{Document, MutationEntry};
use crate::{DomError, DomString, NodeKind};

/// A validated code-unit edit. Counts clamp before endpoint/length arithmetic.
#[derive(Debug, Eq, PartialEq)]
struct EditPlan {
    end: usize,
    new_len: usize,
    bytes: usize,
}

impl EditPlan {
    fn new(len: usize, offset: usize, count: usize, inserted: usize) -> Result<Self, DomError> {
        let available = len.checked_sub(offset).ok_or(DomError::InvalidOffset)?;
        let removed = count.min(available);
        let end = offset.checked_add(removed).ok_or(DomError::DataLimit)?;
        let new_len = (len - removed)
            .checked_add(inserted)
            .ok_or(DomError::DataLimit)?;
        let bytes = new_len
            .checked_mul(std::mem::size_of::<u16>())
            .ok_or(DomError::DataLimit)?;
        Ok(Self {
            end,
            new_len,
            bytes,
        })
    }
}

impl Document {
    /// Replace an entire character-data value, returning the previous owned value.
    ///
    /// Character data is the value of a text, comment or CDATA section node, or
    /// the data (never the target) of a processing instruction. Identity, kind
    /// and payload budget are validated before replacement.
    pub fn set_character_data(
        &mut self,
        id: NodeId,
        value: impl Into<DomString>,
    ) -> Result<DomString, DomError> {
        let fixed = self.fixed_bytes(id)?;
        let value = value.into();
        let changed = self.character_data(id)? != &value;
        let bytes = fixed
            .checked_add(value.data_bytes())
            .ok_or(DomError::DataLimit)?;
        let entry = self.data_entry(id)?;
        self.arena.resize_data(id, bytes)?;
        if changed {
            self.note_mutation();
        }
        self.push_entry(entry);
        let old_len = self.character_data(id)?.as_units().len();
        self.live_replace_data(id, 0, old_len, value.as_units().len());
        Ok(std::mem::replace(self.character_data_mut(id), value))
    }

    /// Replace a range of UTF-16 code units in a character-data node.
    ///
    /// An offset past the end fails. A count past the end is clamped, including
    /// `usize::MAX`. Surrogate halves may be edited individually. Validation and
    /// capacity reservation precede content/accounting changes. Live ranges
    /// follow the DOM's "replace data" steps. An enabled journal records the edit even when the
    /// replacement equals the existing range, as the DOM queues a record.
    pub fn replace_character_data(
        &mut self,
        id: NodeId,
        offset: usize,
        count: usize,
        value: &[u16],
    ) -> Result<(), DomError> {
        let units = self.character_data(id)?.as_units();
        let len = units.len();
        let plan = EditPlan::new(len, offset, count, value.len())?;
        let unchanged = units[offset..plan.end] == *value;
        let entry = self.data_entry(id)?;
        if unchanged {
            self.push_entry(entry);
            self.live_replace_data(id, offset, plan.end - offset, value.len());
            return Ok(());
        }
        self.prepare_character_edit(id, len, &plan)?;
        self.character_data_mut(id)
            .replace_reserved(offset..plan.end, value, plan.new_len);
        self.note_mutation();
        self.push_entry(entry);
        self.live_replace_data(id, offset, plan.end - offset, value.len());
        Ok(())
    }

    /// Append UTF-16 units without cloning the node's existing payload.
    pub fn append_character_data(&mut self, id: NodeId, value: &[u16]) -> Result<(), DomError> {
        let offset = self.character_data(id)?.as_units().len();
        self.replace_character_data(id, offset, 0, value)
    }

    /// Append valid UTF-8 as UTF-16 without a temporary converted string.
    ///
    /// This supports incremental parser text coalescing. Existing lone surrogates
    /// remain unchanged. Budget checks and capacity reservation precede content
    /// and accounting changes; repeated appends reuse capacity with amortized growth.
    pub fn append_utf8_character_data(&mut self, id: NodeId, value: &str) -> Result<(), DomError> {
        let len = self.character_data(id)?.as_units().len();
        if value.is_empty() {
            return Ok(());
        }
        let plan = EditPlan::new(len, len, 0, value.encode_utf16().count())?;
        let entry = self.data_entry(id)?;
        self.prepare_character_edit(id, len, &plan)?;
        self.character_data_mut(id).append_utf8_reserved(value);
        self.note_mutation();
        self.push_entry(entry);
        Ok(())
    }

    /// Read the node's character data in UTF-16 code units.
    pub fn character_data(&self, id: NodeId) -> Result<&DomString, DomError> {
        match self.node(id)?.kind() {
            NodeKind::Text(value) | NodeKind::Comment(value) | NodeKind::CdataSection(value) => {
                Ok(value)
            }
            NodeKind::ProcessingInstruction(instruction) => Ok(&instruction.data),
            _ => Err(DomError::WrongNodeKind),
        }
    }

    /// Split a Text or CDATA node and insert the new Text after it when attached.
    ///
    /// The callback prepares a handle for the new, empty node before the
    /// split is committed. Failure removes that empty node and preserves the
    /// original text, children, and accounting. The two final payloads share
    /// the original byte charge, even at the exact payload limit.
    pub fn split_text_with_preflight<T>(
        &mut self,
        id: NodeId,
        offset: usize,
        prepare: impl FnOnce(&Self, NodeId) -> Result<T, DomError>,
    ) -> Result<T, DomError> {
        let node = self.node(id)?;
        let (NodeKind::Text(text) | NodeKind::CdataSection(text)) = node.kind() else {
            return Err(DomError::WrongNodeKind);
        };
        let units = text.as_units();
        let suffix = units.get(offset..).ok_or(DomError::InvalidOffset)?;
        let len = units.len();
        let (parent, next) = (node.parent(), node.next_sibling());
        let mut tail = Vec::new();
        tail.try_reserve_exact(suffix.len())
            .map_err(|_| DomError::AllocationFailed)?;
        tail.extend_from_slice(suffix);
        let tail = DomString::from_units(tail);
        let data = self.data_entry(id)?;
        let insertion = match (&data, parent) {
            (Some(_), Some(parent)) => Some(MutationEntry::ChildList {
                target: parent,
                connected: self.is_connected(parent),
                added: Vec::new(),
                removed: Vec::new(),
                previous: Some(id),
                next,
            }),
            _ => None,
        };
        let mut added = Vec::new();
        if insertion.is_some() {
            self.reserve_journal(2)?;
            added
                .try_reserve_exact(1)
                .map_err(|_| DomError::AllocationFailed)?;
        }
        let new = self.create_text(DomString::default())?;
        let handle = match prepare(self, new) {
            Ok(handle) => handle,
            Err(error) => {
                self.arena.remove_detached_leaf(new);
                return Err(error);
            }
        };
        self.arena
            .resize_data(id, offset * std::mem::size_of::<u16>())
            .expect("shorter live text payload");
        self.character_data_mut(id)
            .replace_reserved(offset..len, &[], offset);
        self.arena
            .resize_data(new, tail.data_bytes())
            .expect("split payload bytes already released");
        *self.character_data_mut(new) = tail;
        if let Some(parent) = parent {
            // The existing sibling establishes the parent, legal child kind,
            // and depth for another text leaf at this position.
            self.link_before(parent, new, next);
            self.live_split(id, new, parent, offset);
        }
        // "Replace data" for the removed suffix.
        self.live_replace_data(id, offset, len - offset, 0);
        if parent.is_none() && offset != len {
            self.note_mutation();
        }
        // DOM splitText inserts the new node before replacing the old data.
        if let Some(MutationEntry::ChildList {
            target,
            connected,
            removed,
            previous,
            next,
            ..
        }) = insertion
        {
            added.push(new);
            self.push_entry(Some(MutationEntry::ChildList {
                target,
                connected,
                added,
                removed,
                previous,
                next,
            }));
        }
        self.push_entry(data);
        Ok(handle)
    }

    /// A reserved `characterData` entry holding the current value, if recording.
    fn data_entry(&mut self, id: NodeId) -> Result<Option<MutationEntry>, DomError> {
        let Some(old_value) = self.journal_old_data(id)? else {
            return Ok(None);
        };
        self.reserve_journal(1)?;
        Ok(Some(MutationEntry::CharacterData {
            target: id,
            old_value,
        }))
    }

    fn push_entry(&mut self, entry: Option<MutationEntry>) {
        if let Some(entry) = entry {
            self.push_reserved(entry);
        }
    }

    fn character_data_mut(&mut self, id: NodeId) -> &mut DomString {
        match &mut self.live_mut(id).kind {
            NodeKind::Text(value) | NodeKind::Comment(value) | NodeKind::CdataSection(value) => {
                value
            }
            NodeKind::ProcessingInstruction(instruction) => &mut instruction.data,
            _ => unreachable!("validated character-data kind"),
        }
    }

    /// Accounted bytes that a character-data edit keeps (a target name).
    fn fixed_bytes(&self, id: NodeId) -> Result<usize, DomError> {
        let value = self.character_data(id)?.data_bytes();
        Ok(self.live(id).data_bytes - value)
    }

    fn prepare_character_edit(
        &mut self,
        id: NodeId,
        len: usize,
        plan: &EditPlan,
    ) -> Result<(), DomError> {
        let bytes = self
            .fixed_bytes(id)?
            .checked_add(plan.bytes)
            .ok_or(DomError::DataLimit)?;
        self.arena.check_data_resize(id, bytes)?;
        self.character_data_mut(id)
            .reserve_units(plan.new_len.saturating_sub(len))?;
        // No callbacks/interleaving occur between preflight and this commit.
        self.arena.resize_data(id, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_plan_clamps_counts_and_checks_length_and_byte_overflow() {
        assert_eq!(
            EditPlan::new(4, 1, usize::MAX, 2),
            Ok(EditPlan {
                end: 4,
                new_len: 3,
                bytes: 6
            })
        );
        assert_eq!(EditPlan::new(4, 5, 0, 0), Err(DomError::InvalidOffset));
        assert_eq!(EditPlan::new(1, 1, 0, usize::MAX), Err(DomError::DataLimit));
        assert_eq!(
            EditPlan::new(0, 0, 0, usize::MAX / 2 + 1),
            Err(DomError::DataLimit)
        );
        assert_eq!(
            EditPlan::new(0, 0, usize::MAX, 0),
            Ok(EditPlan {
                end: 0,
                new_len: 0,
                bytes: 0
            })
        );
    }
}
