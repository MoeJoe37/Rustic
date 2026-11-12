//! `Number` (§21.1): the constants and predicates of the constructor and
//! the formatting methods of `Number.prototype`.
//!
//! `toFixed`, `toExponential` and `toPrecision` round the exact binary
//! value, ties away from zero, as the specification requires, so they
//! work from the number's exact decimal expansion rather than from
//! Rust's formatting (which rounds ties to even).

use super::{Method, arg, install_methods, method};
use crate::runtime::ErrorKind;
use crate::runtime::agent::{Agent, JsResult};
use crate::runtime::number::{number_to_string, to_integer_or_infinity};
use crate::runtime::object::{Class, PropertyDescriptor};
use crate::runtime::value::Value;

const STATIC_METHODS: &[Method] = &[
    method("isFinite", is_finite, 1),
    method("isInteger", is_integer, 1),
    method("isNaN", is_nan, 1),
    method("isSafeInteger", is_safe_integer, 1),
];

const PROTOTYPE_METHODS: &[Method] = &[
    method("toExponential", to_exponential, 1),
    method("toFixed", to_fixed, 1),
    method("toLocaleString", to_locale_string, 0),
    method("toPrecision", to_precision, 1),
    method("toString", to_string, 1),
];

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let global = agent.intrinsics.global;
    let key = agent.heap.key("Number")?;
    let Some(number) = agent.get(global, key)?.as_object() else {
        unreachable!("the Number constructor is installed first");
    };
    install_methods(agent, number, STATIC_METHODS)?;
    let prototype = agent.intrinsics.number_prototype;
    install_methods(agent, prototype, PROTOTYPE_METHODS)?;
    for (name, value) in [
        ("EPSILON", f64::EPSILON),
        ("MAX_SAFE_INTEGER", MAX_SAFE_INTEGER),
        ("MAX_VALUE", f64::MAX),
        ("MIN_SAFE_INTEGER", -MAX_SAFE_INTEGER),
        ("MIN_VALUE", 5e-324),
        ("NaN", f64::NAN),
        ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
        ("POSITIVE_INFINITY", f64::INFINITY),
    ] {
        let key = agent.heap.key(name)?;
        agent.define_property_or_throw(
            number,
            key,
            PropertyDescriptor::data_with(Value::number(value), false, false, false),
        )?;
    }
    // `Number.parseFloat` and `Number.parseInt` are the global functions
    // themselves (§21.1.2.12, §21.1.2.13).
    for name in ["parseFloat", "parseInt"] {
        let key = agent.heap.key(name)?;
        let f = agent.get(global, key)?;
        agent.define_property_or_throw(
            number,
            key,
            PropertyDescriptor::data_with(f, true, false, true),
        )?;
    }
    Ok(())
}

fn is_finite(_: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(
        arg(args, 0).as_number().is_some_and(f64::is_finite),
    ))
}

fn is_integer(_: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(
        arg(args, 0)
            .as_number()
            .is_some_and(|n| n.is_finite() && n.trunc() == n),
    ))
}

fn is_nan(_: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(
        arg(args, 0).as_number().is_some_and(f64::is_nan),
    ))
}

fn is_safe_integer(_: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(arg(args, 0).as_number().is_some_and(|n| {
        n.is_finite() && n.trunc() == n && n.abs() <= MAX_SAFE_INTEGER
    })))
}

/// `thisNumberValue(value)` (§21.1.3.7.1).
fn this_number(agent: &mut Agent, this: Value) -> JsResult<f64> {
    if let Some(n) = this.as_number() {
        return Ok(n);
    }
    if let Some(o) = this.as_object() {
        if agent.class(o) == Class::Number {
            if let Some(n) = agent.primitive_of(o).and_then(Value::as_number) {
                return Ok(n);
            }
        }
    }
    agent.type_error("Number.prototype method called on an incompatible receiver")
}

fn range_error<T>(agent: &mut Agent, message: &str) -> JsResult<T> {
    Err(agent.throw(ErrorKind::RangeError, message))
}

