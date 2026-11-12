//! BigInt storage (§6.1.6.2): sign and magnitude in base 2³² limbs, least
//! significant first, with no leading zero limbs, and the arithmetic of
//! §6.1.6.2 on them. Results are capped at [`MAX_BITS`] so no operation
//! allocates or computes without bound (TD-112).

use std::cmp::Ordering;

/// The largest magnitude a result may have, in bits. Multiplication and
/// division are quadratic in the limb count, so the cap also bounds the
/// time one operation can take between safe points.
pub const MAX_BITS: u64 = 1 << 20;

/// A result would exceed [`MAX_BITS`] (a RangeError in script).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooBig;

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

    // ---- construction and narrowing ----------------------------------

    /// The BigInt of a signed 64-bit integer.
    pub fn from_i64(x: i64) -> Self {
        let mut out = Self::from_u64(x.unsigned_abs());
        out.negative = x < 0;
        out
    }

    /// The BigInt of an unsigned 64-bit integer.
    pub fn from_u64(x: u64) -> Self {
        Self {
            negative: false,
            // The two halves of the value are its limbs.
            magnitude: vec![x as u32, (x >> 32) as u32],
        }
        .normalize()
    }

    /// The value modulo 2⁶⁴ (`ToBigUint64`).
    pub fn to_u64_wrapping(&self) -> u64 {
        let low = u64::from(self.magnitude.first().copied().unwrap_or(0));
        let high = u64::from(self.magnitude.get(1).copied().unwrap_or(0));
        let magnitude = low | (high << 32);
        if self.negative {
            magnitude.wrapping_neg()
        } else {
            magnitude
        }
    }

    /// The value modulo 2⁶⁴ as a signed integer (`ToBigInt64`).
    pub fn to_i64_wrapping(&self) -> i64 {
        // Reinterpreting the bits is the two's complement wrap.
        self.to_u64_wrapping() as i64
    }

    /// The number of bits in the magnitude (0 for zero).
    pub fn bit_length(&self) -> u64 {
        match self.magnitude.last() {
            None => 0,
            Some(top) => {
                (self.magnitude.len() as u64 - 1) * 32 + u64::from(32 - top.leading_zeros())
            }
        }
    }

    fn checked(self) -> Result<Self, TooBig> {
        if self.bit_length() > MAX_BITS {
            Err(TooBig)
        } else {
            Ok(self)
        }
    }

    fn from_magnitude(magnitude: Vec<u32>, negative: bool) -> Self {
        Self {
            negative,
            magnitude,
        }
        .normalize()
    }

    fn one() -> Self {
        Self::from_u64(1)
    }

    // ---- magnitude helpers -------------------------------------------

    fn add_magnitude(a: &[u32], b: &[u32]) -> Vec<u32> {
        let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
        let mut out = Vec::with_capacity(long.len() + 1);
        let mut carry = 0u64;
        for (i, &limb) in long.iter().enumerate() {
            let sum = u64::from(limb) + u64::from(short.get(i).copied().unwrap_or(0)) + carry;
            // The low half is the limb and the high half the carry.
            out.push(sum as u32);
            carry = sum >> 32;
        }
        if carry != 0 {
            out.push(carry as u32);
        }
        out
    }

    /// `a - b` for `a >= b`.
    fn sub_magnitude(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut out = Vec::with_capacity(a.len());
        let mut borrow = 0i64;
        for (i, &limb) in a.iter().enumerate() {
            let mut d = i64::from(limb) - i64::from(b.get(i).copied().unwrap_or(0)) - borrow;
            borrow = 0;
            if d < 0 {
                d += 1 << 32;
                borrow = 1;
            }
            // `d` is now within one limb.
            out.push(d as u32);
        }
        out
    }

    fn mul_magnitude(a: &[u32], b: &[u32]) -> Vec<u32> {
        if a.is_empty() || b.is_empty() {
            return Vec::new();
        }
        let mut out = vec![0u32; a.len() + b.len()];
        for (i, &x) in a.iter().enumerate() {
            if x == 0 {
                continue;
            }
            let mut carry = 0u64;
            for (j, &y) in b.iter().enumerate() {
                let t = u64::from(x) * u64::from(y) + u64::from(out[i + j]) + carry;
                out[i + j] = t as u32;
                carry = t >> 32;
            }
            out[i + b.len()] = carry as u32;
        }
        out
    }

    fn shl_magnitude(a: &[u32], shift: u64) -> Vec<u32> {
        if a.is_empty() {
            return Vec::new();
        }
        // Callers bound `shift` by `MAX_BITS`, so it fits in `usize`.
        let limbs = (shift / 32) as usize;
        let bits = (shift % 32) as u32;
        let mut out = vec![0u32; limbs];
        let mut carry = 0u32;
        for &limb in a {
            let wide = (u64::from(limb) << bits) | u64::from(carry);
            out.push(wide as u32);
            carry = (wide >> 32) as u32;
        }
        if carry != 0 {
            out.push(carry);
        }
        out
    }

    fn shr_magnitude(a: &[u32], shift: u64) -> Vec<u32> {
        let limbs = shift / 32;
        if limbs >= a.len() as u64 {
            return Vec::new();
        }
        let bits = (shift % 32) as u32;
        let a = &a[limbs as usize..];
        let mut out = Vec::with_capacity(a.len());
        for (i, &limb) in a.iter().enumerate() {
            let next = u64::from(a.get(i + 1).copied().unwrap_or(0));
            let wide = (next << 32) | u64::from(limb);
            out.push((wide >> bits) as u32);
        }
        out
    }

    /// Quotient and remainder of magnitudes, `b` nonzero (Knuth's
    /// algorithm D).
    fn div_rem_magnitude(a: &[u32], b: &[u32]) -> (Vec<u32>, Vec<u32>) {
        if Self::cmp_magnitude(a, b) == Ordering::Less {
            return (Vec::new(), a.to_vec());
        }
        if b.len() == 1 {
            let d = u64::from(b[0]);
            let mut q = vec![0u32; a.len()];
            let mut r = 0u64;
            for i in (0..a.len()).rev() {
                let v = (r << 32) | u64::from(a[i]);
                q[i] = (v / d) as u32;
                r = v % d;
            }
            return (q, vec![r as u32]);
        }
        let s = u64::from(b[b.len() - 1].leading_zeros());
        let v = Self::shl_magnitude(b, s);
        let mut u = Self::shl_magnitude(a, s);
        u.resize(a.len() + 1, 0);
        let n = v.len();
        let m = u.len() - n - 1;
        let mut q = vec![0u32; m + 1];
        const BASE: u64 = 1 << 32;
        for j in (0..=m).rev() {
            let num = (u64::from(u[j + n]) << 32) | u64::from(u[j + n - 1]);
            let mut qhat = num / u64::from(v[n - 1]);
            let mut rhat = num % u64::from(v[n - 1]);
            while qhat >= BASE
                || qhat * u64::from(v[n - 2]) > ((rhat << 32) | u64::from(u[j + n - 2]))
            {
                qhat -= 1;
                rhat += u64::from(v[n - 1]);
                if rhat >= BASE {
                    break;
                }
            }
            let mut borrow = 0i64;
            let mut carry = 0u64;
            for i in 0..n {
                let p = qhat * u64::from(v[i]) + carry;
                carry = p >> 32;
                let t = i64::from(u[i + j]) - borrow - (p & 0xFFFF_FFFF) as i64;
                // Wrapping to a limb is the subtraction modulo 2³².
                u[i + j] = t as u32;
                borrow = i64::from(t < 0);
            }
            let t = i64::from(u[j + n]) - borrow - carry as i64;
            u[j + n] = t as u32;
            if t < 0 {
                qhat -= 1;
                let mut carry = 0u64;
                for i in 0..n {
                    let sum = u64::from(u[i + j]) + u64::from(v[i]) + carry;
                    u[i + j] = sum as u32;
                    carry = sum >> 32;
                }
                u[j + n] = (u64::from(u[j + n]) + carry) as u32;
            }
            q[j] = qhat as u32;
        }
        u.truncate(n);
        (q, Self::shr_magnitude(&u, s))
    }

    // ---- arithmetic (§6.1.6.2) ---------------------------------------

    /// `-x`.
    pub fn neg(&self) -> Self {
        Self::from_magnitude(self.magnitude.clone(), !self.negative)
    }

    /// `x + y`.
    pub fn add(&self, other: &Self) -> Result<Self, TooBig> {
        if self.negative == other.negative {
            return Self::from_magnitude(
                Self::add_magnitude(&self.magnitude, &other.magnitude),
                self.negative,
            )
            .checked();
        }
        Ok(
            match Self::cmp_magnitude(&self.magnitude, &other.magnitude) {
                Ordering::Equal => Self::zero(),
                Ordering::Greater => Self::from_magnitude(
                    Self::sub_magnitude(&self.magnitude, &other.magnitude),
                    self.negative,
                ),
                Ordering::Less => Self::from_magnitude(
                    Self::sub_magnitude(&other.magnitude, &self.magnitude),
                    other.negative,
                ),
            },
        )
    }

    /// `x - y`.
    pub fn sub(&self, other: &Self) -> Result<Self, TooBig> {
        self.add(&other.neg())
    }

    /// `x * y`.
    pub fn mul(&self, other: &Self) -> Result<Self, TooBig> {
        if self.is_zero() || other.is_zero() {
            return Ok(Self::zero());
        }
        if self.bit_length() + other.bit_length() > MAX_BITS + 1 {
            return Err(TooBig);
        }
        Self::from_magnitude(
            Self::mul_magnitude(&self.magnitude, &other.magnitude),
            self.negative != other.negative,
        )
        .checked()
    }

    /// `x / y` truncated toward zero and `x % y` with the sign of `x`;
    /// `None` when `y` is zero.
    pub fn div_rem(&self, other: &Self) -> Option<(Self, Self)> {
        if other.is_zero() {
            return None;
        }
        let (q, r) = Self::div_rem_magnitude(&self.magnitude, &other.magnitude);
        Some((
            Self::from_magnitude(q, self.negative != other.negative),
            Self::from_magnitude(r, self.negative),
        ))
    }

    /// `x ** y`; `None` for a negative exponent.
    pub fn pow(&self, exponent: &Self) -> Option<Result<Self, TooBig>> {
        if exponent.negative {
            return None;
        }
        if exponent.is_zero() {
            return Some(Ok(Self::one()));
        }
        let odd = exponent.magnitude[0] & 1 == 1;
        if self.is_zero() {
            return Some(Ok(Self::zero()));
        }
        if self.magnitude == [1] {
            return Some(Ok(Self::from_magnitude(vec![1], self.negative && odd)));
        }
        // Any other base at least doubles with each step.
        if exponent.bit_length() > 32 {
            return Some(Err(TooBig));
        }
        let mut e = exponent.magnitude[0];
        if (self.bit_length() - 1).saturating_mul(u64::from(e)) > MAX_BITS {
            return Some(Err(TooBig));
        }
        let mut base = self.clone();
        let mut result = Self::one();
        loop {
            if e & 1 == 1 {
                result = match result.mul(&base) {
                    Ok(r) => r,
                    Err(e) => return Some(Err(e)),
                };
            }
            e >>= 1;
            if e == 0 {
                break;
            }
            base = match base.mul(&base) {
                Ok(b) => b,
                Err(e) => return Some(Err(e)),
            };
        }
        Some(Ok(result))
    }

    /// `x << shift` for a non-negative shift count.
    fn shl_bits(&self, shift: u64) -> Result<Self, TooBig> {
        if self.is_zero() {
            return Ok(Self::zero());
        }
        if shift > MAX_BITS || self.bit_length() + shift > MAX_BITS {
            return Err(TooBig);
        }
        Ok(Self::from_magnitude(
            Self::shl_magnitude(&self.magnitude, shift),
            self.negative,
        ))
    }

    /// `x >> shift` for a non-negative shift count, rounding down.
    fn shr_bits(&self, shift: u64) -> Self {
        if !self.negative {
            return Self::from_magnitude(Self::shr_magnitude(&self.magnitude, shift), false);
        }
        // floor(-m / 2ˢ) = -((m - 1) >> s) - 1.
        let less = Self::sub_magnitude(&self.magnitude, &[1]);
        let shifted = Self::shr_magnitude(&less, shift);
        Self::from_magnitude(Self::add_magnitude(&shifted, &[1]), true)
    }

    fn shift_count(count: &Self) -> u64 {
        if count.magnitude.len() > 2 {
            u64::MAX
        } else {
            count.to_u64_wrapping_magnitude()
        }
    }

    fn to_u64_wrapping_magnitude(&self) -> u64 {
        let low = u64::from(self.magnitude.first().copied().unwrap_or(0));
        let high = u64::from(self.magnitude.get(1).copied().unwrap_or(0));
        low | (high << 32)
    }

    /// `x << y`; a negative `y` shifts right.
    pub fn shl(&self, count: &Self) -> Result<Self, TooBig> {
        let n = Self::shift_count(count);
        if count.negative {
            Ok(self.shr_bits(n))
        } else {
            self.shl_bits(n)
        }
    }

    /// `x >> y`; a negative `y` shifts left.
    pub fn shr(&self, count: &Self) -> Result<Self, TooBig> {
        let n = Self::shift_count(count);
        if count.negative {
            self.shl_bits(n)
        } else {
            Ok(self.shr_bits(n))
        }
    }

    /// The low `len` limbs of the infinite two's complement form.
    fn to_twos_complement(&self, len: usize) -> Vec<u32> {
        let mut limbs: Vec<u32> = self.magnitude.iter().copied().take(len).collect();
        limbs.resize(len, 0);
        if self.negative {
            let mut carry = 1u64;
            for limb in &mut limbs {
                let v = u64::from(!*limb) + carry;
                *limb = v as u32;
                carry = v >> 32;
            }
        }
        limbs
    }

    /// The value whose two's complement form is `limbs`, sign-extended
    /// with ones when `negative`.
    fn from_twos_complement(mut limbs: Vec<u32>, negative: bool) -> Self {
        if negative {
            let mut carry = 1u64;
            for limb in &mut limbs {
                let v = u64::from(!*limb) + carry;
                *limb = v as u32;
                carry = v >> 32;
            }
        }
        Self::from_magnitude(limbs, negative)
    }

    fn bitwise(&self, other: &Self, op: fn(u32, u32) -> u32, sign: fn(bool, bool) -> bool) -> Self {
        let len = self.magnitude.len().max(other.magnitude.len()) + 1;
        let a = self.to_twos_complement(len);
        let b = other.to_twos_complement(len);
        let limbs = a.iter().zip(&b).map(|(&x, &y)| op(x, y)).collect();
        Self::from_twos_complement(limbs, sign(self.negative, other.negative))
    }

    /// `x & y`.
    pub fn bit_and(&self, other: &Self) -> Self {
        self.bitwise(other, |x, y| x & y, |a, b| a && b)
    }

    /// `x | y`.
    pub fn bit_or(&self, other: &Self) -> Self {
        self.bitwise(other, |x, y| x | y, |a, b| a || b)
    }

    /// `x ^ y`.
    pub fn bit_xor(&self, other: &Self) -> Self {
        self.bitwise(other, |x, y| x ^ y, |a, b| a != b)
    }

    /// `~x`, which is `-x - 1`.
    pub fn bit_not(&self) -> Self {
        if self.negative {
            Self::from_magnitude(Self::sub_magnitude(&self.magnitude, &[1]), false)
        } else {
            Self::from_magnitude(Self::add_magnitude(&self.magnitude, &[1]), true)
        }
    }

    /// `BigInt.asUintN(bits, x)`: `x` modulo 2ᵇⁱᵗˢ.
    pub fn as_uint_n(&self, bits: u64) -> Result<Self, TooBig> {
        if !self.negative && self.bit_length() <= bits {
            return Ok(self.clone());
        }
        if bits > MAX_BITS {
            return Err(TooBig);
        }
        // `bits` is at most `MAX_BITS`, so the limb count fits in `usize`.
        let len = (bits / 32) as usize + 1;
        let mut limbs = self.to_twos_complement(len);
        let keep = (bits % 32) as u32;
        limbs[len - 1] &= (1u32 << keep) - 1;
        Ok(Self::from_magnitude(limbs, false))
    }

    /// `BigInt.asIntN(bits, x)`: `x` wrapped to a signed integer of
    /// `bits` bits.
    pub fn as_int_n(&self, bits: u64) -> Result<Self, TooBig> {
        if bits == 0 {
            return Ok(Self::zero());
        }
        // Already in range: |x| < 2ᵇⁱᵗˢ⁻¹, or x = -2ᵇⁱᵗˢ⁻¹.
        let length = self.bit_length();
        let power = self.magnitude.iter().map(|l| l.count_ones()).sum::<u32>() == 1;
        if length < bits || (self.negative && power && length == bits) {
            return Ok(self.clone());
        }
        let wrapped = self.as_uint_n(bits)?;
        if wrapped.bit_length() == bits {
            // The sign bit is set: subtract 2ᵇⁱᵗˢ.
            let modulus = Self::one().shl_bits(bits)?;
            wrapped.sub(&modulus)
        } else {
            Ok(wrapped)
        }
    }

    /// Digits in `radix` (2 to 36) with a `-` sign when negative.
    pub fn to_string_radix(&self, radix: u32) -> String {
        if radix == 10 {
            return self.to_string_radix10();
        }
        if self.is_zero() {
            return "0".into();
        }
        // The largest power of the radix in one limb, and its digit count.
        let mut chunk = u64::from(radix);
        let mut width = 1usize;
        while chunk * u64::from(radix) <= u64::from(u32::MAX) {
            chunk *= u64::from(radix);
            width += 1;
        }
        let mut limbs = self.magnitude.clone();
        let mut chunks: Vec<u32> = Vec::new();
        while !limbs.is_empty() {
            let mut remainder = 0u64;
            for limb in limbs.iter_mut().rev() {
                let v = (remainder << 32) | u64::from(*limb);
                // The remainder is below `chunk`, so the quotient fits.
                *limb = (v / chunk) as u32;
                remainder = v % chunk;
            }
            chunks.push(remainder as u32);
            while limbs.last() == Some(&0) {
                limbs.pop();
            }
        }
        let digit = |d: u32| char::from_digit(d, radix).expect("a digit below the radix");
        let mut out = String::new();
        if self.negative {
            out.push('-');
        }
        for (i, &value) in chunks.iter().rev().enumerate() {
            let mut digits = Vec::with_capacity(width);
            let mut v = value;
            while v != 0 {
                digits.push(digit(v % radix));
                v /= radix;
            }
            if i != 0 {
                digits.resize(width, '0');
            }
            out.extend(digits.iter().rev());
        }
        out
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

    fn big(text: &str) -> BigInt {
        match text.strip_prefix('-') {
            Some(digits) => BigInt::from_digits(digits, 10, true).unwrap(),
            None => BigInt::from_digits(text, 10, false).unwrap(),
        }
    }

    #[test]
    fn arithmetic_matches_reference_values() {
        let a = big("123456789012345678901234567890123456789");
        let b = big("-987654321098765432109876543210");
        let text = |x: BigInt| x.to_string_radix10();
        assert_eq!(
            text(a.add(&b).unwrap()),
            "123456788024691357802469135780246913579"
        );
        assert_eq!(
            text(a.sub(&b).unwrap()),
            "123456789999999999999999999999999999999"
        );
        assert_eq!(
            text(a.mul(&b).unwrap()),
            "-121932631137021795226185032733744855963362292333223746380111126352690"
        );
        let (q, r) = a.div_rem(&b).unwrap();
        assert_eq!(text(q.clone()), "-124999998");
        // a = q·b + r with r taking the dividend's sign.
        assert_eq!(q.mul(&b).unwrap().add(&r).unwrap(), a);
        assert!(!r.is_negative());
        let (q, r) = b.div_rem(&big("7")).unwrap();
        assert_eq!(q.mul(&big("7")).unwrap().add(&r).unwrap(), b);
        assert!(r.is_negative() || r.is_zero());
        assert!(a.div_rem(&BigInt::zero()).is_none());
        assert_eq!(
            text(big("2").pow(&big("100")).unwrap().unwrap()),
            "1267650600228229401496703205376"
        );
        assert_eq!(text(big("-3").pow(&big("3")).unwrap().unwrap()), "-27");
        assert!(big("2").pow(&big("-1")).is_none());
        assert_eq!(big("2").pow(&big("99999999999")), Some(Err(TooBig)));
        assert_eq!(
            text(big("-1").pow(&big("99999999999")).unwrap().unwrap()),
            "-1"
        );
    }

    #[test]
    fn long_division_agrees_with_multiplication() {
        // A deterministic sweep over operand sizes around limb boundaries,
        // including the add-back case of algorithm D.
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..400 {
            let limbs = |n: usize, next: &mut dyn FnMut() -> u64| {
                let mut v: Vec<u32> = (0..n).map(|_| next() as u32).collect();
                if round % 3 == 0 {
                    for limb in &mut v {
                        *limb |= 0xFFFF_0000;
                    }
                }
                BigInt::from_magnitude(v, false)
            };
            let a = limbs(1 + round % 9, &mut next);
            let b = limbs(1 + (round / 9) % 5, &mut next);
            if b.is_zero() {
                continue;
            }
            let (q, r) = a.div_rem(&b).unwrap();
            assert!(r < b, "remainder below divisor");
            assert_eq!(q.mul(&b).unwrap().add(&r).unwrap(), a);
        }
        let a = BigInt::from_magnitude(vec![0, 0, 0x8000_0000, 0x7FFF_FFFF], false);
        let b = BigInt::from_magnitude(vec![1, 0, 0x8000_0000], false);
        let (q, r) = a.div_rem(&b).unwrap();
        assert_eq!(q.mul(&b).unwrap().add(&r).unwrap(), a);
    }

    #[test]
    fn bitwise_operators_use_twos_complement() {
        let text = |x: BigInt| x.to_string_radix10();
        assert_eq!(text(big("-1").bit_and(&big("255"))), "255");
        assert_eq!(text(big("-256").bit_or(&big("255"))), "-1");
        assert_eq!(text(big("-1").bit_xor(&big("1"))), "-2");
        assert_eq!(text(big("5").bit_not()), "-6");
        assert_eq!(text(big("-6").bit_not()), "5");
        assert_eq!(
            text(big("-18446744073709551616").bit_and(&big("-4294967296"))),
            "-18446744073709551616"
        );
        assert_eq!(text(big("-5").shr(&big("1")).unwrap()), "-3");
        assert_eq!(text(big("-5").shr(&big("100")).unwrap()), "-1");
        assert_eq!(
            text(big("5").shr(&big("100000000000000000000")).unwrap()),
            "0"
        );
        assert_eq!(text(big("5").shl(&big("-1")).unwrap()), "2");
        assert_eq!(
            text(big("1").shl(&big("70")).unwrap()),
            "1180591620717411303424"
        );
        assert_eq!(big("1").shl(&big("100000000000000000000")), Err(TooBig));
        assert_eq!(
            text(BigInt::zero().shl(&big("100000000000000000000")).unwrap()),
            "0"
        );
    }

    #[test]
    fn wrapping_and_radix_conversions() {
        let text = |x: BigInt| x.to_string_radix10();
        assert_eq!(text(big("-1").as_uint_n(8).unwrap()), "255");
        assert_eq!(text(big("255").as_int_n(8).unwrap()), "-1");
        assert_eq!(text(big("128").as_int_n(8).unwrap()), "-128");
        assert_eq!(text(big("-128").as_int_n(8).unwrap()), "-128");
        assert_eq!(text(big("-129").as_int_n(8).unwrap()), "127");
        assert_eq!(text(big("127").as_int_n(8).unwrap()), "127");
        assert_eq!(text(big("5").as_int_n(0).unwrap()), "0");
        assert_eq!(text(big("5").as_uint_n(0).unwrap()), "0");
        assert_eq!(
            text(big("-7").as_int_n(9_007_199_254_740_991).unwrap()),
            "-7"
        );
        assert_eq!(big("-7").as_uint_n(9_007_199_254_740_991), Err(TooBig));
        assert_eq!(
            text(big("18446744073709551615").as_int_n(64).unwrap()),
            "-1"
        );
        assert_eq!(big("-1").to_u64_wrapping(), u64::MAX);
        assert_eq!(big("18446744073709551617").to_i64_wrapping(), 1);
        assert_eq!(
            BigInt::from_i64(i64::MIN).to_string_radix10(),
            "-9223372036854775808"
        );
        assert_eq!(big("255").to_string_radix(16), "ff");
        assert_eq!(big("-255").to_string_radix(2), "-11111111");
        assert_eq!(
            big("123456789012345678901234567890").to_string_radix(36),
            "byw97um9s91dlz68tsi"
        );
        assert_eq!(big("4294967296").to_string_radix(16), "100000000");
        assert_eq!(big("68719476736").to_string_radix(32), "20000000");
    }
}
