//! `String` (§22.1): `String.fromCharCode`, `fromCodePoint` and `raw`, and
//! the methods of `String.prototype`.
//!
//! Strings are worked on as UTF-16 code units, as the specification
//! defines them. The methods that take a regular expression call its
//! `Symbol.match`/`replace`/`search`/`split` method when one is given;
//! creating a regular expression from a string waits for RegExp (JS-08).

use rustic_base::unicode::{NormalizationForm, normalize_utf16};

use super::{Method, arg, install_methods, method};
use crate::runtime::agent::{Agent, JsResult};
use crate::runtime::number::{is_str_whitespace, to_integer_or_infinity, to_length, to_uint32};
use crate::runtime::object::PropertyDescriptor;
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::{StringId, Value};
use crate::runtime::{ErrorKind, JsStr};

const STATIC_METHODS: &[Method] = &[
    method("fromCharCode", from_char_code, 1),
    method("fromCodePoint", from_code_point, 1),
    method("raw", raw, 1),
];

const PROTOTYPE_METHODS: &[Method] = &[
    method("at", at, 1),
    method("charAt", char_at, 1),
    method("charCodeAt", char_code_at, 1),
    method("codePointAt", code_point_at, 1),
    method("concat", concat, 1),
    method("endsWith", ends_with, 1),
    method("includes", includes, 1),
    method("indexOf", index_of, 1),
    method("isWellFormed", is_well_formed, 0),
    method("lastIndexOf", last_index_of, 1),
    method("localeCompare", locale_compare, 1),
    method("match", match_, 1),
    method("matchAll", match_all, 1),
    method("normalize", normalize, 0),
    method("padEnd", pad_end, 1),
    method("padStart", pad_start, 1),
    method("repeat", repeat, 1),
    method("replace", replace, 2),
    method("replaceAll", replace_all, 2),
    method("search", search, 1),
    method("slice", slice, 2),
    method("split", split, 2),
    method("startsWith", starts_with, 1),
    method("substr", substr, 2),
    method("substring", substring, 2),
    method("toLocaleLowerCase", to_locale_lower_case, 0),
    method("toLocaleUpperCase", to_locale_upper_case, 0),
    method("toLowerCase", to_lower_case, 0),
    method("toUpperCase", to_upper_case, 0),
    method("toWellFormed", to_well_formed, 0),
    method("trim", trim, 0),
    method("trimEnd", trim_end, 0),
    method("trimStart", trim_start, 0),
];

/// The longest string this engine builds, in code units (as in V8).
const MAX_STRING_LENGTH: f64 = 536_870_888.0;

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let global = agent.intrinsics.global;
    let key = agent.heap.key("String")?;
    let Some(string) = agent.get(global, key)?.as_object() else {
        unreachable!("the String constructor is installed first");
    };
    install_methods(agent, string, STATIC_METHODS)?;
    let prototype = agent.intrinsics.string_prototype;
    install_methods(agent, prototype, PROTOTYPE_METHODS)?;
    // `trimLeft` and `trimRight` are the same function objects as
    // `trimStart` and `trimEnd` (Annex B.2.2.15, B.2.2.16).
    for (alias, name) in [("trimLeft", "trimStart"), ("trimRight", "trimEnd")] {
        let key = agent.heap.key(name)?;
        let f = agent.get(prototype, key)?;
        let alias = agent.heap.key(alias)?;
        agent.define_property_or_throw(
            prototype,
            alias,
            PropertyDescriptor::data_with(f, true, false, true),
        )?;
    }
    Ok(())
}

// ---- helpers ----------------------------------------------------------------

/// `RequireObjectCoercible(this)` and `ToString` of it, as code units.
fn this_units(agent: &mut Agent, this: Value) -> JsResult<Vec<u16>> {
    let this = agent.require_object_coercible(this)?;
    units_of(agent, this)
}

fn units_of(agent: &mut Agent, value: Value) -> JsResult<Vec<u16>> {
    let s = agent.to_string(value)?;
    Ok(agent.heap.string(s).to_units())
}

fn make(agent: &mut Agent, units: &[u16]) -> JsResult<Value> {
    agent.string_from(JsStr::from_units(units))
}

fn to_integer(agent: &mut Agent, value: Value) -> JsResult<f64> {
    let n = agent.to_number(value)?;
    Ok(to_integer_or_infinity(n))
}

