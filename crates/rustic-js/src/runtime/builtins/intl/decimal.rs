//! Exact decimal numbers and the digit rounding of ECMA-402 §16.5.3
//! (`FormatNumericToString`). Number formatting works on decimal digits,
//! never on binary floating point, so a BigInt or a numeric string of
//! any length keeps every digit and each rounding mode is exact.

use crate::runtime::{JsStr, number_to_string, string_to_number};

/// A finite decimal number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Decimal {
    pub(super) negative: bool,
    /// The significant digits (each 0 to 9), most significant first,
    /// with no leading or trailing zero. Empty for zero.
    pub(super) digits: Vec<u8>,
    /// Where the decimal point is: the value is `0.DIGITS × 10^point`.
    pub(super) point: i32,
}

/// An Intl mathematical value (§16.5.16 `ToIntlMathematicalValue`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Numeric {
    Finite(Decimal),
    Infinity { negative: bool },
    NaN,
}

/// The `roundingMode` option, in the order of its values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RoundingMode {
    Ceil,
    Floor,
    Expand,
    Trunc,
    HalfCeil,
    HalfFloor,
    HalfExpand,
    HalfTrunc,
    HalfEven,
}

pub(super) const ROUNDING_MODES: &[&str] = &[
    "ceil",
    "floor",
    "expand",
    "trunc",
    "halfCeil",
    "halfFloor",
    "halfExpand",
    "halfTrunc",
    "halfEven",
];

impl RoundingMode {
    pub(super) fn from_index(index: usize) -> Self {
        [
            Self::Ceil,
            Self::Floor,
            Self::Expand,
            Self::Trunc,
            Self::HalfCeil,
            Self::HalfFloor,
            Self::HalfExpand,
            Self::HalfTrunc,
            Self::HalfEven,
        ][index]
    }

    pub(super) fn name(self) -> &'static str {
        ROUNDING_MODES[self as usize]
    }

    /// `GetUnsignedRoundingMode` (§16.5.10): the mode for the magnitude
    /// of a number of the given sign.
    fn unsigned(self, negative: bool) -> Unsigned {
        match (self, negative) {
            (Self::Ceil, false) | (Self::Floor, true) | (Self::Expand, _) => Unsigned::Infinity,
            (Self::Ceil, true) | (Self::Floor, false) | (Self::Trunc, _) => Unsigned::Zero,
            (Self::HalfCeil, false) | (Self::HalfFloor, true) | (Self::HalfExpand, _) => {
                Unsigned::HalfInfinity
            }
            (Self::HalfCeil, true) | (Self::HalfFloor, false) | (Self::HalfTrunc, _) => {
                Unsigned::HalfZero
            }
            (Self::HalfEven, _) => Unsigned::HalfEven,
        }
    }
}

#[derive(Clone, Copy)]
enum Unsigned {
    Zero,
    Infinity,
    HalfZero,
    HalfInfinity,
    HalfEven,
}

/// Where a value lies between two neighboring multiples.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Between {
    Exact,
    BelowHalf,
    Half,
    AboveHalf,
}

impl Decimal {
    pub(super) fn zero(negative: bool) -> Self {
        Self {
            negative,
            digits: Vec::new(),
            point: 0,
        }
    }

    pub(super) fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    /// The exponent of the leading digit: `floor(log10(|x|))`, and 0
    /// for zero.
    pub(super) fn magnitude(&self) -> i32 {
        if self.is_zero() { 0 } else { self.point - 1 }
    }

    /// Multiply by `10^exponent`.
    pub(super) fn shift(&mut self, exponent: i32) {
        if !self.is_zero() {
            self.point += exponent;
        }
    }

    fn normalize(&mut self) {
        let leading = self.digits.iter().take_while(|&&d| d == 0).count();
        self.digits.drain(..leading);
        self.point -= leading as i32;
        while self.digits.last() == Some(&0) {
            self.digits.pop();
        }
        if self.digits.is_empty() {
            self.point = 0;
        }
    }

    /// Digits and a count of trailing zeros dropped, from decimal text
    /// `INT[.FRAC]` already checked to hold only digits and one point.
    fn from_parts(negative: bool, integer: &[u8], fraction: &[u8], exponent: i32) -> Self {
        let mut digits = Vec::with_capacity(integer.len() + fraction.len());
        digits.extend(integer.iter().chain(fraction).map(|byte| byte - b'0'));
        let mut value = Self {
            negative,
            digits,
            point: (integer.len() as i32).saturating_add(exponent),
        };
        value.normalize();
        value
    }

