//! Live ranges and node iterators (DOM §5.2, §6.1).
//!
//! A live range's boundary points and a `NodeIterator`'s reference must
//! follow every mutation of the tree, at the moment it happens, so the
//! document keeps them itself. The binding layer registers one entry per
//! script object and drops it when the object dies; the mutation primitives
//! apply the DOM's update steps: insertion and removal (through `link_before`
//! and `unlink`), "replace data" and `splitText`.

use rustic_base::NodeId;

use super::Document;
use crate::DomError;

/// Most live ranges and node iterators one document tracks at once.
pub const MAX_LIVE_ENTRIES: usize = 1 << 16;

/// A node and an offset into it (DOM §5.1 boundary point).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundaryPoint {
    /// The boundary node.
    pub node: NodeId,
    /// Children before the point, or code units for character data.
    pub offset: u32,
}

/// Identifies a registered live range or node iterator in its document.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct LiveId(u64);

impl LiveId {
    /// The raw value, for binding-layer bookkeeping.
    pub fn get(self) -> u64 {
        self.0
    }

    /// The id with raw value `raw` (from [`Self::get`]).
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Clone, Debug)]
pub(super) struct LiveRange {
    id: LiveId,
    start: BoundaryPoint,
    end: BoundaryPoint,
}

/// A `NodeIterator`'s mutable state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IteratorState {
    /// The iterator's root, which never changes.
    pub root: NodeId,
    /// The reference node.
    pub reference: NodeId,
    /// Whether the iterator sits before its reference.
    pub pointer_before_reference: bool,
}

#[derive(Clone, Debug)]
pub(super) struct LiveIterator {
    id: LiveId,
    state: IteratorState,
}

/// Registered live ranges and iterators.
#[derive(Clone, Debug, Default)]
pub(super) struct LiveSet {
    ranges: Vec<LiveRange>,
    iterators: Vec<LiveIterator>,
}

/// Ids are unique across documents, so an entry keeps its id when its
/// nodes move to another document.
static NEXT_LIVE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl LiveSet {
    pub(super) fn is_empty(&self) -> bool {
        self.ranges.is_empty() && self.iterators.is_empty()
    }

    fn allocate(&mut self) -> Result<LiveId, DomError> {
        if self.ranges.len() + self.iterators.len() >= MAX_LIVE_ENTRIES {
            return Err(DomError::NodeLimit);
        }
        Ok(LiveId(
            NEXT_LIVE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ))
    }

    fn points_mut(&mut self) -> impl Iterator<Item = &mut BoundaryPoint> {
        self.ranges
            .iter_mut()
            .flat_map(|range| [&mut range.start, &mut range.end])
    }
}

impl Document {
    fn check_point(&self, point: BoundaryPoint) -> Result<(), DomError> {
        if point.node.document() != self.id() {
            return Err(DomError::WrongDocument);
        }
        if point.offset as usize > self.node_length(point.node)? {
            return Err(DomError::InvalidOffset);
        }
        Ok(())
    }

    /// A node's length (DOM §4.2): code units of character data, else the
    /// number of children (a doctype has none).
    pub fn node_length(&self, node: NodeId) -> Result<usize, DomError> {
        match self.character_data(node) {
            Ok(data) => Ok(data.as_units().len()),
            Err(DomError::WrongNodeKind) => Ok(self.children(node)?.count()),
            Err(error) => Err(error),
        }
    }

    /// The index of `node` among its siblings.
    pub fn index_of(&self, node: NodeId) -> Result<usize, DomError> {
        let mut index = 0;
        let mut at = self.node(node)?.previous_sibling();
        while let Some(previous) = at {
            index += 1;
            at = self.live(previous).previous_sibling();
        }
        Ok(index)
    }

    /// Whether `node` is `ancestor` or one of its descendants.
    pub fn is_inclusive_descendant(&self, node: NodeId, ancestor: NodeId) -> bool {
        let mut at = Some(node);
        while let Some(id) = at {
            if id == ancestor {
                return true;
            }
            at = self.node(id).ok().and_then(|n| n.parent());
        }
        false
    }

    /// Register a live range with boundary points in this document.
    pub fn create_live_range(
        &mut self,
        start: BoundaryPoint,
        end: BoundaryPoint,
    ) -> Result<LiveId, DomError> {
        self.check_point(start)?;
        self.check_point(end)?;
        self.live_set
            .ranges
            .try_reserve(1)
            .map_err(|_| DomError::AllocationFailed)?;
        let id = self.live_set.allocate()?;
        self.live_set.ranges.push(LiveRange { id, start, end });
        Ok(id)
    }