fn range_error<T>(agent: &mut Agent, message: &str) -> JsResult<T> {
    Err(agent.throw(ErrorKind::RangeError, message))
}

/// A RangeError unless a string of `length` units can be built.
fn check_length(agent: &mut Agent, length: f64) -> JsResult<()> {
    let budget = agent
        .heap
        .byte_limit()
        .saturating_sub(agent.heap.bytes_used()) as f64;
    if length > MAX_STRING_LENGTH || 2.0 * length > budget {
        return range_error(agent, "Invalid string length");
    }
    Ok(())
}

/// `StringIndexOf(string, searchValue, fromIndex)` (§6.1.4.1).
fn find(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return (from <= haystack.len()).then_some(from);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    (from..=haystack.len() - needle.len()).find(|&i| haystack[i..].starts_with(needle))
}

use super::regexp::is_regexp;

/// A clamped position argument: `ToIntegerOrInfinity` clamped to
/// [0, length], or `default` when undefined.
fn position(agent: &mut Agent, value: Value, length: usize, default: usize) -> JsResult<usize> {
    if value == Value::UNDEFINED {
        return Ok(default);
    }
    let n = to_integer(agent, value)?;
    // Clamped to [0, length] before the conversion.
    Ok(n.clamp(0.0, length as f64) as usize)
}

/// Like [`position`] but negative values count from the end.
fn relative(agent: &mut Agent, value: Value, length: usize, default: usize) -> JsResult<usize> {
    if value == Value::UNDEFINED {
        return Ok(default);
    }
    let n = to_integer(agent, value)?;
    let len = length as f64;
    let n = if n < 0.0 {
        (len + n).max(0.0)
    } else {
        n.min(len)
    };
    Ok(n as usize)
}

fn is_lead(u: u16) -> bool {
    (0xD800..0xDC00).contains(&u)
}

fn is_trail(u: u16) -> bool {
    (0xDC00..0xE000).contains(&u)
}

/// `CodePointAt(string, position)` (§11.1.4): the code point and how
/// many units it takes.
fn code_point(units: &[u16], i: usize) -> (u32, usize) {
    let first = units[i];
    if is_lead(first) {
        if let Some(&second) = units.get(i + 1) {
            if is_trail(second) {
                let cp =
                    0x10000 + ((u32::from(first) - 0xD800) << 10) + (u32::from(second) - 0xDC00);
                return (cp, 2);
            }
        }
    }
    (u32::from(first), 1)
}

fn push_code_point(out: &mut Vec<u16>, cp: u32) {
    let mut buffer = [0u16; 2];
    match char::from_u32(cp) {
        Some(c) => out.extend_from_slice(c.encode_utf16(&mut buffer)),
        // A lone surrogate stays a single unit.
        None => out.push(cp as u16),
    }
}

// ---- String statics ---------------------------------------------------------

/// `String.fromCharCode(...codeUnits)` (§22.1.2.1).
fn from_char_code(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let mut units = Vec::with_capacity(args.len());
    for &a in args {
        let n = agent.to_number(a)?;
        units.push(crate::runtime::number::to_uint16(n));
    }
    make(agent, &units)
}

/// `String.fromCodePoint(...codePoints)` (§22.1.2.2).
fn from_code_point(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let mut units = Vec::with_capacity(args.len());
    for &a in args {
        let n = agent.to_number(a)?;
        if n.trunc() != n || !(0.0..=1_114_111.0).contains(&n) {
            return range_error(agent, "Invalid code point");
        }
        // `n` is an integer in [0, 0x10FFFF].
        push_code_point(&mut units, n as u32);
    }
    make(agent, &units)
}

/// `String.raw(template, ...substitutions)` (§22.1.2.4).
fn raw(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let cooked = agent.to_object(arg(args, 0))?;
    let raw_key = agent.heap.key("raw")?;
    let literals = agent.get(cooked, raw_key)?;
    let literals = agent.to_object(literals)?;
    let count = agent.length_of_array_like(literals)?;
    let mut out = Vec::new();
    let mut i = 0.0;
    while i < count {
        let key = if i < 4_294_967_295.0 {
            // An integer index below 2³² − 1.
            PropertyKey::Index(i as u32)
        } else {
            agent
                .heap
                .key(&crate::runtime::number::number_to_string(i))?
        };
        let piece = agent.get(literals, key)?;
        let piece = units_of(agent, piece)?;
        agent.heap.charge(2 * piece.len())?;
        out.extend_from_slice(&piece);
        if i + 1.0 >= count {
            break;
        }
        // The substitution after literal `i`, if any.
        if let Some(&sub) = args.get(i as usize + 1) {
            let sub = units_of(agent, sub)?;
            agent.heap.charge(2 * sub.len())?;
            out.extend_from_slice(&sub);
        }
        i += 1.0;
    }
    make(agent, &out)
}

