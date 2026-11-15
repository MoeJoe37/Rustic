//! Intl.NumberFormat (ECMA-402 §16) and the `toLocaleString` of Number
//! and BigInt, on the engine in `numfmt`.

use std::sync::Arc;

use super::super::{arg, install_methods, method, new_constructor, prototype_from_new_target};
use super::decimal::Numeric;
use super::number_data::data;
use super::numfmt::{
    CURRENCY_DISPLAYS, Format, GROUPINGS, NOTATIONS, Notation, Part, SIGN_DISPLAYS, STYLES, Style,
    UNIT_DISPLAYS, text_of,
};
use super::service::{
    self, Resolution, best_fit, bound, captured, choice, closure, coerce_options, digit_options,
    digit_properties, new_service, option, range_error, rounding_properties, set, set_bound,
    set_text, string_option, text_of as text_value,
};
use super::{canonicalize_locale_list, slot};
use crate::runtime::{
    Agent, JsResult, Kind, ObjectId, OutOfMemory, PreferredType, PropertyDescriptor, PropertyKey,
    Value,
};

/// The units ECMA-402 sanctions (§6.6.2, table 2).
const UNITS: &[&str] = &[
    "acre",
    "bit",
    "byte",
    "celsius",
    "centimeter",
    "day",
    "degree",
    "fahrenheit",
    "fluid-ounce",
    "foot",
    "gallon",
    "gigabit",
    "gigabyte",
    "gram",
    "hectare",
    "hour",
    "inch",
    "kilobit",
    "kilobyte",
    "kilogram",
    "kilometer",
    "liter",
    "megabit",
    "megabyte",
    "meter",
    "microsecond",
    "mile",
    "mile-scandinavian",
    "milliliter",
    "millimeter",
    "millisecond",
    "minute",
    "month",
    "nanosecond",
    "ounce",
    "percent",
    "petabyte",
    "pound",
    "second",
    "stone",
    "terabit",
    "terabyte",
    "week",
    "yard",
    "year",
];

/// `IsWellFormedUnitIdentifier` (§6.6.1).
fn is_unit(text: &str) -> bool {
    if UNITS.contains(&text) {
        return true;
    }
    text.split_once("-per-")
        .is_some_and(|(top, bottom)| UNITS.contains(&top) && UNITS.contains(&bottom))
}

/// `IsWellFormedCurrencyCode` (§6.3.1).
fn is_currency(text: &str) -> bool {
    text.len() == 3 && text.bytes().all(|byte| byte.is_ascii_alphabetic())
}