    /// A live range's start and end.
    pub fn live_range(&self, id: LiveId) -> Option<(BoundaryPoint, BoundaryPoint)> {
        self.live_set
            .ranges
            .iter()
            .find(|range| range.id == id)
            .map(|range| (range.start, range.end))
    }

    /// Move a live range's boundary points (both in this document).
    pub fn set_live_range(
        &mut self,
        id: LiveId,
        start: BoundaryPoint,
        end: BoundaryPoint,
    ) -> Result<(), DomError> {
        self.check_point(start)?;
        self.check_point(end)?;
        let range = self
            .live_set
            .ranges
            .iter_mut()
            .find(|range| range.id == id)
            .ok_or(DomError::InvalidNode)?;
        range.start = start;
        range.end = end;
        Ok(())
    }

    /// Register a `NodeIterator` over `root`.
    pub fn create_node_iterator(&mut self, root: NodeId) -> Result<LiveId, DomError> {
        self.node(root)?;
        self.live_set
            .iterators
            .try_reserve(1)
            .map_err(|_| DomError::AllocationFailed)?;
        let id = self.live_set.allocate()?;
        self.live_set.iterators.push(LiveIterator {
            id,
            state: IteratorState {
                root,
                reference: root,
                pointer_before_reference: true,
            },
        });
        Ok(id)
    }

    /// A `NodeIterator`'s state.
    pub fn node_iterator(&self, id: LiveId) -> Option<IteratorState> {
        self.live_set
            .iterators
            .iter()
            .find(|it| it.id == id)
            .map(|it| it.state)
    }

    /// Move a `NodeIterator` (its root is kept).
    pub fn set_node_iterator(
        &mut self,
        id: LiveId,
        reference: NodeId,
        pointer_before_reference: bool,
    ) -> Result<(), DomError> {
        self.node(reference)?;
        let iterator = self
            .live_set
            .iterators
            .iter_mut()
            .find(|it| it.id == id)
            .ok_or(DomError::InvalidNode)?;
        iterator.state.reference = reference;
        iterator.state.pointer_before_reference = pointer_before_reference;
        Ok(())
    }

    /// Stop tracking a live range or iterator; unknown ids are ignored.
    pub fn drop_live(&mut self, id: LiveId) {
        self.live_set.ranges.retain(|range| range.id != id);
        self.live_set.iterators.retain(|it| it.id != id);
    }

