//! Binding patterns and the expression-to-pattern conversions of the cover
//! grammars (ECMA-262 §13.15.5 destructuring assignment, §15.3 arrow
//! parameters), with their early errors.

use std::collections::HashSet;

use super::expression::{CoverItem, is_reserved_word};
use super::{PResult, Parser};
use crate::ast::{
    ArrayElement, AssignOp, Expression, ExpressionKind, Identifier, ObjectProperty, Pattern,
    PatternKind, PatternProperty, PropertyKey,
};
use crate::source::Span;
use crate::token::{Punct, TokenKind};

/// How a converted pattern is used.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    /// Destructuring assignment: member expressions are allowed.
    Assignment,
    /// Arrow parameters: only bindings.
    Binding,
}

impl Parser<'_> {
    /// A binding identifier: a name that may be declared here.
    pub(super) fn binding_identifier(&mut self) -> PResult<Identifier> {
        let token = self.advance()?;
        match token.kind {
            TokenKind::Name { name, .. } => {
                self.check_identifier(&name, token.span, true)?;
                Ok(Identifier {
                    name,
                    span: token.span,
                })
            }
            _ => Err(self.error(token.span, "expected a binding name")),
        }
    }

    /// `BindingElement`: a binding identifier or pattern with an optional
    /// `= default`.
    pub(super) fn binding_element(&mut self) -> PResult<Pattern> {
        let start = self.token.span.start;
        let target = self.binding_target()?;
        if self.eat(Punct::Eq)? {
            let default = self.assignment(false)?;
            return Ok(Pattern {
                kind: PatternKind::Assignment {
                    target: Box::new(target),
                    default: Box::new(default),
                },
                span: self.span_from(start),
            });
        }
        Ok(target)
    }

    /// A binding identifier, object pattern or array pattern.
    pub(super) fn binding_target(&mut self) -> PResult<Pattern> {
        self.enter()?;
        let result = self.binding_target_inner();
        self.leave();
        result
    }

    fn binding_target_inner(&mut self) -> PResult<Pattern> {
        let start = self.token.span.start;
        if self.eat(Punct::LBracket)? {
            let mut elements = Vec::new();
            let mut rest = None;
            while !self.at(Punct::RBracket) {
                if self.eat(Punct::Comma)? {
                    elements.push(None);
                    continue;
                }
                if self.eat(Punct::Ellipsis)? {
                    rest = Some(Box::new(self.binding_target()?));
                    break;
                }
                elements.push(Some(self.binding_element()?));
                if !self.eat(Punct::Comma)? {
                    break;
                }
            }
            self.expect(Punct::RBracket)?;
            return Ok(Pattern {
                kind: PatternKind::Array { elements, rest },
                span: self.span_from(start),
            });
        }
        if self.eat(Punct::LBrace)? {
            let mut properties = Vec::new();
            let mut rest = None;
            while !self.at(Punct::RBrace) {
                if self.eat(Punct::Ellipsis)? {
                    let id = self.binding_identifier()?;
                    let span = id.span;
                    rest = Some(Box::new(Pattern {
                        kind: PatternKind::Identifier(id),
                        span,
                    }));
                    break;
                }
                let property_start = self.token.span.start;
                let (key, key_token) = self.property_key(false)?;
                let (value, shorthand) = if self.eat(Punct::Colon)? {
                    (self.binding_element()?, false)
                } else {
                    let Some((name, _, span)) = key_token else {
                        return Err(self.unexpected());
                    };
                    self.check_identifier(&name, span, true)?;
                    let mut value = Pattern {
                        kind: PatternKind::Identifier(Identifier { name, span }),
                        span,
                    };
                    if self.eat(Punct::Eq)? {
                        let default = self.assignment(false)?;
                        value = Pattern {
                            kind: PatternKind::Assignment {
                                target: Box::new(value),
                                default: Box::new(default),
                            },
                            span: self.span_from(property_start),
                        };
                    }
                    (value, true)
                };
                properties.push(PatternProperty {
                    key,
                    value,
                    shorthand,
                    span: self.span_from(property_start),
                });
                if !self.eat(Punct::Comma)? {
                    break;
                }
            }
            self.expect(Punct::RBrace)?;
            return Ok(Pattern {
                kind: PatternKind::Object { properties, rest },
                span: self.span_from(start),
            });
        }
        let id = self.binding_identifier()?;
        let span = id.span;
        Ok(Pattern {
            kind: PatternKind::Identifier(id),
            span,
        })
    }

    /// The target of `=`-less assignment operators, `++` and `--`: an
    /// identifier or member expression, possibly parenthesized.
    pub(super) fn simple_target(&self, e: Expression, message: &'static str) -> PResult<Pattern> {
        let span = e.span;
        let mut inner = e;
        loop {
            let inner_span = inner.span;
            match std::mem::replace(&mut inner.kind, ExpressionKind::Null) {
                ExpressionKind::Parenthesized(e) => inner = *e,
                ExpressionKind::Identifier(id) => {
                    if self.ctx.strict && (&*id.name == "eval" || &*id.name == "arguments") {
                        return Err(self.error(
                            span,
                            "cannot assign to `eval` or `arguments` in strict mode",
                        ));
                    }
                    return Ok(Pattern {
                        kind: PatternKind::Identifier(id),
                        span,
                    });
                }
                kind @ (ExpressionKind::Member { .. } | ExpressionKind::SuperMember(_)) => {
                    return Ok(Pattern {
                        kind: PatternKind::Member(Box::new(Expression {
                            kind,
                            span: inner_span,
                        })),
                        span,
                    });
                }
                _ => return Err(self.error(span, message)),
            }
        }
    }

    /// Convert an array or object literal to a destructuring assignment
    /// pattern.
    pub(super) fn assignment_pattern_from(&mut self, e: Expression) -> PResult<Pattern> {
        self.pattern_from(e, Target::Assignment, 0)
    }

    fn pattern_from(&mut self, e: Expression, target: Target, depth: u32) -> PResult<Pattern> {
        // Conversion follows the depth the expression was parsed with, so
        // it stays within the limit; this guards the recursion anyway.
        if depth > self.limits.max_depth {
            return Err(super::ParseError::TooDeep(e.span));
        }
        let span = e.span;
        let mut e = e;
        match std::mem::replace(&mut e.kind, ExpressionKind::Null) {
            ExpressionKind::Identifier(id) => {
                if target == Target::Binding {
                    self.check_identifier(&id.name, id.span, true)?;
                } else if self.ctx.strict && (&*id.name == "eval" || &*id.name == "arguments") {
                    return Err(self.error(
                        span,
                        "cannot assign to `eval` or `arguments` in strict mode",
                    ));
                }
                Ok(Pattern {
                    kind: PatternKind::Identifier(id),
                    span,
                })
            }
            kind @ (ExpressionKind::Member { .. } | ExpressionKind::SuperMember(_))
                if target == Target::Assignment =>
            {
                Ok(Pattern {
                    kind: PatternKind::Member(Box::new(Expression { kind, span })),
                    span,
                })
            }
            ExpressionKind::Parenthesized(inner) if target == Target::Assignment => {
                // `[(a)] = x` and `[(a.b)] = x` are allowed; parenthesized
                // literals are not patterns.
                if matches!(
                    inner.kind,
                    ExpressionKind::Object(_) | ExpressionKind::Array(_)
                ) {
                    return Err(self.error(span, "parenthesized pattern"));
                }
                self.simple_target(*inner, "invalid destructuring target")
            }
            ExpressionKind::Assignment {
                op: AssignOp::Assign,
                target: pattern,
                value,
            } => {
                // `[a = 1] = x`: the element's target was already converted
                // as an assignment target; arrow parameters need bindings.
                let pattern = if target == Target::Binding {
                    self.pattern_to_binding(*pattern)?
                } else {
                    if let PatternKind::Identifier(id) = &pattern.kind {
                        if self.ctx.strict && (&*id.name == "eval" || &*id.name == "arguments") {
                            return Err(self.error(
                                id.span,
                                "cannot assign to `eval` or `arguments` in strict mode",
                            ));
                        }
                    }
                    *pattern
                };
                Ok(Pattern {
                    kind: PatternKind::Assignment {
                        target: Box::new(pattern),
                        default: value,
                    },
                    span,
                })
            }
            ExpressionKind::Array(elements) => {
                if self.rest_not_last.contains(&span.start) {
                    return Err(self.error(span, "rest element must be last"));
                }
                let mut out = Vec::with_capacity(elements.len());
                let mut rest = None;
                let count = elements.len();
                for (i, element) in elements.into_iter().enumerate() {
                    match element {
                        None => out.push(None),
                        Some(ArrayElement::Expression(e)) => {
                            out.push(Some(self.pattern_from(e, target, depth + 1)?));
                        }
                        Some(ArrayElement::Spread(e)) => {
                            if i + 1 != count {
                                return Err(self.error(e.span, "rest element must be last"));
                            }
                            if matches!(e.kind, ExpressionKind::Assignment { .. }) {
                                return Err(self.error(e.span, "rest element with an initializer"));
                            }
                            rest = Some(Box::new(self.pattern_from(e, target, depth + 1)?));
                        }
                    }
                }
                Ok(Pattern {
                    kind: PatternKind::Array {
                        elements: out,
                        rest,
                    },
                    span,
                })
            }
            ExpressionKind::Object(properties) => {
                if self.rest_not_last.contains(&span.start) {
                    return Err(self.error(span, "rest element must be last"));
                }
                let mut out = Vec::with_capacity(properties.len());
                let mut rest = None;
                let count = properties.len();
                for (i, property) in properties.into_iter().enumerate() {
                    match property {
                        ObjectProperty::Property {
                            key,
                            value,
                            shorthand,
                            span,
                        } => {
                            if shorthand {
                                if let PropertyKey::Identifier(id) = &key {
                                    if target == Target::Binding || is_reserved_word(&id.name) {
                                        self.check_identifier(&id.name, id.span, true)?;
                                    }
                                }
                            }
                            let value = self.pattern_from(value, target, depth + 1)?;
                            out.push(PatternProperty {
                                key,
                                value,
                                shorthand,
                                span,
                            });
                        }
                        ObjectProperty::Spread(e) => {
                            if i + 1 != count {
                                return Err(self.error(e.span, "rest element must be last"));
                            }
                            // Object rest takes a simple target only.
                            let pattern = match target {
                                Target::Assignment => {
                                    self.simple_target(e, "invalid rest target")?
                                }
                                Target::Binding => self.pattern_from(e, target, depth + 1)?,
                            };
                            if !matches!(
                                pattern.kind,
                                PatternKind::Identifier(_) | PatternKind::Member(_)
                            ) {
                                return Err(self.error(pattern.span, "invalid rest target"));
                            }
                            rest = Some(Box::new(pattern));
                        }
                        ObjectProperty::Method { span, .. } => {
                            return Err(self.error(span, "method in a destructuring pattern"));
                        }
                    }
                }
                Ok(Pattern {
                    kind: PatternKind::Object {
                        properties: out,
                        rest,
                    },
                    span,
                })
            }
            _ => Err(self.error(span, "invalid destructuring target")),
        }
    }

    /// A pattern that was converted as an assignment target, re-checked as
    /// a binding (for arrow parameters such as `([a = 1]) => a`).
    fn pattern_to_binding(&self, pattern: Pattern) -> PResult<Pattern> {
        let mut stack = vec![&pattern];
        while let Some(p) = stack.pop() {
            match &p.kind {
                PatternKind::Identifier(id) => self.check_identifier(&id.name, id.span, true)?,
                PatternKind::Member(_) => {
                    return Err(self.error(p.span, "member expression in a binding pattern"));
                }
                PatternKind::Object { properties, rest } => {
                    stack.extend(properties.iter().map(|prop| &prop.value));
                    stack.extend(rest.as_deref());
                }
                PatternKind::Array { elements, rest } => {
                    stack.extend(elements.iter().flatten());
                    stack.extend(rest.as_deref());
                }
                PatternKind::Assignment { target, .. } => stack.push(target),
            }
        }
        Ok(pattern)
    }

    /// Convert a parenthesized list to arrow parameters.
    pub(super) fn arrow_params_from(
        &mut self,
        items: Vec<CoverItem>,
        rest: Option<(Expression, Span)>,
    ) -> PResult<(Vec<Pattern>, Option<Pattern>)> {
        // Literals in the list become patterns, so their cover errors are
        // moot.
        let mut params = Vec::with_capacity(items.len());
        for item in items {
            match item {
                CoverItem::Expression(e) => {
                    if matches!(e.kind, ExpressionKind::Parenthesized(_)) {
                        return Err(self.error(e.span, "parenthesized arrow parameter"));
                    }
                    params.push(self.pattern_from(e, Target::Binding, 0)?);
                }
                CoverItem::Spread(e) => {
                    return Err(self.error(e.span, "rest parameter must be last"));
                }
            }
        }
        let rest = match rest {
            Some((e, _)) => {
                if matches!(e.kind, ExpressionKind::Assignment { .. }) {
                    return Err(self.error(e.span, "rest parameter with an initializer"));
                }
                Some(self.pattern_from(e, Target::Binding, 0)?)
            }
            None => None,
        };
        Ok((params, rest))
    }

    /// The names a pattern binds, in source order.
    pub(super) fn bound_names(pattern: &Pattern, out: &mut Vec<(Box<str>, Span)>) {
        let mut stack = vec![pattern];
        let start = out.len();
        while let Some(p) = stack.pop() {
            match &p.kind {
                PatternKind::Identifier(id) => out.push((id.name.clone(), id.span)),
                PatternKind::Member(_) => {}
                PatternKind::Object { properties, rest } => {
                    stack.extend(rest.as_deref());
                    stack.extend(properties.iter().rev().map(|prop| &prop.value));
                }
                PatternKind::Array { elements, rest } => {
                    stack.extend(rest.as_deref());
                    stack.extend(elements.iter().rev().flatten());
                }
                PatternKind::Assignment { target, .. } => stack.push(target),
            }
        }
        out[start..].sort_by_key(|(_, span)| span.start);
    }

    /// Bound names of a parameter list.
    pub(super) fn param_names(
        &self,
        params: &[Pattern],
        rest: Option<&Pattern>,
    ) -> PResult<Vec<(Box<str>, Span)>> {
        let mut names = Vec::new();
        for p in params.iter().chain(rest) {
            Self::bound_names(p, &mut names);
        }
        Ok(names)
    }

    /// Parameter early errors once the body's strictness is known: names
    /// valid as bindings, and unique when `unique` (strict code, arrows,
    /// methods and non-simple lists).
    pub(super) fn check_params(&self, names: &[(Box<str>, Span)], unique: bool) -> PResult<()> {
        let mut seen = HashSet::new();
        for (name, span) in names {
            self.check_identifier(name, *span, true)?;
            if (unique || self.ctx.strict) && !seen.insert(name.clone()) {
                return Err(self.error(*span, format!("duplicate parameter `{name}`")));
            }
        }
        Ok(())
    }
}
