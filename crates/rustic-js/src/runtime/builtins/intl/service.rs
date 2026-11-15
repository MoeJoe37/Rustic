//! The abstract operations ECMA-402 service constructors share (§9.2):
//! reading options, and resolving a locale and its Unicode extension
//! keys against the locales of a data pack.

use std::any::Any;
use std::sync::Arc;

use super::decimal::{DigitOptions, ROUNDING_MODES, RoundingMode, RoundingType};
use super::pack::Table;
use super::{canonicalize_locale_list, locale};
use crate::runtime::{Agent, ErrorKind, Extra, Internal, JsResult, ObjectId, Value};

/// The locale every service falls back to.
pub(super) const DEFAULT_LOCALE: &str = "en-US";

pub(super) fn range_error<T>(agent: &mut Agent, message: &str) -> JsResult<T> {
    Err(agent.throw(ErrorKind::RangeError, message))
}

/// `CoerceOptionsToObject(options)` (§9.2.11): `undefined` reads as an
/// empty object, anything else is converted.
pub(super) fn coerce_options(agent: &mut Agent, value: Value) -> JsResult<Option<ObjectId>> {
    if value == Value::UNDEFINED {
        return Ok(None);
    }
    agent.to_object(value).map(Some)
}

/// `Get(options, name)`.
pub(super) fn option(agent: &mut Agent, options: Option<ObjectId>, name: &str) -> JsResult<Value> {
    let Some(options) = options else {
        return Ok(Value::UNDEFINED);
    };
    let key = agent.heap.key(name)?;
    agent.get(options, key)
}

/// `ToString(value)` as Rust text, charged to the heap while it lives.
pub(super) fn text_of(agent: &mut Agent, value: Value) -> JsResult<String> {
    let text = agent.to_string(value)?;
    agent.heap.charge(agent.heap.string_len(text))?;
    Ok(agent.heap.string(text).to_text())
}

/// `GetOption(options, name, string, values, undefined)` (§9.2.12): any
/// string when `values` is empty.
pub(super) fn string_option(
    agent: &mut Agent,
    options: Option<ObjectId>,
    name: &str,
    values: &[&str],
) -> JsResult<Option<String>> {
    let value = option(agent, options, name)?;
    if value == Value::UNDEFINED {
        return Ok(None);
    }
    let text = text_of(agent, value)?;
    if !values.is_empty() && !values.contains(&text.as_str()) {
        return range_error(agent, &format!("invalid Intl {name} option"));
    }
    Ok(Some(text))
}

/// `GetOption` for one of `values`, as its index; `None` when absent.
pub(super) fn choice_option(
    agent: &mut Agent,
    options: Option<ObjectId>,
    name: &str,
    values: &[&str],
) -> JsResult<Option<usize>> {
    let text = string_option(agent, options, name, values)?;
    Ok(text.and_then(|text| values.iter().position(|value| *value == text)))
}

/// `GetOption` for one of `values`, as its index, with a default.
pub(super) fn choice(
    agent: &mut Agent,
    options: Option<ObjectId>,
    name: &str,
    values: &[&str],
    default: usize,
) -> JsResult<usize> {
    Ok(choice_option(agent, options, name, values)?.unwrap_or(default))
}

/// `DefaultNumberOption(value, minimum, maximum, fallback)` (§9.2.13).
pub(super) fn default_number_option(
    agent: &mut Agent,
    value: Value,
    minimum: u32,
    maximum: u32,
    fallback: Option<u32>,
) -> JsResult<Option<u32>> {
    if value == Value::UNDEFINED {
        return Ok(fallback);
    }
    let number = agent.to_number(value)?;
    if number.is_nan() || number < f64::from(minimum) || number > f64::from(maximum) {
        return range_error(agent, "Intl number option is out of range");
    }
    // In range, so the floor fits.
    Ok(Some(number.floor() as u32))
}

/// `GetNumberOption(options, name, minimum, maximum, fallback)` (§9.2.14).
pub(super) fn number_option(
    agent: &mut Agent,
    options: Option<ObjectId>,
    name: &str,
    minimum: u32,
    maximum: u32,
    fallback: Option<u32>,
) -> JsResult<Option<u32>> {
    let value = option(agent, options, name)?;
    default_number_option(agent, value, minimum, maximum, fallback)
}

