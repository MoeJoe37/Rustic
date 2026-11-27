//! DOM §5 ranges: `AbstractRange`, `StaticRange` and live `Range`.
//!
//! A `Range`'s boundary points live in its document
//! ([`rustic_dom::Document::create_live_range`]), which applies the DOM's
//! mutation steps to them; the script object keeps its document's wrapper
//! and the range id. Objects that die are dropped from their documents at
//! the next collection or registration (see [`track`]).

use rustic_dom::{BoundaryPoint, Document, DocumentId, LiveId, NodeId, NodeKind};
use rustic_js::runtime::{Agent, JsResult, ObjectId, PropertyDescriptor, PropertyKey, Value};

use super::events::{PrivateArray, define_method, with_root};
use super::traversal::{node_of, read_doc, wrap_node};
use super::{creation, illegal_constructor, install_interface, prototype, with_page};

const ABSTRACT_RANGE: &str = "AbstractRange";
const STATIC_RANGE: &str = "StaticRange";
const RANGE: &str = "Range";

/// `StaticRange` slots: start node wrapper, start offset, end node wrapper,
/// end offset. `Range` slots: the document's wrapper and the range id.
const S_DOCUMENT: u32 = 0;
const S_ID: u32 = 1;

/// Script objects whose document entries die with them.
#[derive(Debug, Default)]
pub(super) struct Tracked {
    entries: std::cell::RefCell<Vec<(ObjectId, DocumentId, LiveId)>>,
}

impl Tracked {
    /// Drop the entries of collected objects from `documents`.
    pub(super) fn prune(&self, agent: &Agent, documents: &mut [&mut Document]) {
        let Ok(mut entries) = self.entries.try_borrow_mut() else {
            return;
        };
        entries.retain(|&(weak, document, id)| {
            if agent.weak_ref_deref(weak) != Value::UNDEFINED {
                return true;
            }
            // Adoption may have moved the entry to another document.
            let _ = document;
            for document in documents.iter_mut() {
                document.drop_live(id);
            }
            false
        });
    }
}

/// Remember `object` as the owner of `id` in `document`.
pub(super) fn track(
    agent: &mut Agent,
    object: Value,
    document: DocumentId,
    id: LiveId,
) -> JsResult<()> {
    let weak = agent.new_weak_ref(object)?;
    agent.root(Value::object(weak))?;
    with_page(agent, |_, page| {
        let Ok(mut entries) = page.tracked.entries.try_borrow_mut() else {
            return Ok(());
        };
        entries.push((weak, document, id));
        Ok(())
    })
}

fn retrack(
    agent: &mut Agent,
    old: (DocumentId, LiveId),
    new: (DocumentId, LiveId),
) -> JsResult<()> {
    with_page(agent, |_, page| {
        if let Ok(mut entries) = page.tracked.entries.try_borrow_mut() {
            for entry in entries.iter_mut() {
                if (entry.1, entry.2) == old {
                    (entry.1, entry.2) = new;
                }
            }
        }
        Ok(())
    })
}

fn exception(agent: &mut Agent, name: &str, message: &str) -> rustic_js::runtime::Abrupt {
    creation::exception(agent, name, message)
}

