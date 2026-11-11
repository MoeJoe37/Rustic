//! Tokens of ECMAScript source text (ECMA-262 §12).

use crate::source::Span;

/// A punctuator (ECMA-262 §12.8), including `/` and `/=`, which the lexer
/// only produces where a division may appear, and `}`.
#[allow(missing_docs, reason = "each variant is named after its spelling")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Punct {
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Dot,
    Ellipsis,
    Semicolon,
    Comma,
    Lt,
    Gt,
    LtEq,
    GtEq,
    EqEq,
    NotEq,
    EqEqEq,
    NotEqEq,
    Plus,
    Minus,
    Star,
    Percent,
    StarStar,
    PlusPlus,
    MinusMinus,
    Shl,
    Sar,
    Shr,
    Amp,
    Pipe,
    Caret,
    Bang,
    Tilde,
    AmpAmp,
    PipePipe,
    QuestionQuestion,
    Question,
    QuestionDot,
    Colon,
    Eq,
    PlusEq,
    MinusEq,
    StarEq,
    PercentEq,
    StarStarEq,
    ShlEq,
    SarEq,
    ShrEq,
    AmpEq,
    PipeEq,
    CaretEq,
    AmpAmpEq,
    PipePipeEq,
    QuestionQuestionEq,
    Arrow,
    Slash,
    SlashEq,
}

impl Punct {
    /// The punctuator's source spelling.
    pub fn as_str(self) -> &'static str {
        use Punct as P;
        match self {
            P::LBrace => "{",
            P::RBrace => "}",
            P::LParen => "(",
            P::RParen => ")",
            P::LBracket => "[",
            P::RBracket => "]",
            P::Dot => ".",
            P::Ellipsis => "...",
            P::Semicolon => ";",
            P::Comma => ",",
            P::Lt => "<",
            P::Gt => ">",
            P::LtEq => "<=",
            P::GtEq => ">=",
            P::EqEq => "==",
            P::NotEq => "!=",
            P::EqEqEq => "===",
            P::NotEqEq => "!==",
            P::Plus => "+",
            P::Minus => "-",
            P::Star => "*",
            P::Percent => "%",
            P::StarStar => "**",
            P::PlusPlus => "++",
            P::MinusMinus => "--",
            P::Shl => "<<",
            P::Sar => ">>",
            P::Shr => ">>>",
            P::Amp => "&",
            P::Pipe => "|",
            P::Caret => "^",
            P::Bang => "!",
            P::Tilde => "~",
            P::AmpAmp => "&&",
            P::PipePipe => "||",
            P::QuestionQuestion => "??",
            P::Question => "?",
            P::QuestionDot => "?.",
            P::Colon => ":",
            P::Eq => "=",
            P::PlusEq => "+=",
            P::MinusEq => "-=",
            P::StarEq => "*=",
            P::PercentEq => "%=",
            P::StarStarEq => "**=",
            P::ShlEq => "<<=",
            P::SarEq => ">>=",
            P::ShrEq => ">>>=",
            P::AmpEq => "&=",
            P::PipeEq => "|=",
            P::CaretEq => "^=",
            P::AmpAmpEq => "&&=",
            P::PipePipeEq => "||=",
            P::QuestionQuestionEq => "??=",
            P::Arrow => "=>",
            P::Slash => "/",
            P::SlashEq => "/=",
        }
    }
}

/// A template literal piece: the text between backticks and substitutions.
#[derive(Clone, Debug, PartialEq)]
pub struct TemplatePart {
    /// The template value (TV), or `None` when an escape is invalid, which
    /// is only allowed in tagged templates.
    pub cooked: Option<Vec<u16>>,
    /// The template raw value (TRV): source text with CR LF and CR
    /// normalized to LF.
    pub raw: Vec<u16>,
    /// Whether this piece ends the template (`` ` ``) rather than starting
    /// a substitution (`${`).
    pub tail: bool,
}

/// What a token is.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    /// An `IdentifierName`, including reserved words; the parser decides
    /// which names are keywords in context.
    Name {
        /// The name's code points after escapes are decoded.
        name: Box<str>,
        /// Whether the source spelled the name with a `\u` escape (an
        /// escaped reserved word can never be used as a keyword).
        escaped: bool,
    },
    /// A `PrivateIdentifier` such as `#x` (stored without the `#`).
    PrivateName(Box<str>),
    /// A punctuator.
    Punct(Punct),
    /// A Number literal's value.
    Number {
        /// The mathematical value rounded to a Number.
        value: f64,
        /// Legacy octal (`017`) or non-octal decimal (`08`) integer
        /// literals, which strict mode forbids.
        legacy: bool,
    },
    /// A BigInt literal (`123n`), as its radix and digits without
    /// separators or prefix.
    BigInt {
        /// 2, 8, 10 or 16.
        radix: u32,
        /// ASCII digits in that radix.
        digits: Box<str>,
    },
    /// A string literal's value in UTF-16 code units (lone surrogates are
    /// allowed).
    String {
        /// The string value.
        value: Vec<u16>,
        /// The literal used a legacy octal (`\1`) or non-octal decimal (`\8`)
        /// escape, which strict mode forbids.
        legacy_escape: bool,
    },
    /// A template piece starting with `` ` `` or (after a substitution)
    /// `}`.
    Template(TemplatePart),
    /// A regular expression literal. Pattern validation happens when the
    /// RegExp object is created (TD-88).
    RegExp {
        /// The pattern between the slashes, as source code units.
        body: Vec<u16>,
        /// The flags after the closing slash.
        flags: Box<str>,
    },
    /// End of the source text.
    Eof,
}

/// One token with its source span.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    /// What the token is.
    pub kind: TokenKind,
    /// Where it is.
    pub span: Span,
    /// Whether a line terminator (alone or inside a multi-line comment)
    /// came between the previous token and this one, which automatic
    /// semicolon insertion and restricted productions depend on.
    pub newline_before: bool,
}

impl Token {
    /// The punctuator, if this token is one.
    pub fn punct(&self) -> Option<Punct> {
        match self.kind {
            TokenKind::Punct(p) => Some(p),
            _ => None,
        }
    }

    /// Whether this token is the punctuator `p`.
    pub fn is(&self, p: Punct) -> bool {
        self.punct() == Some(p)
    }

    /// The name, if this token is an unescaped or escaped identifier name.
    pub fn name(&self) -> Option<&str> {
        match &self.kind {
            TokenKind::Name { name, .. } => Some(name),
            _ => None,
        }
    }

    /// Whether this token is the unescaped name `word` (a keyword or a
    /// contextual keyword such as `of`, `get` or `async`).
    pub fn is_word(&self, word: &str) -> bool {
        matches!(&self.kind, TokenKind::Name { name, escaped: false } if &**name == word)
    }
}
