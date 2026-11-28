//! `MutationObserver` and `MutationRecord` ([DOM §4.3]).
//!
//! The document journal ([`rustic_dom::MutationEntry`]) supplies the facts of
//! each change. After every binding operation, and before each microtask
//! checkpoint, [`flush`] turns new entries into records for the interested
//! observers and queues one notification microtask.
//!
//! A registration is a private array held by its node as a heap edge, so it
//! lives exactly as long as the node, follows the node through adoption and
//! keeps its observer alive. The observer keeps weak references to its
//! registrations, so a registration dies with its node and the observer's
//! list is compacted as it grows. `disconnect` marks its registrations dead
//! and removes each one from the node it was attached to; after an adoption
//! moved that node, the node drops the dead registration the next time a
//! mutation looks at it (TD-150, paid).
//!
//! [DOM §4.3]: https://dom.spec.whatwg.org/#mutation-observers

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use rustic_dom::{DomString, MutationEntry, NodeId};
use rustic_js::runtime::{Abrupt, Agent, JsResult, NativeFn, ObjectId, OutOfMemory, Value};

use crate::heap::WrapperKind;
use crate::webidl::{DomStringConversion, from_dom_string, to_dom_string};
use crate::{DomGcError, DomHeap, dom_exception};

use super::collections::NODE_LIST;
use super::events::{PrivateArray, define_method, with_root};
use super::{Page, identity, illegal_constructor, install_interface, prototype, with_page, wrap};

const OBSERVER: &str = "MutationObserver";
const RECORD: &str = "MutationRecord";

const CHILD_LIST: u32 = 1;
const ATTRIBUTES: u32 = 2;
const CHARACTER_DATA: u32 = 4;
const SUBTREE: u32 = 8;
const ATTRIBUTE_OLD_VALUE: u32 = 16;
const CHARACTER_DATA_OLD_VALUE: u32 = 32;

/// Registration slots: `[observer or undefined when dead, option bits,
/// attribute filter array or undefined, source registration or undefined]`.
const REG_OBSERVER: u32 = 0;
const REG_BITS: u32 = 1;
const REG_FILTER: u32 = 2;
const REG_SOURCE: u32 = 3;

/// Observer slots: `[callback, record queue, weak references to the
/// registrations, creation serial, live registrations at the last
/// compaction]`.
const STATE_CALLBACK: u32 = 0;
const STATE_RECORDS: u32 = 1;
const STATE_REGISTRATIONS: u32 = 2;
const STATE_SERIAL: u32 = 3;
const STATE_COMPACTED: u32 = 4;

/// The weak registration list is compacted when it holds more than twice
/// the registrations that were live at the last compaction, and this many.
const COMPACT_MIN: u32 = 64;

/// Storage slots, rooted for the page's lifetime.
const STORE_PENDING: u32 = 0;
const STORE_SLOTS: u32 = 6;

/// The members of a `MutationRecord`, in getter order.
const RECORD_MEMBERS: [&str; 9] = [
    "type",
    "target",
    "addedNodes",
    "removedNodes",
    "previousSibling",
    "nextSibling",
    "attributeName",
    "attributeNamespace",
    "oldValue",
];

/// Page-lifetime observer state.
#[derive(Debug)]
pub(super) struct Observers {
    states: ObjectId,
    registrations: ObjectId,
    records: ObjectId,
    storage: Value,
    notify: Value,
    invoke: Value,
    active: Cell<bool>,
    queued: Cell<bool>,
    flushing: Cell<bool>,
    next_serial: Cell<u32>,
    /// The node each live registration was attached to, for removing its
    /// edge on `disconnect`. Entries whose node no longer holds the
    /// registration are pruned as the map doubles.
    nodes: RefCell<HashMap<Value, NodeId>>,
    nodes_pruned_at: Cell<usize>,
}

impl Observers {
    pub(super) fn new(agent: &mut Agent) -> JsResult<Self> {
        let states = agent.new_weak_map()?;
        let registrations = agent.new_weak_map()?;
        let records = agent.new_weak_map()?;
        let pending = PrivateArray::new(agent, &[])?;
        let slots = PrivateArray::new(agent, &[])?;
        let notify = Value::object(agent.new_native_function(notify, "notify", 0)?);
        let invoke = Value::object(agent.new_native_function(invoke, "invoke", 1)?);
        let storage = PrivateArray::new(
            agent,
            &[
                pending.value(),
                Value::object(states),
                Value::object(registrations),
                Value::object(records),
                notify,
                invoke,
                slots.value(),
            ],
        )?;
        agent.root(storage.value())?;
        Ok(Self {
            states,
            registrations,
            records,
            storage: storage.value(),
            notify,
            invoke,
            active: Cell::new(false),
            queued: Cell::new(false),
            flushing: Cell::new(false),
            next_serial: Cell::new(0),
            nodes: RefCell::new(HashMap::new()),
            nodes_pruned_at: Cell::new(0),
        })
    }

