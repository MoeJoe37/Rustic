//! Focus, text controls, form owners, constraint validation and form
//! submission without navigation ([HTML §6.6], [HTML §4.10]).
//!
//! The focused element is one document-local identity. Text control values
//! and custom validity messages live in private records keyed by the control
//! wrapper, beside the checkbox records of `input.rs`. A submission that
//! passes validation and is not canceled becomes a [`FormSubmission`] the
//! host takes; nothing navigates.
//!
//! [HTML §6.6]: https://html.spec.whatwg.org/multipage/interaction.html#focus
//! [HTML §4.10]: https://html.spec.whatwg.org/multipage/forms.html

use std::cell::{Cell, RefCell};

use rustic_dom::{Document, DomString, Element, Node, NodeId, NodeKind, QualifiedName};
use rustic_js::runtime::{
    Agent, JsResult, LiveIndexAccess, NativeFn, ObjectId, OutOfMemory, PropertyDescriptor, Value,
};

use crate::dom_exception;
use crate::webidl::{DomStringConversion, from_dom_string, to_dom_string};

use super::super::{
    illegal_constructor, install_interface, prototype, read_node, required_arg, with_mut_tree,
    with_page, wrap,
};
use super::input::{attribute, disabled, html_element, reaches_root};
use super::state::{self, Event};
use super::{PrivateArray, Target, define_method, fire, option, with_root};

/// Submissions kept for the host before further ones are counted as dropped.
pub const SUBMISSION_CAPACITY: usize = 64;

/// A form submission that passed validation and was not canceled. Rustic has
/// no navigation yet, so the host receives the entry list instead.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormSubmission {
    /// The submitted form.
    pub form: NodeId,
    /// The button that submitted it, if any.
    pub submitter: Option<NodeId>,
    /// `get`, `post` or `dialog`, from the submitter's or form's method.
    pub method: String,
    /// The `action` content attribute, unresolved; empty when absent.
    pub action: String,
    /// The constructed entry list, in tree order.
    pub entries: Vec<(String, String)>,
}

/// Submissions taken from the page, and how many did not fit.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FormSubmissions {
    /// Submissions in order.
    pub submissions: Vec<FormSubmission>,
    /// Submissions discarded because [`SUBMISSION_CAPACITY`] were waiting.
    pub dropped: u64,
}

/// Page-lifetime form and focus state.
#[derive(Debug)]
pub(in crate::page) struct Forms {
    values: ObjectId,
    validity: ObjectId,
    events: ObjectId,
    elements: ObjectId,
    focused: Cell<Option<NodeId>>,
    submissions: RefCell<FormSubmissions>,
}

/// Control record slots: dirty value (string or undefined), custom validity
/// message, user-edited flag, value when focused (or undefined), cached
/// ValidityState (or undefined).
const VALUE: u32 = 0;
const CUSTOM: u32 = 1;
const EDITED: u32 = 2;
const FOCUS_VALUE: u32 = 3;
const VALIDITY: u32 = 4;

impl Forms {
    /// The focused element, if any (it may since have been removed).
    pub(in crate::page) fn focused(&self) -> Option<NodeId> {
        self.focused.get()
    }

    pub(super) fn new(agent: &mut Agent) -> JsResult<(Self, [Value; 4])> {
        let values = agent.new_weak_map()?;
        let validity = agent.new_weak_map()?;
        let events = agent.new_weak_map()?;
        let elements = agent.new_weak_map()?;
        Ok((
            Self {
                values,
                validity,
                events,
                elements,
                focused: Cell::new(None),
                submissions: RefCell::new(FormSubmissions::default()),
            },
            [values, validity, events, elements].map(Value::object),
        ))
    }

    pub(in crate::page) fn take_submissions(&self) -> FormSubmissions {
        self.submissions
            .try_borrow_mut()
            .map(|mut log| std::mem::take(&mut *log))
            .unwrap_or_default()
    }

    fn record(&self, submission: FormSubmission) {
        let Ok(mut log) = self.submissions.try_borrow_mut() else {
            return;
        };
        if log.submissions.len() >= SUBMISSION_CAPACITY || log.submissions.try_reserve(1).is_err() {
            log.dropped = log.dropped.saturating_add(1);
            return;
        }
        log.submissions.push(submission);
    }
}

// ---- element classification ------------------------------------------------

fn equals_ascii_lower(value: &DomString, expected: &str) -> bool {
    let units = value.as_units();
    units.len() == expected.len()
        && units.iter().zip(expected.bytes()).all(|(&unit, byte)| {
            u8::try_from(unit).is_ok_and(|unit| unit.eq_ignore_ascii_case(&byte))
        })
}

const INPUT_TYPES: [&str; 22] = [
    "hidden",
    "text",
    "search",
    "tel",
    "url",
    "email",
    "password",
    "date",
    "month",
    "week",
    "time",
    "datetime-local",
    "number",
    "range",
    "color",
    "checkbox",
    "radio",
    "file",
    "submit",
    "image",
    "reset",
    "button",
];

/// The `type` state of an input: a known keyword, or `text`.
fn input_type(element: &Element) -> &'static str {
    attribute(element, "type")
        .and_then(|value| {
            INPUT_TYPES
                .iter()
                .find(|name| equals_ascii_lower(value, name))
        })
        .copied()
        .unwrap_or("text")
}

/// The kinds of control this module gives behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    /// `<input>` with its type keyword.
    Input(&'static str),
    TextArea,
    /// `<button>` with `submit`, `reset` or `button`.
    Button(&'static str),
    Select,
    Fieldset,
    Output,
    Object,
}

fn kind(node: &Node) -> Option<Kind> {
    let element = html_element(node)?;
    Some(match element.name().local() {
        "input" => Kind::Input(input_type(element)),
        "textarea" => Kind::TextArea,
        "button" => Kind::Button(
            attribute(element, "type")
                .and_then(|value| {
                    ["submit", "reset", "button"]
                        .into_iter()
                        .find(|name| equals_ascii_lower(value, name))
                })
                .unwrap_or("submit"),
        ),
        "select" => Kind::Select,
        "fieldset" => Kind::Fieldset,
        "output" => Kind::Output,
        "object" => Kind::Object,
        _ => return None,
    })
}

/// Inputs whose value mode is "value".
fn value_mode(input: &str) -> bool {
    matches!(
        input,
        "text"
            | "search"
            | "tel"
            | "url"
            | "email"
            | "password"
            | "date"
            | "month"
            | "week"
            | "time"
            | "datetime-local"
            | "number"
            | "range"
            | "color"
    )
}

/// Controls whose value the keyboard edits.
fn editable(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::TextArea
            | Kind::Input("text" | "search" | "tel" | "url" | "email" | "password" | "number")
    )
}

fn is_submit_button(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Button("submit") | Kind::Input("submit" | "image")
    )
}

fn has_attribute(element: &Element, name: &str) -> bool {
    attribute(element, name).is_some()
}

fn element_of(node: &Node) -> &Element {
    html_element(node).expect("classified control is an HTML element")
}

// ---- form owners ----------------------------------------------------------

fn is_form(document: &Document, id: NodeId) -> bool {
    document
        .node(id)
        .ok()
        .and_then(html_element)
        .is_some_and(|element| element.name().local() == "form")
}

fn root_of(agent: &mut Agent, document: &Document, mut id: NodeId) -> JsResult<NodeId> {
    while let Some(parent) = document.node(id).ok().and_then(Node::parent) {
        agent.check_native_work()?;
        id = parent;
    }
    Ok(id)
}

/// HTML "reset the form owner", computed on demand.
fn form_owner(agent: &mut Agent, document: &Document, id: NodeId) -> JsResult<Option<NodeId>> {
    let node = document
        .node(id)
        .map_err(|error| dom_exception::from_dom_error(agent, error))?;
    if kind(node).is_none() {
        return Ok(None);
    }
    // Every listed element honors the `form` content attribute.
    if let Some(wanted) = attribute(element_of(node), "form") {
        if !reaches_root(document, id, || agent.check_native_work())? {
            return Ok(None);
        }
        for (candidate, _) in document
            .walk(document.root())
            .map_err(|error| dom_exception::from_dom_error(agent, error))?
        {
            agent.check_native_work()?;
            let Some(found) = document.node(candidate).ok().and_then(html_element) else {
                continue;
            };
            if attribute(found, "id") == Some(wanted) {
                return Ok(is_form(document, candidate).then_some(candidate));
            }
        }
        return Ok(None);
    }
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        agent.check_native_work()?;
        if is_form(document, current) {
            return Ok(Some(current));
        }
        ancestor = document.node(current).ok().and_then(Node::parent);
    }
    if let Some(form) = document.parser_form(id) {
        if root_of(agent, document, form)? == root_of(agent, document, id)? {
            return Ok(Some(form));
        }
    }
    Ok(None)
}

