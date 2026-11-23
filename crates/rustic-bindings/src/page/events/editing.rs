//! Caret, selection and IME composition in text controls (M8
//! `a11y.editing`, ADR 0070).
//!
//! Each text control keeps a selection (start, end, direction) in UTF-16
//! code units beside its value. A control whose selection was never set
//! has its caret at the end of the value, so typing into a fresh field
//! appends as it did before carets existed. Keys move the caret by code
//! point (never splitting a surrogate pair), Shift extends the selection,
//! edits replace the selection, and the HTML selection APIs read and write
//! the same state. Composition follows UI Events: `compositionstart`,
//! `compositionupdate` and `compositionend` at the focused control, with
//! the composed text replacing the composition range through
//! `insertCompositionText` input events.

use rustic_dom::{DomString, NodeId};
use rustic_js::runtime::{Agent, JsResult, OutOfMemory, Value};

use super::super::with_page;
use super::forms::{
    Kind, control_receiver, current_value, editable, ensure_record, fire_focus,
    implicit_submission, input_event, new_event, record, snapshot, store_value,
};
use super::{KeyInput, with_root};
use crate::webidl::{DomStringConversion, IntegerConversion, to_dom_string, to_unsigned_long};

/// Record slots after the form-control ones: selection start, end, and
/// whether the direction is backward.
pub(super) const SEL_START: u32 = 5;
const SEL_END: u32 = 6;
const SEL_BACKWARD: u32 = 7;

/// A host-delivered IME composition step.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Composition {
    /// The IME started composing.
    Start,
    /// The composition text is now this.
    Update(DomString),
    /// The IME committed this text and stopped composing.
    End(DomString),
}

/// Controls the HTML selection APIs apply to.
fn selection_applies(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::TextArea | Kind::Input("text" | "search" | "tel" | "url" | "password")
    )
}

/// A selection in UTF-16 code units, `start <= end`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Selection {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) backward: bool,
}

impl Selection {
    fn caret(at: usize) -> Self {
        Selection {
            start: at,
            end: at,
            backward: false,
        }
    }

    fn anchor(self) -> usize {
        if self.backward { self.end } else { self.start }
    }

    fn focus(self) -> usize {
        if self.backward { self.start } else { self.end }
    }

    fn between(anchor: usize, focus: usize) -> Self {
        Selection {
            start: anchor.min(focus),
            end: anchor.max(focus),
            backward: focus < anchor,
        }
    }
}

/// The control's selection clamped to a value of `len` units.
pub(super) fn selection(agent: &mut Agent, control: Value, len: usize) -> JsResult<Selection> {
    let Some(record) = record(agent, control)? else {
        return Ok(Selection::caret(len));
    };
    if record.len(agent)? <= SEL_BACKWARD {
        return Ok(Selection::caret(len));
    }
    let start = record.get(agent, SEL_START)?.as_number();
    let end = record.get(agent, SEL_END)?.as_number();
    let backward = record.get(agent, SEL_BACKWARD)? == Value::TRUE;
    Ok(match (start, end) {
        (Some(start), Some(end)) => {
            let clamp = |v: f64| (v.max(0.0) as usize).min(len);
            let (start, end) = (clamp(start), clamp(end));
            Selection {
                start: start.min(end),
                end,
                backward,
            }
        }
        _ => Selection::caret(len),
    })
}

pub(super) fn set_selection(
    agent: &mut Agent,
    control: Value,
    selection: Selection,
) -> JsResult<()> {
    let record = ensure_record(agent, control)?;
    record.set(agent, SEL_START, Value::number(selection.start as f64))?;
    record.set(agent, SEL_END, Value::number(selection.end as f64))?;
    record.set(agent, SEL_BACKWARD, Value::boolean(selection.backward))
}

/// Forget the selection so the caret follows the end of the value (after a
/// script sets `value`, as HTML moves the cursor to the end).
pub(super) fn reset_selection(agent: &mut Agent, control: Value) -> JsResult<()> {
    if let Some(record) = record(agent, control)? {
        if record.len(agent)? > SEL_BACKWARD {
            record.set(agent, SEL_START, Value::UNDEFINED)?;
            record.set(agent, SEL_END, Value::UNDEFINED)?;
        }
    }
    Ok(())
}

fn is_high(unit: u16) -> bool {
    (0xd800..0xdc00).contains(&unit)
}

fn is_low(unit: u16) -> bool {
    (0xdc00..0xe000).contains(&unit)
}