/// The `localeMatcher` option: whether it is `"best fit"`.
pub(super) fn best_fit(agent: &mut Agent, options: Option<ObjectId>) -> JsResult<bool> {
    Ok(choice(agent, options, "localeMatcher", &["lookup", "best fit"], 1)? == 1)
}

/// The row of the longest prefix of `base` that `locales` has.
fn lookup(locales: &Table, mut base: &str) -> Option<u32> {
    loop {
        if base != "root" {
            if let Some(row) = locales.find_text(base) {
                return Some(row);
            }
        }
        (base, _) = base.rsplit_once('-')?;
    }
}

/// The data row for the base name `base` and the name to report for it.
///
/// CLDR names its locale files by what they add to their parent, so
/// `en-US` and `zh-TW` have no file of their own: the first is what
/// `en` describes and the second is `zh-Hant-TW`. The request is
/// therefore matched with its likely subtags added, and the result is
/// reported in the request's own terms. A request that cannot be
/// matched that way falls back to truncation (`LookupMatcher`, §9.2.3).
fn match_base(locales: &Table, base: &str) -> Option<(u32, String)> {
    let fields = locale::base_fields(base);
    if fields[0] != "und" {
        let full = locale::adjust_likely_subtags(base, false);
        let [language, script, region, _] = locale::base_fields(&full);
        let likely = locale::adjust_likely_subtags(language, false);
        let usual_script = locale::base_fields(&likely)[1] == script;
        let usual_region = usual_script && locale::base_fields(&likely)[2] == region;
        let mut candidates = vec![(format!("{language}-{script}-{region}"), true)];
        if usual_script {
            candidates.push((format!("{language}-{region}"), true));
        }
        candidates.push((format!("{language}-{script}"), usual_region));
        if usual_script {
            candidates.push((language.to_owned(), usual_region));
        }
        for (candidate, keeps_region) in candidates {
            if let Some(row) = locales.find_text(&candidate) {
                let mut name = fields[0].to_owned();
                if !fields[1].is_empty() {
                    name.push('-');
                    name.push_str(fields[1]);
                }
                if !fields[2].is_empty() && keeps_region {
                    name.push('-');
                    name.push_str(fields[2]);
                }
                return Some((row, name));
            }
        }
    }
    lookup(locales, base).map(|row| (row, locales.text(row, 0).to_owned()))
}

fn match_locale(locales: &Table, tag: &str, _best_fit: bool) -> Option<(u32, String)> {
    match_base(locales, locale::split_base(tag).0)
}

/// A resolved locale (§9.2.7 `ResolveLocale`): the data locale and the
/// Unicode extension keywords the service honored.
pub(super) struct Resolution {
    /// The row of the data locale in the pack's locale table.
    pub(super) row: u32,
    /// The data locale's name in the pack.
    pub(super) base: &'static str,
    /// The locale to report, without keywords.
    name: String,
    /// The requested tag that matched, for its extension keywords.
    requested: Option<String>,
    keywords: Vec<(&'static str, String)>,
}

impl Resolution {
    /// Match `requested` against `locales`, or use the default locale.
    pub(super) fn new(locales: &Table, requested: &[String], best_fit: bool) -> Self {
        let found = requested
            .iter()
            .find_map(|tag| match_locale(locales, tag, best_fit).map(|found| (found, tag.clone())));
        let ((row, name), requested) = match found {
            Some((found, tag)) => (found, Some(tag)),
            None => (
                match_base(locales, DEFAULT_LOCALE).expect("the default locale has data"),
                None,
            ),
        };
        Self {
            row,
            base: locales.text(row, 0),
            name,
            requested,
            keywords: Vec::new(),
        }
    }

    /// The value of extension key `key` (§9.2.7 step 9): the option when
    /// the locale supports it, else the request's keyword when
    /// supported, else `default`. A keyword that decided the value stays
    /// in the resolved locale.
    pub(super) fn key(
        &mut self,
        key: &'static str,
        option: Option<&str>,
        supported: impl Fn(&str) -> bool,
        default: &str,
    ) -> String {
        let mut value = default.to_owned();
        let mut keyword = None;
        let requested = self
            .requested
            .as_deref()
            .and_then(|tag| locale::unicode_value(tag, key));
        if let Some(requested) = requested {
            if !requested.is_empty() {
                if supported(requested) {
                    value = requested.to_owned();
                    keyword = Some(requested.to_owned());
                }
            } else if supported("true") {
                value = "true".to_owned();
                keyword = Some(String::new());
            }
        }
        if let Some(option) = option {
            if supported(option) && option != value {
                value = option.to_owned();
                keyword = None;
            }
        }
        if let Some(keyword) = keyword {
            self.keywords.push((key, keyword));
        }
        value
    }

