//! Node cloning and cross-document adoption.
//!
//! Both operations first take a snapshot of the source subtree (a flat preorder
//! plan with parent indexes), then materialize it in the target document. The
//! plan is bounded by the source's live nodes and is built without recursion.
//! Materialization rolls back on any failure, so a rejected clone or adoption
//! leaves both documents, their IDs and their accounting unchanged.

use rustic_base::NodeId;

use super::{Document, MutationEntry};
use crate::{DomError, Element, NodeKind, ShadowRootOptions};

/// The result of [`Document::adopt_node`]: the new root plus an ID map.
///
/// Node IDs embed their document, so adoption gives every node a new ID in the
/// target document. `ids` maps each old ID to its new ID in preorder (template
/// contents and shadow trees follow the whole tree they hang off), with the
/// root first. The binding layer uses this map to retarget wrappers; the old
/// IDs are invalid.
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
    /// Shadow root of an earlier planned host element.
    ShadowOf(usize, ShadowRootOptions),
}

impl Document {
    /// Copy a node (and with `deep`, its descendants) into a new detached tree.
    ///
    /// The copy has the same kinds, names, attributes in order and character
    /// data, and new IDs. Following the HTML cloning steps, a `template` copy gets
    /// its own contents fragment, which a deep clone fills with a copy of the
    /// original contents. Cloning template contents or a shadow root yields a
    /// free-standing fragment. A clonable shadow root is copied with its host. The document root cannot be cloned here; use
    /// [`Self::clone_document`]. Limit or allocation failure creates nothing.
    pub fn clone_node(&mut self, node: NodeId, deep: bool) -> Result<NodeId, DomError> {
        if node == self.root {
            return Err(DomError::WrongNodeKind);
        }
        let plan = snapshot(self, node, deep, false)?;
        Ok(self.materialize(plan)?[0])
    }

    /// Copy a non-document node into this document without changing the source.
    pub fn import_node(
        &mut self,
        source: &Document,
        node: NodeId,
        deep: bool,
    ) -> Result<NodeId, DomError> {
        if node == source.root {
            return Err(DomError::ProtectedRoot);
        }
        let plan = snapshot(source, node, deep, false)?;
        Ok(self.materialize(plan)?[0])
    }

    /// Move a subtree from `source` into this document, as `adoptNode` does.
    ///
    /// The subtree is detached from its source parent, copied here with new IDs
    /// (including nested template contents and shadow roots), and then
    /// destroyed in `source`, so every old ID becomes invalid. The source root,
    /// template contents and shadow roots (which belong to their element)
    /// cannot be adopted. Any failure, including this
    /// document's node, data or depth limits, leaves both documents unchanged.
    pub fn adopt_node(&mut self, source: &mut Document, node: NodeId) -> Result<Adopted, DomError> {
        self.adopt_node_with_preflight(source, node, |_| Ok(()))
    }

    /// Adopt after the host reserves any storage needed to retarget its
    /// handles. The callback receives the number of identities to be moved
    /// and runs before either document changes.
    pub fn adopt_node_with_preflight(
        &mut self,
        source: &mut Document,
        node: NodeId,
        preflight: impl FnOnce(usize) -> Result<(), DomError>,
    ) -> Result<Adopted, DomError> {
        self.transfer(source, node, None, preflight)
    }

    /// Adopt and insert in one checked operation. Inserting a fragment moves
    /// only its children; the empty fragment retains its owner and identity.
    pub fn insert_from_document(
        &mut self,
        source: &mut Document,
        node: NodeId,
        parent: NodeId,
        before: Option<NodeId>,
        preflight: impl FnOnce(usize) -> Result<(), DomError>,
    ) -> Result<Adopted, DomError> {
        self.transfer(source, node, Some((parent, before)), preflight)
    }