fn previous(value: &[u16], at: usize) -> usize {
    match at {
        0 => 0,
        1 => 0,
        _ if is_low(value[at - 1]) && is_high(value[at - 2]) => at - 2,
        _ => at - 1,
    }
}

fn next(value: &[u16], at: usize) -> usize {
    if at >= value.len() {
        value.len()
    } else if at + 1 < value.len() && is_high(value[at]) && is_low(value[at + 1]) {
        at + 2
    } else {
        at + 1
    }
}

fn line_start(value: &[u16], at: usize) -> usize {
    value[..at]
        .iter()
        .rposition(|&unit| unit == 0x0a)
        .map_or(0, |i| i + 1)
}

fn line_end(value: &[u16], at: usize) -> usize {
    value[at..]
        .iter()
        .position(|&unit| unit == 0x0a)
        .map_or(value.len(), |i| at + i)
}

fn named(key: &[u16], name: &str) -> bool {
    key.iter().copied().eq(name.encode_utf16())
}

/// Select the whole value (Control/Command+A and `select()`).
pub(super) fn select_all(agent: &mut Agent, control: Value) -> JsResult<bool> {
    let Some(snapshot) = snapshot(agent, control)? else {
        return Ok(false);
    };
    if !editable(snapshot.kind) {
        return Ok(false);
    }
    set_selection(agent, control, Selection::between(0, snapshot.value.len()))?;
    Ok(true)
}

/// The editing default action of a key press on a text control. Returns
/// whether the key was handled.
pub(super) fn edit_key(agent: &mut Agent, control: Value, key: &KeyInput) -> JsResult<bool> {
    let Some(snapshot) = snapshot(agent, control)? else {
        return Ok(false);
    };
    if !editable(snapshot.kind) {
        return Ok(false);
    }
    let name = key.key.as_units();
    let value = &snapshot.value;
    let selection = selection(agent, control, value.len())?;
    let multiline = snapshot.kind == Kind::TextArea;

    // Caret movement works in read-only and disabled-for-edit fields too.
    let moved = if named(name, "ArrowLeft") {
        Some(
            if !key.modifiers.shift && selection.start != selection.end {
                selection.start
            } else {
                previous(value, selection.focus())
            },
        )
    } else if named(name, "ArrowRight") {
        Some(
            if !key.modifiers.shift && selection.start != selection.end {
                selection.end
            } else {
                next(value, selection.focus())
            },
        )
    } else if named(name, "Home") || (!multiline && named(name, "ArrowUp")) {
        Some(line_start(value, selection.focus()))
    } else if named(name, "End") || (!multiline && named(name, "ArrowDown")) {
        Some(line_end(value, selection.focus()))
    } else {
        None
    };
    if let Some(focus) = moved {
        let next = if key.modifiers.shift {
            Selection::between(selection.anchor(), focus)
        } else {
            Selection::caret(focus)
        };
        set_selection(agent, control, next)?;
        return Ok(true);
    }

    let enter = named(name, "Enter");
    let backspace = named(name, "Backspace");
    let delete = named(name, "Delete");
    let printable = !enter
        && !backspace
        && !delete
        && String::from_utf16(name).is_ok_and(|text| text.chars().count() == 1);
    if enter && !multiline {
        implicit_submission(agent, control, snapshot.owner)?;
        return Ok(true);
    }
    if !(enter || backspace || delete || printable) || snapshot.disabled || snapshot.readonly {
        return Ok(false);
    }
    let (input_type, data): (&str, Option<Vec<u16>>) = if backspace {
        ("deleteContentBackward", None)
    } else if delete {
        ("deleteContentForward", None)
    } else if enter {
        ("insertLineBreak", None)
    } else {
        ("insertText", Some(name.to_vec()))
    };
    let inserted: Vec<u16> = if enter {
        vec![0x0a]
    } else if printable {
        name.to_vec()
    } else {
        Vec::new()
    };
    let range = |value: &[u16], selection: Selection| -> (usize, usize) {
        if selection.start != selection.end || printable || enter {
            (selection.start, selection.end)
        } else if backspace {
            (previous(value, selection.start), selection.start)
        } else {
            (selection.start, next(value, selection.start))
        }
    };
    let (from, to) = range(value, selection);
    if from == to && inserted.is_empty() {
        // Nothing to delete: no input events.
        return Ok(true);
    }
    if snapshot
        .max_length
        .is_some_and(|max| value.len() - (to - from) + inserted.len() > max)
        && !inserted.is_empty()
    {
        return Ok(true);
    }
    let before = input_event(agent, "beforeinput", input_type, data.as_deref(), false)?;
    if !fire_focus(agent, control, before)? {
        return Ok(true);
    }
    // Listeners may have changed the value or the selection.
    let mut value = current_value(agent, control)?;
    let selection = self::selection(agent, control, value.len())?;
    let (from, to) = range(&value, selection);
    value.try_reserve(inserted.len()).map_err(|_| OutOfMemory)?;
    value.splice(from..to, inserted.iter().copied());
    store_value(agent, control, &value, true)?;
    set_selection(agent, control, Selection::caret(from + inserted.len()))?;
    let after = input_event(agent, "input", input_type, data.as_deref(), false)?;
    fire_focus(agent, control, after)?;
    Ok(true)
}

