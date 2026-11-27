//! DOM §5.5 range content operations: `deleteContents`, `extractContents`,
//! `cloneContents`, `insertNode` and `surroundContents`.
//!
//! Each runs in one pass over the range's document with no script in
//! between; the document's live-range steps move every range (this one
//! included) as nodes and data change, and the range is then set to the
//! point the DOM names.

use rustic_dom::{BoundaryPoint, Document, DomError, DomString, NodeId, NodeKind};
use rustic_js::runtime::{Abrupt, Agent, JsResult, Value};

use super::ranges::{contained, points, store};
use super::traversal::{node_of, wrap_node};
use super::{WrapperKind, creation, dom_exception, identity, required_arg, with_page};

fn exception(agent: &mut Agent, name: &str, message: &str) -> Abrupt {
    creation::exception(agent, name, message)
}

fn is_data(document: &Document, node: NodeId) -> bool {
    matches!(
        document.node(node).map(|n| n.kind()),
        Ok(NodeKind::Text(_)
            | NodeKind::Comment(_)
            | NodeKind::CdataSection(_)
            | NodeKind::ProcessingInstruction(_))
    )
}

fn parent(document: &Document, node: NodeId) -> Option<NodeId> {
    document.node(node).ok().and_then(|n| n.parent())
}

fn children(document: &Document, node: NodeId) -> Vec<NodeId> {
    document
        .children(node)
        .map(|c| c.collect())
        .unwrap_or_default()
}

fn length(document: &Document, node: NodeId) -> u32 {
    document.node_length(node).unwrap_or(0) as u32
}

/// Partially contained: an inclusive ancestor of exactly one endpoint's node.
fn partially_contained(
    document: &Document,
    node: NodeId,
    start: BoundaryPoint,
    end: BoundaryPoint,
) -> bool {
    document.is_inclusive_descendant(start.node, node)
        != document.is_inclusive_descendant(end.node, node)
}

/// A copy of `node` (shallow) holding `units` of its data.
fn data_clone(
    document: &mut Document,
    node: NodeId,
    from: u32,
    to: u32,
) -> Result<NodeId, DomError> {
    let units = document.character_data(node)?.as_units()[from as usize..to as usize].to_vec();
    let clone = document.clone_node(node, false)?;
    document.set_character_data(clone, DomString::from_units(units))?;
    Ok(clone)
}

fn cut(document: &mut Document, node: NodeId, from: u32, to: u32) -> Result<(), DomError> {
    document.replace_character_data(node, from as usize, (to - from) as usize, &[])
}

/// The point after the highest ancestor of `start` that is not an inclusive
/// ancestor of `end` (or `start` itself when it contains `end`).
fn collapse_point(document: &Document, start: BoundaryPoint, end: BoundaryPoint) -> BoundaryPoint {
    if document.is_inclusive_descendant(end.node, start.node) {
        return start;
    }
    let mut reference = start.node;
    while let Some(p) = parent(document, reference) {
        if document.is_inclusive_descendant(end.node, p) {
            break;
        }
        reference = p;
    }
    BoundaryPoint {
        node: parent(document, reference).unwrap_or(reference),
        offset: document.index_of(reference).unwrap_or(0) as u32 + 1,
    }
}

enum Failure {
    Dom(DomError),
    Hierarchy,
}

impl From<DomError> for Failure {
    fn from(error: DomError) -> Self {
        Self::Dom(error)
    }
}

/// DOM "extract" (`clone == false`) or "clone the contents" of the range
/// `start`..`end` into a new fragment.
fn copy_out(
    document: &mut Document,
    start: BoundaryPoint,
    end: BoundaryPoint,
    clone: bool,
) -> Result<NodeId, Failure> {
    let fragment = document.create_document_fragment()?;
    if start == end {
        return Ok(fragment);
    }
    if start.node == end.node && is_data(document, start.node) {
        let copy = data_clone(document, start.node, start.offset, end.offset)?;
        document.append_child(fragment, copy)?;
        if !clone {
            cut(document, start.node, start.offset, end.offset)?;
        }
        return Ok(fragment);
    }
    let mut common = start.node;
    while !document.is_inclusive_descendant(end.node, common) {
        common = parent(document, common).ok_or(Failure::Hierarchy)?;
    }
    let kids = children(document, common);
    let first_partial = (!document.is_inclusive_descendant(end.node, start.node))
        .then(|| {
            kids.iter()
                .copied()
                .find(|&c| partially_contained(document, c, start, end))
        })
        .flatten();
    let last_partial = (!document.is_inclusive_descendant(start.node, end.node))
        .then(|| {
            kids.iter()
                .rev()
                .copied()
                .find(|&c| partially_contained(document, c, start, end))
        })
        .flatten();
    let contained_children: Vec<NodeId> = kids
        .iter()
        .copied()
        .filter(|&c| contained(document, c, start, end))
        .collect();
    if contained_children.iter().any(|&c| {
        matches!(
            document.node(c).map(|n| n.kind()),
            Ok(NodeKind::DocumentType(_))
        )
    }) {
        return Err(Failure::Hierarchy);
    }
    if let Some(first) = first_partial {
        if is_data(document, first) {
            let len = length(document, start.node);
            let copy = data_clone(document, start.node, start.offset, len)?;
            document.append_child(fragment, copy)?;
            if !clone {
                cut(document, start.node, start.offset, len)?;
            }
        } else {
            let copy = document.clone_node(first, false)?;
            document.append_child(fragment, copy)?;
            let sub = copy_out(
                document,
                start,
                BoundaryPoint {
                    node: first,
                    offset: length(document, first),
                },
                clone,
            )?;
            document.append_child(copy, sub)?;
        }
    }
    for child in contained_children {
        let node = if clone {
            document.clone_node(child, true)?
        } else {
            child
        };
        document.append_child(fragment, node)?;
    }
    if let Some(last) = last_partial {
        if is_data(document, last) {
            let copy = data_clone(document, end.node, 0, end.offset)?;
            document.append_child(fragment, copy)?;
            if !clone {
                cut(document, end.node, 0, end.offset)?;
            }
        } else {
            let copy = document.clone_node(last, false)?;
            document.append_child(fragment, copy)?;
            let sub = copy_out(
                document,
                BoundaryPoint {
                    node: last,
                    offset: 0,
                },
                end,
                clone,
            )?;
            document.append_child(copy, sub)?;
        }
    }
    Ok(fragment)
}

