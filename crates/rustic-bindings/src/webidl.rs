//! Web IDL conversions for host bindings.
//!
//! String and numeric coercions can execute JavaScript. These functions keep
//! the input object rooted until its coercion completes. Callers keep any
//! other live JavaScript values rooted across the call.

use rustic_dom::DomString;
use rustic_js::runtime::{Abrupt, Agent, JsResult, JsStr, StringId, Value, to_int32, to_uint32};

/// Conversion policy for an IDL `DOMString` argument.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DomStringConversion {
    /// Apply JavaScript's `ToString`.
    #[default]
    Default,
    /// Apply `[LegacyNullToEmptyString]` to the null value.
    LegacyNullToEmptyString,
}

/// Conversion policy for a Web IDL integer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IntegerConversion {
    /// Truncate and wrap modulo the destination width.
    #[default]
    Wrap,
    /// `[Clamp]`: clamp to the range and round half to even.
    Clamp,
    /// `[EnforceRange]`: truncate and reject non-finite or out-of-range values.
    EnforceRange,
}

/// Convert a value to `DOMString`, preserving all UTF-16 units.
pub fn to_dom_string(
    agent: &mut Agent,
    value: Value,
    conversion: DomStringConversion,
) -> JsResult<DomString> {
    if value == Value::NULL && conversion == DomStringConversion::LegacyNullToEmptyString {
        return Ok(DomString::default());
    }
    let id = coerce_string(agent, value)?;
    let s = agent.heap.string(id);
    Ok(DomString::from_units(copy_exact(s.len(), s.units())?))
}

/// Convert a value to `DOMString?`. Null and undefined become `None`.
pub fn to_nullable_dom_string(agent: &mut Agent, value: Value) -> JsResult<Option<DomString>> {
    if value.is_nullish() {
        return Ok(None);
    }
    to_dom_string(agent, value, DomStringConversion::Default).map(Some)
}

/// Convert a DOM string to a JavaScript string with the same UTF-16 units.
pub fn from_dom_string(agent: &mut Agent, value: &DomString) -> JsResult<Value> {
    from_dom_units(agent, value.as_units())
}

pub(crate) fn from_dom_units(agent: &mut Agent, units: &[u16]) -> JsResult<Value> {
    let contents = if units.iter().all(|&unit| unit <= u16::from(u8::MAX)) {
        let bytes = copy_exact(units.len(), units.iter().map(|&unit| unit as u8))?;
        JsStr::Latin1(bytes.into_boxed_slice())
    } else {
        let wide = copy_exact(units.len(), units.iter().copied())?;
        JsStr::Utf16(wide.into_boxed_slice())
    };
    agent.string_from(contents)
}

/// Convert a value to `ByteString`, throwing TypeError for units above 255.
pub fn to_byte_string(agent: &mut Agent, value: Value) -> JsResult<Vec<u8>> {
    let id = coerce_string(agent, value)?;
    let s = agent.heap.string(id);
    if s.units().any(|unit| unit > u16::from(u8::MAX)) {
        return agent.type_error("ByteString contains a code unit greater than 255");
    }
    copy_exact(s.len(), s.units().map(|unit| unit as u8))
}

/// Convert a value to `USVString`, replacing unpaired surrogates with U+FFFD.
pub fn to_usv_string(agent: &mut Agent, value: Value) -> JsResult<String> {
    let id = coerce_string(agent, value)?;
    let s = agent.heap.string(id);
    let chars =
        || char::decode_utf16(s.units()).map(|value| value.unwrap_or(char::REPLACEMENT_CHARACTER));
    let bytes = chars()
        .try_fold(0_usize, |len, ch| len.checked_add(ch.len_utf8()))
        .ok_or(Abrupt::OutOfMemory)?;
    let mut text = String::new();
    text.try_reserve_exact(bytes)
        .map_err(|_| Abrupt::OutOfMemory)?;
    text.extend(chars());
    Ok(text)
}

/// Convert a value to IDL `long`, which is a signed 32-bit integer.
pub fn to_long(agent: &mut Agent, value: Value, conversion: IntegerConversion) -> JsResult<i32> {
    // Each conversion mode confines the result to the signed range.
    integer32(agent, value, conversion, true).map(|value| value as i32)
}

/// Convert a value to IDL `unsigned long`, which is an unsigned 32-bit integer.
pub fn to_unsigned_long(
    agent: &mut Agent,
    value: Value,
    conversion: IntegerConversion,
) -> JsResult<u32> {
    // Each conversion mode confines the result to the unsigned range.
    integer32(agent, value, conversion, false).map(|value| value as u32)
}

fn integer32(
    agent: &mut Agent,
    value: Value,
    conversion: IntegerConversion,
    signed: bool,
) -> JsResult<f64> {
    let number = with_input_root(agent, value, Agent::to_number)?;
    let (lower, upper) = if signed {
        (f64::from(i32::MIN), f64::from(i32::MAX))
    } else {
        (0.0, f64::from(u32::MAX))
    };
    match conversion {
        IntegerConversion::Wrap if signed => Ok(f64::from(to_int32(number))),
        IntegerConversion::Wrap => Ok(f64::from(to_uint32(number))),
        IntegerConversion::Clamp if number.is_nan() => Ok(0.0),
        IntegerConversion::Clamp => Ok(number.clamp(lower, upper).round_ties_even()),
        IntegerConversion::EnforceRange => {
            let integer = number.trunc();
            if (lower..=upper).contains(&integer) {
                Ok(integer)
            } else {
                agent.type_error("number is outside the Web IDL integer range")
            }
        }
    }
}

pub(crate) fn coerce_string(agent: &mut Agent, value: Value) -> JsResult<StringId> {
    with_input_root(agent, value, Agent::to_string)
}

fn with_input_root<T>(
    agent: &mut Agent,
    value: Value,
    coerce: impl FnOnce(&mut Agent, Value) -> JsResult<T>,
) -> JsResult<T> {
    if value.as_object().is_none() {
        return coerce(agent, value);
    }
    let root = agent.root(value)?;
    let result = coerce(agent, value);
    agent.unroot(root);
    result
}

fn copy_exact<T>(length: usize, values: impl Iterator<Item = T>) -> JsResult<Vec<T>> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| Abrupt::OutOfMemory)?;
    output.extend(values);
    Ok(output)
}
