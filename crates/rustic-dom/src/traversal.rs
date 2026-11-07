use std::iter::FusedIterator;

use rustic_base::NodeId;

use crate::Document;

/// Stack-free preorder traversal, including the selected root at relative depth 0.
///
/// The immutable document borrow prevents mutation during traversal. It visits
/// only the chosen subtree, including when that subtree is detached.
#[derive(Debug)]
pub struct Walk<'a> {
    document: &'a Document,
    root: NodeId,
    next: Option<NodeId>,
    depth: usize,
}

impl<'a> Walk<'a> {
    pub(crate) fn new(document: &'a Document, root: NodeId) -> Self {
        Self {
            document,
            root,
            next: Some(root),
            depth: 0,
        }
    }
}

impl Iterator for Walk<'_> {
    type Item = (NodeId, usize);

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.next?;
        let result = (current, self.depth);
        if let Some(child) = self
            .document
            .node(current)
            .expect("validated traversal node")
            .first_child()
        {
            self.next = Some(child);
            self.depth += 1;
        } else {
            let mut cursor = current;
            self.next = None;
            while cursor != self.root {
                let node = self
                    .document
                    .node(cursor)
                    .expect("validated traversal link");
                if let Some(sibling) = node.next_sibling() {
                    self.next = Some(sibling);
                    break;
                }
                cursor = node.parent().expect("non-root traversal node has a parent");
                self.depth -= 1;
            }
        }
        Some(result)
    }
}

impl FusedIterator for Walk<'_> {}
