//! Intl.DisplayNames (ECMA-402 §12) over the display names of every
//! CLDR locale (`names.bin`).

use std::sync::{Arc, OnceLock};

use super::super::{arg, install_methods, method, new_constructor, prototype_from_new_target};
use super::number_data::{currency, data as number_data};
use super::pack::{Pack, Table};
use super::service::{
    self, Resolution, best_fit, choice, choice_option, new_service, options_object, range_error,
    set_text, text_of,
};
use super::{canonicalize_locale_list, locale, slot};
use crate::runtime::{Agent, JsResult, ObjectId, PropertyDescriptor, PropertyKey, Value};

static PACK: Pack = Pack::new(include_bytes!("../../../../data/intl/names.bin"));

const STYLES: &[&str] = &["narrow", "short", "long"];
const TYPES: &[&str] = &[
    "language",
    "region",
    "script",
    "currency",
    "calendar",
    "dateTimeField",
];
const FALLBACKS: &[&str] = &["code", "none"];
const LANGUAGE_DISPLAYS: &[&str] = &["dialect", "standard"];
/// The codes of the `dateTimeField` type, in the generator's order.
const FIELDS: [&str; 12] = [
    "era",
    "year",
    "quarter",
    "month",
    "weekOfYear",
    "weekday",
    "day",
    "dayPeriod",
    "hour",
    "minute",
    "second",
    "timeZoneName",
];

/// Kinds of names in the pack.
const LANGUAGE: u32 = 0;
const SCRIPT: u32 = 1;
const REGION: u32 = 2;
const CALENDAR: u32 = 3;
const FIELD: u32 = 4;
const PATTERN: u32 = 5;
/// Variants of a name (CLDR's `alt`).
const SHORT: u32 = 1;
const STAND_ALONE: u32 = 3;

struct Data {
    locales: Table,
    codes: Table,
    names: Table,
}

fn data() -> &'static Data {
    static DATA: OnceLock<Data> = OnceLock::new();
    DATA.get_or_init(|| Data {
        locales: PACK.table("locales"),
        codes: PACK.table("codes"),
        names: PACK.table("names"),
    })
}

impl Data {
    fn numbered(&self, locale: u32, kind: u32, variant: u32, code: u32) -> Option<&'static str> {
        self.names
            .inherited(&self.locales, locale, kind << 4 | variant, &[code])
            .map(|id| PACK.text(id))
    }

    /// The name of `code`, in `variant` when the locale has that one.
    fn name(&self, locale: u32, kind: u32, variant: u32, code: &str) -> Option<&'static str> {
        let code = self.codes.find_text(code)?;
        [variant, 0]
            .into_iter()
            .find_map(|variant| self.numbered(locale, kind, variant, code))
    }
}

struct DisplayNames {
    locale: String,
    row: u32,
    style: usize,
    kind: usize,
    fallback: usize,
    standard: bool,
}

pub(super) fn install(agent: &mut Agent, intl: ObjectId) -> JsResult<()> {
    let prototype = agent.new_object()?;
    agent.intrinsics.intl[slot::DISPLAY_NAMES_PROTOTYPE] = prototype;
    let constructor = new_constructor(agent, "DisplayNames", construct, 2, prototype)?;
    let key = agent.heap.key("DisplayNames")?;
    agent.define_property_or_throw(
        intl,
        key,
        PropertyDescriptor::data_with(Value::object(constructor), true, false, true),
    )?;
    let tag = agent.string("Intl.DisplayNames")?;
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
            method("of", of, 1),
            method("resolvedOptions", resolved_options, 0),
        ],
    )
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if agent.new_target().is_none() {
        return agent.type_error("Intl.DisplayNames requires new");
    }
    let default = agent.intrinsics.intl[slot::DISPLAY_NAMES_PROTOTYPE];
    let prototype = prototype_from_new_target(agent, default)?;
    let requested = canonicalize_locale_list(agent, arg(args, 0))?;
    let options = options_object(agent, arg(args, 1))?;
    let best = best_fit(agent, options)?;
    let style = choice(agent, options, "style", STYLES, 2)?;
    let Some(kind) = choice_option(agent, options, "type", TYPES)? else {
        return agent.type_error("Intl.DisplayNames needs a type");
    };
    let fallback = choice(agent, options, "fallback", FALLBACKS, 0)?;
    let standard = choice(agent, options, "languageDisplay", LANGUAGE_DISPLAYS, 0)? == 1;
    let resolution = Resolution::new(&data().locales, &requested, best);
    let state = DisplayNames {
        locale: resolution.locale(),
        row: resolution.row,
        style,
        kind,
        fallback,
        standard,
    };
    let bytes = state.locale.len();
    Ok(Value::object(new_service(agent, prototype, state, bytes)?))
}