// ---- access -----------------------------------------------------------------

/// `this` as a string without copying its units: the index accessors
/// are called in loops over long strings.
fn this_string(agent: &mut Agent, this: Value) -> JsResult<StringId> {
    let this = agent.require_object_coercible(this)?;
    agent.to_string(this)
}

/// The units at `index` and after it, if `index` is in range.
fn units_at(agent: &Agent, s: StringId, index: f64) -> Option<(u16, Option<u16>)> {
    let string = agent.heap.string(s);
    if index < 0.0 || index >= string.len() as f64 {
        return None;
    }
    // In range, so the index is exact.
    let index = index as usize;
    Some((string.unit(index)?, string.unit(index + 1)))
}

fn at(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_string(agent, this)?;
    let n = to_integer(agent, arg(args, 0))?;
    let k = if n >= 0.0 {
        n
    } else {
        agent.heap.string_len(s) as f64 + n
    };
    match units_at(agent, s, k) {
        Some((unit, _)) => make(agent, &[unit]),
        None => Ok(Value::UNDEFINED),
    }
}

fn char_at(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_string(agent, this)?;
    let n = to_integer(agent, arg(args, 0))?;
    match units_at(agent, s, n) {
        Some((unit, _)) => make(agent, &[unit]),
        None => Ok(Value::string(agent.names.empty)),
    }
}

fn char_code_at(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_string(agent, this)?;
    let n = to_integer(agent, arg(args, 0))?;
    Ok(units_at(agent, s, n).map_or(Value::NAN, |(unit, _)| Value::number(f64::from(unit))))
}

fn code_point_at(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_string(agent, this)?;
    let n = to_integer(agent, arg(args, 0))?;
    let Some((first, second)) = units_at(agent, s, n) else {
        return Ok(Value::UNDEFINED);
    };
    let pair = [first, second.unwrap_or(0)];
    let units = if second.is_some() {
        &pair[..]
    } else {
        &pair[..1]
    };
    Ok(Value::number(f64::from(code_point(units, 0).0)))
}

fn concat(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let mut s = this_units(agent, this)?;
    for &a in args {
        let piece = units_of(agent, a)?;
        agent.heap.charge(2 * piece.len())?;
        s.extend_from_slice(&piece);
    }
    make(agent, &s)
}

fn slice(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    let from = relative(agent, arg(args, 0), s.len(), 0)?;
    let to = relative(agent, arg(args, 1), s.len(), s.len())?;
    if from >= to {
        return Ok(Value::string(agent.names.empty));
    }
    make(agent, &s[from..to])
}

fn substring(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    let start = position(agent, arg(args, 0), s.len(), 0)?;
    let end = position(agent, arg(args, 1), s.len(), s.len())?;
    let (from, to) = (start.min(end), start.max(end));
    make(agent, &s[from..to])
}

/// `String.prototype.substr(start, length)` (Annex B.2.2.1).
fn substr(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    let start = relative(agent, arg(args, 0), s.len(), 0)?;
    let length = match arg(args, 1) {
        l if l == Value::UNDEFINED => s.len() as f64,
        l => to_integer(agent, l)?,
    };
    let end = (start as f64 + length).clamp(start as f64, s.len() as f64) as usize;
    make(agent, &s[start..end])
}

// ---- searching --------------------------------------------------------------

fn index_of(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    let search = units_of(agent, arg(args, 0))?;
    let from = position(agent, arg(args, 1), s.len(), 0)?;
    Ok(Value::number(
        find(&s, &search, from).map_or(-1.0, |i| i as f64),
    ))
}

