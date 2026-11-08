//! Node cloning and cross-document adoption.
//!
//! Both operations first take a snapshot of the source subtree (a flat preorder
//! plan with parent indexes), then materialize it in the target document. The
//! plan is bounded by the source's live nodes and is built without recursion.
//! Materialization rolls back on any failure, so a rejected clone or adoption
//! leaves both documents, their IDs and their accounting unchanged.

use rustic_base::NodeId;

use super::Document;
use crate::{DomError, Element, NodeKind};

/// The result of [`Document::adopt_node`]: the new root plus an ID map.
///
/// Node IDs embed their document, so adoption gives every node a new ID in the
/// target document. `ids` maps each old ID to its new ID in preorder (template
/// contents follow their template's subtree), with the root first. A future
/// binding layer uses this map to retarget wrappers; the old IDs are invalid.
#[derive(Debug, Eq, PartialEq)]
pub struct Adopted {
    /// The adopted subtree's root in the target document.
    pub root: NodeId,
    /// `(old, new)` pairs for every adopted node, root first.
    pub ids: Vec<(NodeId, NodeId)>,
}

/// One node to create: a payload copy and where it attaches.
struct Planned {
    source: NodeId,
    kind: NodeKind,
    link: Link,
}

#[derive(Clone, Copy)]
enum Link {
    /// The root of the copied subtree.
    Root,
    /// Last child of an earlier planned node.
    ChildOf(usize),
    /// Template contents of an earlier planned template element.
    ContentsOf(usize),
}

impl Document {
    /// Copy a node (and with `deep`, its descendants) into a new detached tree.
    ///
    /// The copy has the same kinds, names, attributes in order and character
    /// data, and new IDs. Following the HTML cloning steps, a `template` copy gets
    /// its own contents fragment, which a deep clone fills with a copy of the
    /// original contents. Cloning template contents yields a free-standing
    /// fragment. The document root cannot be cloned here; use
    /// [`Self::clone_document`]. Limit or allocation failure creates nothing.
    pub fn clone_node(&mut self, node: NodeId, deep: bool) -> Result<NodeId, DomError> {
        if node == self.root {
            return Err(DomError::WrongNodeKind);
        }
        let plan = snapshot(self, node, deep)?;
        Ok(self.materialize(plan)?[0])
    }

    /// Move a subtree from `source` into this document, as `adoptNode` does.
    ///
    /// The subtree is detached from its source parent, copied here with new IDs
    /// (including nested template contents), and then destroyed in `source`, so
    /// every old ID becomes invalid. The source root and template contents (which
    /// belong to their template) cannot be adopted. Any failure, including this
    /// document's node, data or depth limits, leaves both documents unchanged.
    pub fn adopt_node(&mut self, source: &mut Document, node: NodeId) -> Result<Adopted, DomError> {
        if node == source.root {
            return Err(DomError::ProtectedRoot);
        }
        if source.node(node)?.template_host().is_some() {
            return Err(DomError::InvalidHierarchy);
        }
        let plan = snapshot(source, node, true)?;
        let mut ids = Vec::new();
        ids.try_reserve_exact(plan.len())
            .map_err(|_| DomError::AllocationFailed)?;
        ids.extend(plan.iter().map(|item| (item.source, item.source)));
        let created = self.materialize(plan)?;
        for (pair, new) in ids.iter_mut().zip(created) {
            pair.1 = new;
        }
        source.destroy_unchecked(node);
        Ok(Adopted {
            root: ids[0].1,
            ids,
        })
    }

    /// Create a new document with the same limits and a deep copy of this tree.
    ///
    /// The copy is `Complete` (readiness is not copied) and has a new document
    /// ID. It fails, creating nothing, if allocation fails.
    pub fn clone_document(&self) -> Result<Document, DomError> {
        let mut copy = Document::new(self.limits)?;
        for child in self.children(self.root)? {
            let plan = snapshot(self, child, true)?;
            let root = copy.materialize(plan)?[0];
            copy.link_before(copy.root, root, None);
        }
        Ok(copy)
    }

