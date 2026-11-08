//! Rules and declarations (§5.4), parsed from component values.
//!
//! Rules keep their `{}` block as raw component values: a rule's grammar
//! decides how to read it, for example with [`block_contents`].

use rustic_base::SourceSpan;

use crate::tokenizer::Diagnostics;
use crate::{DiagnosticKind, Token, Value};

/// A declaration such as `color: red !important`.
#[derive(Clone, Debug, PartialEq)]
pub struct Declaration {
    /// Unescaped property name, case preserved.
    pub name: String,
    /// Value component values without leading/trailing whitespace or `!important`.
    pub value: Vec<Value>,
    /// Whether the value ended with `!important` (ASCII case-insensitive).
    pub important: bool,
    /// From the name through the last value component.
    pub span: SourceSpan,
}

/// A qualified rule such as a style rule.
#[derive(Clone, Debug, PartialEq)]
pub struct QualifiedRule {
    /// Component values before the block (for a style rule, the selector).
    pub prelude: Vec<Value>,
    /// The `{}` block's raw contents.
    pub block: Vec<Value>,
    /// From the prelude through the block.
    pub span: SourceSpan,
}

/// An at-rule such as `@media screen { ... }` or `@import "a.css";`.
#[derive(Clone, Debug, PartialEq)]
pub struct AtRule {
    /// Unescaped name without `@`.
    pub name: String,
    /// Component values between the name and the block or `;`.
    pub prelude: Vec<Value>,
    /// The `{}` block's raw contents, if the rule has a block.
    pub block: Option<Vec<Value>>,
    /// From the at-keyword through the block or `;`.
    pub span: SourceSpan,
}

/// A rule in a stylesheet or block.
#[derive(Clone, Debug, PartialEq)]
pub enum Rule {
    /// A qualified rule.
    Qualified(QualifiedRule),
    /// An at-rule.
    At(AtRule),
}

/// An item of a block's contents.
#[derive(Clone, Debug, PartialEq)]
pub enum BlockItem {
    /// A declaration.
    Declaration(Declaration),
    /// A nested rule.
    Rule(Rule),
}

/// A cursor over one list of component values.
pub(crate) struct Cursor<'a> {
    items: &'a [Value],
    index: usize,
    /// Byte offset just past the list, for EOF diagnostics.
    end: usize,
}

fn span(start: usize, end: usize) -> SourceSpan {
    SourceSpan::new(start, end.max(start)).expect("ordered span")
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(items: &'a [Value], end: usize) -> Self {
        Self {
            items,
            index: 0,
            end,
        }
    }

    pub(crate) fn peek(&self) -> Option<&'a Value> {
        self.items.get(self.index)
    }

    pub(crate) fn token(&self) -> Option<&'a Token> {
        self.peek().and_then(Value::token)
    }

    pub(crate) fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(Value::is_whitespace) {
            self.index += 1;
        }
    }

    pub(crate) fn at_end(&self) -> bool {
        self.index == self.items.len()
    }

    fn here(&self) -> usize {
        self.peek()
            .map_or(self.end, |value| value.span.range().start)
    }

    fn last_end(&self, start: usize) -> usize {
        self.index
            .checked_sub(1)
            .and_then(|i| self.items.get(i))
            .map_or(start, |value| value.span.range().end)
    }
}

/// Consume a list of rules (§5.4.1); `top_level` skips CDO/CDC.
pub(crate) fn rule_list(c: &mut Cursor, top_level: bool, d: &mut Diagnostics) -> Vec<Rule> {
    let mut rules = Vec::new();
    while let Some(value) = c.peek() {
        match value.token() {
            Some(Token::Whitespace) => c.index += 1,
            Some(Token::Cdo | Token::Cdc) if top_level => c.index += 1,
            Some(Token::AtKeyword(_)) => rules.push(Rule::At(at_rule(c, d))),
            _ => {
                if let Some(rule) = qualified_rule(c, false, d) {
                    rules.push(Rule::Qualified(rule));
                }
            }
        }
    }
    rules
}

/// Consume an at-rule (§5.4.2); the cursor is on the at-keyword.
pub(crate) fn at_rule(c: &mut Cursor, d: &mut Diagnostics) -> AtRule {
    let first = c.peek().expect("at-keyword");
    let start = first.span.range().start;
    let Some(Token::AtKeyword(name)) = first.token() else {
        unreachable!("caller checked the at-keyword")
    };
    c.index += 1;
    let mut prelude = Vec::new();
    loop {
        let Some(value) = c.peek() else {
            d.push(DiagnosticKind::EofInAtRule, start, c.end);
            return AtRule {
                name: name.clone(),
                prelude,
                block: None,
                span: span(start, c.end),
            };
        };
        c.index += 1;
        if matches!(value.token(), Some(Token::Semicolon)) {
            return AtRule {
                name: name.clone(),
                prelude,
                block: None,
                span: span(start, value.span.range().end),
            };
        }
        if let Some(block) = value.curly_block() {
            return AtRule {
                name: name.clone(),
                prelude,
                block: Some(block.to_vec()),
                span: span(start, value.span.range().end),
            };
        }
        prelude.push(value.clone());
    }
}