    /// Round to a multiple of `increment × 10^exponent`
    /// (`ApplyUnsignedRoundingMode`, §16.5.11, on exact digits).
    pub(super) fn round(&mut self, exponent: i32, increment: u32, mode: RoundingMode) {
        if self.is_zero() {
            return;
        }
        // The integer part N of the value scaled by 10^-exponent, and
        // how the rest F compares with one half.
        let cut = i64::from(self.point) - i64::from(exponent);
        let length = self.digits.len() as i64;
        let (mut integer, rest) = if cut <= 0 {
            let rest = if cut < 0 || self.digits[0] < 5 {
                Between::BelowHalf
            } else if self.digits[0] == 5 && self.digits.len() == 1 {
                Between::Half
            } else {
                Between::AboveHalf
            };
            (Vec::new(), rest)
        } else if cut >= length {
            // Already a multiple of 10^exponent; zeros are implied.
            if increment == 1 {
                return;
            }
            let mut integer = self.digits.clone();
            integer.resize(cut as usize, 0);
            (integer, Between::Exact)
        } else {
            let (kept, dropped) = self.digits.split_at(cut as usize);
            let rest = match dropped[0] {
                0..=4 => Between::BelowHalf,
                5 if dropped.len() == 1 => Between::Half,
                _ => Between::AboveHalf,
            };
            (kept.to_vec(), rest)
        };
        // N modulo twice the increment gives both the remainder and
        // whether the lower multiple is an even one.
        let modulus = u64::from(increment) * 2;
        let folded = integer
            .iter()
            .fold(0, |r, &digit| (r * 10 + u64::from(digit)) % modulus);
        let remainder = folded % u64::from(increment);
        let lower_is_odd = folded >= u64::from(increment);
        let increment = u64::from(increment);
        let between = if remainder == 0 && rest == Between::Exact {
            Between::Exact
        } else if rest == Between::Exact {
            match (2 * remainder).cmp(&increment) {
                std::cmp::Ordering::Less => Between::BelowHalf,
                std::cmp::Ordering::Equal => Between::Half,
                std::cmp::Ordering::Greater => Between::AboveHalf,
            }
        } else if 2 * remainder + 1 < increment {
            Between::BelowHalf
        } else if 2 * remainder + 1 == increment {
            rest
        } else {
            Between::AboveHalf
        };
        let up = match (between, mode.unsigned(self.negative)) {
            (Between::Exact, _) | (_, Unsigned::Zero) => false,
            (_, Unsigned::Infinity) => true,
            (Between::BelowHalf, _) => false,
            (Between::AboveHalf, _) => true,
            (Between::Half, Unsigned::HalfZero) => false,
            (Between::Half, Unsigned::HalfInfinity) => true,
            (Between::Half, Unsigned::HalfEven) => lower_is_odd,
        };
        // Both fit: the increment is at most 5000.
        let mut carry = if up { increment as i64 } else { 0 } - remainder as i64;
        for digit in integer.iter_mut().rev() {
            if carry == 0 {
                break;
            }
            let sum = i64::from(*digit) + carry;
            *digit = sum.rem_euclid(10) as u8;
            carry = sum.div_euclid(10);
        }
        while carry > 0 {
            integer.insert(0, (carry % 10) as u8);
            carry /= 10;
        }
        self.point = (integer.len() as i64 + i64::from(exponent)) as i32;
        self.digits = integer;
        self.normalize();
    }

    /// The digits before and after the decimal point, as ASCII: the
    /// integer part is at least `"0"` and the fraction has no trailing
    /// zero.
    pub(super) fn parts(&self) -> (String, String) {
        let digit = |d: &u8| char::from(b'0' + d);
        let length = self.digits.len() as i32;
        let mut integer = String::new();
        let mut fraction = String::new();
        if self.point <= 0 {
            integer.push('0');
            if !self.is_zero() {
                fraction.extend(std::iter::repeat_n('0', (-self.point) as usize));
                fraction.extend(self.digits.iter().map(digit));
            }
        } else if self.point >= length {
            integer.extend(self.digits.iter().map(digit));
            integer.extend(std::iter::repeat_n('0', (self.point - length) as usize));
        } else {
            let (before, after) = self.digits.split_at(self.point as usize);
            integer.extend(before.iter().map(digit));
            fraction.extend(after.iter().map(digit));
        }
        (integer, fraction)
    }

