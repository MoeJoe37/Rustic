//! ECMA-402 plural categories from pinned CLDR cardinal, ordinal and range data.

mod data;
mod digits;

use super::super::{arg, install_methods, method, new_constructor, prototype_from_new_target};
use super::{canonicalize_locale_list, matching};
use crate::runtime::{
    Agent, ErrorKind, Extra, Internal, JsResult, Kind, ObjectId, OutOfMemory, PreferredType,
    PropertyDescriptor, PropertyKey, Value,
};
use digits::Decimal;

const CATEGORIES: &[&str] = &["zero", "one", "two", "few", "many", "other"];
const OTHER: usize = 5;
const TYPES: &[&str] = &["cardinal", "ordinal"];
const NOTATIONS: &[&str] = &["standard", "scientific", "engineering", "compact"];
const COMPACT_DISPLAYS: &[&str] = &["short", "long"];

type Rule = (u8, fn(&Operands) -> usize);

#[derive(Clone, Copy, Debug)]
pub(crate) struct PluralRules {
    locale: &'static str,
    group: usize,
    kind: usize,
    notation: usize,
    compact_display: usize,
    compact_group: usize,
    digits: digits::Options,
}

pub(super) fn install(agent: &mut Agent, intl: ObjectId) -> JsResult<()> {
    let prototype = agent.new_object()?;
    agent.intrinsics.plural_rules_prototype = prototype;
    let constructor = new_constructor(agent, "PluralRules", construct, 0, prototype)?;
    let name = agent.heap.key("PluralRules")?;
    agent.define_property_or_throw(
        intl,
        name,
        PropertyDescriptor::data_with(Value::object(constructor), true, false, true),
    )?;
    let tag = agent.string("Intl.PluralRules")?;
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
            method("resolvedOptions", resolved_options, 0),
            method("select", select, 1),
            method("selectRange", select_range, 2),
        ],
    )
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if agent.new_target().is_none() {
        return agent.type_error("Intl.PluralRules requires new");
    }
    let prototype = prototype_from_new_target(agent, agent.intrinsics.plural_rules_prototype)?;
    let requested = canonicalize_locale_list(agent, arg(args, 0))?;
    let options = matching::constructor_options(agent, arg(args, 1))?;
    let best_fit =
        matching::option_index(agent, options, "localeMatcher", &["best fit", "lookup"])? == 0;
    let kind = matching::option_index(agent, options, "type", TYPES)?;
    let (locale, _) =
        matching::resolve_locale(super::data::LIST_LOCALES, &requested, best_fit, "en-US");
    let table = if kind == 0 {
        data::CARDINAL_LOCALES
    } else {
        data::ORDINAL_LOCALES
    };
    let (_, group) = matching::resolve_locale(table, &[locale.to_owned()], false, "root");
    let notation = matching::option_index(agent, options, "notation", NOTATIONS)?;
    let compact_display =
        matching::option_index(agent, options, "compactDisplay", COMPACT_DISPLAYS)?;
    let (_, compact_group) =
        matching::resolve_locale(data::COMPACT_LOCALES, &[locale.to_owned()], false, "root");
    let digits = digits::options(agent, options, notation == 3)?;
    let value = PluralRules {
        locale,
        group,
        kind,
        notation,
        compact_display,
        compact_group,
        digits,
    };
    agent
        .heap
        .charge(std::mem::size_of::<Extra>() + std::mem::size_of::<PluralRules>())?;
    let object = agent.new_object_with(Some(prototype))?;
    agent.heap.object_mut(object).extra_mut().internal =
        Some(Internal::PluralRules(Box::new(value)));
    Ok(Value::object(object))
}

fn supported_locales(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    matching::supported_locales(agent, arg(args, 0), arg(args, 1), super::data::LIST_LOCALES)
}

fn branded(agent: &mut Agent, this: Value) -> JsResult<PluralRules> {
    if let Some(object) = this.as_object() {
        if let Some(Internal::PluralRules(rules)) = agent.heap.object(object).internal() {
            return Ok(**rules);
        }
    }
    agent.type_error("expected an Intl.PluralRules")
}