    pub(super) fn is_active(&self) -> bool {
        self.active.get()
    }

    fn storage(&self) -> PrivateArray {
        PrivateArray::from_value(self.storage).expect("rooted observer storage")
    }
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(agent, OBSERVER, None, construct, 1, &[], |agent, proto| {
        for (name, behavior, length) in [
            ("observe", observe as NativeFn, 1),
            ("disconnect", disconnect, 0),
            ("takeRecords", take_records, 0),
        ] {
            define_method(agent, proto, name, behavior, length)?;
        }
        Ok(())
    })?;
    let getters: [NativeFn; 9] = [
        record_member::<0>,
        record_member::<1>,
        record_member::<2>,
        record_member::<3>,
        record_member::<4>,
        record_member::<5>,
        record_member::<6>,
        record_member::<7>,
        record_member::<8>,
    ];
    let names: [&str; 9] = [
        "get type",
        "get target",
        "get addedNodes",
        "get removedNodes",
        "get previousSibling",
        "get nextSibling",
        "get attributeName",
        "get attributeNamespace",
        "get oldValue",
    ];
    let mut attributes = Vec::new();
    attributes
        .try_reserve_exact(RECORD_MEMBERS.len())
        .map_err(|_| OutOfMemory)?;
    for ((member, name), getter) in RECORD_MEMBERS.into_iter().zip(names).zip(getters) {
        attributes.push((member, name, getter));
    }
    install_interface(
        agent,
        RECORD,
        None,
        illegal_constructor,
        0,
        &attributes,
        |_, _| Ok(()),
    )
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("MutationObserver requires new");
    };
    let callback = args.first().copied().unwrap_or(Value::UNDEFINED);
    if !agent.is_callable(callback) {
        return agent.type_error("MutationObserver callback is not callable");
    }
    let proto = agent.host_prototype_from_new_target(OBSERVER, new_target)?;
    let observer = Value::object(agent.new_object_with(Some(proto))?);
    with_page(agent, |agent, page| {
        let records = PrivateArray::new(agent, &[])?;
        let registrations = PrivateArray::new(agent, &[])?;
        let serial = page.observers.next_serial.get();
        page.observers.next_serial.set(serial.saturating_add(1));
        let state = PrivateArray::new(
            agent,
            &[
                callback,
                records.value(),
                registrations.value(),
                Value::number(f64::from(serial)),
                Value::number(0.0),
            ],
        )?;
        agent.weak_map_set(page.observers.states, observer, state.value())
    })?;
    Ok(observer)
}

fn state(agent: &mut Agent, page: &Page, observer: Value) -> JsResult<PrivateArray> {
    let value = agent.weak_map_get(page.observers.states, observer);
    match PrivateArray::from_value(value) {
        Some(state) => Ok(state),
        None => agent.type_error("expected a MutationObserver receiver"),
    }
}

fn array_at(agent: &mut Agent, array: PrivateArray, index: u32) -> JsResult<PrivateArray> {
    let value = array.get(agent, index)?;
    Ok(PrivateArray::from_value(value).expect("private observer array"))
}

fn push(agent: &mut Agent, array: PrivateArray, value: Value) -> JsResult<()> {
    let len = array.len(agent)?;
    array.set(agent, len, value)
}

fn values(agent: &mut Agent, array: PrivateArray) -> JsResult<Vec<Value>> {
    let len = array.len(agent)?;
    let mut out = Vec::new();
    out.try_reserve_exact(len as usize)
        .map_err(|_| OutOfMemory)?;
    for index in 0..len {
        out.push(array.get(agent, index)?);
    }
    Ok(out)
}

/// The observer's registrations that are still alive, with the weak
/// references that hold them in its list.
fn owned(agent: &mut Agent, state: PrivateArray) -> JsResult<Vec<(Value, PrivateArray)>> {
    let list = array_at(agent, state, STATE_REGISTRATIONS)?;
    let mut out = Vec::new();
    for weak in values(agent, list)? {
        let target = weak
            .as_object()
            .map_or(Value::UNDEFINED, |id| agent.weak_ref_deref(id));
        if let Some(registration) = PrivateArray::from_value(target) {
            out.try_reserve(1).map_err(|_| OutOfMemory)?;
            out.push((weak, registration));
        }
    }
    Ok(out)
}

