//! BigInt storage (§6.1.6.2): sign and magnitude in base 2³² limbs, least
//! significant first, with no leading zero limbs. Arithmetic arrives with
//! JS-08 (TD-92); this module holds values, compares them and prints them.

use std::cmp::Ordering;

/// An arbitrary-precision integer.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BigInt {
    negative: bool,
    magnitude: Vec<u32>,
}

impl BigInt {
    /// Zero.
    pub fn zero() -> Self {
        Self::default()
    }

    fn normalize(mut self) -> Self {
        while self.magnitude.last() == Some(&0) {
            self.magnitude.pop();
        }
        if self.magnitude.is_empty() {
            self.negative = false;
        }
        self
    }

    /// ASCII digits in `radix` (2 to 36), as the lexer stores BigInt
    /// literals. Returns `None` for an invalid digit.
    pub fn from_digits(digits: &str, radix: u32, negative: bool) -> Option<Self> {
        let mut magnitude: Vec<u32> = Vec::new();
        for c in digits.chars() {
            let d = c.to_digit(radix)?;
            let mut carry = u64::from(d);
            for limb in &mut magnitude {
                let v = u64::from(*limb) * u64::from(radix) + carry;
                // The low 32 bits are the new limb; the rest carries.
                *limb = v as u32;
                carry = v >> 32;
            }
            if carry != 0 {
                magnitude.push(carry as u32);
            }
        }
        Some(
            Self {
                negative,
                magnitude,
            }
            .normalize(),
        )
    }

    /// The exact BigInt of an integral Number, or `None` for a fraction,
    /// NaN or an infinity (`NumberToBigInt` throws a RangeError then).
    pub fn from_f64(x: f64) -> Option<Self> {
        if !x.is_finite() || x.fract() != 0.0 {
            return None;
        }
        let bits = x.abs().to_bits();
        let exponent = ((bits >> 52) & 0x7FF) as i32;
        if exponent == 0 {
            return Some(Self::zero());
        }
        let mantissa = (bits & ((1 << 52) - 1)) | (1 << 52);
        let shift = exponent - 1075;
        let mut magnitude = vec![mantissa as u32, (mantissa >> 32) as u32];
        let mut out = if shift >= 0 {
            let limbs = (shift / 32) as usize;
            let bits = shift % 32;
            let mut shifted = vec![0u32; limbs];
            let mut carry = 0u32;
            for limb in magnitude.drain(..) {
                let wide = (u64::from(limb) << bits) | u64::from(carry);
                shifted.push(wide as u32);
                carry = (wide >> 32) as u32;
            }
            shifted.push(carry);
            shifted
        } else {
            // An integral value has no set bits below 2⁰.
            let value = mantissa >> (-shift);
            vec![value as u32, (value >> 32) as u32]
        };
        out.shrink_to_fit();
        Some(
            Self {
                negative: x < 0.0,
                magnitude: out,
            }
            .normalize(),
        )
    }

    /// Whether the value is zero.
    pub fn is_zero(&self) -> bool {
        self.magnitude.is_empty()
    }

    /// Whether the value is below zero.
    pub fn is_negative(&self) -> bool {
        self.negative
    }

    /// Bytes of limb storage.
    pub fn storage_bytes(&self) -> usize {
        self.magnitude.len() * 4
    }

    fn cmp_magnitude(a: &[u32], b: &[u32]) -> Ordering {
        a.len()
            .cmp(&b.len())
            .then_with(|| a.iter().rev().cmp(b.iter().rev()))
    }

    /// The value as the nearest Number, ties to even.
    pub fn to_f64(&self) -> f64 {
        // Exact for up to 53 significant bits; otherwise round with the
        // decimal parser, which is correctly rounded.
        if self.magnitude.len() <= 1 {
            let m = f64::from(self.magnitude.first().copied().unwrap_or(0));
            return if self.negative { -m } else { m };
        }
        self.to_string_radix10().parse().unwrap_or(f64::NAN)
    }

