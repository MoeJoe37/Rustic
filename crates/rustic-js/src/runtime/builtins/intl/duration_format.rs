//! Intl.DurationFormat (ECMA-402 §13): each unit of a duration through
//! a number format, joined as a unit list.

use std::sync::Arc;

use super::super::{arg, install_methods, method, new_constructor, prototype_from_new_target};
use super::decimal::{Decimal, Numeric, RoundingMode};
use super::number_data::{Symbol, data as number_data};
use super::number_format::{charge_parts, is_type, resolve_system};
use super::numfmt::{Format, Part, Source, Style};
use super::service::{
    self, best_fit, choice, choice_option, new_service, number_option, options_object, range_error,
    set, set_text, string_option,
};
use super::{canonicalize_locale_list, data as list_data, matching, slot};
use crate::runtime::{Agent, JsResult, ObjectId, PropertyDescriptor, PropertyKey, Value};

const UNITS: [&str; 10] = [
    "years",
    "months",
    "weeks",
    "days",
    "hours",
    "minutes",
    "seconds",
    "milliseconds",
    "microseconds",
    "nanoseconds",
];
/// The order `ToDurationRecord` reads the fields in (§13.5.3).
const READ_ORDER: [usize; 10] = [3, 4, 8, 7, 5, 1, 9, 6, 2, 0];
const BASE_STYLES: &[&str] = &["long", "short", "narrow", "digital"];
/// Unit styles; a unit accepts a prefix of this list.
const STYLES: [&str; 6] = [
    "long",
    "short",
    "narrow",
    "numeric",
    "2-digit",
    "fractional",
];
const NUMERIC: usize = 3;
const TWO_DIGIT: usize = 4;
const FRACTIONAL: usize = 5;
const DISPLAYS: &[&str] = &["auto", "always"];

struct DurationFormat {
    locale: String,
    /// The data locale of the number formats.
    row: u32,
    language: &'static str,
    system: u32,
    /// The row of the list patterns.
    list_group: usize,
    style: usize,
    /// Per unit: an index into [`STYLES`] and whether it always shows.
    units: [(usize, bool); 10],
    fractional_digits: Option<u32>,
}

pub(super) fn install(agent: &mut Agent, intl: ObjectId) -> JsResult<()> {
    let prototype = agent.new_object()?;
    agent.intrinsics.intl[slot::DURATION_FORMAT_PROTOTYPE] = prototype;
    let constructor = new_constructor(agent, "DurationFormat", construct, 0, prototype)?;
    let key = agent.heap.key("DurationFormat")?;
    agent.define_property_or_throw(
        intl,
        key,
        PropertyDescriptor::data_with(Value::object(constructor), true, false, true),
    )?;
    let tag = agent.string("Intl.DurationFormat")?;
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
            method("format", format, 1),
            method("formatToParts", format_to_parts, 1),
            method("resolvedOptions", resolved_options, 0),
        ],
    )
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if agent.new_target().is_none() {
        return agent.type_error("Intl.DurationFormat requires new");
    }
    let default = agent.intrinsics.intl[slot::DURATION_FORMAT_PROTOTYPE];
    let prototype = prototype_from_new_target(agent, default)?;
    let requested = canonicalize_locale_list(agent, arg(args, 0))?;
    let options = options_object(agent, arg(args, 1))?;
    let best = best_fit(agent, options)?;
    let system = string_option(agent, options, "numberingSystem", &[])?;
    if system.as_deref().is_some_and(|system| !is_type(system)) {
        return range_error(agent, "invalid numberingSystem option");
    }
    let (locale, row, language, system) = resolve_system(&requested, best, system.as_deref());
    let style = choice(agent, options, "style", BASE_STYLES, 1)?;
    let mut units = [(0, false); 10];
    let mut previous = None;
    for (index, name) in UNITS.iter().enumerate() {
        // GetDurationUnitOptions (§13.5.6).
        let clock = (4..=6).contains(&index);
        let allowed = match index {
            0..=3 => 3,
            4..=6 => 5,
            _ => 4,
        };
        let chosen = choice_option(agent, options, name, &STYLES[..allowed])?;
        let mut always = true;
        let numeric_before = previous.is_some_and(|style| style >= NUMERIC);
        let mut unit_style = match chosen {
            Some(chosen) => chosen,
            None if style == 3 => {
                if !clock {
                    always = false;
                }
                if index < 4 { 1 } else { NUMERIC }
            }
            None if numeric_before => {
                if !matches!(index, 5 | 6) {
                    always = false;
                }
                NUMERIC
            }
            None => {
                always = false;
                style
            }
        };
        if unit_style == NUMERIC && index >= 7 {
            unit_style = FRACTIONAL;
            always = false;
        }
        let display = format!("{name}Display");
        let always = choice(agent, options, &display, DISPLAYS, usize::from(always))? == 1;
        // ValidateDurationUnitStyle (§13.5.6.1).
        if always && unit_style == FRACTIONAL {
            return range_error(agent, "a fractional duration unit cannot always be shown");
        }
        if previous == Some(FRACTIONAL) && unit_style != FRACTIONAL {
            return range_error(agent, "a unit after a fractional unit must be fractional");
        }
        if numeric_before && unit_style < NUMERIC {
            return range_error(agent, "a unit after a numeric unit must be numeric");
        }
        if matches!(index, 5 | 6) && numeric_before {
            unit_style = TWO_DIGIT;
        }
        units[index] = (unit_style, always);
        previous = Some(unit_style);
    }
    let fractional_digits = number_option(agent, options, "fractionalDigits", 0, 9, None)?;
    let (_, list_group) =
        matching::resolve_locale(list_data::LIST_LOCALES, &requested, best, "en-US");
    let state = DurationFormat {
        locale,
        row,
        language,
        system,
        list_group,
        style,
        units,
        fractional_digits,
    };
    let bytes = state.locale.len();
    Ok(Value::object(new_service(agent, prototype, state, bytes)?))
}