/// `String.prototype.lastIndexOf` (§22.1.3.11).
fn last_index_of(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    let search = units_of(agent, arg(args, 0))?;
    let n = agent.to_number(arg(args, 1))?;
    let pos = if n.is_nan() {
        f64::INFINITY
    } else {
        to_integer_or_infinity(n)
    };
    if search.len() > s.len() {
        return Ok(Value::number(-1.0));
    }
    let start = pos.clamp(0.0, (s.len() - search.len()) as f64) as usize;
    let found = (0..=start).rev().find(|&i| s[i..].starts_with(&search));
    Ok(Value::number(found.map_or(-1.0, |i| i as f64)))
}

/// The search string of `includes`/`startsWith`/`endsWith`, which must
/// not be a regular expression.
fn search_units(agent: &mut Agent, value: Value, name: &str) -> JsResult<Vec<u16>> {
    if is_regexp(agent, value)? {
        return agent.type_error(&format!(
            "String.prototype.{name}: first argument must not be a regular expression"
        ));
    }
    units_of(agent, value)
}

fn includes(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    let search = search_units(agent, arg(args, 0), "includes")?;
    let from = position(agent, arg(args, 1), s.len(), 0)?;
    Ok(Value::boolean(find(&s, &search, from).is_some()))
}

fn starts_with(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    let search = search_units(agent, arg(args, 0), "startsWith")?;
    let start = position(agent, arg(args, 1), s.len(), 0)?;
    Ok(Value::boolean(s[start..].starts_with(&search)))
}

fn ends_with(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    let search = search_units(agent, arg(args, 0), "endsWith")?;
    let end = position(agent, arg(args, 1), s.len(), s.len())?;
    Ok(Value::boolean(s[..end].ends_with(&search)))
}

/// String.prototype.normalize (§22.1.3.15).
fn normalize(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let this = agent.require_object_coercible(this)?;
    let string = agent.to_string(this)?;
    let argument = arg(args, 0);
    let form = if argument == Value::UNDEFINED {
        NormalizationForm::Nfc
    } else {
        let form = agent.to_string(argument)?;
        let form = agent.heap.string(form);
        if form.units().eq("NFC".encode_utf16()) {
            NormalizationForm::Nfc
        } else if form.units().eq("NFD".encode_utf16()) {
            NormalizationForm::Nfd
        } else if form.units().eq("NFKC".encode_utf16()) {
            NormalizationForm::Nfkc
        } else if form.units().eq("NFKD".encode_utf16()) {
            NormalizationForm::Nfkd
        } else {
            return range_error(agent, "invalid normalization form");
        }
    };
    let input = agent.heap.string(string);
    if input.units().all(|unit| unit < 0x80) {
        return Ok(Value::string(string));
    }
    let room = agent
        .heap
        .byte_limit()
        .saturating_sub(agent.heap.bytes_used());
    // Four bytes per decomposed code point, with room for a stable-sort
    // buffer, vector growth, and the UTF-16 output.
    let max_decomposed = room / 16;
    let max_result = (room / 2).min(MAX_STRING_LENGTH as usize);
    let Some(output) = normalize_utf16(input.units(), form, max_decomposed, max_result) else {
        return range_error(agent, "Invalid string length");
    };
    if input.units().eq(output.iter().copied()) {
        return Ok(Value::string(string));
    }
    agent.string_from(JsStr::from_units(&output))
}

/// `String.prototype.localeCompare` (§22.1.3.12) without a locale: code
/// point order.
fn locale_compare(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    let that = units_of(agent, arg(args, 0))?;
    let points = |units: &[u16]| -> Vec<u32> {
        let mut out = Vec::with_capacity(units.len());
        let mut i = 0;
        while i < units.len() {
            let (cp, n) = code_point(units, i);
            out.push(cp);
            i += n;
        }
        out
    };
    let order = points(&s).cmp(&points(&that));
    Ok(Value::number(match order {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }))
}

// ---- building ---------------------------------------------------------------

fn repeat(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    let n = to_integer(agent, arg(args, 0))?;
    if n < 0.0 || n == f64::INFINITY {
        return range_error(agent, "Invalid count value");
    }
    if n == 0.0 || s.is_empty() {
        return Ok(Value::string(agent.names.empty));
    }
    check_length(agent, s.len() as f64 * n)?;
    // `n` times the length fits the budget checked just above.
    make(agent, &s.repeat(n as usize))
}