    /// Decimal digits with a `-` sign when negative (`BigInt::toString`).
    pub fn to_string_radix10(&self) -> String {
        if self.is_zero() {
            return "0".into();
        }
        let mut limbs = self.magnitude.clone();
        let mut chunks: Vec<u32> = Vec::new();
        while !limbs.is_empty() {
            let mut remainder = 0u64;
            for limb in limbs.iter_mut().rev() {
                let v = (remainder << 32) | u64::from(*limb);
                // The quotient of a value below 10⁹ · 2³² by 10⁹ fits in 32 bits.
                *limb = (v / 1_000_000_000) as u32;
                remainder = v % 1_000_000_000;
            }
            chunks.push(remainder as u32);
            while limbs.last() == Some(&0) {
                limbs.pop();
            }
        }
        let mut out = String::new();
        if self.negative {
            out.push('-');
        }
        let mut iter = chunks.iter().rev();
        if let Some(first) = iter.next() {
            out.push_str(&first.to_string());
        }
        for chunk in iter {
            out.push_str(&format!("{chunk:09}"));
        }
        out
    }

    /// Compare with a Number exactly (for `<` and `==`); `None` for NaN.
    pub fn cmp_f64(&self, x: f64) -> Option<Ordering> {
        if x.is_nan() {
            return None;
        }
        if x == f64::INFINITY {
            return Some(Ordering::Less);
        }
        if x == f64::NEG_INFINITY {
            return Some(Ordering::Greater);
        }
        let floor = x.floor();
        let whole = BigInt::from_f64(floor).expect("a finite floor is integral");
        Some(match self.cmp(&whole) {
            // Equal to the floor: below x exactly when x has a fraction.
            Ordering::Equal if floor < x => Ordering::Less,
            other => other,
        })
    }
}

impl PartialOrd for BigInt {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BigInt {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => Self::cmp_magnitude(&self.magnitude, &other.magnitude),
            (true, true) => Self::cmp_magnitude(&other.magnitude, &self.magnitude),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_print_in_decimal() {
        let big = BigInt::from_digits("123456789012345678901234567890", 10, false).unwrap();
        assert_eq!(big.to_string_radix10(), "123456789012345678901234567890");
        assert_eq!(
            BigInt::from_digits("ff", 16, true)
                .unwrap()
                .to_string_radix10(),
            "-255"
        );
        assert_eq!(
            BigInt::from_digits("000", 10, true).unwrap(),
            BigInt::zero()
        );
        assert_eq!(
            BigInt::from_digits("1000000000", 10, false)
                .unwrap()
                .to_string_radix10(),
            "1000000000"
        );
        assert!(BigInt::from_digits("12a", 10, false).is_none());
    }

    #[test]
    fn numbers_convert_and_compare_exactly() {
        assert_eq!(
            BigInt::from_f64(2f64.powi(80)).unwrap().to_string_radix10(),
            "1208925819614629174706176"
        );
        assert_eq!(BigInt::from_f64(-42.0).unwrap().to_string_radix10(), "-42");
        assert!(BigInt::from_f64(0.5).is_none());
        let n = BigInt::from_digits("9007199254740993", 10, false).unwrap();
        assert_eq!(n.cmp_f64(9007199254740992.0), Some(Ordering::Greater));
        assert_eq!(n.to_f64(), 9007199254740992.0);
        let two = BigInt::from_digits("2", 10, false).unwrap();
        assert_eq!(two.cmp_f64(2.5), Some(Ordering::Less));
        assert_eq!(two.cmp_f64(1.5), Some(Ordering::Greater));
        assert_eq!(two.cmp_f64(2.0), Some(Ordering::Equal));
        assert_eq!(two.cmp_f64(f64::NAN), None);
        let minus_two = BigInt::from_digits("2", 10, true).unwrap();
        assert_eq!(minus_two.cmp_f64(-1.5), Some(Ordering::Less));
        assert!(minus_two < two);
    }
}