/// Replace the observer's list with `kept` weak references.
fn set_owned(agent: &mut Agent, state: PrivateArray, kept: &[Value]) -> JsResult<()> {
    let list = PrivateArray::new(agent, kept)?;
    state.set(agent, STATE_REGISTRATIONS, list.value())?;
    let live = u32::try_from(kept.len()).unwrap_or(u32::MAX);
    state.set(agent, STATE_COMPACTED, Value::number(f64::from(live)))
}

/// Add a registration to the observer's weak list, compacting the list
/// when collected registrations have made it grow (TD-150).
fn push_owned(agent: &mut Agent, state: PrivateArray, registration: PrivateArray) -> JsResult<()> {
    let weak = Value::object(agent.new_weak_ref(registration.value())?);
    let list = array_at(agent, state, STATE_REGISTRATIONS)?;
    push(agent, list, weak)?;
    let len = list.len(agent)?;
    let compacted = state
        .get(agent, STATE_COMPACTED)?
        .as_number()
        .unwrap_or(0.0) as u32;
    if len > COMPACT_MIN && len > compacted.saturating_mul(2) {
        let live: Vec<Value> = owned(agent, state)?.into_iter().map(|(w, _)| w).collect();
        set_owned(agent, state, &live)?;
    }
    Ok(())
}

/// Mark a registration dead and remove it from the node that holds it,
/// when that node is still where it was attached.
fn kill(
    agent: &mut Agent,
    page: &Page,
    wrappers: &mut DomHeap,
    registration: PrivateArray,
) -> JsResult<()> {
    registration.set(agent, REG_OBSERVER, Value::UNDEFINED)?;
    if let Some(node) = page
        .observers
        .nodes
        .borrow_mut()
        .remove(&registration.value())
    {
        wrappers.remove_edge(node, registration.value());
    }
    Ok(())
}

fn bits(agent: &mut Agent, registration: PrivateArray) -> JsResult<u32> {
    let bits = registration.get(agent, REG_BITS)?;
    // Registration bits are written only by this module, as small integers.
    Ok(bits.as_number().expect("registration bits") as u32)
}

/// A validated `MutationObserverInit`.
struct Options {
    bits: u32,
    filter: Option<Vec<DomString>>,
}

/// Convert the dictionary in Web IDL member order, then apply `observe()`
/// steps 1 to 6.
fn options(agent: &mut Agent, value: Value) -> JsResult<Options> {
    let mut filter = None;
    let mut present = [None::<bool>; 4];
    let (mut child_list, mut subtree) = (false, false);
    if !value.is_nullish() {
        let Some(object) = value.as_object() else {
            return agent.type_error("MutationObserverInit must be an object");
        };
        let key = agent.heap.key("attributeFilter")?;
        let list = agent.get(object, key)?;
        if list != Value::UNDEFINED {
            let mut names = Vec::new();
            agent.iterate_values(list, |agent, item| {
                let name = to_dom_string(agent, item, DomStringConversion::Default)?;
                names.try_reserve(1).map_err(|_| OutOfMemory)?;
                names.push(name);
                Ok(true)
            })?;
            filter = Some(names);
        }
        for (slot, name) in [
            "attributeOldValue",
            "attributes",
            "characterData",
            "characterDataOldValue",
        ]
        .into_iter()
        .enumerate()
        {
            let key = agent.heap.key(name)?;
            let member = agent.get(object, key)?;
            if member != Value::UNDEFINED {
                present[slot] = Some(agent.to_boolean(member));
            }
        }
        let key = agent.heap.key("childList")?;
        let member = agent.get(object, key)?;
        child_list = agent.to_boolean(member);
        let key = agent.heap.key("subtree")?;
        let member = agent.get(object, key)?;
        subtree = agent.to_boolean(member);
    }
    let [attribute_old, mut attributes, mut character_data, data_old] = present;
    if (attribute_old.is_some() || filter.is_some()) && attributes.is_none() {
        attributes = Some(true);
    }
    if data_old.is_some() && character_data.is_none() {
        character_data = Some(true);
    }
    let attributes = attributes == Some(true);
    let character_data = character_data == Some(true);
    if !child_list && !attributes && !character_data {
        return agent
            .type_error("observe() needs at least one of childList, attributes and characterData");
    }
    if attribute_old == Some(true) && !attributes {
        return agent.type_error("attributeOldValue requires attributes");
    }
    if filter.is_some() && !attributes {
        return agent.type_error("attributeFilter requires attributes");
    }
    if data_old == Some(true) && !character_data {
        return agent.type_error("characterDataOldValue requires characterData");
    }
    let flag = |on: bool, bit: u32| if on { bit } else { 0 };
    Ok(Options {
        bits: flag(child_list, CHILD_LIST)
            | flag(attributes, ATTRIBUTES)
            | flag(character_data, CHARACTER_DATA)
            | flag(subtree, SUBTREE)
            | flag(attribute_old == Some(true), ATTRIBUTE_OLD_VALUE)
            | flag(data_old == Some(true), CHARACTER_DATA_OLD_VALUE),
        filter,
    })
}

