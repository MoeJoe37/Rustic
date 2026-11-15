//! Text segmentation for Intl.Segmenter: the grapheme cluster, word and
//! sentence boundaries of UAX #29 (Unicode 17.0.0) without tailoring.
//!
//! Each function takes code points and returns one flag per position
//! `0..=len`, true where a boundary lies before that code point. Each is
//! one forward pass; lookahead only crosses a run it never crosses
//! again, so the work is linear in the text.

use std::sync::OnceLock;

use super::pack::{Pack, Table};

static PACK: Pack = Pack::new(include_bytes!("../../../../data/intl/segment.bin"));

/// Grapheme_Cluster_Break values, in the generator's order.
mod g {
    pub(super) const CR: u32 = 1;
    pub(super) const LF: u32 = 2;
    pub(super) const CONTROL: u32 = 3;
    pub(super) const EXTEND: u32 = 4;
    pub(super) const ZWJ: u32 = 5;
    pub(super) const REGIONAL: u32 = 6;
    pub(super) const PREPEND: u32 = 7;
    pub(super) const SPACING_MARK: u32 = 8;
    pub(super) const L: u32 = 9;
    pub(super) const V: u32 = 10;
    pub(super) const T: u32 = 11;
    pub(super) const LV: u32 = 12;
    pub(super) const LVT: u32 = 13;
}

/// Word_Break values.
mod w {
    pub(super) const CR: u32 = 1;
    pub(super) const LF: u32 = 2;
    pub(super) const NEWLINE: u32 = 3;
    pub(super) const EXTEND: u32 = 4;
    pub(super) const ZWJ: u32 = 5;
    pub(super) const REGIONAL: u32 = 6;
    pub(super) const FORMAT: u32 = 7;
    pub(super) const KATAKANA: u32 = 8;
    pub(super) const HEBREW_LETTER: u32 = 9;
    pub(super) const A_LETTER: u32 = 10;
    pub(super) const SINGLE_QUOTE: u32 = 11;
    pub(super) const DOUBLE_QUOTE: u32 = 12;
    pub(super) const MID_NUM_LET: u32 = 13;
    pub(super) const MID_LETTER: u32 = 14;
    pub(super) const MID_NUM: u32 = 15;
    pub(super) const NUMERIC: u32 = 16;
    pub(super) const EXTEND_NUM_LET: u32 = 17;
    pub(super) const W_SEG_SPACE: u32 = 18;
}

/// Sentence_Break values.
mod s {
    pub(super) const CR: u32 = 1;
    pub(super) const LF: u32 = 2;
    pub(super) const EXTEND: u32 = 3;
    pub(super) const SEP: u32 = 4;
    pub(super) const FORMAT: u32 = 5;
    pub(super) const SP: u32 = 6;
    pub(super) const LOWER: u32 = 7;
    pub(super) const UPPER: u32 = 8;
    pub(super) const O_LETTER: u32 = 9;
    pub(super) const NUMERIC: u32 = 10;
    pub(super) const A_TERM: u32 = 11;
    pub(super) const S_CONTINUE: u32 = 12;
    pub(super) const S_TERM: u32 = 13;
    pub(super) const CLOSE: u32 = 14;
}

/// Indic_Conjunct_Break values.
const CONSONANT: u32 = 1;
const CONJUNCT_EXTEND: u32 = 2;
const LINKER: u32 = 3;

/// The break properties of a code point.
#[derive(Clone, Copy)]
pub(super) struct Properties(u32);

impl Properties {
    fn grapheme(self) -> u32 {
        self.0 & 15
    }

    fn word(self) -> u32 {
        self.0 >> 4 & 31
    }

    fn sentence(self) -> u32 {
        self.0 >> 9 & 15
    }

    fn pictographic(self) -> bool {
        self.0 >> 13 & 1 != 0
    }

    fn conjunct(self) -> u32 {
        self.0 >> 14 & 3
    }

    /// Whether the code point makes a word segment "word-like": a
    /// letter, an ideograph or a digit.
    pub(super) fn word_like(self) -> bool {
        self.0 >> 16 & 1 != 0 || matches!(self.word(), w::NUMERIC | w::KATAKANA)
    }
}

pub(super) fn properties(code: u32) -> Properties {
    static TABLE: OnceLock<Table> = OnceLock::new();
    let table = TABLE.get_or_init(|| PACK.table("breaks"));
    // The first row starts at zero, so a row at or before `code` exists.
    let after = table.lower_bound(&[code + 1]);
    Properties(table.get(after - 1, 1))
}