fn resolved_options(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let rules = branded(agent, this)?;
    let object = agent.new_object()?;
    for (name, text) in [
        ("locale", rules.locale),
        ("type", TYPES[rules.kind]),
        ("notation", NOTATIONS[rules.notation]),
    ] {
        set_string(agent, object, name, text)?;
    }
    if rules.notation == 3 {
        set_string(
            agent,
            object,
            "compactDisplay",
            COMPACT_DISPLAYS[rules.compact_display],
        )?;
    }
    let digits = rules.digits;
    set_number(agent, object, "minimumIntegerDigits", digits.integer)?;
    if let Some((min, max)) = digits.fraction {
        set_number(agent, object, "minimumFractionDigits", min)?;
        set_number(agent, object, "maximumFractionDigits", max)?;
    }
    if let Some((min, max)) = digits.significant {
        set_number(agent, object, "minimumSignificantDigits", min)?;
        set_number(agent, object, "maximumSignificantDigits", max)?;
    }
    let (mask, _) = rules.rule();
    let categories = CATEGORIES
        .iter()
        .enumerate()
        .filter(|(index, _)| mask & (1 << index) != 0)
        .map(|(_, category)| agent.string(category))
        .collect::<JsResult<Vec<_>>>()?;
    let key = agent.heap.key("pluralCategories")?;
    let array = agent.create_array_from_list(&categories)?;
    agent.create_data_property_or_throw(object, key, Value::object(array))?;
    set_number(agent, object, "roundingIncrement", digits.increment)?;
    set_string(agent, object, "roundingMode", digits::MODES[digits.mode])?;
    set_string(
        agent,
        object,
        "roundingPriority",
        digits::PRIORITIES[digits.priority],
    )?;
    set_string(
        agent,
        object,
        "trailingZeroDisplay",
        digits::TRAILING[digits.trailing],
    )?;
    Ok(Value::object(object))
}

fn set_string(agent: &mut Agent, object: ObjectId, name: &str, text: &str) -> JsResult<()> {
    let key = agent.heap.key(name)?;
    let value = agent.string(text)?;
    agent.create_data_property_or_throw(object, key, value)
}

fn set_number(agent: &mut Agent, object: ObjectId, name: &str, value: i32) -> JsResult<()> {
    let key = agent.heap.key(name)?;
    agent.create_data_property_or_throw(object, key, Value::number(f64::from(value)))
}

#[derive(PartialEq, Eq)]
enum Input {
    Finite(Decimal),
    Infinity(bool),
    Nan,
}

fn decimal_input(agent: &mut Agent, value: Value) -> JsResult<Input> {
    let value = agent.to_primitive(value, Some(PreferredType::Number))?;
    if let Kind::BigInt(id) = value.kind() {
        let integer = agent.heap.bigint(id);
        let bits = integer.bit_length();
        let room = agent
            .heap
            .byte_limit()
            .saturating_sub(agent.heap.bytes_used());
        let bound = usize::try_from(bits).map_err(|_| OutOfMemory)?;
        if bound > room / 2 {
            return Err(OutOfMemory.into());
        }
        for _ in 0..bits {
            agent.check_native_work()?;
        }
        let text = agent.heap.bigint(id).to_string_radix10();
        agent.heap.charge(text.len() * 2)?;
        return Ok(Input::Finite(parse_decimal(&text)));
    }
    if let Some(id) = value.as_string() {
        let length = agent.heap.string_len(id);
        agent
            .heap
            .charge(length.checked_mul(4).ok_or(OutOfMemory)?)?;
        for _ in 0..length {
            agent.check_native_work()?;
        }
    }
    let number = agent.to_number(value)?;
    if number.is_nan() {
        return Ok(Input::Nan);
    }
    if number.is_infinite() {
        return Ok(Input::Infinity(number.is_sign_negative()));
    }
    if number != 0.0 {
        if let Some(id) = value.as_string() {
            agent.heap.charge(
                agent
                    .heap
                    .string_len(id)
                    .checked_mul(4)
                    .ok_or(OutOfMemory)?,
            )?;
            agent.heap.flatten(id)?;
            let text = agent.heap.string(id).to_text();
            let text = text.trim_matches(|c: char| {
                crate::unicode::is_whitespace(u32::from(c))
                    || crate::unicode::is_line_terminator(u32::from(c))
            });
            if matches!(
                text.as_bytes().get(..2),
                Some([b'0', b'x' | b'X' | b'o' | b'O' | b'b' | b'B'])
            ) {
                // Nondecimal integers share the existing StringToBigInt grammar.
                let integer =
                    crate::runtime::string_to_bigint(agent.heap.string(id)).ok_or(OutOfMemory)?;
                return Ok(Input::Finite(parse_decimal(&integer.to_string_radix10())));
            }
            return Ok(Input::Finite(parse_decimal(text)));
        }
    }
    let (digits, point) = super::super::number_object::shortest_digits(number.abs());
    Ok(Input::Finite(Decimal::new(
        digits,
        point,
        number.is_sign_negative(),
    )))
}

fn parse_decimal(text: &str) -> Decimal {
    let unsigned = text.trim_start_matches(['+', '-']);
    let (mantissa, exponent) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
    let exponent = exponent.parse::<i32>().expect("finite decimal exponent");
    let integer = mantissa.find('.').unwrap_or(mantissa.len());
    let point = i32::try_from(integer).expect("bounded number input") + exponent;
    let digits = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|digit| digit - b'0')
        .collect();
    Decimal::new(digits, point, text.starts_with('-'))
}

enum Operand {
    Number,
    Integer,
    Visible,
    Fraction,
    TrimmedFraction,
    Exponent,
}

