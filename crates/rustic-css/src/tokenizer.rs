//! CSS Syntax Level 3 §4 tokenization over the caller's UTF-8 text.
//!
//! Input preprocessing (§3.3) is applied on the fly so spans stay byte
//! offsets into the original input: CRLF, CR and FF read as one LF, and NUL
//! reads as U+FFFD. Every loop advances the cursor, and no step recurses.

use rustic_base::{CancellationToken, SourceSpan};

use crate::{CssError, Diagnostic, DiagnosticKind, Numeric, Token};

/// A token with its byte span in the original input.
#[derive(Clone, Debug, PartialEq)]
pub struct SpannedToken {
    /// The token.
    pub token: Token,
    /// Its byte range in the input, excluding preceding comments.
    pub span: SourceSpan,
}

/// Bounded recoverable diagnostics: the first `limit` are kept and the rest counted.
#[derive(Debug, Default)]
pub(crate) struct Diagnostics {
    pub(crate) kept: Vec<Diagnostic>,
    pub(crate) dropped: usize,
    pub(crate) limit: usize,
}

impl Diagnostics {
    pub(crate) fn push(&mut self, kind: DiagnosticKind, start: usize, end: usize) {
        if self.kept.len() < self.limit {
            let span = SourceSpan::new(start, end.max(start)).expect("ordered span");
            self.kept.push(Diagnostic { kind, span });
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || !c.is_ascii()
}

fn is_ident_char(c: char) -> bool {
    is_ident_start(c) || c.is_ascii_digit() || c == '-'
}

fn is_whitespace(c: char) -> bool {
    matches!(c, '\n' | '\t' | ' ')
}

fn is_non_printable(c: char) -> bool {
    matches!(c, '\0'..='\u{8}' | '\u{b}' | '\u{e}'..='\u{1f}' | '\u{7f}')
}

fn valid_escape(a: Option<char>, b: Option<char>) -> bool {
    a == Some('\\') && b != Some('\n')
}

fn starts_ident(a: Option<char>, b: Option<char>, c: Option<char>) -> bool {
    match a {
        Some('-') => b.is_some_and(|b| is_ident_start(b) || b == '-') || valid_escape(b, c),
        Some('\\') => valid_escape(a, b),
        Some(a) => is_ident_start(a),
        None => false,
    }
}

fn starts_number(a: Option<char>, b: Option<char>, c: Option<char>) -> bool {
    let digit = |x: Option<char>| x.is_some_and(|x| x.is_ascii_digit());
    match a {
        Some('+' | '-') => digit(b) || (b == Some('.') && digit(c)),
        Some('.') => digit(b),
        a => digit(a),
    }
}

/// Decode one preprocessed code point at `pos`: `(code point, source bytes)`.
fn decode(source: &str, pos: usize) -> Option<(char, usize)> {
    let rest = source.get(pos..)?;
    let c = rest.chars().next()?;
    Some(match c {
        '\r' if rest.as_bytes().get(1) == Some(&b'\n') => ('\n', 2),
        '\r' | '\u{c}' => ('\n', 1),
        '\0' => ('\u{fffd}', 1),
        c => (c, c.len_utf8()),
    })
}

pub(crate) struct Tokenizer<'a> {
    source: &'a str,
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    pub(crate) fn new(source: &'a str) -> Self {
        Self { source, pos: 0 }
    }

    /// The `n`th upcoming preprocessed code point (`n < 4`).
    fn peek(&self, n: usize) -> Option<char> {
        let mut pos = self.pos;
        for _ in 0..n {
            pos += decode(self.source, pos)?.1;
        }
        decode(self.source, pos).map(|(c, _)| c)
    }

    fn bump(&mut self) -> Option<char> {
        let (c, len) = decode(self.source, self.pos)?;
        self.pos += len;
        Some(c)
    }

    fn starts_with(&self, text: &str) -> bool {
        self.source[self.pos..].starts_with(text)
    }

    fn skip_comments(&mut self, diagnostics: &mut Diagnostics) {
        while self.starts_with("/*") {
            let start = self.pos;
            match self.source[self.pos + 2..].find("*/") {
                Some(end) => self.pos += 2 + end + 2,
                None => {
                    self.pos = self.source.len();
                    diagnostics.push(DiagnosticKind::EofInComment, start, self.pos);
                }
            }
        }
    }

    /// Consume the next token, or `None` at EOF.
    pub(crate) fn next(&mut self, diagnostics: &mut Diagnostics) -> Option<SpannedToken> {
        self.skip_comments(diagnostics);
        let start = self.pos;
        let c = self.peek(0)?;
        let (p1, p2) = (self.peek(1), self.peek(2));
        let token = match c {
            c if is_whitespace(c) => {
                while self.peek(0).is_some_and(is_whitespace) {
                    self.bump();
                }
                Token::Whitespace
            }
            '"' | '\'' => {
                self.bump();
                self.string(c, start, diagnostics)
            }
            '#' if p1.is_some_and(is_ident_char) || valid_escape(p1, p2) => {
                self.bump();
                let id = starts_ident(self.peek(0), self.peek(1), self.peek(2));
                let value = self.ident_sequence(diagnostics);
                Token::Hash { value, id }
            }
            '+' | '.' if starts_number(Some(c), p1, p2) => self.numeric(diagnostics),
            '-' if starts_number(Some(c), p1, p2) => self.numeric(diagnostics),
            '-' if p1 == Some('-') && p2 == Some('>') => {
                self.pos += 3;
                Token::Cdc
            }
            '-' if starts_ident(Some(c), p1, p2) => self.ident_like(start, diagnostics),
            '<' if self.starts_with("<!--") => {
                self.pos += 4;
                Token::Cdo
            }
            '@' if starts_ident(p1, p2, self.peek(3)) => {
                self.bump();
                Token::AtKeyword(self.ident_sequence(diagnostics))
            }
            '\\' if valid_escape(Some(c), p1) => self.ident_like(start, diagnostics),
            '\\' => {
                self.bump();
                diagnostics.push(DiagnosticKind::InvalidEscape, start, self.pos);
                Token::Delim('\\')
            }
            c if c.is_ascii_digit() => self.numeric(diagnostics),
            c if is_ident_start(c) => self.ident_like(start, diagnostics),
            c => {
                self.bump();
                match c {
                    '(' => Token::OpenParen,
                    ')' => Token::CloseParen,
                    ',' => Token::Comma,
                    ':' => Token::Colon,
                    ';' => Token::Semicolon,
                    '[' => Token::OpenSquare,
                    ']' => Token::CloseSquare,
                    '{' => Token::OpenCurly,
                    '}' => Token::CloseCurly,
                    c => Token::Delim(c),
                }
            }
        };
        let span = SourceSpan::new(start, self.pos).expect("cursor only advances");
        Some(SpannedToken { token, span })
    }

    /// Consume an escape after its backslash (§4.3.7).
    fn escape(&mut self, diagnostics: &mut Diagnostics) -> char {
        let start = self.pos.saturating_sub(1);
        match self.bump() {
            None => {
                diagnostics.push(DiagnosticKind::EofInEscape, start, self.pos);
                '\u{fffd}'
            }
            Some(c) if c.is_ascii_hexdigit() => {
                let mut value = c.to_digit(16).expect("hex digit");
                for _ in 0..5 {
                    match self.peek(0).and_then(|c| c.to_digit(16)) {
                        Some(digit) => {
                            self.bump();
                            value = value * 16 + digit;
                        }
                        None => break,
                    }
                }
                if self.peek(0).is_some_and(is_whitespace) {
                    self.bump();
                }
                match char::from_u32(value) {
                    Some(c) if value != 0 => c,
                    _ => '\u{fffd}',
                }
            }
            Some(c) => c,
        }
    }

    fn ident_sequence(&mut self, diagnostics: &mut Diagnostics) -> String {
        let mut value = String::new();
        loop {
            match self.peek(0) {
                Some(c) if is_ident_char(c) => {
                    self.bump();
                    value.push(c);
                }
                c if valid_escape(c, self.peek(1)) => {
                    self.bump();
                    value.push(self.escape(diagnostics));
                }
                _ => return value,
            }
        }
    }

    fn number(&mut self) -> Numeric {
        let start = self.pos;
        let digits = |t: &mut Self| {
            while t.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                t.bump();
            }
        };
        let mut integer = true;
        if matches!(self.peek(0), Some('+' | '-')) {
            self.bump();
        }
        digits(self);
        if self.peek(0) == Some('.') && self.peek(1).is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
            integer = false;
            digits(self);
        }
        let exponent_sign = matches!(self.peek(1), Some('+' | '-'));
        let exponent_digit = self.peek(if exponent_sign { 2 } else { 1 });
        if matches!(self.peek(0), Some('e' | 'E'))
            && exponent_digit.is_some_and(|c| c.is_ascii_digit())
        {
            self.bump();
            if exponent_sign {
                self.bump();
            }
            integer = false;
            digits(self);
        }
        // Numbers contain only ASCII, so the source slice is the representation.
        let repr = self.source[start..self.pos].to_owned();
        let value: f64 = repr.parse().expect("CSS number grammar is a Rust float");
        let value = value.clamp(-f64::MAX, f64::MAX);
        Numeric {
            value,
            integer,
            repr,
        }
    }

    fn numeric(&mut self, diagnostics: &mut Diagnostics) -> Token {
        let number = self.number();
        if starts_ident(self.peek(0), self.peek(1), self.peek(2)) {
            let unit = self.ident_sequence(diagnostics);
            Token::Dimension(number, unit)
        } else if self.peek(0) == Some('%') {
            self.bump();
            Token::Percentage(number)
        } else {
            Token::Number(number)
        }
    }

    fn ident_like(&mut self, start: usize, diagnostics: &mut Diagnostics) -> Token {
        let name = self.ident_sequence(diagnostics);
        if self.peek(0) != Some('(') {
            return Token::Ident(name);
        }
        self.bump();
        if name.eq_ignore_ascii_case("url") {
            while self.peek(0).is_some_and(is_whitespace) && self.peek(1).is_some_and(is_whitespace)
            {
                self.bump();
            }
            let quote = |c: Option<char>| matches!(c, Some('"' | '\''));
            let next = self.peek(0);
            if !(quote(next) || (next.is_some_and(is_whitespace) && quote(self.peek(1)))) {
                return self.url(start, diagnostics);
            }
        }
        Token::Function(name)
    }

    fn string(&mut self, ending: char, start: usize, diagnostics: &mut Diagnostics) -> Token {
        let mut value = String::new();
        loop {
            match self.peek(0) {
                None => {
                    diagnostics.push(DiagnosticKind::EofInString, start, self.pos);
                    return Token::String(value);
                }
                Some(c) if c == ending => {
                    self.bump();
                    return Token::String(value);
                }
                Some('\n') => {
                    diagnostics.push(DiagnosticKind::NewlineInString, start, self.pos);
                    return Token::BadString;
                }
                Some('\\') => {
                    self.bump();
                    match self.peek(0) {
                        None => {}
                        Some('\n') => {
                            self.bump();
                        }
                        Some(_) => value.push(self.escape(diagnostics)),
                    }
                }
                Some(c) => {
                    self.bump();
                    value.push(c);
                }
            }
        }
    }

    fn url(&mut self, start: usize, diagnostics: &mut Diagnostics) -> Token {
        let mut value = String::new();
        while self.peek(0).is_some_and(is_whitespace) {
            self.bump();
        }
        loop {
            match self.bump() {
                Some(')') => return Token::Url(value),
                None => {
                    diagnostics.push(DiagnosticKind::EofInUrl, start, self.pos);
                    return Token::Url(value);
                }
                Some(c) if is_whitespace(c) => {
                    while self.peek(0).is_some_and(is_whitespace) {
                        self.bump();
                    }
                    match self.peek(0) {
                        Some(')') => {
                            self.bump();
                            return Token::Url(value);
                        }
                        None => {
                            diagnostics.push(DiagnosticKind::EofInUrl, start, self.pos);
                            return Token::Url(value);
                        }
                        Some(_) => return self.bad_url(start, diagnostics),
                    }
                }
                Some('"' | '\'' | '(') => return self.bad_url(start, diagnostics),
                Some(c) if is_non_printable(c) => return self.bad_url(start, diagnostics),
                Some('\\') => {
                    if valid_escape(Some('\\'), self.peek(0)) {
                        value.push(self.escape(diagnostics));
                    } else {
                        return self.bad_url(start, diagnostics);
                    }
                }
                Some(c) => value.push(c),
            }
        }
    }

    /// Consume the remnants of a bad URL (§4.3.14).
    fn bad_url(&mut self, start: usize, diagnostics: &mut Diagnostics) -> Token {
        loop {
            match self.peek(0) {
                None => break,
                Some(')') => {
                    self.bump();
                    break;
                }
                c if valid_escape(c, self.peek(1)) => {
                    self.bump();
                    self.escape(diagnostics);
                }
                Some(_) => {
                    self.bump();
                }
            }
        }
        diagnostics.push(DiagnosticKind::BadUrl, start, self.pos);
        Token::BadUrl
    }
}

/// Tokenize all of `source`, enforcing the token limit and cancellation.
pub(crate) fn tokenize(
    source: &str,
    max_tokens: usize,
    cancellation: &CancellationToken,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<SpannedToken>, CssError> {
    let mut tokenizer = Tokenizer::new(source);
    let mut tokens = Vec::new();
    while let Some(token) = tokenizer.next(diagnostics) {
        if tokens.len() == max_tokens {
            return Err(CssError::TokenLimit);
        }
        if tokens.len() % 1024 == 0 {
            cancellation.check().map_err(|_| CssError::Cancelled)?;
        }
        tokens
            .try_reserve(1)
            .map_err(|_| CssError::AllocationFailed)?;
        tokens.push(token);
    }
    Ok(tokens)
}