fn supported_locales(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    service::supported_locales(agent, &data().locales, arg(args, 0), arg(args, 1))
}

fn this_names(agent: &mut Agent, this: Value) -> JsResult<Arc<DisplayNames>> {
    match service::service(agent, this) {
        Some(state) => Ok(state),
        None => agent.type_error("expected an Intl.DisplayNames"),
    }
}

fn letters(text: &str, lengths: std::ops::RangeInclusive<usize>) -> bool {
    lengths.contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_alphabetic())
}

/// Whether `text` is a `unicode_language_id` that starts with a
/// language and is not `root` (§12.5.1 step 1).
fn is_language_id(text: &str) -> bool {
    let mut pieces = text.split('-').peekable();
    let Some(language) = pieces.next() else {
        return false;
    };
    if !(letters(language, 2..=3) || letters(language, 5..=8)) {
        return false;
    }
    if pieces.peek().is_some_and(|piece| letters(piece, 4..=4)) {
        pieces.next();
    }
    let region = |piece: &str| {
        letters(piece, 2..=2) || (piece.len() == 3 && piece.bytes().all(|b| b.is_ascii_digit()))
    };
    if pieces.peek().is_some_and(|piece| region(piece)) {
        pieces.next();
    }
    let mut variants: Vec<String> = Vec::new();
    for piece in pieces {
        let alphanumeric = piece.bytes().all(|byte| byte.is_ascii_alphanumeric());
        let shaped = (5..=8).contains(&piece.len())
            || (piece.len() == 4 && piece.as_bytes()[0].is_ascii_digit());
        let lower = piece.to_ascii_lowercase();
        if !alphanumeric || !shaped || variants.contains(&lower) {
            return false;
        }
        variants.push(lower);
    }
    true
}

/// `CanonicalCodeForDisplayNames` (§12.5.1); `None` for an invalid code.
fn canonical_code(kind: usize, code: &str) -> Option<String> {
    match kind {
        0 => is_language_id(code)
            .then(|| locale::canonicalize(code))
            .flatten(),
        1 => {
            let digits = code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_digit());
            (letters(code, 2..=2) || digits).then(|| code.to_ascii_uppercase())
        }
        2 => letters(code, 4..=4).then(|| {
            let lower = code.to_ascii_lowercase();
            lower[..1].to_ascii_uppercase() + &lower[1..]
        }),
        3 => letters(code, 3..=3).then(|| code.to_ascii_uppercase()),
        4 => {
            let shaped = code.split('-').all(|piece| {
                (3..=8).contains(&piece.len())
                    && piece.bytes().all(|byte| byte.is_ascii_alphanumeric())
            });
            shaped.then(|| code.to_ascii_lowercase())
        }
        _ => FIELDS.contains(&code).then(|| code.to_owned()),
    }
}

impl DisplayNames {
    /// The variant of a name the style asks for.
    fn variant(&self) -> u32 {
        if self.style == 2 { 0 } else { SHORT }
    }

