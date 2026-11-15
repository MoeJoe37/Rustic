//! Intl.RelativeTimeFormat (ECMA-402 §18) over the relative time
//! patterns of every CLDR locale (`relative.bin`).

use std::sync::{Arc, OnceLock};

use super::super::{arg, install_methods, method, new_constructor, prototype_from_new_target};
use super::canonicalize_locale_list;
use super::decimal::Numeric;
use super::number_data::{OTHER, data as number_data};
use super::number_format::{charge_parts, is_type, resolve_system};
use super::numfmt::{Format, Part, Source};
use super::pack::{Pack, Table};
use super::service::{
    self, Resolution, best_fit, choice, coerce_options, new_service, range_error, set_text,
    string_option, text_of,
};
use super::slot;
use crate::runtime::{Agent, JsResult, ObjectId, PropertyDescriptor, PropertyKey, Value};

static PACK: Pack = Pack::new(include_bytes!("../../../../data/intl/relative.bin"));

const UNITS: [&str; 8] = [
    "second", "minute", "hour", "day", "week", "month", "quarter", "year",
];
const STYLES: &[&str] = &["long", "short", "narrow"];
const NUMERICS: &[&str] = &["always", "auto"];
/// Added to a plural category for the patterns of the past.
const PAST: u32 = 8;
/// Added to a day offset for a named day such as "yesterday".
const NAMED: i32 = 18;

struct Data {
    locales: Table,
    relative: Table,
}

fn data() -> &'static Data {
    static DATA: OnceLock<Data> = OnceLock::new();
    DATA.get_or_init(|| Data {
        locales: PACK.table("locales"),
        relative: PACK.table("relative"),
    })
}

impl Data {
    /// The text for `kind` of `unit`; a width without data of its own
    /// uses the next longer one.
    fn text(&self, locale: u32, unit: usize, width: usize, kind: u32) -> Option<&'static str> {
        (0..=width).rev().find_map(|width| {
            let key = (unit as u32) << 8 | (width as u32) << 6 | kind;
            self.relative
                .inherited(&self.locales, locale, key, &[])
                .map(|id| PACK.text(id))
        })
    }
}

struct RelativeTimeFormat {
    locale: String,
    /// The data locale's row in the relative time pack.
    row: u32,
    style: usize,
    auto: bool,
    number: Format,
}

