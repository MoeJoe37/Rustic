//! Number conversions: `Number::toString` (§6.1.6.1.20), `StringToNumber`
//! (§7.1.4.1.1) and the integer conversions (§7.1.6–§7.1.11).

use super::string::JsStr;
use crate::lexer::radix_to_f64;
use crate::unicode::{is_line_terminator, is_whitespace};

/// `Number::toString(x, 10)`: the shortest digits that round-trip, laid
/// out as ECMA-262 specifies.
pub fn number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x < 0.0 {
        return format!("-{}", number_to_string(-x));
    }
    if x.is_infinite() {
        return "Infinity".into();
    }
    // `{:e}` prints the shortest round-trip digits as `d[.ddd]e[-]n`.
    let scientific = format!("{x:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("`{:e}` output of a finite number has an exponent");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i32 = exponent
        .parse()
        .expect("`{:e}` exponent of a finite number is an integer");
    let k = digits.len() as i32;
    let n = exponent + 1;
    if k <= n && n <= 21 {
        let mut out = digits;
        out.extend(std::iter::repeat_n('0', (n - k) as usize));
        out
    } else if 0 < n && n <= 21 {
        let (int, frac) = digits.split_at(n as usize);
        format!("{int}.{frac}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let sign = if e < 0 { '-' } else { '+' };
        let (first, rest) = digits.split_at(1);
        if rest.is_empty() {
            format!("{first}e{sign}{}", e.abs())
        } else {
            format!("{first}.{rest}e{sign}{}", e.abs())
        }
    }
}

fn is_str_whitespace(unit: u16) -> bool {
    let cp = u32::from(unit);
    is_whitespace(cp) || is_line_terminator(cp)
}

/// `StringToNumber`: the StringNumericLiteral grammar, or NaN.
pub fn string_to_number(s: &JsStr) -> f64 {
    let units = s.to_units();
    let start = units.iter().position(|&u| !is_str_whitespace(u));
    let Some(start) = start else {
        return 0.0;
    };
    let end = units
        .iter()
        .rposition(|&u| !is_str_whitespace(u))
        .map_or(start, |e| e + 1);
    // Anything outside ASCII is not part of the grammar.
    let Ok(text) = units[start..end]
        .iter()
        .map(|&u| {
            u8::try_from(u)
                .ok()
                .filter(u8::is_ascii)
                .map(char::from)
                .ok_or(())
        })
        .collect::<Result<String, ()>>()
    else {
        return f64::NAN;
    };
    let bytes = text.as_bytes();
    if bytes.len() > 2 && bytes[0] == b'0' {
        let radix = match bytes[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            let digits = &text[2..];
            return if digits.chars().all(|c| c.is_digit(radix)) {
                radix_to_f64(digits, radix)
            } else {
                f64::NAN
            };
        }
    }
    let (sign, unsigned) = match bytes[0] {
        b'+' => (1.0, &text[1..]),
        b'-' => (-1.0, &text[1..]),
        _ => (1.0, text.as_str()),
    };
    if unsigned == "Infinity" {
        return sign * f64::INFINITY;
    }
    if !is_decimal_literal(unsigned.as_bytes()) {
        return f64::NAN;
    }
    sign * unsigned.parse::<f64>().unwrap_or(f64::NAN)
}

/// StrUnsignedDecimalLiteral without `Infinity`: digits with an optional
/// fraction (at least one digit overall) and an optional exponent.
fn is_decimal_literal(b: &[u8]) -> bool {
    let mut i = 0;
    let int_digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    i += int_digits;
    let mut frac_digits = 0;
    if b.get(i) == Some(&b'.') {
        i += 1;
        frac_digits = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        i += frac_digits;
    }
    if int_digits + frac_digits == 0 {
        return false;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let exp_digits = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        if exp_digits == 0 {
            return false;
        }
        i += exp_digits;
    }
    i == b.len()
}

/// The integer part of `x` modulo 2³², for ToInt32 and ToUint32.
fn modulo_2_32(x: f64) -> u32 {
    if !x.is_finite() {
        return 0;
    }
    let int = x.trunc();
    let m = int.rem_euclid(4_294_967_296.0);
    // `rem_euclid` leaves `m` in [0, 2³²), so it fits in `u32`.
    m as u32
}

/// `ToUint32` of a Number.
pub fn to_uint32(x: f64) -> u32 {
    modulo_2_32(x)
}

/// `ToInt32` of a Number.
pub fn to_int32(x: f64) -> i32 {
    modulo_2_32(x) as i32
}

/// `ToUint16` of a Number.
pub fn to_uint16(x: f64) -> u16 {
    // Truncation is the modulo 2¹⁶ step of the definition.
    modulo_2_32(x) as u16
}

/// `ToIntegerOrInfinity` of a Number.
pub fn to_integer_or_infinity(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 {
        0.0
    } else {
        x.trunc()
    }
}

/// `ToLength` of a Number: an integer in [0, 2⁵³ − 1].
pub fn to_length(x: f64) -> f64 {
    to_integer_or_infinity(x).clamp(0.0, 9_007_199_254_740_991.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_to_string_matches_ecma262_layout() {
        for (n, s) in [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.5, "-1.5"),
            (0.1, "0.1"),
            (0.1 + 0.2, "0.30000000000000004"),
            (123e18, "123000000000000000000"),
            (123e20, "1.23e+22"),
            (1e21, "1e+21"),
            (1.5e21, "1.5e+21"),
            (0.000001, "0.000001"),
            (0.0000001, "1e-7"),
            (1.25e-7, "1.25e-7"),
            (5e-324, "5e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
            (f64::NAN, "NaN"),
            (9007199254740992.0, "9007199254740992"),
            (123.456, "123.456"),
        ] {
            assert_eq!(number_to_string(n), s, "{n:e}");
        }
    }

    #[test]
    fn string_to_number_follows_the_grammar() {
        let num = |s: &str| string_to_number(&JsStr::from_text(s));
        assert_eq!(num(""), 0.0);
        assert_eq!(num(" \n\u{2028}\u{FEFF} "), 0.0);
        assert_eq!(num(" 42 "), 42.0);
        assert_eq!(num("-0").to_bits(), (-0.0f64).to_bits());
        assert_eq!(num("+.5"), 0.5);
        assert_eq!(num("5."), 5.0);
        assert_eq!(num("1e3"), 1000.0);
        assert_eq!(num("1E-3"), 0.001);
        assert_eq!(num("0x1F"), 31.0);
        assert_eq!(num("0b101"), 5.0);
        assert_eq!(num("0o17"), 15.0);
        assert_eq!(num("-Infinity"), f64::NEG_INFINITY);
        for bad in [
            "-0x1",
            "0x",
            "1_000",
            "1e",
            ".",
            "e5",
            "inf",
            "Infinityx",
            "1 2",
            "١",
            "NaN",
            "0x1g",
        ] {
            assert!(num(bad).is_nan(), "{bad}");
        }
        assert_eq!(num("0.1000000000000000055511151231257827"), 0.1);
    }

    #[test]
    fn integer_conversions_wrap() {
        assert_eq!(to_int32(2_147_483_648.0), -2_147_483_648);
        assert_eq!(to_int32(-1.9), -1);
        assert_eq!(to_uint32(-1.0), 4_294_967_295);
        assert_eq!(to_uint32(f64::NAN), 0);
        assert_eq!(to_uint32(f64::INFINITY), 0);
        assert_eq!(to_uint32(4_294_967_297.5), 1);
        assert_eq!(to_uint16(65_537.0), 1);
        assert_eq!(to_length(-5.0), 0.0);
        assert_eq!(to_length(f64::INFINITY), 9_007_199_254_740_991.0);
    }
}
