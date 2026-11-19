//! `ResizeObserver` ([Resize Observer §3]): observations are gathered and
//! broadcast in the rendering turn, after animation frame callbacks, from
//! the same layout snapshot that serves script reads.
//!
//! An observer with at least one observation is held by the page, and it
//! holds its targets, until `unobserve` or `disconnect` (TD-155).
//!
//! [Resize Observer §3]: https://drafts.csswg.org/resize-observer/#api

use std::cell::Cell;

use rustic_dom::NodeKind;
use rustic_js::runtime::{
    Abrupt, Agent, IntegrityLevel, JsResult, NativeFn, ObjectId, OutOfMemory, Value,
};

use crate::heap::WrapperKind;
use crate::webidl::coerce_string;

use super::events::{PrivateArray, define_method, with_root};
use super::{
    BoxSizes, LayoutReadError, Page, identity, illegal_constructor, install_interface, prototype,
    read_node, with_page,
};

const OBSERVER: &str = "ResizeObserver";
const ENTRY: &str = "ResizeObserverEntry";
const SIZE: &str = "ResizeObserverSize";

/// Observer state: `[callback, observations, serial, active targets]`.
const CALLBACK: u32 = 0;
const OBSERVATIONS: u32 = 1;
const SERIAL: u32 = 2;
const ACTIVE: u32 = 3;

/// Observation: `[target, box, last inline size, last block size]`.
const TARGET: u32 = 0;
const BOX: u32 = 1;
const LAST_INLINE: u32 = 2;
const LAST_BLOCK: u32 = 3;

const CONTENT_BOX: f64 = 0.0;
const BORDER_BOX: f64 = 1.0;
const DEVICE_PIXEL_CONTENT_BOX: f64 = 2.0;

/// Page-lifetime state.
#[derive(Debug)]
pub(super) struct ResizeObservers {
    states: ObjectId,
    entries: ObjectId,
    sizes: ObjectId,
    /// Rooted `[observers with observations]`.
    storage: Value,
    next_serial: Cell<u32>,
}

impl ResizeObservers {
    pub(super) fn new(agent: &mut Agent) -> JsResult<Self> {
        let states = agent.new_weak_map()?;
        let entries = agent.new_weak_map()?;
        let sizes = agent.new_weak_map()?;
        let observing = PrivateArray::new(agent, &[])?;
        let storage = PrivateArray::new(
            agent,
            &[
                observing.value(),
                Value::object(states),
                Value::object(entries),
                Value::object(sizes),
            ],
        )?;
        agent.root(storage.value())?;
        Ok(Self {
            states,
            entries,
            sizes,
            storage: storage.value(),
            next_serial: Cell::new(0),
        })
    }

