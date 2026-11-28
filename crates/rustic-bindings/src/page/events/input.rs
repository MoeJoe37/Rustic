//! Host input and the activation behavior of HTML buttons and checkboxes.

use rustic_dom::{
    Document, DomString, Element, HTML_NAMESPACE, Node, NodeId, NodeKind, QualifiedName,
};
use rustic_js::runtime::{Agent, JsResult, NativeFn, PropertyDescriptor, Value};

use super::super::with_mut_tree;
use super::{
    Event, PrivateArray, Target, fire, install_interface, read_node, state, ui, with_page,
    with_root, wrap,
};
use crate::dom_exception;

/// Modifier keys held during an input event.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers {
    /// Control is pressed.
    pub control: bool,
    /// Alt is pressed.
    pub alt: bool,
    /// Shift is pressed.
    pub shift: bool,
    /// Meta is pressed.
    pub meta: bool,
}

/// A host-normalized keyboard event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyInput {
    /// Logical key value, such as `Enter` or a single space.
    pub key: DomString,
    /// Physical key code, such as `Enter` or `Space`.
    pub code: DomString,
    /// Whether this event is an automatic repeat.
    pub repeat: bool,
    /// KeyboardEvent location supplied by the host.
    pub location: u32,
    /// Modifiers at the time of input.
    pub modifiers: Modifiers,
}

/// Input selected by the host for delivery to a live document node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputEvent {
    /// Primary mouse click at viewport coordinates in CSS pixels.
    Click {
        /// Horizontal viewport coordinate.
        client_x: i32,
        /// Vertical viewport coordinate.
        client_y: i32,
        /// Modifiers at the time of input.
        modifiers: Modifiers,
    },
    /// A key press.
    KeyDown(KeyInput),
    /// A key release.
    KeyUp(KeyInput),
    /// An IME composition step at the focused control (a11y.editing).
    Composition(super::editing::Composition),
    /// A request from assistive technology (a11y.bridges, ADR 0073).
    Assistive(AssistiveAction),
}

/// What assistive technology asks a node to do; the platform bridge maps
/// UI Automation, NSAccessibility and AT-SPI actions to these.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssistiveAction {
    /// Focus the node (when it is a focusable area).
    Focus,
    /// The default action: a trusted `click` without coordinates, so links,
    /// buttons and checkboxes activate as with the keyboard.
    Click,
    /// Replace a text control's value as typing would.
    SetValue(DomString),
    /// Select UTF-16 offsets `anchor..focus` in a text control.
    SetSelection {
        /// Where the selection starts.
        anchor: u32,
        /// Where it ends (the caret).
        focus: u32,
    },
}

pub(in crate::page) fn connected(document: &Document, target: NodeId) -> bool {
    reaches_root(document, target, || Ok(())).expect("unmetered ancestry walk")
}

pub(in crate::page) fn reaches_root(
    document: &Document,
    target: NodeId,
    mut charge: impl FnMut() -> JsResult<()>,
) -> JsResult<bool> {
    let mut current = Some(target);
    while let Some(id) = current {
        charge()?;
        let Ok(node) = document.node(id) else {
            return Ok(false);
        };
        if id == document.root() {
            return Ok(true);
        }
        current = node.parent().or_else(|| node.shadow_host());
    }
    Ok(false)
}

fn live_target(agent: &mut Agent, id: NodeId) -> JsResult<Option<Value>> {
    with_page(agent, |agent, page| {
        if id.document() != page.main {
            return Ok(None);
        }
        page.read(agent, page.main, |agent, document| {
            if !reaches_root(document, id, || agent.check_native_work())? {
                return Ok(None);
            }
            let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
                return agent.type_error("DOM bindings are busy");
            };
            wrap(&mut wrappers, agent, document, id).map(Some)
        })
    })
}