/// `StringPad(S, maxLength, fillString, placement)` (§22.1.3.17.2).
fn pad(agent: &mut Agent, this: Value, args: &[Value], at_start: bool) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    let n = agent.to_number(arg(args, 0))?;
    let max_length = to_length(n);
    if max_length <= s.len() as f64 {
        return make(agent, &s);
    }
    let fill = match arg(args, 1) {
        f if f == Value::UNDEFINED => vec![u16::from(b' ')],
        f => units_of(agent, f)?,
    };
    if fill.is_empty() {
        return make(agent, &s);
    }
    check_length(agent, max_length)?;
    // `max_length` fits the budget checked just above.
    let fill_length = max_length as usize - s.len();
    let filler: Vec<u16> = fill.iter().copied().cycle().take(fill_length).collect();
    let out = if at_start {
        [filler, s].concat()
    } else {
        [s, filler].concat()
    };
    make(agent, &out)
}

fn pad_start(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    pad(agent, this, args, true)
}

fn pad_end(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    pad(agent, this, args, false)
}

fn trim_with(agent: &mut Agent, this: Value, start: bool, end: bool) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    let mut from = 0;
    let mut to = s.len();
    if start {
        while from < to && is_str_whitespace(s[from]) {
            from += 1;
        }
    }
    if end {
        while to > from && is_str_whitespace(s[to - 1]) {
            to -= 1;
        }
    }
    make(agent, &s[from..to])
}

fn trim(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    trim_with(agent, this, true, true)
}

fn trim_start(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    trim_with(agent, this, true, false)
}

fn trim_end(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    trim_with(agent, this, false, true)
}

fn is_well_formed(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    Ok(Value::boolean(char::decode_utf16(s).all(|r| r.is_ok())))
}

fn to_well_formed(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    let mut out = Vec::with_capacity(s.len());
    for r in char::decode_utf16(s) {
        push_code_point(&mut out, r.map_or(0xFFFD, u32::from));
    }
    make(agent, &out)
}