fn dom_error(agent: &mut Agent, error: rustic_dom::DomError) -> rustic_js::runtime::Abrupt {
    super::dom_exception::from_dom_error(agent, error)
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        ABSTRACT_RANGE,
        None,
        illegal_constructor,
        0,
        &[
            ("startContainer", "get startContainer", start_container),
            ("startOffset", "get startOffset", start_offset),
            ("endContainer", "get endContainer", end_container),
            ("endOffset", "get endOffset", end_offset),
            ("collapsed", "get collapsed", collapsed),
        ],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        STATIC_RANGE,
        Some(ABSTRACT_RANGE),
        construct_static,
        1,
        &[],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        RANGE,
        Some(ABSTRACT_RANGE),
        construct_range,
        0,
        &[(
            "commonAncestorContainer",
            "get commonAncestorContainer",
            common_ancestor,
        )],
        |agent, proto| {
            for (name, method, length) in [
                ("setStart", set_start as rustic_js::runtime::NativeFn, 2),
                ("setEnd", set_end, 2),
                ("setStartBefore", set_start_before, 1),
                ("setStartAfter", set_start_after, 1),
                ("setEndBefore", set_end_before, 1),
                ("setEndAfter", set_end_after, 1),
                ("collapse", collapse, 0),
                ("selectNode", select_node, 1),
                ("selectNodeContents", select_node_contents, 1),
                ("compareBoundaryPoints", compare_boundary_points, 2),
                ("cloneRange", clone_range, 0),
                ("detach", detach, 0),
                ("isPointInRange", is_point_in_range, 2),
                ("comparePoint", compare_point, 2),
                ("intersectsNode", intersects_node, 1),
                ("toString", to_string, 0),
                ("deleteContents", super::range_content::delete_contents, 0),
                ("extractContents", super::range_content::extract_contents, 0),
                ("cloneContents", super::range_content::clone_contents, 0),
                ("insertNode", super::range_content::insert_node, 1),
                (
                    "surroundContents",
                    super::range_content::surround_contents,
                    1,
                ),
            ] {
                define_method(agent, proto, name, method, length)?;
            }
            Ok(())
        },
    )?;
    // The comparison constants on the interface object and prototype.
    let proto = prototype(agent, RANGE)?;
    let key = PropertyKey::String(agent.names.constructor);
    let constructor = agent.get(proto, key)?.as_object();
    for (index, name) in [
        "START_TO_START",
        "START_TO_END",
        "END_TO_END",
        "END_TO_START",
    ]
    .into_iter()
    .enumerate()
    {
        let key = agent.heap.key(name)?;
        let value = PropertyDescriptor::data_with(Value::number(index as f64), false, true, false);
        agent.define_property_or_throw(proto, key, value)?;
        if let Some(constructor) = constructor {
            agent.define_property_or_throw(constructor, key, value)?;
        }
    }
    let document = prototype(agent, "Document")?;
    define_method(agent, document, "createRange", create_range, 0)
}

/// How a range is held: a live document entry or static slots.
enum Held {
    Live(PrivateArray, NodeId, LiveId),
    Static(PrivateArray),
}

fn held(agent: &mut Agent, this: Value) -> JsResult<Held> {
    let slots = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.ranges, this))
    })?;
    let Some(slots) = PrivateArray::from_value(slots) else {
        return agent.type_error("expected a range receiver");
    };
    if slots.len(agent)? == 2 {
        let document = slots.get(agent, S_DOCUMENT)?;
        let document = node_of(agent, document)?;
        let raw = slots.get(agent, S_ID)?.as_number().unwrap_or(0.0) as u64;
        Ok(Held::Live(slots, document, LiveId::from_raw(raw)))
    } else {
        Ok(Held::Static(slots))
    }
}

fn live(agent: &mut Agent, this: Value) -> JsResult<(PrivateArray, NodeId, LiveId)> {
    let (slots, document, id) = match held(agent, this)? {
        Held::Live(slots, document, id) => (slots, document, id),
        Held::Static(_) => return agent.type_error("expected a Range receiver"),
    };
    if read_doc(agent, document, |d| d.live_range(id).is_some())? {
        return Ok((slots, document, id));
    }
    // Adoption moved the range's nodes, and the range, to another document.
    let Some(found) = locate(agent, id)? else {
        return agent.type_error("the range's document is gone");
    };
    let wrapper = wrap_node(agent, found)?;
    slots.set(agent, S_DOCUMENT, wrapper)?;
    Ok((slots, found, id))
}

/// The root of the document holding live entry `id`.
fn locate(agent: &mut Agent, id: LiveId) -> JsResult<Option<NodeId>> {
    with_page(agent, |agent, page| {
        let mut documents = vec![page.main];
        if let Ok(additional) = page.additional.try_borrow() {
            documents.extend(additional.keys().copied());
        }
        for document in documents {
            let root = page.read(agent, document, |_, d| {
                Ok((d.live_range(id).is_some() || d.node_iterator(id).is_some()).then(|| d.root()))
            })?;
            if root.is_some() {
                return Ok(root);
            }
        }
        Ok(None)
    })
}