    /// The resolved locale: the data locale and the honored keywords.
    pub(super) fn locale(&self) -> String {
        let mut keywords = self.keywords.clone();
        keywords.sort();
        let mut tag = self.name.clone();
        if !keywords.is_empty() {
            tag.push_str("-u");
            for (key, value) in keywords {
                tag.push('-');
                tag.push_str(key);
                if !value.is_empty() {
                    tag.push('-');
                    tag.push_str(&value);
                }
            }
        }
        tag
    }
}

/// `SupportedLocales` (§9.2.9) behind every `supportedLocalesOf`.
pub(super) fn supported_locales(
    agent: &mut Agent,
    locales: &Table,
    requested: Value,
    options: Value,
) -> JsResult<Value> {
    let requested = canonicalize_locale_list(agent, requested)?;
    let options = coerce_options(agent, options)?;
    let best_fit = best_fit(agent, options)?;
    let matches = requested
        .iter()
        .filter(|tag| match_locale(locales, tag, best_fit).is_some())
        .map(|tag| agent.string(tag))
        .collect::<JsResult<Vec<_>>>()?;
    Ok(Value::object(agent.create_array_from_list(&matches)?))
}

/// A service object whose prototype is `prototype`, holding `state`.
/// `bytes` is what the state owns beyond its own size.
pub(super) fn new_service<T: Any + Send + Sync>(
    agent: &mut Agent,
    prototype: ObjectId,
    state: T,
    bytes: usize,
) -> JsResult<ObjectId> {
    let bytes = std::mem::size_of::<T>() + bytes;
    agent.heap.charge(std::mem::size_of::<Extra>() + bytes)?;
    let object = agent.new_object_with(Some(prototype))?;
    agent.heap.object_mut(object).extra_mut().internal = Some(Internal::Service {
        state: Arc::new(state),
        bound: Value::UNDEFINED,
        bytes: u32::try_from(bytes).unwrap_or(u32::MAX),
    });
    Ok(object)
}

/// The settings of `value` when it is a service object of type `T`.
pub(super) fn service<T: Any + Send + Sync>(agent: &Agent, value: Value) -> Option<Arc<T>> {
    match agent.heap.object(value.as_object()?).internal() {
        Some(Internal::Service { state, .. }) => Arc::clone(state).downcast().ok(),
        _ => None,
    }
}

/// The bound function of a service object, undefined before it is made.
pub(super) fn bound(agent: &Agent, object: ObjectId) -> Value {
    match agent.heap.object(object).internal() {
        Some(Internal::Service { bound, .. }) => *bound,
        _ => Value::UNDEFINED,
    }
}

pub(super) fn set_bound(agent: &mut Agent, object: ObjectId, function: Value) {
    if let Some(Internal::Service { bound, .. }) = agent.heap.object_mut(object).internal_mut() {
        *bound = function;
    }
}

/// A native function that remembers `captured`.
pub(super) fn closure(
    agent: &mut Agent,
    behavior: crate::runtime::NativeFn,
    length: u32,
    captured: Value,
) -> JsResult<ObjectId> {
    let function = agent.new_native_function(behavior, "", length)?;
    agent.heap.charge(std::mem::size_of::<Extra>() + 8)?;
    agent.heap.object_mut(function).extra_mut().internal =
        Some(Internal::Captures(Box::new([captured])));
    Ok(function)
}

/// What the running [`closure`] remembers.
pub(super) fn captured(agent: &Agent) -> Value {
    let Some(function) = agent.active_function() else {
        return Value::UNDEFINED;
    };
    match agent.heap.object(function).internal() {
        Some(Internal::Captures(values)) => values[0],
        _ => Value::UNDEFINED,
    }
}

/// Define `name` on `object` as a data property holding `text`.
pub(super) fn set_text(
    agent: &mut Agent,
    object: ObjectId,
    name: &str,
    text: &str,
) -> JsResult<()> {
    let value = agent.string(text)?;
    set(agent, object, name, value)
}

/// `CreateDataPropertyOrThrow(object, name, value)`.
pub(super) fn set(agent: &mut Agent, object: ObjectId, name: &str, value: Value) -> JsResult<()> {
    let key = agent.heap.key(name)?;
    agent.create_data_property_or_throw(object, key, value)?;
    Ok(())
}

const INCREMENTS: [u32; 15] = [
    1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 2500, 5000,
];

/// `SetNumberFormatDigitOptions` (§16.1.3).
pub(super) fn digit_options(
    agent: &mut Agent,
    options: Option<ObjectId>,
    minimum_fraction_default: u32,
    mut maximum_fraction_default: u32,
    compact: bool,
) -> JsResult<DigitOptions> {
    let minimum_integer =
        number_option(agent, options, "minimumIntegerDigits", 1, 21, Some(1))?.unwrap_or(1);
    let minimum_fraction = option(agent, options, "minimumFractionDigits")?;
    let maximum_fraction = option(agent, options, "maximumFractionDigits")?;
    let minimum_significant = option(agent, options, "minimumSignificantDigits")?;
    let maximum_significant = option(agent, options, "maximumSignificantDigits")?;
    let increment =
        number_option(agent, options, "roundingIncrement", 1, 5000, Some(1))?.unwrap_or(1);
    if !INCREMENTS.contains(&increment) {
        return range_error(agent, "invalid roundingIncrement");
    }
    let mode = choice(agent, options, "roundingMode", ROUNDING_MODES, 6)?;
    let priority = choice(
        agent,
        options,
        "roundingPriority",
        &["auto", "morePrecision", "lessPrecision"],
        0,
    )?;
    let strip = choice(
        agent,
        options,
        "trailingZeroDisplay",
        &["auto", "stripIfInteger"],
        0,
    )? == 1;
    if increment != 1 {
        maximum_fraction_default = minimum_fraction_default;
    }
    let has_significant =
        minimum_significant != Value::UNDEFINED || maximum_significant != Value::UNDEFINED;
    let has_fraction = minimum_fraction != Value::UNDEFINED || maximum_fraction != Value::UNDEFINED;
    let mut need_significant = true;
    let mut need_fraction = true;
    if priority == 0 {
        need_significant = has_significant;
        if need_significant || (!has_fraction && compact) {
            need_fraction = false;
        }
    }
    let mut result = DigitOptions {
        minimum_integer,
        minimum_fraction: 0,
        maximum_fraction: 0,
        minimum_significant: 1,
        maximum_significant: 21,
        rounding_type: RoundingType::FractionDigits,
        increment,
        mode: RoundingMode::from_index(mode),
        strip_if_integer: strip,
        priority: priority as u8,
    };
    if need_significant && has_significant {
        let minimum =
            default_number_option(agent, minimum_significant, 1, 21, Some(1))?.unwrap_or(1);
        result.minimum_significant = minimum;
        result.maximum_significant =
            default_number_option(agent, maximum_significant, minimum, 21, Some(21))?.unwrap_or(21);
    }
    if need_fraction {
        if has_fraction {
            let minimum = default_number_option(agent, minimum_fraction, 0, 100, None)?;
            let maximum = default_number_option(agent, maximum_fraction, 0, 100, None)?;
            (result.minimum_fraction, result.maximum_fraction) = match (minimum, maximum) {
                (None, Some(maximum)) => (minimum_fraction_default.min(maximum), maximum),
                (Some(minimum), None) => (minimum, maximum_fraction_default.max(minimum)),
                (Some(minimum), Some(maximum)) if minimum <= maximum => (minimum, maximum),
                (Some(_), Some(_)) => {
                    return range_error(agent, "minimumFractionDigits exceeds the maximum");
                }
                (None, None) => (minimum_fraction_default, maximum_fraction_default),
            };
        } else {
            result.minimum_fraction = minimum_fraction_default;
            result.maximum_fraction = maximum_fraction_default;
        }
    }
    result.rounding_type = if !need_significant && !need_fraction {
        result.minimum_significant = 1;
        result.maximum_significant = 2;
        result.priority = 1;
        RoundingType::MorePrecision
    } else if priority == 1 {
        RoundingType::MorePrecision
    } else if priority == 2 {
        RoundingType::LessPrecision
    } else if has_significant {
        RoundingType::SignificantDigits
    } else {
        RoundingType::FractionDigits
    };
    if increment != 1 {
        if result.rounding_type != RoundingType::FractionDigits {
            return agent.type_error("roundingIncrement needs fraction digit rounding");
        }
        if result.maximum_fraction != result.minimum_fraction {
            return range_error(agent, "roundingIncrement needs equal fraction digit limits");
        }
    }
    Ok(result)
}

/// The digit options as `resolvedOptions` lists them between
/// `minimumIntegerDigits` and `roundingIncrement`.
pub(super) fn digit_properties(
    agent: &mut Agent,
    object: ObjectId,
    digits: &DigitOptions,
) -> JsResult<()> {
    set(
        agent,
        object,
        "minimumIntegerDigits",
        Value::number(f64::from(digits.minimum_integer)),
    )?;
    if digits.rounding_type != RoundingType::SignificantDigits {
        set(
            agent,
            object,
            "minimumFractionDigits",
            Value::number(f64::from(digits.minimum_fraction)),
        )?;
        set(
            agent,
            object,
            "maximumFractionDigits",
            Value::number(f64::from(digits.maximum_fraction)),
        )?;
    }
    if digits.rounding_type != RoundingType::FractionDigits {
        set(
            agent,
            object,
            "minimumSignificantDigits",
            Value::number(f64::from(digits.minimum_significant)),
        )?;
        set(
            agent,
            object,
            "maximumSignificantDigits",
            Value::number(f64::from(digits.maximum_significant)),
        )?;
    }
    Ok(())
}

/// The rounding options as `resolvedOptions` lists them.
pub(super) fn rounding_properties(
    agent: &mut Agent,
    object: ObjectId,
    digits: &DigitOptions,
) -> JsResult<()> {
    set(
        agent,
        object,
        "roundingIncrement",
        Value::number(f64::from(digits.increment)),
    )?;
    set_text(agent, object, "roundingMode", digits.mode.name())?;
    let priority = ["auto", "morePrecision", "lessPrecision"][usize::from(digits.priority)];
    set_text(agent, object, "roundingPriority", priority)?;
    let display = if digits.strip_if_integer {
        "stripIfInteger"
    } else {
        "auto"
    };
    set_text(agent, object, "trailingZeroDisplay", display)
}

/// The legacy constructor behavior of NumberFormat and DateTimeFormat
/// (§16.1.1 step 32, `ChainNumberFormat`): called as a function on an
/// instance of the constructor in `slot`, the new object is stored on
/// `this` under the fallback symbol and `this` is returned.
pub(super) fn legacy_result(
    agent: &mut Agent,
    this: Value,
    object: ObjectId,
    slot: usize,
) -> JsResult<Value> {
    if agent.new_target().is_none() {
        if let Some(receiver) = this.as_object() {
            let constructor = Value::object(agent.intrinsics.intl[slot]);
            if agent.ordinary_has_instance(constructor, this)? {
                let key = crate::runtime::PropertyKey::Symbol(agent.symbols.intl_fallback);
                agent.define_property_or_throw(
                    receiver,
                    key,
                    crate::runtime::PropertyDescriptor::data_with(
                        Value::object(object),
                        false,
                        false,
                        false,
                    ),
                )?;
                return Ok(this);
            }
        }
    }
    Ok(Value::object(object))
}

/// `UnwrapNumberFormat` and `UnwrapDateTimeFormat`: `this`, or what a
/// legacy constructor call stored on it.
pub(super) fn unwrap_legacy<T: Any + Send + Sync>(
    agent: &mut Agent,
    this: Value,
    slot: usize,
) -> JsResult<Value> {
    let Some(object) = this.as_object() else {
        return agent.type_error("expected an Intl object");
    };
    if service::<T>(agent, this).is_none() {
        let constructor = Value::object(agent.intrinsics.intl[slot]);
        if agent.ordinary_has_instance(constructor, this)? {
            let key = crate::runtime::PropertyKey::Symbol(agent.symbols.intl_fallback);
            return agent.get(object, key);
        }
    }
    Ok(this)
}