    /// Every node a live range or iterator refers to, for garbage
    /// collection: they keep their trees alive.
    pub fn live_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.live_set
            .ranges
            .iter()
            .flat_map(|range| [range.start.node, range.end.node])
            .chain(
                self.live_set
                    .iterators
                    .iter()
                    .flat_map(|it| [it.state.root, it.state.reference]),
            )
    }

    /// Move the entries of `source` that refer to an adopted detached
    /// subtree into this document, renaming their nodes by `ids` (old, new).
    /// A range's boundary points share one root, so a range with one point
    /// in the subtree has both there; an iterator moves with its root.
    pub(super) fn adopt_live(&mut self, source: &mut Document, ids: &[(NodeId, NodeId)]) {
        if source.live_set.is_empty() {
            return;
        }
        let renamed = |node: NodeId| {
            ids.iter()
                .find(|(old, _)| *old == node)
                .map(|&(_, new)| new)
        };
        let mut kept = Vec::new();
        for mut range in std::mem::take(&mut source.live_set.ranges) {
            match (renamed(range.start.node), renamed(range.end.node)) {
                (Some(start), Some(end)) if self.live_set.ranges.try_reserve(1).is_ok() => {
                    range.start.node = start;
                    range.end.node = end;
                    self.live_set.ranges.push(range);
                }
                _ => kept.push(range),
            }
        }
        source.live_set.ranges = kept;
        let mut kept = Vec::new();
        for mut iterator in std::mem::take(&mut source.live_set.iterators) {
            match (
                renamed(iterator.state.root),
                renamed(iterator.state.reference),
            ) {
                (Some(root), Some(reference)) if self.live_set.iterators.try_reserve(1).is_ok() => {
                    iterator.state.root = root;
                    iterator.state.reference = reference;
                    self.live_set.iterators.push(iterator);
                }
                _ => kept.push(iterator),
            }
        }
        source.live_set.iterators = kept;
    }

    /// `normalize()`'s merge of `run`, the Text siblings after `node`, into
    /// `node` (DOM §4.4 step 6): append their `units`, move live boundary
    /// points from each merged node (or from just before it) into `node`,
    /// then remove the run.
    pub fn merge_text(
        &mut self,
        node: NodeId,
        run: &[NodeId],
        units: &[u16],
    ) -> Result<(), DomError> {
        let mut length = self.character_data(node)?.as_units().len() as u32;
        self.append_character_data(node, units)?;
        for &current in run {
            let parent = self.node(current)?.parent();
            let index = self.index_of(current)? as u32;
            let current_length = self.character_data(current)?.as_units().len() as u32;
            for point in self.live_set.points_mut() {
                if point.node == current {
                    point.node = node;
                    point.offset += length;
                } else if Some(point.node) == parent && point.offset == index {
                    point.node = node;
                    point.offset = length;
                }
            }
            length += current_length;
        }
        for &current in run {
            self.detach(current)?;
        }
        Ok(())
    }

    /// The DOM's removal steps for live ranges and node iterators, before
    /// `node` leaves `parent` (DOM §4.2.4 "remove" steps 4–8).
    pub(super) fn live_before_remove(&mut self, node: NodeId, parent: NodeId) {
        if self.live_set.is_empty() {
            return;
        }
        let index = self.index_of(node).unwrap_or(0) as u32;
        let mut set = std::mem::take(&mut self.live_set);
        for point in set.points_mut() {
            if self.is_inclusive_descendant(point.node, node) {
                *point = BoundaryPoint {
                    node: parent,
                    offset: index,
                };
            } else if point.node == parent && point.offset > index {
                point.offset -= 1;
            }
        }
        for iterator in &mut set.iterators {
            self.pre_remove(&mut iterator.state, node);
        }
        self.live_set = set;
    }

    /// NodeIterator pre-removing steps (DOM §6.1).
    fn pre_remove(&self, state: &mut IteratorState, removed: NodeId) {
        if removed == state.root
            || removed.document() != state.root.document()
            || !self.is_inclusive_descendant(state.reference, removed)
        {
            return;
        }
        if state.pointer_before_reference {
            // The first following node inside root and outside `removed`.
            let mut at = removed;
            let next = loop {
                if at == state.root {
                    break None;
                }
                if let Some(sibling) = self.live(at).next_sibling() {
                    break Some(sibling);
                }
                match self.live(at).parent() {
                    Some(parent) => at = parent,
                    None => break None,
                }
            };
            if let Some(next) = next.filter(|&n| self.is_inclusive_descendant(n, state.root)) {
                state.reference = next;
                return;
            }
            state.pointer_before_reference = false;
        }
        state.reference = match self.live(removed).previous_sibling() {
            None => self.live(removed).parent().unwrap_or(state.root),
            Some(mut last) => {
                while let Some(child) = self.live(last).last_child() {
                    last = child;
                }
                last
            }
        };
    }

    /// The insertion steps for live ranges after `child` joined `parent`
    /// (DOM §4.2.3 "insert" step 6).
    pub(super) fn live_after_insert(&mut self, parent: NodeId, child: NodeId) {
        if self.live_set.ranges.is_empty() {
            return;
        }
        let index = self.index_of(child).unwrap_or(0) as u32;
        for point in self.live_set.points_mut() {
            if point.node == parent && point.offset > index {
                point.offset += 1;
            }
        }
    }

    /// "Replace data" steps 8–11 (DOM §4.10) for `count` units at `offset`
    /// replaced by `added` units.
    pub(super) fn live_replace_data(
        &mut self,
        node: NodeId,
        offset: usize,
        count: usize,
        added: usize,
    ) {
        if self.live_set.ranges.is_empty() {
            return;
        }
        let (offset, count, added) = (offset as u32, count as u32, added as u32);
        for point in self.live_set.points_mut() {
            if point.node != node {
                continue;
            }
            if point.offset > offset && point.offset <= offset + count {
                point.offset = offset;
            } else if point.offset > offset + count {
                point.offset = point.offset + added - count;
            }
        }
    }

    /// `splitText` steps 7.2–7.5 (DOM §4.11), after `new` was inserted
    /// after `node` in `parent`.
    pub(super) fn live_split(&mut self, node: NodeId, new: NodeId, parent: NodeId, offset: usize) {
        if self.live_set.ranges.is_empty() {
            return;
        }
        let offset = offset as u32;
        let after = self.index_of(node).unwrap_or(0) as u32 + 1;
        for point in self.live_set.points_mut() {
            if point.node == node && point.offset > offset {
                point.node = new;
                point.offset -= offset;
            } else if point.node == parent && point.offset == after {
                point.offset += 1;
            }
        }
    }
}