/// The listed elements whose owner is `form`, in tree order.
fn owned_controls(agent: &mut Agent, document: &Document, form: NodeId) -> JsResult<Vec<NodeId>> {
    let root = root_of(agent, document, form)?;
    let mut controls = Vec::new();
    for (id, _) in document
        .walk(root)
        .map_err(|error| dom_exception::from_dom_error(agent, error))?
    {
        agent.check_native_work()?;
        let Some(kind) = document.node(id).ok().and_then(kind) else {
            continue;
        };
        if matches!(kind, Kind::Input("image")) {
            continue;
        }
        if form_owner(agent, document, id)? == Some(form) {
            controls.try_reserve(1).map_err(|_| OutOfMemory)?;
            controls.push(id);
        }
    }
    Ok(controls)
}

// ---- control records --------------------------------------------------------

fn record(agent: &mut Agent, control: Value) -> JsResult<Option<PrivateArray>> {
    with_page(agent, |agent, page| {
        Ok(PrivateArray::from_value(
            agent.weak_map_get(page.events.forms.values, control),
        ))
    })
}

fn ensure_record(agent: &mut Agent, control: Value) -> JsResult<PrivateArray> {
    if let Some(record) = record(agent, control)? {
        return Ok(record);
    }
    let empty = agent.string("")?;
    let record = PrivateArray::new(
        agent,
        &[
            Value::UNDEFINED,
            empty,
            Value::FALSE,
            Value::UNDEFINED,
            Value::UNDEFINED,
        ],
    )?;
    with_page(agent, |agent, page| {
        agent.weak_map_set(page.events.forms.values, control, record.value())
    })?;
    Ok(record)
}

fn units_of(agent: &Agent, value: Value) -> Vec<u16> {
    value
        .as_string()
        .map(|id| agent.heap.string(id).units().collect())
        .unwrap_or_default()
}

/// The facts validation and submission need about one control.
struct Snapshot {
    kind: Kind,
    name: Option<DomString>,
    value: Vec<u16>,
    checked: bool,
    disabled: bool,
    readonly: bool,
    required: bool,
    multiple: bool,
    min: Option<f64>,
    max: Option<f64>,
    min_length: Option<usize>,
    max_length: Option<usize>,
    in_datalist: bool,
    owner: Option<NodeId>,
}

fn non_negative(value: Option<&DomString>) -> Option<usize> {
    let text = String::from_utf16(value?.as_units()).ok()?;
    let text = text.trim_matches(|c: char| c.is_ascii_whitespace());
    text.parse::<u32>().ok().map(|value| value as usize)
}

