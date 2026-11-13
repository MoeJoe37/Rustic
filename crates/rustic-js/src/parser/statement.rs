//! Statements, declarations, functions, classes and modules (ECMA-262
//! §14–§16).

use std::collections::HashSet;

use super::expression::is_reserved_word;
use super::scope::{PrivateKind, ScopeKind};
use super::{Context, PResult, Parser};
use crate::ast::{
    CatchClause, Class, ClassElement, DefaultExport, ExportDeclaration, ExportSpecifier,
    Expression, ExpressionKind, ForHead, ForInit, Function, Identifier, ImportAttribute,
    ImportDeclaration, ImportSpecifier, MethodKind, ModuleExportName, PatternKind, PropertyKey,
    Statement, StatementKind, SwitchCase, VariableDeclaration, VariableDeclarator, VariableKind,
};
use crate::source::Span;
use crate::token::{Punct, TokenKind};

/// Where a statement appears, for Annex B function declaration rules.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Position {
    /// A statement list item (or a label in one): labelled functions are
    /// allowed in sloppy mode.
    ListItem,
    /// The body of an `if` (Annex B.3.3 allows a plain function there).
    IfBody,
    /// The body of a loop or `with`, or anything else.
    Other,
}

fn stmt(kind: StatementKind, span: Span) -> Statement {
    Statement { kind, span }
}

/// Whether `units` is well-formed UTF-16 (module export names must be).
fn well_formed(units: &[u16]) -> bool {
    char::decode_utf16(units.iter().copied()).all(|c| c.is_ok())
}