    fn observing(&self, agent: &mut Agent) -> JsResult<PrivateArray> {
        let storage = PrivateArray::from_value(self.storage).expect("rooted storage");
        let list = storage.get(agent, 0)?;
        Ok(PrivateArray::from_value(list).expect("observer list"))
    }
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(agent, OBSERVER, None, construct, 1, &[], |agent, proto| {
        for (name, method, length) in [
            ("observe", observe as NativeFn, 1),
            ("unobserve", unobserve, 1),
            ("disconnect", disconnect, 0),
        ] {
            define_method(agent, proto, name, method, length)?;
        }
        Ok(())
    })?;
    install_interface(
        agent,
        ENTRY,
        None,
        illegal_constructor,
        0,
        &[
            ("target", "get target", entry_field::<0>),
            ("contentRect", "get contentRect", entry_field::<1>),
            ("borderBoxSize", "get borderBoxSize", entry_field::<2>),
            ("contentBoxSize", "get contentBoxSize", entry_field::<3>),
            (
                "devicePixelContentBoxSize",
                "get devicePixelContentBoxSize",
                entry_field::<4>,
            ),
        ],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        SIZE,
        None,
        illegal_constructor,
        0,
        &[
            ("inlineSize", "get inlineSize", size_field::<0>),
            ("blockSize", "get blockSize", size_field::<1>),
        ],
        |_, _| Ok(()),
    )
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

fn array_at(agent: &mut Agent, array: PrivateArray, index: u32) -> JsResult<PrivateArray> {
    let value = array.get(agent, index)?;
    Ok(PrivateArray::from_value(value).expect("private observer array"))
}

fn state(agent: &mut Agent, page: &Page, observer: Value) -> JsResult<PrivateArray> {
    match PrivateArray::from_value(agent.weak_map_get(page.resize_observers.states, observer)) {
        Some(state) => Ok(state),
        None => agent.type_error("expected a ResizeObserver receiver"),
    }
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("ResizeObserver requires new");
    };
    let callback = args.first().copied().unwrap_or(Value::UNDEFINED);
    if !agent.is_callable(callback) {
        return agent.type_error("ResizeObserver callback is not callable");
    }
    let proto = agent.host_prototype_from_new_target(OBSERVER, new_target)?;
    let observer = Value::object(agent.new_object_with(Some(proto))?);
    with_page(agent, |agent, page| {
        let observations = PrivateArray::new(agent, &[])?;
        let active = PrivateArray::new(agent, &[])?;
        let serial = page.resize_observers.next_serial.get();
        page.resize_observers
            .next_serial
            .set(serial.saturating_add(1));
        let state = PrivateArray::new(
            agent,
            &[
                callback,
                observations.value(),
                Value::number(f64::from(serial)),
                active.value(),
            ],
        )?;
        agent.weak_map_set(page.resize_observers.states, observer, state.value())
    })?;
    Ok(observer)
}

fn element_target(agent: &mut Agent, target: Value) -> JsResult<()> {
    let is_element = with_page(agent, |agent, page| {
        let Ok(wrappers) = page.wrappers.try_borrow() else {
            return agent.type_error("DOM bindings are busy");
        };
        let id = identity(agent, &wrappers, target, WrapperKind::Node)?;
        drop(wrappers);
        page.read(agent, id.document(), |_, document| {
            Ok(matches!(
                document.node(id).map(|node| node.kind()),
                Ok(NodeKind::Element(_))
            ))
        })
    })?;
    if is_element {
        Ok(())
    } else {
        agent.type_error("ResizeObserver observes elements")
    }
}

fn box_option(agent: &mut Agent, options: Value) -> JsResult<f64> {
    if options.is_nullish() {
        return Ok(CONTENT_BOX);
    }
    let Some(object) = options.as_object() else {
        return agent.type_error("ResizeObserverOptions must be an object");
    };
    let key = agent.heap.key("box")?;
    let value = agent.get(object, key)?;
    if value == Value::UNDEFINED {
        return Ok(CONTENT_BOX);
    }
    let id = coerce_string(agent, value)?;
    let text = agent.heap.string(id).to_text();
    match text.as_str() {
        "content-box" => Ok(CONTENT_BOX),
        "border-box" => Ok(BORDER_BOX),
        "device-pixel-content-box" => Ok(DEVICE_PIXEL_CONTENT_BOX),
        _ => agent.type_error("invalid ResizeObserverBoxOptions value"),
    }
}

/// Remove `target`'s observation; `true` if the observer still observes.
fn remove_observation(agent: &mut Agent, state: PrivateArray, target: Value) -> JsResult<bool> {
    let observations = array_at(agent, state, OBSERVATIONS)?;
    let mut kept = Vec::new();
    for value in values(agent, observations)? {
        let observation = PrivateArray::from_value(value).expect("observation");
        if observation.get(agent, TARGET)? != target {
            kept.try_reserve(1).map_err(|_| OutOfMemory)?;
            kept.push(value);
        }
    }
    let any = !kept.is_empty();
    let list = PrivateArray::new(agent, &kept)?;
    state.set(agent, OBSERVATIONS, list.value())?;
    Ok(any)
}