/// Replace a text control's value as the user would (an assistive
/// technology's set-value action): cancelable `beforeinput` with
/// `insertReplacementText`, the change, `input`, caret at the end.
pub(super) fn replace_value(agent: &mut Agent, control: Value, text: &[u16]) -> JsResult<bool> {
    let Some(snapshot) = snapshot(agent, control)? else {
        return Ok(false);
    };
    if !editable(snapshot.kind) || snapshot.disabled || snapshot.readonly {
        return Ok(false);
    }
    let before = input_event(
        agent,
        "beforeinput",
        "insertReplacementText",
        Some(text),
        false,
    )?;
    if !fire_focus(agent, control, before)? {
        return Ok(true);
    }
    let mut units = text.to_vec();
    if snapshot.kind != Kind::TextArea {
        units.retain(|&unit| unit != 0x0a && unit != 0x0d);
    }
    if let Some(max) = snapshot.max_length {
        units.truncate(max);
    }
    store_value(agent, control, &units, true)?;
    set_selection(agent, control, Selection::caret(units.len()))?;
    let after = input_event(agent, "input", "insertReplacementText", Some(text), false)?;
    fire_focus(agent, control, after)?;
    Ok(true)
}

/// Set the selection from an assistive technology: anchor and focus in
/// UTF-16 units, clamped.
pub(super) fn select_range(
    agent: &mut Agent,
    control: Value,
    anchor: u32,
    focus: u32,
) -> JsResult<bool> {
    let Some(snapshot) = snapshot(agent, control)? else {
        return Ok(false);
    };
    if !editable(snapshot.kind) {
        return Ok(false);
    }
    let len = snapshot.value.len();
    let clamp = |v: u32| (v as usize).min(len);
    set_selection(
        agent,
        control,
        Selection::between(clamp(anchor), clamp(focus)),
    )?;
    Ok(true)
}

// ---- composition -------------------------------------------------------------

fn composition_event(agent: &mut Agent, kind: &str, data: &[u16]) -> JsResult<Value> {
    let data = crate::webidl::from_dom_units(agent, data)?;
    with_root(agent, data, |agent| {
        let cancelable = if kind == "compositionstart" {
            super::state::Event::CANCELABLE
        } else {
            0
        };
        new_event(
            agent,
            "CompositionEvent",
            kind,
            super::state::Event::BUBBLES | super::state::Event::COMPOSED | cancelable,
            &[data],
        )
    })
}

fn composing(agent: &mut Agent) -> JsResult<Option<(NodeId, usize, usize)>> {
    with_page(agent, |_, page| Ok(page.events.forms.composition.get()))
}

fn set_composing(agent: &mut Agent, state: Option<(NodeId, usize, usize)>) -> JsResult<()> {
    with_page(agent, |_, page| {
        page.events.forms.composition.set(state);
        Ok(())
    })
}

/// Start composing at `target`; returns whether it was not canceled.
fn start(agent: &mut Agent, id: NodeId, target: Value) -> JsResult<bool> {
    let (selected, selection) = match snapshot(agent, target)? {
        Some(snapshot) if editable(snapshot.kind) => {
            let selection = selection(agent, target, snapshot.value.len())?;
            (
                snapshot.value[selection.start..selection.end].to_vec(),
                Some(selection),
            )
        }
        _ => (Vec::new(), None),
    };
    let event = composition_event(agent, "compositionstart", &selected)?;
    if !super::input::fire_trusted(agent, target, event)? {
        set_composing(agent, None)?;
        return Ok(false);
    }
    let (from, len) = selection.map_or((0, 0), |s| (s.start, s.end - s.start));
    set_composing(agent, Some((id, from, len)))?;
    Ok(true)
}