/// Run `operation` on the range's document with its boundary points.
fn with_range<T>(
    agent: &mut Agent,
    this: Value,
    operation: impl FnOnce(&mut Agent, &mut Document, BoundaryPoint, BoundaryPoint) -> JsResult<T>,
) -> JsResult<T> {
    let (start, end) = points(agent, this)?;
    with_page(agent, |agent, page| {
        page.write(agent, start.node.document(), |agent, document| {
            operation(agent, document, start, end)
        })
    })
}

fn failure(agent: &mut Agent, failure: Failure) -> Abrupt {
    match failure {
        Failure::Dom(error) => dom_exception::from_dom_error(agent, error),
        Failure::Hierarchy => exception(
            agent,
            "HierarchyRequestError",
            "a doctype cannot be moved into a fragment",
        ),
    }
}

fn extract(agent: &mut Agent, this: Value, clone: bool) -> JsResult<Value> {
    let (fragment, point) = with_range(agent, this, |agent, document, start, end| {
        let point = collapse_point(document, start, end);
        let fragment = copy_out(document, start, end, clone).map_err(|f| failure(agent, f))?;
        Ok((fragment, point))
    })?;
    if !clone {
        let (start, end) = points(agent, this)?;
        if start != end || start != point {
            store(agent, this, point, point)?;
        }
    }
    wrap_node(agent, fragment)
}

pub(super) fn extract_contents(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    extract(agent, this, false)
}

pub(super) fn clone_contents(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    extract(agent, this, true)
}

pub(super) fn delete_contents(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let point = with_range(agent, this, |agent, document, start, end| {
        let fail = |agent: &mut Agent, error| dom_exception::from_dom_error(agent, error);
        if start == end {
            return Ok(None);
        }
        if start.node == end.node && is_data(document, start.node) {
            cut(document, start.node, start.offset, end.offset).map_err(|e| fail(agent, e))?;
            return Ok(None);
        }
        // Contained nodes whose parent is not contained, in tree order.
        let root = super::ranges::root_of(document, start.node);
        let mut remove = Vec::new();
        if let Ok(walk) = document.walk(root) {
            for (id, _) in walk {
                if contained(document, id, start, end)
                    && !parent(document, id).is_some_and(|p| contained(document, p, start, end))
                {
                    remove.push(id);
                }
            }
        }
        let point = collapse_point(document, start, end);
        if is_data(document, start.node) {
            let len = length(document, start.node);
            cut(document, start.node, start.offset, len).map_err(|e| fail(agent, e))?;
        }
        for node in remove {
            document.detach(node).map_err(|e| fail(agent, e))?;
        }
        if is_data(document, end.node) {
            cut(document, end.node, 0, end.offset).map_err(|e| fail(agent, e))?;
        }
        Ok(Some(point))
    })?;
    if let Some(point) = point {
        store(agent, this, point, point)?;
    }
    Ok(Value::UNDEFINED)
}