/// A live range's boundary points.
pub(super) fn points(agent: &mut Agent, this: Value) -> JsResult<(BoundaryPoint, BoundaryPoint)> {
    let (_, document, id) = live(agent, this)?;
    match read_doc(agent, document, |d| d.live_range(id))? {
        Some(points) => Ok(points),
        None => agent.type_error("the range's document is gone"),
    }
}

fn abstract_points(agent: &mut Agent, this: Value) -> JsResult<(Value, u32, Value, u32)> {
    match held(agent, this)? {
        Held::Static(slots) => {
            let start = slots.get(agent, 0)?;
            let start_offset = slots.get(agent, 1)?.as_number().unwrap_or(0.0) as u32;
            let end = slots.get(agent, 2)?;
            let end_offset = slots.get(agent, 3)?.as_number().unwrap_or(0.0) as u32;
            Ok((start, start_offset, end, end_offset))
        }
        Held::Live(..) => {
            let (start, end) = points(agent, this)?;
            let start_node = wrap_node(agent, start.node)?;
            let end_node = wrap_node(agent, end.node)?;
            Ok((start_node, start.offset, end_node, end.offset))
        }
    }
}

fn start_container(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(abstract_points(agent, this)?.0)
}

fn start_offset(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::number(f64::from(abstract_points(agent, this)?.1)))
}

fn end_container(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(abstract_points(agent, this)?.2)
}

fn end_offset(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::number(f64::from(abstract_points(agent, this)?.3)))
}

fn collapsed(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (start, start_offset, end, end_offset) = abstract_points(agent, this)?;
    Ok(Value::boolean(start == end && start_offset == end_offset))
}

/// The root of `node`'s tree (its topmost inclusive ancestor).
pub(super) fn root_of(document: &Document, node: NodeId) -> NodeId {
    let mut at = node;
    while let Some(parent) = document.node(at).ok().and_then(|n| n.parent()) {
        at = parent;
    }
    at
}

/// Inclusive ancestors of `node`, from `node` up.
fn ancestors(document: &Document, node: NodeId) -> Vec<NodeId> {
    let mut out = vec![node];
    let mut at = node;
    while let Some(parent) = document.node(at).ok().and_then(|n| n.parent()) {
        out.push(parent);
        at = parent;
    }
    out
}

/// DOM §5.5 "contained": (`node`, 0) is after `start` and (`node`,
/// length) is before `end` (both in `node`'s tree).
pub(super) fn contained(
    document: &Document,
    node: NodeId,
    start: BoundaryPoint,
    end: BoundaryPoint,
) -> bool {
    let length = document.node_length(node).unwrap_or(0) as u32;
    position(document, BoundaryPoint { node, offset: 0 }, start) == std::cmp::Ordering::Greater
        && position(
            document,
            BoundaryPoint {
                node,
                offset: length,
            },
            end,
        ) == std::cmp::Ordering::Less
}

/// Tree order of two nodes in one tree.
pub(super) fn tree_order(document: &Document, a: NodeId, b: NodeId) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if a == b {
        return Ordering::Equal;
    }
    let (mut pa, mut pb) = (ancestors(document, a), ancestors(document, b));
    pa.reverse();
    pb.reverse();
    let mut i = 0;
    while i < pa.len() && i < pb.len() && pa[i] == pb[i] {
        i += 1;
    }
    if i == pa.len() {
        return Ordering::Less; // a is an ancestor of b
    }
    if i == pb.len() {
        return Ordering::Greater;
    }
    let index = |n: NodeId| document.index_of(n).unwrap_or(0);
    index(pa[i]).cmp(&index(pb[i]))
}

