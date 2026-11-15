//! CLDR plural rules (LDML part 3 §5): the operands of a formatted
//! number and the evaluation of a locale's rules on them.

use super::number_data::{OTHER, data};

/// The plural operands of a number as it is displayed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Operands {
    /// `i`: the integer digits. Values past 10^18 keep their low digits,
    /// which is all a rule can tell apart (rules compare remainders of
    /// powers of ten and small constants).
    integer: u64,
    /// `v`: how many fraction digits are shown.
    visible: u64,
    /// `w`: the same without trailing zeros.
    significant: u64,
    /// `f`: the fraction digits as an integer.
    fraction: u64,
    /// `t`: the same without trailing zeros.
    trimmed: u64,
    /// `c` and `e`: the exponent of a compact number.
    exponent: u64,
}

const WRAP: u64 = 1_000_000_000_000_000_000;

fn low_digits(digits: &str) -> u64 {
    let mut value = 0u64;
    let mut wrapped = false;
    for byte in digits.bytes() {
        value = value * 10 + u64::from(byte - b'0');
        if value >= WRAP {
            value %= WRAP;
            wrapped = true;
        }
    }
    if wrapped { value + WRAP } else { value }
}

impl Operands {
    /// The operands of the ASCII digit strings `integer` and `fraction`
    /// scaled by `10^exponent` (a compact number such as 1.2M has the
    /// operands of 1200000 and the exponent 6).
    pub(super) fn new(integer: &str, fraction: &str, exponent: u32) -> Self {
        let moved = (exponent as usize).min(fraction.len());
        let mut whole = String::with_capacity(integer.len() + exponent as usize);
        whole.push_str(integer);
        whole.push_str(&fraction[..moved]);
        whole.extend(std::iter::repeat_n('0', exponent as usize - moved));
        let fraction = &fraction[moved..];
        let trimmed = fraction.trim_end_matches('0');
        Self {
            integer: low_digits(&whole),
            visible: fraction.len() as u64,
            significant: trimmed.len() as u64,
            fraction: low_digits(fraction),
            trimmed: low_digits(trimmed),
            exponent: u64::from(exponent),
        }
    }

    /// The value of an operand; `None` for `n` when it is not an
    /// integer, which no relation with integers holds for.
    fn value(&self, operand: u8) -> Option<u64> {
        match operand {
            b'n' => (self.trimmed == 0).then_some(self.integer),
            b'i' => Some(self.integer),
            b'v' => Some(self.visible),
            b'w' => Some(self.significant),
            b'f' => Some(self.fraction),
            b't' => Some(self.trimmed),
            _ => Some(self.exponent),
        }
    }
}

/// A cursor over a rule's text.
struct Rule<'a> {
    text: &'a [u8],
    at: usize,
}

impl Rule<'_> {
    fn skip_spaces(&mut self) {
        while self.text.get(self.at) == Some(&b' ') {
            self.at += 1;
        }
    }

    fn eat(&mut self, word: &[u8]) -> bool {
        self.skip_spaces();
        if self.text[self.at..].starts_with(word) {
            self.at += word.len();
            true
        } else {
            false
        }
    }

    fn number(&mut self) -> u64 {
        self.skip_spaces();
        let mut value = 0u64;
        while let Some(digit) = self.text.get(self.at).filter(|byte| byte.is_ascii_digit()) {
            value = value * 10 + u64::from(digit - b'0');
            self.at += 1;
        }
        value
    }

    /// `relation = operand ('%' value)? ('=' | '!=') range (',' range)*`
    fn relation(&mut self, operands: &Operands) -> bool {
        self.skip_spaces();
        let operand = self.text.get(self.at).copied().unwrap_or(b'n');
        self.at += 1;
        let mut value = operands.value(operand);
        if self.eat(b"%") {
            let modulus = self.number().max(1);
            value = value.map(|value| value % modulus);
        }
        let negated = self.eat(b"!=");
        if !negated {
            self.eat(b"=");
        }
        let mut within = false;
        loop {
            let low = self.number();
            let high = if self.eat(b"..") { self.number() } else { low };
            within |= value.is_some_and(|value| (low..=high).contains(&value));
            if !self.eat(b",") {
                break;
            }
        }
        within != negated
    }

    /// `condition = and_condition ('or' and_condition)*`
    fn condition(&mut self, operands: &Operands) -> bool {
        let mut any = false;
        loop {
            let mut all = true;
            loop {
                all &= self.relation(operands);
                if !self.eat(b"and") {
                    break;
                }
            }
            any |= all;
            if !self.eat(b"or") {
                return any;
            }
        }
    }
}

/// The plural category of `operands` in `locale`: `kind` is 0 for
/// cardinal and 1 for ordinal rules. The result indexes [`CATEGORIES`].
pub(super) fn category(locale: &str, kind: usize, operands: &Operands) -> usize {
    let data = data();
    let present = data.plural_categories(kind, locale);
    for category in 0..OTHER {
        if present & 1 << category == 0 {
            continue;
        }
        let text = data.plural_rule(kind, locale, category);
        if !text.is_empty()
            && (Rule {
                text: text.as_bytes(),
                at: 0,
            })
            .condition(operands)
        {
            return category;
        }
    }
    OTHER
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATEGORIES: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];

    fn select(locale: &str, kind: usize, integer: &str, fraction: &str) -> &'static str {
        CATEGORIES[category(locale, kind, &Operands::new(integer, fraction, 0))]
    }

    #[test]
    fn rules_follow_the_displayed_digits() {
        assert_eq!(select("en", 0, "1", ""), "one");
        assert_eq!(select("en", 0, "1", "0"), "other");
        assert_eq!(select("en", 0, "2", ""), "other");
        assert_eq!(select("en-US", 1, "22", ""), "two");
        assert_eq!(select("en", 1, "13", ""), "other");
        assert_eq!(select("ar", 0, "0", ""), "zero");
        assert_eq!(select("ar", 0, "103", ""), "few");
        assert_eq!(select("ar", 0, "111", ""), "many");
        assert_eq!(select("ru", 0, "21", ""), "one");
        assert_eq!(select("ru", 0, "1", "5"), "other");
        assert_eq!(select("fr", 0, "1", "5"), "one");
        assert_eq!(select("ja", 0, "1", ""), "other");
        assert_eq!(select("zz", 0, "1", ""), "other");
    }

    #[test]
    fn compact_exponents_and_long_numbers_keep_their_operands() {
        let million = Operands::new("1", "", 6);
        assert_eq!(CATEGORIES[category("fr", 0, &million)], "many");
        let shifted = Operands::new("1", "25", 1);
        assert_eq!(
            (shifted.integer, shifted.visible, shifted.fraction),
            (12, 1, 5)
        );
        let long = Operands::new("123456789012345678901234567890", "", 0);
        assert_eq!(long.integer % 100, 90);
        assert_eq!(CATEGORIES[category("ru", 0, &long)], "many");
    }
}