pub(in crate::page) fn deliver(agent: &mut Agent, id: NodeId, input: &InputEvent) -> JsResult<()> {
    let released = match input {
        InputEvent::KeyUp(key) if is_key(key, " ") => {
            with_page(agent, |_, page| Ok(page.events.pressed_space.take()))?
        }
        InputEvent::KeyDown(key) if is_key(key, " ") && !key.repeat => {
            with_page(agent, |_, page| Ok(page.events.pressed_space.take()))?;
            None
        }
        _ => None,
    };
    // Inert nodes are skipped by hit testing: a click on content blocked
    // by a modal dialog lands on the dialog (its `::backdrop`).
    let (id, outside) = match input {
        InputEvent::Click { .. } => crate::page::elements::click_target(agent, id)?,
        _ => (id, false),
    };
    let Some(target) = live_target(agent, id)? else {
        return Ok(());
    };
    with_root(agent, target, |agent| match input {
        InputEvent::Click {
            client_x,
            client_y,
            modifiers,
        } => {
            // The press light-dismisses popovers and `closedby=any` dialogs
            // it falls outside of (HTML §6.12.6, §4.11.4).
            crate::page::elements::light_dismiss(agent, id, outside)?;
            if !disabled_click_target(agent, target)? {
                // A press focuses the clicked area before the click event
                // (a backdrop is not part of any focusable area).
                if !outside {
                    super::forms::focus_for_click(agent, target)?;
                }
                let event = ui::click_event(agent, Some((*client_x, *client_y)), *modifiers)?;
                fire_trusted(agent, target, event)?;
            }
            Ok(())
        }
        InputEvent::KeyDown(key) | InputEvent::KeyUp(key) => {
            let down = matches!(input, InputEvent::KeyDown(_));
            let event = ui::key_event(agent, if down { "keydown" } else { "keyup" }, key)?;
            if fire_trusted(agent, target, event)? {
                key_default(agent, id, target, key, down, released)?;
            }
            Ok(())
        }
        InputEvent::Composition(step) => super::editing::compose(agent, id, target, step),
        InputEvent::Assistive(action) => match action {
            AssistiveAction::Focus => super::forms::focus_element(agent, target).map(drop),
            AssistiveAction::Click => {
                if !disabled_click_target(agent, target)? {
                    let event = ui::click_event(agent, None, Modifiers::default())?;
                    fire_trusted(agent, target, event)?;
                }
                Ok(())
            }
            AssistiveAction::SetValue(text) => {
                super::editing::replace_value(agent, target, text.as_units()).map(drop)
            }
            AssistiveAction::SetSelection { anchor, focus } => {
                super::editing::select_range(agent, target, *anchor, *focus).map(drop)
            }
        },
    })
}

fn is_key(input: &KeyInput, value: &str) -> bool {
    input
        .key
        .as_units()
        .iter()
        .copied()
        .eq(value.encode_utf16())
}

