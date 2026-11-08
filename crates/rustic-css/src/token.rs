//! CSS tokens as defined by CSS Syntax Level 3 §4.

/// A numeric token payload.
#[derive(Clone, Debug, PartialEq)]
pub struct Numeric {
    /// The parsed value. Always finite: out-of-range literals saturate to
    /// `±f64::MAX` instead of becoming infinite.
    pub value: f64,
    /// `true` for the "integer" type flag (no fraction or exponent in source).
    pub integer: bool,
    /// The number's source text, such as `+1.5e3`, kept for serialization.
    pub repr: String,
}

/// A token produced by the CSS tokenizer.
///
/// Names and string values are already unescaped. Comments are consumed and
/// never produce tokens; there is no separate unicode-range token (CSS Syntax
/// parses `U+` ranges from ordinary tokens where a grammar asks for them).
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    /// `<ident-token>`.
    Ident(String),
    /// `<function-token>`: a name immediately followed by `(`.
    Function(String),
    /// `<at-keyword-token>` without the `@`.
    AtKeyword(String),
    /// `<hash-token>`; `id` is the "id" type flag (the value would start an ident).
    Hash {
        /// Unescaped value without the `#`.
        value: String,
        /// Whether the value is a valid identifier start.
        id: bool,
    },
    /// `<string-token>`, unescaped and without quotes.
    String(String),
    /// `<bad-string-token>`: an unescaped newline ended a string.
    BadString,
    /// `<url-token>` from an unquoted `url(...)`.
    Url(String),
    /// `<bad-url-token>`.
    BadUrl,
    /// `<delim-token>`: any other single code point.
    Delim(char),
    /// `<number-token>`.
    Number(Numeric),
    /// `<percentage-token>`; the value is not divided by 100.
    Percentage(Numeric),
    /// `<dimension-token>` with its unescaped unit.
    Dimension(Numeric, String),
    /// `<whitespace-token>`: one or more whitespace code points.
    Whitespace,
    /// `<CDO-token>`, `<!--`.
    Cdo,
    /// `<CDC-token>`, `-->`.
    Cdc,
    /// `:`.
    Colon,
    /// `;`.
    Semicolon,
    /// `,`.
    Comma,
    /// `[`.
    OpenSquare,
    /// `]`.
    CloseSquare,
    /// `(`.
    OpenParen,
    /// `)`.
    CloseParen,
    /// `{`.
    OpenCurly,
    /// `}`.
    CloseCurly,
}

impl Token {
    /// Whether this is a `<whitespace-token>`.
    pub fn is_whitespace(&self) -> bool {
        matches!(self, Self::Whitespace)
    }
}