fn floating(value: &[u16]) -> Option<f64> {
    let text = String::from_utf16(value).ok()?;
    // HTML "rules for parsing floating-point number values", restricted to
    // the valid serialization (no leading '+', no surrounding spaces).
    if text.is_empty()
        || text.starts_with('+')
        || text.starts_with('.')
        || text.ends_with('.')
        || text
            .chars()
            .any(|c| !(c.is_ascii_digit() || matches!(c, '-' | '.' | 'e' | 'E' | '+')))
    {
        return None;
    }
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

/// HTML value sanitization for the input types that have one here.
fn sanitize(input: &str, value: Vec<u16>) -> Vec<u16> {
    let strip = |value: Vec<u16>| -> Vec<u16> {
        value
            .into_iter()
            .filter(|&unit| unit != 0x0a && unit != 0x0d)
            .collect()
    };
    let trim = |value: Vec<u16>| -> Vec<u16> {
        let space = |unit: &u16| matches!(*unit, 0x09 | 0x0a | 0x0c | 0x0d | 0x20);
        let start = value
            .iter()
            .position(|unit| !space(unit))
            .unwrap_or(value.len());
        let end = value
            .iter()
            .rposition(|unit| !space(unit))
            .map_or(start, |end| end + 1);
        value[start..end].to_vec()
    };
    match input {
        "text" | "search" | "tel" | "password" => strip(value),
        "url" | "email" => trim(strip(value)),
        "number" => {
            if floating(&value).is_some() {
                value
            } else {
                Vec::new()
            }
        }
        _ => value,
    }
}

fn snapshot(agent: &mut Agent, control: Value) -> JsResult<Option<Snapshot>> {
    let dirty = match record(agent, control)? {
        Some(record) => Some(record.get(agent, VALUE)?),
        None => None,
    };
    let dirty = dirty.filter(|value| *value != Value::UNDEFINED);
    let dirty_units = dirty.map(|value| units_of(agent, value));
    let checked = checkedness(agent, control)?;
    read_node(agent, control, |agent, _, document, id, node| {
        let Some(kind) = kind(node) else {
            return Ok(None);
        };
        let element = element_of(node);
        let value = match kind {
            Kind::Input(input) if value_mode(input) => sanitize(
                input,
                dirty_units.unwrap_or_else(|| {
                    attribute(element, "value").map_or_else(Vec::new, |v| v.as_units().to_vec())
                }),
            ),
            Kind::Input("checkbox" | "radio") => attribute(element, "value")
                .map_or_else(|| "on".encode_utf16().collect(), |v| v.as_units().to_vec()),
            Kind::TextArea => dirty_units.map_or_else(|| text_content(document, id), Ok)?,
            _ => attribute(element, "value").map_or_else(Vec::new, |v| v.as_units().to_vec()),
        };
        let mut in_datalist = false;
        let mut ancestor = node.parent();
        while let Some(current) = ancestor {
            agent.check_native_work()?;
            let parent = document.node(current).ok();
            if parent
                .and_then(html_element)
                .is_some_and(|element| element.name().local() == "datalist")
            {
                in_datalist = true;
            }
            ancestor = parent.and_then(Node::parent);
        }
        let number = |name| attribute(element, name).and_then(|v| floating(v.as_units()));
        Ok(Some(Snapshot {
            kind,
            name: attribute(element, "name").cloned(),
            value,
            checked,
            disabled: disabled(agent, document, id, node)?,
            readonly: has_attribute(element, "readonly"),
            required: has_attribute(element, "required"),
            multiple: has_attribute(element, "multiple"),
            min: number("min"),
            max: number("max"),
            min_length: non_negative(attribute(element, "minlength")),
            max_length: non_negative(attribute(element, "maxlength")),
            in_datalist,
            owner: form_owner(agent, document, id)?,
        }))
    })
}

fn text_content(document: &Document, id: NodeId) -> JsResult<Vec<u16>> {
    let mut units = Vec::new();
    for child in document.children(id).map_err(|_| OutOfMemory)? {
        if let Ok(NodeKind::Text(text)) = document.node(child).map(Node::kind) {
            units
                .try_reserve(text.as_units().len())
                .map_err(|_| OutOfMemory)?;
            units.extend_from_slice(text.as_units());
        }
    }
    Ok(units)
}

fn checkedness(agent: &mut Agent, control: Value) -> JsResult<bool> {
    let is_check = read_node(agent, control, |_, _, _, _, node| {
        Ok(matches!(
            kind(node),
            Some(Kind::Input("checkbox" | "radio"))
        ))
    })?;
    if !is_check {
        return Ok(false);
    }
    super::input::checked_state(agent, control)
}

// ---- constraint validation ----------------------------------------------------

/// HTML "candidate for constraint validation".
fn will_validate(snapshot: &Snapshot) -> bool {
    let barred_type = match snapshot.kind {
        Kind::Input(input) => {
            matches!(input, "hidden" | "reset" | "button" | "image")
                || (snapshot.readonly && input != "checkbox" && input != "radio")
        }
        Kind::TextArea => snapshot.readonly,
        Kind::Button(kind) => kind != "submit",
        Kind::Select => false,
        Kind::Fieldset | Kind::Output | Kind::Object => true,
    };
    !barred_type && !snapshot.disabled && !snapshot.in_datalist
}

#[derive(Clone, Copy, Default)]
struct Flags {
    value_missing: bool,
    type_mismatch: bool,
    too_long: bool,
    too_short: bool,
    range_underflow: bool,
    range_overflow: bool,
    custom_error: bool,
}

impl Flags {
    fn valid(self) -> bool {
        !(self.value_missing
            || self.type_mismatch
            || self.too_long
            || self.too_short
            || self.range_underflow
            || self.range_overflow
            || self.custom_error)
    }
}

fn valid_email(value: &[u16]) -> bool {
    let Ok(text) = String::from_utf16(value) else {
        return false;
    };
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    const LOCAL: &str = ".!#$%&'*+/=?^_`{|}~-";
    !local.is_empty()
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || LOCAL.contains(c))
        && !domain.is_empty()
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

fn valid_absolute_url(value: &[u16]) -> bool {
    let Ok(text) = String::from_utf16(value) else {
        return false;
    };
    let Some((scheme, rest)) = text.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !rest.chars().any(char::is_whitespace)
}

fn radio_group_checked(agent: &mut Agent, control: Value, snapshot: &Snapshot) -> JsResult<bool> {
    if snapshot.checked {
        return Ok(true);
    }
    let Some(name) = snapshot
        .name
        .clone()
        .filter(|name| !name.as_units().is_empty())
    else {
        return Ok(false);
    };
    let members = read_node(agent, control, |agent, wrappers, document, id, _| {
        let root = root_of(agent, document, id)?;
        let mut members = Vec::new();
        for (candidate, _) in document
            .walk(root)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?
        {
            agent.check_native_work()?;
            if candidate == id {
                continue;
            }
            let Some(node) = document.node(candidate).ok() else {
                continue;
            };
            if kind(node) != Some(Kind::Input("radio"))
                || attribute(element_of(node), "name") != Some(&name)
                || form_owner(agent, document, candidate)? != snapshot.owner
            {
                continue;
            }
            members.try_reserve(1).map_err(|_| OutOfMemory)?;
            members.push(wrap(wrappers, agent, document, candidate)?);
        }
        PrivateArray::new(agent, &members)
    })?;
    with_root(agent, members.value(), |agent| {
        for index in 0..members.len(agent)? {
            let member = members.get(agent, index)?;
            if super::input::checked_state(agent, member)? {
                return Ok(true);
            }
        }
        Ok(false)
    })
}

fn flags(agent: &mut Agent, control: Value, snapshot: &Snapshot) -> JsResult<Flags> {
    let custom = match record(agent, control)? {
        Some(record) => {
            let message = record.get(agent, CUSTOM)?;
            !units_of(agent, message).is_empty()
        }
        None => false,
    };
    let edited = match record(agent, control)? {
        Some(record) => record.get(agent, EDITED)? == Value::TRUE,
        None => false,
    };
    let mut flags = Flags {
        custom_error: custom,
        ..Flags::default()
    };
    let empty = snapshot.value.is_empty();
    match snapshot.kind {
        Kind::Input("checkbox") => flags.value_missing = snapshot.required && !snapshot.checked,
        Kind::Input("radio") => {
            flags.value_missing =
                snapshot.required && !radio_group_checked(agent, control, snapshot)?;
        }
        Kind::Input(input) if value_mode(input) || input == "file" => {
            flags.value_missing =
                snapshot.required && empty && input != "range" && input != "color";
            if !empty {
                flags.type_mismatch = match input {
                    "email" => {
                        if snapshot.multiple {
                            snapshot
                                .value
                                .split(|&unit| unit == u16::from(b','))
                                .any(|part| !valid_email(part))
                        } else {
                            !valid_email(&snapshot.value)
                        }
                    }
                    "url" => !valid_absolute_url(&snapshot.value),
                    _ => false,
                };
                if matches!(input, "number" | "range") {
                    if let Some(number) = floating(&snapshot.value) {
                        flags.range_underflow = snapshot.min.is_some_and(|min| number < min);
                        flags.range_overflow = snapshot.max.is_some_and(|max| number > max);
                    }
                }
            }
        }
        Kind::TextArea => flags.value_missing = snapshot.required && empty,
        _ => {}
    }
    if edited && editable(snapshot.kind) && !empty {
        let length = snapshot.value.len();
        flags.too_long = snapshot.max_length.is_some_and(|max| length > max);
        flags.too_short = snapshot.min_length.is_some_and(|min| length < min);
    }
    Ok(flags)
}

/// Whether the control is a candidate that satisfies its constraints.
fn satisfies(agent: &mut Agent, control: Value) -> JsResult<bool> {
    let Some(snapshot) = snapshot(agent, control)? else {
        return Ok(true);
    };
    if !will_validate(&snapshot) {
        return Ok(true);
    }
    flags(agent, control, &snapshot).map(Flags::valid)
}

fn fire_simple(agent: &mut Agent, target: Value, kind: &str, flags: u16) -> JsResult<bool> {
    let event = state::trusted_event(agent, "Event", kind, flags, Value::NULL, Value::NULL)?;
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

/// HTML "check validity": fire `invalid` at a failing candidate.
fn check_control(agent: &mut Agent, control: Value) -> JsResult<bool> {
    if satisfies(agent, control)? {
        return Ok(true);
    }
    fire_simple(agent, control, "invalid", Event::CANCELABLE)?;
    Ok(false)
}

/// HTML "statically validate the constraints" of a form.
fn check_form(agent: &mut Agent, form: Value) -> JsResult<bool> {
    let controls = controls_of(agent, form)?;
    with_root(agent, controls.value(), |agent| {
        let mut valid = true;
        for index in 0..controls.len(agent)? {
            let control = controls.get(agent, index)?;
            if !satisfies(agent, control)? {
                valid = false;
                fire_simple(agent, control, "invalid", Event::CANCELABLE)?;
            }
        }
        Ok(valid)
    })
}

fn controls_of(agent: &mut Agent, form: Value) -> JsResult<PrivateArray> {
    read_node(agent, form, |agent, wrappers, document, id, _| {
        let ids = owned_controls(agent, document, id)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(ids.len())
            .map_err(|_| OutOfMemory)?;
        for control in ids {
            values.push(wrap(wrappers, agent, document, control)?);
        }
        PrivateArray::new(agent, &values)
    })
}

// ---- submission and reset ------------------------------------------------------

fn text(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

/// HTML "submit" from a submitter (or none), without navigation.
fn submit(
    agent: &mut Agent,
    form: Value,
    submitter: Option<Value>,
    from_script: bool,
) -> JsResult<()> {
    let (no_validate, connected) = read_node(agent, form, |agent, _, document, id, node| {
        let element = element_of(node);
        Ok((
            has_attribute(element, "novalidate"),
            reaches_root(document, id, || agent.check_native_work())?,
        ))
    })?;
    if !connected {
        return Ok(());
    }
    let submitter_no_validate = match submitter {
        Some(button) => read_node(agent, button, |_, _, _, _, node| {
            Ok(html_element(node).is_some_and(|element| has_attribute(element, "formnovalidate")))
        })?,
        None => false,
    };
    if !from_script {
        if !no_validate && !submitter_no_validate && !check_form(agent, form)? {
            return Ok(());
        }
        let event = submit_event(agent, submitter.unwrap_or(Value::NULL))?;
        let proceed = with_root(agent, event, |agent| {
            let state = Event::receiver(agent, event)?;
            fire(
                agent,
                Target {
                    value: form,
                    node: true,
                },
                event,
                state,
            )
        })?;
        if !proceed {
            return Ok(());
        }
    }
    let entries = entry_list(agent, form, submitter)?;
    let submitter = match submitter {
        Some(button) => Some(read_node(agent, button, |_, _, _, id, _| Ok(id))?),
        None => None,
    };
    let submission = read_node(agent, form, |_, _, _, id, node| {
        let element = element_of(node);
        let method = attribute(element, "method")
            .and_then(|value| {
                ["post", "dialog"]
                    .into_iter()
                    .find(|name| equals_ascii_lower(value, name))
            })
            .unwrap_or("get");
        Ok(FormSubmission {
            form: id,
            submitter,
            method: method.to_owned(),
            action: attribute(element, "action").map_or_else(String::new, |v| text(v.as_units())),
            entries,
        })
    })?;
    with_page(agent, |_, page| {
        page.events.forms.record(submission);
        Ok(())
    })
}

fn entry_list(
    agent: &mut Agent,
    form: Value,
    submitter: Option<Value>,
) -> JsResult<Vec<(String, String)>> {
    let controls = controls_of(agent, form)?;
    with_root(agent, controls.value(), |agent| {
        let mut entries = Vec::new();
        for index in 0..controls.len(agent)? {
            agent.check_native_work()?;
            let control = controls.get(agent, index)?;
            let Some(snapshot) = snapshot(agent, control)? else {
                continue;
            };
            if snapshot.in_datalist || snapshot.disabled {
                continue;
            }
            match snapshot.kind {
                Kind::Button(_) | Kind::Input("submit" | "reset" | "button" | "image")
                    if Some(control) != submitter =>
                {
                    continue;
                }
                Kind::Input("checkbox" | "radio") if !snapshot.checked => continue,
                Kind::Input("file")
                | Kind::Select
                | Kind::Fieldset
                | Kind::Output
                | Kind::Object => {
                    continue;
                }
                _ => {}
            }
            let Some(name) = snapshot.name.filter(|name| !name.as_units().is_empty()) else {
                continue;
            };
            entries.try_reserve(1).map_err(|_| OutOfMemory)?;
            entries.push((text(name.as_units()), text(&snapshot.value)));
        }
        Ok(entries)
    })
}

/// HTML form `reset()`: fire `reset`, then restore each control's default.
fn reset(agent: &mut Agent, form: Value) -> JsResult<()> {
    if !fire_simple(agent, form, "reset", Event::BUBBLES | Event::CANCELABLE)? {
        return Ok(());
    }
    let controls = controls_of(agent, form)?;
    with_root(agent, controls.value(), |agent| {
        for index in 0..controls.len(agent)? {
            let control = controls.get(agent, index)?;
            if let Some(record) = record(agent, control)? {
                record.set(agent, VALUE, Value::UNDEFINED)?;
                record.set(agent, EDITED, Value::FALSE)?;
            }
            super::input::reset_checkedness(agent, control)?;
        }
        Ok(())
    })
}

/// Activation behavior of submit and reset buttons, after an uncanceled
/// `click` dispatch whose path starts at `target`.
pub(super) fn activate_button(
    agent: &mut Agent,
    path: PrivateArray,
    bubbles: bool,
) -> JsResult<()> {
    let window = with_page(agent, |_, page| Ok(page.window))?;
    let len = if bubbles { path.len(agent)? } else { 1 };
    for index in 0..len {
        agent.check_native_work()?;
        let value = path.get(agent, index)?;
        if value == window {
            return Ok(());
        }
        let found = read_node(agent, value, |agent, wrappers, document, id, node| {
            let Some(kind) = kind(node) else {
                return Ok(None);
            };
            if disabled(agent, document, id, node)? {
                return Ok(Some(None));
            }
            let reset = matches!(kind, Kind::Button("reset") | Kind::Input("reset"));
            if !(is_submit_button(kind) || reset) {
                return Ok(Some(None));
            }
            let Some(form) = form_owner(agent, document, id)? else {
                return Ok(Some(None));
            };
            let form = wrap(wrappers, agent, document, form)?;
            Ok(Some(Some((form, reset))))
        })?;
        match found {
            None => continue,
            Some(None) => return Ok(()),
            Some(Some((form, reset_form))) => {
                return with_root(agent, form, |agent| {
                    if reset_form {
                        reset(agent, form)
                    } else {
                        submit(agent, form, Some(value), false)
                    }
                });
            }
        }
    }
    Ok(())
}

// ---- events -----------------------------------------------------------------

fn base_fields(view: Value) -> [Value; 5] {
    [view, Value::FALSE, Value::FALSE, Value::FALSE, Value::FALSE]
}

fn new_event(
    agent: &mut Agent,
    interface: &'static str,
    event_type: &str,
    flags: u16,
    fields: &[Value],
) -> JsResult<Value> {
    let view = with_page(agent, |_, page| Ok(page.window))?;
    let ui = interface != "SubmitEvent";
    let extra = PrivateArray::new(agent, &base_fields(view))?;
    let holder = PrivateArray::new(agent, fields)?;
    with_root(agent, holder.value(), |agent| {
        with_root(agent, extra.value(), |agent| {
            let event = state::trusted_event(
                agent,
                interface,
                event_type,
                flags | if ui { Event::UI } else { 0 },
                Value::number(0.0),
                if ui { extra.value() } else { Value::NULL },
            )?;
            with_page(agent, |agent, page| {
                agent.weak_map_set(page.events.forms.events, event, holder.value())
            })?;
            Ok(event)
        })
    })
}

fn submit_event(agent: &mut Agent, submitter: Value) -> JsResult<Value> {
    new_event(
        agent,
        "SubmitEvent",
        "submit",
        Event::BUBBLES | Event::CANCELABLE,
        &[submitter],
    )
}

fn focus_event(agent: &mut Agent, kind: &str, related: Value) -> JsResult<Value> {
    let bubbles = matches!(kind, "focusin" | "focusout");
    new_event(
        agent,
        "FocusEvent",
        kind,
        Event::COMPOSED | if bubbles { Event::BUBBLES } else { 0 },
        &[related],
    )
}

fn input_event(
    agent: &mut Agent,
    kind: &str,
    input_type: &str,
    data: Option<&[u16]>,
) -> JsResult<Value> {
    let input_type = agent.string(input_type)?;
    with_root(agent, input_type, |agent| {
        let data = match data {
            Some(units) => crate::webidl::from_dom_units(agent, units)?,
            None => Value::NULL,
        };
        with_root(agent, data, |agent| {
            let cancelable = if kind == "beforeinput" {
                Event::CANCELABLE
            } else {
                0
            };
            new_event(
                agent,
                "InputEvent",
                kind,
                Event::BUBBLES | Event::COMPOSED | cancelable,
                &[data, input_type, Value::FALSE],
            )
        })
    })
}

fn event_field<const SLOT: u32>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        let fields = agent.weak_map_get(page.events.forms.events, this);
        match PrivateArray::from_value(fields) {
            Some(fields) if fields.len(agent)? > SLOT => fields.get(agent, SLOT),
            _ => agent.type_error("invalid event receiver"),
        }
    })
}