/// `Number.prototype.toString(radix)` (§21.1.3.6).
fn to_string(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let x = this_number(agent, this)?;
    let radix = match arg(args, 0) {
        r if r == Value::UNDEFINED => 10.0,
        r => {
            let n = agent.to_number(r)?;
            to_integer_or_infinity(n)
        }
    };
    if !(2.0..=36.0).contains(&radix) {
        return range_error(agent, "toString() radix must be between 2 and 36");
    }
    if radix == 10.0 {
        return agent.string(&number_to_string(x));
    }
    // `radix` is an integer in [2, 36].
    agent.string(&number_to_radix_string(x, radix as u32))
}

fn to_locale_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let x = this_number(agent, this)?;
    agent.string(&number_to_string(x))
}

/// `Number.prototype.toFixed(fractionDigits)` (§21.1.3.3).
fn to_fixed(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let x = this_number(agent, this)?;
    let f = to_integer_or_infinity(agent.to_number(arg(args, 0))?);
    if !(0.0..=100.0).contains(&f) {
        return range_error(agent, "toFixed() digits must be between 0 and 100");
    }
    if !x.is_finite() || x.abs() >= 1e21 {
        return agent.string(&number_to_string(x));
    }
    // `f` is an integer in [0, 100].
    agent.string(&fixed(x, f as i32))
}

/// `Number.prototype.toExponential(fractionDigits)` (§21.1.3.2).
fn to_exponential(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let x = this_number(agent, this)?;
    let digits = arg(args, 0);
    let f = to_integer_or_infinity(agent.to_number(digits)?);
    if !x.is_finite() {
        return agent.string(&number_to_string(x));
    }
    if !(0.0..=100.0).contains(&f) {
        return range_error(agent, "toExponential() digits must be between 0 and 100");
    }
    let (sign, magnitude) = split_sign(x);
    let (digits, point) = if digits == Value::UNDEFINED {
        shortest_digits(magnitude)
    } else {
        // `f` is an integer in [0, 100].
        significant(magnitude, f as i32 + 1)
    };
    agent.string(&format!("{sign}{}", exponential_form(&digits, point)))
}

/// `Number.prototype.toPrecision(precision)` (§21.1.3.5).
fn to_precision(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let x = this_number(agent, this)?;
    let precision = arg(args, 0);
    if precision == Value::UNDEFINED {
        return agent.string(&number_to_string(x));
    }
    let p = to_integer_or_infinity(agent.to_number(precision)?);
    if !x.is_finite() {
        return agent.string(&number_to_string(x));
    }
    if !(1.0..=100.0).contains(&p) {
        return range_error(agent, "toPrecision() argument must be between 1 and 100");
    }
    // `p` is an integer in [1, 100].
    let p = p as i32;
    let (sign, magnitude) = split_sign(x);
    let (digits, point) = significant(magnitude, p);
    let e = point - 1;
    let text = if e < -6 || e >= p {
        exponential_form(&digits, point)
    } else {
        positional(&digits, point, (p - 1 - e).max(0))
    };
    agent.string(&format!("{sign}{text}"))
}

fn split_sign(x: f64) -> (&'static str, f64) {
    if x < 0.0 { ("-", -x) } else { ("", x.abs()) }
}

// ---- digit generation -------------------------------------------------------

