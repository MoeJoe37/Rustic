//! Component values (§5.4.7–5.4.9), built iteratively from tokens.

use rustic_base::SourceSpan;

use crate::tokenizer::{Diagnostics, SpannedToken};
use crate::{CssError, DiagnosticKind, Token};

/// Which bracket a simple block uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockKind {
    /// `{ ... }`.
    Curly,
    /// `[ ... ]`.
    Square,
    /// `( ... )`.
    Paren,
}

/// A component value: a preserved token, a function or a simple block.
#[derive(Clone, Debug, PartialEq)]
pub enum ComponentValue {
    /// Any token other than an opening bracket or function token. Unmatched
    /// closing brackets are preserved as tokens.
    Token(Token),
    /// A function and its arguments, up to the matching `)` or EOF.
    Function {
        /// Unescaped function name.
        name: String,
        /// Argument component values, including whitespace and commas.
        arguments: Vec<Value>,
    },
    /// A simple block, up to its matching close bracket or EOF.
    Block {
        /// The opening bracket.
        kind: BlockKind,
        /// The block's component values.
        contents: Vec<Value>,
    },
}

/// A component value with its byte span in the original input.
#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    /// The component value.
    pub value: ComponentValue,
    /// Its byte range, from the first token through the closing bracket.
    pub span: SourceSpan,
}

impl Value {
    /// The token, if this is a preserved token.
    pub fn token(&self) -> Option<&Token> {
        match &self.value {
            ComponentValue::Token(token) => Some(token),
            _ => None,
        }
    }

    /// Whether this is whitespace.
    pub fn is_whitespace(&self) -> bool {
        self.token().is_some_and(Token::is_whitespace)
    }

    /// The contents of a `{}` block.
    pub fn curly_block(&self) -> Option<&[Value]> {
        match &self.value {
            ComponentValue::Block {
                kind: BlockKind::Curly,
                contents,
            } => Some(contents),
            _ => None,
        }
    }
}

enum Open {
    Function(String),
    Block(BlockKind),
}

struct Frame {
    open: Open,
    start: usize,
    items: Vec<Value>,
}

impl Frame {
    fn closes(&self, token: &Token) -> bool {
        matches!(
            (&self.open, token),
            (
                Open::Function(_) | Open::Block(BlockKind::Paren),
                Token::CloseParen
            ) | (Open::Block(BlockKind::Square), Token::CloseSquare)
                | (Open::Block(BlockKind::Curly), Token::CloseCurly)
        )
    }

    fn finish(self, end: usize) -> Value {
        let value = match self.open {
            Open::Function(name) => ComponentValue::Function {
                name,
                arguments: self.items,
            },
            Open::Block(kind) => ComponentValue::Block {
                kind,
                contents: self.items,
            },
        };
        Value {
            value,
            span: SourceSpan::new(self.start, end).expect("ordered span"),
        }
    }
}

/// Group tokens into component values with an explicit stack.
///
/// `max_nesting` bounds open functions/blocks, which also bounds the depth of
/// the result (and therefore of its recursive drop).
pub(crate) fn build(
    tokens: Vec<SpannedToken>,
    source_len: usize,
    max_nesting: usize,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<Value>, CssError> {
    let mut root: Vec<Value> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    for SpannedToken { token, span } in tokens {
        let open = match &token {
            Token::Function(name) => Some(Open::Function(name.clone())),
            Token::OpenCurly => Some(Open::Block(BlockKind::Curly)),
            Token::OpenSquare => Some(Open::Block(BlockKind::Square)),
            Token::OpenParen => Some(Open::Block(BlockKind::Paren)),
            _ => None,
        };
        if let Some(open) = open {
            if stack.len() == max_nesting {
                return Err(CssError::NestingLimit);
            }
            stack.push(Frame {
                open,
                start: span.range().start,
                items: Vec::new(),
            });
            continue;
        }
        if stack.last().is_some_and(|frame| frame.closes(&token)) {
            let frame = stack.pop().expect("checked frame");
            let value = frame.finish(span.range().end);
            stack
                .last_mut()
                .map_or(&mut root, |f| &mut f.items)
                .push(value);
            continue;
        }
        let value = Value {
            value: ComponentValue::Token(token),
            span,
        };
        stack
            .last_mut()
            .map_or(&mut root, |f| &mut f.items)
            .push(value);
    }
    while let Some(frame) = stack.pop() {
        diagnostics.push(DiagnosticKind::EofInBlock, frame.start, source_len);
        let value = frame.finish(source_len);
        stack
            .last_mut()
            .map_or(&mut root, |f| &mut f.items)
            .push(value);
    }
    Ok(root)
}