fn observe(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let target = args.first().copied().unwrap_or(Value::UNDEFINED);
    let node = with_page(agent, |agent, page| {
        state(agent, page, receiver)?;
        let Ok(wrappers) = page.wrappers.try_borrow() else {
            return agent.type_error("DOM bindings are busy");
        };
        identity(agent, &wrappers, target, WrapperKind::Node)
    })?;
    let options = options(agent, args.get(1).copied().unwrap_or(Value::UNDEFINED))?;
    let filter = match &options.filter {
        Some(names) => {
            let list = PrivateArray::new(agent, &[])?;
            with_root(agent, list.value(), |agent| {
                for name in names {
                    let value = from_dom_string(agent, name)?;
                    push(agent, list, value)?;
                }
                Ok(())
            })?;
            list.value()
        }
        None => Value::UNDEFINED,
    };
    with_root(agent, filter, |agent| {
        with_page(agent, |agent, page| {
            activate(agent, page)?;
            let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
                return agent.type_error("DOM bindings are busy");
            };
            let state = state(agent, page, receiver)?;
            // Step 7: an existing registration of this observer is updated
            // and its transient registrations are dropped.
            let mut existing = None;
            for edge in copy_edges(&wrappers, node)? {
                if let Some(registration) = live_registration(agent, page, edge)? {
                    if registration.get(agent, REG_OBSERVER)? == receiver {
                        existing = Some(registration);
                        break;
                    }
                }
            }
            if let Some(registration) = existing {
                let mut kept = Vec::new();
                for (weak, other) in owned(agent, state)? {
                    if other.get(agent, REG_SOURCE)? == registration.value() {
                        kill(agent, page, &mut wrappers, other)?;
                    } else {
                        kept.try_reserve(1).map_err(|_| OutOfMemory)?;
                        kept.push(weak);
                    }
                }
                set_owned(agent, state, &kept)?;
                registration.set(agent, REG_BITS, Value::number(f64::from(options.bits)))?;
                registration.set(agent, REG_FILTER, filter)?;
                return Ok(Value::UNDEFINED);
            }
            // Step 8: a new registration held by the node.
            let registration = PrivateArray::new(
                agent,
                &[
                    receiver,
                    Value::number(f64::from(options.bits)),
                    filter,
                    Value::UNDEFINED,
                ],
            )?;
            hold(agent, page, &mut wrappers, node, registration)?;
            push_owned(agent, state, registration)?;
            Ok(Value::UNDEFINED)
        })
    })
}

/// Brand a registration and make `node` keep it alive.
fn hold(
    agent: &mut Agent,
    page: &Page,
    wrappers: &mut DomHeap,
    node: NodeId,
    registration: PrivateArray,
) -> JsResult<()> {
    agent.weak_map_set(
        page.observers.registrations,
        registration.value(),
        Value::TRUE,
    )?;
    page.read(agent, node.document(), |agent, document| {
        wrappers
            .add_edge(document, node, registration.value())
            .map_err(|error| edge_error(agent, error))
    })?;
    let mut nodes = page.observers.nodes.borrow_mut();
    nodes.try_reserve(1).map_err(|_| OutOfMemory)?;
    nodes.insert(registration.value(), node);
    // Forget registrations whose node collected or moved them away.
    let pruned = page.observers.nodes_pruned_at.get();
    if nodes.len() > 64 && nodes.len() > pruned.saturating_mul(2) {
        nodes.retain(|&value, &mut node| wrappers.edges(node).contains(&value));
        page.observers.nodes_pruned_at.set(nodes.len());
    }
    Ok(())
}

fn edge_error(agent: &mut Agent, error: DomGcError) -> Abrupt {
    match error {
        DomGcError::Dom(rustic_dom::DomError::AllocationFailed) => Abrupt::OutOfMemory,
        DomGcError::Dom(error) => dom_exception::from_dom_error(agent, error),
        _ => agent.throw(
            rustic_js::runtime::ErrorKind::TypeError,
            "node is outside this document host",
        ),
    }
}