/// Script-constructed FocusEvent, InputEvent and SubmitEvent: base event
/// options plus the interface's own members.
fn construct<const KIND: u8>(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("event constructor requires new");
    };
    let (interface, members): (&'static str, &[&str]) = match KIND {
        0 => ("FocusEvent", &["relatedTarget"]),
        1 => ("InputEvent", &["data", "inputType", "isComposing"]),
        _ => ("SubmitEvent", &["submitter"]),
    };
    let event_type = required_arg(agent, args, 0)?;
    let event_type = Value::string(crate::webidl::coerce_string(agent, event_type)?);
    with_root(agent, event_type, |agent| {
        let init = args.get(1).copied().unwrap_or(Value::UNDEFINED);
        let (flags, _) = state::init_options::<false>(agent, init)?;
        let mut fields = Vec::new();
        for &member in members {
            let value = match init.as_object() {
                Some(object) => option(agent, object, member)?,
                None => Value::UNDEFINED,
            };
            let value = match (member, value) {
                ("inputType", Value::UNDEFINED) => agent.string("")?,
                ("inputType", value) => Value::string(crate::webidl::coerce_string(agent, value)?),
                ("isComposing", value) => Value::boolean(agent.to_boolean(value)),
                ("data", value) if value.is_nullish() => Value::NULL,
                ("data", value) => Value::string(crate::webidl::coerce_string(agent, value)?),
                (_, value) if value.is_nullish() => Value::NULL,
                (_, value) => value,
            };
            fields.try_reserve(1).map_err(|_| OutOfMemory)?;
            fields.push(value);
        }
        let holder = PrivateArray::new(agent, &fields)?;
        with_root(agent, holder.value(), |agent| {
            let prototype = agent.host_prototype_from_new_target(interface, new_target)?;
            let ui = KIND != 2;
            let view = Value::NULL;
            let extra = if ui {
                PrivateArray::new(agent, &base_fields(view))?.value()
            } else {
                Value::NULL
            };
            let event = with_root(agent, extra, |agent| {
                state::create(
                    agent,
                    prototype,
                    event_type,
                    flags | if ui { Event::UI } else { 0 },
                    Value::number(0.0),
                    extra,
                )
            })?;
            with_page(agent, |agent, page| {
                agent.weak_map_set(page.events.forms.events, event, holder.value())
            })?;
            Ok(event)
        })
    })
}

// ---- focus ------------------------------------------------------------------

/// The tab index of a focusable area, or `None` when the element cannot be
/// focused. Disabled controls and hidden inputs are not focusable.
fn tab_index(
    agent: &mut Agent,
    document: &Document,
    id: NodeId,
    node: &Node,
) -> JsResult<Option<i32>> {
    let Some(element) = html_element(node) else {
        return Ok(None);
    };
    let explicit = attribute(element, "tabindex").and_then(|value| {
        let text = String::from_utf16(value.as_units()).ok()?;
        text.trim_matches(|c: char| c.is_ascii_whitespace())
            .parse::<i32>()
            .ok()
    });
    let natural = match element.name().local() {
        "a" | "area" => has_attribute(element, "href"),
        "button" | "select" | "textarea" => true,
        "input" => input_type(element) != "hidden",
        "summary" => true,
        _ => false,
    };
    if kind(node).is_some() && disabled(agent, document, id, node)? {
        return Ok(None);
    }
    Ok(match (explicit, natural) {
        (Some(index), _) => Some(index),
        (None, true) => Some(0),
        (None, false) => None,
    })
}