fn supported_locales(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    service::supported_locales(agent, &number_data().locales, arg(args, 0), arg(args, 1))
}

fn this_format(agent: &mut Agent, this: Value) -> JsResult<Arc<DurationFormat>> {
    match service::service(agent, this) {
        Some(state) => Ok(state),
        None => agent.type_error("expected an Intl.DurationFormat"),
    }
}

/// `ToDurationRecord` (§13.5.3): the ten fields as integral Numbers.
fn duration_record(agent: &mut Agent, input: Value) -> JsResult<[f64; 10]> {
    let Some(object) = input.as_object() else {
        if input.as_string().is_some() {
            return range_error(agent, "duration strings are not supported");
        }
        return agent.type_error("a duration must be an object");
    };
    let mut record = [0.0; 10];
    let mut any = false;
    for index in READ_ORDER {
        let key = agent.heap.key(UNITS[index])?;
        let value = agent.get(object, key)?;
        if value == Value::UNDEFINED {
            continue;
        }
        any = true;
        let number = agent.to_number(value)?;
        if !number.is_finite() || number.fract() != 0.0 {
            return range_error(agent, "duration fields must be integers");
        }
        record[index] = number + 0.0;
    }
    if !any {
        return agent.type_error("a duration needs at least one field");
    }
    // IsValidDuration (§13.5.2).
    let negative = record.iter().any(|&value| value < 0.0);
    let positive = record.iter().any(|&value| value > 0.0);
    let calendar_limit = 4_294_967_296.0;
    let seconds = record[3] as i128 * 86_400
        + record[4] as i128 * 3_600
        + record[5] as i128 * 60
        + record[6] as i128;
    let nanoseconds = seconds * 1_000_000_000
        + record[7] as i128 * 1_000_000
        + record[8] as i128 * 1_000
        + record[9] as i128;
    let limit = 9_007_199_254_740_992_i128 * 1_000_000_000;
    if (negative && positive)
        || record[..3]
            .iter()
            .any(|value| value.abs() >= calendar_limit)
        || nanoseconds.abs() >= limit
    {
        return range_error(agent, "invalid duration");
    }
    Ok(record)
}

/// An integer of nanosecond-scale units as a decimal with `exponent`
/// fraction digits.
fn fraction(total: i128, exponent: i32, negative: bool) -> Numeric {
    let mut value = match Numeric::from_integer_text(&total.unsigned_abs().to_string()) {
        Numeric::Finite(value) => value,
        _ => Decimal::zero(false),
    };
    value.shift(-exponent);
    value.negative = negative;
    Numeric::Finite(value)
}

impl DurationFormat {
    /// The number format of one unit.
    fn number_format(&self, index: usize, fractional: bool, signed: bool) -> Format {
        let mut format = Format::plain(self.row, self.language, self.system);
        let (style, _) = self.units[index];
        if fractional {
            format.digits.maximum_fraction = self.fractional_digits.unwrap_or(9);
            format.digits.minimum_fraction = self.fractional_digits.unwrap_or(0);
            format.digits.mode = RoundingMode::Trunc;
        }
        if !signed {
            format.sign = 1;
        }
        if style == TWO_DIGIT {
            format.digits.minimum_integer = 2;
        }
        if style < NUMERIC {
            format.style = Style::Unit;
            format.unit = UNITS[index].trim_end_matches('s').to_owned();
            format.unit_display = [2, 0, 1][style];
        } else {
            format.grouping = 0;
        }
        format
    }

