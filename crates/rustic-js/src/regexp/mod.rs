//! Regular expressions (ECMA-262 §22.2): the pattern parser with its
//! early errors, and a backtracking matcher with bounded memory and a
//! cancellation hook, backed by a linear-time matcher for searches that
//! backtrack too much. The RegExp objects that use it are in
//! `runtime/builtins/regexp.rs`.

mod case;
mod class;
mod parse;
mod pike;
mod vm;

pub use parse::PatternError;
pub(crate) use vm::Input;
pub use vm::UNSET;

/// The flags of a regular expression.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    /// `d`
    pub has_indices: bool,
    /// `g`
    pub global: bool,
    /// `i`
    pub ignore_case: bool,
    /// `m`
    pub multiline: bool,
    /// `s`
    pub dot_all: bool,
    /// `u`
    pub unicode: bool,
    /// `v`
    pub sets: bool,
    /// `y`
    pub sticky: bool,
}

impl Flags {
    /// Parse flag letters: `None` for an unknown or repeated letter, or
    /// for `u` with `v`.
    pub fn parse(text: impl IntoIterator<Item = u16>) -> Option<Flags> {
        let mut f = Flags::default();
        for unit in text {
            let slot = match unit {
                0x64 => &mut f.has_indices,
                0x67 => &mut f.global,
                0x69 => &mut f.ignore_case,
                0x6D => &mut f.multiline,
                0x73 => &mut f.dot_all,
                0x75 => &mut f.unicode,
                0x76 => &mut f.sets,
                0x79 => &mut f.sticky,
                _ => return None,
            };
            if *slot {
                return None;
            }
            *slot = true;
        }
        (!(f.unicode && f.sets)).then_some(f)
    }

    /// The flags in canonical order (`hasIndices` first), as the `flags`
    /// getter prints them.
    pub fn to_text(self) -> String {
        [
            (self.has_indices, 'd'),
            (self.global, 'g'),
            (self.ignore_case, 'i'),
            (self.multiline, 'm'),
            (self.dot_all, 's'),
            (self.unicode, 'u'),
            (self.sets, 'v'),
            (self.sticky, 'y'),
        ]
        .into_iter()
        .filter_map(|(on, c)| on.then_some(c))
        .collect()
    }
}

/// Why a match did not finish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecError {
    /// The backtracking state outgrew its bound.
    TooComplex,
    /// The cancellation hook asked to stop.
    Cancelled,
}

/// A compiled regular expression.
#[derive(Debug)]
pub struct Regex {
    program: vm::Program,
    /// The same pattern for the linear-time matcher, when it can run it.
    pike: Option<vm::Program>,
    /// Capturing groups, not counting the whole match.
    pub groups: u32,
    /// Group names (UTF-16) and the groups that carry each.
    pub names: Vec<(Vec<u16>, Vec<u32>)>,
    /// The flags it was compiled with.
    pub flags: Flags,
}

impl Regex {
    /// Parse and compile `pattern` (UTF-16 code units).
    pub fn new(pattern: &[u16], flags: Flags) -> Result<Regex, PatternError> {
        let parsed = parse::parse(pattern, flags)?;
        if parsed.uses_properties {
            return Err(PatternError {
                message: "Unicode property escapes are not supported yet",
                unsupported: true,
            });
        }
        let program = vm::compile(&parsed.node, parsed.groups, flags);
        let pike = vm::compile_pike(&parsed.node, parsed.groups, flags);
        Ok(Regex {
            program,
            pike,
            groups: parsed.groups,
            names: parsed.names,
            flags,
        })
    }

    /// Bytes the compiled program holds, for the heap budget.
    pub fn footprint(&self) -> usize {
        std::mem::size_of::<Regex>()
            + self.program.footprint()
            + self.pike.as_ref().map_or(0, vm::Program::footprint)
    }