fn key_default(
    agent: &mut Agent,
    id: NodeId,
    target: Value,
    key: &KeyInput,
    down: bool,
    released: Option<NodeId>,
) -> JsResult<()> {
    if down
        && (key.modifiers.control || key.modifiers.meta)
        && !key.modifiers.alt
        && (is_key(key, "a") || is_key(key, "A"))
    {
        super::editing::select_all(agent, target)?;
        return Ok(());
    }
    if key.modifiers.control || key.modifiers.alt || key.modifiers.meta {
        return Ok(());
    }
    // An uncanceled Escape is a close request for the topmost modal dialog
    // or auto popover (HTML §7.3 close watchers).
    if down && is_key(key, "Escape") {
        crate::page::elements::close_request(agent)?;
        return Ok(());
    }
    if down && is_key(key, "Tab") {
        return super::forms::navigate(agent, key.modifiers.shift);
    }
    // Text controls handle printable keys, Backspace and Enter.
    if down && super::forms::edit(agent, target, key)? {
        return Ok(());
    }
    let space = is_key(key, " ");
    let enter = down && is_key(key, "Enter");
    if !(enter || space && (down && !key.repeat || released == Some(id))) {
        return Ok(());
    }

    // Listeners can remove, adopt, change the type of, or disable the target.
    let control = read_node(agent, target, |agent, _, document, current, node| {
        if current != id || !reaches_root(document, id, || agent.check_native_work())? {
            return Ok(None);
        }
        enabled_control(agent, document, id, node)
    })?;
    match (space, down, control) {
        (true, true, Some(Control::Button | Control::Checkbox)) => {
            with_page(agent, |_, page| {
                page.events.pressed_space.set(Some(id));
                Ok(())
            })?;
        }
        (false, true, Some(Control::Button))
        | (true, false, Some(Control::Button | Control::Checkbox)) => {
            let event = ui::click_event(agent, None, key.modifiers)?;
            fire_trusted(agent, target, event)?;
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn fire_trusted(agent: &mut Agent, target: Value, event: Value) -> JsResult<bool> {
    with_root(agent, event, |agent| {
        let state = Event::receiver(agent, event)?;
        fire(
            agent,
            Target {
                value: target,
                node: true,
            },
            event,
            state,
        )
    })
}

pub(super) fn html_element(node: &Node) -> Option<&Element> {
    let NodeKind::Element(element) = node.kind() else {
        return None;
    };
    (element.name().namespace() == Some(HTML_NAMESPACE)).then_some(element)
}

pub(super) fn attribute<'a>(element: &'a Element, local: &str) -> Option<&'a DomString> {
    element.attributes().iter().find_map(|attribute| {
        let name = attribute.name();
        (name.namespace().is_none() && name.local() == local).then(|| attribute.value())
    })
}

#[derive(Clone, Copy)]
enum Control {
    Button,
    Checkbox,
    OtherInput,
}

impl Control {
    fn of(node: &Node) -> Option<Self> {
        let element = html_element(node)?;
        match element.name().local() {
            "button" => Some(Self::Button),
            "input" => {
                let checkbox = attribute(element, "type").is_some_and(|value| {
                    let units = value.as_units();
                    units.len() == 8
                        && units.iter().zip(b"checkbox").all(|(&unit, &byte)| {
                            u8::try_from(unit).is_ok_and(|value| value.eq_ignore_ascii_case(&byte))
                        })
                });
                Some(if checkbox {
                    Self::Checkbox
                } else {
                    Self::OtherInput
                })
            }
            _ => None,
        }
    }
}

fn enabled_control(
    agent: &mut Agent,
    document: &Document,
    id: NodeId,
    node: &Node,
) -> JsResult<Option<Control>> {
    let control = Control::of(node);
    if control.is_some() && disabled(agent, document, id, node)? {
        return Ok(None);
    }
    Ok(control)
}

pub(super) fn disabled(
    agent: &mut Agent,
    document: &Document,
    id: NodeId,
    node: &Node,
) -> JsResult<bool> {
    if html_element(node).is_some_and(|element| attribute(element, "disabled").is_some()) {
        return Ok(true);
    }
    let mut child = id;
    let mut next = node.parent();
    while let Some(parent) = next {
        agent.check_native_work()?;
        let node = document
            .node(parent)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        if html_element(node).is_some_and(|element| {
            element.name().local() == "fieldset" && attribute(element, "disabled").is_some()
        }) && first_legend(agent, document, parent)? != Some(child)
        {
            return Ok(true);
        }
        child = parent;
        next = node.parent();
    }
    Ok(false)
}

fn first_legend(
    agent: &mut Agent,
    document: &Document,
    fieldset: NodeId,
) -> JsResult<Option<NodeId>> {
    let children = document
        .children(fieldset)
        .map_err(|error| dom_exception::from_dom_error(agent, error))?;
    for child in children {
        agent.check_native_work()?;
        if document
            .node(child)
            .ok()
            .and_then(html_element)
            .is_some_and(|element| element.name().local() == "legend")
        {
            return Ok(Some(child));
        }
    }
    Ok(None)
}

fn disabled_click_target(agent: &mut Agent, target: Value) -> JsResult<bool> {
    read_node(agent, target, |agent, _, document, mut id, node| {
        let mut node = node;
        loop {
            agent.check_native_work()?;
            if Control::of(node).is_some() {
                return disabled(agent, document, id, node);
            }
            let Some(parent) = node.parent() else {
                return Ok(false);
            };
            id = parent;
            node = document
                .node(id)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        }
    })
}

pub(super) fn activation_target(
    agent: &mut Agent,
    path: PrivateArray,
    bubbles: bool,
) -> JsResult<Option<Value>> {
    let window = with_page(agent, |_, page| Ok(page.window))?;
    let len = if bubbles { path.len(agent)? } else { 1 };
    for index in 0..len {
        agent.check_native_work()?;
        let value = path.get(agent, index)?;
        if value == window {
            break;
        }
        let control = read_node(
            agent,
            value,
            |agent, _, document, id, node| match Control::of(node) {
                Some(Control::Checkbox) if disabled(agent, document, id, node)? => {
                    Ok(Some(Control::OtherInput))
                }
                other => Ok(other),
            },
        )?;
        match control {
            Some(Control::Checkbox) => return Ok(Some(value)),
            Some(_) => break,
            None => {}
        }
    }
    Ok(None)
}

const DIRTY_CHECKED: u32 = 0;
const INDETERMINATE: u32 = 1;

pub(in crate::page) fn install_controls(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        "HTMLInputElement",
        Some("HTMLElement"),
        crate::page::custom_elements::builtins::constructor("HTMLInputElement"),
        0,
        &[],
        |agent, proto| {
            for (name, getter_name, getter, setter_name, setter) in [
                (
                    "checked",
                    "get checked",
                    checked as NativeFn,
                    "set checked",
                    set_checked as NativeFn,
                ),
                (
                    "defaultChecked",
                    "get defaultChecked",
                    default_checked,
                    "set defaultChecked",
                    set_default_checked,
                ),
                (
                    "indeterminate",
                    "get indeterminate",
                    indeterminate,
                    "set indeterminate",
                    set_indeterminate,
                ),
            ] {
                let get = Value::object(agent.new_native_function(getter, getter_name, 0)?);
                let set = Value::object(agent.new_native_function(setter, setter_name, 1)?);
                let key = agent.heap.key(name)?;
                agent.define_property_or_throw(
                    proto,
                    key,
                    PropertyDescriptor::accessor(get, set, true, true),
                )?;
            }
            Ok(())
        },
    )
}