fn focused_value(agent: &mut Agent) -> JsResult<Option<Value>> {
    let focused = with_page(agent, |_, page| Ok(page.events.forms.focused.get()))?;
    let Some(id) = focused else {
        return Ok(None);
    };
    with_page(agent, |agent, page| {
        if id.document() != page.main {
            return Ok(None);
        }
        page.read(agent, page.main, |agent, document| {
            let Ok(node) = document.node(id) else {
                return Ok(None);
            };
            if !reaches_root(document, id, || agent.check_native_work())?
                || tab_index(agent, document, id, node)?.is_none()
            {
                return Ok(None);
            }
            let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
                return agent.type_error("DOM bindings are busy");
            };
            wrap(&mut wrappers, agent, document, id).map(Some)
        })
    })
}

/// HTML "focus update steps" from the current focus to `next` (`None` is
/// the viewport): change, blur and focusout, then focus and focusin.
fn move_focus(agent: &mut Agent, next: Option<(NodeId, Value)>) -> JsResult<()> {
    let old = focused_value(agent)?;
    let old_id = match old {
        Some(value) => Some(read_node(agent, value, |_, _, _, id, _| Ok(id))?),
        None => None,
    };
    if old_id == next.map(|(id, _)| id) {
        return Ok(());
    }
    let next_value = next.map_or(Value::NULL, |(_, value)| value);
    with_root(agent, next_value, |agent| {
        if let Some(old) = old {
            with_root(agent, old, |agent| {
                // A text control whose value changed since it gained focus
                // commits with `change` when it loses focus.
                if let Some(record) = record(agent, old)? {
                    let before = record.get(agent, FOCUS_VALUE)?;
                    let now = current_value(agent, old)?;
                    record.set(agent, FOCUS_VALUE, Value::UNDEFINED)?;
                    if before != Value::UNDEFINED && units_of(agent, before) != now {
                        fire_simple(agent, old, "change", Event::BUBBLES)?;
                    }
                }
                with_page(agent, |_, page| {
                    page.events.forms.focused.set(None);
                    Ok(())
                })?;
                for kind in ["blur", "focusout"] {
                    let event = focus_event(agent, kind, next_value)?;
                    fire_focus(agent, old, event)?;
                }
                Ok(())
            })?;
        }
        let Some((id, value)) = next else {
            return Ok(());
        };
        with_page(agent, |_, page| {
            page.events.forms.focused.set(Some(id));
            Ok(())
        })?;
        let editable_now = read_node(agent, value, |_, _, _, _, node| {
            Ok(kind(node).is_some_and(editable))
        })?;
        if editable_now {
            let now = current_value(agent, value)?;
            let now = crate::webidl::from_dom_units(agent, &now)?;
            ensure_record(agent, value)?.set(agent, FOCUS_VALUE, now)?;
        }
        let related = old.unwrap_or(Value::NULL);
        for kind in ["focus", "focusin"] {
            let event = focus_event(agent, kind, related)?;
            fire_focus(agent, value, event)?;
        }
        Ok(())
    })
}