/// Extended grapheme cluster boundaries (UAX #29 §3.1.1).
pub(super) fn grapheme_boundaries(text: &[Properties]) -> Vec<bool> {
    let mut out = Vec::with_capacity(text.len() + 1);
    out.push(true);
    let Some(first) = text.first() else {
        return out;
    };
    let mut prev = first.grapheme();
    // GB11: ExtPict Extend* (then ZWJ) ends at the previous code point.
    let mut pictographic = first.pictographic();
    let mut pictographic_zwj = false;
    let mut regional = usize::from(prev == g::REGIONAL);
    // GB9c: 0 none, 1 consonant then extend/linker, 2 the same with a linker.
    let mut conjunct = u8::from(first.conjunct() == CONSONANT);
    for properties in &text[1..] {
        let cur = properties.grapheme();
        let incb = properties.conjunct();
        let pict = properties.pictographic();
        let boundary = match (prev, cur) {
            (g::CR, g::LF) => false,
            (g::CONTROL | g::CR | g::LF, _) | (_, g::CONTROL | g::CR | g::LF) => true,
            (g::L, g::L | g::V | g::LV | g::LVT)
            | (g::LV | g::V, g::V | g::T)
            | (g::LVT | g::T, g::T)
            | (_, g::EXTEND | g::ZWJ | g::SPACING_MARK)
            | (g::PREPEND, _) => false,
            _ if incb == CONSONANT && conjunct == 2 => false,
            (g::ZWJ, _) if pict && pictographic_zwj => false,
            (g::REGIONAL, g::REGIONAL) => regional % 2 == 0,
            _ => true,
        };
        out.push(boundary);
        pictographic_zwj = pictographic && cur == g::ZWJ;
        pictographic = pict || (pictographic && cur == g::EXTEND);
        regional = if cur == g::REGIONAL { regional + 1 } else { 0 };
        conjunct = match incb {
            CONSONANT => 1,
            LINKER if conjunct > 0 => 2,
            CONJUNCT_EXTEND if conjunct > 0 => conjunct,
            _ => 0,
        };
        prev = cur;
    }
    out.push(true);
    out
}

fn word_ignored(class: u32) -> bool {
    matches!(class, w::EXTEND | w::FORMAT | w::ZWJ)
}

fn word_newline(class: u32) -> bool {
    matches!(class, w::CR | w::LF | w::NEWLINE)
}

fn letter(class: u32) -> bool {
    matches!(class, w::A_LETTER | w::HEBREW_LETTER)
}

/// Word boundaries (UAX #29 §4.1.1).
pub(super) fn word_boundaries(text: &[Properties]) -> Vec<bool> {
    let mut out = Vec::with_capacity(text.len() + 1);
    out.push(true);
    let Some(first) = text.first() else {
        return out;
    };
    // The classes of the last two code points after WB4 removed the
    // Extend, Format and ZWJ that follow another code point.
    let mut before = 0;
    let mut prev = first.word();
    let mut regional = usize::from(prev == w::REGIONAL);
    // The class of the code point right before, with nothing removed.
    let mut raw = prev;
    // The next class after position `index`, skipping what WB4 removes.
    let following = |index: usize| {
        text[index + 1..]
            .iter()
            .map(|properties| properties.word())
            .find(|class| !word_ignored(*class))
            .unwrap_or(0)
    };
    for (index, properties) in text.iter().enumerate().skip(1) {
        let cur = properties.word();
        let quote = |class| matches!(class, w::MID_NUM_LET | w::SINGLE_QUOTE);
        let boundary = if raw == w::CR && cur == w::LF {
            false
        } else if word_newline(raw) || word_newline(cur) {
            true
        } else if (raw == w::ZWJ && properties.pictographic())
            || (raw == w::W_SEG_SPACE && cur == w::W_SEG_SPACE)
            || word_ignored(cur)
        {
            false
        } else {
            let mid_letter = |class| class == w::MID_LETTER || quote(class);
            let mid_number = |class| class == w::MID_NUM || quote(class);
            let joined = (letter(prev) && letter(cur))
                || (letter(prev) && mid_letter(cur) && letter(following(index)))
                || (letter(before) && mid_letter(prev) && letter(cur))
                || (prev == w::HEBREW_LETTER && cur == w::SINGLE_QUOTE)
                || (prev == w::HEBREW_LETTER
                    && cur == w::DOUBLE_QUOTE
                    && following(index) == w::HEBREW_LETTER)
                || (before == w::HEBREW_LETTER
                    && prev == w::DOUBLE_QUOTE
                    && cur == w::HEBREW_LETTER)
                || (prev == w::NUMERIC && (cur == w::NUMERIC || letter(cur)))
                || (letter(prev) && cur == w::NUMERIC)
                || (before == w::NUMERIC && mid_number(prev) && cur == w::NUMERIC)
                || (prev == w::NUMERIC && mid_number(cur) && following(index) == w::NUMERIC)
                || (prev == w::KATAKANA && cur == w::KATAKANA)
                || ((letter(prev) || matches!(prev, w::NUMERIC | w::KATAKANA | w::EXTEND_NUM_LET))
                    && cur == w::EXTEND_NUM_LET)
                || (prev == w::EXTEND_NUM_LET
                    && (letter(cur) || matches!(cur, w::NUMERIC | w::KATAKANA)))
                || (prev == w::REGIONAL && cur == w::REGIONAL && regional % 2 == 1);
            !joined
        };
        out.push(boundary);
        raw = cur;
        // WB4: the code point joins the one before it, unless that is
        // a line end.
        if !(word_ignored(cur) && !word_newline(prev)) || boundary {
            before = prev;
            prev = cur;
            regional = if cur == w::REGIONAL { regional + 1 } else { 0 };
        }
    }
    out.push(true);
    out
}