fn content_checked(agent: &mut Agent, target: Value) -> JsResult<bool> {
    read_node(agent, target, |agent, _, _, _, node| {
        let Some(element) = html_element(node).filter(|element| element.name().local() == "input")
        else {
            return agent.type_error("expected HTMLInputElement");
        };
        Ok(attribute(element, "checked").is_some())
    })
}

fn record(agent: &mut Agent, target: Value) -> JsResult<Option<PrivateArray>> {
    with_page(agent, |agent, page| {
        Ok(PrivateArray::from_value(
            agent.weak_map_get(page.events.controls, target),
        ))
    })
}

fn ensure_record(agent: &mut Agent, target: Value) -> JsResult<PrivateArray> {
    if let Some(record) = record(agent, target)? {
        return Ok(record);
    }
    let record = PrivateArray::new(agent, &[Value::UNDEFINED, Value::FALSE])?;
    with_page(agent, |agent, page| {
        agent.weak_map_set(page.events.controls, target, record.value())
    })?;
    Ok(record)
}

fn checked(agent: &mut Agent, target: Value, _: &[Value]) -> JsResult<Value> {
    let default = content_checked(agent, target)?;
    if let Some(record) = record(agent, target)? {
        let checked = record.get(agent, DIRTY_CHECKED)?;
        if checked != Value::UNDEFINED {
            return Ok(checked);
        }
    }
    Ok(Value::boolean(default))
}

/// The checkedness of a checkbox or radio input.
pub(super) fn checked_state(agent: &mut Agent, target: Value) -> JsResult<bool> {
    checked(agent, target, &[]).map(|value| value == Value::TRUE)
}

/// Form reset: restore the default checkedness and clear indeterminate.
pub(super) fn reset_checkedness(agent: &mut Agent, target: Value) -> JsResult<()> {
    if let Some(record) = record(agent, target)? {
        record.set(agent, DIRTY_CHECKED, Value::UNDEFINED)?;
    }
    Ok(())
}

fn default_checked(agent: &mut Agent, target: Value, _: &[Value]) -> JsResult<Value> {
    content_checked(agent, target).map(Value::boolean)
}