/// DOM "insert" a node into a range; returns whether the range was collapsed
/// and the end it gets then.
fn insert_into(agent: &mut Agent, this: Value, node_value: Value) -> JsResult<()> {
    let node = node_of(agent, node_value)?;
    let (start, end) = points(agent, this)?;
    let collapsed = start == end;
    let new_end = with_page(agent, |agent, page| {
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return agent.type_error("DOM bindings are busy");
        };
        identity(agent, &wrappers, node_value, WrapperKind::Node)?;
        page.write(agent, start.node.document(), |agent, document| {
            let hierarchy = |agent: &mut Agent| {
                exception(
                    agent,
                    "HierarchyRequestError",
                    "the node cannot be inserted at the range's start",
                )
            };
            let kind = document.node(start.node).map(|n| n.kind());
            let text = matches!(kind, Ok(NodeKind::Text(_) | NodeKind::CdataSection(_)));
            let instruction = matches!(
                kind,
                Ok(NodeKind::ProcessingInstruction(_) | NodeKind::Comment(_))
            );
            let start_parent = parent(document, start.node);
            if instruction || (text && start_parent.is_none()) || start.node == node {
                return Err(hierarchy(agent));
            }
            let mut reference = if text {
                Some(start.node)
            } else {
                children(document, start.node)
                    .get(start.offset as usize)
                    .copied()
            };
            let target = match reference {
                Some(r) if text => parent(document, r).unwrap_or(start.node),
                _ => start.node,
            };
            // Pre-insertion validity that splitting must not precede.
            if node.document() == target.document()
                && document.is_inclusive_descendant(target, node)
            {
                return Err(hierarchy(agent));
            }
            if text {
                let split = document
                    .split_text_with_preflight(start.node, start.offset as usize, |_, new| Ok(new))
                    .map_err(|e| dom_exception::from_dom_error(agent, e))?;
                reference = Some(split);
            }
            if reference == Some(node) {
                reference = document.node(node).ok().and_then(|n| n.next_sibling());
            }
            let node_is_fragment = node.document() == target.document()
                && matches!(
                    document.node(node).map(|n| n.kind()),
                    Ok(NodeKind::DocumentFragment)
                );
            let added = if node_is_fragment {
                children(document, node).len() as u32
            } else {
                1
            };
            if node.document() == target.document() && parent(document, node).is_some() {
                document
                    .detach(node)
                    .map_err(|e| dom_exception::from_dom_error(agent, e))?;
            }
            let offset = match reference {
                None => length(document, target),
                Some(r) => document.index_of(r).unwrap_or(0) as u32,
            } + added;
            super::insert_node(agent, document, &mut wrappers, node, target, reference)?;
            Ok(BoundaryPoint {
                node: target,
                offset,
            })
        })
    })?;
    if collapsed {
        let (start, _) = points(agent, this)?;
        store(agent, this, start, new_end)?;
    } else {
        let _ = end;
    }
    Ok(())
}

pub(super) fn insert_node(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let node = required_arg(agent, args, 0)?;
    insert_into(agent, this, node)?;
    Ok(Value::UNDEFINED)
}

pub(super) fn surround_contents(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let new_parent_value = required_arg(agent, args, 0)?;
    let new_parent = node_of(agent, new_parent_value)?;
    let (start, end) = points(agent, this)?;
    let partial = super::traversal::read_doc(agent, start.node, |document| {
        let root = super::ranges::root_of(document, start.node);
        document.walk(root).is_ok_and(|mut walk| {
            walk.any(|(id, _)| {
                partially_contained(document, id, start, end)
                    && !matches!(
                        document.node(id).map(|n| n.kind()),
                        Ok(NodeKind::Text(_) | NodeKind::CdataSection(_))
                    )
            })
        })
    })?;
    if partial {
        return Err(exception(
            agent,
            "InvalidStateError",
            "the range partially contains a non-Text node",
        ));
    }
    let bad = super::traversal::read_doc(agent, new_parent, |document| {
        matches!(
            document.node(new_parent).map(|n| n.kind()),
            Ok(NodeKind::Document | NodeKind::DocumentType(_) | NodeKind::DocumentFragment)
        )
    })?;
    if bad {
        return Err(exception(
            agent,
            "InvalidNodeTypeError",
            "a document, doctype or fragment cannot surround a range",
        ));
    }
    let fragment = extract(agent, this, false)?;
    with_page(agent, |agent, page| {
        page.write(agent, new_parent.document(), |agent, document| {
            for child in children(document, new_parent) {
                document
                    .detach(child)
                    .map_err(|e| dom_exception::from_dom_error(agent, e))?;
            }
            Ok(())
        })
    })?;
    insert_into(agent, this, new_parent_value)?;
    let fragment_id = node_of(agent, fragment)?;
    with_page(agent, |agent, page| {
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return agent.type_error("DOM bindings are busy");
        };
        page.write(agent, new_parent.document(), |agent, document| {
            super::insert_node(
                agent,
                document,
                &mut wrappers,
                fragment_id,
                new_parent,
                None,
            )
        })
    })?;
    // Select newParent.
    let selection = super::traversal::read_doc(agent, new_parent, |document| {
        parent(document, new_parent).map(|p| (p, document.index_of(new_parent).unwrap_or(0) as u32))
    })?;
    if let Some((p, index)) = selection {
        store(
            agent,
            this,
            BoundaryPoint {
                node: p,
                offset: index,
            },
            BoundaryPoint {
                node: p,
                offset: index + 1,
            },
        )?;
    }
    Ok(Value::UNDEFINED)
}