/// DOM §5.1: the position of boundary point `a` relative to `b`.
pub(super) fn position(
    document: &Document,
    a: BoundaryPoint,
    b: BoundaryPoint,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if a.node == b.node {
        return a.offset.cmp(&b.offset);
    }
    if tree_order(document, a.node, b.node) == Ordering::Greater {
        return position(document, b, a).reverse();
    }
    if document.is_inclusive_descendant(b.node, a.node) {
        let mut child = b.node;
        while let Some(parent) = document.node(child).ok().and_then(|n| n.parent()) {
            if parent == a.node {
                break;
            }
            child = parent;
        }
        if (document.index_of(child).unwrap_or(0) as u32) < a.offset {
            return Ordering::Greater;
        }
    }
    Ordering::Less
}

fn offset_arg(agent: &mut Agent, args: &[Value], at: usize) -> JsResult<u32> {
    let value = super::required_arg(agent, args, at)?;
    let number = agent.to_number(value)?;
    Ok(if number.is_finite() {
        number.trunc().rem_euclid(4_294_967_296.0) as u32
    } else {
        0
    })
}

/// Create a live range in `node`'s document at (`node`, 0) twice.
fn new_live(
    agent: &mut Agent,
    proto: ObjectId,
    start: BoundaryPoint,
    end: BoundaryPoint,
) -> JsResult<Value> {
    let object = Value::object(agent.new_object_with(Some(proto))?);
    with_root(agent, object, |agent| {
        let document_root = read_doc(agent, start.node, |d| d.root())?;
        let document_wrapper = wrap_node(agent, document_root)?;
        let id = with_page(agent, |agent, page| {
            page.write(agent, start.node.document(), |agent, document| {
                document
                    .create_live_range(start, end)
                    .map_err(|error| dom_error(agent, error))
            })
        })?;
        let slots = PrivateArray::new(agent, &[document_wrapper, Value::number(id.get() as f64)])?;
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.ranges, object, slots.value())
        })?;
        track(agent, object, start.node.document(), id)?;
        Ok(object)
    })
}

/// `new Range()`: collapsed at the start of the current document.
fn construct_range(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("Range construction requires new");
    };
    let proto = agent.host_prototype_from_new_target(RANGE, target)?;
    let root = with_page(agent, |agent, page| {
        page.read(agent, page.main, |_, document| Ok(document.root()))
    })?;
    let point = BoundaryPoint {
        node: root,
        offset: 0,
    };
    new_live(agent, proto, point, point)
}

/// `document.createRange()`.
fn create_range(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let document = node_of(agent, this)?;
    let proto = prototype(agent, RANGE)?;
    let point = BoundaryPoint {
        node: document,
        offset: 0,
    };
    new_live(agent, proto, point, point)
}

/// `new StaticRange(init)`.
fn construct_static(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("StaticRange construction requires new");
    };
    let init = super::required_arg(agent, args, 0)?;
    let Some(init) = init.as_object() else {
        return agent.type_error("StaticRangeInit must be an object");
    };
    let mut values = Vec::new();
    for (name, is_node) in [
        ("endContainer", true),
        ("endOffset", false),
        ("startContainer", true),
        ("startOffset", false),
    ] {
        let key = agent.heap.key(name)?;
        let value = agent.get(init, key)?;
        if value == Value::UNDEFINED {
            return agent.type_error("StaticRangeInit member is missing");
        }
        values.push(if is_node {
            let id = node_of(agent, value)?;
            let doctype = read_doc(agent, id, |d| {
                matches!(d.node(id).map(|n| n.kind()), Ok(NodeKind::DocumentType(_)))
            })?;
            if doctype {
                return Err(exception(
                    agent,
                    "InvalidNodeTypeError",
                    "a doctype cannot hold a boundary point",
                ));
            }
            value
        } else {
            let number = agent.to_number(value)?;
            Value::number(if number.is_finite() {
                number.trunc().rem_euclid(4_294_967_296.0)
            } else {
                0.0
            })
        });
    }
    let proto = agent.host_prototype_from_new_target(STATIC_RANGE, target)?;
    let object = Value::object(agent.new_object_with(Some(proto))?);
    let slots = PrivateArray::new(agent, &[values[2], values[3], values[0], values[1]])?;
    with_page(agent, |agent, page| {
        agent.weak_map_set(page.ranges, object, slots.value())
    })?;
    Ok(object)
}

