//! The number formatting engine behind Intl.NumberFormat and
//! Intl.PluralRules (ECMA-402 §16.5): `PartitionNumberPattern` and
//! `PartitionNumberRangePattern` over CLDR patterns.
//!
//! A formatted number is built in layers, innermost first: the digits
//! (with an exponent in scientific notation), the compact affix
//! ("K", " thousand"), the affixes of the style's pattern (sign, percent
//! sign, currency symbol), and the unit or currency name around them.
//! Range formatting compares the layers of both ends to decide what
//! the two numbers share.

use super::decimal::{Decimal, DigitOptions, Formatted, Numeric};
use super::number_data::{OTHER, Symbol, currency, data, pattern};
use super::plural::{self, Operands};

/// Which end of a range a part belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    Shared,
    Start,
    End,
}

impl Source {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Start => "startRange",
            Self::End => "endRange",
        }
    }
}

/// One part of a formatted number (`formatToParts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Part {
    pub(super) kind: &'static str,
    pub(super) text: String,
    pub(super) source: Source,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Style {
    Decimal,
    Percent,
    Currency,
    Unit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Notation {
    Standard,
    Scientific,
    Engineering,
    Compact,
}

pub(super) const STYLES: &[&str] = &["decimal", "percent", "currency", "unit"];
pub(super) const NOTATIONS: &[&str] = &["standard", "scientific", "engineering", "compact"];
pub(super) const CURRENCY_DISPLAYS: &[&str] = &["code", "symbol", "narrowSymbol", "name"];
pub(super) const UNIT_DISPLAYS: &[&str] = &["short", "narrow", "long"];
pub(super) const SIGN_DISPLAYS: &[&str] = &["auto", "never", "always", "exceptZero", "negative"];
/// `useGrouping` values in the order of [`Format::grouping`].
pub(super) const GROUPINGS: &[&str] = &["false", "min2", "auto", "always"];

/// The resolved settings of a number format.
#[derive(Clone, Debug)]
pub(super) struct Format {
    /// The data locale's row in the number pack.
    pub(super) row: u32,
    /// The data locale's name, for its plural rules.
    pub(super) language: &'static str,
    /// The numbering system's number.
    pub(super) system: u32,
    pub(super) style: Style,
    /// The upper-case currency code; empty unless the style is currency.
    pub(super) currency: String,
    /// An index into [`CURRENCY_DISPLAYS`].
    pub(super) currency_display: usize,
    pub(super) accounting: bool,
    /// The unit identifier; empty unless the style is unit.
    pub(super) unit: String,
    /// An index into [`UNIT_DISPLAYS`].
    pub(super) unit_display: usize,
    pub(super) notation: Notation,
    pub(super) compact_long: bool,
    pub(super) digits: DigitOptions,
    /// An index into [`GROUPINGS`].
    pub(super) grouping: usize,
    /// An index into [`SIGN_DISPLAYS`].
    pub(super) sign: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Literal(String),
    Minus,
    Plus,
    Approximately,
    Percent,
    PerMille,
    Currency,
}

#[derive(Clone, Debug, Default)]
struct Affixes {
    prefix: Vec<Token>,
    suffix: Vec<Token>,
}

impl Affixes {
    fn has(&self, token: &Token) -> bool {
        self.prefix.contains(token) || self.suffix.contains(token)
    }
}

/// A CLDR number pattern (LDML part 3 §3), cut into its affixes.
#[derive(Clone, Debug, Default)]
struct Pattern {
    positive: Affixes,
    negative: Option<Affixes>,
    /// The size of the group next to the decimal point, 0 for none.
    primary: usize,
    /// The size of every further group.
    secondary: usize,
    /// How many `0` the number part has: a compact pattern's digits.
    zeros: usize,
}

fn push_literal(tokens: &mut Vec<Token>, c: char) {
    if let Some(Token::Literal(text)) = tokens.last_mut() {
        text.push(c);
    } else {
        tokens.push(Token::Literal(c.to_string()));
    }
}

impl Pattern {
    fn parse(text: &str) -> Self {
        let mut result = Self::default();
        let mut affixes = Affixes::default();
        // 0: in the prefix, 1: in the number, 2: in the suffix.
        let mut state = 0;
        let mut number = String::new();
        let mut first = true;
        let mut chars = text.chars().peekable();
        let mut quoted = false;
        while let Some(c) = chars.next() {
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                } else {
                    quoted = !quoted;
                    continue;
                }
            } else if !quoted {
                if c == ';' {
                    result.positive = std::mem::take(&mut affixes);
                    first = false;
                    state = 0;
                    continue;
                }
                let numeric = matches!(c, '#' | '0' | ',' | '.' | '@')
                    || (state == 1 && matches!(c, 'E' | '+'));
                if numeric && state < 2 {
                    state = 1;
                    if first {
                        number.push(c);
                    }
                    continue;
                }
                if state == 1 {
                    state = 2;
                }
                let token = match c {
                    '-' => Some(Token::Minus),
                    '+' => Some(Token::Plus),
                    '%' => Some(Token::Percent),
                    '‰' => Some(Token::PerMille),
                    '¤' => {
                        while chars.peek() == Some(&'¤') {
                            chars.next();
                        }
                        Some(Token::Currency)
                    }
                    _ => None,
                };
                if let Some(token) = token {
                    let side = if state == 0 {
                        &mut affixes.prefix
                    } else {
                        &mut affixes.suffix
                    };
                    side.push(token);
                    continue;
                }
            }
            if state == 1 {
                state = 2;
            }
            let side = if state == 0 {
                &mut affixes.prefix
            } else {
                &mut affixes.suffix
            };
            push_literal(side, c);
        }
        if first {
            result.positive = affixes;
        } else {
            result.negative = Some(affixes);
        }
        let integer = number.split(['.', 'E']).next().unwrap_or("");
        let mut groups = integer.rsplit(',');
        let last = groups.next().map_or(0, str::len);
        if let Some(previous) = groups.next() {
            result.primary = last;
            result.secondary = if groups.next().is_some() {
                previous.len()
            } else {
                last
            };
        }
        result.zeros = number.bytes().filter(|&byte| byte == b'0').count();
        result
    }
}

