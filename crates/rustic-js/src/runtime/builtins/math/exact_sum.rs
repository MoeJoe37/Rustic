//! Exact binary64 summation in integer multiples of the least subnormal, 2^-1074.

use std::cmp::Ordering;

pub(super) const MAX_TERMS: u64 = (1 << 53) - 1;

const FRACTION_BITS: usize = 52;
const FRACTION_MASK: u64 = (1 << FRACTION_BITS) - 1;
const SIGN_BIT: u64 = 1 << 63;
// The largest finite Number takes 2098 bits in units of 2^-1074. Even
// MAX_TERMS copies take at most 2151 bits, which fit in 34 words.
const WORDS: usize = 34;
const WORD_BITS: usize = u64::BITS as usize;
type Magnitude = [u64; WORDS];

pub(super) struct ExactSum {
    positive: Magnitude,
    negative: Magnitude,
}

impl ExactSum {
    pub(super) fn new() -> Self {
        Self {
            positive: [0; WORDS],
            negative: [0; WORDS],
        }
    }

    pub(super) fn add(&mut self, number: f64) {
        debug_assert!(number.is_finite());
        let bits = number.to_bits();
        let exponent = ((bits >> FRACTION_BITS) & 0x7ff) as usize;
        let mut significand = bits & FRACTION_MASK;
        if exponent != 0 {
            significand |= 1 << FRACTION_BITS;
        }
        let shift = exponent.saturating_sub(1);
        let magnitude = if bits & SIGN_BIT == 0 {
            &mut self.positive
        } else {
            &mut self.negative
        };
        let mut carry = u128::from(significand) << (shift % WORD_BITS);
        for word in &mut magnitude[shift / WORD_BITS..] {
            if carry == 0 {
                break;
            }
            carry += u128::from(*word);
            *word = carry as u64;
            carry >>= WORD_BITS;
        }
        debug_assert_eq!(carry, 0, "the term limit bounds the exact sum");
    }

    pub(super) fn finish(mut self) -> f64 {
        let (larger, smaller, sign) =
            match self.positive.iter().rev().cmp(self.negative.iter().rev()) {
                Ordering::Equal => return 0.0,
                Ordering::Greater => (&mut self.positive, &self.negative, 0),
                Ordering::Less => (&mut self.negative, &self.positive, SIGN_BIT),
            };
        let mut borrow = false;
        for (word, &subtrahend) in larger.iter_mut().zip(smaller) {
            let (difference, first) = word.overflowing_sub(subtrahend);
            let (difference, second) = difference.overflowing_sub(u64::from(borrow));
            *word = difference;
            borrow = first || second;
        }
        debug_assert!(!borrow);
        f64::from_bits(sign | rounded_bits(larger))
    }
}

fn rounded_bits(magnitude: &Magnitude) -> u64 {
    let Some(index) = magnitude.iter().rposition(|&word| word != 0) else {
        return 0;
    };
    let highest = index * WORD_BITS + (WORD_BITS - 1 - magnitude[index].leading_zeros() as usize);
    if highest < FRACTION_BITS {
        // Every subnormal is an exact multiple of the accumulator's unit.
        return magnitude[0];
    }
    let shift = highest - FRACTION_BITS;
    if shift >= 2046 {
        return f64::INFINITY.to_bits();
    }

    let word = shift / WORD_BITS;
    let adjacent = (u128::from(magnitude[word + 1]) << WORD_BITS) | u128::from(magnitude[word]);
    let mut significand = (adjacent >> (shift % WORD_BITS)) as u64;
    if let Some(halfway) = shift.checked_sub(1) {
        let word = halfway / WORD_BITS;
        let bit = 1 << (halfway % WORD_BITS);
        let halfway_bit = magnitude[word] & bit != 0;
        let below_halfway_bit =
            magnitude[word] & (bit - 1) != 0 || magnitude[..word].iter().any(|&word| word != 0);
        if halfway_bit && (below_halfway_bit || significand & 1 != 0) {
            significand += 1;
        }
    }

    // The leading bit of the significand supplies the +1 in the biased
    // exponent. A rounding carry increments it again, possibly to infinity.
    ((shift as u64) << FRACTION_BITS) + significand
}