    /// `PartitionDurationFormatPattern` (§13.5.10) before the list is
    /// joined: one group of parts per list element. A part's source is
    /// `Start` when it carries the unit at the same index of the result.
    fn groups(&self, record: &[f64; 10]) -> Vec<Vec<(Part, Option<usize>)>> {
        let negative = record.iter().any(|&value| value < 0.0);
        let mut result: Vec<Vec<(Part, Option<usize>)>> = Vec::new();
        let mut need_separator = false;
        let mut signed = true;
        for index in 0..10 {
            let (style, always) = self.units[index];
            let mut zero = record[index] == 0.0;
            let mut value = Numeric::from_number(record[index].abs());
            let mut done = false;
            let mut fractional = false;
            if (6..=8).contains(&index) && self.units[index + 1].0 == FRACTIONAL {
                // The smaller units are this unit's fraction.
                let exponent = [9, 6, 3][index - 6];
                let scale = [1_000_000_000_i128, 1_000_000, 1_000];
                let total: i128 = (index..10)
                    .map(|unit| {
                        record[unit] as i128 * scale[index - 6]
                            / 1000_i128.pow((unit - index) as u32)
                    })
                    .sum();
                zero = total == 0;
                value = fraction(total, exponent, false);
                fractional = true;
                done = true;
            }
            let required = index == 5
                && need_separator
                && (self.units[6].1 || record[6..].iter().any(|&value| value != 0.0));
            if !zero || always || required {
                let shows_sign = signed;
                signed = false;
                if let Numeric::Finite(decimal) = &mut value {
                    decimal.negative = shows_sign && negative;
                }
                let format = self.number_format(index, fractional, shows_sign);
                let parts = format.format(&value);
                let mut list = if need_separator {
                    let mut list = result.pop().unwrap_or_default();
                    let separator =
                        number_data().symbol(self.row, self.system, Symbol::TimeSeparator);
                    list.push((
                        Part {
                            kind: "literal",
                            text: if separator.is_empty() { ":" } else { separator }.to_owned(),
                            source: Source::Shared,
                        },
                        None,
                    ));
                    list
                } else {
                    Vec::new()
                };
                list.extend(parts.into_iter().map(|part| (part, Some(index))));
                if !need_separator && style >= NUMERIC {
                    need_separator = true;
                }
                result.push(list);
            }
            if done {
                break;
            }
        }
        result
    }

    /// Join the groups with the locale's unit list patterns.
    fn parts(&self, record: &[f64; 10]) -> Vec<(Part, Option<usize>)> {
        let groups = self.groups(record);
        // List styles are long, short, narrow; digital joins as short.
        let list_style = if self.style == 3 { 1 } else { self.style };
        let templates = &list_data::LIST_PATTERNS[self.list_group][2 * 3 + list_style];
        let count = groups.len();
        let template_at = |index: usize| match (count, index) {
            (2, _) => templates[0],
            (_, 0) => templates[1],
            (_, index) if index + 2 == count => templates[3],
            _ => templates[2],
        };
        let literal = |text: &str| {
            (
                Part {
                    kind: "literal",
                    text: text.to_owned(),
                    source: Source::Shared,
                },
                None,
            )
        };
        let mut output = Vec::new();
        let mut suffixes = Vec::new();
        for (index, group) in groups.into_iter().enumerate() {
            if index + 1 < count {
                let template = template_at(index);
                let (prefix, rest) = template.split_once("{0}").unwrap_or(("", template));
                let (infix, suffix) = rest.split_once("{1}").unwrap_or((rest, ""));
                if !prefix.is_empty() {
                    output.push(literal(prefix));
                }
                output.extend(group);
                if !infix.is_empty() {
                    output.push(literal(infix));
                }
                suffixes.push(suffix);
            } else {
                output.extend(group);
            }
        }
        for suffix in suffixes.into_iter().rev() {
            if !suffix.is_empty() {
                output.push(literal(suffix));
            }
        }
        output
    }
}

fn partition(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
) -> JsResult<Vec<(Part, Option<usize>)>> {
    let state = this_format(agent, this)?;
    let record = duration_record(agent, arg(args, 0))?;
    let parts = state.parts(&record);
    let plain: Vec<Part> = parts.iter().map(|(part, _)| part.clone()).collect();
    charge_parts(agent, &plain)?;
    Ok(parts)
}

fn format(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let parts = partition(agent, this, args)?;
    let text: String = parts.iter().map(|(part, _)| part.text.as_str()).collect();
    agent.string(&text)
}

fn format_to_parts(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let parts = partition(agent, this, args)?;
    let mut values = Vec::with_capacity(parts.len());
    for (part, unit) in &parts {
        let object = agent.new_object()?;
        set_text(agent, object, "type", part.kind)?;
        set_text(agent, object, "value", &part.text)?;
        if let Some(unit) = unit {
            set_text(agent, object, "unit", UNITS[*unit].trim_end_matches('s'))?;
        }
        values.push(Value::object(object));
    }
    Ok(Value::object(agent.create_array_from_list(&values)?))
}

fn resolved_options(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let state = this_format(agent, this)?;
    let object = agent.new_object()?;
    set_text(agent, object, "locale", &state.locale)?;
    let system = number_data().numbering_system_name(state.system);
    set_text(agent, object, "numberingSystem", system)?;
    set_text(agent, object, "style", BASE_STYLES[state.style])?;
    for (index, name) in UNITS.iter().enumerate() {
        let (style, always) = state.units[index];
        let style = if style == FRACTIONAL { NUMERIC } else { style };
        set_text(agent, object, name, STYLES[style])?;
        set_text(
            agent,
            object,
            &format!("{name}Display"),
            DISPLAYS[usize::from(always)],
        )?;
    }
    if let Some(digits) = state.fractional_digits {
        set(
            agent,
            object,
            "fractionalDigits",
            Value::number(f64::from(digits)),
        )?;
    }
    Ok(Value::object(object))
}