fn copy_edges(wrappers: &DomHeap, node: NodeId) -> Result<Vec<Value>, OutOfMemory> {
    let edges = wrappers.edges(node);
    let mut out = Vec::new();
    out.try_reserve_exact(edges.len())
        .map_err(|_| OutOfMemory)?;
    out.extend_from_slice(edges);
    Ok(out)
}

/// The registration behind a node edge, if the edge is one and still live.
fn live_registration(
    agent: &mut Agent,
    page: &Page,
    edge: Value,
) -> JsResult<Option<PrivateArray>> {
    if !agent.weak_map_has(page.observers.registrations, edge) {
        return Ok(None);
    }
    let registration = PrivateArray::from_value(edge).expect("branded registration");
    if registration.get(agent, REG_OBSERVER)? == Value::UNDEFINED {
        return Ok(None);
    }
    Ok(Some(registration))
}

fn disconnect(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        let state = state(agent, page, receiver)?;
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return agent.type_error("DOM bindings are busy");
        };
        for (_, registration) in owned(agent, state)? {
            agent.check_native_work()?;
            kill(agent, page, &mut wrappers, registration)?;
        }
        drop(wrappers);
        set_owned(agent, state, &[])?;
        let records = PrivateArray::new(agent, &[])?;
        state.set(agent, STATE_RECORDS, records.value())?;
        Ok(Value::UNDEFINED)
    })
}

fn take_records(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    // Entries from earlier host or parser edits become records first.
    with_page(agent, |agent, page| state(agent, page, receiver).map(drop))?;
    with_page(agent, |agent, page| {
        let state = state(agent, page, receiver)?;
        take_queue(agent, state).map(Value::object)
    })
}

/// Empty an observer's record queue into a new ordinary array.
fn take_queue(agent: &mut Agent, state: PrivateArray) -> JsResult<ObjectId> {
    let queue = array_at(agent, state, STATE_RECORDS)?;
    let records = values(agent, queue)?;
    let array = agent.create_array_from_list(&records)?;
    with_root(agent, Value::object(array), |agent| {
        let empty = PrivateArray::new(agent, &[])?;
        state.set(agent, STATE_RECORDS, empty.value())
    })?;
    Ok(array)
}

fn record_member<const MEMBER: u32>(
    agent: &mut Agent,
    receiver: Value,
    _: &[Value],
) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        let value = agent.weak_map_get(page.observers.records, receiver);
        match PrivateArray::from_value(value) {
            Some(record) => record.get(agent, MEMBER),
            None => agent.type_error("expected a MutationRecord receiver"),
        }
    })
}

/// Start recording in every document of the page.
fn activate(agent: &mut Agent, page: &Page) -> JsResult<()> {
    page.observers.active.set(true);
    enable_journals(agent, page)
}

pub(super) fn enable_journals(agent: &mut Agent, page: &Page) -> JsResult<()> {
    let Some(mut main) = page.tree.write() else {
        return agent.type_error("document is busy");
    };
    main.set_mutation_journal(true);
    drop(main);
    let Ok(documents) = page.additional.try_borrow() else {
        return agent.type_error("document registry is busy");
    };
    for cell in documents.values() {
        let Ok(mut document) = cell.tree.try_borrow_mut() else {
            return agent.type_error("document is busy");
        };
        document.set_mutation_journal(true);
    }
    Ok(())
}

/// Turn journal entries into queued records. A no-op while another binding
/// operation holds the page; that operation flushes when it finishes.
pub(super) fn flush(agent: &mut Agent, page: &Page) -> JsResult<()> {
    if (!page.observers.active.get() && !page.custom_elements.active())
        || page.observers.flushing.get()
    {
        return Ok(());
    }
    let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
        return Ok(());
    };
    let Some(entries) = take_entries(page)? else {
        return Ok(());
    };
    if entries.is_empty() {
        return Ok(());
    }
    page.observers.flushing.set(true);
    let mut result = Ok(());
    if page.observers.active.get() {
        for entry in &entries {
            result = queue_entry(agent, page, &mut wrappers, entry);
            if result.is_err() {
                break;
            }
        }
    }
    page.observers.flushing.set(false);
    drop(wrappers);
    result?;
    super::custom_elements::react(agent, page, &entries)
}

