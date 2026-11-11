//! The ECMAScript lexical grammar (ECMA-262 §12) over UTF-16 source.
//!
//! The lexer is a cursor the parser drives one token at a time, because
//! the grammar has several goal symbols: a `/` starts a regular expression
//! or is a division depending on the syntactic context, and a `}` closes a
//! block or continues a template. Scanning is iterative and every loop
//! advances, so time is linear in the source length.

use std::borrow::Cow;

use crate::error::SyntaxError;
use crate::source::Span;
use crate::token::{Punct, TemplatePart, Token, TokenKind};
use crate::unicode::{is_identifier_part, is_identifier_start, is_line_terminator, is_whitespace};

/// Which token a `/` starts (ECMA-262 §12 `InputElementDiv` versus
/// `InputElementRegExp`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goal {
    /// `/` and `/=` are division punctuators.
    Div,
    /// `/` starts a regular expression literal.
    RegExp,
}

/// A position in the source that the parser can save and restore to look
/// ahead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pos: usize,
    line_start: bool,
}

/// Scans tokens from UTF-16 source text.
#[derive(Clone, Debug)]
pub struct Lexer<'a> {
    src: &'a [u16],
    pos: usize,
    /// No token has been scanned on the current line yet (for `-->`).
    line_start: bool,
    /// Annex B HTML-like comments (`<!--`, `-->`) are recognized; they are
    /// disabled for module source.
    html_comments: bool,
}

type LexResult<T> = Result<T, SyntaxError>;

fn err<T>(start: usize, end: usize, message: impl Into<Cow<'static, str>>) -> LexResult<T> {
    Err(SyntaxError::new(Span::new(off(start), off(end)), message))
}