    /// The name of a language identifier (LDML part 1 §6 "Display Name
    /// Elements"): the longest named prefix, then the remaining subtags
    /// in parentheses.
    fn language(&self, code: &str) -> Option<String> {
        let data = data();
        let variant = self.variant();
        let [language, script, region, variants] = locale::base_fields(code);
        let name = |code: &str| data.name(self.row, LANGUAGE, variant, code);
        let (mut script_left, mut region_left) = (script, region);
        let mut found = None;
        if !self.standard {
            if !script.is_empty() && !region.is_empty() {
                found = name(&format!("{language}-{script}-{region}"));
                if found.is_some() {
                    (script_left, region_left) = ("", "");
                }
            }
            if found.is_none() && !script.is_empty() {
                found = name(&format!("{language}-{script}"));
                if found.is_some() {
                    script_left = "";
                }
            }
            if found.is_none() && !region.is_empty() {
                found = name(&format!("{language}-{region}"));
                if found.is_some() {
                    region_left = "";
                }
            }
        }
        let base = found.or_else(|| name(language))?;
        let mut extras: Vec<&str> = Vec::new();
        if !script_left.is_empty() {
            let named = data
                .name(self.row, SCRIPT, STAND_ALONE, script_left)
                .unwrap_or(script_left);
            extras.push(named);
        }
        if !region_left.is_empty() {
            let named = data
                .name(self.row, REGION, variant, region_left)
                .unwrap_or(region_left);
            extras.push(named);
        }
        extras.extend(variants.split('-').filter(|piece| !piece.is_empty()));
        if extras.is_empty() {
            return Some(base.to_owned());
        }
        let separator = data.numbered(self.row, PATTERN, 0, 1).unwrap_or("{0}, {1}");
        let mut joined = extras[0].to_owned();
        for extra in &extras[1..] {
            joined = separator.replace("{0}", &joined).replace("{1}", extra);
        }
        let pattern = data
            .numbered(self.row, PATTERN, 0, 0)
            .unwrap_or("{0} ({1})");
        Some(pattern.replace("{0}", base).replace("{1}", &joined))
    }

    fn name(&self, code: &str) -> Option<String> {
        let data = data();
        let variant = self.variant();
        let owned = |text: Option<&'static str>| text.map(str::to_owned);
        match self.kind {
            0 => self.language(code),
            1 => owned(data.name(self.row, REGION, variant, code)),
            2 => owned(data.name(self.row, SCRIPT, variant, code)),
            3 => {
                let numbers = number_data();
                let row = Resolution::new(&numbers.locales, std::slice::from_ref(&self.locale), false).row;
                let field = match self.style {
                    0 => currency::NARROW,
                    1 => currency::SYMBOL,
                    _ => currency::NAME,
                };
                owned(
                    numbers
                        .currency(row, code, field)
                        .or_else(|| numbers.currency(row, code, currency::SYMBOL))
                        .filter(|name| *name != code),
                )
            }
            4 => {
                // CLDR names calendars by their older identifiers.
                let code = match code {
                    "gregory" => "gregorian",
                    "ethioaa" => "ethiopic-amete-alem",
                    "islamicc" => "islamic-civil",
                    other => other,
                };
                owned(data.name(self.row, CALENDAR, 0, code))
            }
            _ => {
                let field = FIELDS.iter().position(|field| *field == code)? as u32;
                // Narrow falls back to short and short to long.
                let width = [2, 1, 0][self.style];
                owned(
                    (0..=width)
                        .rev()
                        .find_map(|width| data.numbered(self.row, FIELD, width, field)),
                )
            }
        }
    }
}

fn of(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let state = this_names(agent, this)?;
    let code = text_of(agent, arg(args, 0))?;
    let Some(code) = canonical_code(state.kind, &code) else {
        return range_error(agent, "invalid code for Intl.DisplayNames");
    };
    match state.name(&code) {
        Some(name) => agent.string(&name),
        None if state.fallback == 0 => agent.string(&code),
        None => Ok(Value::UNDEFINED),
    }
}

fn resolved_options(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let state = this_names(agent, this)?;
    let object = agent.new_object()?;
    set_text(agent, object, "locale", &state.locale)?;
    set_text(agent, object, "style", STYLES[state.style])?;
    set_text(agent, object, "type", TYPES[state.kind])?;
    set_text(agent, object, "fallback", FALLBACKS[state.fallback])?;
    if state.kind == 0 {
        let display = LANGUAGE_DISPLAYS[usize::from(state.standard)];
        set_text(agent, object, "languageDisplay", display)?;
    }
    Ok(Value::object(object))
}