/// Decimal digits `d` and a point `p` with `x = 0.d₁d₂… × 10^p`: the
/// exact value of the finite, non-negative `x`. Zero is `([], 0)`.
fn exact_decimal(x: f64) -> (Vec<u8>, i32) {
    if x == 0.0 {
        return (Vec::new(), 0);
    }
    let bits = x.to_bits();
    let biased = ((bits >> 52) & 0x7FF) as i32;
    let fraction = bits & ((1 << 52) - 1);
    let (mantissa, exponent) = if biased == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1 << 52), biased - 1075)
    };
    // A big integer in base 10⁹, least significant limb first.
    let mut big = vec![
        (mantissa % 1_000_000_000) as u32,
        (mantissa / 1_000_000_000) as u32,
    ];
    let multiply = |big: &mut Vec<u32>, factor: u64| {
        let mut carry = 0u64;
        for limb in big.iter_mut() {
            let v = u64::from(*limb) * factor + carry;
            *limb = (v % 1_000_000_000) as u32;
            carry = v / 1_000_000_000;
        }
        while carry > 0 {
            big.push((carry % 1_000_000_000) as u32);
            carry /= 1_000_000_000;
        }
    };
    // x = mantissa × 2^exponent; a negative power of two is a power of
    // five over the same power of ten.
    let (mut left, factor, chunk_factor, chunk) = if exponent >= 0 {
        (exponent, 2u64, 1u64 << 29, 29)
    } else {
        (-exponent, 5u64, 1_220_703_125u64, 13)
    };
    while left >= chunk {
        multiply(&mut big, chunk_factor);
        left -= chunk;
    }
    for _ in 0..left {
        multiply(&mut big, factor);
    }
    while big.len() > 1 && big.last() == Some(&0) {
        big.pop();
    }
    let mut text = big.last().expect("at least one limb").to_string();
    for limb in big.iter().rev().skip(1) {
        text.push_str(&format!("{limb:09}"));
    }
    let mut digits: Vec<u8> = text.bytes().map(|b| b - b'0').collect();
    let point = digits.len() as i32 + exponent.min(0);
    while digits.last() == Some(&0) {
        digits.pop();
    }
    (digits, point)
}

