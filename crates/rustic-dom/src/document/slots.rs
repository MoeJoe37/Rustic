//! Named and manual slot assignment, shared by layout and script.

use std::collections::{HashMap, HashSet};

use crate::{DomError, Element, HTML_NAMESPACE, NodeId, NodeKind, SlotAssignment};

use super::Document;

/// A snapshot of slot distribution for a borrowed document.
#[derive(Debug)]
pub struct SlotMap<'a> {
    document: &'a Document,
    assigned: HashMap<NodeId, Vec<NodeId>>,
    by_node: HashMap<NodeId, NodeId>,
    slots: Vec<NodeId>,
    active: HashSet<NodeId>,
}

fn attribute<'a>(element: &'a Element, name: &str) -> &'a [u16] {
    element
        .attributes()
        .iter()
        .find(|attr| attr.name().namespace().is_none() && attr.name().local() == name)
        .map_or(&[], |attr| attr.value().as_units())
}

fn slottable(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Element(_) | NodeKind::Text(_) | NodeKind::CdataSection(_)
    )
}

impl Document {
    /// Whether this document owns any shadow roots, including detached ones.
    pub fn has_shadow_roots(&self) -> bool {
        self.shadow_roots != 0
    }

    /// Whether this is an HTML slot element in any tree.
    pub fn is_slot(&self, id: NodeId) -> bool {
        self.element(id).is_ok_and(|element| {
            element.name().namespace() == Some(HTML_NAMESPACE) && element.name().local() == "slot"
        })
    }

    /// Record an explicit ordered slot assignment after checking all handles.
    /// Only children of the slot's host participate in distribution.
    pub fn assign_slot(&mut self, slot: NodeId, nodes: &[NodeId]) -> Result<(), DomError> {
        self.node(slot)?;
        if !self.is_slot(slot) {
            return Err(DomError::WrongNodeKind);
        }
        let mut unique = HashSet::new();
        let mut assigned = Vec::new();
        for &node in nodes {
            if !slottable(self.node(node)?.kind()) {
                return Err(DomError::WrongNodeKind);
            }
            unique
                .try_reserve(1)
                .map_err(|_| DomError::AllocationFailed)?;
            if unique.insert(node) {
                assigned
                    .try_reserve(1)
                    .map_err(|_| DomError::AllocationFailed)?;
                assigned.push(node);
            }
        }
        self.manual_slots
            .try_reserve(1)
            .map_err(|_| DomError::AllocationFailed)?;
        self.manual_owners
            .try_reserve(assigned.len())
            .map_err(|_| DomError::AllocationFailed)?;
        self.forget_manual_slot(slot);
        for &node in &assigned {
            self.forget_manual_assignee(node);
            self.manual_owners.insert(node, slot);
        }
        if !assigned.is_empty() {
            self.manual_slots.insert(slot, assigned);
        }
        self.note_mutation();
        Ok(())
    }

    pub(super) fn forget_manual_slot(&mut self, id: NodeId) {
        if let Some(assigned) = self.manual_slots.remove(&id) {
            for node in assigned {
                self.manual_owners.remove(&node);
            }
        }
    }

    pub(super) fn forget_manual_assignee(&mut self, id: NodeId) {
        if let Some(slot) = self.manual_owners.remove(&id) {
            let list = self.manual_slots.get_mut(&slot).expect("manual slot list");
            list.retain(|&node| node != id);
            if list.is_empty() {
                self.manual_slots.remove(&slot);
            }
        }
    }

    /// Resolve slots in each owned shadow tree in tree order. A slot in a
    /// nested shadow tree belongs only to its own host.
    pub fn slot_map(&self) -> Result<SlotMap<'_>, DomError> {
        let mut map = SlotMap {
            document: self,
            assigned: HashMap::new(),
            by_node: HashMap::new(),
            slots: Vec::new(),
            active: HashSet::new(),
        };
        if !self.has_shadow_roots() {
            return Ok(map);
        }
        for host in self.arena.live_ids() {
            let Ok(element) = self.element(host) else {
                continue;
            };
            let Some((root, options)) = element.shadow_root else {
                continue;
            };
            let mut names = HashMap::new();
            for (slot, _) in self.walk(root)? {
                if !self.is_slot(slot) {
                    continue;
                }
                map.slots
                    .try_reserve(1)
                    .map_err(|_| DomError::AllocationFailed)?;
                map.slots.push(slot);
                map.active
                    .try_reserve(1)
                    .map_err(|_| DomError::AllocationFailed)?;
                map.active.insert(slot);
                match options.slot_assignment {
                    SlotAssignment::Named => {
                        names
                            .try_reserve(1)
                            .map_err(|_| DomError::AllocationFailed)?;
                        names
                            .entry(attribute(self.element(slot)?, "name"))
                            .or_insert(slot);
                    }
                    SlotAssignment::Manual => {
                        for &node in self.manual_slots.get(&slot).into_iter().flatten() {
                            if self
                                .node(node)
                                .is_ok_and(|node| node.parent() == Some(host))
                            {
                                map.add(slot, node)?;
                            }
                        }
                    }
                }
            }
            if names.is_empty() {
                continue;
            }
            for child in self.children(host)? {
                let node = self.node(child)?;
                if slottable(node.kind()) {
                    let name = match node.kind() {
                        NodeKind::Element(element) => attribute(element, "slot"),
                        _ => &[],
                    };
                    if let Some(&slot) = names.get(name) {
                        map.add(slot, child)?;
                    }
                }
            }
        }
        Ok(map)
    }
}