    /// Check `pattern` without compiling it (literal early errors).
    pub fn validate(pattern: &[u16], flags: Flags) -> Result<(), PatternError> {
        parse::parse(pattern, flags).map(drop)
    }

    /// Check that `pattern` is valid and uses nothing unsupported.
    pub fn validate_supported(pattern: &[u16], flags: Flags) -> Result<(), PatternError> {
        let parsed = parse::parse(pattern, flags)?;
        if parsed.uses_properties {
            return Err(PatternError {
                message: "Unicode property escapes",
                unsupported: true,
            });
        }
        Ok(())
    }

    /// The first match at `start` or after it (only at `start` when
    /// `sticky`) in Latin-1 text, one byte per code unit: the capture
    /// positions, two per group, [`UNSET`] for a group that did not
    /// participate. `poll` is called now and then; returning true stops
    /// the search with [`ExecError::Cancelled`].
    pub fn search_latin1(
        &self,
        input: &[u8],
        start: usize,
        sticky: bool,
        poll: &mut dyn FnMut() -> bool,
    ) -> Result<Option<Vec<u32>>, ExecError> {
        self.search(input, start, sticky, poll)
    }

    /// [`Regex::search_latin1`] over UTF-16 code units.
    pub fn search_utf16(
        &self,
        input: &[u16],
        start: usize,
        sticky: bool,
        poll: &mut dyn FnMut() -> bool,
    ) -> Result<Option<Vec<u32>>, ExecError> {
        self.search(input, start, sticky, poll)
    }

    /// The first match at `start` or after it (only at `start` when
    /// `sticky`).
    pub(crate) fn search<I: Input + ?Sized>(
        &self,
        input: &I,
        start: usize,
        sticky: bool,
        poll: &mut dyn FnMut() -> bool,
    ) -> Result<Option<Vec<u32>>, ExecError> {
        let left = if self.pike.is_some() {
            FALLBACK_BACKTRACKS
        } else {
            u64::MAX
        };
        self.search_with(input, start, sticky, poll, left)
    }

    /// [`Regex::search`] moving to the linear-time matcher after `left`
    /// backtracks, summed over every start position.
    fn search_with<I: Input + ?Sized>(
        &self,
        input: &I,
        start: usize,
        sticky: bool,
        poll: &mut dyn FnMut() -> bool,
        left: u64,
    ) -> Result<Option<Vec<u32>>, ExecError> {
        let unicode = self.flags.unicode || self.flags.sets;
        let mut budget = vm::Budget {
            left,
            exhausted: false,
        };
        let mut pos = start;
        while pos <= input.len() {
            let found = match self.program.match_at(input, pos, poll, &mut budget) {
                Err(ExecError::TooComplex) if budget.exhausted && self.pike.is_some() => {
                    // Earlier starts did not match, so the linear-time
                    // matcher picks up here.
                    let pike = self.pike.as_ref().expect("checked");
                    let found = pike.pike_search(input, pos, sticky, poll)?;
                    return Ok(found.map(|regs| self.groups_of(regs)));
                }
                result => result?,
            };
            if let Some(regs) = found {
                return Ok(Some(self.groups_of(regs)));
            }
            if sticky {
                break;
            }
            // AdvanceStringIndex: a whole surrogate pair with `u`.
            pos += if unicode
                && pos + 1 < input.len()
                && (0xD800..0xDC00).contains(&input.unit(pos))
                && (0xDC00..0xE000).contains(&input.unit(pos + 1))
            {
                2
            } else {
                1
            };
        }
        Ok(None)
    }

    /// The capture registers of a match, without the matcher's own.
    fn groups_of(&self, mut regs: Vec<u32>) -> Vec<u32> {
        regs.truncate(2 * (self.groups as usize + 1));
        regs
    }
}

/// Backtracks a search takes before moving to the linear-time matcher
/// (V8's experimental engine moves after a similar count).
const FALLBACK_BACKTRACKS: u64 = 1 << 16;

#[cfg(test)]
mod tests {
    use super::*;