/// A formatted number in layers; each affix layer has the parts before
/// and after what it wraps.
#[derive(Clone, Debug, Default)]
struct Layers {
    number: Vec<Part>,
    inner: [Vec<Part>; 2],
    middle: [Vec<Part>; 2],
    outer: [Vec<Part>; 2],
}

impl Layers {
    fn flatten(self) -> Vec<Part> {
        let [outer_before, outer_after] = self.outer;
        let [middle_before, middle_after] = self.middle;
        let [inner_before, inner_after] = self.inner;
        let mut parts = Vec::new();
        for piece in [
            outer_before,
            middle_before,
            inner_before,
            self.number,
            inner_after,
            middle_after,
            outer_after,
        ] {
            for part in piece {
                push(&mut parts, part.kind, &part.text);
            }
        }
        parts
    }
}

/// Append a part, joining neighboring literals.
fn push(parts: &mut Vec<Part>, kind: &'static str, text: &str) {
    if text.is_empty() {
        return;
    }
    if kind == "literal" {
        if let Some(last) = parts.last_mut().filter(|last| last.kind == "literal") {
            last.text.push_str(text);
            return;
        }
    }
    parts.push(Part {
        kind,
        text: text.to_owned(),
        source: Source::Shared,
    });
}

/// Append `text` as a part of `kind`, with the white space at its ends
/// as literals.
fn push_spaced(parts: &mut Vec<Part>, kind: &'static str, text: &str) {
    let body = text.trim_start();
    push(parts, "literal", &text[..text.len() - body.len()]);
    let trimmed = body.trim_end();
    push(parts, kind, trimmed);
    push(parts, "literal", &body[trimmed.len()..]);
}

pub(super) fn text_of(parts: &[Part]) -> String {
    parts.iter().map(|part| part.text.as_str()).collect()
}

