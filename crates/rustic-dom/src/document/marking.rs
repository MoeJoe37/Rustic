//! The DOM's half of garbage collection (ADR 0012, ADR 0016).
//!
//! The JavaScript heap owns wrappers, listeners and every other script
//! object; the document owns nodes. A collection runs one marking phase over
//! both: whenever the JS collector reaches a node (through a wrapper or a
//! host edge), the binding layer asks the document to mark that node's whole
//! tree, because parent, sibling and child links are visible to script.
//! Template contents and shadow roots live and die with their owning
//! element. The document's own tree is always live. After marking, every
//! unmarked tree is destroyed through [`Document::destroy_subtree`].

use rustic_base::{DocumentId, NodeId};

use crate::{Document, DomError, NodeKind};

/// Mark bits for one collection over one document's nodes.
#[derive(Clone, Debug)]
pub struct TreeMarks {
    document: DocumentId,
    marks: Vec<bool>,
    /// Scratch: tree roots waiting to be walked.
    pending: Vec<NodeId>,
}

impl TreeMarks {
    /// Whether `node` was marked live.
    pub fn is_marked(&self, node: NodeId) -> bool {
        node.document() == self.document
            && self
                .marks
                .get(node.slot() as usize)
                .copied()
                .unwrap_or(false)
    }
}

impl Document {
    /// Empty marks for a collection. The document must stay unchanged
    /// until marking and sweeping finish.
    pub fn tree_marks(&self) -> Result<TreeMarks, DomError> {
        let slots = self.arena.slot_count();
        let mut marks = Vec::new();
        marks
            .try_reserve_exact(slots)
            .map_err(|_| DomError::AllocationFailed)?;
        marks.resize(slots, false);
        Ok(TreeMarks {
            document: self.id(),
            marks,
            pending: Vec::new(),
        })
    }

    /// Mark the tree holding `node`, together with the trees its elements
    /// own (template contents, shadow roots) and the element owning it, and
    /// call `visit` once for every node newly marked. Marking is iterative.
    pub fn mark_tree(
        &self,
        marks: &mut TreeMarks,
        node: NodeId,
        visit: &mut dyn FnMut(NodeId),
    ) -> Result<(), DomError> {
        if marks.document != self.id() {
            return Err(DomError::WrongDocument);
        }
        self.node(node)?;
        if marks.is_marked(node) {
            return Ok(());
        }
        marks.pending.push(node);
        while let Some(start) = marks.pending.pop() {
            let mut tree = start;
            while let Some(parent) = self.live(tree).parent() {
                tree = parent;
            }
            if marks.marks[tree.slot() as usize] {
                continue;
            }
            if let Some(host) = self.live(tree).owner_host() {
                marks.pending.push(host);
            }
            for (id, _) in self.walk(tree)? {
                marks.marks[id.slot() as usize] = true;
                visit(id);
                if let NodeKind::Element(element) = self.live(id).kind() {
                    marks.pending.extend(element.owned_roots());
                }
            }
        }
        Ok(())
    }

    /// Mark the document's own tree, which is always live.
    pub fn mark_document_tree(
        &self,
        marks: &mut TreeMarks,
        visit: &mut dyn FnMut(NodeId),
    ) -> Result<(), DomError> {
        self.mark_tree(marks, self.root(), visit)
    }

    /// Destroy every tree `marks` did not reach and return how many nodes
    /// went. Template contents and shadow roots go with their owner.
    pub fn sweep_unmarked_trees(&mut self, marks: &TreeMarks) -> Result<usize, DomError> {
        if marks.document != self.id() {
            return Err(DomError::WrongDocument);
        }
        let before = self.node_count();
        for slot in 0..self.arena.slot_count() {
            let Some(id) = self.arena.live_id(slot) else {
                continue;
            };
            let node = self.live(id);
            if !marks.is_marked(id) && node.parent().is_none() && node.owner_host().is_none() {
                self.destroy_subtree(id)?;
            }
        }
        Ok(before - self.node_count())
    }
}