/// Every document's entries, main document first, or `None` while any
/// document is borrowed.
fn take_entries(page: &Page) -> Result<Option<Vec<MutationEntry>>, OutOfMemory> {
    let Some(mut main) = page.tree.write() else {
        return Ok(None);
    };
    let Ok(documents) = page.additional.try_borrow() else {
        return Ok(None);
    };
    let mut borrowed = Vec::new();
    borrowed
        .try_reserve_exact(documents.len())
        .map_err(|_| OutOfMemory)?;
    for cell in documents.values() {
        let Ok(document) = cell.tree.try_borrow_mut() else {
            return Ok(None);
        };
        borrowed.push(document);
    }
    // A cross-document move logs into one journal, so per-document order is
    // enough; documents created since the last flush start recording now.
    main.set_mutation_journal(true);
    let mut entries = main.take_mutation_entries();
    for document in &mut borrowed {
        document.set_mutation_journal(true);
        let more = document.take_mutation_entries();
        entries.try_reserve(more.len()).map_err(|_| OutOfMemory)?;
        entries.extend(more);
    }
    Ok(Some(entries))
}

/// DOM "queue a mutation record" for one journal entry.
fn queue_entry(
    agent: &mut Agent,
    page: &Page,
    wrappers: &mut DomHeap,
    entry: &MutationEntry,
) -> JsResult<()> {
    let target = entry.target();
    let ancestors = page.read(agent, target.document(), |agent, document| {
        let mut out = Vec::new();
        let mut at = Some(target);
        while let Some(id) = at {
            agent.check_native_work()?;
            let Ok(node) = document.node(id) else {
                // A host destroyed the node before its entry was seen.
                return Ok(None);
            };
            out.try_reserve(1).map_err(|_| OutOfMemory)?;
            out.push(id);
            at = node.parent();
        }
        Ok(Some(out))
    })?;
    let Some(ancestors) = ancestors else {
        return Ok(());
    };
    let (kind, required) = match entry {
        MutationEntry::ChildList { .. } => ("childList", CHILD_LIST),
        MutationEntry::Attribute { .. } => ("attributes", ATTRIBUTES),
        MutationEntry::CharacterData { .. } => ("characterData", CHARACTER_DATA),
    };
    let old_bit = match entry {
        MutationEntry::Attribute { .. } => ATTRIBUTE_OLD_VALUE,
        MutationEntry::CharacterData { .. } => CHARACTER_DATA_OLD_VALUE,
        MutationEntry::ChildList { .. } => 0,
    };
    // Observers in first-seen order, each with whether it wants the old value.
    let mut interested: Vec<(Value, bool)> = Vec::new();
    // Live subtree registrations of the parent's inclusive ancestors.
    let mut transient_sources = Vec::new();
    for (depth, &node) in ancestors.iter().enumerate() {
        for edge in copy_edges(wrappers, node)? {
            agent.check_native_work()?;
            if !agent.weak_map_has(page.observers.registrations, edge) {
                continue;
            }
            let Some(registration) = live_registration(agent, page, edge)? else {
                wrappers.remove_edge(node, edge);
                continue;
            };
            let bits = bits(agent, registration)?;
            if bits & SUBTREE != 0 {
                transient_sources.try_reserve(1).map_err(|_| OutOfMemory)?;
                transient_sources.push(registration);
            }
            if depth > 0 && bits & SUBTREE == 0 {
                continue;
            }
            if bits & required == 0 {
                continue;
            }
            if let MutationEntry::Attribute { name, .. } = entry {
                let filter = registration.get(agent, REG_FILTER)?;
                if let Some(filter) = PrivateArray::from_value(filter) {
                    if name.namespace().is_some() || !filter_contains(agent, filter, name.local())?
                    {
                        continue;
                    }
                }
            }
            let observer = registration.get(agent, REG_OBSERVER)?;
            let wants_old = bits & old_bit != 0;
            match interested.iter_mut().find(|(seen, _)| *seen == observer) {
                Some((_, old)) => *old |= wants_old,
                None => {
                    interested.try_reserve(1).map_err(|_| OutOfMemory)?;
                    interested.push((observer, wants_old));
                }
            }
        }
    }
    for (observer, wants_old) in interested {
        let record = new_record(agent, page, wrappers, entry, kind, wants_old)?;
        with_root(agent, record, |agent| {
            enqueue(agent, page, observer, record)
        })?;
    }
    if let MutationEntry::ChildList { removed, .. } = entry {
        if !removed.is_empty() && !transient_sources.is_empty() {
            add_transients(agent, page, wrappers, removed, &transient_sources)?;
        }
    }
    Ok(())
}