/// Whether a currency sign's edge is a letter or punctuation, which
/// CLDR's currency spacing separates from a digit (`[[:^S:]&[:^Z:]]`).
fn wants_space(c: char) -> bool {
    if c.is_alphanumeric() {
        return true;
    }
    c.is_ascii_punctuation() && !"$+<=>^`|~".contains(c)
}

/// What the rounding of a number produced, before any affix.
struct Core {
    parts: Vec<Part>,
    /// -1 for a minus sign, 1 for a plus sign, 0 for none.
    sign: i8,
    /// The plural category of the number as displayed.
    category: usize,
    /// For compact notation with an exponent: the index of the pattern
    /// (the number of zeros of the magnitude).
    zeros: Option<u32>,
}

impl Format {
    fn sign_of(&self, negative: bool, zero: bool) -> i8 {
        match self.sign {
            0 => -i8::from(negative),
            1 => 0,
            2 => {
                if negative {
                    -1
                } else {
                    1
                }
            }
            3 if zero => 0,
            3 => {
                if negative {
                    -1
                } else {
                    1
                }
            }
            _ => -i8::from(negative && !zero),
        }
    }

    fn symbol(&self, symbol: Symbol) -> &'static str {
        data().symbol(self.row, self.system, symbol)
    }

    /// ASCII digits in the numbering system's digits.
    fn transliterate(&self, digits: &str) -> String {
        let system: Vec<char> = data().digits(self.system).chars().collect();
        if system.len() != 10 {
            return digits.to_owned();
        }
        digits
            .bytes()
            .map(|byte| system[usize::from(byte - b'0')])
            .collect()
    }

    fn compact_kind(&self) -> u32 {
        if self.style == Style::Currency && self.currency_display != 3 {
            2
        } else {
            u32::from(self.compact_long)
        }
    }

    /// `ComputeExponentForMagnitude` (§16.5.14), and for compact
    /// notation also the pattern index.
    fn exponent_for(&self, magnitude: i32) -> (i32, Option<u32>) {
        match self.notation {
            Notation::Standard => (0, None),
            Notation::Scientific => (magnitude, None),
            Notation::Engineering => (magnitude.div_euclid(3) * 3, None),
            Notation::Compact => {
                if magnitude < 3 {
                    return (0, None);
                }
                // CLDR's patterns end at 10^14; larger numbers keep its unit.
                let zeros = magnitude.min(14) as u32;
                let found =
                    data().compact(self.row, self.system, self.compact_kind(), zeros, OTHER);
                match found.map(Pattern::parse) {
                    Some(pattern) if found != Some("0") && pattern.zeros > 0 => {
                        (zeros as i32 - (pattern.zeros as i32 - 1), Some(zeros))
                    }
                    _ => (0, None),
                }
            }
        }
    }

    /// `ComputeExponent` (§16.5.13) and the rounding of the scaled value.
    fn scale(&self, value: &Decimal) -> (Formatted, i32, Option<u32>) {
        if self.notation == Notation::Standard || value.is_zero() {
            return (self.digits.format(value), 0, None);
        }
        let magnitude = value.magnitude();
        let (exponent, zeros) = self.exponent_for(magnitude);
        let mut scaled = value.clone();
        scaled.shift(-exponent);
        let formatted = self.digits.format(&scaled);
        if formatted.rounded.is_zero() || formatted.rounded.magnitude() == magnitude - exponent {
            return (formatted, exponent, zeros);
        }
        let (exponent, zeros) = self.exponent_for(magnitude + 1);
        let mut scaled = value.clone();
        scaled.shift(-exponent);
        (self.digits.format(&scaled), exponent, zeros)
    }

    /// The digits of a rounded number, grouped as `pattern` asks.
    fn digit_parts(&self, formatted: &Formatted, pattern: &Pattern, parts: &mut Vec<Part>) {
        let money = self.style == Style::Currency;
        let pick = |special: Symbol, general: Symbol| {
            let text = if money { self.symbol(special) } else { "" };
            if text.is_empty() {
                self.symbol(general)
            } else {
                text
            }
        };
        let integer = &formatted.integer;
        let (mut primary, mut secondary) = (pattern.primary, pattern.secondary);
        if primary == 0 && self.grouping == 3 {
            (primary, secondary) = (3, 3);
        }
        let minimum = match self.grouping {
            1 => data().minimum_grouping_digits(self.row).max(2) as usize,
            2 => data().minimum_grouping_digits(self.row) as usize,
            _ => 1,
        };
        let grouped = self.grouping != 0
            && primary != 0
            && integer.len() > primary
            && integer.len() - primary >= minimum;
        if grouped {
            let secondary = secondary.max(1);
            let head = integer.len() - primary;
            let first = match head % secondary {
                0 => secondary,
                rest => rest,
            };
            let separator = pick(Symbol::CurrencyGroup, Symbol::Group);
            let mut at = 0;
            let mut end = first;
            while at < head {
                push(parts, "integer", &self.transliterate(&integer[at..end]));
                push(parts, "group", separator);
                at = end;
                end += secondary;
            }
            push(parts, "integer", &self.transliterate(&integer[head..]));
        } else {
            push(parts, "integer", &self.transliterate(integer));
        }
        if !formatted.fraction.is_empty() {
            push(
                parts,
                "decimal",
                pick(Symbol::CurrencyDecimal, Symbol::Decimal),
            );
            push(parts, "fraction", &self.transliterate(&formatted.fraction));
        }
    }

    fn core(&self, x: &Numeric, pattern: &Pattern) -> Core {
        let mut parts = Vec::new();
        let value = match x {
            Numeric::NaN => {
                push(&mut parts, "nan", self.symbol(Symbol::Nan));
                return Core {
                    parts,
                    sign: i8::from(self.sign == 2),
                    category: OTHER,
                    zeros: None,
                };
            }
            Numeric::Infinity { negative } => {
                push(&mut parts, "infinity", self.symbol(Symbol::Infinity));
                return Core {
                    parts,
                    sign: self.sign_of(*negative, false),
                    category: OTHER,
                    zeros: None,
                };
            }
            Numeric::Finite(value) => value,
        };
        let mut value = value.clone();
        if self.style == Style::Percent {
            value.shift(2);
        }
        let (formatted, exponent, zeros) = self.scale(&value);
        self.digit_parts(&formatted, pattern, &mut parts);
        if matches!(self.notation, Notation::Scientific | Notation::Engineering) {
            push(
                &mut parts,
                "exponentSeparator",
                self.symbol(Symbol::Exponential),
            );
            if exponent < 0 {
                push(
                    &mut parts,
                    "exponentMinusSign",
                    self.symbol(Symbol::MinusSign),
                );
            }
            let digits = exponent.unsigned_abs().to_string();
            push(&mut parts, "exponentInteger", &self.transliterate(&digits));
        }
        let operands = Operands::new(&formatted.integer, &formatted.fraction, 0);
        Core {
            parts,
            sign: self.sign_of(formatted.rounded.negative, formatted.rounded.is_zero()),
            category: plural::category(self.language, 0, &operands),
            zeros,
        }
    }

    fn currency_text(&self) -> &str {
        let data = data();
        let field = |field| data.currency(self.row, &self.currency, field);
        let found = match self.currency_display {
            1 => field(currency::SYMBOL),
            2 => field(currency::NARROW).or_else(|| field(currency::SYMBOL)),
            _ => None,
        };
        found.unwrap_or(&self.currency)
    }

    /// The pattern of the style: its affixes and its grouping.
    fn style_pattern(&self) -> Pattern {
        let data = data();
        let mut kind = match self.style {
            Style::Decimal | Style::Unit => pattern::DECIMAL,
            Style::Percent => pattern::PERCENT,
            Style::Currency if self.currency_display == 3 => pattern::DECIMAL,
            Style::Currency if self.accounting => pattern::ACCOUNTING,
            Style::Currency => pattern::CURRENCY,
        };
        if kind == pattern::CURRENCY || kind == pattern::ACCOUNTING {
            // A currency sign that meets the number with a letter takes
            // the pattern CLDR gives for that ("¤ #,##0.00").
            let text = self.currency_text();
            let plain = Pattern::parse(data.pattern(self.row, self.system, kind).unwrap_or(""));
            let lettered = if plain.positive.prefix.last() == Some(&Token::Currency) {
                text.chars().next_back().is_some_and(char::is_alphabetic)
            } else if plain.positive.suffix.first() == Some(&Token::Currency) {
                text.chars().next().is_some_and(char::is_alphabetic)
            } else {
                false
            };
            let spaced = kind + pattern::ALPHA_NEXT_TO_NUMBER;
            if lettered && data.pattern(self.row, self.system, spaced).is_some() {
                kind = spaced;
            }
        }
        let text = data
            .pattern(self.row, self.system, kind)
            .unwrap_or("#,##0.###");
        let mut parsed = Pattern::parse(text);
        if matches!(self.notation, Notation::Scientific | Notation::Engineering)
            && self.grouping != 3
        {
            parsed.primary = 0;
        }
        parsed
    }

    /// Turn affix tokens into parts. `compact` marks the literals of a
    /// compact pattern; `prefix` says the tokens come before the number.
    fn emit(&self, tokens: &[Token], signs: &[Token], compact: bool, prefix: bool) -> Vec<Part> {
        let mut parts = Vec::new();
        let count = tokens.len();
        for (index, token) in tokens.iter().enumerate() {
            match token {
                Token::Literal(text) if compact => push_spaced(&mut parts, "compact", text),
                Token::Literal(text) => push(&mut parts, "literal", text),
                Token::Minus => {
                    for sign in signs {
                        self.emit_sign(sign, &mut parts);
                    }
                }
                Token::Plus | Token::Approximately => self.emit_sign(token, &mut parts),
                Token::Percent => push(&mut parts, "percentSign", self.symbol(Symbol::PercentSign)),
                Token::PerMille => push(&mut parts, "literal", self.symbol(Symbol::PerMille)),
                Token::Currency => {
                    let text = self.currency_text();
                    // CLDR currency spacing: a letter of the currency
                    // sign does not touch a digit.
                    let touches = if prefix {
                        index + 1 == count
                    } else {
                        index == 0
                    };
                    let edge = if prefix {
                        text.chars().next_back()
                    } else {
                        text.chars().next()
                    };
                    let space = touches && edge.is_some_and(wants_space);
                    let between = |kind| {
                        data()
                            .pattern(self.row, self.system, kind)
                            .unwrap_or("\u{a0}")
                    };
                    if space && !prefix {
                        push(
                            &mut parts,
                            "literal",
                            between(pattern::SPACE_BEFORE_CURRENCY),
                        );
                    }
                    push(&mut parts, "currency", text);
                    if space && prefix {
                        push(
                            &mut parts,
                            "literal",
                            between(pattern::SPACE_AFTER_CURRENCY),
                        );
                    }
                }
            }
        }
        parts
    }

    fn emit_sign(&self, sign: &Token, parts: &mut Vec<Part>) {
        match sign {
            Token::Plus => push(parts, "plusSign", self.symbol(Symbol::PlusSign)),
            Token::Approximately => push(
                parts,
                "approximatelySign",
                self.symbol(Symbol::ApproximatelySign),
            ),
            _ => push(parts, "minusSign", self.symbol(Symbol::MinusSign)),
        }
    }

    /// The affixes of `pattern` for a number with `sign`, placing a plus
    /// or approximately sign where the pattern puts its minus sign.
    fn affixes(
        &self,
        pattern: &Pattern,
        sign: i8,
        approximately: bool,
        compact: bool,
    ) -> [Vec<Part>; 2] {
        let plus = sign == 1 && !pattern.positive.has(&Token::Plus);
        let negative = pattern.negative.as_ref().filter(|negative| {
            sign == -1 || (negative.has(&Token::Minus) && (plus || approximately))
        });
        let prepend = negative.is_none() && (sign == -1 || plus || approximately);
        let mut signs = Vec::new();
        if approximately {
            signs.push(Token::Approximately);
        }
        if plus {
            signs.push(Token::Plus);
        } else if sign == -1 || !approximately {
            signs.push(Token::Minus);
        }
        let chosen = negative.unwrap_or(&pattern.positive);
        let mut before = Vec::new();
        if prepend {
            for sign in &signs {
                self.emit_sign(sign, &mut before);
            }
        }
        before.extend(self.emit(&chosen.prefix, &signs, compact, true));
        let after = self.emit(&chosen.suffix, &signs, compact, false);
        [before, after]
    }

    /// The pattern that puts a unit around a number, with `{0}`.
    fn unit_pattern(&self, category: usize) -> String {
        let data = data();
        let width = [1, 2, 0][self.unit_display];
        let single = |name: &str, field: u32| {
            let unit = data.unit(name)?;
            data.unit_pattern(self.row, unit, width, field).or_else(|| {
                (field < OTHER as u32)
                    .then(|| data.unit_pattern(self.row, unit, width, OTHER as u32))
                    .flatten()
            })
        };
        if let Some(found) = single(&self.unit, category as u32) {
            return found.to_owned();
        }
        if let Some((numerator, denominator)) = self.unit.split_once("-per-") {
            let top = single(numerator, category as u32).unwrap_or("{0}");
            if let Some(per) = single(denominator, 6) {
                return per.replace("{0}", top);
            }
            let bottom = single(denominator, 1)
                .map(|text| text.replace("{0}", "").trim().to_owned())
                .unwrap_or_else(|| denominator.to_owned());
            return data
                .per_pattern(self.row, width)
                .replace("{0}", top)
                .replace("{1}", &bottom);
        }
        format!("{{0}} {}", self.unit)
    }

    /// The parts around a number for a unit or a currency's name.
    fn name_affixes(&self, category: usize) -> [Vec<Part>; 2] {
        let data = data();
        let (text, kind, name) = match self.style {
            Style::Unit => (self.unit_pattern(category), "unit", String::new()),
            Style::Currency if self.currency_display == 3 => {
                let field = |field| data.currency(self.row, &self.currency, field);
                let name = field(currency::PLURAL_NAME + category as u32)
                    .or_else(|| field(currency::PLURAL_NAME + OTHER as u32))
                    .or_else(|| field(currency::NAME))
                    .unwrap_or(&self.currency);
                let kind = pattern::CURRENCY_UNIT;
                let text = data
                    .pattern(self.row, self.system, kind + category as u32)
                    .or_else(|| data.pattern(self.row, self.system, kind + OTHER as u32))
                    .unwrap_or("{0} {1}");
                (text.to_owned(), "currency", name.to_owned())
            }
            _ => return [Vec::new(), Vec::new()],
        };
        let (before, after) = text.split_once("{0}").unwrap_or(("", &text));
        [before, after].map(|side| {
            let mut parts = Vec::new();
            if kind == "unit" {
                push_spaced(&mut parts, kind, side);
            } else if let Some((left, right)) = side.split_once("{1}") {
                push(&mut parts, "literal", left);
                push(&mut parts, kind, &name);
                push(&mut parts, "literal", right);
            } else {
                push(&mut parts, "literal", side);
            }
            parts
        })
    }

    fn layers(&self, x: &Numeric, approximately: bool) -> Layers {
        let style = self.style_pattern();
        let core = self.core(x, &style);
        let mut layers = Layers {
            number: core.parts,
            ..Layers::default()
        };
        let compact = core.zeros.and_then(|zeros| {
            data().compact(
                self.row,
                self.system,
                self.compact_kind(),
                zeros,
                core.category,
            )
        });
        let compact = compact.map(Pattern::parse);
        match compact {
            Some(compact) if self.compact_kind() == 2 => {
                // A compact currency pattern carries the currency sign.
                layers.middle = self.affixes(&compact, core.sign, approximately, true);
            }
            Some(compact) => {
                layers.inner = [
                    self.emit(&compact.positive.prefix, &[], true, true),
                    self.emit(&compact.positive.suffix, &[], true, false),
                ];
                layers.middle = self.affixes(&style, core.sign, approximately, false);
            }
            None => layers.middle = self.affixes(&style, core.sign, approximately, false),
        }
        layers.outer = self.name_affixes(core.category);
        layers
    }

    /// `PartitionNumberPattern` (§16.5.4).
    pub(super) fn format(&self, x: &Numeric) -> Vec<Part> {
        self.layers(x, false).flatten()
    }

    /// `PartitionNumberRangePattern` (§16.5.19) for two numbers that are
    /// not NaN. What the ends share follows ICU's default: a unit or a
    /// currency name is written once, and so is a currency or percent
    /// sign longer than one character.
    pub(super) fn format_range(&self, x: &Numeric, y: &Numeric) -> Vec<Part> {
        let start = self.layers(x, false);
        let end = self.layers(y, false);
        if text_of(&start.clone().flatten()) == text_of(&end.clone().flatten()) {
            return self.layers(x, true).flatten();
        }
        let same = |a: &[Vec<Part>; 2], b: &[Vec<Part>; 2]| a == b;
        let length = |a: &[Vec<Part>; 2]| -> usize {
            a.iter()
                .flatten()
                .map(|part| part.text.chars().count())
                .sum()
        };
        let share_outer = same(&start.outer, &end.outer);
        let share_middle =
            share_outer && same(&start.middle, &end.middle) && length(&start.middle) > 1;
        let spaced = length(&start.inner) > 0
            || (!share_middle && length(&start.middle) > 0)
            || (!share_outer && length(&start.outer) > 0)
            || matches!(self.notation, Notation::Scientific | Notation::Engineering);
        let range = data()
            .pattern(self.row, self.system, pattern::RANGE)
            .unwrap_or("{0}–{1}");
        let (before, rest) = range.split_once("{0}").unwrap_or(("", range));
        let (between, after) = rest.split_once("{1}").unwrap_or((rest, ""));
        let mut separator = between.to_owned();
        if spaced {
            if !separator.starts_with(char::is_whitespace) {
                separator.insert(0, ' ');
            }
            if !separator.ends_with(char::is_whitespace) {
                separator.push(' ');
            }
        }
        let mut parts: Vec<Part> = Vec::new();
        let mut add = |piece: Vec<Part>, source: Source| {
            for part in piece {
                let joins = part.kind == "literal"
                    && parts
                        .last()
                        .is_some_and(|last| last.kind == "literal" && last.source == source);
                if joins {
                    if let Some(last) = parts.last_mut() {
                        last.text.push_str(&part.text);
                    }
                } else if !part.text.is_empty() {
                    parts.push(Part { source, ..part });
                }
            }
        };
        let literal = |text: &str| {
            vec![Part {
                kind: "literal",
                text: text.to_owned(),
                source: Source::Shared,
            }]
        };
        let [start_outer, start_outer_after] = start.outer;
        let [start_middle, start_middle_after] = start.middle;
        let [start_inner, start_inner_after] = start.inner;
        let [end_outer, end_outer_after] = end.outer;
        let [end_middle, end_middle_after] = end.middle;
        let [end_inner, end_inner_after] = end.inner;
        add(literal(before), Source::Shared);
        if share_outer {
            add(start_outer, Source::Shared);
        } else {
            add(start_outer, Source::Start);
        }
        if share_middle {
            add(start_middle, Source::Shared);
        } else {
            add(start_middle, Source::Start);
        }
        add(start_inner, Source::Start);
        add(start.number, Source::Start);
        add(start_inner_after, Source::Start);
        if !share_middle {
            add(start_middle_after, Source::Start);
        }
        if !share_outer {
            add(start_outer_after, Source::Start);
        }
        add(literal(&separator), Source::Shared);
        if !share_outer {
            add(end_outer, Source::End);
        }
        if !share_middle {
            add(end_middle, Source::End);
        }
        add(end_inner, Source::End);
        add(end.number, Source::End);
        add(end_inner_after, Source::End);
        if share_middle {
            add(end_middle_after, Source::Shared);
        } else {
            add(end_middle_after, Source::End);
        }
        if share_outer {
            add(end_outer_after, Source::Shared);
        } else {
            add(end_outer_after, Source::End);
        }
        add(literal(after), Source::Shared);
        parts
    }
}