    /// How many significant digits `parts` shows.
    fn significant_shown(&self) -> usize {
        if self.is_zero() {
            1
        } else {
            self.digits.len().max(self.point.max(0) as usize)
        }
    }
}

impl Numeric {
    /// The value of a Number. The digits are those of `Number::toString`,
    /// the shortest that read back as the same double, which is what
    /// engines format (a double's exact binary value would turn 1.005
    /// into 1.00499999999999989…).
    pub(super) fn from_number(x: f64) -> Self {
        if x.is_nan() {
            return Self::NaN;
        }
        if x.is_infinite() {
            return Self::Infinity { negative: x < 0.0 };
        }
        let negative = x.is_sign_negative();
        match Self::parse_decimal(number_to_string(x.abs()).as_bytes()) {
            Some(mut value) => {
                value.negative = negative;
                Self::Finite(value)
            }
            None => Self::NaN,
        }
    }

    /// The value of a BigInt from its decimal text.
    pub(super) fn from_integer_text(text: &str) -> Self {
        let (negative, digits) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        Self::Finite(Decimal::from_parts(negative, digits.as_bytes(), &[], 0))
    }

    /// `StrDecimalLiteral` without a sign or `Infinity`.
    fn parse_decimal(text: &[u8]) -> Option<Decimal> {
        let digits = |bytes: &[u8]| bytes.iter().all(u8::is_ascii_digit);
        let (mantissa, exponent) = match text.iter().position(|&b| b == b'e' || b == b'E') {
            Some(at) => {
                let (sign, rest) = match text[at + 1..].split_first()? {
                    (b'-', rest) => (-1, rest),
                    (b'+', rest) => (1, rest),
                    _ => (1, &text[at + 1..]),
                };
                if rest.is_empty() || !digits(rest) {
                    return None;
                }
                // Longer exponents overflow or underflow a double anyway.
                let exponent = rest
                    .iter()
                    .fold(0i64, |e, b| (e * 10 + i64::from(b - b'0')).min(1_000_000));
                (&text[..at], (sign * exponent) as i32)
            }
            None => (text, 0),
        };
        let (integer, fraction) = match mantissa.iter().position(|&b| b == b'.') {
            Some(at) => (&mantissa[..at], &mantissa[at + 1..]),
            None => (mantissa, &[][..]),
        };
        if (integer.is_empty() && fraction.is_empty()) || !digits(integer) || !digits(fraction) {
            return None;
        }
        Some(Decimal::from_parts(false, integer, fraction, exponent))
    }

    /// The value of a String (§16.5.16 steps 3–4): the exact value of a
    /// `StringNumericLiteral`, except that what a Number would round to
    /// infinity or zero is that infinity or zero.
    pub(super) fn from_string(string: &JsStr) -> Self {
        let text = string.to_text();
        let trimmed = text.trim_matches(|c: char| {
            u16::try_from(u32::from(c)).is_ok_and(crate::runtime::number::is_str_whitespace)
        });
        let as_number = string_to_number(string);
        if as_number.is_nan() {
            return Self::NaN;
        }
        if as_number.is_infinite() {
            return Self::Infinity {
                negative: as_number < 0.0,
            };
        }
        let (negative, unsigned) = match trimmed.as_bytes().split_first() {
            Some((b'-', rest)) => (true, rest),
            Some((b'+', rest)) => (false, rest),
            _ => (false, trimmed.as_bytes()),
        };
        if as_number == 0.0 {
            return Self::Finite(Decimal::zero(negative));
        }
        let radix = match unsigned {
            [b'0', b'x' | b'X', ..] => 16,
            [b'0', b'o' | b'O', ..] => 8,
            [b'0', b'b' | b'B', ..] => 2,
            _ => 10,
        };
        if radix != 10 {
            // Decimal digits, least significant first.
            let mut decimal = vec![0u8];
            for byte in &unsigned[2..] {
                let mut carry = char::from(*byte).to_digit(radix).unwrap_or(0);
                for digit in &mut decimal {
                    let value = u32::from(*digit) * radix + carry;
                    *digit = (value % 10) as u8;
                    carry = value / 10;
                }
                while carry > 0 {
                    decimal.push((carry % 10) as u8);
                    carry /= 10;
                }
            }
            decimal.reverse();
            let mut value = Decimal {
                negative: false,
                point: decimal.len() as i32,
                digits: decimal,
            };
            value.normalize();
            return Self::Finite(value);
        }
        match Self::parse_decimal(unsigned) {
            Some(mut value) => {
                value.negative = negative;
                Self::Finite(value)
            }
            None => Self::NaN,
        }
    }
}