fn set_observing(agent: &mut Agent, observer: Value, observing: bool) -> JsResult<()> {
    with_page(agent, |agent, page| {
        let list = page.resize_observers.observing(agent)?;
        let current = values(agent, list)?;
        let present = current.contains(&observer);
        if observing && !present {
            let len = list.len(agent)?;
            list.set(agent, len, observer)?;
        } else if !observing && present {
            let kept: Vec<Value> = current
                .into_iter()
                .filter(|&value| value != observer)
                .collect();
            let replacement = PrivateArray::new(agent, &kept)?;
            let storage = PrivateArray::from_value(page.resize_observers.storage).expect("storage");
            storage.set(agent, 0, replacement.value())?;
        }
        Ok(())
    })
}

fn observe(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let state = with_page(agent, |agent, page| state(agent, page, receiver))?;
    let target = args.first().copied().unwrap_or(Value::UNDEFINED);
    element_target(agent, target)?;
    let observed_box = box_option(agent, args.get(1).copied().unwrap_or(Value::UNDEFINED))?;
    remove_observation(agent, state, target)?;
    // The first gathering always reports: no size equals -1.
    let observation = PrivateArray::new(
        agent,
        &[
            target,
            Value::number(observed_box),
            Value::number(-1.0),
            Value::number(-1.0),
        ],
    )?;
    let observations = array_at(agent, state, OBSERVATIONS)?;
    let len = observations.len(agent)?;
    observations.set(agent, len, observation.value())?;
    set_observing(agent, receiver, true)?;
    Ok(Value::UNDEFINED)
}

fn unobserve(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let state = with_page(agent, |agent, page| state(agent, page, receiver))?;
    let target = args.first().copied().unwrap_or(Value::UNDEFINED);
    element_target(agent, target)?;
    let observing = remove_observation(agent, state, target)?;
    set_observing(agent, receiver, observing)?;
    Ok(Value::UNDEFINED)
}

fn disconnect(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    let state = with_page(agent, |agent, page| state(agent, page, receiver))?;
    let empty = PrivateArray::new(agent, &[])?;
    state.set(agent, OBSERVATIONS, empty.value())?;
    let empty = PrivateArray::new(agent, &[])?;
    state.set(agent, ACTIVE, empty.value())?;
    set_observing(agent, receiver, false)?;
    Ok(Value::UNDEFINED)
}

fn entry_field<const SLOT: u32>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        match PrivateArray::from_value(agent.weak_map_get(page.resize_observers.entries, this)) {
            Some(fields) => fields.get(agent, SLOT),
            None => agent.type_error("expected a ResizeObserverEntry receiver"),
        }
    })
}

fn size_field<const SLOT: u32>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        match PrivateArray::from_value(agent.weak_map_get(page.resize_observers.sizes, this)) {
            Some(fields) => fields.get(agent, SLOT),
            None => agent.type_error("expected a ResizeObserverSize receiver"),
        }
    })
}

/// The target's depth (number of ancestors) and its layout box sizes.
fn measure(agent: &mut Agent, target: Value) -> JsResult<(u32, BoxSizes)> {
    let (depth, id) = read_node(agent, target, |agent, _, document, id, node| {
        let mut depth = 0u32;
        let mut current = node.parent();
        while let Some(parent) = current {
            agent.check_native_work()?;
            depth = depth.saturating_add(1);
            current = document.node(parent).ok().and_then(|node| node.parent());
        }
        Ok((depth, id))
    })?;
    let sizes = with_page(agent, |agent, page| {
        if id.document() != page.main {
            return Ok(None);
        }
        let result = page.tree.box_sizes(id);
        agent.check_native_work()?;
        match result {
            Ok(sizes) => Ok(sizes),
            Err(LayoutReadError::Limit) => Err(Abrupt::Throw(crate::dom_exception::new(
                agent,
                "QuotaExceededError",
                "layout flush budget exhausted",
            )?)),
            Err(LayoutReadError::Unavailable) => Ok(None),
        }
    })?;
    Ok((depth, sizes.unwrap_or_default()))
}

