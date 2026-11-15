//! Gregorian date and clock patterns from pinned CLDR data.

use std::sync::OnceLock;

use super::pack::{Pack, Table};

static PACK: Pack = Pack::new(include_bytes!("../../../../data/intl/date-time.bin"));

pub(super) const RANGE: u32 = 12;
pub(super) const GMT: u32 = 13;
pub(super) const GMT_ZERO: u32 = 14;
pub(super) const HOUR_OFFSET: u32 = 15;
pub(super) const UTC_LONG: u32 = 20;
pub(super) const SKELETON_START: u32 = 512;
const INTERVAL_START: u32 = 4096;
pub(super) const MONTHS: u32 = 64;
pub(super) const WEEKDAYS: u32 = 144;
pub(super) const ERAS: u32 = 192;
pub(super) const PERIODS: u32 = 216;
pub(super) const BUDDHIST_ERAS: u32 = 252;
pub(super) const ISLAMIC_ERAS: u32 = 256;
pub(super) const ISLAMIC_MONTHS: u32 = 288;

pub(super) struct Data {
    pub(super) locales: Table,
    values: Table,
    skeletons: Table,
    periods: Table,
    cycles: Table,
}

pub(super) fn data() -> &'static Data {
    static DATA: OnceLock<Data> = OnceLock::new();
    DATA.get_or_init(|| Data {
        locales: PACK.table("locales"),
        values: PACK.table("values"),
        skeletons: PACK.table("skeletons"),
        periods: PACK.table("periods"),
        cycles: PACK.table("cycles"),
    })
}

impl Data {
    pub(super) fn value(&self, locale: u32, key: u32) -> Option<&'static str> {
        self.values
            .inherited(&self.locales, locale, key, &[])
            .map(|id| PACK.text(id))
    }

    pub(super) fn alternate_cycle(&self, region: &str, hour12: bool) -> char {
        self.cycles
            .find_text(region)
            .or_else(|| self.cycles.find_text("001"))
            .and_then(|row| char::from_u32(self.cycles.get(row, if hour12 { 1 } else { 2 })))
            .unwrap_or(if hour12 { 'h' } else { 'H' })
    }

    /// A fixed period (noon or midnight) takes precedence at the exact time.
    pub(super) fn period(&self, language: &str, minute: u32, exact: bool) -> u32 {
        let table = &self.periods;
        let mut start = 0;
        let mut end = table.rows();
        while start < end {
            let middle = start + (end - start) / 2;
            if table.text(middle, 0) < language {
                start = middle + 1;
            } else {
                end = middle;
            }
        }
        let mut fallback = u32::from(minute >= 720);
        for row in start..table.rows() {
            if table.text(row, 0) != language {
                break;
            }
            let begin = table.get(row, 1);
            let end = table.get(row, 2);
            if begin == end {
                let period = table.get(row, 3);
                if minute == begin && exact && period != 2 {
                    return period;
                }
            } else if (begin < end && (begin..end).contains(&minute))
                || (begin > end && (minute >= begin || minute < end))
            {
                fallback = table.get(row, 3);
            }
        }
        fallback
    }
}