fn filter_contains(agent: &mut Agent, filter: PrivateArray, local: &str) -> JsResult<bool> {
    for value in values(agent, filter)? {
        let id = value.as_string().expect("filter strings");
        let units = agent.heap.string(id).units();
        if units.eq(local.encode_utf16()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// DOM "remove" step 15: removed nodes carry transient registrations for
/// their former ancestors' subtree observers until the next notification.
fn add_transients(
    agent: &mut Agent,
    page: &Page,
    wrappers: &mut DomHeap,
    removed: &[NodeId],
    sources: &[PrivateArray],
) -> JsResult<()> {
    for &node in removed {
        for &source in sources {
            agent.check_native_work()?;
            let observer = source.get(agent, REG_OBSERVER)?;
            let bits = source.get(agent, REG_BITS)?;
            let filter = source.get(agent, REG_FILTER)?;
            let transient = PrivateArray::new(agent, &[observer, bits, filter, source.value()])?;
            with_root(agent, transient.value(), |agent| {
                let state = state(agent, page, observer)?;
                hold(agent, page, wrappers, node, transient)?;
                push_owned(agent, state, transient)
            })?;
        }
    }
    Ok(())
}

fn new_record(
    agent: &mut Agent,
    page: &Page,
    wrappers: &mut DomHeap,
    entry: &MutationEntry,
    kind: &str,
    wants_old: bool,
) -> JsResult<Value> {
    let proto = prototype(agent, RECORD)?;
    let record = Value::object(agent.new_object_with(Some(proto))?);
    let fields = PrivateArray::new(agent, &[Value::NULL; 9])?;
    agent.weak_map_set(page.observers.records, record, fields.value())?;
    with_root(agent, record, |agent| {
        let kind = agent.string(kind)?;
        fields.set(agent, 0, kind)?;
        let target = wrap_node(agent, page, wrappers, entry.target())?;
        fields.set(agent, 1, target)?;
        let (added, removed, previous, next): (&[NodeId], &[NodeId], _, _) = match entry {
            MutationEntry::ChildList {
                added,
                removed,
                previous,
                next,
                ..
            } => (added, removed, *previous, *next),
            _ => (&[], &[], None, None),
        };
        let added = node_list(agent, page, wrappers, added)?;
        fields.set(agent, 2, added)?;
        let removed = node_list(agent, page, wrappers, removed)?;
        fields.set(agent, 3, removed)?;
        for (slot, sibling) in [(4, previous), (5, next)] {
            if let Some(id) = sibling {
                let value = wrap_node(agent, page, wrappers, id)?;
                fields.set(agent, slot, value)?;
            }
        }
        match entry {
            MutationEntry::Attribute {
                name, old_value, ..
            } => {
                let local = agent.string(name.local())?;
                fields.set(agent, 6, local)?;
                if let Some(namespace) = name.namespace() {
                    let namespace = agent.string(namespace)?;
                    fields.set(agent, 7, namespace)?;
                }
                if let (true, Some(old)) = (wants_old, old_value) {
                    let old = from_dom_string(agent, old)?;
                    fields.set(agent, 8, old)?;
                }
            }
            MutationEntry::CharacterData { old_value, .. } if wants_old => {
                let old = from_dom_string(agent, old_value)?;
                fields.set(agent, 8, old)?;
            }
            _ => {}
        }
        Ok(())
    })?;
    Ok(record)
}

fn wrap_node(
    agent: &mut Agent,
    page: &Page,
    wrappers: &mut DomHeap,
    id: NodeId,
) -> JsResult<Value> {
    page.read(agent, id.document(), |agent, document| {
        wrap(wrappers, agent, document, id)
    })
}

/// A static `NodeList`; nodes may belong to different documents.
fn node_list(
    agent: &mut Agent,
    page: &Page,
    wrappers: &mut DomHeap,
    nodes: &[NodeId],
) -> JsResult<Value> {
    let proto = prototype(agent, NODE_LIST)?;
    let holder = PrivateArray::new(agent, &[])?;
    with_root(agent, holder.value(), |agent| {
        for &id in nodes {
            let value = wrap_node(agent, page, wrappers, id)?;
            push(agent, holder, value)?;
        }
        let items = values(agent, holder)?;
        agent
            .new_indexed_snapshot(NODE_LIST, proto, items.into_boxed_slice())
            .map(Value::object)
    })
}

/// Append to the record queue and schedule the notification microtask.
fn enqueue(agent: &mut Agent, page: &Page, observer: Value, record: Value) -> JsResult<()> {
    let state = state(agent, page, observer)?;
    let queue = array_at(agent, state, STATE_RECORDS)?;
    push(agent, queue, record)?;
    let pending = array_at(agent, page.observers.storage(), STORE_PENDING)?;
    if !values(agent, pending)?.contains(&observer) {
        push(agent, pending, observer)?;
    }
    queue_notification(agent, page)
}

pub(super) fn signal_slot(agent: &mut Agent, page: &Page, slot: Value) -> JsResult<()> {
    let slots = array_at(agent, page.observers.storage(), STORE_SLOTS)?;
    if !values(agent, slots)?.contains(&slot) {
        push(agent, slots, slot)?;
    }
    queue_notification(agent, page)
}

fn queue_notification(agent: &mut Agent, page: &Page) -> JsResult<()> {
    if !page.observers.queued.get() {
        agent.enqueue_microtask(page.observers.notify, Value::UNDEFINED)?;
        page.observers.queued.set(true);
    }
    Ok(())
}

/// DOM "notify mutation observers", run as a microtask.
fn notify(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let (pending, slots) = with_page(agent, |agent, page| {
        page.observers.queued.set(false);
        let pending = array_at(agent, page.observers.storage(), STORE_PENDING)?;
        let empty = PrivateArray::new(agent, &[])?;
        page.observers
            .storage()
            .set(agent, STORE_PENDING, empty.value())?;
        let slots = array_at(agent, page.observers.storage(), STORE_SLOTS)?;
        let empty = PrivateArray::new(agent, &[])?;
        page.observers
            .storage()
            .set(agent, STORE_SLOTS, empty.value())?;
        Ok((pending, slots))
    })?;
    let roots = PrivateArray::new(agent, &[pending.value(), slots.value()])?;
    with_root(agent, roots.value(), |agent| {
        let mut observers = Vec::new();
        for observer in values(agent, pending)? {
            let serial = with_page(agent, |agent, page| {
                let state = state(agent, page, observer)?;
                state.get(agent, STATE_SERIAL)
            })?;
            observers.try_reserve(1).map_err(|_| OutOfMemory)?;
            observers.push((serial.as_number().unwrap_or(0.0), observer));
        }
        // Observers are notified in creation order, as engines do.
        observers.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, observer) in observers {
            agent.check_native_work()?;
            let (callback, records) = with_page(agent, |agent, page| {
                let state = state(agent, page, observer)?;
                let records = take_queue(agent, state)?;
                drop_transients(agent, page, state)?;
                Ok((state.get(agent, STATE_CALLBACK)?, records))
            })?;
            let records = Value::object(records);
            let empty = {
                let key = rustic_js::runtime::PropertyKey::String(agent.names.length);
                let len = agent.get(records.as_object().expect("array"), key)?;
                len == Value::number(0.0)
            };
            if empty {
                continue;
            }
            let result = with_root(agent, records, |agent| {
                agent.call(callback, observer, &[records, observer])
            });
            match result {
                Ok(_) => {}
                Err(Abrupt::Throw(error)) => super::events::report_callback_error(agent, error)?,
                Err(error) => return Err(error),
            }
        }
        for index in 0..slots.len(agent)? {
            let slot = slots.get(agent, index)?;
            super::events::fire_slot_change(agent, slot)?;
        }
        Ok(Value::UNDEFINED)
    })
}

/// Mark this observer's transient registrations dead, remove them from
/// their nodes and forget them.
fn drop_transients(agent: &mut Agent, page: &Page, state: PrivateArray) -> JsResult<()> {
    let mut kept = Vec::new();
    let mut transients = Vec::new();
    for (weak, registration) in owned(agent, state)? {
        if registration.get(agent, REG_SOURCE)? != Value::UNDEFINED {
            transients.try_reserve(1).map_err(|_| OutOfMemory)?;
            transients.push(registration);
        } else {
            kept.try_reserve(1).map_err(|_| OutOfMemory)?;
            kept.push(weak);
        }
    }
    if !transients.is_empty() {
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return agent.type_error("DOM bindings are busy");
        };
        for registration in transients {
            kill(agent, page, &mut wrappers, registration)?;
        }
        drop(wrappers);
        set_owned(agent, state, &kept)?;
    }
    Ok(())
}

/// HTML `queueMicrotask(callback)`: the callback runs after jobs already
/// queued, and an exception it throws is reported, not propagated.
pub(super) fn queue_microtask(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let callback = args.first().copied().unwrap_or(Value::UNDEFINED);
    if !agent.is_callable(callback) {
        return agent.type_error("queueMicrotask callback is not callable");
    }
    with_page(agent, |agent, page| {
        agent.enqueue_microtask(page.observers.invoke, callback)?;
        Ok(Value::UNDEFINED)
    })
}

fn invoke(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let callback = args.first().copied().unwrap_or(Value::UNDEFINED);
    match agent.call(callback, Value::UNDEFINED, &[]) {
        Ok(_) => Ok(Value::UNDEFINED),
        Err(Abrupt::Throw(error)) => {
            super::events::report_callback_error(agent, error)?;
            Ok(Value::UNDEFINED)
        }
        Err(error) => Err(error),
    }
}
