//! Script scrolling (CSSOM View §5 and §7; ADR 0064): `window.scrollX`,
//! `scrollY`, `pageXOffset`, `pageYOffset`, `scroll()`, `scrollTo()` and
//! `scrollBy()`, and `Element.scrollTop`, `scrollLeft`, `scroll()`,
//! `scrollTo()` and `scrollBy()`.
//!
//! Positions live in the host's [`super::PageTree`], which clamps them to
//! the layout's scroll ranges. A change queues a `scroll` event for the
//! next rendering opportunity ("run the scroll steps"): at the document
//! (bubbling to the window) for the viewport, at the element otherwise.
//! Scrolling is always instant (`behavior` is ignored).

use rustic_dom::{NodeId, NodeKind};
use rustic_js::runtime::{Agent, JsResult, NativeFn, ObjectId, PropertyDescriptor, Value};

use super::{events, read_node, with_page};

/// Most `scroll` event targets waiting for a rendering opportunity.
const MAX_PENDING: usize = 1024;

/// Scroll event targets waiting for the next rendering opportunity:
/// `None` is the document (the viewport scrolled).
#[derive(Debug, Default)]
pub(super) struct PendingScrolls(std::cell::RefCell<Vec<Option<NodeId>>>);

impl PendingScrolls {
    pub(super) fn take(&self) -> Vec<Option<NodeId>> {
        std::mem::take(&mut self.0.borrow_mut())
    }

    fn push(&self, target: Option<NodeId>) {
        let mut pending = self.0.borrow_mut();
        if !pending.contains(&target) && pending.len() < MAX_PENDING {
            pending.push(target);
        }
    }
}

fn accessor(
    agent: &mut Agent,
    object: ObjectId,
    name: &str,
    get: NativeFn,
    set: Option<NativeFn>,
) -> JsResult<()> {
    let get = Value::object(agent.new_native_function(get, &format!("get {name}"), 0)?);
    let set = match set {
        Some(set) => Value::object(agent.new_native_function(set, &format!("set {name}"), 1)?),
        None => Value::UNDEFINED,
    };
    let key = agent.heap.key(name)?;
    agent.define_property_or_throw(
        object,
        key,
        PropertyDescriptor::accessor(get, set, true, true),
    )
}

/// Install the Window members on the global object.
pub(super) fn install_window(agent: &mut Agent) -> JsResult<()> {
    let global = agent.intrinsics.global;
    for (name, get) in [
        ("scrollX", window_x as NativeFn),
        ("pageXOffset", window_x),
        ("scrollY", window_y),
        ("pageYOffset", window_y),
    ] {
        accessor(agent, global, name, get, None)?;
    }
    for (name, method) in [
        ("scroll", window_scroll_to as NativeFn),
        ("scrollTo", window_scroll_to),
        ("scrollBy", window_scroll_by),
    ] {
        events::define_method(agent, global, name, method, 0)?;
    }
    Ok(())
}

/// Install the Element members on its prototype.
pub(super) fn install_element(agent: &mut Agent, prototype: ObjectId) -> JsResult<()> {
    accessor(
        agent,
        prototype,
        "scrollTop",
        element_top,
        Some(set_element_top),
    )?;
    accessor(
        agent,
        prototype,
        "scrollLeft",
        element_left,
        Some(set_element_left),
    )?;
    for (name, method) in [
        ("scroll", element_scroll_to as NativeFn),
        ("scrollTo", element_scroll_to),
        ("scrollBy", element_scroll_by),
    ] {
        events::define_method(agent, prototype, name, method, 0)?;
    }
    Ok(())
}

fn position(agent: &mut Agent, target: Option<NodeId>) -> JsResult<(f64, f64)> {
    with_page(agent, |_, page| Ok(page.tree.scroll_position(target)))
}

/// Scroll `target` to `(x, y)` and queue its `scroll` event if it moved.
fn scroll(agent: &mut Agent, target: Option<NodeId>, x: f64, y: f64) -> JsResult<()> {
    with_page(agent, |agent, page| {
        let moved = page.tree.scroll_to(target, x, y);
        agent.check_native_work()?;
        // A missing layout scrolls nothing, as an unrendered document does.
        if moved.unwrap_or(false) {
            page.scrolls.push(target);
        }
        Ok(())
    })
}

