//! A recursive-descent parser from ECMAScript source text to [`crate::ast`]
//! (ECMA-262 §13–§16), with early errors, strict mode and automatic
//! semicolon insertion.
//!
//! Arrow parameters and destructuring assignment targets are parsed with
//! the specification's cover grammars: the parser reads an expression, then
//! converts it to a pattern when `=>` or `=` follows, so it never
//! backtracks. Recursion is bounded by [`ParseLimits::max_depth`];
//! left-nested chains are built iteratively.

mod expression;
mod pattern;
mod scope;
mod statement;

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;

use rustic_base::CancellationToken;

use crate::ast::{Program, ProgramKind};
use crate::error::SyntaxError;
use crate::lexer::{Goal, Lexer};
use crate::source::{Source, Span};
use crate::token::{Punct, Token, TokenKind};

use scope::{ClassScope, Scope, ScopeKind};

/// Resource limits for one parse.
#[derive(Clone, Debug)]
pub struct ParseLimits {
    /// Most nested statements, expressions, patterns and functions. Deeper
    /// source fails with [`ParseError::TooDeep`] instead of overflowing the
    /// stack.
    pub max_depth: u32,
    /// Most bytes of call stack the parser may use beyond the frame that
    /// called it, measured at every nesting level. Unoptimized builds use
    /// far more stack per level than optimized ones, so this guard keeps
    /// [`Self::max_depth`] safe in both; exceeding it also fails with
    /// [`ParseError::TooDeep`]. The default suits any thread with at least
    /// 2 MiB of stack, Rust's default for spawned threads.
    pub max_stack_bytes: usize,
    /// Polled between statements.
    pub cancellation: CancellationToken,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            max_depth: 256,
            max_stack_bytes: 1024 * 1024,
            cancellation: CancellationToken::default(),
        }
    }
}

/// Why a parse failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The source is not a valid script or module.
    Syntax(SyntaxError),
    /// Nesting exceeded [`ParseLimits::max_depth`] at this span.
    TooDeep(Span),
    /// The cancellation token was signalled.
    Cancelled,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Syntax(e) => e.fmt(f),
            ParseError::TooDeep(span) => write!(f, "nesting too deep at {span}"),
            ParseError::Cancelled => f.write_str("parse cancelled"),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<SyntaxError> for ParseError {
    fn from(e: SyntaxError) -> Self {
        ParseError::Syntax(e)
    }
}

type PResult<T> = Result<T, ParseError>;

/// Parse `source` as a `Script` (ECMA-262 §16.1). Annex B syntax (HTML-like
/// comments, legacy octal literals, sloppy-mode function declarations in
/// statement positions) is accepted, as web browsers must.
pub fn parse_script(source: &Source, limits: &ParseLimits) -> Result<Program, ParseError> {
    Parser::new(source, limits, false)?.program()
}

/// Parse `source` as the code of an `eval` (§19.2.1.1): a script that is
/// strict when the calling code is, and may use `new.target` when the
/// call is inside a function.
pub(crate) fn parse_eval(
    source: &Source,
    limits: &ParseLimits,
    strict: bool,
    new_target: bool,
) -> Result<Program, ParseError> {
    let mut parser = Parser::new(source, limits, false)?;
    parser.ctx.strict = strict;
    parser.ctx.new_target = new_target;
    parser.program()
}

/// Parse `source` as a `Module` (ECMA-262 §16.2): strict, with `import`,
/// `export` and top-level `await`.
pub fn parse_module(source: &Source, limits: &ParseLimits) -> Result<Program, ParseError> {
    Parser::new(source, limits, true)?.program()
}

/// Per-function syntactic context.
#[derive(Clone, Copy, Debug, Default)]
struct Context {
    strict: bool,
    /// `return` is allowed.
    in_function: bool,
    /// `await` is an operator (async functions and module top level).
    is_async: bool,
    /// `yield` is an operator.
    is_generator: bool,
    /// `new.target` is allowed.
    new_target: bool,
    /// `super.x` is allowed.
    super_property: bool,
    /// `super()` is allowed.
    super_call: bool,
    /// Class field initializers and static blocks forbid `arguments`.
    no_arguments: bool,
    /// Class static blocks reserve `await` as an identifier.
    static_block: bool,
}