/// Move a live range to new boundary points, re-registering it when they
/// are in another document.
pub(super) fn store(
    agent: &mut Agent,
    this: Value,
    start: BoundaryPoint,
    end: BoundaryPoint,
) -> JsResult<()> {
    let (slots, document, id) = live(agent, this)?;
    if start.node.document() == document.document() {
        return with_page(agent, |agent, page| {
            page.write(agent, document.document(), |agent, d| {
                d.set_live_range(id, start, end)
                    .map_err(|error| dom_error(agent, error))
            })
        });
    }
    let new_id = with_page(agent, |agent, page| {
        page.write(agent, start.node.document(), |agent, d| {
            d.create_live_range(start, end)
                .map_err(|error| dom_error(agent, error))
        })
    })?;
    with_page(agent, |agent, page| {
        page.write(agent, document.document(), |_, d| {
            d.drop_live(id);
            Ok(())
        })
    })?;
    let root = read_doc(agent, start.node, |d| d.root())?;
    let wrapper = wrap_node(agent, root)?;
    slots.set(agent, S_DOCUMENT, wrapper)?;
    slots.set(agent, S_ID, Value::number(new_id.get() as f64))?;
    retrack(
        agent,
        (document.document(), id),
        (start.node.document(), new_id),
    )
}

/// DOM §5.5 "set the start or end".
fn set_boundary(
    agent: &mut Agent,
    this: Value,
    node: NodeId,
    offset: u32,
    start: bool,
) -> JsResult<()> {
    let (old_start, old_end) = points(agent, this)?;
    let check = read_doc(agent, node, |d| {
        let kind = d
            .node(node)
            .map(|n| matches!(n.kind(), NodeKind::DocumentType(_)));
        (kind, d.node_length(node))
    })?;
    match check {
        (Ok(true), _) => {
            return Err(exception(
                agent,
                "InvalidNodeTypeError",
                "a doctype cannot hold a boundary point",
            ));
        }
        (_, Ok(length)) if offset as usize > length => {
            return Err(exception(
                agent,
                "IndexSizeError",
                "offset is past the node's length",
            ));
        }
        _ => {}
    }
    let point = BoundaryPoint { node, offset };
    let same_root = |a: NodeId, b: NodeId, agent: &mut Agent| -> JsResult<bool> {
        if a.document() != b.document() {
            return Ok(false);
        }
        read_doc(agent, a, |d| root_of(d, a) == root_of(d, b))
    };
    let (new_start, new_end) = if start {
        let other = old_end;
        let after = same_root(node, other.node, agent)?
            && read_doc(agent, node, |d| position(d, point, other))? == std::cmp::Ordering::Greater;
        if !same_root(node, other.node, agent)? || after {
            (point, point)
        } else {
            (point, other)
        }
    } else {
        let other = old_start;
        let before = same_root(node, other.node, agent)?
            && read_doc(agent, node, |d| position(d, point, other))? == std::cmp::Ordering::Less;
        if !same_root(node, other.node, agent)? || before {
            (point, point)
        } else {
            (other, point)
        }
    };
    store(agent, this, new_start, new_end)
}

fn node_arg(agent: &mut Agent, args: &[Value], at: usize) -> JsResult<NodeId> {
    let value = super::required_arg(agent, args, at)?;
    node_of(agent, value)
}

fn set_start(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let node = node_arg(agent, args, 0)?;
    let offset = offset_arg(agent, args, 1)?;
    set_boundary(agent, this, node, offset, true)?;
    Ok(Value::UNDEFINED)
}

fn set_end(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let node = node_arg(agent, args, 0)?;
    let offset = offset_arg(agent, args, 1)?;
    set_boundary(agent, this, node, offset, false)?;
    Ok(Value::UNDEFINED)
}