fn fire_focus(agent: &mut Agent, target: Value, event: Value) -> JsResult<bool> {
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

/// Run the focusing steps for `target` if it is a focusable area.
pub(super) fn focus_element(agent: &mut Agent, target: Value) -> JsResult<bool> {
    let focusable = read_node(agent, target, |agent, _, document, id, node| {
        if id.document() != document.id()
            || !reaches_root(document, id, || agent.check_native_work())?
        {
            return Ok(None);
        }
        Ok(tab_index(agent, document, id, node)?.map(|_| id))
    })?;
    let main = with_page(agent, |_, page| Ok(page.main))?;
    match focusable {
        Some(id) if id.document() == main => {
            move_focus(agent, Some((id, target)))?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Focus the nearest focusable inclusive ancestor of a clicked node, or
/// the viewport when there is none (HTML focusing on activation).
pub(super) fn focus_for_click(agent: &mut Agent, target: Value) -> JsResult<()> {
    let found = read_node(agent, target, |agent, wrappers, document, id, _| {
        let mut current = Some(id);
        while let Some(candidate) = current {
            agent.check_native_work()?;
            let node = document
                .node(candidate)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
            if tab_index(agent, document, candidate, node)?.is_some() {
                return Ok(Some((
                    candidate,
                    wrap(wrappers, agent, document, candidate)?,
                )));
            }
            current = node.parent();
        }
        Ok(None)
    })?;
    move_focus(agent, found)
}

/// Sequential focus navigation for an unhandled Tab key.
pub(super) fn navigate(agent: &mut Agent, backward: bool) -> JsResult<()> {
    let current = with_page(agent, |_, page| Ok(page.events.forms.focused.get()))?;
    let next = with_page(agent, |agent, page| {
        page.read(agent, page.main, |agent, document| {
            let mut positive = Vec::new();
            let mut zero = Vec::new();
            for (id, _) in document
                .walk(document.root())
                .map_err(|error| dom_exception::from_dom_error(agent, error))?
            {
                agent.check_native_work()?;
                let Ok(node) = document.node(id) else {
                    continue;
                };
                match tab_index(agent, document, id, node)? {
                    Some(index) if index > 0 => {
                        positive.try_reserve(1).map_err(|_| OutOfMemory)?;
                        positive.push((index, id));
                    }
                    Some(0) => {
                        zero.try_reserve(1).map_err(|_| OutOfMemory)?;
                        zero.push(id);
                    }
                    _ => {}
                }
            }
            positive.sort_by_key(|&(index, _)| index);
            let mut order: Vec<NodeId> = positive.into_iter().map(|(_, id)| id).collect();
            order.try_reserve(zero.len()).map_err(|_| OutOfMemory)?;
            order.extend(zero);
            if order.is_empty() {
                return Ok(None);
            }
            let position =
                current.and_then(|id| order.iter().position(|&candidate| candidate == id));
            let index = match (position, backward) {
                (Some(at), false) => (at + 1) % order.len(),
                (Some(at), true) => (at + order.len() - 1) % order.len(),
                (None, false) => 0,
                (None, true) => order.len() - 1,
            };
            let id = order[index];
            let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
                return agent.type_error("DOM bindings are busy");
            };
            Ok(Some((id, wrap(&mut wrappers, agent, document, id)?)))
        })
    })?;
    if next.is_some() {
        move_focus(agent, next)?;
    }
    Ok(())
}

// ---- text editing -----------------------------------------------------------------

fn current_value(agent: &mut Agent, control: Value) -> JsResult<Vec<u16>> {
    Ok(snapshot(agent, control)?
        .map(|snapshot| snapshot.value)
        .unwrap_or_default())
}

fn store_value(agent: &mut Agent, control: Value, units: &[u16], edited: bool) -> JsResult<()> {
    let value = crate::webidl::from_dom_units(agent, units)?;
    with_root(agent, value, |agent| {
        let record = ensure_record(agent, control)?;
        record.set(agent, VALUE, value)?;
        if edited {
            record.set(agent, EDITED, Value::TRUE)?;
        }
        Ok(())
    })
}

/// The editing default action of a key press on a text control:
/// `beforeinput`, the change, then `input`. Enter in a single-line input
/// submits its form implicitly. Returns whether the key was handled.
pub(super) fn edit(agent: &mut Agent, control: Value, key: &[u16]) -> JsResult<bool> {
    let Some(snapshot) = snapshot(agent, control)? else {
        return Ok(false);
    };
    if !editable(snapshot.kind) {
        return Ok(false);
    }
    let enter = key.iter().copied().eq("Enter".encode_utf16());
    let backspace = key.iter().copied().eq("Backspace".encode_utf16());
    let printable =
        !enter && !backspace && String::from_utf16(key).is_ok_and(|text| text.chars().count() == 1);
    if enter && snapshot.kind != Kind::TextArea {
        implicit_submission(agent, control, snapshot.owner)?;
        return Ok(true);
    }
    if !(enter || backspace || printable) || snapshot.disabled || snapshot.readonly {
        return Ok(false);
    }
    let (input_type, data): (&str, Option<Vec<u16>>) = if backspace {
        ("deleteContentBackward", None)
    } else if enter {
        ("insertLineBreak", None)
    } else {
        ("insertText", Some(key.to_vec()))
    };
    let before = input_event(agent, "beforeinput", input_type, data.as_deref())?;
    if !fire_focus(agent, control, before)? {
        return Ok(true);
    }
    let mut value = current_value(agent, control)?;
    if backspace {
        let cut = match value.as_slice() {
            [.., high, low]
                if (0xd800..0xdc00).contains(high) && (0xdc00..0xe000).contains(low) =>
            {
                2
            }
            [] => 0,
            _ => 1,
        };
        value.truncate(value.len() - cut);
    } else {
        let inserted: Vec<u16> = if enter { vec![0x0a] } else { key.to_vec() };
        if snapshot
            .max_length
            .is_some_and(|max| value.len() + inserted.len() > max)
        {
            return Ok(true);
        }
        value.try_reserve(inserted.len()).map_err(|_| OutOfMemory)?;
        value.extend(inserted);
    }
    store_value(agent, control, &value, true)?;
    let after = input_event(agent, "input", input_type, data.as_deref())?;
    fire_focus(agent, control, after)?;
    Ok(true)
}

/// HTML implicit submission: click the form's default button, or submit
/// a form whose single text field blocks implicit submission.
fn implicit_submission(agent: &mut Agent, control: Value, owner: Option<NodeId>) -> JsResult<()> {
    let Some(form_id) = owner else {
        return Ok(());
    };
    let (form, default_button) = read_node(agent, control, |agent, wrappers, document, _, _| {
        let form = wrap(wrappers, agent, document, form_id)?;
        let mut default_button = None;
        for id in owned_controls(agent, document, form_id)? {
            if document
                .node(id)
                .ok()
                .and_then(kind)
                .is_some_and(is_submit_button)
            {
                default_button = Some(wrap(wrappers, agent, document, id)?);
                break;
            }
        }
        Ok((form, default_button))
    })?;
    with_root(agent, form, |agent| match default_button {
        Some(button) => with_root(agent, button, |agent| {
            let disabled_button = read_node(agent, button, |agent, _, document, id, node| {
                disabled(agent, document, id, node)
            })?;
            if !disabled_button {
                synthetic_click(agent, button)?;
            }
            Ok(())
        }),
        None => submit(agent, form, None, false),
    })
}

fn synthetic_click(agent: &mut Agent, target: Value) -> JsResult<()> {
    let event = super::ui::click_event(agent, None, super::Modifiers::default())?;
    with_root(agent, event, |agent| {
        let state = Event::receiver(agent, event)?;
        state.untrust(agent)?;
        fire(
            agent,
            Target {
                value: target,
                node: true,
            },
            event,
            state,
        )
        .map(drop)
    })
}

// ---- bindings ---------------------------------------------------------------

fn define_accessor(
    agent: &mut Agent,
    prototype: ObjectId,
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
        prototype,
        key,
        PropertyDescriptor::accessor(get, set, true, true),
    )
}

/// Reflected string attributes, by index into [`STRINGS`].
const STRINGS: [&str; 8] = [
    "name",
    "placeholder",
    "min",
    "max",
    "pattern",
    "action",
    "autocomplete",
    "step",
];
/// Reflected boolean attributes, by index into [`BOOLEANS`].
const BOOLEANS: [&str; 6] = [
    "required",
    "readonly",
    "disabled",
    "multiple",
    "novalidate",
    "formnovalidate",
];

fn reflect_string<const INDEX: usize>(
    agent: &mut Agent,
    this: Value,
    _: &[Value],
) -> JsResult<Value> {
    read_node(agent, this, |agent, _, _, _, node| {
        let Some(element) = html_element(node) else {
            return agent.type_error("expected an HTML element");
        };
        match attribute(element, STRINGS[INDEX]) {
            Some(value) => from_dom_string(agent, value),
            None => agent.string(""),
        }
    })
}

fn set_reflect_string<const INDEX: usize>(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
) -> JsResult<Value> {
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let value = to_dom_string(agent, value, DomStringConversion::Default)?;
    set_content(agent, this, STRINGS[INDEX], Some(value))
}

fn reflect_bool<const INDEX: usize>(
    agent: &mut Agent,
    this: Value,
    _: &[Value],
) -> JsResult<Value> {
    read_node(agent, this, |agent, _, _, _, node| {
        let Some(element) = html_element(node) else {
            return agent.type_error("expected an HTML element");
        };
        Ok(Value::boolean(has_attribute(element, BOOLEANS[INDEX])))
    })
}

fn set_reflect_bool<const INDEX: usize>(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
) -> JsResult<Value> {
    let on = agent.to_boolean(args.first().copied().unwrap_or(Value::UNDEFINED));
    set_content(agent, this, BOOLEANS[INDEX], on.then(DomString::default))
}

fn set_content(
    agent: &mut Agent,
    this: Value,
    name: &str,
    value: Option<DomString>,
) -> JsResult<Value> {
    with_mut_tree(agent, this, |agent, document, _, id| {
        let result = match value {
            Some(value) => {
                let name = QualifiedName::from_parsed(None, None, name)
                    .map_err(|error| dom_exception::from_dom_error(agent, error))?;
                document.set_attribute(id, name, value).map(drop)
            }
            None => document.remove_attribute(id, None, name).map(drop),
        };
        result.map_err(|error| dom_exception::from_dom_error(agent, error))?;
        Ok(Value::UNDEFINED)
    })
}

fn control_receiver(agent: &mut Agent, this: Value) -> JsResult<Kind> {
    read_node(agent, this, |agent, _, _, _, node| match kind(node) {
        Some(kind) => Ok(kind),
        None => agent.type_error("expected a form control"),
    })
}

fn input_type_getter(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    match control_receiver(agent, this)? {
        Kind::Input(input) => agent.string(input),
        Kind::Button(button) => agent.string(button),
        Kind::TextArea => agent.string("textarea"),
        Kind::Select => agent.string("select-one"),
        Kind::Fieldset => agent.string("fieldset"),
        Kind::Output => agent.string("output"),
        Kind::Object => agent.string(""),
    }
}

fn input_type_setter(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    control_receiver(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let value = to_dom_string(agent, value, DomStringConversion::Default)?;
    set_content(agent, this, "type", Some(value))
}

fn value_getter(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    control_receiver(agent, this)?;
    let value = current_value(agent, this)?;
    crate::webidl::from_dom_units(agent, &value)
}

fn value_setter(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let kind = control_receiver(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let value = to_dom_string(agent, value, DomStringConversion::LegacyNullToEmptyString)?;
    match kind {
        Kind::Input(input) if value_mode(input) => {
            let units = sanitize(input, value.as_units().to_vec());
            store_value(agent, this, &units, false)?;
            // A script change resets the user-edit flag used by tooLong.
            ensure_record(agent, this)?.set(agent, EDITED, Value::FALSE)?;
            Ok(Value::UNDEFINED)
        }
        Kind::TextArea => {
            store_value(agent, this, value.as_units(), false)?;
            ensure_record(agent, this)?.set(agent, EDITED, Value::FALSE)?;
            Ok(Value::UNDEFINED)
        }
        _ => set_content(agent, this, "value", Some(value)),
    }
}

fn default_value_getter(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let kind = control_receiver(agent, this)?;
    read_node(agent, this, |agent, _, document, id, node| {
        if kind == Kind::TextArea {
            let units = text_content(document, id)?;
            return crate::webidl::from_dom_units(agent, &units);
        }
        match attribute(element_of(node), "value") {
            Some(value) => from_dom_string(agent, value),
            None => agent.string(""),
        }
    })
}

fn default_value_setter(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let kind = control_receiver(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let value = to_dom_string(agent, value, DomStringConversion::Default)?;
    if kind == Kind::TextArea {
        return with_mut_tree(agent, this, |agent, document, _, id| {
            document
                .replace_children_with_text(id, value)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
            Ok(Value::UNDEFINED)
        });
    }
    set_content(agent, this, "value", Some(value))
}

fn long_reflect<const MIN: bool>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, this, |agent, _, _, _, node| {
        let Some(element) = html_element(node) else {
            return agent.type_error("expected an HTML element");
        };
        let name = if MIN { "minlength" } else { "maxlength" };
        Ok(Value::number(
            non_negative(attribute(element, name)).map_or(-1.0, |value| value as f64),
        ))
    })
}

fn set_long_reflect<const MIN: bool>(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
) -> JsResult<Value> {
    let value = crate::webidl::to_long(
        agent,
        args.first().copied().unwrap_or(Value::UNDEFINED),
        crate::webidl::IntegerConversion::Wrap,
    )?;
    if value < 0 {
        return Err(super::super::creation::exception(
            agent,
            "IndexSizeError",
            "the length must not be negative",
        ));
    }
    let name = if MIN { "minlength" } else { "maxlength" };
    set_content(agent, this, name, Some(DomString::from(value.to_string())))
}

fn form_getter(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    control_receiver(agent, this)?;
    read_node(
        agent,
        this,
        |agent, wrappers, document, id, _| match form_owner(agent, document, id)? {
            Some(form) => wrap(wrappers, agent, document, form),
            None => Ok(Value::NULL),
        },
    )
}

fn will_validate_getter(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    control_receiver(agent, this)?;
    let snapshot = snapshot(agent, this)?;
    Ok(Value::boolean(snapshot.as_ref().is_some_and(will_validate)))
}

fn validation_message(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    control_receiver(agent, this)?;
    let Some(snapshot) = snapshot(agent, this)? else {
        return agent.string("");
    };
    if !will_validate(&snapshot) {
        return agent.string("");
    }
    let flags = flags(agent, this, &snapshot)?;
    if flags.custom_error {
        let record = ensure_record(agent, this)?;
        return record.get(agent, CUSTOM);
    }
    let message = if flags.value_missing {
        "Please fill out this field."
    } else if flags.type_mismatch {
        "Please enter a valid value."
    } else if flags.too_long {
        "Please shorten this text."
    } else if flags.too_short {
        "Please lengthen this text."
    } else if flags.range_underflow {
        "Value is too small."
    } else if flags.range_overflow {
        "Value is too large."
    } else {
        ""
    };
    agent.string(message)
}

fn check_validity(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    if is_form_element(agent, this)? {
        return check_form(agent, this).map(Value::boolean);
    }
    control_receiver(agent, this)?;
    check_control(agent, this).map(Value::boolean)
}

fn set_custom_validity(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    control_receiver(agent, this)?;
    let message = required_arg(agent, args, 0)?;
    let message = to_dom_string(agent, message, DomStringConversion::Default)?;
    let message = from_dom_string(agent, &message)?;
    with_root(agent, message, |agent| {
        ensure_record(agent, this)?.set(agent, CUSTOM, message)
    })?;
    Ok(Value::UNDEFINED)
}

fn validity_getter(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    control_receiver(agent, this)?;
    let record = ensure_record(agent, this)?;
    let cached = record.get(agent, VALIDITY)?;
    if cached != Value::UNDEFINED {
        return Ok(cached);
    }
    let proto = prototype(agent, "ValidityState")?;
    let state = Value::object(agent.new_object_with(Some(proto))?);
    with_root(agent, state, |agent| {
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.events.forms.validity, state, this)
        })?;
        record.set(agent, VALIDITY, state)
    })?;
    Ok(state)
}