/// CSSOM View "normalize non-finite values".
fn finite(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

/// The `(x, y)` a scroll method asks for: `(x, y)` numbers, or a
/// `ScrollToOptions` dictionary whose missing members keep `current`
/// (for `scrollBy`, `current` is zero and the result is a delta).
fn requested(agent: &mut Agent, args: &[Value], current: (f64, f64)) -> JsResult<(f64, f64)> {
    match args {
        [] => Ok(current),
        [options] => {
            if options.is_nullish() {
                return Ok(current);
            }
            let Some(object) = options.as_object() else {
                return agent.type_error("expected a ScrollToOptions dictionary");
            };
            let member = |agent: &mut Agent, name: &str, keep: f64| -> JsResult<f64> {
                let key = agent.heap.key(name)?;
                let value = agent.get(object, key)?;
                if value == Value::UNDEFINED {
                    Ok(keep)
                } else {
                    Ok(finite(agent.to_number(value)?))
                }
            };
            let x = member(agent, "left", current.0)?;
            let y = member(agent, "top", current.1)?;
            Ok((x, y))
        }
        [x, y, ..] => {
            let x = finite(agent.to_number(*x)?);
            let y = finite(agent.to_number(*y)?);
            Ok((x, y))
        }
    }
}

fn window_x(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::number(position(agent, None)?.0))
}

fn window_y(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::number(position(agent, None)?.1))
}

fn window_scroll_to(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let current = position(agent, None)?;
    let (x, y) = requested(agent, args, current)?;
    scroll(agent, None, x, y)?;
    Ok(Value::UNDEFINED)
}

fn window_scroll_by(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let (dx, dy) = requested(agent, args, (0.0, 0.0))?;
    let (x, y) = position(agent, None)?;
    scroll(agent, None, x + dx, y + dy)?;
    Ok(Value::UNDEFINED)
}

/// The element `receiver` names, if it is connected to the main document.
fn element(agent: &mut Agent, receiver: Value) -> JsResult<Option<NodeId>> {
    let node = read_node(agent, receiver, |agent, _, document, id, node| {
        if !matches!(node.kind(), NodeKind::Element(_)) {
            return agent.type_error("expected Element");
        }
        events::reaches_root(document, id, || agent.check_native_work())
            .map(|connected| connected.then_some(id))
    })?;
    with_page(agent, |_, page| {
        Ok(node.filter(|node| node.document() == page.main))
    })
}

fn element_position(agent: &mut Agent, receiver: Value) -> JsResult<Option<(NodeId, (f64, f64))>> {
    match element(agent, receiver)? {
        Some(node) => Ok(Some((node, position(agent, Some(node))?))),
        None => Ok(None),
    }
}

fn element_top(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    let y = element_position(agent, receiver)?.map_or(0.0, |(_, (_, y))| y);
    Ok(Value::number(y))
}

fn element_left(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    let x = element_position(agent, receiver)?.map_or(0.0, |(_, (x, _))| x);
    Ok(Value::number(x))
}

fn set_element_top(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let value = finite(agent.to_number(args.first().copied().unwrap_or(Value::UNDEFINED))?);
    if let Some((node, (x, _))) = element_position(agent, receiver)? {
        scroll(agent, Some(node), x, value)?;
    }
    Ok(Value::UNDEFINED)
}

fn set_element_left(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let value = finite(agent.to_number(args.first().copied().unwrap_or(Value::UNDEFINED))?);
    if let Some((node, (_, y))) = element_position(agent, receiver)? {
        scroll(agent, Some(node), value, y)?;
    }
    Ok(Value::UNDEFINED)
}

fn element_scroll_to(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    if let Some((node, current)) = element_position(agent, receiver)? {
        let (x, y) = requested(agent, args, current)?;
        scroll(agent, Some(node), x, y)?;
    }
    Ok(Value::UNDEFINED)
}

fn element_scroll_by(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    if let Some((node, (x, y))) = element_position(agent, receiver)? {
        let (dx, dy) = requested(agent, args, (0.0, 0.0))?;
        scroll(agent, Some(node), x + dx, y + dy)?;
    }
    Ok(Value::UNDEFINED)
}