/// The parent and index of `node`, or `InvalidNodeTypeError` without a parent.
fn parent_and_index(agent: &mut Agent, node: NodeId) -> JsResult<(NodeId, u32)> {
    let found = read_doc(agent, node, |d| {
        d.node(node)
            .ok()
            .and_then(|n| n.parent())
            .map(|parent| (parent, d.index_of(node).unwrap_or(0) as u32))
    })?;
    match found {
        Some(found) => Ok(found),
        None => Err(exception(
            agent,
            "InvalidNodeTypeError",
            "the node has no parent",
        )),
    }
}

fn set_relative(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
    start: bool,
    after: bool,
) -> JsResult<Value> {
    let node = node_arg(agent, args, 0)?;
    let (parent, index) = parent_and_index(agent, node)?;
    set_boundary(agent, this, parent, index + u32::from(after), start)?;
    Ok(Value::UNDEFINED)
}

fn set_start_before(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    set_relative(agent, this, args, true, false)
}

fn set_start_after(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    set_relative(agent, this, args, true, true)
}

fn set_end_before(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    set_relative(agent, this, args, false, false)
}

fn set_end_after(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    set_relative(agent, this, args, false, true)
}

fn collapse(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let to_start = match args.first() {
        Some(&value) => agent.to_boolean(value),
        None => false,
    };
    let (start, end) = points(agent, this)?;
    let point = if to_start { start } else { end };
    store(agent, this, point, point)?;
    Ok(Value::UNDEFINED)
}

fn select_node(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let node = node_arg(agent, args, 0)?;
    let (parent, index) = parent_and_index(agent, node)?;
    store(
        agent,
        this,
        BoundaryPoint {
            node: parent,
            offset: index,
        },
        BoundaryPoint {
            node: parent,
            offset: index + 1,
        },
    )?;
    Ok(Value::UNDEFINED)
}

fn select_node_contents(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let node = node_arg(agent, args, 0)?;
    let (doctype, length) = read_doc(agent, node, |d| {
        (
            matches!(
                d.node(node).map(|n| n.kind()),
                Ok(NodeKind::DocumentType(_))
            ),
            d.node_length(node).unwrap_or(0) as u32,
        )
    })?;
    if doctype {
        return Err(exception(
            agent,
            "InvalidNodeTypeError",
            "a doctype has no contents",
        ));
    }
    store(
        agent,
        this,
        BoundaryPoint { node, offset: 0 },
        BoundaryPoint {
            node,
            offset: length,
        },
    )?;
    Ok(Value::UNDEFINED)
}

fn compare_boundary_points(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let how = super::required_arg(agent, args, 0)?;
    let how = agent.to_number(how)?;
    let how = if how.is_finite() {
        how.trunc().rem_euclid(65_536.0) as u16
    } else {
        0
    };
    let source = super::required_arg(agent, args, 1)?;
    let (start, end) = points(agent, this)?;
    let (source_start, source_end) = points(agent, source)?;
    if how > 3 {
        return Err(exception(agent, "NotSupportedError", "unknown comparison"));
    }
    let same_root = start.node.document() == source_start.node.document()
        && read_doc(agent, start.node, |d| {
            root_of(d, start.node) == root_of(d, source_start.node)
        })?;
    if !same_root {
        return Err(exception(
            agent,
            "WrongDocumentError",
            "the ranges are in different trees",
        ));
    }
    let (this_point, other_point) = match how {
        0 => (start, source_start),
        1 => (end, source_start),
        2 => (end, source_end),
        _ => (start, source_end),
    };
    let order = read_doc(agent, start.node, |d| position(d, this_point, other_point))?;
    Ok(Value::number(match order {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }))
}

fn clone_range(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (start, end) = points(agent, this)?;
    let proto = prototype(agent, RANGE)?;
    new_live(agent, proto, start, end)
}

fn detach(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    live(agent, this)?;
    Ok(Value::UNDEFINED)
}

