//! Expressions (ECMA-262 §13), including the cover grammars for arrow
//! parameters and destructuring assignment.

use super::{Cover, PResult, Parser};
use crate::ast::{
    Argument, ArrayElement, ArrowBody, ArrowFunction, AssignOp, BinaryOp, Expression,
    ExpressionKind, Identifier, LogicalOp, MemberProperty, MethodKind, ObjectProperty, Pattern,
    PatternKind, PropertyKey, Template, UnaryOp, UpdateOp,
};
use crate::source::Span;
use crate::token::{Punct, TokenKind};

/// Words that can never be identifiers (ECMA-262 §12.7.2 `ReservedWord`
/// minus `await` and `yield`, which depend on context).
pub(super) fn is_reserved_word(name: &str) -> bool {
    matches!(
        name,
        "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "import"
            | "in"
            | "instanceof"
            | "new"
            | "null"
            | "return"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
    )
}

/// Words reserved only in strict mode code (§13.1.1).
pub(super) fn is_strict_reserved(name: &str) -> bool {
    matches!(
        name,
        "implements"
            | "interface"
            | "let"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "static"
            | "yield"
    )
}

/// Binary operator precedence; `**` is right-associative.
fn binary_precedence(p: Punct, no_in: bool) -> Option<(u8, BinaryOrLogical)> {
    use BinaryOp as B;
    use BinaryOrLogical::{Binary, Logical};
    use Punct as P;
    Some(match p {
        P::QuestionQuestion => (1, Logical(LogicalOp::Coalesce)),
        P::PipePipe => (2, Logical(LogicalOp::Or)),
        P::AmpAmp => (3, Logical(LogicalOp::And)),
        P::Pipe => (4, Binary(B::BitOr)),
        P::Caret => (5, Binary(B::BitXor)),
        P::Amp => (6, Binary(B::BitAnd)),
        P::EqEq => (7, Binary(B::Eq)),
        P::NotEq => (7, Binary(B::NotEq)),
        P::EqEqEq => (7, Binary(B::StrictEq)),
        P::NotEqEq => (7, Binary(B::StrictNotEq)),
        P::Lt => (8, Binary(B::Lt)),
        P::Gt => (8, Binary(B::Gt)),
        P::LtEq => (8, Binary(B::LtEq)),
        P::GtEq => (8, Binary(B::GtEq)),
        P::Shl => (9, Binary(B::Shl)),
        P::Sar => (9, Binary(B::Sar)),
        P::Shr => (9, Binary(B::Shr)),
        P::Plus => (10, Binary(B::Add)),
        P::Minus => (10, Binary(B::Sub)),
        P::Star => (11, Binary(B::Mul)),
        P::Slash => (11, Binary(B::Div)),
        P::Percent => (11, Binary(B::Rem)),
        P::StarStar => (12, Binary(B::Exp)),
        _ => {
            let _ = no_in;
            return None;
        }
    })
}

const RELATIONAL: u8 = 8;
const SHIFT: u8 = 9;

#[derive(Clone, Copy)]
enum BinaryOrLogical {
    Binary(BinaryOp),
    Logical(LogicalOp),
}

fn assign_op(p: Punct) -> Option<AssignOp> {
    use AssignOp as A;
    use Punct as P;
    Some(match p {
        P::Eq => A::Assign,
        P::PlusEq => A::Add,
        P::MinusEq => A::Sub,
        P::StarEq => A::Mul,
        P::SlashEq => A::Div,
        P::PercentEq => A::Rem,
        P::StarStarEq => A::Exp,
        P::ShlEq => A::Shl,
        P::SarEq => A::Sar,
        P::ShrEq => A::Shr,
        P::PipeEq => A::BitOr,
        P::CaretEq => A::BitXor,
        P::AmpEq => A::BitAnd,
        P::AmpAmpEq => A::And,
        P::PipePipeEq => A::Or,
        P::QuestionQuestionEq => A::Coalesce,
        _ => return None,
    })
}

fn expr(kind: ExpressionKind, span: Span) -> Expression {
    Expression { kind, span }
}

fn is_bare_arrow(e: &Expression) -> bool {
    matches!(e.kind, ExpressionKind::Arrow(_))
}