    fn transfer(
        &mut self,
        source: &mut Document,
        node: NodeId,
        insert: Option<(NodeId, Option<NodeId>)>,
        preflight: impl FnOnce(usize) -> Result<(), DomError>,
    ) -> Result<Adopted, DomError> {
        if node == source.root {
            return Err(DomError::ProtectedRoot);
        }
        let node_kind = source.node(node)?;
        if node_kind.owner_host().is_some() {
            return Err(DomError::InvalidHierarchy);
        }
        let fragment = insert.is_some() && matches!(node_kind.kind(), NodeKind::DocumentFragment);
        if let Some((parent, before)) = insert {
            if !matches!(
                self.node(parent)?.kind(),
                NodeKind::Document | NodeKind::DocumentFragment | NodeKind::Element(_)
            ) {
                return Err(DomError::InvalidHierarchy);
            }
            if let Some(before) = before {
                if self.node(before)?.parent() != Some(parent) {
                    return Err(DomError::InvalidReference);
                }
            }
            if fragment && node_kind.first_child().is_none() {
                return Ok(Adopted {
                    root: node,
                    ids: Vec::new(),
                });
            }
        }
        let mut plan = snapshot(source, node, true, true)?;
        let mut roots = Vec::new();
        if fragment {
            roots
                .try_reserve_exact(source.children(node)?.count())
                .map_err(|_| DomError::AllocationFailed)?;
            // The fragment itself keeps its original identity and takes no
            // space in the destination. Its former children are forest roots.
            plan.remove(0);
            for item in &mut plan {
                item.link = match item.link {
                    Link::ChildOf(0) => Link::Root,
                    Link::ChildOf(index) => Link::ChildOf(index - 1),
                    Link::ContentsOf(index) => Link::ContentsOf(index - 1),
                    Link::ShadowOf(index, mode) => Link::ShadowOf(index - 1, mode),
                    Link::Root => unreachable!("only the fragment was a root"),
                };
            }
        }
        let mut ids = Vec::new();
        ids.try_reserve_exact(plan.len())
            .map_err(|_| DomError::AllocationFailed)?;
        ids.extend(plan.iter().map(|item| (item.source, item.source)));
        // The DOM's removal record: from the old parent, or the fragment's
        // children. It lists the moved nodes by their new identities.
        let old = source.live(node);
        let removal = match (fragment, old.parent()) {
            (true, _) => Some((node, None, None, roots.capacity())),
            (false, Some(parent)) => Some((parent, old.previous_sibling(), old.next_sibling(), 1)),
            (false, None) => None,
        };
        let into_self = self.mutation_journal_enabled();
        let mut removed = Vec::new();
        let removal = match removal {
            Some(removal)
                if (into_self && self.reserve_journal(2)?)
                    || (!into_self && source.reserve_journal(1)?) =>
            {
                removed
                    .try_reserve_exact(removal.3)
                    .map_err(|_| DomError::AllocationFailed)?;
                Some(removal)
            }
            _ => None,
        };
        preflight(plan.len())?;
        let created = self.materialize(plan)?;
        for (pair, &new) in ids.iter_mut().zip(&created) {
            pair.1 = new;
        }
        if let Err(error) = self.adopt_manual_slots(source, &ids) {
            self.destroy_materialized(&created);
            return Err(error);
        }
        let root = ids[0].1;
        if fragment {
            roots.extend(
                created
                    .iter()
                    .copied()
                    .filter(|&id| self.is_detached_root(id)),
            );
        }
        if let Some((parent, before)) = insert {
            let result = if fragment {
                self.insert_forest(parent, before, &roots)
            } else {
                self.insert_before(parent, root, before)
            };
            if let Err(error) = result {
                self.destroy_materialized(&created);
                return Err(error);
            }
        }
        if let Some((target, previous, next, _)) = removal {
            if fragment {
                removed.extend_from_slice(&roots);
            } else {
                removed.push(root);
            }
            let entry = MutationEntry::ChildList {
                target,
                connected: source.is_connected(target),
                added: Vec::new(),
                removed,
                previous,
                next,
            };
            if into_self {
                // The insertion entry was pushed first; the removal precedes it.
                let log = self.journal.as_mut().expect("enabled journal");
                let at = log.len() - usize::from(insert.is_some());
                log.insert(at, entry);
            } else {
                source.push_reserved(entry);
            }
        }
        if fragment {
            while let Some(child) = source.live(node).first_child() {
                source.destroy_unchecked(child);
            }
            return Ok(Adopted { root: node, ids });
        }
        // Ranges and iterators inside a detached subtree follow its nodes
        // (an attached one's removal already moved them to its old parent).
        if source.live(node).parent().is_none() {
            self.adopt_live(source, &ids);
        }
        source.destroy_unchecked(node);
        Ok(Adopted { root, ids })
    }