fn validity_flag<const FLAG: u8>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let control = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.events.forms.validity, this))
    })?;
    if control == Value::UNDEFINED {
        return agent.type_error("expected a ValidityState receiver");
    }
    let Some(snapshot) = snapshot(agent, control)? else {
        return Ok(Value::boolean(FLAG == 7));
    };
    let flags = if will_validate(&snapshot) {
        flags(agent, control, &snapshot)?
    } else {
        Flags::default()
    };
    Ok(Value::boolean(match FLAG {
        0 => flags.value_missing,
        1 => flags.type_mismatch,
        2 => flags.too_long,
        3 => flags.too_short,
        4 => flags.range_underflow,
        5 => flags.range_overflow,
        6 => flags.custom_error,
        _ => flags.valid(),
    }))
}

fn always_false(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    // patternMismatch, stepMismatch and badInput are not computed (TD-154).
    validity_flag::<6>(agent, this, args)?;
    Ok(Value::FALSE)
}

fn is_form_element(agent: &mut Agent, this: Value) -> JsResult<bool> {
    read_node(agent, this, |_, _, _, _, node| {
        Ok(html_element(node).is_some_and(|element| element.name().local() == "form"))
    })
}

fn form_receiver(agent: &mut Agent, this: Value) -> JsResult<()> {
    if is_form_element(agent, this)? {
        Ok(())
    } else {
        agent.type_error("expected an HTMLFormElement receiver")
    }
}

fn form_submit(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    form_receiver(agent, this)?;
    submit(agent, this, None, true)?;
    Ok(Value::UNDEFINED)
}

fn form_request_submit(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    form_receiver(agent, this)?;
    let submitter = args.first().copied().unwrap_or(Value::UNDEFINED);
    let submitter = if submitter.is_nullish() {
        None
    } else {
        let valid = read_node(agent, submitter, |agent, _, document, id, node| {
            let submit_button = kind(node).is_some_and(is_submit_button);
            let owner = form_owner(agent, document, id)?;
            Ok((submit_button, owner))
        })?;
        let form_id = read_node(agent, this, |_, _, _, id, _| Ok(id))?;
        if !valid.0 {
            return agent.type_error("the submitter is not a submit button");
        }
        if valid.1 != Some(form_id) {
            return Err(super::super::creation::exception(
                agent,
                "NotFoundError",
                "the submitter's form owner is not this form",
            ));
        }
        Some(submitter)
    };
    submit(agent, this, submitter, false)?;
    Ok(Value::UNDEFINED)
}

fn form_reset(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    form_receiver(agent, this)?;
    reset(agent, this)?;
    Ok(Value::UNDEFINED)
}

fn form_method(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, this, |agent, _, _, _, node| {
        let Some(element) = html_element(node) else {
            return agent.type_error("expected an HTML element");
        };
        let method = attribute(element, "method")
            .and_then(|value| {
                ["post", "dialog"]
                    .into_iter()
                    .find(|name| equals_ascii_lower(value, name))
            })
            .unwrap_or("get");
        agent.string(method)
    })
}

fn set_form_method(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let value = to_dom_string(agent, value, DomStringConversion::Default)?;
    set_content(agent, this, "method", Some(value))
}

static ELEMENTS: LiveIndexAccess = LiveIndexAccess {
    length: elements_length,
    item: elements_item,
    named: None,
};

fn elements_length(agent: &mut Agent, form: Value) -> JsResult<u32> {
    let controls = controls_of(agent, form)?;
    controls.len(agent)
}

fn elements_item(agent: &mut Agent, form: Value, index: u32) -> JsResult<Option<Value>> {
    let controls = controls_of(agent, form)?;
    if index >= controls.len(agent)? {
        return Ok(None);
    }
    controls.get(agent, index).map(Some)
}

fn form_elements(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    form_receiver(agent, this)?;
    let cached = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.events.forms.elements, this))
    })?;
    if cached != Value::UNDEFINED {
        return Ok(cached);
    }
    let proto = prototype(agent, "HTMLCollection")?;
    let view = Value::object(agent.new_indexed_view("HTMLCollection", proto, this, &ELEMENTS)?);
    with_root(agent, view, |agent| {
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.events.forms.elements, this, view)
        })
    })?;
    Ok(view)
}

fn form_length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    form_receiver(agent, this)?;
    Ok(Value::number(f64::from(elements_length(agent, this)?)))
}

fn focus_method(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    focus_element(agent, this)?;
    Ok(Value::UNDEFINED)
}

fn blur_method(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let current = focused_value(agent)?;
    if current == Some(this) {
        move_focus(agent, None)?;
    }
    Ok(Value::UNDEFINED)
}

fn click_method(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    // A disabled form control does nothing (HTML `click()` step 1).
    let disabled_control = read_node(agent, this, |agent, _, document, id, node| {
        Ok(kind(node).is_some() && disabled(agent, document, id, node)?)
    })?;
    if !disabled_control {
        synthetic_click(agent, this)?;
    }
    Ok(Value::UNDEFINED)
}

fn tab_index_getter(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, this, |agent, _, document, id, node| {
        let index = tab_index(agent, document, id, node)?.unwrap_or(-1);
        Ok(Value::number(f64::from(index)))
    })
}

fn tab_index_setter(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let value = crate::webidl::to_long(
        agent,
        args.first().copied().unwrap_or(Value::UNDEFINED),
        crate::webidl::IntegerConversion::Wrap,
    )?;
    set_content(
        agent,
        this,
        "tabindex",
        Some(DomString::from(value.to_string())),
    )
}