fn observed_size(sizes: &BoxSizes, observed_box: f64) -> [f64; 2] {
    if observed_box == BORDER_BOX {
        sizes.border
    } else {
        sizes.content
    }
}

/// Gather active observations deeper than `depth`; `true` if any were
/// skipped because they were not deeper.
fn gather(agent: &mut Agent, depth: u32) -> JsResult<(bool, bool)> {
    let observers = with_page(agent, |agent, page| {
        let list = page.resize_observers.observing(agent)?;
        values(agent, list)
    })?;
    let holder = PrivateArray::new(agent, &observers)?;
    with_root(agent, holder.value(), |agent| {
        let (mut any_active, mut skipped) = (false, false);
        for observer in observers {
            let state = with_page(agent, |agent, page| state(agent, page, observer))?;
            let mut active = Vec::new();
            let observations = array_at(agent, state, OBSERVATIONS)?;
            for value in values(agent, observations)? {
                agent.check_native_work()?;
                let observation = PrivateArray::from_value(value).expect("observation");
                let target = observation.get(agent, TARGET)?;
                let observed_box = observation.get(agent, BOX)?.as_number().unwrap_or(0.0);
                let (target_depth, sizes) = measure(agent, target)?;
                let [inline, block] = observed_size(&sizes, observed_box);
                let last = [
                    observation
                        .get(agent, LAST_INLINE)?
                        .as_number()
                        .unwrap_or(-1.0),
                    observation
                        .get(agent, LAST_BLOCK)?
                        .as_number()
                        .unwrap_or(-1.0),
                ];
                if last == [inline, block] {
                    continue;
                }
                if target_depth > depth || depth == 0 {
                    active.try_reserve(1).map_err(|_| OutOfMemory)?;
                    active.push(value);
                } else {
                    skipped = true;
                }
            }
            any_active |= !active.is_empty();
            let list = PrivateArray::new(agent, &active)?;
            state.set(agent, ACTIVE, list.value())?;
        }
        Ok((any_active, skipped))
    })
}

fn size_object(agent: &mut Agent, inline: f64, block: f64) -> JsResult<Value> {
    let proto = prototype(agent, SIZE)?;
    let object = Value::object(agent.new_object_with(Some(proto))?);
    with_root(agent, object, |agent| {
        let fields = PrivateArray::new(agent, &[Value::number(inline), Value::number(block)])?;
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.resize_observers.sizes, object, fields.value())
        })?;
        Ok(object)
    })
}

fn frozen_sizes(agent: &mut Agent, inline: f64, block: f64) -> JsResult<Value> {
    let size = size_object(agent, inline, block)?;
    with_root(agent, size, |agent| {
        let array = agent.create_array_from_list(&[size])?;
        agent.set_integrity_level(array, IntegrityLevel::Frozen)?;
        Ok(Value::object(array))
    })
}

fn new_entry(agent: &mut Agent, target: Value, sizes: &BoxSizes) -> JsResult<Value> {
    let rect = super::dom_rect::new_readonly(
        agent,
        [
            sizes.padding[0],
            sizes.padding[1],
            sizes.content[0],
            sizes.content[1],
        ],
    )?;
    with_root(agent, rect, |agent| {
        let border = frozen_sizes(agent, sizes.border[0], sizes.border[1])?;
        with_root(agent, border, |agent| {
            let content = frozen_sizes(agent, sizes.content[0], sizes.content[1])?;
            with_root(agent, content, |agent| {
                // One CSS pixel per device pixel in the headless host (TD-155).
                let device = frozen_sizes(agent, sizes.content[0], sizes.content[1])?;
                with_root(agent, device, |agent| {
                    let fields =
                        PrivateArray::new(agent, &[target, rect, border, content, device])?;
                    with_root(agent, fields.value(), |agent| {
                        let proto = prototype(agent, ENTRY)?;
                        let entry = Value::object(agent.new_object_with(Some(proto))?);
                        with_page(agent, |agent, page| {
                            agent.weak_map_set(page.resize_observers.entries, entry, fields.value())
                        })?;
                        Ok(entry)
                    })
                })
            })
        })
    })
}