impl Parser<'_> {
    /// Statements until `end`, starting with a directive prologue. Returns
    /// the statements; `self.ctx.strict` is updated by `"use strict"`.
    pub(super) fn statement_list_with_directives(
        &mut self,
        end: impl Fn(&Self) -> bool,
        top: bool,
    ) -> PResult<Vec<Statement>> {
        let _ = top;
        let mut body = Vec::new();
        let mut prologue = true;
        let mut legacy_escape: Option<Span> = None;
        while !end(self) {
            self.check_cancelled()?;
            if prologue {
                if let TokenKind::String {
                    legacy_escape: legacy,
                    ..
                } = &self.token.kind
                {
                    let token_span = self.token.span;
                    let legacy = *legacy;
                    // Strictness of earlier directives decides the check,
                    // so read the token before parsing it.
                    let mut statement = self.statement_list_item()?;
                    let is_directive = matches!(
                        &statement.kind,
                        StatementKind::Expression { expression, .. }
                            if matches!(expression.kind, ExpressionKind::String(_))
                                && expression.span == token_span
                    );
                    if is_directive {
                        if let StatementKind::Expression { directive, .. } = &mut statement.kind {
                            *directive = true;
                        }
                        if legacy {
                            legacy_escape.get_or_insert(token_span);
                        }
                        if self.source_is(token_span, "\"use strict\"")
                            || self.source_is(token_span, "'use strict'")
                        {
                            self.saw_use_strict = true;
                            self.ctx.strict = true;
                            if let Some(span) = legacy_escape {
                                return Err(
                                    self.error(span, "octal escape sequence in strict mode")
                                );
                            }
                        }
                    } else {
                        prologue = false;
                    }
                    body.push(statement);
                    continue;
                }
                prologue = false;
            }
            body.push(self.statement_list_item()?);
        }
        Ok(body)
    }

    /// `StatementListItem` (and `ModuleItem` at module top level).
    fn statement_list_item(&mut self) -> PResult<Statement> {
        self.enter()?;
        let result = self.statement_list_item_inner();
        self.leave();
        result
    }

    fn statement_list_item_inner(&mut self) -> PResult<Statement> {
        let start = self.token.span.start;
        if self.at_word("function") {
            let function = self.function_declaration(start, false, false)?;
            return Ok(stmt(
                StatementKind::Function(Box::new(function)),
                self.span_from(start),
            ));
        }
        if self.at_word("async") {
            let next = self.peek()?;
            if next.is_word("function") && !next.newline_before {
                self.advance()?;
                let function = self.function_declaration(start, true, false)?;
                return Ok(stmt(
                    StatementKind::Function(Box::new(function)),
                    self.span_from(start),
                ));
            }
        }
        if self.at_word("class") {
            let class = self.class(true)?;
            return Ok(stmt(
                StatementKind::Class(Box::new(class)),
                self.span_from(start),
            ));
        }
        if self.at_word("const") || self.at_let_declaration()? {
            let declaration = self.lexical_declaration(false)?;
            self.semicolon()?;
            return Ok(stmt(
                StatementKind::Variable(declaration),
                self.span_from(start),
            ));
        }
        if self.module && self.at_word("import") {
            let next = self.peek()?;
            if !next.is(Punct::LParen) && !next.is(Punct::Dot) {
                return self.import_declaration();
            }
        }
        if self.at_word("export") {
            if !self.module || self.scopes.len() != 1 {
                return Err(self.error(self.token.span, "`export` outside module top level"));
            }
            return self.export_declaration();
        }
        if self.at_word("import") && self.module {
            // `import(...)` and `import.meta` are expressions.
            return self.statement(Position::ListItem);
        }
        self.statement(Position::ListItem)
    }

    /// Whether the current `let` starts a lexical declaration.
    fn at_let_declaration(&mut self) -> PResult<bool> {
        if !self.at_word("let") {
            return Ok(false);
        }
        let next = self.peek()?;
        Ok(match &next.kind {
            TokenKind::Punct(Punct::LBracket | Punct::LBrace) => true,
            TokenKind::Name { name, .. } => {
                // `let in`, `let instanceof` stay expressions (sloppy `let`
                // identifier); other names are bindings, checked later.
                !(is_reserved_word(name) && !matches!(&**name, "yield" | "await" | "let"))
                    || name.as_ref() == "let"
            }
            _ => false,
        })
    }

    fn statement(&mut self, position: Position) -> PResult<Statement> {
        self.enter()?;
        let result = self.statement_inner(position);
        self.leave();
        result
    }

    fn statement_inner(&mut self, position: Position) -> PResult<Statement> {
        let start = self.token.span.start;
        let pending = std::mem::take(&mut self.pending_labels);
        if let TokenKind::Name {
            name,
            escaped: false,
        } = &self.token.kind
        {
            let is_loop = matches!(&**name, "for" | "while" | "do");
            if is_loop {
                let n = self.jumps.labels.len();
                for label in &mut self.jumps.labels[n - pending.min(n)..] {
                    label.1 = true;
                }
            }
            match &**name {
                "var" => {
                    self.advance()?;
                    let declaration = self.variable_declarations(VariableKind::Var, false)?;
                    self.semicolon()?;
                    return Ok(stmt(
                        StatementKind::Variable(declaration),
                        self.span_from(start),
                    ));
                }
                "if" => return self.if_statement(),
                "for" => return self.for_statement(),
                "while" => {
                    self.advance()?;
                    self.expect(Punct::LParen)?;
                    let test = self.expression(false)?;
                    self.expect(Punct::RParen)?;
                    let body = self.loop_body()?;
                    return Ok(stmt(
                        StatementKind::While {
                            test,
                            body: Box::new(body),
                        },
                        self.span_from(start),
                    ));
                }
                "do" => {
                    self.advance()?;
                    let body = self.loop_body()?;
                    self.expect_word("while")?;
                    self.expect(Punct::LParen)?;
                    let test = self.expression(false)?;
                    self.expect(Punct::RParen)?;
                    // ASI after `do-while` always applies (§12.10.1).
                    self.eat(Punct::Semicolon)?;
                    return Ok(stmt(
                        StatementKind::DoWhile {
                            body: Box::new(body),
                            test,
                        },
                        self.span_from(start),
                    ));
                }
                "continue" | "break" => return self.jump_statement(),
                "return" => {
                    let token = self.advance()?;
                    if !self.ctx.in_function {
                        return Err(self.error(token.span, "`return` outside a function"));
                    }
                    let argument = if self.at(Punct::Semicolon)
                        || self.at(Punct::RBrace)
                        || self.at_eof()
                        || self.token.newline_before
                    {
                        None
                    } else {
                        Some(self.expression(false)?)
                    };
                    self.semicolon()?;
                    return Ok(stmt(StatementKind::Return(argument), self.span_from(start)));
                }
                "with" => {
                    let token = self.advance()?;
                    if self.ctx.strict {
                        return Err(self.error(token.span, "`with` in strict mode"));
                    }
                    self.expect(Punct::LParen)?;
                    let object = self.expression(false)?;
                    self.expect(Punct::RParen)?;
                    let body = self.statement(Position::Other)?;
                    return Ok(stmt(
                        StatementKind::With {
                            object,
                            body: Box::new(body),
                        },
                        self.span_from(start),
                    ));
                }
                "switch" => return self.switch_statement(),
                "throw" => {
                    self.advance()?;
                    if self.token.newline_before {
                        return Err(self.error(self.token.span, "line break after `throw`"));
                    }
                    let argument = self.expression(false)?;
                    self.semicolon()?;
                    return Ok(stmt(StatementKind::Throw(argument), self.span_from(start)));
                }
                "try" => return self.try_statement(),
                "debugger" => {
                    self.advance()?;
                    self.semicolon()?;
                    return Ok(stmt(StatementKind::Debugger, self.span_from(start)));
                }
                "function" => {
                    // Annex B.3.3: `if (x) function f() {}` in sloppy mode.
                    if position == Position::IfBody && !self.ctx.strict {
                        self.push_scope(ScopeKind::Block);
                        let function = self.function_declaration(start, false, true);
                        self.pop_scope();
                        let function = function?;
                        if function.is_generator || function.is_async {
                            return Err(self.error(
                                function.span,
                                "generator declaration in a statement position",
                            ));
                        }
                        return Ok(stmt(
                            StatementKind::Function(Box::new(function)),
                            self.span_from(start),
                        ));
                    }
                    return Err(self.error(
                        self.token.span,
                        "function declaration in a statement position",
                    ));
                }
                "class" | "const" => {
                    return Err(self.error(self.token.span, "declaration in a statement position"));
                }
                "let" => {
                    if self.peek()?.is(Punct::LBracket) {
                        return Err(self.error(
                            self.token.span,
                            "lexical declaration in a statement position",
                        ));
                    }
                }
                "async" => {
                    let next = self.peek()?;
                    if next.is_word("function") && !next.newline_before {
                        return Err(
                            self.error(self.token.span, "async function in a statement position")
                        );
                    }
                }
                _ => {}
            }
        }
        if self.at(Punct::LBrace) {
            return self.block_statement();
        }
        if self.eat(Punct::Semicolon)? {
            return Ok(stmt(StatementKind::Empty, self.span_from(start)));
        }
        // A label: an identifier followed by `:`.
        if let TokenKind::Name { name, .. } = &self.token.kind {
            let name = name.clone();
            if self.peek()?.is(Punct::Colon) && !is_reserved_word(&name) {
                return self.labeled_statement(name, position, pending);
            }
        }
        let expression = self.expression(false)?;
        self.semicolon()?;
        Ok(stmt(
            StatementKind::Expression {
                expression,
                directive: false,
            },
            self.span_from(start),
        ))
    }

    fn labeled_statement(
        &mut self,
        name: Box<str>,
        position: Position,
        pending: usize,
    ) -> PResult<Statement> {
        let start = self.token.span.start;
        let token = self.advance()?;
        self.check_identifier(&name, token.span, false)?;
        if self.jumps.labels.iter().any(|(label, _)| *label == name) {
            return Err(self.error(token.span, format!("duplicate label `{name}`")));
        }
        self.expect(Punct::Colon)?;
        self.jumps.labels.push((name.clone(), false));
        self.pending_labels = pending + 1;
        let body = if self.at_word("function") {
            // Annex B.3.1: labelled function declarations in sloppy mode,
            // never as the body of an `if` or a loop.
            if self.ctx.strict || position != Position::ListItem {
                self.jumps.labels.pop();
                return Err(self.error(self.token.span, "labelled function declaration"));
            }
            let fstart = self.token.span.start;
            let function = self.function_declaration(fstart, false, false);
            let function = function.and_then(|f| {
                if f.is_generator || f.is_async {
                    Err(self.error(f.span, "labelled generator declaration"))
                } else {
                    Ok(f)
                }
            });
            match function {
                Ok(f) => Ok(stmt(
                    StatementKind::Function(Box::new(f)),
                    self.span_from(fstart),
                )),
                Err(e) => Err(e),
            }
        } else {
            self.statement(position)
        };
        self.pending_labels = 0;
        self.jumps.labels.pop();
        let body = body?;
        Ok(stmt(
            StatementKind::Labeled {
                label: Identifier {
                    name,
                    span: token.span,
                },
                body: Box::new(body),
            },
            self.span_from(start),
        ))
    }

    fn block_statement(&mut self) -> PResult<Statement> {
        let start = self.token.span.start;
        let body = self.block(ScopeKind::Block)?;
        Ok(stmt(StatementKind::Block(body), self.span_from(start)))
    }

    /// `{ StatementList }` in a new scope of `kind`.
    fn block(&mut self, kind: ScopeKind) -> PResult<Vec<Statement>> {
        self.expect(Punct::LBrace)?;
        self.push_scope(kind);
        let result = self.statements_until_brace();
        self.pop_scope();
        let body = result?;
        self.expect(Punct::RBrace)?;
        Ok(body)
    }

    fn statements_until_brace(&mut self) -> PResult<Vec<Statement>> {
        let mut body = Vec::new();
        while !self.at(Punct::RBrace) {
            if self.at_eof() {
                return Err(self.unexpected());
            }
            self.check_cancelled()?;
            body.push(self.statement_list_item()?);
        }
        Ok(body)
    }

    fn loop_body(&mut self) -> PResult<Statement> {
        self.jumps.loops += 1;
        let body = self.statement(Position::Other);
        self.jumps.loops -= 1;
        body
    }

    fn if_statement(&mut self) -> PResult<Statement> {
        let start = self.token.span.start;
        self.advance()?;
        self.expect(Punct::LParen)?;
        let test = self.expression(false)?;
        self.expect(Punct::RParen)?;
        let consequent = self.statement(Position::IfBody)?;
        let alternate = if self.eat_word("else")? {
            Some(Box::new(self.statement(Position::IfBody)?))
        } else {
            None
        };
        Ok(stmt(
            StatementKind::If {
                test,
                consequent: Box::new(consequent),
                alternate,
            },
            self.span_from(start),
        ))
    }

    fn jump_statement(&mut self) -> PResult<Statement> {
        let start = self.token.span.start;
        let token = self.advance()?;
        let is_continue = token.is_word("continue");
        let label = match &self.token.kind {
            TokenKind::Name { name, .. }
                if !self.token.newline_before && !is_reserved_word(name) =>
            {
                let label = Identifier {
                    name: name.clone(),
                    span: self.token.span,
                };
                self.advance()?;
                Some(label)
            }
            _ => None,
        };
        match &label {
            Some(label) => {
                let found = self
                    .jumps
                    .labels
                    .iter()
                    .find(|(name, _)| *name == label.name);
                match found {
                    None => {
                        return Err(
                            self.error(label.span, format!("undefined label `{}`", label.name))
                        );
                    }
                    Some((_, is_loop)) if is_continue && !is_loop => {
                        return Err(self.error(label.span, "`continue` to a non-loop label"));
                    }
                    Some(_) => {}
                }
            }
            None => {
                let ok = if is_continue {
                    self.jumps.loops > 0
                } else {
                    self.jumps.loops > 0 || self.jumps.switches > 0
                };
                if !ok {
                    return Err(self.error(
                        token.span,
                        if is_continue {
                            "`continue` outside a loop"
                        } else {
                            "`break` outside a loop or switch"
                        },
                    ));
                }
            }
        }
        self.semicolon()?;
        let kind = if is_continue {
            StatementKind::Continue(label)
        } else {
            StatementKind::Break(label)
        };
        Ok(stmt(kind, self.span_from(start)))
    }

    fn switch_statement(&mut self) -> PResult<Statement> {
        let start = self.token.span.start;
        self.advance()?;
        self.expect(Punct::LParen)?;
        let discriminant = self.expression(false)?;
        self.expect(Punct::RParen)?;
        self.expect(Punct::LBrace)?;
        self.push_scope(ScopeKind::Block);
        self.jumps.switches += 1;
        let result = self.switch_cases();
        self.jumps.switches -= 1;
        self.pop_scope();
        let cases = result?;
        self.expect(Punct::RBrace)?;
        Ok(stmt(
            StatementKind::Switch {
                discriminant,
                cases,
            },
            self.span_from(start),
        ))
    }

    fn switch_cases(&mut self) -> PResult<Vec<SwitchCase>> {
        let mut cases = Vec::new();
        let mut has_default = false;
        while !self.at(Punct::RBrace) {
            let start = self.token.span.start;
            let test = if self.eat_word("case")? {
                Some(self.expression(false)?)
            } else if self.at_word("default") {
                let token = self.advance()?;
                if has_default {
                    return Err(self.error(token.span, "duplicate `default` clause"));
                }
                has_default = true;
                None
            } else {
                return Err(self.unexpected());
            };
            self.expect(Punct::Colon)?;
            let mut body = Vec::new();
            while !self.at(Punct::RBrace) && !self.at_word("case") && !self.at_word("default") {
                if self.at_eof() {
                    return Err(self.unexpected());
                }
                body.push(self.statement_list_item()?);
            }
            cases.push(SwitchCase {
                test,
                body,
                span: self.span_from(start),
            });
        }
        Ok(cases)
    }

    fn try_statement(&mut self) -> PResult<Statement> {
        let start = self.token.span.start;
        self.advance()?;
        let block = self.block(ScopeKind::Block)?;
        let handler = if self.at_word("catch") {
            let catch_start = self.token.span.start;
            self.advance()?;
            self.push_scope(ScopeKind::Catch);
            let result = self.catch_rest(catch_start);
            self.pop_scope();
            Some(result?)
        } else {
            None
        };
        let finalizer = if self.eat_word("finally")? {
            Some(self.block(ScopeKind::Block)?)
        } else {
            None
        };
        if handler.is_none() && finalizer.is_none() {
            return Err(self.unexpected());
        }
        Ok(stmt(
            StatementKind::Try {
                block,
                handler,
                finalizer,
            },
            self.span_from(start),
        ))
    }

    fn catch_rest(&mut self, start: u32) -> PResult<CatchClause> {
        let param = if self.eat(Punct::LParen)? {
            let param = self.binding_target()?;
            self.expect(Punct::RParen)?;
            let mut names = Vec::new();
            Self::bound_names(&param, &mut names);
            let simple = matches!(param.kind, PatternKind::Identifier(_));
            self.declare_catch_params(&names, simple)?;
            Some(param)
        } else {
            None
        };
        let body = self.block(ScopeKind::Block)?;
        Ok(CatchClause {
            param,
            body,
            span: self.span_from(start),
        })
    }

    // ---- variables ------------------------------------------------------

    /// `let`/`const` declarations (the keyword is current).
    fn lexical_declaration(&mut self, in_for: bool) -> PResult<VariableDeclaration> {
        let token = self.advance()?;
        if let TokenKind::Name { escaped: true, .. } = token.kind {
            return Err(self.error(token.span, "escaped declaration keyword"));
        }
        let kind = if token.is_word("const") {
            VariableKind::Const
        } else {
            VariableKind::Let
        };
        self.variable_declarations(kind, in_for)
    }

    /// Declarators after `var`, `let` or `const`. In a `for` head
    /// (`in_for`), `in` is not an operator and initializers are checked by
    /// the caller.
    fn variable_declarations(
        &mut self,
        kind: VariableKind,
        in_for: bool,
    ) -> PResult<VariableDeclaration> {
        let mut declarations = Vec::new();
        loop {
            let start = self.token.span.start;
            let target = self.binding_target()?;
            let init = if self.eat(Punct::Eq)? {
                Some(self.assignment(in_for)?)
            } else {
                None
            };
            let for_in_of = in_for && (self.at_word("in") || self.at_word("of"));
            if init.is_none() && !for_in_of {
                if kind == VariableKind::Const {
                    return Err(self.error(self.span_from(start), "`const` without an initializer"));
                }
                if !matches!(target.kind, PatternKind::Identifier(_)) {
                    return Err(self.error(
                        self.span_from(start),
                        "destructuring declaration without an initializer",
                    ));
                }
            }
            let mut names = Vec::new();
            Self::bound_names(&target, &mut names);
            for (name, span) in names {
                if kind == VariableKind::Var {
                    self.declare_var(&name, span)?;
                } else {
                    if &*name == "let" {
                        return Err(self.error(span, "`let` cannot be a lexically bound name"));
                    }
                    self.declare_lexical(&name, span, false)?;
                }
            }
            declarations.push(VariableDeclarator {
                target,
                init,
                span: self.span_from(start),
            });
            if !self.eat(Punct::Comma)? {
                break;
            }
        }
        Ok(VariableDeclaration { kind, declarations })
    }

    fn for_statement(&mut self) -> PResult<Statement> {
        let start = self.token.span.start;
        self.advance()?;
        let is_await = if self.at_word("await") {
            if !self.ctx.is_async {
                return Err(self.unexpected());
            }
            self.advance()?;
            true
        } else {
            false
        };
        self.expect(Punct::LParen)?;
        self.push_scope(ScopeKind::Block);
        let result = self.for_rest(start, is_await);
        self.pop_scope();
        result
    }

    fn for_rest(&mut self, start: u32, is_await: bool) -> PResult<Statement> {
        let init_start = self.token.span.start;
        let init = if self.at(Punct::Semicolon) {
            None
        } else if self.at_word("var") {
            self.advance()?;
            Some(ForInit::Variable(
                self.variable_declarations(VariableKind::Var, true)?,
            ))
        } else if self.at_word("const") || self.at_let_declaration()? {
            Some(ForInit::Variable(self.lexical_declaration(true)?))
        } else {
            let starts_with_let = self.at_word("let");
            let starts_with_async = self.at_word("async") && !self.peek()?.is(Punct::Arrow);
            let e = self.for_head_expression()?;
            if self.at_word("of") {
                if starts_with_let {
                    return Err(self.error(e.span, "`for (let of ...)` is not allowed"));
                }
                if starts_with_async && !is_await && matches!(e.kind, ExpressionKind::Identifier(_))
                {
                    return Err(self.error(e.span, "`for (async of ...)` is not allowed"));
                }
            }
            Some(ForInit::Expression(e))
        };
        if self.at_word("of") || self.at_word("in") {
            let is_of = self.at_word("of");
            if is_await && !is_of {
                return Err(self.unexpected());
            }
            let left = match init {
                Some(ForInit::Variable(declaration)) => {
                    if declaration.declarations.len() != 1 {
                        return Err(self.error(
                            self.span_from(init_start),
                            "one binding expected in a for-in/of head",
                        ));
                    }
                    let d = &declaration.declarations[0];
                    if d.init.is_some() {
                        // Annex B.3.5: `for (var x = 1 in o)` in sloppy mode.
                        let annex_b = !is_of
                            && !self.ctx.strict
                            && declaration.kind == VariableKind::Var
                            && matches!(d.target.kind, PatternKind::Identifier(_));
                        if !annex_b {
                            return Err(self.error(d.span, "initializer in a for-in/of head"));
                        }
                    }
                    ForHead::Variable(declaration)
                }
                Some(ForInit::Expression(e)) => {
                    let pattern =
                        if matches!(e.kind, ExpressionKind::Object(_) | ExpressionKind::Array(_)) {
                            self.assignment_pattern_from(e)?
                        } else {
                            self.simple_target(e, "invalid for-in/of target")?
                        };
                    ForHead::Pattern(pattern)
                }
                None => return Err(self.unexpected()),
            };
            self.advance()?;
            let right = if is_of {
                self.assignment(false)?
            } else {
                self.expression(false)?
            };
            self.expect(Punct::RParen)?;
            let body = self.loop_body()?;
            let kind = if is_of {
                StatementKind::ForOf {
                    left,
                    right,
                    body: Box::new(body),
                    is_await,
                }
            } else {
                StatementKind::ForIn {
                    left,
                    right,
                    body: Box::new(body),
                }
            };
            return Ok(stmt(kind, self.span_from(start)));
        }
        if is_await {
            return Err(self.unexpected());
        }
        if let Some(ForInit::Variable(declaration)) = &init {
            for d in &declaration.declarations {
                if d.init.is_none()
                    && (declaration.kind == VariableKind::Const
                        || !matches!(d.target.kind, PatternKind::Identifier(_)))
                {
                    return Err(self.error(d.span, "missing initializer"));
                }
            }
        }
        self.expect(Punct::Semicolon)?;
        let test = if self.at(Punct::Semicolon) {
            None
        } else {
            Some(self.expression(false)?)
        };
        self.expect(Punct::Semicolon)?;
        let update = if self.at(Punct::RParen) {
            None
        } else {
            Some(self.expression(false)?)
        };
        self.expect(Punct::RParen)?;
        let body = self.loop_body()?;
        Ok(stmt(
            StatementKind::For {
                init,
                test,
                update,
                body: Box::new(body),
            },
            self.span_from(start),
        ))
    }

    /// The expression after `for (`: a destructuring pattern may follow
    /// (`for ({ a = 1 } of b)`), so cover errors wait for `in`/`of`.
    fn for_head_expression(&mut self) -> PResult<Expression> {
        let outer = std::mem::take(&mut self.cover);
        let e = self.expression_cover(true);
        let inner = std::mem::replace(&mut self.cover, outer);
        let e = e?;
        let pattern_follows = (self.at_word("of") || self.at_word("in"))
            && matches!(e.kind, ExpressionKind::Object(_) | ExpressionKind::Array(_));
        if !pattern_follows {
            self.report_cover(&inner)?;
        }
        self.merge_positions(inner);
        Ok(e)
    }

    // ---- functions ------------------------------------------------------

    /// A function declaration at `function` (after `async` when
    /// `is_async`). `in_if` is the Annex B `if (x) function f() {}` form.
    fn function_declaration(
        &mut self,
        start: u32,
        is_async: bool,
        in_if: bool,
    ) -> PResult<Function> {
        self.function_declaration_named(start, is_async, in_if, true)
    }

    fn function_declaration_named(
        &mut self,
        start: u32,
        is_async: bool,
        in_if: bool,
        name_required: bool,
    ) -> PResult<Function> {
        let keyword = self.expect_word("function")?;
        if let TokenKind::Name { escaped: true, .. } = keyword.kind {
            return Err(self.error(keyword.span, "escaped `function` keyword"));
        }
        let is_generator = self.eat(Punct::Star)?;
        // A declaration's name follows the enclosing context's yield/await
        // rules.
        let id = if !name_required && self.at(Punct::LParen) {
            None
        } else {
            Some(self.binding_identifier()?)
        };
        if let Some(id) = &id {
            let block_function = !self.ctx.strict && !is_async && !is_generator;
            if self.at_var_scope_top() {
                self.declare_var(&id.name, id.span)?;
            } else {
                let _ = in_if;
                self.declare_lexical(&id.name, id.span, block_function)?;
            }
        }
        self.function_rest(start, id, is_async, is_generator, FunctionKind::Normal)
    }

    /// A function expression at `function` (after `async`).
    pub(super) fn function_expression(&mut self, start: u32, is_async: bool) -> PResult<Function> {
        let keyword = self.expect_word("function")?;
        if let TokenKind::Name { escaped: true, .. } = keyword.kind {
            return Err(self.error(keyword.span, "escaped `function` keyword"));
        }
        let is_generator = self.eat(Punct::Star)?;
        let id = if self.at(Punct::LParen) {
            None
        } else {
            // An expression's name follows its own yield/await rules.
            let saved = self.ctx;
            self.ctx.is_generator = is_generator;
            self.ctx.is_async = is_async;
            self.ctx.static_block = false;
            let id = self.binding_identifier();
            self.ctx = saved;
            Some(id?)
        };
        self.function_rest(start, id, is_async, is_generator, FunctionKind::Normal)
    }

    /// A method body after its key (object literal or class).
    pub(super) fn method(
        &mut self,
        start: u32,
        kind: MethodKind,
        is_async: bool,
        is_generator: bool,
        derived_constructor: bool,
    ) -> PResult<Function> {
        self.function_rest(
            start,
            None,
            is_async,
            is_generator,
            FunctionKind::Method {
                kind,
                super_call: derived_constructor,
            },
        )
    }

    /// Parameters and body, in a new function context.
    fn function_rest(
        &mut self,
        start: u32,
        id: Option<Identifier>,
        is_async: bool,
        is_generator: bool,
        kind: FunctionKind,
    ) -> PResult<Function> {
        let saved_ctx = self.ctx;
        let saved_jumps = std::mem::take(&mut self.jumps);
        let saved_cover = std::mem::take(&mut self.cover);
        let saved_arrow = self.arrow_start.take();
        let saved_labels = std::mem::take(&mut self.pending_labels);
        self.ctx = Context {
            strict: saved_ctx.strict,
            in_function: true,
            is_async,
            is_generator,
            new_target: true,
            super_property: matches!(kind, FunctionKind::Method { .. }),
            super_call: matches!(
                kind,
                FunctionKind::Method {
                    super_call: true,
                    ..
                }
            ),
            no_arguments: false,
            static_block: false,
        };
        self.push_scope(ScopeKind::Function);
        let result = self.function_inner(start, id, is_async, is_generator, kind);
        self.pop_scope();
        self.ctx = saved_ctx;
        self.jumps = saved_jumps;
        self.cover = saved_cover;
        self.arrow_start = saved_arrow;
        self.pending_labels = saved_labels;
        result
    }

    fn function_inner(
        &mut self,
        start: u32,
        id: Option<Identifier>,
        is_async: bool,
        is_generator: bool,
        kind: FunctionKind,
    ) -> PResult<Function> {
        self.expect(Punct::LParen)?;
        let mut params = Vec::new();
        let mut rest = None;
        while !self.at(Punct::RParen) {
            if self.eat(Punct::Ellipsis)? {
                rest = Some(self.binding_target()?);
                break;
            }
            params.push(self.binding_element()?);
            if !self.eat(Punct::Comma)? {
                break;
            }
        }
        self.expect(Punct::RParen)?;
        if let Some(span) = self.cover.yield_expr {
            return Err(self.error(span, "`yield` in formal parameters"));
        }
        if let Some(span) = self.cover.await_expr {
            return Err(self.error(span, "`await` in formal parameters"));
        }
        if let FunctionKind::Method { kind, .. } = kind {
            match kind {
                MethodKind::Get if !params.is_empty() || rest.is_some() => {
                    return Err(self.error(self.span_from(start), "getter with parameters"));
                }
                MethodKind::Set if params.len() != 1 || rest.is_some() => {
                    return Err(self.error(
                        self.span_from(start),
                        "setter must have exactly one parameter",
                    ));
                }
                _ => {}
            }
        }
        let names = self.param_names(&params, rest.as_ref())?;
        self.declare_params(&names);
        let simple = rest.is_none()
            && params
                .iter()
                .all(|p| matches!(p.kind, PatternKind::Identifier(_)));
        let (body, use_strict) = self.function_body()?;
        if use_strict && !simple {
            return Err(self.error(
                self.span_from(start),
                "\"use strict\" in a function with non-simple parameters",
            ));
        }
        let unique = !simple || matches!(kind, FunctionKind::Method { .. });
        self.check_params(&names, unique)?;
        if self.ctx.strict {
            if let Some(id) = &id {
                // The name follows the function's strictness (§15.2.1).
                let saved = self.ctx;
                self.ctx.is_generator = false;
                self.ctx.is_async = false;
                let checked = self.check_identifier(&id.name, id.span, true);
                self.ctx = saved;
                checked?;
            }
        }
        Ok(Function {
            id,
            params,
            rest,
            body,
            is_async,
            is_generator,
            strict: self.ctx.strict,
            span: self.span_from(start),
        })
    }

    /// `{ FunctionBody }`; returns the statements and whether its
    /// directive prologue contains `"use strict"` (which non-simple
    /// parameter lists forbid even in code that is already strict).
    pub(super) fn function_body(&mut self) -> PResult<(Vec<Statement>, bool)> {
        self.expect(Punct::LBrace)?;
        let saved = std::mem::replace(&mut self.saw_use_strict, false);
        let body =
            self.statement_list_with_directives(|p| p.at(Punct::RBrace) || p.at_eof(), false);
        let use_strict = std::mem::replace(&mut self.saw_use_strict, saved);
        let body = body?;
        self.expect(Punct::RBrace)?;
        Ok((body, use_strict))
    }

    // ---- classes --------------------------------------------------------

    /// A class at `class`. Declarations bind their name in the current
    /// scope; class code is always strict.
    pub(super) fn class(&mut self, declaration: bool) -> PResult<Class> {
        self.class_named(declaration, declaration)
    }

    fn class_named(&mut self, declaration: bool, name_required: bool) -> PResult<Class> {
        let start = self.token.span.start;
        let keyword = self.advance()?;
        if let TokenKind::Name { escaped: true, .. } = keyword.kind {
            return Err(self.error(keyword.span, "escaped `class` keyword"));
        }
        let saved_strict = self.ctx.strict;
        self.ctx.strict = true;
        let result = self.class_inner(start, declaration, name_required);
        self.ctx.strict = saved_strict;
        result
    }

    fn class_inner(
        &mut self,
        start: u32,
        declaration: bool,
        name_required: bool,
    ) -> PResult<Class> {
        let id = match &self.token.kind {
            TokenKind::Name { name, .. } if &**name != "extends" => {
                Some(self.binding_identifier()?)
            }
            _ if name_required => return Err(self.unexpected()),
            _ => None,
        };
        if declaration {
            if let Some(id) = &id {
                self.declare_lexical(&id.name, id.span, false)?;
            }
        }
        let super_class = if self.eat_word("extends")? {
            let lhs_start = self.token.span.start;
            let e = self.left_hand_side_for_heritage()?;
            let _ = lhs_start;
            Some(Box::new(e))
        } else {
            None
        };
        self.expect(Punct::LBrace)?;
        self.push_class();
        let body = self.class_body(super_class.is_some());
        let popped = self.pop_class();
        let body = body?;
        popped?;
        self.expect(Punct::RBrace)?;
        Ok(Class {
            id,
            super_class,
            body,
            span: self.span_from(start),
        })
    }

    fn left_hand_side_for_heritage(&mut self) -> PResult<Expression> {
        let saved_arrow = self.arrow_start.take();
        let e = self.left_hand_side();
        self.arrow_start = saved_arrow;
        e
    }

    fn class_body(&mut self, derived: bool) -> PResult<Vec<ClassElement>> {
        let mut elements = Vec::new();
        let mut has_constructor = false;
        while !self.at(Punct::RBrace) {
            if self.eat(Punct::Semicolon)? {
                continue;
            }
            if self.at_eof() {
                return Err(self.unexpected());
            }
            let element = self.class_element(derived)?;
            if let ClassElement::Method {
                kind: MethodKind::Constructor,
                span,
                ..
            } = &element
            {
                if has_constructor {
                    return Err(self.error(*span, "duplicate constructor"));
                }
                has_constructor = true;
            }
            elements.push(element);
        }
        Ok(elements)
    }

    fn class_element(&mut self, derived: bool) -> PResult<ClassElement> {
        let start = self.token.span.start;
        let mut is_static = false;
        if self.at_word("static") {
            let next = self.peek()?;
            let is_name = matches!(
                next.punct(),
                Some(Punct::LParen | Punct::Eq | Punct::Semicolon | Punct::RBrace)
            ) || (next.newline_before
                && !matches!(
                    next.kind,
                    TokenKind::Name { .. }
                        | TokenKind::PrivateName(_)
                        | TokenKind::String { .. }
                        | TokenKind::Number { .. }
                        | TokenKind::BigInt { .. }
                )
                && !next.is(Punct::LBracket)
                && !next.is(Punct::Star)
                && !next.is(Punct::LBrace));
            if !is_name {
                self.advance()?;
                is_static = true;
                if self.at(Punct::LBrace) {
                    return self.static_block(start);
                }
            }
        }
        let (is_async, is_generator, accessor) = self.method_modifiers()?;
        let key_start = self.token.span.start;
        let (key, _) = self.property_key(true)?;
        let key_span = self.span_from(key_start);
        let key_name = key.static_name();
        let named =
            |n: &str| key_name.as_deref() == Some(&n.encode_utf16().collect::<Vec<_>>()[..]);
        if self.at(Punct::LParen) || is_async || is_generator || accessor.is_some() {
            let mut kind = accessor.unwrap_or(MethodKind::Method);
            let is_constructor =
                !is_static && named("constructor") && !matches!(key, PropertyKey::Computed(_));
            if is_constructor {
                if kind != MethodKind::Method || is_async || is_generator {
                    return Err(self.error(key_span, "special constructor method"));
                }
                kind = MethodKind::Constructor;
            }
            if is_static && named("prototype") && !matches!(key, PropertyKey::Computed(_)) {
                return Err(self.error(key_span, "static method named `prototype`"));
            }
            if let PropertyKey::Private(id) = &key {
                let private_kind = match kind {
                    MethodKind::Get => PrivateKind::Getter,
                    MethodKind::Set => PrivateKind::Setter,
                    _ => PrivateKind::Other,
                };
                self.declare_private(&id.name, id.span, private_kind, is_static)?;
            }
            let function = self.method(
                start,
                kind,
                is_async,
                is_generator,
                is_constructor && derived,
            )?;
            return Ok(ClassElement::Method {
                key,
                function: Box::new(function),
                kind,
                is_static,
                span: self.span_from(start),
            });
        }
        // A field.
        if !matches!(key, PropertyKey::Computed(_))
            && (named("constructor") || (is_static && named("prototype")))
        {
            return Err(self.error(key_span, "invalid field name"));
        }
        if let PropertyKey::Private(id) = &key {
            self.declare_private(&id.name, id.span, PrivateKind::Other, is_static)?;
        }
        let value = if self.eat(Punct::Eq)? {
            Some(self.field_initializer()?)
        } else {
            None
        };
        self.semicolon()?;
        Ok(ClassElement::Field {
            key,
            value,
            is_static,
            span: self.span_from(start),
        })
    }

    /// A field initializer: a function-like context where `arguments` is
    /// forbidden and `super.x` allowed.
    fn field_initializer(&mut self) -> PResult<Expression> {
        let saved_ctx = self.ctx;
        let saved_jumps = std::mem::take(&mut self.jumps);
        self.ctx = Context {
            strict: true,
            in_function: false,
            is_async: false,
            is_generator: false,
            new_target: true,
            super_property: true,
            super_call: false,
            no_arguments: true,
            static_block: false,
        };
        let value = self.assignment(false);
        self.ctx = saved_ctx;
        self.jumps = saved_jumps;
        value
    }

    fn static_block(&mut self, start: u32) -> PResult<ClassElement> {
        let saved_ctx = self.ctx;
        let saved_jumps = std::mem::take(&mut self.jumps);
        self.ctx = Context {
            strict: true,
            in_function: false,
            is_async: false,
            is_generator: false,
            new_target: true,
            super_property: true,
            super_call: false,
            no_arguments: true,
            static_block: true,
        };
        let body = self.block(ScopeKind::Function);
        self.ctx = saved_ctx;
        self.jumps = saved_jumps;
        Ok(ClassElement::StaticBlock(body?, self.span_from(start)))
    }

    // ---- modules --------------------------------------------------------

    fn module_string(&mut self) -> PResult<Vec<u16>> {
        let token = self.advance()?;
        match token.kind {
            TokenKind::String {
                value,
                legacy_escape,
            } => {
                if legacy_escape {
                    return Err(self.error(token.span, "octal escape sequence in strict mode"));
                }
                Ok(value)
            }
            _ => Err(self.error(token.span, "expected a module specifier string")),
        }
    }

    fn module_export_name(&mut self) -> PResult<ModuleExportName> {
        let token = self.advance()?;
        match token.kind {
            TokenKind::Name { name, .. } => Ok(ModuleExportName::Identifier(Identifier {
                name,
                span: token.span,
            })),
            TokenKind::String {
                value,
                legacy_escape,
            } => {
                if legacy_escape {
                    return Err(self.error(token.span, "octal escape sequence in strict mode"));
                }
                if !well_formed(&value) {
                    return Err(self.error(token.span, "export name is not well-formed Unicode"));
                }
                Ok(ModuleExportName::String(value, token.span))
            }
            _ => Err(self.error(token.span, "expected an export name")),
        }
    }

    fn import_attributes(&mut self) -> PResult<Vec<ImportAttribute>> {
        let mut attributes = Vec::new();
        if !self.at_word("with") {
            return Ok(attributes);
        }
        self.advance()?;
        self.expect(Punct::LBrace)?;
        let mut keys = HashSet::new();
        while !self.at(Punct::RBrace) {
            let start = self.token.span.start;
            let token = self.advance()?;
            let key: Vec<u16> = match token.kind {
                TokenKind::Name { name, .. } => name.encode_utf16().collect(),
                TokenKind::String { value, .. } => value,
                _ => return Err(self.error(token.span, "expected an attribute key")),
            };
            if !keys.insert(key.clone()) {
                return Err(self.error(token.span, "duplicate import attribute"));
            }
            self.expect(Punct::Colon)?;
            let value = self.module_string()?;
            attributes.push(ImportAttribute {
                key,
                value,
                span: self.span_from(start),
            });
            if !self.eat(Punct::Comma)? {
                break;
            }
        }
        self.expect(Punct::RBrace)?;
        Ok(attributes)
    }

    fn import_binding(&mut self) -> PResult<Identifier> {
        let id = self.binding_identifier()?;
        self.declare_lexical(&id.name, id.span, false)?;
        Ok(id)
    }

    fn import_declaration(&mut self) -> PResult<Statement> {
        let start = self.token.span.start;
        let token = self.advance()?;
        if self.scopes.len() != 1 {
            return Err(self.error(token.span, "`import` outside module top level"));
        }
        let mut specifiers = Vec::new();
        if !matches!(self.token.kind, TokenKind::String { .. }) {
            if matches!(self.token.kind, TokenKind::Name { .. }) {
                specifiers.push(ImportSpecifier::Default(self.import_binding()?));
                if !self.eat(Punct::Comma)? {
                    self.expect_word("from")?;
                    return self.import_tail(start, specifiers);
                }
            }
            if self.eat(Punct::Star)? {
                self.expect_word("as")?;
                specifiers.push(ImportSpecifier::Namespace(self.import_binding()?));
            } else if self.eat(Punct::LBrace)? {
                while !self.at(Punct::RBrace) {
                    let imported = self.module_export_name()?;
                    let local = if self.eat_word("as")? {
                        self.import_binding()?
                    } else {
                        match &imported {
                            ModuleExportName::Identifier(id) => {
                                self.check_identifier(&id.name, id.span, true)?;
                                self.declare_lexical(&id.name, id.span, false)?;
                                id.clone()
                            }
                            ModuleExportName::String(_, span) => {
                                return Err(self.error(*span, "string import name needs `as`"));
                            }
                        }
                    };
                    specifiers.push(ImportSpecifier::Named { imported, local });
                    if !self.eat(Punct::Comma)? {
                        break;
                    }
                }
                self.expect(Punct::RBrace)?;
            } else if specifiers.is_empty() {
                return Err(self.unexpected());
            }
            self.expect_word("from")?;
        }
        self.import_tail(start, specifiers)
    }

    fn import_tail(&mut self, start: u32, specifiers: Vec<ImportSpecifier>) -> PResult<Statement> {
        let source = self.module_string()?;
        let attributes = self.import_attributes()?;
        self.semicolon()?;
        Ok(stmt(
            StatementKind::Import(ImportDeclaration {
                specifiers,
                source,
                attributes,
            }),
            self.span_from(start),
        ))
    }

    fn export_declaration(&mut self) -> PResult<Statement> {
        let start = self.token.span.start;
        self.advance()?;
        let declaration = if self.eat(Punct::Star)? {
            let exported = if self.eat_word("as")? {
                let name = self.module_export_name()?;
                self.export_module_name(&name)?;
                Some(name)
            } else {
                None
            };
            self.expect_word("from")?;
            let source = self.module_string()?;
            let attributes = self.import_attributes()?;
            self.semicolon()?;
            ExportDeclaration::All {
                exported,
                source,
                attributes,
            }
        } else if self.eat(Punct::LBrace)? {
            let mut specifiers = Vec::new();
            while !self.at(Punct::RBrace) {
                let local = self.module_export_name()?;
                let exported = if self.eat_word("as")? {
                    self.module_export_name()?
                } else {
                    local.clone()
                };
                specifiers.push(ExportSpecifier { local, exported });
                if !self.eat(Punct::Comma)? {
                    break;
                }
            }
            self.expect(Punct::RBrace)?;
            let (source, attributes) = if self.eat_word("from")? {
                let source = self.module_string()?;
                (Some(source), self.import_attributes()?)
            } else {
                (None, Vec::new())
            };
            for s in &specifiers {
                self.export_module_name(&s.exported)?;
                if source.is_none() {
                    // Local exports name bindings, not strings or keywords.
                    match &s.local {
                        ModuleExportName::Identifier(id) => {
                            if is_reserved_word(&id.name)
                                || matches!(
                                    &*id.name,
                                    "await"
                                        | "yield"
                                        | "let"
                                        | "static"
                                        | "implements"
                                        | "interface"
                                        | "package"
                                        | "private"
                                        | "protected"
                                        | "public"
                                )
                            {
                                return Err(self.error(
                                    id.span,
                                    format!("cannot export reserved word `{}`", id.name),
                                ));
                            }
                            self.export_refs.push((id.name.clone(), id.span));
                        }
                        ModuleExportName::String(_, span) => {
                            return Err(self.error(*span, "string local name without `from`"));
                        }
                    }
                }
            }
            self.semicolon()?;
            ExportDeclaration::Named {
                specifiers,
                source,
                attributes,
            }
        } else if self.at_word("default") {
            let default = self.advance()?;
            self.export_name("default".encode_utf16().collect(), default.span)?;
            let fstart = self.token.span.start;
            if self.at_word("function")
                || (self.at_word("async") && {
                    let next = self.peek()?;
                    next.is_word("function") && !next.newline_before
                })
            {
                let is_async = self.eat_word("async")?;
                let function = self.function_declaration_named(fstart, is_async, false, false)?;
                ExportDeclaration::Default(DefaultExport::Function(Box::new(function)))
            } else if self.at_word("class") {
                let class = self.class_named(true, false)?;
                ExportDeclaration::Default(DefaultExport::Class(Box::new(class)))
            } else {
                let e = self.assignment(false)?;
                self.semicolon()?;
                ExportDeclaration::Default(DefaultExport::Expression(e))
            }
        } else {
            let statement_start = self.token.span.start;
            let declaration = if self.at_word("var") {
                self.statement(Position::ListItem)?
            } else if self.at_word("function")
                || self.at_word("class")
                || self.at_word("const")
                || self.at_word("let")
                || self.at_word("async")
            {
                self.statement_list_item()?
            } else {
                return Err(self.unexpected());
            };
            let mut names = Vec::new();
            match &declaration.kind {
                StatementKind::Variable(v) => {
                    for d in &v.declarations {
                        Self::bound_names(&d.target, &mut names);
                    }
                }
                StatementKind::Function(f) => {
                    names.extend(f.id.iter().map(|id| (id.name.clone(), id.span)))
                }
                StatementKind::Class(c) => {
                    names.extend(c.id.iter().map(|id| (id.name.clone(), id.span)))
                }
                _ => {
                    return Err(
                        self.error(self.span_from(statement_start), "expected a declaration")
                    );
                }
            }
            for (name, span) in names {
                self.export_name(name.encode_utf16().collect(), span)?;
            }
            ExportDeclaration::Declaration(Box::new(declaration))
        };
        Ok(stmt(
            StatementKind::Export(declaration),
            self.span_from(start),
        ))
    }

    fn export_module_name(&mut self, name: &ModuleExportName) -> PResult<()> {
        match name {
            ModuleExportName::Identifier(id) => {
                self.export_name(id.name.encode_utf16().collect(), id.span)
            }
            ModuleExportName::String(s, span) => self.export_name(s.clone(), *span),
        }
    }
}

/// What kind of function body is being parsed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FunctionKind {
    Normal,
    Method { kind: MethodKind, super_call: bool },
}