fn active_element(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let main_document = with_page(agent, |_, page| Ok(page.root))?;
    if this != main_document {
        read_node(agent, this, |agent, _, _, _, node| {
            if matches!(node.kind(), NodeKind::Document) {
                Ok(())
            } else {
                agent.type_error("expected a Document receiver")
            }
        })?;
        return Ok(Value::NULL);
    }
    if let Some(focused) = focused_value(agent)? {
        return Ok(focused);
    }
    // No focused area: the body element, else the document element.
    let body = agent.heap.key("body")?;
    let value = agent.get(this.as_object().expect("document wrapper"), body)?;
    if value != Value::NULL {
        return Ok(value);
    }
    let key = agent.heap.key("documentElement")?;
    agent.get(this.as_object().expect("document wrapper"), key)
}

fn has_focus(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let main_document = with_page(agent, |_, page| Ok(page.root))?;
    Ok(Value::boolean(this == main_document))
}

/// Install focus members, control interfaces and the event interfaces.
pub(in crate::page) fn install(agent: &mut Agent) -> JsResult<()> {
    let html = prototype(agent, "HTMLElement")?;
    define_method(agent, html, "focus", focus_method, 0)?;
    define_method(agent, html, "blur", blur_method, 0)?;
    define_method(agent, html, "click", click_method, 0)?;
    define_accessor(
        agent,
        html,
        "tabIndex",
        tab_index_getter,
        Some(tab_index_setter),
    )?;
    let document = prototype(agent, "Document")?;
    define_accessor(agent, document, "activeElement", active_element, None)?;
    define_method(agent, document, "hasFocus", has_focus, 0)?;

    for (interface, kind) in [("FocusEvent", 0u8), ("InputEvent", 1), ("SubmitEvent", 2)] {
        let (parent, constructor, attributes): (&str, NativeFn, &[(&str, &str, NativeFn)]) =
            match kind {
                0 => (
                    "UIEvent",
                    construct::<0>,
                    &[("relatedTarget", "get relatedTarget", event_field::<0>)],
                ),
                1 => (
                    "UIEvent",
                    construct::<1>,
                    &[
                        ("data", "get data", event_field::<0>),
                        ("inputType", "get inputType", event_field::<1>),
                        ("isComposing", "get isComposing", event_field::<2>),
                    ],
                ),
                _ => (
                    "Event",
                    construct::<2>,
                    &[("submitter", "get submitter", event_field::<0>)],
                ),
            };
        install_interface(
            agent,
            interface,
            Some(parent),
            constructor,
            1,
            attributes,
            |_, _| Ok(()),
        )?;
    }

    install_interface(
        agent,
        "ValidityState",
        None,
        illegal_constructor,
        0,
        &[
            ("valueMissing", "get valueMissing", validity_flag::<0>),
            ("typeMismatch", "get typeMismatch", validity_flag::<1>),
            ("patternMismatch", "get patternMismatch", always_false),
            ("tooLong", "get tooLong", validity_flag::<2>),
            ("tooShort", "get tooShort", validity_flag::<3>),
            ("rangeUnderflow", "get rangeUnderflow", validity_flag::<4>),
            ("rangeOverflow", "get rangeOverflow", validity_flag::<5>),
            ("stepMismatch", "get stepMismatch", always_false),
            ("badInput", "get badInput", always_false),
            ("customError", "get customError", validity_flag::<6>),
            ("valid", "get valid", validity_flag::<7>),
        ],
        |_, _| Ok(()),
    )?;

    let input = prototype(agent, "HTMLInputElement")?;
    install_control(agent, input, true)?;
    define_accessor(
        agent,
        input,
        "defaultValue",
        default_value_getter,
        Some(default_value_setter),
    )?;
    for (name, get, set) in [
        (
            "placeholder",
            reflect_string::<1> as NativeFn,
            set_reflect_string::<1> as NativeFn,
        ),
        ("min", reflect_string::<2>, set_reflect_string::<2>),
        ("max", reflect_string::<3>, set_reflect_string::<3>),
        ("pattern", reflect_string::<4>, set_reflect_string::<4>),
        ("step", reflect_string::<7>, set_reflect_string::<7>),
        ("autocomplete", reflect_string::<6>, set_reflect_string::<6>),
        ("required", reflect_bool::<0>, set_reflect_bool::<0>),
        ("readOnly", reflect_bool::<1>, set_reflect_bool::<1>),
        ("multiple", reflect_bool::<3>, set_reflect_bool::<3>),
        ("formNoValidate", reflect_bool::<5>, set_reflect_bool::<5>),
        (
            "maxLength",
            long_reflect::<false>,
            set_long_reflect::<false>,
        ),
        ("minLength", long_reflect::<true>, set_long_reflect::<true>),
    ] {
        define_accessor(agent, input, name, get, Some(set))?;
    }

    install_interface(
        agent,
        "HTMLTextAreaElement",
        Some("HTMLElement"),
        illegal_constructor,
        0,
        &[],
        |agent, proto| {
            install_control(agent, proto, false)?;
            define_accessor(
                agent,
                proto,
                "defaultValue",
                default_value_getter,
                Some(default_value_setter),
            )?;
            for (name, get, set) in [
                (
                    "placeholder",
                    reflect_string::<1> as NativeFn,
                    set_reflect_string::<1> as NativeFn,
                ),
                ("required", reflect_bool::<0>, set_reflect_bool::<0>),
                ("readOnly", reflect_bool::<1>, set_reflect_bool::<1>),
                (
                    "maxLength",
                    long_reflect::<false>,
                    set_long_reflect::<false>,
                ),
                ("minLength", long_reflect::<true>, set_long_reflect::<true>),
            ] {
                define_accessor(agent, proto, name, get, Some(set))?;
            }
            define_accessor(agent, proto, "textLength", text_length, None)
        },
    )?;
    install_interface(
        agent,
        "HTMLButtonElement",
        Some("HTMLElement"),
        illegal_constructor,
        0,
        &[],
        |agent, proto| {
            install_control(agent, proto, true)?;
            define_accessor(
                agent,
                proto,
                "formNoValidate",
                reflect_bool::<5>,
                Some(set_reflect_bool::<5>),
            )
        },
    )?;
    install_interface(
        agent,
        "HTMLFormElement",
        Some("HTMLElement"),
        illegal_constructor,
        0,
        &[],
        |agent, proto| {
            for (name, method, length) in [
                ("submit", form_submit as NativeFn, 0),
                ("requestSubmit", form_request_submit, 0),
                ("reset", form_reset, 0),
                ("checkValidity", check_validity, 0),
                ("reportValidity", check_validity, 0),
            ] {
                define_method(agent, proto, name, method, length)?;
            }
            define_accessor(agent, proto, "elements", form_elements, None)?;
            define_accessor(agent, proto, "length", form_length, None)?;
            define_accessor(
                agent,
                proto,
                "name",
                reflect_string::<0>,
                Some(set_reflect_string::<0>),
            )?;
            define_accessor(
                agent,
                proto,
                "action",
                reflect_string::<5>,
                Some(set_reflect_string::<5>),
            )?;
            define_accessor(agent, proto, "method", form_method, Some(set_form_method))?;
            define_accessor(
                agent,
                proto,
                "noValidate",
                reflect_bool::<4>,
                Some(set_reflect_bool::<4>),
            )
        },
    )
}

fn text_length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let value = current_value(agent, this)?;
    Ok(Value::number(value.len() as f64))
}

/// Members shared by input, textarea and button: name, value, type, form,
/// disabled and the constraint validation API.
fn install_control(agent: &mut Agent, proto: ObjectId, typed: bool) -> JsResult<()> {
    define_accessor(
        agent,
        proto,
        "name",
        reflect_string::<0>,
        Some(set_reflect_string::<0>),
    )?;
    define_accessor(
        agent,
        proto,
        "disabled",
        reflect_bool::<2>,
        Some(set_reflect_bool::<2>),
    )?;
    define_accessor(agent, proto, "value", value_getter, Some(value_setter))?;
    define_accessor(
        agent,
        proto,
        "type",
        input_type_getter,
        typed.then_some(input_type_setter as NativeFn),
    )?;
    define_accessor(agent, proto, "form", form_getter, None)?;
    define_accessor(agent, proto, "willValidate", will_validate_getter, None)?;
    define_accessor(agent, proto, "validity", validity_getter, None)?;
    define_accessor(agent, proto, "validationMessage", validation_message, None)?;
    for (name, method, length) in [
        ("checkValidity", check_validity as NativeFn, 0),
        ("reportValidity", check_validity, 0),
        ("setCustomValidity", set_custom_validity, 1),
    ] {
        define_method(agent, proto, name, method, length)?;
    }
    Ok(())
}