/// Labels and breakable statements of the current function.
#[derive(Clone, Debug, Default)]
struct Jumps {
    /// Labels in scope, and whether each labels an iteration statement.
    labels: Vec<(Box<str>, bool)>,
    /// Enclosing loops (`continue` and `break` targets).
    loops: u32,
    /// Enclosing `switch` statements (`break` targets).
    switches: u32,
}

/// Positions recorded while parsing an expression that may turn out to be
/// a pattern or arrow parameter list (the cover grammars).
#[derive(Clone, Copy, Debug, Default)]
struct Cover {
    /// A `{ a = 1 }` shorthand initializer, only valid in a pattern.
    shorthand_init: Option<Span>,
    /// A second `__proto__: value` in one object literal, only valid in a
    /// pattern.
    duplicate_proto: Option<Span>,
    /// A `yield` expression (invalid in arrow parameters of generators).
    yield_expr: Option<Span>,
    /// An `await` expression (invalid in async arrow parameters).
    await_expr: Option<Span>,
    /// `await` used as an identifier (invalid in async arrow parameters).
    await_ident: Option<Span>,
}

struct Parser<'a> {
    src: &'a [u16],
    lexer: Lexer<'a>,
    /// The current token.
    token: Token,
    /// End of the previous token, for node spans.
    prev_end: u32,
    module: bool,
    ctx: Context,
    jumps: Jumps,
    cover: Cover,
    scopes: Vec<Scope>,
    classes: Vec<ClassScope>,
    depth: u32,
    /// [`stack_address`] where parsing started.
    stack_base: usize,
    limits: &'a ParseLimits,
    /// Names a module exports, for duplicate detection.
    exported: Vec<(Vec<u16>, Span)>,
    /// `export { local }` references, checked against module bindings.
    export_refs: Vec<(Box<str>, Span)>,
    /// Where the assignment expression being parsed starts: only an
    /// operand starting there may be an arrow function.
    arrow_start: Option<u32>,
    /// The `no_in` flag of that assignment expression, for a concise arrow
    /// body.
    no_in_arrow_body: bool,
    /// Start offsets of array and object literals whose spread element is
    /// followed by a comma (never valid as a pattern).
    rest_not_last: HashSet<u32>,
    /// Labels directly enclosing the statement being parsed.
    pending_labels: usize,
    /// The directive prologue being parsed contained `"use strict"`.
    saw_use_strict: bool,
}

impl<'a> Parser<'a> {
    fn new(source: &'a Source, limits: &'a ParseLimits, module: bool) -> PResult<Self> {
        let mut lexer = Lexer::new(source.units(), !module);
        let token = lexer.next_token(Goal::RegExp)?;
        Ok(Self {
            src: source.units(),
            lexer,
            token,
            prev_end: 0,
            module,
            ctx: Context {
                strict: module,
                is_async: module,
                ..Context::default()
            },
            jumps: Jumps::default(),
            cover: Cover::default(),
            scopes: Vec::new(),
            classes: Vec::new(),
            depth: 0,
            stack_base: stack_address(),
            limits,
            exported: Vec::new(),
            export_refs: Vec::new(),
            arrow_start: None,
            no_in_arrow_body: false,
            rest_not_last: HashSet::new(),
            pending_labels: 0,
            saw_use_strict: false,
        })
    }

    fn program(mut self) -> PResult<Program> {
        let kind = if self.module {
            ProgramKind::Module
        } else {
            ProgramKind::Script
        };
        self.push_scope(if self.module {
            ScopeKind::Module
        } else {
            ScopeKind::Script
        });
        let body = self.statement_list_with_directives(|p| p.at_eof(), true)?;
        if !self.at_eof() {
            return Err(self.unexpected());
        }
        if self.module {
            self.check_module_exports()?;
        }
        self.pop_scope();
        Ok(Program {
            kind,
            body,
            strict: self.ctx.strict,
            span: Span::new(0, self.src.len() as u32),
        })
    }

    // ---- errors -------------------------------------------------------

    fn error(&self, span: Span, message: impl Into<Cow<'static, str>>) -> ParseError {
        ParseError::Syntax(SyntaxError::new(span, message))
    }