    fn adopt_manual_slots(
        &mut self,
        source: &Document,
        ids: &[(NodeId, NodeId)],
    ) -> Result<(), DomError> {
        if source.manual_slots.is_empty() {
            return Ok(());
        }
        let mut renames = std::collections::HashMap::new();
        renames
            .try_reserve(ids.len())
            .map_err(|_| DomError::AllocationFailed)?;
        renames.extend(ids.iter().copied());
        for &(old, new) in ids {
            let Some(assigned) = source.manual_slots.get(&old) else {
                continue;
            };
            let mut moved = Vec::new();
            moved
                .try_reserve(assigned.len())
                .map_err(|_| DomError::AllocationFailed)?;
            moved.extend(assigned.iter().filter_map(|id| renames.get(id).copied()));
            self.assign_slot(new, &moved)?;
        }
        Ok(())
    }

    fn insert_forest(
        &mut self,
        parent: NodeId,
        before: Option<NodeId>,
        roots: &[NodeId],
    ) -> Result<(), DomError> {
        let mut at = parent;
        let mut depth = 0usize;
        while let Some(ancestor) = self.live(at).parent() {
            at = ancestor;
            depth += 1;
        }
        for &root in roots {
            if parent != self.root && matches!(self.live(root).kind(), NodeKind::DocumentType(_)) {
                return Err(DomError::InvalidHierarchy);
            }
            let height = self.walk(root)?.map(|(_, depth)| depth).max().unwrap_or(0);
            depth
                .checked_add(1)
                .and_then(|depth| depth.checked_add(height))
                .filter(|&depth| depth <= self.limits.max_depth)
                .ok_or(DomError::DepthLimit)?;
        }
        if parent == self.root {
            self.validate_document_order(roots, before)?;
        }
        let entry = if self.reserve_journal(1)? {
            let previous = match before {
                Some(id) => self.live(id).previous_sibling(),
                None => self.live(parent).last_child(),
            };
            Some(MutationEntry::ChildList {
                target: parent,
                connected: self.is_connected(parent),
                added: super::journal::nodes(roots)?,
                removed: Vec::new(),
                previous,
                next: before,
            })
        } else {
            None
        };
        for &root in roots {
            self.link_before(parent, root, before);
        }
        if let Some(entry) = entry {
            self.push_reserved(entry);
        }
        Ok(())
    }

    /// Create a new document with the same limits and a deep copy of this tree.
    ///
    /// The copy is `Complete` (readiness is not copied) and has a new document
    /// ID. It fails, creating nothing, if allocation fails.
    pub fn clone_document(&self) -> Result<Document, DomError> {
        self.clone_document_with_children(true)
    }