/// Case mapping over the valid stretches of `s`, keeping lone
/// surrogates. Each stretch is mapped as a whole so that context-
/// dependent mappings (the final sigma) see their neighbors.
fn map_case(s: &[u16], upper: bool) -> Vec<u16> {
    if s.iter().all(|&u| u < 0x80) {
        return s
            .iter()
            .map(|&u| {
                let b = u as u8;
                u16::from(if upper {
                    b.to_ascii_uppercase()
                } else {
                    b.to_ascii_lowercase()
                })
            })
            .collect();
    }
    let mut out = Vec::with_capacity(s.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut Vec<u16>| {
        let mapped = if upper {
            run.to_uppercase()
        } else {
            run.to_lowercase()
        };
        out.extend(mapped.encode_utf16());
        run.clear();
    };
    for r in char::decode_utf16(s.iter().copied()) {
        match r {
            Ok(c) => run.push(c),
            Err(e) => {
                flush(&mut run, &mut out);
                out.push(e.unpaired_surrogate());
            }
        }
    }
    flush(&mut run, &mut out);
    out
}

fn to_lower_case(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    make(agent, &map_case(&s, false))
}

fn to_upper_case(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let s = this_units(agent, this)?;
    make(agent, &map_case(&s, true))
}

fn to_locale_lower_case(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(agent, this)?;
    let units = super::intl::locale_case_input(agent, units, arg(args, 0), false)?;
    make(agent, &map_case(&units, false))
}

fn to_locale_upper_case(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let units = this_units(agent, this)?;
    let units = super::intl::locale_case_input(agent, units, arg(args, 0), true)?;
    make(agent, &map_case(&units, true))
}

// ---- regular-expression entry points ------------------------------------------

/// Calls `value[symbol](...args)` when `value` is an object with that
/// method; primitives are never asked (ES2025).
fn delegate(
    agent: &mut Agent,
    value: Value,
    symbol: crate::runtime::SymbolId,
    receiver_args: &[Value],
) -> JsResult<Option<Value>> {
    if value.as_object().is_none() {
        return Ok(None);
    }
    match agent.get_method(value, PropertyKey::Symbol(symbol))? {
        Some(method) => agent.call(method, value, receiver_args).map(Some),
        None => Ok(None),
    }
}

/// `RegExpCreate(pattern, flags)` then `Invoke(rx, symbol, «ToString(this)»)`.
fn via_regexp(
    agent: &mut Agent,
    this: Value,
    pattern: Value,
    flags: Option<&str>,
    symbol: crate::runtime::SymbolId,
) -> JsResult<Value> {
    let s = agent.to_string(this)?;
    super::regexp::create_and_invoke(agent, pattern, flags, symbol, Value::string(s))
}

fn match_(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let this = agent.require_object_coercible(this)?;
    let symbol = agent.symbols.match_;
    if let Some(result) = delegate(agent, arg(args, 0), symbol, &[this])? {
        return Ok(result);
    }
    via_regexp(agent, this, arg(args, 0), None, symbol)
}

fn match_all(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let this = agent.require_object_coercible(this)?;
    let regexp = arg(args, 0);
    if is_regexp(agent, regexp)? {
        let o = regexp.as_object().expect("regular expressions are objects");
        let key = agent.heap.key("flags")?;
        let flags = agent.get(o, key)?;
        let flags = agent.require_object_coercible(flags)?;
        let flags = units_of(agent, flags)?;
        if !flags.contains(&u16::from(b'g')) {
            return agent.type_error("String.prototype.matchAll needs a global regular expression");
        }
    }
    let symbol = agent.symbols.match_all;
    if let Some(result) = delegate(agent, regexp, symbol, &[this])? {
        return Ok(result);
    }
    via_regexp(agent, this, regexp, Some("g"), symbol)
}

fn search(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let this = agent.require_object_coercible(this)?;
    let symbol = agent.symbols.search;
    if let Some(result) = delegate(agent, arg(args, 0), symbol, &[this])? {
        return Ok(result);
    }
    via_regexp(agent, this, arg(args, 0), None, symbol)
}

/// `GetSubstitution` (§22.1.3.19.1) with no captures.
fn substitution(matched: &[u16], s: &[u16], position: usize, template: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(template.len());
    let mut i = 0;
    while i < template.len() {
        let u = template[i];
        if u == u16::from(b'$') && i + 1 < template.len() {
            let next = template[i + 1];
            match u8::try_from(next).unwrap_or(0) {
                b'$' => {
                    out.push(u);
                    i += 2;
                    continue;
                }
                b'&' => {
                    out.extend_from_slice(matched);
                    i += 2;
                    continue;
                }
                b'`' => {
                    out.extend_from_slice(&s[..position]);
                    i += 2;
                    continue;
                }
                b'\'' => {
                    let tail = (position + matched.len()).min(s.len());
                    out.extend_from_slice(&s[tail..]);
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        out.push(u);
        i += 1;
    }
    out
}

/// The replacement text for one match at `position`.
fn replacement(
    agent: &mut Agent,
    replace_value: Value,
    template: Option<&[u16]>,
    matched: &[u16],
    s: &[u16],
    position: usize,
) -> JsResult<Vec<u16>> {
    match template {
        Some(template) => Ok(substitution(matched, s, position, template)),
        None => {
            let matched_value = make(agent, matched)?;
            let whole = make(agent, s)?;
            let result = agent.call(
                replace_value,
                Value::UNDEFINED,
                &[matched_value, Value::number(position as f64), whole],
            )?;
            units_of(agent, result)
        }
    }
}

/// `String.prototype.replace` (§22.1.3.19).
fn replace(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let this = agent.require_object_coercible(this)?;
    let (search_value, replace_value) = (arg(args, 0), arg(args, 1));
    let symbol = agent.symbols.replace;
    if let Some(result) = delegate(agent, search_value, symbol, &[this, replace_value])? {
        return Ok(result);
    }
    let s = units_of(agent, this)?;
    let search = units_of(agent, search_value)?;
    let template = if agent.is_callable(replace_value) {
        None
    } else {
        Some(units_of(agent, replace_value)?)
    };
    let Some(position) = find(&s, &search, 0) else {
        return make(agent, &s);
    };
    let middle = replacement(
        agent,
        replace_value,
        template.as_deref(),
        &search,
        &s,
        position,
    )?;
    let out = [&s[..position], &middle[..], &s[position + search.len()..]].concat();
    agent.heap.charge(2 * out.len())?;
    make(agent, &out)
}

/// `String.prototype.replaceAll` (§22.1.3.20).
fn replace_all(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let this = agent.require_object_coercible(this)?;
    let (search_value, replace_value) = (arg(args, 0), arg(args, 1));
    if search_value.as_object().is_some() {
        if is_regexp(agent, search_value)? {
            let o = search_value
                .as_object()
                .expect("regular expressions are objects");
            let key = agent.heap.key("flags")?;
            let flags = agent.get(o, key)?;
            let flags = agent.require_object_coercible(flags)?;
            let flags = units_of(agent, flags)?;
            if !flags.contains(&u16::from(b'g')) {
                return agent
                    .type_error("String.prototype.replaceAll needs a global regular expression");
            }
        }
        let symbol = agent.symbols.replace;
        if let Some(result) = delegate(agent, search_value, symbol, &[this, replace_value])? {
            return Ok(result);
        }
    }
    let s = units_of(agent, this)?;
    let search = units_of(agent, search_value)?;
    let template = if agent.is_callable(replace_value) {
        None
    } else {
        Some(units_of(agent, replace_value)?)
    };
    let advance = search.len().max(1);
    let mut positions = Vec::new();
    let mut at = find(&s, &search, 0);
    while let Some(p) = at {
        agent.heap.charge(8)?;
        positions.push(p);
        at = find(&s, &search, p + advance);
    }
    let mut end_of_last = 0;
    let mut out = Vec::new();
    for p in positions {
        let middle = replacement(agent, replace_value, template.as_deref(), &search, &s, p)?;
        agent.heap.charge(2 * (p - end_of_last + middle.len()))?;
        out.extend_from_slice(&s[end_of_last..p]);
        out.extend_from_slice(&middle);
        end_of_last = p + search.len();
    }
    out.extend_from_slice(&s[end_of_last.min(s.len())..]);
    make(agent, &out)
}

/// `String.prototype.split` (§22.1.3.23).
fn split(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let this = agent.require_object_coercible(this)?;
    let (separator, limit) = (arg(args, 0), arg(args, 1));
    let symbol = agent.symbols.split;
    if let Some(result) = delegate(agent, separator, symbol, &[this, limit])? {
        return Ok(result);
    }
    let s = units_of(agent, this)?;
    let lim = if limit == Value::UNDEFINED {
        u32::MAX
    } else {
        let n = agent.to_number(limit)?;
        to_uint32(n)
    };
    let r = units_of(agent, separator)?;
    let mut pieces: Vec<Value> = Vec::new();
    if lim == 0 {
        return Ok(Value::object(agent.create_array_from_list(&[])?));
    }
    if separator == Value::UNDEFINED {
        let whole = make(agent, &s)?;
        return Ok(Value::object(agent.create_array_from_list(&[whole])?));
    }
    let lim = lim as usize;
    if r.is_empty() {
        for &u in s.iter().take(lim) {
            agent.heap.charge(8)?;
            pieces.push(make(agent, &[u])?);
        }
        return Ok(Value::object(agent.create_array_from_list(&pieces)?));
    }
    if s.is_empty() {
        let whole = make(agent, &s)?;
        return Ok(Value::object(agent.create_array_from_list(&[whole])?));
    }
    let mut start = 0;
    let mut at = find(&s, &r, 0);
    while let Some(p) = at {
        agent.heap.charge(8)?;
        pieces.push(make(agent, &s[start..p])?);
        if pieces.len() >= lim {
            return Ok(Value::object(agent.create_array_from_list(&pieces)?));
        }
        start = p + r.len();
        at = find(&s, &r, start);
    }
    pieces.push(make(agent, &s[start..])?);
    Ok(Value::object(agent.create_array_from_list(&pieces)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn case_mapping_keeps_lone_surrogates_and_final_sigma() {
        assert_eq!(map_case(&units("Straße"), true), units("STRASSE"));
        assert_eq!(map_case(&units("ΟΔΟΣ"), false), units("οδος"));
        let mut lone = units("a");
        lone.push(0xD800);
        lone.extend(units("b"));
        assert_eq!(map_case(&lone, true), vec![0x41, 0xD800, 0x42]);
    }

    #[test]
    fn substitution_patterns() {
        let s = units("abcde");
        let out = substitution(&units("cd"), &s, 2, &units("[$&|$`|$'|$$|$1]"));
        assert_eq!(String::from_utf16_lossy(&out), "[cd|ab|e|$|$1]");
    }
}