    /// Create every planned node; on failure destroy what was created.
    fn materialize(&mut self, plan: Vec<Planned>) -> Result<Vec<NodeId>, DomError> {
        let mut created: Vec<NodeId> = Vec::new();
        created
            .try_reserve_exact(plan.len())
            .map_err(|_| DomError::AllocationFailed)?;
        for item in plan {
            let id = match self.arena.insert(item.kind) {
                Ok(id) => id,
                Err(error) => {
                    if let Some(&root) = created.first() {
                        self.destroy_unchecked(root);
                    }
                    return Err(error);
                }
            };
            match item.link {
                Link::Root => {}
                Link::ChildOf(parent) => self.link_before(created[parent], id, None),
                Link::ContentsOf(template) => {
                    let template = created[template];
                    self.live_mut(id).template_host = Some(template);
                    self.element_mut(template).template_contents = Some(id);
                }
            }
            created.push(id);
        }
        // The copy has the source subtree's height, but the target's limit may
        // be lower (adoption), so check it once the tree exists.
        let root = created[0];
        if self.subtree_height(root)? > self.limits.max_depth {
            self.destroy_unchecked(root);
            return Err(DomError::DepthLimit);
        }
        Ok(created)
    }

    /// Greatest edge depth within the tree rooted at `root` or any template
    /// contents nested in it (each contents fragment is its own tree root).
    fn subtree_height(&self, root: NodeId) -> Result<usize, DomError> {
        let mut height = 0;
        let mut roots = vec![root];
        while let Some(next) = roots.pop() {
            for (id, depth) in self.walk(next)? {
                height = height.max(depth);
                if let NodeKind::Element(element) = self.live(id).kind() {
                    roots.extend(element.template_contents());
                }
            }
        }
        Ok(height)
    }
}

/// Plan a copy of `root`, its descendants when `deep`, and template contents.
fn snapshot(document: &Document, root: NodeId, deep: bool) -> Result<Vec<Planned>, DomError> {
    document.node(root)?;
    let mut plan: Vec<Planned> = Vec::new();
    // Pending tree roots: (source root, link of that root).
    let mut pending = vec![(root, Link::Root)];
    while let Some((tree, link)) = pending.pop() {
        // parents[d] is the plan index of the current ancestor at depth d.
        let mut parents: Vec<usize> = Vec::new();
        for (id, depth) in document.walk(tree)? {
            if depth > 0 && !deep {
                break;
            }
            let node = document.live(id);
            parents.truncate(depth);
            let link = match depth {
                0 => link,
                _ => Link::ChildOf(parents[depth - 1]),
            };
            let index = plan.len();
            plan.try_reserve(1)
                .map_err(|_| DomError::AllocationFailed)?;
            plan.push(Planned {
                source: id,
                kind: copy_kind(node.kind()),
                link,
            });
            parents.push(index);
            if let NodeKind::Element(element) = node.kind() {
                if let Some(contents) = element.template_contents() {
                    if deep {
                        pending.push((contents, Link::ContentsOf(index)));
                    } else {
                        plan.try_reserve(1)
                            .map_err(|_| DomError::AllocationFailed)?;
                        plan.push(Planned {
                            source: contents,
                            kind: NodeKind::DocumentFragment,
                            link: Link::ContentsOf(index),
                        });
                    }
                }
            }
        }
    }
    Ok(plan)
}

/// Copy a payload; template links are rebuilt by the materializer.
fn copy_kind(kind: &NodeKind) -> NodeKind {
    match kind {
        NodeKind::Document => NodeKind::Document,
        NodeKind::DocumentFragment => NodeKind::DocumentFragment,
        NodeKind::Element(element) => NodeKind::Element(Element {
            name: element.name.clone(),
            attributes: element.attributes.clone(),
            template_contents: None,
        }),
        NodeKind::Text(value) => NodeKind::Text(value.clone()),
        NodeKind::Comment(value) => NodeKind::Comment(value.clone()),
        NodeKind::CdataSection(value) => NodeKind::CdataSection(value.clone()),
        NodeKind::ProcessingInstruction(value) => NodeKind::ProcessingInstruction(value.clone()),
        NodeKind::DocumentType(value) => NodeKind::DocumentType(value.clone()),
    }
}
