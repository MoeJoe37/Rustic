//! ECMAScript regular expressions (`rustic-js::regexp`): crash-freedom
//! and a representation differential.
//!
//! The input is a flags byte, a pattern up to the first newline, and a
//! subject after it. Invariants: parsing and matching never panic or
//! overflow the stack; [`Regex::validate`] accepts exactly the patterns
//! [`Regex::new`] accepts or reports as unsupported; a match lies inside the
//! subject with every capture well ordered; and matching the subject as
//! Latin-1 bytes gives the same result as matching it as UTF-16, since the
//! engine stores short strings in either form. Every match is bounded by a
//! poll budget, so exponential patterns end as cancelled.
use crate::Target;
use rustic_js::regexp::{ExecError, Flags, Regex, UNSET};

pub(crate) struct JsRegExpTarget;

/// Matcher polls (16,384 steps each) before a match is cancelled.
const POLL_BUDGET: u32 = 64;

/// Patterns in the shape the target reads: flags byte, pattern, subject.
pub(crate) fn seeds() -> Vec<Vec<u8>> {
    const CASES: &[(u8, &str, &str)] = &[
        (0, "a(b)?c|d", "xacd"),
        (1, "[^ab]c", "Ac bc Bc dc"),
        (8, "(?<y>\\d{4})-(?<m>\\d\\d)\\k<m>", "2026-1111"),
        (2, "^b$", "a\nb\nc"),
        (4, "a.c", "a\nc"),
        (0, "(a+)+b", "aaaaaaaaaaaaaaaaaaaaaaaa"),
        (0, "(?<=\\$)\\d+(?!\\d)", "cost $42"),
        (0, "(?<!x)y{2,3}?", "xyyy yyy"),
        (9, "\\w\\b\\W", "\u{17f}! k "),
        (0, "(?i:[^ab])c(?-i:d)", "Bcd xcD"),
        (16, "[\\d--[4-6]]+", "123456789"),
        (16, "[\\q{abc|d}a-c&&b]", "abcd"),
        (0, "(a*)*b|\\1", "aab"),
        (32, "y", "xyz"),
        (0, "\\cJ\\x41\\u0042[\\b]\\0", "\nAB\u{8}\0"),
        (0, "a{2,1", "a{2,1"),
        (0, "]{}", "]{}"),
    ];
    // Each case as written and with one flag toggled.
    let toggles = [0u8, 1, 2, 4, 8, 16, 32];
    let mut seeds = Vec::new();
    for &(flags, pattern, subject) in CASES {
        for toggle in toggles {
            let mut input = vec![flags ^ toggle];
            input.extend(latin1(pattern));
            input.push(b'\n');
            input.extend(latin1(subject));
            seeds.push(input);
        }
    }
    seeds
}

/// `text` truncated to Latin-1, which is how the target reads its bytes.
fn latin1(text: &str) -> Vec<u8> {
    text.chars().map(|c| u32::from(c).min(0xFF) as u8).collect()
}

/// `i`, `m`, `s`, `u`, `v` (only without `u`), `y` from the flags byte.
fn flags(byte: u8) -> Flags {
    let unicode = byte & 8 != 0;
    Flags {
        ignore_case: byte & 1 != 0,
        multiline: byte & 2 != 0,
        dot_all: byte & 4 != 0,
        unicode,
        sets: !unicode && byte & 16 != 0,
        sticky: byte & 32 != 0,
        ..Flags::default()
    }
}

type Outcome = Result<Option<Vec<u32>>, ExecError>;

fn check_match(outcome: &Outcome, len: usize) -> Result<(), String> {
    let Ok(Some(regs)) = outcome else {
        return Ok(());
    };
    for pair in regs.chunks(2) {
        let (start, end) = (pair[0], pair[1]);
        if (start == UNSET) != (end == UNSET) {
            return Err(format!("half-set capture {pair:?}"));
        }
        if start != UNSET && (start > end || end as usize > len) {
            return Err(format!("capture {pair:?} outside 0..{len}"));
        }
    }
    if regs.first() == Some(&UNSET) {
        return Err("a match without a whole-match position".into());
    }
    Ok(())
}

impl Target for JsRegExpTarget {
    fn name(&self) -> &'static str {
        "js-regexp"
    }

    fn max_len(&self) -> usize {
        512
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"(?:",
            b"(?<n>",
            b"\\k<n>",
            b"(?=",
            b"(?!",
            b"(?<=",
            b"(?<!",
            b"(?i:",
            b"(?-i:",
            b"[^",
            b"[a-z]",
            b"\\d",
            b"\\w",
            b"\\b",
            b"\\B",
            b"\\1",
            b"\\u{1F600}",
            b"\\uD83D\\uDE00",
            b"\\p{L}",
            b"\\q{",
            b"--",
            b"&&",
            b"{2,3}",
            b"*?",
            b"+",
            b"|",
            b"^",
            b"$",
            b".",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let Some((&flag_byte, rest)) = input.split_first() else {
            return Ok(());
        };
        let flags = flags(flag_byte);
        let split = rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
        let pattern: Vec<u16> = rest[..split].iter().map(|&b| u16::from(b)).collect();
        let subject = rest.get(split + 1..).unwrap_or(&[]);
        let valid = Regex::validate(&pattern, flags).is_ok();
        let regex = match Regex::new(&pattern, flags) {
            Ok(regex) => regex,
            Err(e) => {
                if valid != e.unsupported {
                    return Err(format!(
                        "validate says {valid} but compiling fails with {:?}",
                        e.message
                    ));
                }
                return Ok(());
            }
        };
        if !valid {
            return Err("validate rejects a pattern that compiles".into());
        }
        let wide: Vec<u16> = subject.iter().map(|&b| u16::from(b)).collect();
        let mut polls = 0;
        let narrow = regex.search_latin1(subject, 0, flags.sticky, &mut || {
            polls += 1;
            polls > POLL_BUDGET
        });
        let mut polls = 0;
        let utf16 = regex.search_utf16(&wide, 0, flags.sticky, &mut || {
            polls += 1;
            polls > POLL_BUDGET
        });
        check_match(&narrow, subject.len())?;
        let comparable = |o: &Outcome| o.is_ok();
        if comparable(&narrow) && comparable(&utf16) && narrow != utf16 {
            return Err(format!(
                "Latin-1 and UTF-16 subjects match differently: {narrow:?} and {utf16:?}"
            ));
        }
        if narrow.is_ok() != utf16.is_ok() {
            return Err(format!(
                "one representation finished and the other did not: {narrow:?} and {utf16:?}"
            ));
        }
        Ok(())
    }
}