/// Broadcast the gathered observations in observer creation order; the
/// shallowest depth among them limits the next gathering.
fn broadcast(agent: &mut Agent) -> JsResult<u32> {
    let observers = with_page(agent, |agent, page| {
        let list = page.resize_observers.observing(agent)?;
        values(agent, list)
    })?;
    let holder = PrivateArray::new(agent, &observers)?;
    with_root(agent, holder.value(), |agent| {
        let mut ordered = Vec::new();
        for observer in observers {
            let serial = with_page(agent, |agent, page| {
                state(agent, page, observer)?.get(agent, SERIAL)
            })?;
            ordered.try_reserve(1).map_err(|_| OutOfMemory)?;
            ordered.push((serial.as_number().unwrap_or(0.0), observer));
        }
        ordered.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut shallowest = u32::MAX;
        for (_, observer) in ordered {
            let state = with_page(agent, |agent, page| state(agent, page, observer))?;
            let active = array_at(agent, state, ACTIVE)?;
            let observations = values(agent, active)?;
            if observations.is_empty() {
                continue;
            }
            let empty = PrivateArray::new(agent, &[])?;
            state.set(agent, ACTIVE, empty.value())?;
            let entries = PrivateArray::new(agent, &[])?;
            with_root(agent, entries.value(), |agent| {
                for value in observations {
                    let observation = PrivateArray::from_value(value).expect("observation");
                    let target = observation.get(agent, TARGET)?;
                    let observed_box = observation.get(agent, BOX)?.as_number().unwrap_or(0.0);
                    let (depth, sizes) = measure(agent, target)?;
                    let entry = new_entry(agent, target, &sizes)?;
                    let len = entries.len(agent)?;
                    entries.set(agent, len, entry)?;
                    let [inline, block] = observed_size(&sizes, observed_box);
                    observation.set(agent, LAST_INLINE, Value::number(inline))?;
                    observation.set(agent, LAST_BLOCK, Value::number(block))?;
                    shallowest = shallowest.min(depth);
                }
                let list = values(agent, entries)?;
                let array = Value::object(agent.create_array_from_list(&list)?);
                let callback = state.get(agent, CALLBACK)?;
                let result = with_root(agent, array, |agent| {
                    agent.call(callback, observer, &[array, observer])
                });
                match result {
                    Ok(_) => Ok(()),
                    Err(Abrupt::Throw(error)) => super::events::report_callback_error(agent, error),
                    Err(error) => Err(error),
                }
            })?;
        }
        Ok(shallowest)
    })
}

/// The Resize Observer steps of "update the rendering": gather and
/// broadcast until nothing is active, then report skipped observations.
pub(super) fn run(agent: &mut Agent) -> JsResult<()> {
    let observing = with_page(agent, |agent, page| {
        let list = page.resize_observers.observing(agent)?;
        list.len(agent)
    })?;
    if observing == 0 {
        return Ok(());
    }
    let mut depth = 0;
    let (mut active, mut skipped) = gather(agent, depth)?;
    while active {
        depth = broadcast(agent)?;
        (active, skipped) = gather(agent, depth)?;
    }
    if skipped {
        let error = agent.new_error(
            rustic_js::runtime::ErrorKind::Error,
            "ResizeObserver loop completed with undelivered notifications.",
        )?;
        super::events::report_exception(agent, error)?;
    }
    Ok(())
}