/// Shared checks of `isPointInRange` and `comparePoint`: `None` when the
/// node is in another tree.
fn point_order(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Option<i8>> {
    let node = node_arg(agent, args, 0)?;
    let offset = offset_arg(agent, args, 1)?;
    let (start, end) = points(agent, this)?;
    if node.document() != start.node.document()
        || read_doc(agent, node, |d| root_of(d, node) != root_of(d, start.node))?
    {
        return Ok(None);
    }
    let (doctype, length) = read_doc(agent, node, |d| {
        (
            matches!(
                d.node(node).map(|n| n.kind()),
                Ok(NodeKind::DocumentType(_))
            ),
            d.node_length(node).unwrap_or(0),
        )
    })?;
    if doctype {
        return Err(exception(
            agent,
            "InvalidNodeTypeError",
            "a doctype has no points",
        ));
    }
    if offset as usize > length {
        return Err(exception(
            agent,
            "IndexSizeError",
            "offset is past the node's length",
        ));
    }
    let point = BoundaryPoint { node, offset };
    let (before, after) = read_doc(agent, node, |d| {
        (
            position(d, point, start) == std::cmp::Ordering::Less,
            position(d, point, end) == std::cmp::Ordering::Greater,
        )
    })?;
    Ok(Some(if before {
        -1
    } else if after {
        1
    } else {
        0
    }))
}

fn is_point_in_range(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(point_order(agent, this, args)? == Some(0)))
}

fn compare_point(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    match point_order(agent, this, args)? {
        Some(order) => Ok(Value::number(f64::from(order))),
        None => Err(exception(
            agent,
            "WrongDocumentError",
            "the node is in another tree",
        )),
    }
}

fn intersects_node(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let node = node_arg(agent, args, 0)?;
    let (start, end) = points(agent, this)?;
    if node.document() != start.node.document()
        || read_doc(agent, node, |d| root_of(d, node) != root_of(d, start.node))?
    {
        return Ok(Value::boolean(false));
    }
    let Some((parent, index)) = read_doc(agent, node, |d| {
        d.node(node)
            .ok()
            .and_then(|n| n.parent())
            .map(|p| (p, d.index_of(node).unwrap_or(0) as u32))
    })?
    else {
        return Ok(Value::boolean(true));
    };
    let result = read_doc(agent, node, |d| {
        position(
            d,
            BoundaryPoint {
                node: parent,
                offset: index,
            },
            end,
        ) == std::cmp::Ordering::Less
            && position(
                d,
                BoundaryPoint {
                    node: parent,
                    offset: index + 1,
                },
                start,
            ) == std::cmp::Ordering::Greater
    })?;
    Ok(Value::boolean(result))
}

/// The concatenated data of the range's Text nodes (DOM §5.5 stringifier).
fn to_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (start, end) = points(agent, this)?;
    let units = read_doc(agent, start.node, |d| {
        let text = |id: NodeId| match d.node(id).map(|n| n.kind()) {
            Ok(NodeKind::Text(value) | NodeKind::CdataSection(value)) => Some(value.as_units()),
            _ => None,
        };
        let mut out: Vec<u16> = Vec::new();
        if start.node == end.node {
            if let Some(units) = text(start.node) {
                out.extend_from_slice(&units[start.offset as usize..end.offset as usize]);
                return out;
            }
        }
        if let Some(units) = text(start.node) {
            out.extend_from_slice(&units[start.offset as usize..]);
        }
        // Text nodes contained in the range, in tree order.
        let root = root_of(d, start.node);
        if let Ok(walk) = d.walk(root) {
            for (id, _) in walk {
                if let Some(units) = text(id) {
                    if contained(d, id, start, end) {
                        out.extend_from_slice(units);
                    }
                }
            }
        }
        if let Some(units) = text(end.node) {
            out.extend_from_slice(&units[..end.offset as usize]);
        }
        out
    })?;
    let string = agent
        .heap
        .alloc_string(rustic_js::runtime::JsStr::from_units(&units))?;
    Ok(Value::string(string))
}

/// The nearest common inclusive ancestor of the start and end nodes.
fn common_ancestor(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (start, end) = points(agent, this)?;
    let common = read_doc(agent, start.node, |d| {
        let mut at = start.node;
        while !d.is_inclusive_descendant(end.node, at) {
            match d.node(at).ok().and_then(|n| n.parent()) {
                Some(parent) => at = parent,
                None => break,
            }
        }
        at
    })?;
    wrap_node(agent, common)
}