pub(super) const PROPERTIES: [&str; 11] = [
    "weekday",
    "era",
    "year",
    "month",
    "day",
    "dayPeriod",
    "hour",
    "minute",
    "second",
    "fractionalSecondDigits",
    "timeZoneName",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub(super) enum Field {
    Weekday,
    Era,
    Year,
    Month,
    Day,
    Period,
    Hour,
    Minute,
    Second,
    Fraction,
    Zone,
}

impl Field {
    pub(super) const ALL: [Self; 11] = [
        Self::Weekday,
        Self::Era,
        Self::Year,
        Self::Month,
        Self::Day,
        Self::Period,
        Self::Hour,
        Self::Minute,
        Self::Second,
        Self::Fraction,
        Self::Zone,
    ];

    fn for_letter(letter: char) -> Option<Self> {
        Some(match letter {
            'E' | 'e' | 'c' => Self::Weekday,
            'G' => Self::Era,
            'y' | 'u' => Self::Year,
            'M' | 'L' => Self::Month,
            'd' => Self::Day,
            'a' | 'b' | 'B' => Self::Period,
            'H' | 'h' | 'K' | 'k' => Self::Hour,
            'm' => Self::Minute,
            's' => Self::Second,
            'S' => Self::Fraction,
            'z' | 'v' | 'O' => Self::Zone,
            _ => return None,
        })
    }

    fn letter(self, cycle: char) -> char {
        match self {
            Self::Weekday => 'E',
            Self::Era => 'G',
            Self::Year => 'y',
            Self::Month => 'M',
            Self::Day => 'd',
            Self::Period => 'B',
            Self::Hour => cycle,
            Self::Minute => 'm',
            Self::Second => 's',
            Self::Fraction => 'S',
            Self::Zone => 'z',
        }
    }

    pub(super) fn property(self) -> &'static str {
        PROPERTIES[self as usize]
    }

    pub(super) fn part(self) -> &'static str {
        match self {
            Self::Fraction => "fractionalSecond",
            _ => self.property(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Width {
    Numeric,
    TwoDigit,
    Short,
    Long,
    Narrow,
    ShortOffset,
    LongOffset,
    ShortGeneric,
    LongGeneric,
    Fraction(u8),
}

impl Width {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Numeric => "numeric",
            Self::TwoDigit => "2-digit",
            Self::Short => "short",
            Self::Long => "long",
            Self::Narrow => "narrow",
            Self::ShortOffset => "shortOffset",
            Self::LongOffset => "longOffset",
            Self::ShortGeneric => "shortGeneric",
            Self::LongGeneric => "longGeneric",
            Self::Fraction(_) => "numeric",
        }
    }

    pub(super) fn text_index(self) -> u32 {
        match self {
            Self::Long => 1,
            Self::Narrow => 2,
            _ => 0,
        }
    }

    fn distance(self, other: Self) -> i32 {
        if self == other {
            return 0;
        }
        match (self, other) {
            (Self::Numeric | Self::TwoDigit, Self::Numeric | Self::TwoDigit) => 3,
            (Self::Short | Self::Long | Self::Narrow, Self::Short | Self::Long | Self::Narrow) => 5,
            _ => 12,
        }
    }
}

pub(super) type Fields = [Option<Width>; 11];

#[derive(Clone)]
pub(super) enum Token {
    Literal(String),
    Field {
        kind: Field,
        letter: char,
        width: Width,
    },
}

/// LDML pattern letters and quoted literals. Unimplemented pattern letters
/// exclude a candidate instead of leaking the LDML spelling into output.
pub(super) fn parse(text: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut literal = String::new();
    let mut letters = text.chars().peekable();
    let mut quoted = false;
    while let Some(letter) = letters.next() {
        if letter == '\'' {
            if letters.next_if_eq(&'\'').is_some() {
                literal.push('\'');
            } else {
                quoted = !quoted;
            }
        } else if quoted || !letter.is_ascii_alphabetic() {
            literal.push(letter);
        } else {
            let kind = Field::for_letter(letter)?;
            if !literal.is_empty() {
                tokens.push(Token::Literal(std::mem::take(&mut literal)));
            }
            let mut count: u8 = 1;
            while letters.next_if_eq(&letter).is_some() {
                count = count.checked_add(1)?;
            }
            let width = match kind {
                Field::Fraction => Width::Fraction(count.min(3)),
                Field::Weekday | Field::Era | Field::Period => match count {
                    4 => Width::Long,
                    5 => Width::Narrow,
                    _ => Width::Short,
                },
                Field::Zone => match (letter, count) {
                    ('v', 4..) => Width::LongGeneric,
                    ('v', _) => Width::ShortGeneric,
                    ('O', 4..) => Width::LongOffset,
                    ('O', _) => Width::ShortOffset,
                    (_, 4..) => Width::Long,
                    _ => Width::Short,
                },
                Field::Month if count >= 3 => match count {
                    4 => Width::Long,
                    5 => Width::Narrow,
                    _ => Width::Short,
                },
                _ if count == 2 => Width::TwoDigit,
                _ => Width::Numeric,
            };
            tokens.push(Token::Field {
                kind,
                letter,
                width,
            });
        }
    }
    if !literal.is_empty() {
        tokens.push(Token::Literal(literal));
    }
    (!quoted).then_some(tokens)
}

pub(super) fn fields(pattern: &[Token]) -> Fields {
    let mut fields = [None; 11];
    for token in pattern {
        if let Token::Field { kind, width, .. } = token {
            fields[*kind as usize] = Some(*width);
        }
    }
    fields
}

pub(super) fn default_cycle(row: u32) -> char {
    let short = data().value(row, 7).and_then(parse).unwrap_or_default();
    short
        .into_iter()
        .find_map(|token| match token {
            Token::Field {
                kind: Field::Hour,
                letter,
                ..
            } => Some(letter),
            _ => None,
        })
        .unwrap_or('H')
}

fn clock_12(cycle: char) -> bool {
    matches!(cycle, 'h' | 'K')
}

fn score(request: &Fields, candidate: &[Token], cycle: char) -> Option<i32> {
    let has_period = request[Field::Period as usize].is_some();
    let mut cost = 0;
    for token in candidate {
        if let Token::Field { kind, letter, .. } = token {
            if *kind == Field::Period && clock_12(cycle) && request[Field::Hour as usize].is_some()
            {
                if has_period && *letter != 'B' {
                    cost += 5;
                }
                continue;
            }
            request[*kind as usize]?;
            if *kind == Field::Hour && clock_12(*letter) != clock_12(cycle) {
                cost += 20;
            }
        }
    }
    for (wanted, actual) in request.iter().zip(fields(candidate)) {
        cost += match (wanted, actual) {
            (Some(wanted), Some(actual)) => wanted.distance(actual),
            (Some(_), None) => 120,
            _ => 0,
        };
    }
    Some(cost)
}

/// Choose a CLDR skeleton. When no skeleton supplies every requested
/// field, add the missing fields in component order.
pub(super) fn components(row: u32, request: &Fields, cycle: char, best_fit: bool) -> Vec<Token> {
    if request.iter().all(Option::is_none) {
        return Vec::new();
    }
    let data = data();
    let mut best = (i32::MAX, Vec::new());
    for key in 0..data.skeletons.rows() {
        let Some(candidate) = data.value(row, SKELETON_START + key).and_then(parse) else {
            continue;
        };
        if let Some(cost) = score(request, &candidate, cycle) {
            if cost < best.0 {
                best = (cost, candidate);
            }
        }
    }
    let mut tokens = best.1;
    let present = fields(&tokens);
    for token in &mut tokens {
        if let Token::Field {
            kind,
            letter,
            width,
        } = token
        {
            if let Some(wanted) = request[*kind as usize] {
                // A minute or second inside a clock keeps the pattern's
                // padding. A date with a fixed numeric order may do so too.
                let padded =
                    matches!(kind, Field::Minute | Field::Second) && *width == Width::TwoDigit;
                if (best_fit && !padded) || matches!(kind, Field::Period | Field::Zone) {
                    *width = wanted;
                }
                if *kind == Field::Period {
                    *letter = 'B';
                }
            }
        }
    }
    for kind in Field::ALL {
        if let Some(width) = request[kind as usize] {
            if present[kind as usize].is_none() {
                if kind == Field::Fraction {
                    let at = tokens.iter().position(|token| {
                        matches!(
                            token,
                            Token::Field {
                                kind: Field::Second,
                                ..
                            }
                        )
                    });
                    if let Some(at) = at {
                        tokens.insert(
                            at + 1,
                            Token::Field {
                                kind,
                                letter: 'S',
                                width,
                            },
                        );
                        continue;
                    }
                }
                if !tokens.is_empty() {
                    tokens.push(Token::Literal(" ".to_owned()));
                }
                tokens.push(Token::Field {
                    kind,
                    letter: kind.letter(cycle),
                    width,
                });
            }
        }
    }
    clock(
        &mut tokens,
        cycle,
        request[Field::Period as usize].is_some(),
    );
    tokens
}

/// Select an interval pattern with the same fields as the single-date
/// pattern. Shared fields occur once; the differing field must appear
/// twice for the pattern to preserve both endpoints.
pub(super) fn interval(
    row: u32,
    single: &[Token],
    difference: Field,
    cycle: char,
) -> Option<Vec<Token>> {
    let request = fields(single);
    let data = data();
    let mut best: Option<(i32, Vec<Token>)> = None;
    for key in 0..data.skeletons.rows() {
        let index = INTERVAL_START + 16 * key + difference as u32;
        let Some(candidate) = data.value(row, index).and_then(parse) else {
            continue;
        };
        let actual = fields(&candidate);
        if !request
            .iter()
            .zip(actual)
            .all(|(want, have)| want.is_some() == have.is_some())
        {
            continue;
        }
        let duplicates = candidate
            .iter()
            .filter(|token| matches!(token, Token::Field {kind, ..} if *kind == difference))
            .count();
        if duplicates != 2 {
            continue;
        }
        let Some(cost) = score(&request, &candidate, cycle) else {
            continue;
        };
        if best.as_ref().is_none_or(|(current, _)| cost < *current) {
            best = Some((cost, candidate));
        }
    }
    let (_, mut tokens) = best?;
    for token in &mut tokens {
        if let Token::Field {
            kind,
            letter,
            width,
        } = token
        {
            *width = request[*kind as usize].expect("interval field width");
            if *kind == Field::Hour {
                *letter = cycle;
            }
        }
    }
    Some(tokens)
}

pub(super) fn clock(tokens: &mut Vec<Token>, cycle: char, explicit_period: bool) {
    let mut has_hour = false;
    for token in tokens.iter_mut() {
        if let Token::Field {
            kind: Field::Hour,
            letter,
            ..
        } = token
        {
            has_hour = true;
            *letter = cycle;
        }
    }
    if !has_hour || explicit_period {
        return;
    }
    let period = tokens.iter().position(|token| {
        matches!(
            token,
            Token::Field {
                kind: Field::Period,
                ..
            }
        )
    });
    match (clock_12(cycle), period) {
        (true, None) => {
            tokens.push(Token::Literal(" ".to_owned()));
            tokens.push(Token::Field {
                kind: Field::Period,
                letter: 'a',
                width: Width::Short,
            });
        }
        (false, Some(at)) => {
            tokens.remove(at);
            for adjacent in [at.saturating_sub(1), at] {
                if let Some(Token::Literal(text)) = tokens.get(adjacent) {
                    if text.trim().is_empty() {
                        tokens.remove(adjacent);
                        break;
                    }
                }
            }
        }
        _ => {}
    }
}

pub(super) fn join<T>(
    template: &str,
    mut arguments: [Vec<T>; 2],
    mut literal: impl FnMut(String) -> T,
) -> Vec<T> {
    fn unquote(text: &str) -> String {
        let mut chars = text.chars().peekable();
        let mut decoded = String::new();
        while let Some(c) = chars.next() {
            if c != '\'' || chars.next_if_eq(&'\'').is_some() {
                decoded.push(c);
            }
        }
        decoded
    }
    let mut result = Vec::new();
    let mut rest = template;
    while let Some(at) = rest.find('{') {
        let (prefix, after) = rest.split_at(at);
        let index = match after.as_bytes() {
            [b'{', b'0', b'}', ..] => 0,
            [b'{', b'1', b'}', ..] => 1,
            _ => break,
        };
        if !prefix.is_empty() {
            result.push(literal(unquote(prefix)));
        }
        result.append(&mut arguments[index]);
        rest = &after[3..];
    }
    if !rest.is_empty() {
        result.push(literal(unquote(rest)));
    }
    result
}