    /// Clone this document's format, optionally including its child trees.
    pub fn clone_document_with_children(&self, deep: bool) -> Result<Document, DomError> {
        let mut copy = Document::new_with_format(self.limits, self.format())?;
        if !deep {
            return Ok(copy);
        }
        for child in self.children(self.root)? {
            let plan = snapshot(self, child, true, false)?;
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
                    self.destroy_materialized(&created);
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
                Link::ShadowOf(host, mode) => {
                    let host = created[host];
                    self.live_mut(id).shadow_host = Some(host);
                    self.element_mut(host).shadow_root = Some((id, mode));
                    self.shadow_roots += 1;
                }
            }
            created.push(id);
        }
        // The target may have a lower depth limit than the source.
        let depth_check = created
            .iter()
            .copied()
            .filter(|&id| self.is_detached_root(id))
            .try_for_each(|root| {
                if self.subtree_height(root)? > self.limits.max_depth {
                    Err(DomError::DepthLimit)
                } else {
                    Ok(())
                }
            });
        if let Err(error) = depth_check {
            self.destroy_materialized(&created);
            return Err(error);
        }
        Ok(created)
    }

    fn is_detached_root(&self, id: NodeId) -> bool {
        self.node(id)
            .is_ok_and(|node| node.parent().is_none() && node.owner_host().is_none())
    }

    fn destroy_materialized(&mut self, created: &[NodeId]) {
        for &id in created {
            if self.is_detached_root(id) {
                self.destroy_unchecked(id);
            }
        }
    }

    /// Greatest edge depth within the tree rooted at `root` or any template
    /// contents or shadow root nested in it (each is its own tree root).
    fn subtree_height(&self, root: NodeId) -> Result<usize, DomError> {
        let mut height = 0;
        let mut roots = Vec::new();
        let mut next = Some(root);
        while let Some(root) = next {
            for (id, depth) in self.walk(root)? {
                height = height.max(depth);
                if let NodeKind::Element(element) = self.live(id).kind() {
                    let owned = element.owned_roots();
                    roots
                        .try_reserve(owned.size_hint().1.unwrap_or(0))
                        .map_err(|_| DomError::AllocationFailed)?;
                    roots.extend(owned);
                }
            }
            next = roots.pop();
        }
        Ok(height)
    }
}

/// Plan a copy of `root`, its descendants when `deep`, template contents, and
/// clonable shadow roots. Adoption also copies non-clonable shadow roots.
fn snapshot(
    document: &Document,
    root: NodeId,
    deep: bool,
    adopting: bool,
) -> Result<Vec<Planned>, DomError> {
    document.node(root)?;
    let mut plan: Vec<Planned> = Vec::new();
    // Pending tree roots: (source root, link of that root).
    let mut pending = vec![(root, Link::Root, deep)];
    while let Some((tree, link, deep)) = pending.pop() {
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
                kind: copy_kind(node.kind(), adopting),
                link,
            });
            parents.push(index);
            if let NodeKind::Element(element) = node.kind() {
                if let Some((shadow_root, mode)) = element
                    .shadow_root
                    .filter(|(_, options)| adopting || options.clonable)
                {
                    pending.push((shadow_root, Link::ShadowOf(index, mode), true));
                }
                if let Some(contents) = element.template_contents() {
                    if deep {
                        pending.push((contents, Link::ContentsOf(index), true));
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
fn copy_kind(kind: &NodeKind, adoption: bool) -> NodeKind {
    match kind {
        NodeKind::Document => NodeKind::Document,
        NodeKind::DocumentFragment => NodeKind::DocumentFragment,
        NodeKind::Element(element) => NodeKind::Element(Element {
            name: element.name.clone(),
            attributes: element.attributes.clone(),
            custom_is: element.custom_is.clone(),
            keep_registry_null: element.keep_registry_null,
            form_associated: adoption && element.form_associated,
            template_contents: None,
            shadow_root: None,
        }),
        NodeKind::Text(value) => NodeKind::Text(value.clone()),
        NodeKind::Comment(value) => NodeKind::Comment(value.clone()),
        NodeKind::CdataSection(value) => NodeKind::CdataSection(value.clone()),
        NodeKind::ProcessingInstruction(value) => NodeKind::ProcessingInstruction(value.clone()),
        NodeKind::DocumentType(value) => NodeKind::DocumentType(value.clone()),
    }
}