pub(super) fn install(agent: &mut Agent, intl: ObjectId) -> JsResult<()> {
    let prototype = agent.new_object()?;
    agent.intrinsics.intl[slot::RELATIVE_TIME_FORMAT_PROTOTYPE] = prototype;
    let constructor = new_constructor(agent, "RelativeTimeFormat", construct, 0, prototype)?;
    let key = agent.heap.key("RelativeTimeFormat")?;
    agent.define_property_or_throw(
        intl,
        key,
        PropertyDescriptor::data_with(Value::object(constructor), true, false, true),
    )?;
    let tag = agent.string("Intl.RelativeTimeFormat")?;
    agent.define_property_or_throw(
        prototype,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    install_methods(
        agent,
        constructor,
        &[method("supportedLocalesOf", supported_locales, 1)],
    )?;
    install_methods(
        agent,
        prototype,
        &[
            method("format", format, 2),
            method("formatToParts", format_to_parts, 2),
            method("resolvedOptions", resolved_options, 0),
        ],
    )
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if agent.new_target().is_none() {
        return agent.type_error("Intl.RelativeTimeFormat requires new");
    }
    let default = agent.intrinsics.intl[slot::RELATIVE_TIME_FORMAT_PROTOTYPE];
    let prototype = prototype_from_new_target(agent, default)?;
    let requested = canonicalize_locale_list(agent, arg(args, 0))?;
    let options = coerce_options(agent, arg(args, 1))?;
    let best = best_fit(agent, options)?;
    let system = string_option(agent, options, "numberingSystem", &[])?;
    if system.as_deref().is_some_and(|system| !is_type(system)) {
        return range_error(agent, "invalid numberingSystem option");
    }
    let (locale, number_row, language, system) =
        resolve_system(&requested, best, system.as_deref());
    let row = Resolution::new(&data().locales, &requested, best).row;
    let style = choice(agent, options, "style", STYLES, 0)?;
    let auto = choice(agent, options, "numeric", NUMERICS, 0)? == 1;
    let state = RelativeTimeFormat {
        locale,
        row,
        style,
        auto,
        number: Format::plain(number_row, language, system),
    };
    let bytes = state.locale.len();
    Ok(Value::object(new_service(agent, prototype, state, bytes)?))
}

fn supported_locales(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    service::supported_locales(agent, &data().locales, arg(args, 0), arg(args, 1))
}

fn this_format(agent: &mut Agent, this: Value) -> JsResult<Arc<RelativeTimeFormat>> {
    match service::service(agent, this) {
        Some(state) => Ok(state),
        None => agent.type_error("expected an Intl.RelativeTimeFormat"),
    }
}

/// `PartitionRelativeTimePattern` (§18.5.2): the parts and the unit
/// that the parts of the number carry.
fn partition(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
) -> JsResult<(Vec<Part>, &'static str)> {
    let state = this_format(agent, this)?;
    let value = agent.to_number(arg(args, 0))?;
    let unit = text_of(agent, arg(args, 1))?;
    if !value.is_finite() {
        return range_error(agent, "a relative time must be finite");
    }
    // SingularRelativeTimeUnit (§18.5.1).
    let name = unit.strip_suffix('s').unwrap_or(&unit);
    let Some(unit) = UNITS.iter().position(|known| *known == name) else {
        return range_error(agent, "invalid relative time unit");
    };
    let data = data();
    let literal = |text: &str| Part {
        kind: "literal",
        text: text.to_owned(),
        source: Source::Shared,
    };
    if state.auto && value.fract() == 0.0 && value.abs() <= 3.0 {
        let named = (NAMED + value as i32) as u32;
        if let Some(text) = data.text(state.row, unit, state.style, named) {
            return Ok((vec![literal(text)], UNITS[unit]));
        }
    }
    let past = if value.is_sign_negative() { PAST } else { 0 };
    let (number, category) = state
        .number
        .format_plural(&Numeric::from_number(value.abs()));
    let pattern = data
        .text(state.row, unit, state.style, past + category as u32)
        .or_else(|| data.text(state.row, unit, state.style, past + OTHER as u32))
        .unwrap_or("{0}");
    let (before, after) = pattern.split_once("{0}").unwrap_or((pattern, ""));
    let mut parts = Vec::new();
    if !before.is_empty() {
        parts.push(literal(before));
    }
    // The parts of the number are marked with a source to tell them
    // from the literals around them.
    parts.extend(number.into_iter().map(|part| Part {
        source: Source::Start,
        ..part
    }));
    if !after.is_empty() {
        parts.push(literal(after));
    }
    charge_parts(agent, &parts)?;
    Ok((parts, UNITS[unit]))
}

fn format(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (parts, _) = partition(agent, this, args)?;
    let text: String = parts.iter().map(|part| part.text.as_str()).collect();
    agent.string(&text)
}

fn format_to_parts(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (parts, unit) = partition(agent, this, args)?;
    let mut values = Vec::with_capacity(parts.len());
    for part in &parts {
        let object = agent.new_object()?;
        set_text(agent, object, "type", part.kind)?;
        set_text(agent, object, "value", &part.text)?;
        if part.source == Source::Start {
            set_text(agent, object, "unit", unit)?;
        }
        values.push(Value::object(object));
    }
    Ok(Value::object(agent.create_array_from_list(&values)?))
}

fn resolved_options(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let state = this_format(agent, this)?;
    let object = agent.new_object()?;
    set_text(agent, object, "locale", &state.locale)?;
    set_text(agent, object, "style", STYLES[state.style])?;
    set_text(agent, object, "numeric", NUMERICS[usize::from(state.auto)])?;
    let system = number_data().numbering_system_name(state.number.system);
    set_text(agent, object, "numberingSystem", system)?;
    Ok(Value::object(object))
}