fn update(agent: &mut Agent, id: NodeId, target: Value, text: &[u16]) -> JsResult<()> {
    let event = composition_event(agent, "compositionupdate", text)?;
    super::input::fire_trusted(agent, target, event)?;
    let Some((_, from, len)) = composing(agent)?.filter(|state| state.0 == id) else {
        return Ok(());
    };
    let Some(snapshot) = snapshot(agent, target)? else {
        return Ok(());
    };
    if !editable(snapshot.kind) || snapshot.disabled || snapshot.readonly {
        return Ok(());
    }
    let before = input_event(
        agent,
        "beforeinput",
        "insertCompositionText",
        Some(text),
        true,
    )?;
    super::input::fire_trusted(agent, target, before)?;
    let mut value = current_value(agent, target)?;
    let from = from.min(value.len());
    let to = (from + len).min(value.len());
    value.try_reserve(text.len()).map_err(|_| OutOfMemory)?;
    value.splice(from..to, text.iter().copied());
    store_value(agent, target, &value, true)?;
    set_selection(agent, target, Selection::caret(from + text.len()))?;
    set_composing(agent, Some((id, from, text.len())))?;
    let after = input_event(agent, "input", "insertCompositionText", Some(text), true)?;
    super::input::fire_trusted(agent, target, after)?;
    Ok(())
}

/// Deliver one composition step to `target`.
pub(super) fn compose(
    agent: &mut Agent,
    id: NodeId,
    target: Value,
    step: &Composition,
) -> JsResult<()> {
    let active = composing(agent)?.is_some_and(|state| state.0 == id);
    match step {
        Composition::Start => {
            start(agent, id, target)?;
        }
        Composition::Update(text) => {
            if active || start(agent, id, target)? {
                update(agent, id, target, text.as_units())?;
            }
        }
        Composition::End(text) => {
            if !active && !start(agent, id, target)? {
                return Ok(());
            }
            let current = match (composing(agent)?, snapshot(agent, target)?) {
                (Some((_, from, len)), Some(snapshot)) if editable(snapshot.kind) => {
                    snapshot.value.get(from..from + len).map(<[u16]>::to_vec)
                }
                _ => None,
            };
            if current.as_deref() != Some(text.as_units()) {
                update(agent, id, target, text.as_units())?;
            }
            set_composing(agent, None)?;
            let event = composition_event(agent, "compositionend", text.as_units())?;
            super::input::fire_trusted(agent, target, event)?;
        }
    }
    Ok(())
}

// ---- the HTML selection API --------------------------------------------------

fn applies(agent: &mut Agent, this: Value) -> JsResult<bool> {
    Ok(selection_applies(control_receiver(agent, this)?))
}

fn invalid_state(agent: &mut Agent) -> JsResult<Value> {
    let error = crate::dom_exception::new(
        agent,
        "InvalidStateError",
        "the selection API does not apply to this control",
    )?;
    Err(rustic_js::runtime::Abrupt::Throw(error))
}

fn current_selection(agent: &mut Agent, this: Value) -> JsResult<(Vec<u16>, Selection)> {
    let value = current_value(agent, this)?;
    let selection = selection(agent, this, value.len())?;
    Ok((value, selection))
}

pub(super) fn selection_start(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    if !applies(agent, this)? {
        return Ok(Value::NULL);
    }
    let (_, selection) = current_selection(agent, this)?;
    Ok(Value::number(selection.start as f64))
}

pub(super) fn selection_end(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    if !applies(agent, this)? {
        return Ok(Value::NULL);
    }
    let (_, selection) = current_selection(agent, this)?;
    Ok(Value::number(selection.end as f64))
}

pub(super) fn selection_direction(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    if !applies(agent, this)? {
        return Ok(Value::NULL);
    }
    let (_, selection) = current_selection(agent, this)?;
    agent.string(if selection.backward {
        "backward"
    } else {
        "forward"
    })
}

fn offset(agent: &mut Agent, value: Value) -> JsResult<usize> {
    Ok(to_unsigned_long(agent, value, IntegerConversion::Wrap)? as usize)
}

fn direction(agent: &mut Agent, value: Option<Value>) -> JsResult<bool> {
    match value {
        None | Some(Value::UNDEFINED) => Ok(false),
        Some(value) => {
            let text = to_dom_string(agent, value, DomStringConversion::Default)?;
            Ok(text
                .as_units()
                .iter()
                .copied()
                .eq("backward".encode_utf16()))
        }
    }
}