impl SlotMap<'_> {
    fn add(&mut self, slot: NodeId, node: NodeId) -> Result<(), DomError> {
        self.assigned
            .try_reserve(1)
            .map_err(|_| DomError::AllocationFailed)?;
        self.by_node
            .try_reserve(1)
            .map_err(|_| DomError::AllocationFailed)?;
        let nodes = self.assigned.entry(slot).or_default();
        nodes
            .try_reserve(1)
            .map_err(|_| DomError::AllocationFailed)?;
        nodes.push(node);
        self.by_node.insert(node, slot);
        Ok(())
    }

    /// Slots in shadow trees. Slots in a given tree are in tree order.
    pub fn slots(&self) -> &[NodeId] {
        &self.slots
    }

    /// The assigned slot, irrespective of the root's open or closed mode.
    pub fn assigned_slot(&self, node: NodeId) -> Option<NodeId> {
        self.by_node.get(&node).copied()
    }

    /// The slottables directly assigned to this slot, in assignment order.
    pub fn assigned_nodes(&self, slot: NodeId) -> &[NodeId] {
        self.assigned.get(&slot).map_or(&[], Vec::as_slice)
    }

    /// Flatten assigned nodes and, for an empty slot, its fallback slottables.
    pub fn flattened_nodes(&self, slot: NodeId) -> Result<Vec<NodeId>, DomError> {
        let doc = self.document;
        if !self.active.contains(&slot) {
            return Ok(Vec::new());
        }
        let mut pending = vec![slot];
        let mut result = Vec::new();
        while let Some(node) = pending.pop() {
            if self.active.contains(&node) {
                let assigned = self.assigned_nodes(node);
                if assigned.is_empty() {
                    let mut next = doc.node(node)?.last_child();
                    while let Some(child) = next {
                        let entry = doc.node(child)?;
                        next = entry.previous_sibling();
                        if slottable(entry.kind()) {
                            pending
                                .try_reserve(1)
                                .map_err(|_| DomError::AllocationFailed)?;
                            pending.push(child);
                        }
                    }
                } else {
                    pending
                        .try_reserve(assigned.len())
                        .map_err(|_| DomError::AllocationFailed)?;
                    pending.extend(assigned.iter().rev().copied());
                }
            } else {
                result
                    .try_reserve(1)
                    .map_err(|_| DomError::AllocationFailed)?;
                result.push(node);
            }
        }
        Ok(result)
    }

    /// Traverse the rendered tree in preorder. Hosts expose their shadow
    /// children, slots expose assigned nodes or fallback, and unassigned
    /// light children are omitted.
    pub fn flat_walk(&self, root: NodeId) -> Result<FlatWalk<'_>, DomError> {
        self.document.node(root)?;
        Ok(FlatWalk {
            slots: self,
            root,
            pending: Vec::new(),
            next: Some((root, 0)),
        })
    }
}

/// Iterative preorder traversal over a slot-distributed tree.
pub struct FlatWalk<'a> {
    slots: &'a SlotMap<'a>,
    root: NodeId,
    pending: Vec<(NodeId, usize)>,
    next: Option<(NodeId, usize)>,
}

impl Iterator for FlatWalk<'_> {
    type Item = (NodeId, usize);

    fn next(&mut self) -> Option<Self::Item> {
        let (id, depth) = self.next.take().or_else(|| self.pending.pop())?;
        let doc = self.slots.document;
        let node = doc.live(id);
        if id != self.root {
            let assigned = self.slots.assigned_slot(id).is_some();
            if !assigned {
                self.next = node.next_sibling().map(|sibling| (sibling, depth));
            }
        }
        let assigned = self.slots.assigned_nodes(id);
        let child = if assigned.is_empty() {
            match node.kind() {
                NodeKind::Element(element) => element
                    .shadow_root()
                    .map_or_else(|| node.first_child(), |root| doc.live(root).first_child()),
                _ => node.first_child(),
            }
        } else {
            assigned.first().copied()
        };
        if let Some(child) = child {
            self.pending.extend(self.next.take());
            self.pending
                .extend(assigned.iter().skip(1).rev().map(|&id| (id, depth + 1)));
            self.next = Some((child, depth + 1));
        }
        Some((id, depth))
    }
}
