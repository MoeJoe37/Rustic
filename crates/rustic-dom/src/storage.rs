use std::num::NonZeroU64;

use rustic_base::{Budget, DocumentId, NodeId};

use crate::{DomError, Node, NodeKind};

#[derive(Debug)]
struct Slot {
    generation: NonZeroU64,
    node: Option<Box<Node>>,
    next_free: Option<u32>,
}

/// Storage owns identity, slot reuse, and accounting; it does not edit tree links.
#[derive(Debug)]
pub(crate) struct Arena {
    id: DocumentId,
    slots: Vec<Slot>,
    free: Option<u32>,
    nodes: Budget,
    data: Budget,
}

impl Arena {
    pub(crate) fn new(max_nodes: usize, max_data_bytes: usize) -> Result<Self, DomError> {
        Ok(Self {
            id: DocumentId::allocate().map_err(|_| DomError::IdentityExhausted)?,
            slots: Vec::new(),
            free: None,
            nodes: Budget::new(max_nodes),
            data: Budget::new(max_data_bytes),
        })
    }

    pub(crate) fn id(&self) -> DocumentId {
        self.id
    }
    pub(crate) fn node_count(&self) -> usize {
        self.nodes.used()
    }
    pub(crate) fn data_bytes(&self) -> usize {
        self.data.used()
    }

    pub(crate) fn get(&self, id: NodeId) -> Result<&Node, DomError> {
        Ok(self
            .slot(id)?
            .node
            .as_deref()
            .expect("validated occupied slot"))
    }

    pub(crate) fn get_mut(&mut self, id: NodeId) -> Result<&mut Node, DomError> {
        self.slot(id)?;
        Ok(self.slots[id.slot() as usize]
            .node
            .as_deref_mut()
            .expect("validated occupied slot"))
    }

    fn slot(&self, id: NodeId) -> Result<&Slot, DomError> {
        if id.document() != self.id {
            return Err(DomError::WrongDocument);
        }
        let slot = self
            .slots
            .get(id.slot() as usize)
            .ok_or(DomError::InvalidNode)?;
        if slot.generation != id.generation() || slot.node.is_none() {
            return Err(DomError::InvalidNode);
        }
        Ok(slot)
    }

    pub(crate) fn insert(&mut self, kind: NodeKind) -> Result<NodeId, DomError> {
        let data_bytes = kind.data_bytes()?;
        let index = match self.free {
            Some(index) => index,
            None => {
                let index =
                    u32::try_from(self.slots.len()).map_err(|_| DomError::IdentityExhausted)?;
                self.slots
                    .try_reserve(1)
                    .map_err(|_| DomError::AllocationFailed)?;
                index
            }
        };
        self.nodes.charge(1).map_err(|_| DomError::NodeLimit)?;
        if self.data.charge(data_bytes).is_err() {
            self.nodes.release(1).expect("undo node charge");
            return Err(DomError::DataLimit);
        }
        let node = Some(Box::new(Node::new(kind, data_bytes)));
        let generation = if self.free.is_some() {
            let slot = &mut self.slots[index as usize];
            self.free = slot.next_free.take();
            slot.node = node;
            slot.generation
        } else {
            self.slots.push(Slot {
                generation: NonZeroU64::MIN,
                node,
                next_free: None,
            });
            NonZeroU64::MIN
        };
        Ok(NodeId::from_parts(self.id, index, generation))
    }

    pub(crate) fn check_data_resize(&self, id: NodeId, bytes: usize) -> Result<(), DomError> {
        let old = self.get(id)?.data_bytes;
        if bytes > old && bytes - old > self.data.remaining() {
            return Err(DomError::DataLimit);
        }
        Ok(())
    }

    pub(crate) fn resize_data(&mut self, id: NodeId, bytes: usize) -> Result<(), DomError> {
        self.check_data_resize(id, bytes)?;
        let old = self.get(id)?.data_bytes;
        if bytes > old {
            self.data
                .charge(bytes - old)
                .map_err(|_| DomError::DataLimit)?;
        } else {
            self.data
                .release(old - bytes)
                .expect("release owned payload bytes");
        }
        self.get_mut(id)?.data_bytes = bytes;
        Ok(())
    }

    pub(crate) fn remove_detached_leaf(&mut self, id: NodeId) {
        let node = self.slots[id.slot() as usize]
            .node
            .take()
            .expect("live removal node");
        debug_assert!(node.parent.is_none() && node.first_child.is_none());
        self.nodes.release(1).expect("release owned node");
        self.data
            .release(node.data_bytes)
            .expect("release owned payload");
        let slot = &mut self.slots[id.slot() as usize];
        if let Some(next) = slot
            .generation
            .get()
            .checked_add(1)
            .and_then(NonZeroU64::new)
        {
            slot.generation = next;
            slot.next_free = self.free;
            self.free = Some(id.slot());
        }
        // A generation at u64::MAX is retired permanently, never wrapped/reused.
    }

    /// Number of slots ever allocated, including vacant and retired ones.
    ///
    /// Collectors size per-slot mark tables with this; it is not a node count.
    pub(crate) fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Every live node identity in slot order, which is deterministic.
    pub(crate) fn live_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.slots.iter().enumerate().filter_map(|(index, slot)| {
            slot.node.as_ref().map(|_| {
                let index = u32::try_from(index).expect("slot index was checked at insertion");
                NodeId::from_parts(self.id, index, slot.generation)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_slot_generation_is_retired_instead_of_reused() {
        let mut arena = Arena::new(2, 0).unwrap();
        let first = arena.insert(NodeKind::Document).unwrap();
        arena.slots[first.slot() as usize].generation = NonZeroU64::new(u64::MAX).unwrap();
        let last = NodeId::from_parts(arena.id(), first.slot(), NonZeroU64::new(u64::MAX).unwrap());
        arena.remove_detached_leaf(last);
        let next = arena.insert(NodeKind::Document).unwrap();
        assert_ne!(next.slot(), last.slot());
        assert_eq!(arena.get(last).unwrap_err(), DomError::InvalidNode);
    }
}