fn apply_range(
    agent: &mut Agent,
    this: Value,
    start: usize,
    end: usize,
    backward: bool,
) -> JsResult<Value> {
    let len = current_value(agent, this)?.len();
    let end = end.min(len);
    let start = start.min(end);
    set_selection(
        agent,
        this,
        Selection {
            start,
            end,
            backward,
        },
    )?;
    Ok(Value::UNDEFINED)
}

pub(super) fn set_selection_start(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
) -> JsResult<Value> {
    if !applies(agent, this)? {
        return invalid_state(agent);
    }
    let start = offset(agent, args.first().copied().unwrap_or(Value::UNDEFINED))?;
    let (_, selection) = current_selection(agent, this)?;
    apply_range(
        agent,
        this,
        start,
        selection.end.max(start),
        selection.backward,
    )
}

pub(super) fn set_selection_end(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    if !applies(agent, this)? {
        return invalid_state(agent);
    }
    let end = offset(agent, args.first().copied().unwrap_or(Value::UNDEFINED))?;
    let (_, selection) = current_selection(agent, this)?;
    apply_range(
        agent,
        this,
        selection.start.min(end),
        end,
        selection.backward,
    )
}

pub(super) fn set_selection_direction(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
) -> JsResult<Value> {
    if !applies(agent, this)? {
        return invalid_state(agent);
    }
    let backward = direction(agent, args.first().copied())?;
    let (_, selection) = current_selection(agent, this)?;
    apply_range(agent, this, selection.start, selection.end, backward)
}

pub(super) fn set_selection_range(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
) -> JsResult<Value> {
    if !applies(agent, this)? {
        return invalid_state(agent);
    }
    let start = offset(agent, args.first().copied().unwrap_or(Value::UNDEFINED))?;
    let end = offset(agent, args.get(1).copied().unwrap_or(Value::UNDEFINED))?;
    let backward = direction(agent, args.get(2).copied())?;
    apply_range(agent, this, start, end, backward)
}

pub(super) fn select_method(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    if applies(agent, this)? {
        let len = current_value(agent, this)?.len();
        set_selection(agent, this, Selection::between(0, len))?;
    }
    Ok(Value::UNDEFINED)
}

/// `setRangeText(replacement[, start, end[, selectionMode]])`.
pub(super) fn set_range_text(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    if !applies(agent, this)? {
        return invalid_state(agent);
    }
    let Some(&replacement) = args.first() else {
        return agent.type_error("setRangeText requires a replacement");
    };
    let replacement = to_dom_string(agent, replacement, DomStringConversion::Default)?;
    let (mut value, selection) = current_selection(agent, this)?;
    let (start, end) = if args.len() >= 3 {
        let start = offset(agent, args[1])?;
        let end = offset(agent, args[2])?;
        if start > end {
            let error = crate::dom_exception::new(
                agent,
                "IndexSizeError",
                "the start offset is after the end offset",
            )?;
            return Err(rustic_js::runtime::Abrupt::Throw(error));
        }
        (start.min(value.len()), end.min(value.len()))
    } else {
        (selection.start, selection.end)
    };
    let mode = match args.get(3).copied() {
        None | Some(Value::UNDEFINED) => String::from("preserve"),
        Some(mode) => String::from_utf16_lossy(
            to_dom_string(agent, mode, DomStringConversion::Default)?.as_units(),
        ),
    };
    let units = replacement.as_units();
    value.try_reserve(units.len()).map_err(|_| OutOfMemory)?;
    value.splice(start..end, units.iter().copied());
    store_value(agent, this, &value, false)?;
    let new_end = start + units.len();
    let removed = end - start;
    let next = match mode.as_str() {
        "select" => Selection::between(start, new_end),
        "start" => Selection::caret(start),
        "end" => Selection::caret(new_end),
        _ => {
            // "preserve": offsets after the range shift by the length
            // change; offsets inside it snap to its edges.
            let shift = |offset: usize| offset - removed + units.len();
            let s = if selection.start > end {
                shift(selection.start)
            } else if selection.start > start {
                start
            } else {
                selection.start
            };
            let e = if selection.end > end {
                shift(selection.end)
            } else if selection.end > start {
                new_end
            } else {
                selection.end
            };
            Selection {
                start: s.min(e),
                end: e,
                backward: selection.backward,
            }
        }
    };
    set_selection(agent, this, next)?;
    Ok(Value::UNDEFINED)
}
