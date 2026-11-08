use rustic_base::NodeId;

use super::Document;
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
        let bytes = fixed
            .checked_add(value.data_bytes())
            .ok_or(DomError::DataLimit)?;
        self.arena.resize_data(id, bytes)?;
        Ok(std::mem::replace(self.character_data_mut(id), value))
    }

    /// Replace a range of UTF-16 code units in a character-data node.
    ///
    /// An offset past the end fails. A count past the end is clamped, including
    /// `usize::MAX`. Surrogate halves may be edited individually. Validation and
    /// capacity reservation precede content/accounting changes. This internal
    /// operation does not yet update live ranges or queue mutation records.
    pub fn replace_character_data(
        &mut self,
        id: NodeId,
        offset: usize,
        count: usize,
        value: &[u16],
    ) -> Result<(), DomError> {
        let len = self.character_data(id)?.as_units().len();
        let plan = EditPlan::new(len, offset, count, value.len())?;
        self.prepare_character_edit(id, len, &plan)?;
        self.character_data_mut(id)
            .replace_reserved(offset..plan.end, value, plan.new_len);
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
        let plan = EditPlan::new(len, len, 0, value.encode_utf16().count())?;
        self.prepare_character_edit(id, len, &plan)?;
        self.character_data_mut(id).append_utf8_reserved(value);
        Ok(())
    }

    fn character_data(&self, id: NodeId) -> Result<&DomString, DomError> {
        match self.node(id)?.kind() {
            NodeKind::Text(value) | NodeKind::Comment(value) | NodeKind::CdataSection(value) => {
                Ok(value)
            }
            NodeKind::ProcessingInstruction(instruction) => Ok(&instruction.data),
            _ => Err(DomError::WrongNodeKind),
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