    fn units(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// The groups of the first match of `pattern` in `text`, as text.
    fn find(pattern: &str, flags: &str, text: &str) -> Option<Vec<Option<String>>> {
        let flags = Flags::parse(units(flags)).expect("valid flags");
        let re = Regex::new(&units(pattern), flags).expect("valid pattern");
        let input = units(text);
        let m = re
            .search(&input[..], 0, flags.sticky, &mut || false)
            .expect("finishes")?;
        Some(
            m.chunks(2)
                .map(|p| {
                    (p[0] != UNSET && p[1] != UNSET)
                        .then(|| String::from_utf16_lossy(&input[p[0] as usize..p[1] as usize]))
                })
                .collect(),
        )
    }

    fn first(pattern: &str, flags: &str, text: &str) -> Option<String> {
        find(pattern, flags, text).map(|g| g[0].clone().expect("group 0"))
    }

    #[test]
    fn basic_matching() {
        assert_eq!(first("b+", "", "abbbc").as_deref(), Some("bbb"));
        assert_eq!(first("b+?", "", "abbbc").as_deref(), Some("b"));
        assert_eq!(first("a|ab|abc", "", "abc").as_deref(), Some("a"));
        assert_eq!(first("^abc$", "", "abc").as_deref(), Some("abc"));
        assert_eq!(first("^b", "m", "a\nb").as_deref(), Some("b"));
        assert_eq!(first("a.c", "", "a\nc"), None);
        assert_eq!(first("a.c", "s", "a\nc").as_deref(), Some("a\nc"));
        assert_eq!(first("\\bfoo\\b", "", "a foo b").as_deref(), Some("foo"));
        assert_eq!(first("[a-c]+", "", "xxabcabd").as_deref(), Some("abcab"));
        assert_eq!(first("[^a-c]+", "", "abxyz").as_deref(), Some("xyz"));
        assert_eq!(first("\\d{2,3}", "", "a12345").as_deref(), Some("123"));
        assert_eq!(first("x{2}", "", "xxx").as_deref(), Some("xx"));
        assert_eq!(first("ABC", "i", "xabcx").as_deref(), Some("abc"));
        assert_eq!(first("a{,2}", "", "a{,2}").as_deref(), Some("a{,2}"));
    }

    #[test]
    fn groups_and_references() {
        assert_eq!(
            find("(a)|(b)", "", "b"),
            Some(vec![Some("b".into()), None, Some("b".into())])
        );
        // Captures inside a quantified group reset at each iteration.
        assert_eq!(
            find("(?:(a)|b)+", "", "ab"),
            Some(vec![Some("ab".into()), None])
        );
        assert_eq!(first("(\\w)\\1", "", "abccd").as_deref(), Some("cc"));
        assert_eq!(first("(?<x>.)\\k<x>", "", "abbc").as_deref(), Some("bb"));
        assert_eq!(first("(a)\\1", "i", "aA").as_deref(), Some("aA"));
        // Empty iterations past the minimum stop a loop.
        assert_eq!(first("(a*)*b", "", "aab").as_deref(), Some("aab"));
        assert_eq!(
            find("(a*)+", "", "b"),
            Some(vec![Some("".into()), Some("".into())])
        );
        assert_eq!(
            find("(z)((a+)?(b+)?(c))*", "", "zaacbbbcac"),
            Some(vec![
                Some("zaacbbbcac".into()),
                Some("z".into()),
                Some("ac".into()),
                Some("a".into()),
                None,
                Some("c".into()),
            ])
        );
    }

    #[test]
    fn lookaround() {
        assert_eq!(first("a(?=b)", "", "acab").as_deref(), Some("a"));
        assert_eq!(first("a(?!b)", "", "abac").as_deref(), Some("a"));
        assert_eq!(first("(?<=\\$)\\d+", "", "a1 $42").as_deref(), Some("42"));
        assert_eq!(first("(?<!\\$)\\b\\d+", "", "$4 42").as_deref(), Some("42"));
        assert_eq!(
            find("(?<=(\\d+)(\\d+))$", "", "1053"),
            Some(vec![Some("".into()), Some("1".into()), Some("053".into())])
        );
        assert_eq!(
            find("(?=(a+))a*b\\1", "", "baaabac"),
            Some(vec![Some("aba".into()), Some("a".into())])
        );
    }

    #[test]
    fn unicode_mode() {
        assert_eq!(first(".", "u", "😀x").as_deref(), Some("😀"));
        assert_eq!(
            first(".", "", "😀x").map(|s| s.encode_utf16().count()),
            Some(1)
        );
        assert_eq!(first("\\u{1F600}", "u", "a😀").as_deref(), Some("😀"));
        assert_eq!(first("[😀-😂]", "u", "a😁").as_deref(), Some("😁"));
        assert_eq!(first("\\w", "iu", "\u{17F}").as_deref(), Some("\u{17F}"));
        assert_eq!(first("\\w", "i", "\u{17F}"), None);
        assert_eq!(first("[\\q{abc|d}x]", "v", "zabc").as_deref(), Some("abc"));
        assert_eq!(first("[\\w--\\d]+", "v", "12ab3").as_deref(), Some("ab"));
        assert_eq!(
            first("[[a-z]&&[aeiou]]+", "v", "xyzaeb").as_deref(),
            Some("ae")
        );
        assert_eq!(first("(?i:a)b", "", "Ab").as_deref(), Some("Ab"));
        assert_eq!(first("(?i:a)b", "", "AB"), None);
    }

    #[test]
    fn early_errors() {
        let bad = |p: &str, f: &str| {
            Regex::new(&units(p), Flags::parse(units(f)).expect("flags")).is_err()
        };
        assert!(bad("a{2,1}", ""));
        assert!(bad("(?<a>.)(?<a>.)", ""));
        assert!(!bad("(?<a>x)|(?<a>y)", ""));
        assert!(bad("\\1", "u"));
        assert!(!bad("\\1", ""));
        assert!(bad("{", "u"));
        assert!(!bad("{", ""));
        assert!(bad("(", ""));
        assert!(bad(")", ""));
        assert!(bad("[b-a]", ""));
        assert!(bad("\\k<x>", "u"));
        assert!(!bad("\\k", ""));
        assert!(bad("(?<a>.)\\k", ""));
        assert!(bad("[a&&&b]", "v"));
        assert!(bad("[^\\q{ab}]", "v"));
        assert!(bad("(?i-i:a)", ""));
        assert!(bad("a**", ""));
        assert!(bad("(?<=a)+", ""));
        assert!(!bad("(?=a)+", ""));
        assert!(bad("(?=a)+", "u"));
    }

    #[test]
    fn catastrophic_patterns_stop_when_asked() {
        // A backreference keeps the pattern on the backtracker.
        let re = Regex::new(&units("(a|a)*\\1b"), Flags::default()).expect("valid");
        assert!(re.pike.is_none());
        let input = units(&"a".repeat(40));
        let mut polls = 0;
        let result = re.search(&input[..], 0, false, &mut || {
            polls += 1;
            polls > 10
        });
        assert_eq!(result, Err(ExecError::Cancelled));
    }

    #[test]
    fn exponential_patterns_finish_in_linear_time() {
        // TD-111: each of these takes exponential (or, for the last,
        // quadratic) time on the backtracker alone.
        for (pattern, text) in [
            ("(a+)+b", "a".repeat(5_000)),
            ("(a*)*b", "a".repeat(5_000)),
            ("(?:a|a)*b", "a".repeat(5_000)),
            ("(x+x+)+y", "x".repeat(5_000)),
            ("^(\\w+\\s?)*$", format!("{}!", "word ".repeat(1_000))),
            ("a*b", "a".repeat(20_000)),
        ] {
            let re = Regex::new(&units(pattern), Flags::default()).expect("valid");
            let input = units(&text);
            let mut polls = 0;
            let result = re.search(&input[..], 0, false, &mut || {
                polls += 1;
                false
            });
            assert_eq!(result, Ok(None), "{pattern}");
            // Polls come every 16,384 steps: linear work, not 2^5000.
            assert!(polls < 20_000, "{pattern}: {polls} polls");
        }
        let re = Regex::new(&units("(a+)+b"), Flags::default()).expect("valid");
        let input = units(&format!("{}b", "a".repeat(5_000)));
        let found = re.search(&input[..], 0, false, &mut || false);
        assert_eq!(found, Ok(Some(vec![0, 5_001, 0, 5_000])));
    }

    /// Search with both matchers alone and compare.
    fn agree(pattern: &str, flags: &str, text: &str) {
        let flags = Flags::parse(units(flags)).expect("valid flags");
        let re = Regex::new(&units(pattern), flags).expect("valid pattern");
        let pike = re.pike.as_ref().expect("a linear-time program");
        let input = units(text);
        for start in 0..=input.len() {
            for sticky in [false, true] {
                let backtracker =
                    re.search_with(&input[..], start, sticky, &mut || false, u64::MAX);
                let linear = pike
                    .pike_search(&input[..], start, sticky, &mut || false)
                    .map(|m| m.map(|regs| re.groups_of(regs)));
                assert_eq!(
                    backtracker, linear,
                    "/{pattern}/ on {text:?} from {start}, sticky {sticky}"
                );
            }
        }
    }

    #[test]
    fn the_linear_time_matcher_agrees_with_the_backtracker() {
        let cases: &[(&str, &str, &[&str])] = &[
            ("a|ab", "", &["ab", "xab", ""]),
            ("ab|a", "", &["ab", "xa"]),
            ("(a|ab)(c|bcd)(d*)", "", &["abcd", "abcdd"]),
            ("(a+)(a*)", "", &["aaaa"]),
            ("(a+?)(a*)", "", &["aaaa"]),
            ("(a*?)b", "", &["aab", "b", "ab"]),
            ("((a)|b)+", "", &["ab", "ba", "aab"]),
            ("(z)((a+)?(b+)?(c))*", "", &["zaacbbbcac"]),
            ("(a)|b", "", &["b", "a"]),
            ("(?:(a)|b)*", "", &["aba", "ab"]),
            ("(\\w+)\\s(\\w+)", "", &["hello big world"]),
            ("a{2,4}", "", &["aaaaa", "a", "aaa"]),
            ("a{2,4}?", "", &["aaaaa"]),
            ("(ab){2}", "", &["abababx"]),
            ("(a|b){1,3}c", "", &["ababc", "abc", "c"]),
            ("^abc$", "m", &["x\nabc\ny", "abc"]),
            ("\\bfoo\\b", "", &["a foo b", "foobar"]),
            ("\\Bo", "", &["foo o"]),
            ("[a-c]+", "i", &["xABCaz"]),
            (".+", "", &["ab\ncd"]),
            (".+", "s", &["ab\ncd"]),
            ("x*", "", &["", "y", "xxy"]),
            ("(a*)*", "", &["b", "aa"]),
            ("(a*)+", "", &["b", "aa"]),
            ("(a|)*b", "", &["aab"]),
            ("(?:a?)*?b", "", &["ab"]),
            ("(\\u{1F600}|.)", "u", &["\u{1F600}x"]),
            (".", "u", &["\u{1F600}", "a\u{1F600}"]),
            ("[\\q{abc|d}x]+", "v", &["abcdxq"]),
            ("(?i:a)b", "", &["Ab", "AB"]),
            ("(a)|(b)|(c)", "", &["c"]),
            ("((a)|(b))*", "", &["ab", "ba"]),
        ];
        for (pattern, flags, texts) in cases {
            for text in *texts {
                agree(pattern, flags, text);
            }
        }
    }
}