/// Whether `text` has the form of a Unicode extension `type`: runs of 3
/// to 8 letters and digits joined by hyphens.
fn is_type(text: &str) -> bool {
    text.split('-').all(|piece| {
        (3..=8).contains(&piece.len()) && piece.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

/// The settings of a number format object.
struct NumberFormat {
    locale: String,
    format: Format,
}

/// `ResolveLocale` with the `nu` key against the number data.
fn resolve_system(
    requested: &[String],
    best: bool,
    system: Option<&str>,
) -> (String, u32, &'static str, u32) {
    let data = data();
    let mut resolution = Resolution::new(&data.locales, requested, best);
    let default = data.numbering_system_name(data.default_numbering_system(resolution.row));
    let system = system.map(str::to_ascii_lowercase);
    let name = resolution.key(
        "nu",
        system.as_deref(),
        |name| data.numbering_system(name).is_some(),
        default,
    );
    let number = data
        .numbering_system(&name)
        .unwrap_or_else(|| data.default_numbering_system(resolution.row));
    (resolution.locale(), resolution.row, resolution.base, number)
}

/// `InitializeNumberFormat`'s reading of `locales` and `options`
/// (§16.1.1 steps 3 to 31).
fn create(agent: &mut Agent, locales: Value, options: Value) -> JsResult<NumberFormat> {
    let requested = canonicalize_locale_list(agent, locales)?;
    let options = coerce_options(agent, options)?;
    let best = best_fit(agent, options)?;
    let system = string_option(agent, options, "numberingSystem", &[])?;
    if system.as_deref().is_some_and(|system| !is_type(system)) {
        return range_error(agent, "invalid numberingSystem option");
    }
    let (locale, row, language, system) = resolve_system(&requested, best, system.as_deref());
    // SetNumberFormatUnitOptions (§16.1.2).
    let style = [Style::Decimal, Style::Percent, Style::Currency, Style::Unit]
        [choice(agent, options, "style", STYLES, 0)?];
    let currency = string_option(agent, options, "currency", &[])?;
    match &currency {
        None if style == Style::Currency => {
            return agent.type_error("the currency style needs a currency");
        }
        Some(code) if !is_currency(code) => return range_error(agent, "invalid currency code"),
        _ => {}
    }
    let currency_display = choice(agent, options, "currencyDisplay", CURRENCY_DISPLAYS, 1)?;
    let accounting = choice(
        agent,
        options,
        "currencySign",
        &["standard", "accounting"],
        0,
    )? == 1;
    let unit = string_option(agent, options, "unit", &[])?;
    match &unit {
        None if style == Style::Unit => return agent.type_error("the unit style needs a unit"),
        Some(unit) if !is_unit(unit) => return range_error(agent, "invalid unit identifier"),
        _ => {}
    }
    let unit_display = choice(agent, options, "unitDisplay", UNIT_DISPLAYS, 0)?;
    let currency = currency
        .filter(|_| style == Style::Currency)
        .map(|code| code.to_ascii_uppercase())
        .unwrap_or_default();
    let unit = unit.filter(|_| style == Style::Unit).unwrap_or_default();
    let notation = [
        Notation::Standard,
        Notation::Scientific,
        Notation::Engineering,
        Notation::Compact,
    ][choice(agent, options, "notation", NOTATIONS, 0)?];
    let (minimum, maximum) = if style == Style::Currency && notation == Notation::Standard {
        let digits = data().currency_digits(&currency);
        (digits, digits)
    } else if style == Style::Percent {
        (0, 0)
    } else {
        (0, 3)
    };
    let compact = notation == Notation::Compact;
    let digits = digit_options(agent, options, minimum, maximum, compact)?;
    let compact_long = choice(agent, options, "compactDisplay", &["short", "long"], 0)? == 1;
    let default_grouping = if compact { 1 } else { 2 };
    // GetBooleanOrStringNumberFormatOption (§16.1.4).
    let value = option(agent, options, "useGrouping")?;
    let grouping = if value == Value::UNDEFINED {
        default_grouping
    } else if value == Value::TRUE {
        3
    } else if !agent.to_boolean(value) {
        0
    } else {
        let text = text_value(agent, value)?;
        match text.as_str() {
            "min2" => 1,
            "auto" => 2,
            "always" => 3,
            "true" | "false" => default_grouping,
            _ => return range_error(agent, "invalid useGrouping option"),
        }
    };
    let sign = choice(agent, options, "signDisplay", SIGN_DISPLAYS, 0)?;
    Ok(NumberFormat {
        locale,
        format: Format {
            row,
            language,
            system,
            style,
            currency,
            currency_display,
            accounting,
            unit,
            unit_display,
            notation,
            compact_long,
            digits,
            grouping,
            sign,
        },
    })
}

pub(super) fn install(agent: &mut Agent, intl: ObjectId) -> JsResult<()> {
    let prototype = agent.new_object()?;
    agent.intrinsics.intl[slot::NUMBER_FORMAT_PROTOTYPE] = prototype;
    let constructor = new_constructor(agent, "NumberFormat", construct, 0, prototype)?;
    agent.intrinsics.intl[slot::NUMBER_FORMAT] = constructor;
    let key = agent.heap.key("NumberFormat")?;
    agent.define_property_or_throw(
        intl,
        key,
        PropertyDescriptor::data_with(Value::object(constructor), true, false, true),
    )?;
    let tag = agent.string("Intl.NumberFormat")?;
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
    let getter = agent.new_native_function(format_getter, "get format", 0)?;
    let key = agent.heap.key("format")?;
    agent.define_property_or_throw(
        prototype,
        key,
        PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, false, true),
    )?;
    install_methods(
        agent,
        prototype,
        &[
            method("formatRange", format_range, 2),
            method("formatRangeToParts", format_range_to_parts, 2),
            method("formatToParts", format_to_parts, 1),
            method("resolvedOptions", resolved_options, 0),
        ],
    )
}

fn construct(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let default = agent.intrinsics.intl[slot::NUMBER_FORMAT_PROTOTYPE];
    let prototype = prototype_from_new_target(agent, default)?;
    let state = create(agent, arg(args, 0), arg(args, 1))?;
    let bytes = state.locale.len() + state.format.currency.len() + state.format.unit.len();
    let object = new_service(agent, prototype, state, bytes)?;
    service::legacy_result(agent, this, object, slot::NUMBER_FORMAT)
}

fn supported_locales(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    service::supported_locales(agent, &data().locales, arg(args, 0), arg(args, 1))
}

fn this_format(agent: &mut Agent, this: Value) -> JsResult<Arc<NumberFormat>> {
    match service::service(agent, this) {
        Some(state) => Ok(state),
        None => agent.type_error("expected an Intl.NumberFormat"),
    }
}

/// `UnwrapNumberFormat` (§16.5.2).
fn unwrap(agent: &mut Agent, this: Value) -> JsResult<(ObjectId, Arc<NumberFormat>)> {
    let value = service::unwrap_legacy::<NumberFormat>(agent, this, slot::NUMBER_FORMAT)?;
    match (value.as_object(), service::service(agent, value)) {
        (Some(object), Some(state)) => Ok((object, state)),
        _ => agent.type_error("expected an Intl.NumberFormat"),
    }
}

/// `ToIntlMathematicalValue` (§16.5.16).
fn mathematical_value(agent: &mut Agent, value: Value) -> JsResult<Numeric> {
    let primitive = agent.to_primitive(value, Some(PreferredType::Number))?;
    match primitive.kind() {
        Kind::BigInt(id) => {
            // Printing in decimal is not linear: pay for it up front.
            let bits =
                usize::try_from(agent.heap.bigint(id).bit_length()).map_err(|_| OutOfMemory)?;
            agent.heap.charge(bits / 2)?;
            for _ in 0..bits {
                agent.check_native_work()?;
            }
            let text = agent.heap.bigint(id).to_string_radix10();
            Ok(Numeric::from_integer_text(&text))
        }
        Kind::String(id) => {
            let length = agent.heap.string_len(id);
            agent
                .heap
                .charge(length.checked_mul(4).ok_or(OutOfMemory)?)?;
            for _ in 0..length {
                agent.check_native_work()?;
            }
            Ok(Numeric::from_string(agent.heap.string(id)))
        }
        _ => Ok(Numeric::from_number(agent.to_number(primitive)?)),
    }
}

fn charge_parts(agent: &mut Agent, parts: &[Part]) -> JsResult<()> {
    let bytes: usize = parts
        .iter()
        .map(|part| part.text.len() + std::mem::size_of::<Part>())
        .sum();
    agent.heap.charge(bytes)?;
    Ok(())
}

fn format_value(agent: &mut Agent, format: &Format, x: &Numeric) -> JsResult<Value> {
    let parts = format.format(x);
    charge_parts(agent, &parts)?;
    agent.string(&text_of(&parts))
}

/// `[{ type, value }]`, with `source` for the parts of a range.
fn parts_array(agent: &mut Agent, parts: &[Part], ranged: bool) -> JsResult<Value> {
    charge_parts(agent, parts)?;
    let mut values = Vec::with_capacity(parts.len());
    for part in parts {
        agent.check_native_work()?;
        let object = agent.new_object()?;
        set_text(agent, object, "type", part.kind)?;
        set_text(agent, object, "value", &part.text)?;
        if ranged {
            set_text(agent, object, "source", part.source.name())?;
        }
        values.push(Value::object(object));
    }
    Ok(Value::object(agent.create_array_from_list(&values)?))
}

fn format_getter(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (object, _) = unwrap(agent, this)?;
    let existing = bound(agent, object);
    if existing != Value::UNDEFINED {
        return Ok(existing);
    }
    let function = closure(agent, bound_format, 1, Value::object(object))?;
    set_bound(agent, object, Value::object(function));
    Ok(Value::object(function))
}

fn bound_format(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let target = captured(agent);
    let state = this_format(agent, target)?;
    let x = mathematical_value(agent, arg(args, 0))?;
    format_value(agent, &state.format, &x)
}

fn format_to_parts(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let state = this_format(agent, this)?;
    let x = mathematical_value(agent, arg(args, 0))?;
    let parts = state.format.format(&x);
    parts_array(agent, &parts, false)
}

fn range_parts(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Vec<Part>> {
    let state = this_format(agent, this)?;
    let (start, end) = (arg(args, 0), arg(args, 1));
    if start == Value::UNDEFINED || end == Value::UNDEFINED {
        return agent.type_error("a number range needs a start and an end");
    }
    let x = mathematical_value(agent, start)?;
    let y = mathematical_value(agent, end)?;
    if x == Numeric::NaN || y == Numeric::NaN {
        return range_error(agent, "a number range cannot include NaN");
    }
    let parts = state.format.format_range(&x, &y);
    charge_parts(agent, &parts)?;
    Ok(parts)
}

fn format_range(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let parts = range_parts(agent, this, args)?;
    agent.string(&text_of(&parts))
}

fn format_range_to_parts(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let parts = range_parts(agent, this, args)?;
    parts_array(agent, &parts, true)
}

fn resolved_options(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, state) = unwrap(agent, this)?;
    let format = &state.format;
    let object = agent.new_object()?;
    set_text(agent, object, "locale", &state.locale)?;
    let system = data().numbering_system_name(format.system);
    set_text(agent, object, "numberingSystem", system)?;
    set_text(agent, object, "style", STYLES[format.style as usize])?;
    if format.style == Style::Currency {
        set_text(agent, object, "currency", &format.currency)?;
        let display = CURRENCY_DISPLAYS[format.currency_display];
        set_text(agent, object, "currencyDisplay", display)?;
        let sign = if format.accounting {
            "accounting"
        } else {
            "standard"
        };
        set_text(agent, object, "currencySign", sign)?;
    }
    if format.style == Style::Unit {
        set_text(agent, object, "unit", &format.unit)?;
        set_text(
            agent,
            object,
            "unitDisplay",
            UNIT_DISPLAYS[format.unit_display],
        )?;
    }
    digit_properties(agent, object, &format.digits)?;
    if format.grouping == 0 {
        set(agent, object, "useGrouping", Value::FALSE)?;
    } else {
        set_text(agent, object, "useGrouping", GROUPINGS[format.grouping])?;
    }
    set_text(
        agent,
        object,
        "notation",
        NOTATIONS[format.notation as usize],
    )?;
    if format.notation == Notation::Compact {
        let display = if format.compact_long { "long" } else { "short" };
        set_text(agent, object, "compactDisplay", display)?;
    }
    set_text(agent, object, "signDisplay", SIGN_DISPLAYS[format.sign])?;
    rounding_properties(agent, object, &format.digits)?;
    Ok(Value::object(object))
}

/// `Number.prototype.toLocaleString` and `BigInt.prototype.toLocaleString`
/// (§19.2.1, §19.3.1): format `value` with a number format made from
/// `locales` and `options`.
pub(in super::super) fn to_locale_string(
    agent: &mut Agent,
    value: Value,
    locales: Value,
    options: Value,
) -> JsResult<Value> {
    let state = create(agent, locales, options)?;
    let x = mathematical_value(agent, value)?;
    format_value(agent, &state.format, &x)
}