fn paragraph_end(class: u32) -> bool {
    matches!(class, s::SEP | s::CR | s::LF)
}

fn terminator(class: u32) -> bool {
    matches!(class, s::A_TERM | s::S_TERM)
}

/// Sentence boundaries (UAX #29 §5.1).
pub(super) fn sentence_boundaries(text: &[Properties]) -> Vec<bool> {
    let mut out = Vec::with_capacity(text.len() + 1);
    out.push(true);
    let Some(first) = text.first() else {
        return out;
    };
    let mut before = 0;
    let mut prev = first.sentence();
    let mut raw = prev;
    // After `SATerm Close* Sp*`: the terminator and whether a space
    // followed it.
    let mut ending: Option<(u32, bool)> = terminator(prev).then_some((prev, false));
    // SB8's answer for the current ending, found on first use.
    let mut lower_follows: Option<bool> = None;
    for (index, properties) in text.iter().enumerate().skip(1) {
        let cur = properties.sentence();
        let ignored = matches!(cur, s::EXTEND | s::FORMAT);
        let boundary = if raw == s::CR && cur == s::LF {
            false
        } else if paragraph_end(prev) {
            true
        } else if ignored
            || (prev == s::A_TERM && cur == s::NUMERIC)
            || (matches!(before, s::UPPER | s::LOWER) && prev == s::A_TERM && cur == s::UPPER)
        {
            false
        } else if let Some((kind, spaced)) = ending {
            let continues = kind == s::A_TERM
                && *lower_follows.get_or_insert_with(|| {
                    text[index..]
                        .iter()
                        .map(|properties| properties.sentence())
                        .find(|class| {
                            matches!(*class, s::O_LETTER | s::UPPER | s::LOWER)
                                || paragraph_end(*class)
                                || terminator(*class)
                        })
                        == Some(s::LOWER)
                });
            let joined = continues
                || cur == s::S_CONTINUE
                || terminator(cur)
                || (!spaced && cur == s::CLOSE)
                || cur == s::SP
                || paragraph_end(cur);
            !joined
        } else {
            false
        };
        out.push(boundary);
        raw = cur;
        // SB5: the code point joins the one before it, unless that
        // ended a paragraph.
        if ignored && !paragraph_end(prev) {
            continue;
        }
        ending = if terminator(cur) {
            lower_follows = None;
            Some((cur, false))
        } else {
            match ending {
                Some((kind, false)) if cur == s::CLOSE => Some((kind, false)),
                Some((kind, _)) if cur == s::SP => Some((kind, true)),
                _ => None,
            }
        };
        before = prev;
        prev = cur;
    }
    out.push(true);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASES: &str = include_str!("../../../../tests/fixtures/segmentation_cases.txt");

    #[test]
    fn boundaries_match_the_unicode_conformance_tests() {
        let mut counts = [0; 3];
        for line in CASES.lines().filter(|line| !line.starts_with('#')) {
            let (kind, case) = line.split_once(' ').expect("a kind and a case");
            let mut text = Vec::new();
            let mut expected = Vec::new();
            let mut boundary = false;
            for token in case.split(' ') {
                if token == "/" {
                    boundary = true;
                } else {
                    text.push(properties(u32::from_str_radix(token, 16).expect("hex")));
                    expected.push(boundary);
                    boundary = false;
                }
            }
            expected.push(boundary);
            let (index, actual) = match kind {
                "g" => (0, grapheme_boundaries(&text)),
                "w" => (1, word_boundaries(&text)),
                _ => (2, sentence_boundaries(&text)),
            };
            assert_eq!(actual, expected, "{line}");
            counts[index] += 1;
        }
        assert!(counts.iter().all(|&count| count > 400), "{counts:?}");
    }

    #[test]
    fn empty_text_has_one_boundary() {
        assert_eq!(grapheme_boundaries(&[]), [true]);
        assert_eq!(word_boundaries(&[]), [true]);
        assert_eq!(sentence_boundaries(&[]), [true]);
    }
}