/// Whether a checkbox's indeterminate IDL state is set.
pub(super) fn indeterminate_state(agent: &mut Agent, target: Value) -> JsResult<bool> {
    Ok(match record(agent, target)? {
        Some(record) => record.get(agent, INDETERMINATE)? == Value::TRUE,
        None => false,
    })
}

fn indeterminate(agent: &mut Agent, target: Value, _: &[Value]) -> JsResult<Value> {
    content_checked(agent, target)?;
    match record(agent, target)? {
        Some(record) => record.get(agent, INDETERMINATE),
        None => Ok(Value::FALSE),
    }
}

fn set_checked(agent: &mut Agent, target: Value, args: &[Value]) -> JsResult<Value> {
    set_state(agent, target, DIRTY_CHECKED, args)
}

fn set_indeterminate(agent: &mut Agent, target: Value, args: &[Value]) -> JsResult<Value> {
    set_state(agent, target, INDETERMINATE, args)
}

fn set_state(agent: &mut Agent, target: Value, slot: u32, args: &[Value]) -> JsResult<Value> {
    content_checked(agent, target)?;
    let value = argument_bool(agent, args);
    ensure_record(agent, target)?.set(agent, slot, Value::boolean(value))?;
    Ok(Value::UNDEFINED)
}

fn argument_bool(agent: &Agent, args: &[Value]) -> bool {
    agent.to_boolean(args.first().copied().unwrap_or(Value::UNDEFINED))
}

fn set_default_checked(agent: &mut Agent, target: Value, args: &[Value]) -> JsResult<Value> {
    let current = content_checked(agent, target)?;
    let value = argument_bool(agent, args);
    if current == value {
        return Ok(Value::UNDEFINED);
    }
    with_mut_tree(agent, target, |agent, document, _, id| {
        if value {
            let name = QualifiedName::from_parsed(None, None, "checked").expect("static name");
            document
                .set_attribute(id, name, DomString::default())
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        } else {
            document
                .remove_attribute(id, None, "checked")
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        }
        Ok(Value::UNDEFINED)
    })
}

pub(super) struct Preactivation {
    record: PrivateArray,
    checked: bool,
    dirty: bool,
    indeterminate: Value,
}

pub(super) fn preactivate(agent: &mut Agent, target: Value) -> JsResult<Preactivation> {
    let default = content_checked(agent, target)?;
    let record = ensure_record(agent, target)?;
    let dirty = record.get(agent, DIRTY_CHECKED)?;
    let checked = if dirty == Value::UNDEFINED {
        default
    } else {
        dirty == Value::TRUE
    };
    let indeterminate = record.get(agent, INDETERMINATE)?;
    record.set(agent, DIRTY_CHECKED, Value::boolean(!checked))?;
    record.set(agent, INDETERMINATE, Value::FALSE)?;
    Ok(Preactivation {
        record,
        checked,
        dirty: dirty != Value::UNDEFINED,
        indeterminate,
    })
}

pub(super) fn finish_activation(
    agent: &mut Agent,
    target: Value,
    previous: Preactivation,
    canceled: bool,
) -> JsResult<()> {
    if canceled {
        let still_checkbox = read_node(agent, target, |_, _, _, _, node| {
            Ok(matches!(Control::of(node), Some(Control::Checkbox)))
        })?;
        if !still_checkbox {
            return Ok(());
        }
        let checked = if previous.dirty || content_checked(agent, target)? != previous.checked {
            Value::boolean(previous.checked)
        } else {
            Value::UNDEFINED
        };
        previous.record.set(agent, DIRTY_CHECKED, checked)?;
        previous
            .record
            .set(agent, INDETERMINATE, previous.indeterminate)?;
        return Ok(());
    }

    let connected_checkbox = read_node(agent, target, |agent, _, document, id, node| {
        Ok(matches!(Control::of(node), Some(Control::Checkbox))
            && reaches_root(document, id, || agent.check_native_work())?)
    })?;
    if connected_checkbox {
        for (kind, flags) in [
            ("input", Event::BUBBLES | Event::COMPOSED),
            ("change", Event::BUBBLES),
        ] {
            let event =
                state::trusted_event(agent, "Event", kind, flags, Value::NULL, Value::NULL)?;
            fire_trusted(agent, target, event)?;
        }
    }
    Ok(())
}
