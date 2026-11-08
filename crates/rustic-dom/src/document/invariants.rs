//! Whole-document structural invariant checker, used as a test and fuzz oracle.

use std::fmt;

use rustic_base::NodeId;

use crate::{Document, NodeKind};

/// A broken structural invariant: the node where it was found and the rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvariantViolation {
    /// The node at which the check failed.
    pub node: NodeId,
    /// Short description of the rule that failed.
    pub rule: &'static str,
}

impl fmt::Display for InvariantViolation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "DOM invariant violated at {:?}: {}",
            self.node, self.rule
        )
    }
}

impl std::error::Error for InvariantViolation {}

impl Document {
    /// Check every structural invariant of the document in linear time.
    ///
    /// This is a verification aid for tests, fuzzing and the DOM conformance
    /// gate, not a runtime safety mechanism: checked mutation APIs maintain these
    /// rules, so a violation is an engine bug. It checks link symmetry, acyclic
    /// parent chains, the depth limit, permitted parent/child kinds, document
    /// child order, template contents ownership, and node/payload accounting.
    /// It allocates one byte per storage slot and nothing per node.
    pub fn check_invariants(&self) -> Result<(), InvariantViolation> {
        let fail = |node, rule| Err(InvariantViolation { node, rule });
        let root = self.root;
        match self.node(root) {
            Ok(node) if matches!(node.kind(), NodeKind::Document) && node.parent().is_none() => {}
            _ => return fail(root, "root must be a parentless document node"),
        }

        // Pass 1: every link points at a live node and is mirrored.
        let mut live = 0usize;
        let mut bytes = 0usize;
        for id in self.arena.live_ids() {
            live += 1;
            let node = self.live(id);
            let Ok(expected) = node.kind().data_bytes() else {
                return fail(id, "payload size overflows");
            };
            if expected != node.data_bytes {
                return fail(id, "accounted payload differs from the node's data");
            }
            bytes = bytes.saturating_add(node.data_bytes);
            if matches!(node.kind(), NodeKind::Document) && id != root {
                return fail(id, "only the root may be a document node");
            }
            for link in [
                node.parent(),
                node.first_child(),
                node.last_child(),
                node.previous_sibling(),
                node.next_sibling(),
            ]
            .into_iter()
            .flatten()
            {
                if self.node(link).is_err() {
                    return fail(id, "link names a stale or foreign node");
                }
            }
            match (node.first_child(), node.last_child()) {
                (None, None) => {}
                (Some(first), Some(last)) => {
                    let (first, last) = (self.live(first), self.live(last));
                    if first.parent() != Some(id) || last.parent() != Some(id) {
                        return fail(id, "first/last child has another parent");
                    }
                    if first.previous_sibling().is_some() || last.next_sibling().is_some() {
                        return fail(id, "first/last child has an outer sibling");
                    }
                    if !matches!(
                        node.kind(),
                        NodeKind::Document | NodeKind::Element(_) | NodeKind::DocumentFragment
                    ) {
                        return fail(id, "node kind cannot have children");
                    }
                }
                _ => return fail(id, "only one of first/last child is set"),
            }
            match node.parent() {
                None => {
                    if node.previous_sibling().is_some() || node.next_sibling().is_some() {
                        return fail(id, "parentless node has siblings");
                    }
                }
                Some(parent) => {
                    let parent_node = self.live(parent);
                    let previous_ok = match node.previous_sibling() {
                        Some(previous) => {
                            let previous = self.live(previous);
                            previous.next_sibling() == Some(id) && previous.parent() == Some(parent)
                        }
                        None => parent_node.first_child() == Some(id),
                    };
                    let next_ok = match node.next_sibling() {
                        Some(next) => {
                            let next = self.live(next);
                            next.previous_sibling() == Some(id) && next.parent() == Some(parent)
                        }
                        None => parent_node.last_child() == Some(id),
                    };
                    if !previous_ok || !next_ok {
                        return fail(id, "sibling links are not mirrored");
                    }
                    if matches!(node.kind(), NodeKind::DocumentFragment) {
                        return fail(id, "a fragment cannot be a child");
                    }
                    if matches!(node.kind(), NodeKind::DocumentType(_)) && parent != root {
                        return fail(id, "a doctype must be a child of the document");
                    }
                }
            }
        }
        if live != self.node_count() {
            return fail(root, "live slots differ from the node budget");
        }
        if bytes != self.data_bytes() {
            return fail(root, "payload bytes differ from the data budget");
        }

        // Pass 2: template contents are owned by exactly one HTML template, are
        // parentless, and name that template as their host. Marks are per slot.
        let mut owned = vec![false; self.arena.slot_count()];
        for id in self.arena.live_ids() {
            if let NodeKind::Element(element) = self.live(id).kind() {
                let Some(contents) = element.template_contents() else {
                    continue;
                };
                if element.name().namespace() != Some(crate::HTML_NAMESPACE)
                    || element.name().local() != "template"
                {
                    return fail(id, "only an HTML template owns contents");
                }
                let Ok(fragment) = self.node(contents) else {
                    return fail(id, "template contents are stale");
                };
                if !matches!(fragment.kind(), NodeKind::DocumentFragment)
                    || fragment.parent().is_some()
                    || fragment.template_host() != Some(id)
                {
                    return fail(contents, "template contents must be a parentless fragment");
                }
                let mark = &mut owned[contents.slot() as usize];
                if *mark {
                    return fail(contents, "template contents have two owners");
                }
                *mark = true;
            }
        }

        for id in self.arena.live_ids() {
            if self.live(id).template_host().is_some() && !owned[id.slot() as usize] {
                return fail(id, "template host does not own these contents");
            }
        }

        // Pass 3: parent chains are acyclic and within the depth limit. Every
        // parentless node is a tree root; walking each once visits each node
        // once, so a node not reached here lies on a parent-link cycle.
        let mut reached = 0usize;
        for id in self.arena.live_ids() {
            if self.live(id).parent().is_some() {
                continue;
            }
            for (node, depth) in self.walk(id).expect("live root") {
                reached += 1;
                if reached > live {
                    return fail(node, "child links revisit a node");
                }
                if depth > self.limits.max_depth {
                    return fail(node, "tree exceeds the depth limit");
                }
            }
        }
        if reached != live {
            return fail(root, "a parent chain is cyclic");
        }

        // Pass 4: document children follow the doctype/element order rules.
        let mut element_seen = false;
        let mut doctype_seen = false;
        for child in self.children(root).expect("live root") {
            match self.live(child).kind() {
                NodeKind::Comment(_) | NodeKind::ProcessingInstruction(_) => {}
                NodeKind::Element(_) if !element_seen => element_seen = true,
                NodeKind::DocumentType(_) if !doctype_seen && !element_seen => doctype_seen = true,
                _ => return fail(child, "document children break doctype/element order"),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{Document, DocumentLimits, QualifiedName};

    fn tree() -> (Document, [rustic_base::NodeId; 3]) {
        let mut doc = Document::new(DocumentLimits::default()).unwrap();
        let html = doc
            .create_element(QualifiedName::html("html").unwrap())
            .unwrap();
        let a = doc
            .create_element(QualifiedName::html("a").unwrap())
            .unwrap();
        let b = doc.create_text("b").unwrap();
        doc.append_child(doc.root(), html).unwrap();
        doc.append_child(html, a).unwrap();
        doc.append_child(html, b).unwrap();
        doc.check_invariants().unwrap();
        (doc, [html, a, b])
    }

    // Each corruption bypasses the checked APIs, as an engine bug would.
    #[test]
    fn corrupted_links_kinds_and_accounting_are_detected() {
        let rule = |doc: &Document| doc.check_invariants().unwrap_err().rule;

        let (mut doc, [_, a, _]) = tree();
        doc.live_mut(a).next_sibling = None;
        assert_eq!(rule(&doc), "sibling links are not mirrored");

        let (mut doc, [html, _, _]) = tree();
        doc.live_mut(html).last_child = None;
        assert_eq!(rule(&doc), "only one of first/last child is set");

        let (mut doc, [_, a, _]) = tree();
        doc.live_mut(a).data_bytes += 1;
        assert_eq!(rule(&doc), "accounted payload differs from the node's data");

        let (mut doc, [_, _, b]) = tree();
        let text = doc.create_text("c").unwrap();
        doc.live_mut(b).first_child = Some(text);
        doc.live_mut(b).last_child = Some(text);
        doc.live_mut(text).parent = Some(b);
        assert_eq!(rule(&doc), "node kind cannot have children");

        let (mut doc, _) = tree();
        // Two detached elements whose parent links form a cycle with no root.
        let x = doc
            .create_element(QualifiedName::html("x").unwrap())
            .unwrap();
        let y = doc
            .create_element(QualifiedName::html("y").unwrap())
            .unwrap();
        for (node, other) in [(x, y), (y, x)] {
            let node = doc.live_mut(node);
            node.parent = Some(other);
            node.first_child = Some(other);
            node.last_child = Some(other);
        }
        assert_eq!(rule(&doc), "a parent chain is cyclic");

        let (mut doc, _) = tree();
        let second = doc
            .create_element(QualifiedName::html("body").unwrap())
            .unwrap();
        doc.link_last_child(doc.root, second);
        assert_eq!(rule(&doc), "document children break doctype/element order");
    }
}
