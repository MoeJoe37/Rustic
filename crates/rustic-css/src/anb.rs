//! The `An+B` microsyntax (CSS Syntax Level 3 §6), parsed from tokens.
//!
//! Working on tokens rather than text is what lets escaped digits and dashes
//! behave as the specification requires: `3n-1` is one dimension token with
//! unit `n-1`, while `3n -1` is a dimension followed by a signed number.

use crate::{Numeric, Token, Value};

/// Parse `values` (the arguments of an `:nth-*()` function, or any other
/// component value list) as `An+B`, returning `(A, B)`.
///
/// Leading and trailing whitespace is allowed; anything else that is not part
/// of the grammar makes the whole input invalid. Out-of-range integers
/// saturate to the `i32` range.
///
/// ```
/// use rustic_css::{CssParser, parse_an_plus_b};
/// let values = CssParser::default().component_values(" -n + 3 ").unwrap().value;
/// assert_eq!(parse_an_plus_b(&values), Some((-1, 3)));
/// ```
pub fn parse_an_plus_b(values: &[Value]) -> Option<(i32, i32)> {
    let mut tokens = Vec::with_capacity(values.len());
    for value in values {
        tokens.push(value.token()?);
    }
    let mut p = Tokens { tokens, pos: 0 };
    p.skip_whitespace();
    let result = match p.next()? {
        Token::Number(n) if n.integer => (0, saturate(n.value)),
        Token::Dimension(n, unit) if n.integer => p.after_n(saturate(n.value), unit)?,
        Token::Ident(ident) => match ident.to_ascii_lowercase().as_str() {
            "odd" => (2, 1),
            "even" => (2, 0),
            lower => match lower.strip_prefix('-') {
                Some(rest) => p.after_n(-1, rest)?,
                None => p.after_n(1, lower)?,
            },
        },
        Token::Delim('+') => match p.next()? {
            // `+n`: the ident must follow the `+` directly.
            Token::Ident(ident) if !ident.starts_with('-') => p.after_n(1, ident)?,
            _ => return None,
        },
        _ => return None,
    };
    p.skip_whitespace();
    p.at_end().then_some(result)
}

fn saturate(value: f64) -> i32 {
    // `as` saturates out-of-range floats; values are integral here.
    value as i32
}

fn is_signless(n: &Numeric) -> bool {
    n.integer && n.repr.starts_with(|c: char| c.is_ascii_digit())
}

fn is_signed(n: &Numeric) -> bool {
    n.integer && n.repr.starts_with(['+', '-'])
}

struct Tokens<'a> {
    tokens: Vec<&'a Token>,
    pos: usize,
}

impl<'a> Tokens<'a> {
    fn next(&mut self) -> Option<&'a Token> {
        let token = self.tokens.get(self.pos).copied();
        self.pos += 1;
        token
    }

    fn skip_whitespace(&mut self) {
        while self.tokens.get(self.pos).is_some_and(|t| t.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    /// After the `A` part: `rest` is the `n...` text of an ident or unit.
    fn after_n(&mut self, a: i32, rest: &str) -> Option<(i32, i32)> {
        let rest = rest.to_ascii_lowercase();
        let digits = match rest.as_str() {
            "n" => return Some((a, self.b_tail()?)),
            "n-" => {
                self.skip_whitespace();
                return match self.next()? {
                    Token::Number(n) if is_signless(n) => Some((a, saturate(-n.value))),
                    _ => None,
                };
            }
            other => other.strip_prefix("n-")?,
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let b = digits
            .bytes()
            .fold(0i64, |acc, d| (acc * 10 + i64::from(d - b'0')).min(1 << 40));
        Some((a, i32::try_from(-b).unwrap_or(i32::MIN)))
    }

    /// An optional `B` after `An`: a signed integer, or `+`/`-` and a
    /// signless integer, with whitespace allowed around the sign.
    fn b_tail(&mut self) -> Option<i32> {
        self.skip_whitespace();
        let Some(token) = self.tokens.get(self.pos).copied() else {
            return Some(0);
        };
        match token {
            Token::Number(n) if is_signed(n) => {
                self.pos += 1;
                Some(saturate(n.value))
            }
            Token::Delim(sign @ ('+' | '-')) => {
                self.pos += 1;
                self.skip_whitespace();
                match self.next()? {
                    Token::Number(n) if is_signless(n) => Some(if *sign == '-' {
                        saturate(-n.value)
                    } else {
                        saturate(n.value)
                    }),
                    _ => None,
                }
            }
            // Leave anything else for the caller's end-of-input check.
            _ => Some(0),
        }
    }
}