/// Which digit limits decide the rounding (`[[RoundingType]]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RoundingType {
    SignificantDigits,
    FractionDigits,
    MorePrecision,
    LessPrecision,
}

/// The digit options of a number format or plural rules object
/// (`SetNumberFormatDigitOptions`, §16.1.3).
#[derive(Clone, Copy, Debug)]
pub(super) struct DigitOptions {
    pub(super) minimum_integer: u32,
    pub(super) minimum_fraction: u32,
    pub(super) maximum_fraction: u32,
    pub(super) minimum_significant: u32,
    pub(super) maximum_significant: u32,
    pub(super) rounding_type: RoundingType,
    pub(super) increment: u32,
    pub(super) mode: RoundingMode,
    /// `trailingZeroDisplay` is `"stripIfInteger"`.
    pub(super) strip_if_integer: bool,
    /// `[[ComputedRoundingPriority]]`: 0 auto, 1 more, 2 less precision.
    pub(super) priority: u8,
}

/// A number rounded and cut into digit strings
/// (`FormatNumericToString`, §16.5.3).
#[derive(Clone, Debug)]
pub(super) struct Formatted {
    /// The rounded value, for its sign, zero test and plural operands.
    pub(super) rounded: Decimal,
    /// ASCII digits before the decimal point, padded to the minimum.
    pub(super) integer: String,
    /// ASCII digits after it; empty when there is no fraction.
    pub(super) fraction: String,
}

impl DigitOptions {
    /// `ToRawPrecision` (§16.5.8): the rounded value, its fraction padded
    /// to the minimum of significant digits, and the exponent of the
    /// last digit rounding kept.
    fn raw_precision(&self, x: &Decimal) -> (Decimal, String, String, i32) {
        let mut rounded = x.clone();
        let keep = self.maximum_significant as i32;
        rounded.round(rounded.magnitude() - (keep - 1), 1, self.mode);
        let magnitude = rounded.magnitude() - keep + 1;
        let (integer, mut fraction) = rounded.parts();
        let shown = rounded.significant_shown();
        let wanted = self.minimum_significant as usize;
        if wanted > shown {
            fraction.extend(std::iter::repeat_n('0', wanted - shown));
        }
        (rounded, integer, fraction, magnitude)
    }

    /// `ToRawFixed` (§16.5.9).
    fn raw_fixed(&self, x: &Decimal) -> (Decimal, String, String, i32) {
        let mut rounded = x.clone();
        let exponent = -(self.maximum_fraction as i32);
        rounded.round(exponent, self.increment, self.mode);
        let (integer, mut fraction) = rounded.parts();
        let wanted = self.minimum_fraction as usize;
        if wanted > fraction.len() {
            fraction.extend(std::iter::repeat_n('0', wanted - fraction.len()));
        }
        (rounded, integer, fraction, exponent)
    }