/// `digits`/`point` cut to their first `keep` digits (which may be zero
/// or negative), rounding half away from zero on the exact value.
fn round_at(mut digits: Vec<u8>, mut point: i32, keep: i32) -> (Vec<u8>, i32) {
    if keep < 0 {
        return (Vec::new(), point);
    }
    let keep = keep as usize;
    if digits.len() <= keep {
        return (digits, point);
    }
    let up = digits[keep] >= 5;
    digits.truncate(keep);
    if up {
        let mut i = keep;
        loop {
            if i == 0 {
                digits.insert(0, 1);
                point += 1;
                break;
            }
            i -= 1;
            if digits[i] == 9 {
                digits[i] = 0;
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    while digits.last() == Some(&0) {
        digits.pop();
    }
    (digits, point)
}

/// Exactly `count` significant digits of the non-negative `x`, padded
/// with zeros.
fn significant(x: f64, count: i32) -> (Vec<u8>, i32) {
    if x == 0.0 {
        return (vec![0; count as usize], 1);
    }
    let (digits, point) = exact_decimal(x);
    let (mut digits, point) = round_at(digits, point, count);
    digits.resize(count as usize, 0);
    (digits, point)
}

/// The shortest round-trip digits of the non-negative `x`.
fn shortest_digits(x: f64) -> (Vec<u8>, i32) {
    if x == 0.0 {
        return (vec![0], 1);
    }
    let scientific = format!("{x:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("`{:e}` output of a finite number has an exponent");
    let digits = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect();
    let exponent: i32 = exponent.parse().expect("an integer exponent");
    (digits, exponent + 1)
}

/// `d.ddde±n`.
fn exponential_form(digits: &[u8], point: i32) -> String {
    let mut out = String::new();
    out.push(char::from(b'0' + digits[0]));
    if digits.len() > 1 {
        out.push('.');
        out.extend(digits[1..].iter().map(|&d| char::from(b'0' + d)));
    }
    let e = point - 1;
    out.push_str(&format!("e{}{}", if e < 0 { '-' } else { '+' }, e.abs()));
    out
}

/// The digits laid out positionally with `fraction` digits after the
/// point (zeros filled in).
fn positional(digits: &[u8], point: i32, fraction: i32) -> String {
    let digit = |i: i32| -> char {
        let d = usize::try_from(i)
            .ok()
            .and_then(|i| digits.get(i))
            .copied()
            .unwrap_or(0);
        char::from(b'0' + d)
    };
    let mut out = String::new();
    if point <= 0 {
        out.push('0');
    } else {
        out.extend((0..point).map(digit));
    }
    if fraction > 0 {
        out.push('.');
        out.extend((point..point + fraction).map(digit));
    }
    out
}

/// `toFixed`'s layout of the finite `x` below 10²¹.
fn fixed(x: f64, f: i32) -> String {
    let (sign, magnitude) = split_sign(x);
    let (digits, point) = exact_decimal(magnitude);
    let (digits, point) = round_at(digits, point, point + f);
    format!("{sign}{}", positional(&digits, point, f))
}

/// `Number::toString(x, radix)` for radixes other than 10: as many
/// digits as tell `x` apart from its neighbors, the approach browsers
/// take (§6.1.6.1.20 leaves the digits implementation-approximated).
pub(crate) fn number_to_radix_string(x: f64, radix: u32) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let negative = x < 0.0;
    let value = x.abs();
    let base = f64::from(radix);
    let digit = |d: u32| char::from_digit(d, radix).expect("a digit below the radix");
    let mut integer = value.floor();
    let mut fraction = value - integer;
    // Half the distance to the next double: digits below it say nothing.
    let next = f64::from_bits(value.to_bits() + 1);
    let mut delta = (0.5 * (next - value)).max(f64::from_bits(1));
    let mut fraction_digits: Vec<u32> = Vec::new();
    if fraction >= delta {
        loop {
            fraction *= base;
            delta *= base;
            // `fraction` is below `base` here.
            let d = fraction.floor() as u32;
            fraction_digits.push(d);
            fraction -= f64::from(d);
            if (fraction > 0.5 || (fraction == 0.5 && d & 1 == 1)) && fraction + delta > 1.0 {
                // Round up and carry.
                loop {
                    match fraction_digits.pop() {
                        None => {
                            integer += 1.0;
                            break;
                        }
                        Some(last) if last + 1 < radix => {
                            fraction_digits.push(last + 1);
                            break;
                        }
                        Some(_) => {}
                    }
                }
                break;
            }
            if fraction < delta {
                break;
            }
        }
    }
    let mut integer_digits: Vec<char> = Vec::new();
    // Digits beyond the double's precision are zeros.
    while integer / base >= 9_007_199_254_740_992.0 {
        integer /= base;
        integer_digits.push('0');
    }
    loop {
        let remainder = integer % base;
        // `remainder` is an integer below the radix.
        integer_digits.push(digit(remainder as u32));
        integer = (integer - remainder) / base;
        if integer <= 0.0 {
            break;
        }
    }
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.extend(integer_digits.iter().rev());
    if !fraction_digits.is_empty() {
        out.push('.');
        out.extend(fraction_digits.iter().map(|&d| digit(d)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_decimal_expands_binary_values() {
        assert_eq!(exact_decimal(0.5), (vec![5], 0));
        assert_eq!(exact_decimal(1.25), (vec![1, 2, 5], 1));
        assert_eq!(exact_decimal(1000.0), (vec![1], 4));
        let (digits, point) = exact_decimal(0.1);
        assert_eq!(point, 0);
        assert_eq!(
            &digits[..20],
            &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 5, 5]
        );
        assert_eq!(exact_decimal(5e-324).1, -323);
    }

    #[test]
    fn formatting_rounds_ties_away_from_zero() {
        assert_eq!(fixed(0.5, 0), "1");
        assert_eq!(fixed(2.5, 0), "3");
        assert_eq!(fixed(1.25, 1), "1.3");
        assert_eq!(fixed(1.005, 2), "1.00");
        assert_eq!(fixed(-1.5, 0), "-2");
        assert_eq!(fixed(123.456, 5), "123.45600");
        assert_eq!(fixed(0.000001, 2), "0.00");
        assert_eq!(fixed(-0.000001, 2), "-0.00");
        assert_eq!(fixed(9.995, 2), "9.99");
        assert_eq!(fixed(99.5, 0), "100");
        let (d, p) = significant(123456.0, 2);
        assert_eq!(exponential_form(&d, p), "1.2e+5");
    }

    #[test]
    fn radix_strings_match_browsers() {
        assert_eq!(number_to_radix_string(255.0, 16), "ff");
        assert_eq!(number_to_radix_string(-255.5, 16), "-ff.8");
        assert_eq!(number_to_radix_string(0.5, 2), "0.1");
        assert_eq!(number_to_radix_string(35.0, 36), "z");
    }
}