    fn unexpected(&self) -> ParseError {
        let message = match &self.token.kind {
            TokenKind::Eof => "unexpected end of input".into(),
            TokenKind::Punct(p) => format!("unexpected `{}`", p.as_str()).into(),
            TokenKind::Name { .. } => "unexpected identifier".into(),
            TokenKind::PrivateName(_) => "unexpected private name".into(),
            TokenKind::Template(_) => "unexpected template".into(),
            TokenKind::RegExp { .. } => "unexpected regular expression".into(),
            _ => Cow::Borrowed("unexpected literal"),
        };
        self.error(self.token.span, message)
    }

    // ---- depth ----------------------------------------------------------

    /// Enter one nesting level; pair with [`Self::leave`].
    fn enter(&mut self) -> PResult<()> {
        if self.depth >= self.limits.max_depth
            || stack_address().abs_diff(self.stack_base) > self.limits.max_stack_bytes
        {
            return Err(ParseError::TooDeep(self.token.span));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn check_cancelled(&self) -> PResult<()> {
        self.limits
            .cancellation
            .check()
            .map_err(|_| ParseError::Cancelled)
    }

    // ---- tokens ---------------------------------------------------------

    /// Move to the next token. Tokens are scanned with the division goal;
    /// a `/` where an expression starts is rescanned as a regular
    /// expression by [`Self::regexp_here`].
    fn advance(&mut self) -> PResult<Token> {
        let next = self.lexer.next_token(Goal::Div)?;
        self.prev_end = self.token.span.end;
        Ok(std::mem::replace(&mut self.token, next))
    }

    /// Rescan the current `/` or `/=` token as a regular expression.
    fn regexp_here(&mut self) -> PResult<()> {
        let mut checkpoint = self.lexer.clone();
        checkpoint.restore_to(self.token.span.start);
        let newline_before = self.token.newline_before;
        let mut token = checkpoint.next_token(Goal::RegExp)?;
        token.newline_before = newline_before;
        self.lexer = checkpoint;
        self.token = token;
        Ok(())
    }

    /// The token after the current one, without consuming anything.
    fn peek(&self) -> PResult<Token> {
        let mut lexer = self.lexer.clone();
        Ok(lexer.next_token(Goal::Div)?)
    }

    fn at(&self, p: Punct) -> bool {
        self.token.is(p)
    }

    fn at_word(&self, word: &str) -> bool {
        self.token.is_word(word)
    }

    fn at_eof(&self) -> bool {
        matches!(self.token.kind, TokenKind::Eof)
    }

    fn eat(&mut self, p: Punct) -> PResult<bool> {
        if self.at(p) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn eat_word(&mut self, word: &str) -> PResult<bool> {
        if self.at_word(word) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn expect(&mut self, p: Punct) -> PResult<Token> {
        if self.at(p) {
            self.advance()
        } else {
            Err(self.unexpected())
        }
    }

    fn expect_word(&mut self, word: &str) -> PResult<Token> {
        if self.at_word(word) {
            self.advance()
        } else {
            Err(self.unexpected())
        }
    }

    /// Automatic semicolon insertion (ECMA-262 §12.10): a `;`, or nothing
    /// before `}`, the end of input or a line terminator.
    fn semicolon(&mut self) -> PResult<()> {
        if self.eat(Punct::Semicolon)? {
            return Ok(());
        }
        if self.at(Punct::RBrace) || self.at_eof() || self.token.newline_before {
            return Ok(());
        }
        Err(self.unexpected())
    }

    /// A span from `start` to the end of the previous token.
    fn span_from(&self, start: u32) -> Span {
        Span::new(start, self.prev_end.max(start))
    }

    /// Whether the source text of `span` is exactly `text` (ASCII), used
    /// for directives, which must be written without escapes.
    fn source_is(&self, span: Span, text: &str) -> bool {
        let units = &self.src[span.start as usize..span.end as usize];
        units.len() == text.len()
            && units
                .iter()
                .zip(text.bytes())
                .all(|(&u, b)| u == u16::from(b))
    }
}

/// The address of a local in the calling frame: an approximate stack
/// pointer, so the distance between two calls is the stack used between
/// them (whichever way the stack grows). Only the address is taken; the
/// local is never read through it.
#[inline(never)]
fn stack_address() -> usize {
    let marker = 0u8;
    std::hint::black_box(&marker) as *const u8 as usize
}