impl Parser<'_> {
    /// `Expression`: a comma-separated sequence.
    pub(super) fn expression(&mut self, no_in: bool) -> PResult<Expression> {
        let start = self.token.span.start;
        let first = self.assignment(no_in)?;
        if !self.at(Punct::Comma) {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat(Punct::Comma)? {
            items.push(self.assignment(no_in)?);
        }
        Ok(expr(ExpressionKind::Sequence(items), self.span_from(start)))
    }

    /// [`Self::expression`] whose first item keeps cover errors pending.
    pub(super) fn expression_cover(&mut self, no_in: bool) -> PResult<Expression> {
        let start = self.token.span.start;
        let first = self.assignment_cover(no_in)?;
        if !self.at(Punct::Comma) {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat(Punct::Comma)? {
            items.push(self.assignment(no_in)?);
        }
        Ok(expr(ExpressionKind::Sequence(items), self.span_from(start)))
    }

    /// `AssignmentExpression` used as a value: cover-grammar errors (such
    /// as `{ a = 1 }`) are reported here unless it became a pattern.
    pub(super) fn assignment(&mut self, no_in: bool) -> PResult<Expression> {
        let outer = std::mem::take(&mut self.cover);
        let result = self.assignment_cover(no_in);
        let inner = std::mem::replace(&mut self.cover, outer);
        let e = result?;
        self.report_cover(&inner)?;
        self.merge_positions(inner);
        Ok(e)
    }

    /// Report pattern-only constructs in an expression that stayed one.
    pub(super) fn report_cover(&self, cover: &Cover) -> PResult<()> {
        if let Some(span) = cover.shorthand_init {
            return Err(self.error(span, "shorthand property initializer outside a pattern"));
        }
        if let Some(span) = cover.duplicate_proto {
            return Err(self.error(span, "duplicate `__proto__` property"));
        }
        Ok(())
    }

    /// Keep yield/await positions (arrow parameter checks need them) but
    /// not pattern-only errors, which the caller already reported.
    pub(super) fn merge_positions(&mut self, inner: Cover) {
        self.cover.yield_expr = self.cover.yield_expr.or(inner.yield_expr);
        self.cover.await_expr = self.cover.await_expr.or(inner.await_expr);
        self.cover.await_ident = self.cover.await_ident.or(inner.await_ident);
    }

    /// `AssignmentExpression` inside a construct that may itself become a
    /// pattern (array/object literal members, parenthesized lists, async
    /// arrow arguments): cover-grammar errors propagate to the caller.
    pub(super) fn assignment_cover(&mut self, no_in: bool) -> PResult<Expression> {
        self.enter()?;
        let result = self.assignment_inner(no_in);
        self.leave();
        result
    }

    fn assignment_inner(&mut self, no_in: bool) -> PResult<Expression> {
        let start = self.token.span.start;
        if self.ctx.is_generator && self.at_word("yield") {
            return self.yield_expression(no_in);
        }
        // Arrow functions start here and nowhere else in an operand.
        let saved_arrow = self.arrow_start.replace(start);
        let saved_no_in = std::mem::replace(&mut self.no_in_arrow_body, no_in);
        let lhs = self.conditional(no_in);
        self.arrow_start = saved_arrow;
        self.no_in_arrow_body = saved_no_in;
        let lhs = lhs?;
        if is_bare_arrow(&lhs) {
            return Ok(lhs);
        }
        let Some(op) = self.token.punct().and_then(assign_op) else {
            return Ok(lhs);
        };
        let target = if op == AssignOp::Assign
            && matches!(
                lhs.kind,
                ExpressionKind::Object(_) | ExpressionKind::Array(_)
            ) {
            let pattern = self.assignment_pattern_from(lhs)?;
            // The literal became a pattern, so its cover errors are moot.
            self.cover.shorthand_init = None;
            self.cover.duplicate_proto = None;
            pattern
        } else {
            self.simple_target(lhs, "invalid assignment target")?
        };
        self.advance()?;
        let value = self.assignment(no_in)?;
        Ok(expr(
            ExpressionKind::Assignment {
                op,
                target: Box::new(target),
                value: Box::new(value),
            },
            self.span_from(start),
        ))
    }

    fn yield_expression(&mut self, no_in: bool) -> PResult<Expression> {
        let start = self.token.span.start;
        let token = self.advance()?;
        if let TokenKind::Name { escaped: true, .. } = token.kind {
            return Err(self.error(token.span, "escaped `yield` keyword"));
        }
        self.cover.yield_expr.get_or_insert(token.span);
        let delegate = !self.token.newline_before && self.eat(Punct::Star)?;
        let ends = self.token.newline_before && !delegate
            || self.at_eof()
            || matches!(
                self.token.punct(),
                Some(
                    Punct::RParen
                        | Punct::RBracket
                        | Punct::RBrace
                        | Punct::Comma
                        | Punct::Semicolon
                        | Punct::Colon
                        | Punct::Question
                )
            )
            || (no_in && self.at_word("in"));
        let argument = if ends && !delegate {
            None
        } else {
            Some(Box::new(self.assignment(no_in)?))
        };
        Ok(expr(
            ExpressionKind::Yield { argument, delegate },
            self.span_from(start),
        ))
    }

    fn conditional(&mut self, no_in: bool) -> PResult<Expression> {
        let start = self.token.span.start;
        let test = self.binary(0, no_in)?;
        if is_bare_arrow(&test) || !self.at(Punct::Question) {
            return Ok(test);
        }
        self.advance()?;
        let consequent = self.assignment(false)?;
        self.expect(Punct::Colon)?;
        let alternate = self.assignment(no_in)?;
        Ok(expr(
            ExpressionKind::Conditional {
                test: Box::new(test),
                consequent: Box::new(consequent),
                alternate: Box::new(alternate),
            },
            self.span_from(start),
        ))
    }

    /// Precedence climbing over binary and logical operators. Left
    /// associative chains are folded in a loop; only `**` recurses on its
    /// right operand, and that recursion counts toward the depth limit.
    fn binary(&mut self, min: u8, no_in: bool) -> PResult<Expression> {
        let start = self.token.span.start;
        let mut left = if let TokenKind::PrivateName(name) = &self.token.kind {
            // `#x in obj` (§13.10): a private name only starts a relational
            // `in` operand.
            let name = Identifier {
                name: name.clone(),
                span: self.token.span,
            };
            if min > RELATIONAL || no_in {
                return Err(self.unexpected());
            }
            self.advance()?;
            if !self.at_word("in") {
                return Err(self.unexpected());
            }
            self.advance()?;
            self.reference_private(&name.name, name.span)?;
            let right = self.binary(SHIFT, no_in)?;
            expr(
                ExpressionKind::PrivateIn {
                    name,
                    right: Box::new(right),
                },
                self.span_from(start),
            )
        } else {
            self.unary()?
        };
        loop {
            if is_bare_arrow(&left) {
                return Ok(left);
            }
            let (precedence, op) = match &self.token.kind {
                TokenKind::Punct(p) => match binary_precedence(*p, no_in) {
                    Some(found) => found,
                    None => break,
                },
                TokenKind::Name {
                    name,
                    escaped: false,
                } if &**name == "instanceof" => {
                    (RELATIONAL, BinaryOrLogical::Binary(BinaryOp::Instanceof))
                }
                TokenKind::Name {
                    name,
                    escaped: false,
                } if &**name == "in" && !no_in => {
                    (RELATIONAL, BinaryOrLogical::Binary(BinaryOp::In))
                }
                _ => break,
            };
            if precedence < min {
                break;
            }
            self.advance()?;
            let right = if matches!(op, BinaryOrLogical::Binary(BinaryOp::Exp)) {
                self.enter()?;
                let right = self.binary(precedence, no_in);
                self.leave();
                right?
            } else {
                self.binary(precedence + 1, no_in)?
            };
            let span = self.span_from(start);
            left = match op {
                BinaryOrLogical::Binary(op) => expr(
                    ExpressionKind::Binary {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                    span,
                ),
                BinaryOrLogical::Logical(op) => {
                    // `??` does not mix with `&&`/`||` without parentheses.
                    let mixes = |e: &Expression, coalesce: bool| match &e.kind {
                        ExpressionKind::Logical { op, .. } => {
                            (*op == LogicalOp::Coalesce) != coalesce
                        }
                        _ => false,
                    };
                    let coalesce = op == LogicalOp::Coalesce;
                    if mixes(&left, coalesce) || mixes(&right, coalesce) {
                        return Err(
                            self.error(span, "`??` mixed with `&&` or `||` needs parentheses")
                        );
                    }
                    expr(
                        ExpressionKind::Logical {
                            op,
                            left: Box::new(left),
                            right: Box::new(right),
                        },
                        span,
                    )
                }
            };
        }
        Ok(left)
    }

    fn unary(&mut self) -> PResult<Expression> {
        let start = self.token.span.start;
        let op = match &self.token.kind {
            TokenKind::Punct(Punct::Minus) => Some(UnaryOp::Minus),
            TokenKind::Punct(Punct::Plus) => Some(UnaryOp::Plus),
            TokenKind::Punct(Punct::Bang) => Some(UnaryOp::Not),
            TokenKind::Punct(Punct::Tilde) => Some(UnaryOp::BitNot),
            TokenKind::Name {
                name,
                escaped: false,
            } => match &**name {
                "typeof" => Some(UnaryOp::Typeof),
                "void" => Some(UnaryOp::Void),
                "delete" => Some(UnaryOp::Delete),
                _ => None,
            },
            _ => None,
        };
        if let Some(op) = op {
            self.advance()?;
            self.enter()?;
            let argument = self.unary();
            self.leave();
            let argument = argument?;
            if op == UnaryOp::Delete {
                self.check_delete(&argument)?;
            }
            return self.no_exponent(expr(
                ExpressionKind::Unary {
                    op,
                    argument: Box::new(argument),
                },
                self.span_from(start),
            ));
        }
        if self.at(Punct::PlusPlus) || self.at(Punct::MinusMinus) {
            let op = if self.at(Punct::PlusPlus) {
                UpdateOp::Increment
            } else {
                UpdateOp::Decrement
            };
            self.advance()?;
            self.enter()?;
            let argument = self.unary();
            self.leave();
            let target = self.simple_target(argument?, "invalid update target")?;
            return Ok(expr(
                ExpressionKind::Update {
                    op,
                    prefix: true,
                    target: Box::new(target),
                },
                self.span_from(start),
            ));
        }
        if self.at_await_operator()? {
            let token = self.advance()?;
            if self.ctx.static_block {
                return Err(self.error(token.span, "`await` in a class static block"));
            }
            self.cover.await_expr.get_or_insert(token.span);
            self.enter()?;
            let argument = self.unary();
            self.leave();
            return self.no_exponent(expr(
                ExpressionKind::Await(Box::new(argument?)),
                self.span_from(start),
            ));
        }
        self.postfix()
    }

    /// Whether the current `await` is the operator: inside async functions
    /// and at module top level.
    fn at_await_operator(&mut self) -> PResult<bool> {
        if !self.at_word("await") {
            return Ok(false);
        }
        if self.ctx.is_async {
            return Ok(true);
        }
        if self.ctx.static_block {
            return Err(self.error(self.token.span, "`await` in a class static block"));
        }
        Ok(false)
    }

    /// A unary expression cannot be the base of `**` (§13.6).
    fn no_exponent(&self, e: Expression) -> PResult<Expression> {
        if self.at(Punct::StarStar) {
            return Err(self.error(
                self.token.span,
                "unary operator before `**` needs parentheses",
            ));
        }
        Ok(e)
    }

    fn check_delete(&self, argument: &Expression) -> PResult<()> {
        let mut inner = argument;
        while let ExpressionKind::Parenthesized(e) = &inner.kind {
            inner = e;
        }
        match &inner.kind {
            ExpressionKind::Identifier(_) if self.ctx.strict => Err(self.error(
                argument.span,
                "`delete` of an unqualified name in strict mode",
            )),
            ExpressionKind::Member {
                property: MemberProperty::Private(_),
                ..
            } => Err(self.error(argument.span, "`delete` of a private field")),
            ExpressionKind::OptionalChain(chain)
                if matches!(
                    chain.kind,
                    ExpressionKind::Member {
                        property: MemberProperty::Private(_),
                        ..
                    }
                ) =>
            {
                Err(self.error(argument.span, "`delete` of a private field"))
            }
            _ => Ok(()),
        }
    }

    fn postfix(&mut self) -> PResult<Expression> {
        let start = self.token.span.start;
        let e = self.left_hand_side()?;
        if is_bare_arrow(&e) {
            return Ok(e);
        }
        if (self.at(Punct::PlusPlus) || self.at(Punct::MinusMinus)) && !self.token.newline_before {
            let op = if self.at(Punct::PlusPlus) {
                UpdateOp::Increment
            } else {
                UpdateOp::Decrement
            };
            let target = self.simple_target(e, "invalid update target")?;
            self.advance()?;
            return Ok(expr(
                ExpressionKind::Update {
                    op,
                    prefix: false,
                    target: Box::new(target),
                },
                self.span_from(start),
            ));
        }
        Ok(e)
    }

    /// `LeftHandSideExpression`: `new`, member accesses, calls, optional
    /// chains and tagged templates.
    pub(super) fn left_hand_side(&mut self) -> PResult<Expression> {
        let start = self.token.span.start;
        let base = if self.at_word("new") {
            self.new_expression()?
        } else if self.at_word("super") {
            self.super_expression()?
        } else if self.at_word("import") {
            self.import_expression()?
        } else {
            self.primary()?
        };
        if is_bare_arrow(&base) {
            return Ok(base);
        }
        self.call_tail(start, base, true)
    }

    /// Member, call, optional-chain and tagged-template suffixes. Without
    /// `calls` (a `new` callee), it stops before `(` and rejects `?.`.
    fn call_tail(&mut self, start: u32, mut e: Expression, calls: bool) -> PResult<Expression> {
        let mut in_chain = false;
        loop {
            let span_start = start;
            if self.eat(Punct::Dot)? {
                let property = self.member_name()?;
                e = expr(
                    ExpressionKind::Member {
                        object: Box::new(e),
                        property,
                        optional: false,
                    },
                    self.span_from(span_start),
                );
            } else if self.at(Punct::QuestionDot) {
                if !calls {
                    return Err(self.error(self.token.span, "optional chain in a `new` callee"));
                }
                self.advance()?;
                in_chain = true;
                if self.at(Punct::LParen) {
                    let arguments = self.arguments()?;
                    e = expr(
                        ExpressionKind::Call {
                            callee: Box::new(e),
                            arguments,
                            optional: true,
                        },
                        self.span_from(span_start),
                    );
                } else if self.eat(Punct::LBracket)? {
                    let property = self.expression(false)?;
                    self.expect(Punct::RBracket)?;
                    e = expr(
                        ExpressionKind::Member {
                            object: Box::new(e),
                            property: MemberProperty::Computed(Box::new(property)),
                            optional: true,
                        },
                        self.span_from(span_start),
                    );
                } else if matches!(self.token.kind, TokenKind::Template(_)) {
                    return Err(self.error(self.token.span, "tagged template in an optional chain"));
                } else {
                    let property = self.member_name()?;
                    e = expr(
                        ExpressionKind::Member {
                            object: Box::new(e),
                            property,
                            optional: true,
                        },
                        self.span_from(span_start),
                    );
                }
            } else if self.eat(Punct::LBracket)? {
                let property = self.expression(false)?;
                self.expect(Punct::RBracket)?;
                e = expr(
                    ExpressionKind::Member {
                        object: Box::new(e),
                        property: MemberProperty::Computed(Box::new(property)),
                        optional: false,
                    },
                    self.span_from(span_start),
                );
            } else if matches!(self.token.kind, TokenKind::Template(_)) {
                if in_chain {
                    return Err(self.error(self.token.span, "tagged template in an optional chain"));
                }
                let template = self.template(true)?;
                e = expr(
                    ExpressionKind::TaggedTemplate {
                        tag: Box::new(e),
                        template,
                    },
                    self.span_from(span_start),
                );
            } else if calls && self.at(Punct::LParen) {
                let arguments = self.arguments()?;
                e = expr(
                    ExpressionKind::Call {
                        callee: Box::new(e),
                        arguments,
                        optional: false,
                    },
                    self.span_from(span_start),
                );
            } else {
                break;
            }
        }
        if in_chain {
            let span = e.span;
            e = expr(ExpressionKind::OptionalChain(Box::new(e)), span);
        }
        Ok(e)
    }

    /// The property after `.` or `?.`: any identifier name or `#name`.
    fn member_name(&mut self) -> PResult<MemberProperty> {
        let token = self.advance()?;
        match token.kind {
            TokenKind::Name { name, .. } => Ok(MemberProperty::Name(Identifier {
                name,
                span: token.span,
            })),
            TokenKind::PrivateName(name) => {
                self.reference_private(&name, token.span)?;
                Ok(MemberProperty::Private(Identifier {
                    name,
                    span: token.span,
                }))
            }
            _ => Err(self.error(token.span, "expected a property name")),
        }
    }

    pub(super) fn arguments(&mut self) -> PResult<Vec<Argument>> {
        self.expect(Punct::LParen)?;
        let mut arguments = Vec::new();
        while !self.at(Punct::RParen) {
            if self.eat(Punct::Ellipsis)? {
                arguments.push(Argument::Spread(self.assignment(false)?));
            } else {
                arguments.push(Argument::Expression(self.assignment(false)?));
            }
            if !self.eat(Punct::Comma)? {
                break;
            }
        }
        self.expect(Punct::RParen)?;
        Ok(arguments)
    }

    fn new_expression(&mut self) -> PResult<Expression> {
        let start = self.token.span.start;
        let new = self.advance()?;
        if let TokenKind::Name { escaped: true, .. } = new.kind {
            return Err(self.error(new.span, "escaped `new` keyword"));
        }
        if self.eat(Punct::Dot)? {
            let target = self.advance()?;
            if !target.is_word("target") {
                return Err(self.error(target.span, "expected `new.target`"));
            }
            if !self.ctx.new_target {
                return Err(self.error(self.span_from(start), "`new.target` outside a function"));
            }
            return Ok(expr(ExpressionKind::NewTarget, self.span_from(start)));
        }
        self.enter()?;
        let callee_start = self.token.span.start;
        let callee = if self.at_word("new") {
            self.new_expression()
        } else if self.at_word("super") {
            self.super_expression()
        } else if self.at_word("import") {
            // `new import.meta` is allowed; `new import(...)` is not.
            if self.peek()?.is(Punct::Dot) {
                self.import_expression()
            } else {
                Err(self.error(self.token.span, "`new import(...)` is not allowed"))
            }
        } else {
            let saved = self.arrow_start.take();
            let primary = self.primary();
            self.arrow_start = saved;
            primary
        };
        self.leave();
        let callee = self.call_tail(callee_start, callee?, false)?;
        let arguments = if self.at(Punct::LParen) {
            self.arguments()?
        } else {
            Vec::new()
        };
        Ok(expr(
            ExpressionKind::New {
                callee: Box::new(callee),
                arguments,
            },
            self.span_from(start),
        ))
    }

    fn super_expression(&mut self) -> PResult<Expression> {
        let start = self.token.span.start;
        let token = self.advance()?;
        if let TokenKind::Name { escaped: true, .. } = token.kind {
            return Err(self.error(token.span, "escaped `super` keyword"));
        }
        if self.at(Punct::LParen) {
            if !self.ctx.super_call {
                return Err(self.error(token.span, "`super()` outside a derived constructor"));
            }
            let arguments = self.arguments()?;
            return Ok(expr(
                ExpressionKind::SuperCall(arguments),
                self.span_from(start),
            ));
        }
        if !self.ctx.super_property {
            return Err(self.error(token.span, "`super` outside a method"));
        }
        let property = if self.eat(Punct::Dot)? {
            match self.member_name()? {
                MemberProperty::Private(id) => {
                    return Err(self.error(id.span, "`super.#name` is not allowed"));
                }
                property => property,
            }
        } else if self.eat(Punct::LBracket)? {
            let property = self.expression(false)?;
            self.expect(Punct::RBracket)?;
            MemberProperty::Computed(Box::new(property))
        } else {
            return Err(self.unexpected());
        };
        Ok(expr(
            ExpressionKind::SuperMember(property),
            self.span_from(start),
        ))
    }

    fn import_expression(&mut self) -> PResult<Expression> {
        let start = self.token.span.start;
        let token = self.advance()?;
        if let TokenKind::Name { escaped: true, .. } = token.kind {
            return Err(self.error(token.span, "escaped `import` keyword"));
        }
        if self.eat(Punct::Dot)? {
            let meta = self.advance()?;
            if !meta.is_word("meta") {
                return Err(self.error(meta.span, "expected `import.meta`"));
            }
            if !self.module {
                return Err(self.error(self.span_from(start), "`import.meta` outside a module"));
            }
            return Ok(expr(ExpressionKind::ImportMeta, self.span_from(start)));
        }
        self.expect(Punct::LParen)?;
        let source = self.assignment(false)?;
        let mut options = None;
        if self.eat(Punct::Comma)? && !self.at(Punct::RParen) {
            options = Some(Box::new(self.assignment(false)?));
            self.eat(Punct::Comma)?;
        }
        self.expect(Punct::RParen)?;
        Ok(expr(
            ExpressionKind::ImportCall {
                source: Box::new(source),
                options,
            },
            self.span_from(start),
        ))
    }

    /// A template literal starting at the current token; `tagged` allows
    /// invalid escapes (their cooked value is undefined).
    pub(super) fn template(&mut self, tagged: bool) -> PResult<Template> {
        let mut quasis = Vec::new();
        let mut expressions = Vec::new();
        loop {
            let token = std::mem::replace(&mut self.token.kind, TokenKind::Eof);
            let TokenKind::Template(part) = token else {
                return Err(self.unexpected());
            };
            if !tagged && part.cooked.is_none() {
                return Err(self.error(self.token.span, "invalid escape sequence in template"));
            }
            let tail = part.tail;
            quasis.push(part);
            if tail {
                self.advance()?;
                break;
            }
            // `${`: a substitution follows.
            self.advance()?;
            expressions.push(self.expression(false)?);
            if !self.at(Punct::RBrace) {
                return Err(self.unexpected());
            }
            let brace = self.token.span;
            let next = self.lexer.template_continuation(brace)?;
            self.token = next;
        }
        Ok(Template {
            quasis,
            expressions,
        })
    }

    /// An `IdentifierReference` name, checked for the current context.
    pub(super) fn identifier_reference(&mut self, name: &str, span: Span) -> PResult<Identifier> {
        self.check_identifier(name, span, false)?;
        if name == "arguments" && self.ctx.no_arguments {
            return Err(self.error(span, "`arguments` in a class field or static block"));
        }
        if name == "await" {
            self.cover.await_ident.get_or_insert(span);
        }
        Ok(Identifier {
            name: name.into(),
            span,
        })
    }

    /// Early errors shared by identifier references, bindings and labels.
    /// `binding` adds the strict-mode `eval`/`arguments` rule.
    pub(super) fn check_identifier(&self, name: &str, span: Span, binding: bool) -> PResult<()> {
        if is_reserved_word(name) {
            return Err(self.error(span, format!("`{name}` is a reserved word")));
        }
        if self.ctx.strict && is_strict_reserved(name) {
            return Err(self.error(span, format!("`{name}` is reserved in strict mode")));
        }
        if name == "yield" && self.ctx.is_generator {
            return Err(self.error(span, "`yield` is reserved in generators"));
        }
        if name == "await" && (self.ctx.is_async || self.module || self.ctx.static_block) {
            return Err(self.error(span, "`await` is reserved here"));
        }
        if binding && self.ctx.strict && (name == "eval" || name == "arguments") {
            return Err(self.error(span, format!("cannot bind `{name}` in strict mode")));
        }
        Ok(())
    }

    fn primary(&mut self) -> PResult<Expression> {
        let start = self.token.span.start;
        let span = self.token.span;
        match &self.token.kind {
            TokenKind::Punct(Punct::LParen) => self.parenthesized(),
            TokenKind::Punct(Punct::LBracket) => self.array_literal(),
            TokenKind::Punct(Punct::LBrace) => self.object_literal(),
            TokenKind::Punct(Punct::Slash | Punct::SlashEq) => {
                self.regexp_here()?;
                self.primary()
            }
            TokenKind::Template(_) => {
                let template = self.template(false)?;
                Ok(expr(
                    ExpressionKind::Template(template),
                    self.span_from(start),
                ))
            }
            TokenKind::Number { value, legacy } => {
                let (value, legacy) = (*value, *legacy);
                if legacy && self.ctx.strict {
                    return Err(self.error(span, "legacy octal literal in strict mode"));
                }
                self.advance()?;
                Ok(expr(ExpressionKind::Number(value), span))
            }
            TokenKind::BigInt { radix, digits } => {
                let kind = ExpressionKind::BigInt(*radix, digits.clone());
                self.advance()?;
                Ok(expr(kind, span))
            }
            TokenKind::String {
                value,
                legacy_escape,
            } => {
                if *legacy_escape && self.ctx.strict {
                    return Err(self.error(span, "octal escape sequence in strict mode"));
                }
                let kind = ExpressionKind::String(value.clone());
                self.advance()?;
                Ok(expr(kind, span))
            }
            TokenKind::RegExp { body, flags } => {
                let kind = ExpressionKind::RegExp {
                    body: body.clone(),
                    flags: flags.clone(),
                };
                self.advance()?;
                Ok(expr(kind, span))
            }
            TokenKind::Name { name, escaped } => {
                let (name, escaped) = (name.clone(), *escaped);
                self.name_primary(&name, escaped, span)
            }
            _ => Err(self.unexpected()),
        }
    }

    fn name_primary(&mut self, name: &str, escaped: bool, span: Span) -> PResult<Expression> {
        let start = span.start;
        if !escaped {
            let keyword = match name {
                "this" => Some(ExpressionKind::This),
                "null" => Some(ExpressionKind::Null),
                "true" => Some(ExpressionKind::Boolean(true)),
                "false" => Some(ExpressionKind::Boolean(false)),
                _ => None,
            };
            if let Some(kind) = keyword {
                self.advance()?;
                return Ok(expr(kind, span));
            }
            match name {
                "function" => {
                    let function = self.function_expression(start, false)?;
                    return Ok(expr(
                        ExpressionKind::Function(Box::new(function)),
                        self.span_from(start),
                    ));
                }
                "class" => {
                    let class = self.class(false)?;
                    return Ok(expr(
                        ExpressionKind::Class(Box::new(class)),
                        self.span_from(start),
                    ));
                }
                "async" => {
                    if let Some(e) = self.async_primary(span)? {
                        return Ok(e);
                    }
                }
                _ => {}
            }
        }
        self.advance()?;
        // `x => body`: a single-parameter arrow function.
        if self.arrow_start == Some(start) && self.at(Punct::Arrow) && !self.token.newline_before {
            if name == "await" {
                self.cover.await_ident.get_or_insert(span);
            }
            let param = self.binding_name_for_arrow(name, span)?;
            return self.arrow_function(start, vec![param], None, false);
        }
        let id = self.identifier_reference(name, span)?;
        Ok(expr(ExpressionKind::Identifier(id), span))
    }

    /// `async function`, `async x => y` and `async (...) => y`; `None` when
    /// `async` is a plain identifier here.
    fn async_primary(&mut self, span: Span) -> PResult<Option<Expression>> {
        let start = span.start;
        let next = self.peek()?;
        if next.newline_before {
            return Ok(None);
        }
        if next.is_word("function") {
            self.advance()?;
            let function = self.function_expression(start, true)?;
            return Ok(Some(expr(
                ExpressionKind::Function(Box::new(function)),
                self.span_from(start),
            )));
        }
        if self.arrow_start != Some(start) {
            return Ok(None);
        }
        let arrow_after_name = matches!(next.kind, TokenKind::Name { .. }) && {
            let mut lexer = self.lexer.clone();
            lexer.next_token(crate::lexer::Goal::Div)?;
            let after = lexer.next_token(crate::lexer::Goal::Div)?;
            after.is(Punct::Arrow) && !after.newline_before
        };
        if let (true, TokenKind::Name { name, .. }) = (arrow_after_name, &next.kind) {
            // `async x => y`.
            let name = name.clone();
            self.advance()?;
            let param_span = self.token.span;
            self.advance()?;
            if !self.at(Punct::Arrow) || self.token.newline_before {
                return Err(self.unexpected());
            }
            if name.as_ref() == "await" {
                return Err(self.error(param_span, "`await` as an async arrow parameter"));
            }
            let param = self.binding_name_for_arrow(&name, param_span)?;
            return self
                .arrow_function(start, vec![param], None, true)
                .map(Some);
        }
        if !next.is(Punct::LParen) {
            return Ok(None);
        }
        // `async(...)`: a call, unless `=>` follows (the cover grammar
        // CoverCallExpressionAndAsyncArrowHead).
        self.advance()?;
        let outer = std::mem::take(&mut self.cover);
        let parsed = self.cover_list(true);
        let inner = std::mem::replace(&mut self.cover, outer);
        let (items, rest, trailing_comma) = parsed?;
        if self.at(Punct::Arrow) && !self.token.newline_before {
            if let Some(span) = inner.await_expr.or(inner.await_ident) {
                return Err(self.error(span, "`await` in async arrow parameters"));
            }
            if let Some(span) = inner.yield_expr {
                return Err(self.error(span, "`yield` in arrow parameters"));
            }
            let (params, rest) = self.arrow_params_from(items, rest)?;
            return self.arrow_function(start, params, rest, true).map(Some);
        }
        self.report_cover(&inner)?;
        self.merge_positions(inner);
        let _ = trailing_comma;
        let mut arguments = Vec::with_capacity(items.len());
        for item in items {
            arguments.push(match item {
                CoverItem::Expression(e) => Argument::Expression(e),
                CoverItem::Spread(e) => Argument::Spread(e),
            });
        }
        if let Some((e, _)) = rest {
            arguments.push(Argument::Spread(e));
        }
        let callee = expr(
            ExpressionKind::Identifier(Identifier {
                name: "async".into(),
                span,
            }),
            span,
        );
        Ok(Some(expr(
            ExpressionKind::Call {
                callee: Box::new(callee),
                arguments,
                optional: false,
            },
            self.span_from(start),
        )))
    }

    /// Items of `( ... )` up to and including `)`. With `spread_items`
    /// (call arguments), `...x` may appear anywhere; otherwise only last.
    fn cover_list(&mut self, spread_items: bool) -> PResult<CoverList> {
        self.expect(Punct::LParen)?;
        let mut items = Vec::new();
        let mut rest = None;
        let mut trailing_comma = false;
        while !self.at(Punct::RParen) {
            if self.at(Punct::Ellipsis) {
                let dots = self.advance()?;
                let e = self.assignment_cover(false)?;
                if !self.at(Punct::RParen) {
                    if spread_items {
                        items.push(CoverItem::Spread(e));
                        self.expect(Punct::Comma)?;
                        continue;
                    }
                    return Err(self.unexpected());
                }
                rest = Some((e, dots.span));
                break;
            }
            items.push(CoverItem::Expression(self.assignment_cover(false)?));
            if !self.eat(Punct::Comma)? {
                break;
            }
            trailing_comma = self.at(Punct::RParen);
        }
        self.expect(Punct::RParen)?;
        Ok((items, rest, trailing_comma))
    }

    /// `( ... )`: a parenthesized expression or arrow parameters.
    fn parenthesized(&mut self) -> PResult<Expression> {
        let start = self.token.span.start;
        let arrow_here = self.arrow_start == Some(start);
        let outer = std::mem::take(&mut self.cover);
        let saved_arrow = self.arrow_start.take();
        let parsed = self.cover_list(false);
        self.arrow_start = saved_arrow;
        let inner = std::mem::replace(&mut self.cover, outer);
        let (items, rest, trailing_comma) = parsed?;
        if arrow_here && self.at(Punct::Arrow) && !self.token.newline_before {
            if let Some(span) = inner.yield_expr {
                if self.ctx.is_generator {
                    return Err(self.error(span, "`yield` in arrow parameters"));
                }
            }
            if let Some(span) = inner.await_expr {
                return Err(self.error(span, "`await` in arrow parameters"));
            }
            let (params, rest) = self.arrow_params_from(items, rest)?;
            // `await` as an identifier anywhere inside still matters to an
            // enclosing async arrow head.
            self.merge_positions(inner);
            return self.arrow_function(start, params, rest, false);
        }
        if let Some((_, span)) = rest {
            return Err(self.error(span, "rest element outside arrow parameters"));
        }
        if items.is_empty() || trailing_comma {
            return Err(self.error(self.span_from(start), "expected an expression"));
        }
        self.cover.shorthand_init = self.cover.shorthand_init.or(inner.shorthand_init);
        self.cover.duplicate_proto = self.cover.duplicate_proto.or(inner.duplicate_proto);
        self.merge_positions(inner);
        let mut exprs: Vec<Expression> = items
            .into_iter()
            .map(|item| match item {
                CoverItem::Expression(e) | CoverItem::Spread(e) => e,
            })
            .collect();
        let inner_expr = if exprs.len() == 1 {
            exprs.remove(0)
        } else {
            let span = exprs[0].span.to(exprs[exprs.len() - 1].span);
            expr(ExpressionKind::Sequence(exprs), span)
        };
        Ok(expr(
            ExpressionKind::Parenthesized(Box::new(inner_expr)),
            self.span_from(start),
        ))
    }

    fn array_literal(&mut self) -> PResult<Expression> {
        let start = self.token.span.start;
        self.advance()?;
        let saved_arrow = self.arrow_start.take();
        let result = self.array_elements(start);
        self.arrow_start = saved_arrow;
        let elements = result?;
        Ok(expr(ExpressionKind::Array(elements), self.span_from(start)))
    }

    fn array_elements(&mut self, start: u32) -> PResult<Vec<Option<ArrayElement>>> {
        let mut elements = Vec::new();
        while !self.at(Punct::RBracket) {
            if self.eat(Punct::Comma)? {
                elements.push(None);
                continue;
            }
            if self.eat(Punct::Ellipsis)? {
                elements.push(Some(ArrayElement::Spread(self.assignment_cover(false)?)));
                if self.at(Punct::Comma) {
                    // `[...a, b]` and `[...a,]` cannot be patterns.
                    self.rest_not_last.insert(start);
                }
            } else {
                elements.push(Some(ArrayElement::Expression(
                    self.assignment_cover(false)?,
                )));
            }
            if !self.eat(Punct::Comma)? {
                break;
            }
        }
        self.expect(Punct::RBracket)?;
        Ok(elements)
    }

    fn object_literal(&mut self) -> PResult<Expression> {
        let start = self.token.span.start;
        self.advance()?;
        let saved_arrow = self.arrow_start.take();
        let result = self.object_properties(start);
        self.arrow_start = saved_arrow;
        let properties = result?;
        Ok(expr(
            ExpressionKind::Object(properties),
            self.span_from(start),
        ))
    }

    fn object_properties(&mut self, start: u32) -> PResult<Vec<ObjectProperty>> {
        let mut properties = Vec::new();
        let mut proto_seen = false;
        while !self.at(Punct::RBrace) {
            if self.at(Punct::Ellipsis) {
                self.advance()?;
                properties.push(ObjectProperty::Spread(self.assignment_cover(false)?));
                if self.at(Punct::Comma) {
                    self.rest_not_last.insert(start);
                }
            } else {
                let property = self.object_property()?;
                if let ObjectProperty::Property {
                    key,
                    shorthand: false,
                    span,
                    ..
                } = &property
                {
                    if key.static_name().as_deref() == Some(PROTO) {
                        if proto_seen {
                            // Allowed in patterns, so it is a cover error.
                            self.cover.duplicate_proto.get_or_insert(*span);
                        }
                        proto_seen = true;
                    }
                }
                properties.push(property);
            }
            if !self.eat(Punct::Comma)? {
                break;
            }
        }
        self.expect(Punct::RBrace)?;
        Ok(properties)
    }

    fn object_property(&mut self) -> PResult<ObjectProperty> {
        let start = self.token.span.start;
        let (is_async, is_generator, accessor) = self.method_modifiers()?;
        let (key, key_token) = self.property_key(false)?;
        if is_async || is_generator || accessor.is_some() || self.at(Punct::LParen) {
            let kind = accessor.unwrap_or(MethodKind::Method);
            let function = self.method(start, kind, is_async, is_generator, false)?;
            return Ok(ObjectProperty::Method {
                key,
                function: Box::new(function),
                kind,
                span: self.span_from(start),
            });
        }
        if self.eat(Punct::Colon)? {
            let value = self.assignment_cover(false)?;
            return Ok(ObjectProperty::Property {
                key,
                value,
                shorthand: false,
                span: self.span_from(start),
            });
        }
        // Shorthand `{ a }` or CoverInitializedName `{ a = 1 }`.
        let Some((name, escaped, span)) = key_token else {
            return Err(self.unexpected());
        };
        if escaped && is_reserved_word(&name) {
            return Err(self.error(span, "escaped reserved word"));
        }
        let id = self.identifier_reference(&name, span)?;
        let mut value = expr(ExpressionKind::Identifier(id.clone()), span);
        if self.at(Punct::Eq) {
            let eq = self.advance()?;
            self.cover.shorthand_init.get_or_insert(eq.span);
            let default = self.assignment(false)?;
            value = expr(
                ExpressionKind::Assignment {
                    op: AssignOp::Assign,
                    target: Box::new(Pattern {
                        kind: PatternKind::Identifier(id),
                        span,
                    }),
                    value: Box::new(default),
                },
                self.span_from(start),
            );
        }
        Ok(ObjectProperty::Property {
            key,
            value,
            shorthand: true,
            span: self.span_from(start),
        })
    }

    /// `async`, `*` and `get`/`set` before a method key. A modifier word
    /// followed by `(`, `:`, `,`, `}` or `=` is the key itself.
    pub(super) fn method_modifiers(&mut self) -> PResult<(bool, bool, Option<MethodKind>)> {
        let mut is_async = false;
        let mut accessor = None;
        if self.at_word("async") || self.at_word("get") || self.at_word("set") {
            let next = self.peek()?;
            // `get`/`set` must be followed by a property name (`get *a` is
            // a field named `get`, then a generator after ASI).
            let accessor_without_name = !self.at_word("async") && next.is(Punct::Star);
            let is_key = accessor_without_name
                || matches!(
                    next.punct(),
                    Some(
                        Punct::LParen
                            | Punct::Colon
                            | Punct::Comma
                            | Punct::RBrace
                            | Punct::Eq
                            | Punct::Semicolon
                    )
                )
                || matches!(next.kind, TokenKind::Eof);
            let async_newline = self.at_word("async") && next.newline_before;
            if !is_key && !async_newline {
                let word = self.advance()?;
                if word.is_word("async") {
                    is_async = true;
                } else if word.is_word("get") {
                    accessor = Some(MethodKind::Get);
                } else {
                    accessor = Some(MethodKind::Set);
                }
            }
        }
        let is_generator = accessor.is_none() && self.eat(Punct::Star)?;
        Ok((is_async, is_generator, accessor))
    }

    /// A property name. Returns the key and, for identifier names, the
    /// name, escape flag and span (shorthand properties need them).
    pub(super) fn property_key(
        &mut self,
        private: bool,
    ) -> PResult<(PropertyKey, Option<KeyName>)> {
        let token = self.advance()?;
        let span = token.span;
        Ok(match token.kind {
            TokenKind::Name { name, escaped } => (
                PropertyKey::Identifier(Identifier {
                    name: name.clone(),
                    span,
                }),
                Some((name, escaped, span)),
            ),
            TokenKind::String {
                value,
                legacy_escape,
            } => {
                if legacy_escape && self.ctx.strict {
                    return Err(self.error(span, "octal escape sequence in strict mode"));
                }
                (PropertyKey::String(value, span), None)
            }
            TokenKind::Number { value, legacy } => {
                if legacy && self.ctx.strict {
                    return Err(self.error(span, "legacy octal literal in strict mode"));
                }
                (PropertyKey::Number(value, span), None)
            }
            TokenKind::BigInt { radix, digits } => (PropertyKey::BigInt(radix, digits, span), None),
            TokenKind::Punct(Punct::LBracket) => {
                let e = self.assignment(false)?;
                self.expect(Punct::RBracket)?;
                (PropertyKey::Computed(Box::new(e)), None)
            }
            TokenKind::PrivateName(name) if private => {
                (PropertyKey::Private(Identifier { name, span }), None)
            }
            _ => return Err(self.error(span, "expected a property name")),
        })
    }

    fn binding_name_for_arrow(&mut self, name: &str, span: Span) -> PResult<Pattern> {
        Ok(Pattern {
            kind: PatternKind::Identifier(Identifier {
                name: name.into(),
                span,
            }),
            span,
        })
    }

    /// The parameters are converted; parse `=>` and the body.
    fn arrow_function(
        &mut self,
        start: u32,
        params: Vec<Pattern>,
        rest: Option<Pattern>,
        is_async: bool,
    ) -> PResult<Expression> {
        self.expect(Punct::Arrow)?;
        let saved_ctx = self.ctx;
        let saved_jumps = std::mem::take(&mut self.jumps);
        let saved_cover = std::mem::take(&mut self.cover);
        self.ctx.in_function = true;
        self.ctx.is_async = is_async;
        self.ctx.is_generator = false;
        // Arrows are function boundaries for the static block `await`
        // rules, but not for `arguments` in class fields.
        self.ctx.static_block = false;
        self.push_scope(super::scope::ScopeKind::Function);
        let result = self.arrow_rest(start, params, rest, is_async);
        self.pop_scope();
        self.ctx = saved_ctx;
        self.jumps = saved_jumps;
        self.cover = saved_cover;
        result
    }

    fn arrow_rest(
        &mut self,
        start: u32,
        params: Vec<Pattern>,
        rest: Option<Pattern>,
        is_async: bool,
    ) -> PResult<Expression> {
        let names = self.param_names(&params, rest.as_ref())?;
        self.declare_params(&names);
        let simple = rest.is_none()
            && params
                .iter()
                .all(|p| matches!(p.kind, PatternKind::Identifier(_)));
        let (body, strict) = if self.at(Punct::LBrace) {
            let (body, use_strict) = self.function_body()?;
            if use_strict && !simple {
                return Err(self.error(
                    self.span_from(start),
                    "\"use strict\" in a function with non-simple parameters",
                ));
            }
            (ArrowBody::Block(body), self.ctx.strict)
        } else {
            let saved_arrow = self.arrow_start.take();
            let no_in = self.no_in_arrow_body;
            let body = self.assignment(no_in);
            self.arrow_start = saved_arrow;
            (ArrowBody::Expression(Box::new(body?)), self.ctx.strict)
        };
        // Arrow parameters are always unique and follow the (possibly
        // newly) strict rules.
        self.check_params(&names, true)?;
        Ok(expr(
            ExpressionKind::Arrow(Box::new(ArrowFunction {
                params,
                rest,
                body,
                is_async,
                strict,
                span: self.span_from(start),
            })),
            self.span_from(start),
        ))
    }
}

/// UTF-16 `__proto__`.
const PROTO: &[u16] = &[0x5F, 0x5F, 0x70, 0x72, 0x6F, 0x74, 0x6F, 0x5F, 0x5F];

/// One item of a parenthesized list or async arrow head.
pub(super) enum CoverItem {
    Expression(Expression),
    Spread(Expression),
}

/// Items, a final rest element (with the `...` span) and whether a
/// trailing comma followed the last item.
type CoverList = (Vec<CoverItem>, Option<(Expression, Span)>, bool);

/// An identifier property name: its name, whether it was escaped, and its
/// span (shorthand properties need them).
pub(super) type KeyName = (Box<str>, bool, Span);