    /// `FormatNumericToString(self, x)` for a finite `x`.
    pub(super) fn format(&self, x: &Decimal) -> Formatted {
        let (rounded, mut integer, mut fraction) = match self.rounding_type {
            RoundingType::SignificantDigits => {
                let (rounded, integer, fraction, _) = self.raw_precision(x);
                (rounded, integer, fraction)
            }
            RoundingType::FractionDigits => {
                let (rounded, integer, fraction, _) = self.raw_fixed(x);
                (rounded, integer, fraction)
            }
            RoundingType::MorePrecision | RoundingType::LessPrecision => {
                let significant = self.raw_precision(x);
                let fixed = self.raw_fixed(x);
                let more = self.rounding_type == RoundingType::MorePrecision;
                let chosen = if (significant.3 <= fixed.3) == more {
                    significant
                } else {
                    fixed
                };
                (chosen.0, chosen.1, chosen.2)
            }
        };
        if self.strip_if_integer && fraction.bytes().all(|byte| byte == b'0') {
            fraction.clear();
        }
        let wanted = self.minimum_integer as usize;
        if wanted > integer.len() {
            integer.insert_str(0, &"0".repeat(wanted - integer.len()));
        }
        Formatted {
            rounded,
            integer,
            fraction,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decimal(text: &str) -> Decimal {
        match Numeric::from_string(&JsStr::from_text(text)) {
            Numeric::Finite(value) => value,
            other => panic!("{text} is {other:?}"),
        }
    }

    fn rounded(text: &str, exponent: i32, increment: u32, mode: RoundingMode) -> String {
        let mut value = decimal(text);
        value.round(exponent, increment, mode);
        let (integer, fraction) = value.parts();
        let sign = if value.negative { "-" } else { "" };
        if fraction.is_empty() {
            format!("{sign}{integer}")
        } else {
            format!("{sign}{integer}.{fraction}")
        }
    }

    #[test]
    fn strings_keep_every_digit_and_follow_the_number_grammar() {
        let big = "987654321987654321987654321.123456789123456789";
        let value = decimal(big);
        assert_eq!(value.parts().0, "987654321987654321987654321");
        assert_eq!(value.parts().1, "123456789123456789");
        assert_eq!(decimal("  0x1F ").parts().0, "31");
        assert_eq!(decimal("0b101").parts().0, "5");
        assert_eq!(decimal("1.5e3").parts().0, "1500");
        assert_eq!(decimal(".5").parts(), ("0".into(), "5".into()));
        assert_eq!(decimal("5.").parts(), ("5".into(), String::new()));
        assert_eq!(decimal("12e-4").parts(), ("0".into(), "0012".into()));
        assert!(decimal("-0").negative && decimal("-0").is_zero());
        assert!(decimal("").is_zero());
        for bad in ["1_000", "1,5", "abc", "1e", "-0x10", ".", "1e1.5"] {
            assert_eq!(
                Numeric::from_string(&JsStr::from_text(bad)),
                Numeric::NaN,
                "{bad}"
            );
        }
        assert_eq!(
            Numeric::from_string(&JsStr::from_text("-1e400")),
            Numeric::Infinity { negative: true }
        );
        assert!(decimal("1e-400").is_zero());
        assert_eq!(
            Numeric::from_number(1.005),
            Numeric::Finite(decimal("1.005"))
        );
        assert_eq!(Numeric::from_number(1e21), Numeric::Finite(decimal("1e21")));
        assert_eq!(Numeric::from_number(-0.0), Numeric::Finite(decimal("-0")));
    }

    #[test]
    fn every_rounding_mode_is_exact_on_both_signs() {
        use RoundingMode::*;
        // value, then the result per mode in declaration order.
        let table: &[(&str, [&str; 9])] = &[
            ("2.5", ["3", "2", "3", "2", "3", "2", "3", "2", "2"]),
            (
                "-2.5",
                ["-2", "-3", "-3", "-2", "-2", "-3", "-3", "-2", "-2"],
            ),
            ("3.5", ["4", "3", "4", "3", "4", "3", "4", "3", "4"]),
            ("2.4", ["3", "2", "3", "2", "2", "2", "2", "2", "2"]),
            (
                "-2.6",
                ["-2", "-3", "-3", "-2", "-3", "-3", "-3", "-3", "-3"],
            ),
            ("2", ["2", "2", "2", "2", "2", "2", "2", "2", "2"]),
            ("0.01", ["1", "0", "1", "0", "0", "0", "0", "0", "0"]),
            (
                "2.5000000000000000000001",
                ["3", "2", "3", "2", "3", "3", "3", "3", "3"],
            ),
        ];
        let modes = [
            Ceil, Floor, Expand, Trunc, HalfCeil, HalfFloor, HalfExpand, HalfTrunc, HalfEven,
        ];
        for (value, expected) in table {
            for (mode, want) in modes.iter().zip(expected) {
                let got = rounded(value, 0, 1, *mode);
                // A negative value that rounds to zero keeps its sign in
                // the decimal, as `-0` does.
                assert_eq!(got.trim_start_matches('-'), want.trim_start_matches('-'));
                assert_eq!(got.starts_with('-') && got != "-0", want.starts_with('-'));
            }
        }
    }

    #[test]
    fn rounding_carries_and_honors_increments() {
        use RoundingMode::*;
        assert_eq!(rounded("9.995", -2, 1, HalfExpand), "10");
        assert_eq!(
            rounded("999999999999999999999.5", 0, 1, HalfExpand),
            "1000000000000000000000"
        );
        assert_eq!(rounded("1234.5678", -2, 1, HalfEven), "1234.57");
        assert_eq!(rounded("1234.5678", 2, 1, HalfExpand), "1200");
        assert_eq!(rounded("0.0004", -2, 1, Expand), "0.01");
        // Nickel rounding: multiples of 0.05.
        assert_eq!(rounded("1.02", -2, 5, HalfExpand), "1");
        assert_eq!(rounded("1.025", -2, 5, HalfExpand), "1.05");
        assert_eq!(rounded("1.025", -2, 5, HalfEven), "1");
        assert_eq!(rounded("1.075", -2, 5, HalfEven), "1.1");
        assert_eq!(rounded("1.03", -2, 5, Floor), "1");
        assert_eq!(rounded("-1.03", -2, 5, Floor), "-1.05");
        assert_eq!(rounded("1.05", -2, 5, Ceil), "1.05");
        assert_eq!(rounded("12", 0, 5000, HalfExpand), "0");
        assert_eq!(rounded("2500", 0, 5000, HalfExpand), "5000");
        assert_eq!(rounded("2500", 0, 5000, HalfEven), "0");
        assert_eq!(rounded("7500", 0, 5000, HalfEven), "10000");
        assert_eq!(rounded("99999", 0, 250, Expand), "100000");
        assert_eq!(
            rounded("1e30", 0, 25, HalfExpand),
            format!("1{}", "0".repeat(30))
        );
        assert_eq!(rounded("12", 3, 25, HalfExpand), "0");
        assert_eq!(rounded("13000", 3, 25, HalfExpand), "25000");
    }

    #[test]
    fn digit_options_pad_and_choose_between_precisions() {
        let base = DigitOptions {
            minimum_integer: 1,
            minimum_fraction: 0,
            maximum_fraction: 3,
            minimum_significant: 1,
            maximum_significant: 21,
            rounding_type: RoundingType::FractionDigits,
            increment: 1,
            mode: RoundingMode::HalfExpand,
            strip_if_integer: false,
            priority: 0,
        };
        let show = |options: &DigitOptions, text: &str| {
            let formatted = options.format(&decimal(text));
            if formatted.fraction.is_empty() {
                formatted.integer
            } else {
                format!("{}.{}", formatted.integer, formatted.fraction)
            }
        };
        assert_eq!(show(&base, "1234.56789"), "1234.568");
        let fixed = DigitOptions {
            minimum_fraction: 2,
            maximum_fraction: 2,
            minimum_integer: 3,
            ..base
        };
        assert_eq!(show(&fixed, "1.005"), "001.01");
        assert_eq!(show(&fixed, "0"), "000.00");
        let significant = DigitOptions {
            rounding_type: RoundingType::SignificantDigits,
            minimum_significant: 3,
            maximum_significant: 4,
            ..base
        };
        assert_eq!(show(&significant, "0"), "0.00");
        assert_eq!(show(&significant, "1"), "1.00");
        assert_eq!(show(&significant, "123456"), "123500");
        assert_eq!(show(&significant, "0.00012345"), "0.0001235");
        assert_eq!(show(&significant, "9.9996"), "10.0");
        // 1.23456 with at most 2 fraction digits or 2 significant ones.
        let both = DigitOptions {
            maximum_fraction: 2,
            maximum_significant: 2,
            rounding_type: RoundingType::MorePrecision,
            ..base
        };
        assert_eq!(show(&both, "1.23456"), "1.23");
        assert_eq!(show(&both, "123.456"), "123.46");
        let less = DigitOptions {
            rounding_type: RoundingType::LessPrecision,
            ..both
        };
        assert_eq!(show(&less, "1.23456"), "1.2");
        assert_eq!(show(&less, "123.456"), "120");
        let strip = DigitOptions {
            strip_if_integer: true,
            ..fixed
        };
        assert_eq!(show(&strip, "5"), "005");
        assert_eq!(show(&strip, "5.1"), "005.10");
    }
}