struct Operands {
    decimal: Decimal,
    visible: i32,
    exponent: i32,
}

fn operand_value(digits: impl Iterator<Item = u8>, modulo: u32) -> u32 {
    if modulo == 0 {
        digits.fold(0, |value: u32, digit| {
            value.saturating_mul(10).saturating_add(u32::from(digit))
        })
    } else {
        digits.fold(0, |remainder, digit| {
            (remainder * 10 + u32::from(digit)) % modulo
        })
    }
}

impl Operands {
    fn matches(&self, operand: Operand, modulo: u32, ranges: &[(u32, u32)]) -> bool {
        let decimal = &self.decimal;
        let fraction = decimal.fraction_digits();
        let number = match operand {
            Operand::Number if fraction != 0 => return false,
            Operand::Number | Operand::Integer => {
                operand_value(decimal.digit_positions(0, decimal.point), modulo)
            }
            Operand::Visible => self.visible as u32,
            Operand::Fraction | Operand::TrimmedFraction => {
                let count = if matches!(operand, Operand::Fraction) {
                    self.visible
                } else {
                    fraction
                };
                operand_value(
                    decimal.digit_positions(decimal.point, decimal.point + count),
                    modulo,
                )
            }
            Operand::Exponent => self.exponent as u32,
        };
        let number = if modulo == 0 { number } else { number % modulo };
        ranges
            .iter()
            .any(|&(start, end)| (start..=end).contains(&number))
    }
}

#[derive(PartialEq, Eq)]
enum Formatted {
    Finite {
        decimal: Decimal,
        visible: i32,
        scale: i32,
    },
    Infinity(bool),
    Nan,
}

impl PluralRules {
    fn rule(self) -> Rule {
        let rules = if self.kind == 0 {
            data::CARDINAL_RULES
        } else {
            data::ORDINAL_RULES
        };
        rules[self.group]
    }

    fn scale(self, decimal: &Decimal) -> i32 {
        if decimal.digits.is_empty() {
            return 0;
        }
        let magnitude = decimal.point - 1;
        match self.notation {
            1 => magnitude,
            2 => magnitude.div_euclid(3) * 3,
            3 => {
                let scales = &data::COMPACT_SCALES[self.compact_group][self.compact_display];
                let index = usize::try_from(magnitude)
                    .unwrap_or(0)
                    .min(scales.len() - 1);
                i32::from(scales[index])
            }
            _ => 0,
        }
    }

    fn format(self, input: Input) -> (usize, Formatted) {
        let mut decimal = match input {
            Input::Finite(decimal) => decimal,
            Input::Infinity(negative) => return (OTHER, Formatted::Infinity(negative)),
            Input::Nan => return (OTHER, Formatted::Nan),
        };
        let mut scale = self.scale(&decimal);
        decimal.point -= scale;
        let (mut rounded, mut visible) = self.digits.round(&decimal);
        if !rounded.digits.is_empty() {
            rounded.point += scale;
            let next_scale = self.scale(&rounded);
            if next_scale != scale {
                decimal.point += scale - next_scale;
                scale = next_scale;
                (rounded, visible) = self.digits.round(&decimal);
            } else {
                rounded.point -= scale;
            }
        }
        let formatted = Formatted::Finite {
            decimal: rounded.clone(),
            visible,
            scale,
        };
        // CLDR's compact operands use the decimal value at its original
        // magnitude, with the compact exponent available as a separate field.
        if self.notation == 3 && !rounded.digits.is_empty() {
            rounded.point += scale;
            visible = (visible - scale).max(0);
        }
        let category = self.rule().1(&Operands {
            decimal: rounded,
            visible,
            exponent: if self.notation == 3 { scale } else { 0 },
        });
        (category, formatted)
    }
}

fn select(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let rules = branded(agent, this)?;
    let number = decimal_input(agent, arg(args, 0))?;
    agent.string(CATEGORIES[rules.format(number).0])
}

fn select_range(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let rules = branded(agent, this)?;
    let (first, second) = (arg(args, 0), arg(args, 1));
    if first == Value::UNDEFINED || second == Value::UNDEFINED {
        return agent.type_error("Intl.PluralRules.selectRange requires both endpoints");
    }
    let first = decimal_input(agent, first)?;
    let second = decimal_input(agent, second)?;
    if first == Input::Nan || second == Input::Nan {
        return Err(agent.throw(ErrorKind::RangeError, "invalid Intl plural range endpoint"));
    }
    let (start, first) = rules.format(first);
    let (end, second) = rules.format(second);
    let category = if first == second {
        start
    } else {
        matching::match_locale(data::RANGE_LOCALES, rules.locale, false)
            .map_or(OTHER, |(_, group)| {
                data::RANGE_RULES[group][start * 6 + end]
            })
    };
    agent.string(CATEGORIES[category])
}