/// Offsets fit in `u32` because [`crate::Source`] bounds source length.
fn off(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

fn hex_value(unit: u16) -> Option<u32> {
    char::from_u32(u32::from(unit))?.to_digit(16)
}

impl<'a> Lexer<'a> {
    /// A lexer at the start of `src`. A leading hashbang comment (`#!`) is
    /// skipped (ECMA-262 §12.5).
    pub fn new(src: &'a [u16], html_comments: bool) -> Self {
        let mut lexer = Self {
            src,
            pos: 0,
            line_start: true,
            html_comments,
        };
        if src.starts_with(&[0x23, 0x21]) {
            lexer.pos = 2;
            while lexer.pos < src.len() && !is_line_terminator(u32::from(src[lexer.pos])) {
                lexer.pos += 1;
            }
        }
        lexer
    }

    /// The current position, to [`Self::restore`] after looking ahead.
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            pos: self.pos,
            line_start: self.line_start,
        }
    }

    /// Return to a saved position.
    pub fn restore(&mut self, checkpoint: Checkpoint) {
        self.pos = checkpoint.pos;
        self.line_start = checkpoint.line_start;
    }

    /// Continue scanning at code unit `offset` (to rescan a token with
    /// another goal).
    pub(crate) fn restore_to(&mut self, offset: u32) {
        self.pos = (offset as usize).min(self.src.len());
        self.line_start = false;
    }

    fn unit(&self, i: usize) -> Option<u16> {
        self.src.get(i).copied()
    }

    fn peek_unit(&self) -> Option<u16> {
        self.unit(self.pos)
    }

    /// The code point at `i` and its length in code units (a lone
    /// surrogate is its own code point).
    fn code_point(&self, i: usize) -> Option<(u32, usize)> {
        let first = self.unit(i)?;
        if (0xD800..0xDC00).contains(&first) {
            if let Some(second @ 0xDC00..=0xDFFF) = self.unit(i + 1) {
                let cp =
                    0x10000 + ((u32::from(first) - 0xD800) << 10) + (u32::from(second) - 0xDC00);
                return Some((cp, 2));
            }
        }
        Some((u32::from(first), 1))
    }

    fn eat(&mut self, unit: u8) -> bool {
        if self.peek_unit() == Some(u16::from(unit)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Skip whitespace and comments; returns whether a line terminator was
    /// crossed.
    fn skip_trivia(&mut self) -> LexResult<bool> {
        let mut newline = false;
        loop {
            let Some((cp, len)) = self.code_point(self.pos) else {
                return Ok(newline);
            };
            if is_whitespace(cp) {
                self.pos += len;
            } else if is_line_terminator(cp) {
                self.pos += len;
                newline = true;
                self.line_start = true;
            } else if cp == u32::from(b'/') && self.unit(self.pos + 1) == Some(u16::from(b'/')) {
                self.skip_line_comment();
            } else if cp == u32::from(b'/') && self.unit(self.pos + 1) == Some(u16::from(b'*')) {
                if self.skip_block_comment()? {
                    newline = true;
                    self.line_start = true;
                }
            } else if self.html_comments
                && self.src[self.pos..].starts_with(&[0x3C, 0x21, 0x2D, 0x2D])
            {
                // Annex B.1.1 SingleLineHTMLOpenComment: `<!--`.
                self.skip_line_comment();
            } else if self.html_comments
                && self.line_start
                && self.src[self.pos..].starts_with(&[0x2D, 0x2D, 0x3E])
            {
                // Annex B.1.1 HTMLCloseComment: `-->` first on a line (only
                // whitespace or comments before it).
                self.skip_line_comment();
            } else {
                return Ok(newline);
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while let Some(unit) = self.peek_unit() {
            if is_line_terminator(u32::from(unit)) {
                break;
            }
            self.pos += 1;
        }
    }

    /// Skip `/* ... */`; returns whether it contained a line terminator.
    fn skip_block_comment(&mut self) -> LexResult<bool> {
        let start = self.pos;
        self.pos += 2;
        let mut newline = false;
        loop {
            match self.peek_unit() {
                None => return err(start, self.pos, "unterminated comment"),
                Some(0x2A) if self.unit(self.pos + 1) == Some(0x2F) => {
                    self.pos += 2;
                    return Ok(newline);
                }
                Some(unit) => {
                    newline |= is_line_terminator(u32::from(unit));
                    self.pos += 1;
                }
            }
        }
    }

    /// Scan the next token with the given goal.
    pub fn next_token(&mut self, goal: Goal) -> LexResult<Token> {
        let newline_before = self.skip_trivia()?;
        let start = self.pos;
        let kind = self.scan(goal)?;
        self.line_start = false;
        Ok(Token {
            kind,
            span: Span::new(off(start), off(self.pos)),
            newline_before,
        })
    }

    fn scan(&mut self, goal: Goal) -> LexResult<TokenKind> {
        let start = self.pos;
        let Some((cp, len)) = self.code_point(self.pos) else {
            return Ok(TokenKind::Eof);
        };
        if is_identifier_start(cp) || cp == u32::from(b'\\') {
            let (name, escaped) = self.identifier_name()?;
            return Ok(TokenKind::Name {
                name: name.into_boxed_str(),
                escaped,
            });
        }
        let Ok(ascii) = u8::try_from(cp) else {
            return err(start, start + len, "unexpected character");
        };
        match ascii {
            b'0'..=b'9' => self.numeric_literal(),
            b'.' if self
                .unit(self.pos + 1)
                .is_some_and(|u| (0x30..=0x39).contains(&u)) =>
            {
                self.numeric_literal()
            }
            b'"' | b'\'' => self.string_literal(),
            b'`' => {
                self.pos += 1;
                self.template_part(start).map(TokenKind::Template)
            }
            b'#' => {
                self.pos += 1;
                match self.code_point(self.pos) {
                    Some((cp, _)) if is_identifier_start(cp) || cp == u32::from(b'\\') => {
                        let (name, _) = self.identifier_name()?;
                        Ok(TokenKind::PrivateName(name.into_boxed_str()))
                    }
                    _ => err(start, self.pos, "`#` must start a private name"),
                }
            }
            b'/' if goal == Goal::RegExp => self.regexp_literal(),
            _ => self.punctuator().map(TokenKind::Punct),
        }
    }

    fn punctuator(&mut self) -> LexResult<Punct> {
        use Punct as P;
        let start = self.pos;
        let rest = &self.src[self.pos..];
        let at = |i: usize| rest.get(i).copied().unwrap_or(0);
        // Non-ASCII units map to NUL, which no punctuator contains.
        let ch = |i: usize| u8::try_from(at(i)).map_or('\0', char::from);
        let (punct, len) = match (ch(0), ch(1)) {
            ('{', _) => (P::LBrace, 1),
            ('}', _) => (P::RBrace, 1),
            ('(', _) => (P::LParen, 1),
            (')', _) => (P::RParen, 1),
            ('[', _) => (P::LBracket, 1),
            (']', _) => (P::RBracket, 1),
            (';', _) => (P::Semicolon, 1),
            (',', _) => (P::Comma, 1),
            (':', _) => (P::Colon, 1),
            ('~', _) => (P::Tilde, 1),
            ('.', '.') if at(2) == 0x2E => (P::Ellipsis, 3),
            ('.', _) => (P::Dot, 1),
            ('<', '<') if at(2) == 0x3D => (P::ShlEq, 3),
            ('<', '<') => (P::Shl, 2),
            ('<', '=') => (P::LtEq, 2),
            ('<', _) => (P::Lt, 1),
            ('>', '>') if at(2) == 0x3E && at(3) == 0x3D => (P::ShrEq, 4),
            ('>', '>') if at(2) == 0x3E => (P::Shr, 3),
            ('>', '>') if at(2) == 0x3D => (P::SarEq, 3),
            ('>', '>') => (P::Sar, 2),
            ('>', '=') => (P::GtEq, 2),
            ('>', _) => (P::Gt, 1),
            ('=', '=') if at(2) == 0x3D => (P::EqEqEq, 3),
            ('=', '=') => (P::EqEq, 2),
            ('=', '>') => (P::Arrow, 2),
            ('=', _) => (P::Eq, 1),
            ('!', '=') if at(2) == 0x3D => (P::NotEqEq, 3),
            ('!', '=') => (P::NotEq, 2),
            ('!', _) => (P::Bang, 1),
            ('+', '+') => (P::PlusPlus, 2),
            ('+', '=') => (P::PlusEq, 2),
            ('+', _) => (P::Plus, 1),
            ('-', '-') => (P::MinusMinus, 2),
            ('-', '=') => (P::MinusEq, 2),
            ('-', _) => (P::Minus, 1),
            ('*', '*') if at(2) == 0x3D => (P::StarStarEq, 3),
            ('*', '*') => (P::StarStar, 2),
            ('*', '=') => (P::StarEq, 2),
            ('*', _) => (P::Star, 1),
            ('%', '=') => (P::PercentEq, 2),
            ('%', _) => (P::Percent, 1),
            ('&', '&') if at(2) == 0x3D => (P::AmpAmpEq, 3),
            ('&', '&') => (P::AmpAmp, 2),
            ('&', '=') => (P::AmpEq, 2),
            ('&', _) => (P::Amp, 1),
            ('|', '|') if at(2) == 0x3D => (P::PipePipeEq, 3),
            ('|', '|') => (P::PipePipe, 2),
            ('|', '=') => (P::PipeEq, 2),
            ('|', _) => (P::Pipe, 1),
            ('^', '=') => (P::CaretEq, 2),
            ('^', _) => (P::Caret, 1),
            ('?', '?') if at(2) == 0x3D => (P::QuestionQuestionEq, 3),
            ('?', '?') => (P::QuestionQuestion, 2),
            // `?.` followed by a digit is `?` then a number (`a?.5:b`).
            ('?', '.') if !(0x30..=0x39).contains(&at(2)) => (P::QuestionDot, 2),
            ('?', _) => (P::Question, 1),
            ('/', '=') => (P::SlashEq, 2),
            ('/', _) => (P::Slash, 1),
            _ => return err(start, start + 1, "unexpected character"),
        };
        self.pos += len;
        Ok(punct)
    }

    /// `IdentifierName` with `\u` escapes; returns the decoded name and
    /// whether any escape was used.
    fn identifier_name(&mut self) -> LexResult<(String, bool)> {
        let mut name = String::new();
        let mut escaped = false;
        let mut first = true;
        loop {
            let start = self.pos;
            let Some((cp, len)) = self.code_point(self.pos) else {
                break;
            };
            let valid = |cp| {
                if first {
                    is_identifier_start(cp)
                } else {
                    is_identifier_part(cp)
                }
            };
            let cp = if cp == u32::from(b'\\') {
                if self.unit(self.pos + 1) != Some(u16::from(b'u')) {
                    return err(start, self.pos + 1, "invalid escape in identifier");
                }
                self.pos += 2;
                let cp = self.unicode_escape_body(start)?;
                if !valid(cp) {
                    return err(start, self.pos, "escape is not an identifier character");
                }
                escaped = true;
                cp
            } else if valid(cp) {
                self.pos += len;
                cp
            } else {
                break;
            };
            // Identifier code points are never surrogates.
            let Some(c) = char::from_u32(cp) else {
                return err(start, self.pos, "invalid identifier character");
            };
            name.push(c);
            first = false;
        }
        Ok((name, escaped))
    }

    /// After `\u`: `XXXX` or `{X...}` up to U+10FFFF.
    fn unicode_escape_body(&mut self, start: usize) -> LexResult<u32> {
        if self.eat(b'{') {
            let mut value: u32 = 0;
            let mut digits = 0;
            while let Some(d) = self.peek_unit().and_then(hex_value) {
                self.pos += 1;
                digits += 1;
                value = value.saturating_mul(16).saturating_add(d);
                if value > 0x10FFFF {
                    return err(start, self.pos, "code point escape above U+10FFFF");
                }
            }
            if digits == 0 || !self.eat(b'}') {
                return err(start, self.pos, "malformed code point escape");
            }
            return Ok(value);
        }
        let mut value = 0;
        for _ in 0..4 {
            let Some(d) = self.peek_unit().and_then(hex_value) else {
                return err(start, self.pos, "malformed unicode escape");
            };
            self.pos += 1;
            value = value * 16 + d;
        }
        Ok(value)
    }

    fn numeric_literal(&mut self) -> LexResult<TokenKind> {
        let start = self.pos;
        let first = self.peek_unit();
        let second = self.unit(self.pos + 1).map(|u| u | 0x20);
        let kind = if first == Some(u16::from(b'0')) && matches!(second, Some(0x78 | 0x6F | 0x62)) {
            // 0x / 0o / 0b (case-insensitive).
            let radix = match second {
                Some(0x78) => 16,
                Some(0x6F) => 8,
                _ => 2,
            };
            self.pos += 2;
            let digits = self.digits(radix, true)?;
            if digits.is_empty() {
                return err(start, self.pos, "missing digits after radix prefix");
            }
            if self.eat(b'n') {
                TokenKind::BigInt {
                    radix,
                    digits: digits.into_boxed_str(),
                }
            } else {
                TokenKind::Number {
                    value: radix_to_f64(&digits, radix),
                    legacy: false,
                }
            }
        } else if first == Some(u16::from(b'0'))
            && self
                .unit(self.pos + 1)
                .is_some_and(|u| (0x30..=0x39).contains(&u))
        {
            // Annex B legacy octal (`017`) or non-octal decimal (`019`, `08.5`).
            self.pos += 1;
            let digits = self.digits(10, false)?;
            if digits.bytes().all(|b| b < b'8') {
                TokenKind::Number {
                    value: radix_to_f64(&digits, 8),
                    legacy: true,
                }
            } else {
                let mut text = String::from("0");
                text.push_str(&digits);
                self.decimal_tail(start, &mut text)?;
                TokenKind::Number {
                    value: parse_decimal(&text),
                    legacy: true,
                }
            }
        } else {
            if first == Some(u16::from(b'0')) && self.unit(self.pos + 1) == Some(u16::from(b'_')) {
                return err(
                    start,
                    self.pos + 2,
                    "numeric separator after a leading zero",
                );
            }
            let mut text = self.digits(10, true)?;
            let integer = text.clone();
            let fractional = self.decimal_tail(start, &mut text)?;
            if !fractional && self.peek_unit() == Some(u16::from(b'n')) {
                if integer.len() > 1 && integer.starts_with('0') {
                    return err(start, self.pos + 1, "BigInt literal with a leading zero");
                }
                self.pos += 1;
                TokenKind::BigInt {
                    radix: 10,
                    digits: integer.into_boxed_str(),
                }
            } else {
                TokenKind::Number {
                    value: parse_decimal(&text),
                    legacy: false,
                }
            }
        };
        // §12.9.3: the source character after a numeric literal must not be
        // an IdentifierStart or DecimalDigit.
        if let Some((cp, len)) = self.code_point(self.pos) {
            if is_identifier_start(cp) || (0x30..=0x39).contains(&cp) || cp == u32::from(b'\\') {
                return err(
                    start,
                    self.pos + len,
                    "identifier starts right after a number",
                );
            }
        }
        Ok(kind)
    }

    /// The fraction and exponent of a decimal literal, appended to `text`;
    /// returns whether either was present.
    fn decimal_tail(&mut self, start: usize, text: &mut String) -> LexResult<bool> {
        let mut fractional = false;
        if self.eat(b'.') {
            fractional = true;
            text.push('.');
            if self.peek_unit() == Some(u16::from(b'_')) {
                return err(start, self.pos + 1, "numeric separator after `.`");
            }
            text.push_str(&self.digits(10, true)?);
        }
        if matches!(self.peek_unit(), Some(0x65 | 0x45)) {
            fractional = true;
            self.pos += 1;
            text.push('e');
            if let Some(sign @ (0x2B | 0x2D)) = self.peek_unit() {
                self.pos += 1;
                text.push(if sign == 0x2D { '-' } else { '+' });
            }
            let exponent = self.digits(10, true)?;
            if exponent.is_empty() {
                return err(start, self.pos, "missing exponent digits");
            }
            text.push_str(&exponent);
        }
        Ok(fractional)
    }

    /// Digits in `radix`, with `_` separators when allowed (each separator
    /// must sit between two digits). Returns the digits without separators.
    fn digits(&mut self, radix: u32, separators: bool) -> LexResult<String> {
        let mut out = String::new();
        let mut after_separator = false;
        while let Some(unit) = self.peek_unit() {
            if unit == u16::from(b'_') {
                if !separators || out.is_empty() || after_separator {
                    return err(self.pos, self.pos + 1, "misplaced numeric separator");
                }
                after_separator = true;
                self.pos += 1;
                continue;
            }
            let Some(digit) = char::from_u32(u32::from(unit)).filter(|c| c.is_digit(radix)) else {
                break;
            };
            out.push(digit.to_ascii_lowercase());
            after_separator = false;
            self.pos += 1;
        }
        if after_separator {
            return err(self.pos - 1, self.pos, "misplaced numeric separator");
        }
        Ok(out)
    }

    fn string_literal(&mut self) -> LexResult<TokenKind> {
        let start = self.pos;
        let quote = self.src[self.pos];
        self.pos += 1;
        let mut value = Vec::new();
        let mut legacy_escape = false;
        loop {
            let Some(unit) = self.peek_unit() else {
                return err(start, self.pos, "unterminated string literal");
            };
            if unit == quote {
                self.pos += 1;
                return Ok(TokenKind::String {
                    value,
                    legacy_escape,
                });
            }
            match unit {
                // LS and PS are allowed in strings (ES2019); CR and LF are not.
                0x0A | 0x0D => return err(start, self.pos, "unterminated string literal"),
                0x5C => {
                    let escape = self.pos;
                    self.pos += 1;
                    match self.escape(escape, false)? {
                        Escape::Units(units) => value.extend_from_slice(&units),
                        Escape::LineContinuation => {}
                        Escape::Legacy(units) => {
                            legacy_escape = true;
                            value.extend_from_slice(&units);
                        }
                        Escape::Invalid(message) => return err(escape, self.pos, message),
                    }
                }
                _ => {
                    value.push(unit);
                    self.pos += 1;
                }
            }
        }
    }

    /// One escape sequence after `\` in a string (`template == false`) or
    /// template.
    fn escape(&mut self, start: usize, template: bool) -> LexResult<Escape> {
        let Some(unit) = self.peek_unit() else {
            return err(start, self.pos, "unterminated escape");
        };
        self.pos += 1;
        let simple = |u: u8| Ok(Escape::Units(vec![u16::from(u)]));
        match unit {
            0x62 => simple(0x08),
            0x74 => simple(0x09),
            0x6E => simple(0x0A),
            0x76 => simple(0x0B),
            0x66 => simple(0x0C),
            0x72 => simple(0x0D),
            0x0D => {
                self.eat(b'\n');
                Ok(Escape::LineContinuation)
            }
            0x0A | 0x2028 | 0x2029 => Ok(Escape::LineContinuation),
            0x30 if !self.peek_unit().is_some_and(|u| (0x30..=0x39).contains(&u)) => simple(0),
            0x30..=0x39 => {
                if template {
                    return Ok(Escape::Invalid(
                        "octal escapes are not allowed in templates",
                    ));
                }
                if unit >= 0x38 {
                    // NonOctalDecimalEscapeSequence `\8` and `\9`.
                    return Ok(Escape::Legacy(vec![unit]));
                }
                // LegacyOctalEscapeSequence: up to three octal digits, at
                // most \377.
                let mut value = u32::from(unit - 0x30);
                let max_digits = if unit <= 0x33 { 3 } else { 2 };
                for _ in 1..max_digits {
                    match self.peek_unit() {
                        Some(d @ 0x30..=0x37) => {
                            value = value * 8 + u32::from(d - 0x30);
                            self.pos += 1;
                        }
                        _ => break,
                    }
                }
                // At most \377, so the value fits a code unit.
                Ok(Escape::Legacy(vec![value as u16]))
            }
            0x78 => {
                let (Some(hi), Some(lo)) = (
                    self.peek_unit().and_then(hex_value),
                    self.unit(self.pos + 1).and_then(hex_value),
                ) else {
                    return Ok(Escape::Invalid("malformed hexadecimal escape"));
                };
                self.pos += 2;
                Ok(Escape::Units(vec![(hi * 16 + lo) as u16]))
            }
            0x75 => {
                let saved = self.pos;
                match self.unicode_escape_body(start) {
                    Ok(cp) => Ok(Escape::Units(code_point_units(cp))),
                    Err(e) if template => {
                        self.pos = saved;
                        Ok(Escape::Invalid(match e.message {
                            Cow::Borrowed(m) => m,
                            Cow::Owned(_) => "malformed unicode escape",
                        }))
                    }
                    Err(e) => Err(e),
                }
            }
            other => {
                // NonEscapeCharacter: the code point itself (a surrogate
                // pair stays together).
                let mut units = vec![other];
                if (0xD800..0xDC00).contains(&other) {
                    if let Some(next @ 0xDC00..=0xDFFF) = self.peek_unit() {
                        units.push(next);
                        self.pos += 1;
                    }
                }
                Ok(Escape::Units(units))
            }
        }
    }

    /// A template piece after `` ` `` or `}`; `start` is that character.
    fn template_part(&mut self, start: usize) -> LexResult<TemplatePart> {
        let mut cooked = Some(Vec::new());
        let mut raw = Vec::new();
        loop {
            let Some(unit) = self.peek_unit() else {
                return err(start, self.pos, "unterminated template literal");
            };
            match unit {
                0x60 => {
                    self.pos += 1;
                    return Ok(TemplatePart {
                        cooked,
                        raw,
                        tail: true,
                    });
                }
                0x24 if self.unit(self.pos + 1) == Some(0x7B) => {
                    self.pos += 2;
                    return Ok(TemplatePart {
                        cooked,
                        raw,
                        tail: false,
                    });
                }
                0x5C => {
                    let escape = self.pos;
                    self.pos += 1;
                    let result = self.escape(escape, true)?;
                    // TRV: the escape's source text, with line terminators
                    // normalized.
                    push_raw(&mut raw, &self.src[escape..self.pos]);
                    match (result, cooked.as_mut()) {
                        (Escape::Units(units), Some(c)) => c.extend_from_slice(&units),
                        (Escape::LineContinuation, _) | (_, None) => {}
                        (Escape::Legacy(_) | Escape::Invalid(_), _) => cooked = None,
                    }
                }
                0x0D => {
                    // CR LF and CR are LF in both TV and TRV.
                    self.pos += 1;
                    self.eat(b'\n');
                    raw.push(0x0A);
                    if let Some(c) = cooked.as_mut() {
                        c.push(0x0A);
                    }
                }
                _ => {
                    self.pos += 1;
                    raw.push(unit);
                    if let Some(c) = cooked.as_mut() {
                        c.push(unit);
                    }
                }
            }
        }
    }

    /// Continue a template after the `}` that closes a substitution: the
    /// parser calls this with the lexer positioned just past that `}`
    /// (`brace` is its span).
    pub fn template_continuation(&mut self, brace: Span) -> LexResult<Token> {
        let start = brace.start as usize;
        self.pos = brace.end as usize;
        let part = self.template_part(start)?;
        self.line_start = false;
        Ok(Token {
            kind: TokenKind::Template(part),
            span: Span::new(brace.start, off(self.pos)),
            newline_before: false,
        })
    }

    fn regexp_literal(&mut self) -> LexResult<TokenKind> {
        let start = self.pos;
        self.pos += 1;
        let mut in_class = false;
        loop {
            let Some(unit) = self.peek_unit() else {
                return err(start, self.pos, "unterminated regular expression");
            };
            if is_line_terminator(u32::from(unit)) {
                return err(start, self.pos, "unterminated regular expression");
            }
            self.pos += 1;
            match unit {
                0x5C => match self.peek_unit() {
                    Some(next) if !is_line_terminator(u32::from(next)) => self.pos += 1,
                    _ => return err(start, self.pos, "unterminated regular expression"),
                },
                0x5B => in_class = true,
                0x5D => in_class = false,
                0x2F if !in_class => break,
                _ => {}
            }
        }
        let body = self.src[start + 1..self.pos - 1].to_vec();
        if body.first() == Some(&0x2A) {
            return err(start, self.pos, "regular expression body starts with `*`");
        }
        let flags_start = self.pos;
        let mut flags = String::new();
        while let Some((cp, len)) = self.code_point(self.pos) {
            if cp == u32::from(b'\\') {
                return err(self.pos, self.pos + 1, "escape in regular expression flags");
            }
            if !is_identifier_part(cp) {
                break;
            }
            self.pos += len;
            flags.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
        }
        // ECMA-262 §22.2.3.1 early errors: known flags, no duplicates, not
        // both `u` and `v`.
        let mut seen = 0u16;
        for c in flags.chars() {
            let bit = match c {
                'd' => 1,
                'g' => 2,
                'i' => 4,
                'm' => 8,
                's' => 16,
                'u' => 32,
                'v' => 64,
                'y' => 128,
                _ => return err(flags_start, self.pos, "unknown regular expression flag"),
            };
            if seen & bit != 0 {
                return err(flags_start, self.pos, "duplicate regular expression flag");
            }
            seen |= bit;
        }
        if seen & 96 == 96 {
            return err(
                flags_start,
                self.pos,
                "regular expression flags `u` and `v` together",
            );
        }
        Ok(TokenKind::RegExp {
            body,
            flags: flags.into_boxed_str(),
        })
    }
}

enum Escape {
    Units(Vec<u16>),
    LineContinuation,
    /// A legacy octal or non-octal decimal escape (strict-mode error).
    Legacy(Vec<u16>),
    /// Not a valid escape (allowed only as a tagged template's undefined
    /// cooked value).
    Invalid(&'static str),
}

/// TRV of escape source text: CR LF and CR become LF.
fn push_raw(raw: &mut Vec<u16>, text: &[u16]) {
    let mut i = 0;
    while i < text.len() {
        if text[i] == 0x0D {
            raw.push(0x0A);
            if text.get(i + 1) == Some(&0x0A) {
                i += 1;
            }
        } else {
            raw.push(text[i]);
        }
        i += 1;
    }
}

/// UTF-16 encoding of a code point up to U+10FFFF (surrogate code points
/// encode as themselves).
pub(crate) fn code_point_units(cp: u32) -> Vec<u16> {
    if cp < 0x10000 {
        vec![cp as u16]
    } else {
        let v = cp - 0x10000;
        vec![0xD800 + (v >> 10) as u16, 0xDC00 + (v & 0x3FF) as u16]
    }
}

/// A decimal literal's value, correctly rounded (Rust's float parsing is
/// exact for any digit count).
fn parse_decimal(text: &str) -> f64 {
    let text = if text.starts_with('.') {
        Cow::Owned(format!("0{text}"))
    } else {
        Cow::Borrowed(text)
    };
    text.parse().unwrap_or(f64::NAN)
}

/// Digits in a power-of-two radix (or octal for legacy literals), rounded
/// to nearest, ties to even, as ECMA-262 §12.9.3.1 requires.
pub(crate) fn radix_to_f64(digits: &str, radix: u32) -> f64 {
    let bits_per_digit = match radix {
        2 => 1,
        8 => 3,
        16 => 4,
        _ => return parse_decimal(digits),
    };
    // Collect the significant bits (most significant first) without
    // leading zeros.
    let mut bits: Vec<bool> = Vec::new();
    for d in digits.chars().filter_map(|c| c.to_digit(radix)) {
        for shift in (0..bits_per_digit).rev() {
            let bit = (d >> shift) & 1 == 1;
            if bit || !bits.is_empty() {
                bits.push(bit);
            }
        }
    }
    if bits.is_empty() {
        return 0.0;
    }
    if bits.len() <= 53 {
        let mut value = 0u64;
        for &bit in &bits {
            value = (value << 1) | u64::from(bit);
        }
        return value as f64;
    }
    // Keep 53 bits, round on the 54th with the rest as a sticky bit.
    let mut mantissa = 0u64;
    for &bit in &bits[..53] {
        mantissa = (mantissa << 1) | u64::from(bit);
    }
    let round = bits[53];
    let sticky = bits[54..].iter().any(|&b| b);
    if round && (sticky || mantissa & 1 == 1) {
        mantissa += 1;
    }
    let exponent = (bits.len() - 53) as i32;
    if exponent > 1023 {
        return f64::INFINITY;
    }
    // `mantissa` may have become 2^53 after rounding; the multiplication
    // by a power of two is exact either way (or overflows to infinity).
    mantissa as f64 * 2f64.powi(exponent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_of_two_radix_literals_round_to_nearest_even() {
        assert_eq!(radix_to_f64("ff", 16), 255.0);
        // 2^53 + 1 rounds down to 2^53 (tie to even), 2^53 + 3 up.
        assert_eq!(radix_to_f64("20000000000001", 16), 9007199254740992.0);
        assert_eq!(radix_to_f64("20000000000003", 16), 9007199254740996.0);
        // A sticky bit breaks the tie upwards.
        assert_eq!(
            radix_to_f64("200000000000010000000001", 16),
            radix_to_f64("200000000000020000000000", 16)
        );
        assert_eq!(radix_to_f64(&"f".repeat(300), 16), f64::INFINITY);
        assert_eq!(radix_to_f64("777", 8), 511.0);
        assert_eq!(radix_to_f64("0000101", 2), 5.0);
    }
}