/// Consume a qualified rule (§5.4.3). When `nested`, a `;` ends it invalidly.
pub(crate) fn qualified_rule(
    c: &mut Cursor,
    nested: bool,
    d: &mut Diagnostics,
) -> Option<QualifiedRule> {
    let start = c.here();
    let mut prelude = Vec::new();
    loop {
        let Some(value) = c.peek() else {
            d.push(DiagnosticKind::EofInRule, start, c.end);
            return None;
        };
        c.index += 1;
        match value.token() {
            Some(Token::Semicolon) if nested => {
                d.push(DiagnosticKind::InvalidRule, start, value.span.range().end);
                return None;
            }
            Some(Token::CloseCurly) => {
                d.push(
                    DiagnosticKind::UnmatchedClose,
                    value.span.range().start,
                    value.span.range().end,
                );
            }
            _ => {}
        }
        if let Some(block) = value.curly_block() {
            return Some(QualifiedRule {
                prelude,
                block: block.to_vec(),
                span: span(start, value.span.range().end),
            });
        }
        prelude.push(value.clone());
    }
}

/// Consume a declaration from exactly `items` (§5.4.6); `None` if invalid.
pub(crate) fn declaration(items: &[Value], d: &mut Diagnostics) -> Option<Declaration> {
    let start = items.first()?.span.range().start;
    let end = items.last()?.span.range().end;
    let mut c = Cursor::new(items, end);
    let Some(Token::Ident(name)) = c.token() else {
        d.push(DiagnosticKind::InvalidDeclaration, start, end);
        return None;
    };
    c.index += 1;
    c.skip_whitespace();
    if !matches!(c.token(), Some(Token::Colon)) {
        d.push(DiagnosticKind::InvalidDeclaration, start, end);
        return None;
    }
    c.index += 1;
    c.skip_whitespace();
    let mut value = items[c.index..].to_vec();
    let trim = |value: &mut Vec<Value>| {
        while value.last().is_some_and(Value::is_whitespace) {
            value.pop();
        }
    };
    trim(&mut value);
    let mut important = false;
    if let Some(Token::Ident(word)) = value.last().and_then(Value::token) {
        if word.eq_ignore_ascii_case("important") {
            let bang = value[..value.len() - 1]
                .iter()
                .rposition(|v| !v.is_whitespace());
            if let Some(bang) = bang.filter(|&i| value[i].token() == Some(&Token::Delim('!'))) {
                value.truncate(bang);
                trim(&mut value);
                important = true;
            }
        }
    }
    // CSS Nesting: outside custom properties, a `{}` block mixed with other
    // values means this is a nested rule (`a:hover {..}`), not a declaration.
    if !name.starts_with("--")
        && value.iter().any(|v| v.curly_block().is_some())
        && value
            .iter()
            .any(|v| v.curly_block().is_none() && !v.is_whitespace())
    {
        d.push(DiagnosticKind::InvalidDeclaration, start, end);
        return None;
    }
    let last = value
        .last()
        .map_or(c.last_end(start), |v| v.span.range().end);
    Some(Declaration {
        name: name.clone(),
        value,
        important,
        span: span(start, if important { end } else { last }),
    })
}

/// Consume a block's contents (CSS Syntax §5.4.4, with CSS Nesting): declarations,
/// at-rules and nested qualified rules, in source order.
pub(crate) fn block_contents(c: &mut Cursor, d: &mut Diagnostics) -> Vec<BlockItem> {
    let mut items = Vec::new();
    while let Some(value) = c.peek() {
        match value.token() {
            Some(Token::Whitespace | Token::Semicolon) => c.index += 1,
            Some(Token::AtKeyword(_)) => items.push(BlockItem::Rule(Rule::At(at_rule(c, d)))),
            Some(Token::Ident(_)) => {
                let end = c.items[c.index..]
                    .iter()
                    .position(|v| v.token() == Some(&Token::Semicolon))
                    .map_or(c.items.len(), |offset| c.index + offset);
                let mut probe = Diagnostics {
                    limit: 0,
                    ..Diagnostics::default()
                };
                if let Some(declaration) = declaration(&c.items[c.index..end], &mut probe) {
                    items.push(BlockItem::Declaration(declaration));
                    c.index = end;
                } else if let Some(rule) = qualified_rule(c, true, d) {
                    items.push(BlockItem::Rule(Rule::Qualified(rule)));
                }
            }
            _ => {
                if let Some(rule) = qualified_rule(c, true, d) {
                    items.push(BlockItem::Rule(Rule::Qualified(rule)));
                }
            }
        }
    }
    items
}
