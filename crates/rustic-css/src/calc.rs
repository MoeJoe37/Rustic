//! Math functions: `calc()`, `min()`, `max()` and `clamp()` (CSS Values and
//! Units Level 4 §10, Editor's Draft reviewed 2026-11-08).
//!
//! A math function parses into a calculation tree, is type-checked against
//! the context it appears in (`<number>`, `<length>` or
//! `<length-percentage>`) and is simplified as §10.10 describes: absolute
//! lengths become `px`, like terms in a sum combine, numbers multiply into the
//! other factor, and `min()`/`max()`/`clamp()` arguments with the same unit
//! reduce. What remains (for example `calc(50% + 2em)`) is kept as a tree and
//! resolved later, during computed-value time or layout.
//!
//! Supported syntax: `+ - * /`, parentheses, nested math functions and the
//! constants `e`, `pi`, `infinity`, `-infinity` and `NaN`. Multiplication and
//! division follow the classic restriction: one side of `*` and the right
//! side of `/` must be a `<number>`. Typed division such as `10px / 2px`,
//! `round()`, trigonometric and other Level 4 functions, and `none` in
//! `clamp()` are not supported yet and make the declaration invalid.
//!
//! Lengths and percentages inside a tree are always finite: an operation that
//! would produce an infinite or NaN dimension produces the largest finite
//! value or zero instead, as §10.9 requires of top-level results. Numbers may
//! be infinite or NaN inside a tree; a number result is censored the same
//! way when it is used. Parsing is recursive only into nested functions and
//! parentheses, and stops at [`MAX_CALC_DEPTH`] levels and [`MAX_CALC_NODES`]
//! nodes.

use std::cmp::Ordering;
use std::fmt;

use crate::color::write_number;
use crate::values::BlockKind;
use crate::{ComponentValue, Length, LengthPercentage, LengthUnit, Token, Value};

/// The deepest nesting of math functions and parentheses accepted.
pub const MAX_CALC_DEPTH: usize = 32;

/// The most nodes (values, operators and functions) one math function may have.
pub const MAX_CALC_NODES: usize = 1024;

/// A node of a calculation tree.
#[derive(Clone, Debug, PartialEq)]
pub enum CalcNode {
    /// A `<number>`, possibly infinite or NaN.
    Number(f64),
    /// A `<percentage>`, such as `50` for `50%`. Always finite.
    Percentage(f64),
    /// A `<length>`. Always finite.
    Length(Length),
    /// A sum of two or more terms. Subtraction is a [`CalcNode::Negate`] term.
    Sum(Vec<CalcNode>),
    /// A product of two or more factors. Division is a [`CalcNode::Invert`]
    /// factor.
    Product(Vec<CalcNode>),
    /// The negation of its child.
    Negate(Box<CalcNode>),
    /// The reciprocal of its child (always a number).
    Invert(Box<CalcNode>),
    /// `min()` of one or more arguments.
    Min(Vec<CalcNode>),
    /// `max()` of one or more arguments.
    Max(Vec<CalcNode>),
    /// `clamp(min, value, max)`.
    Clamp(Box<[CalcNode; 3]>),
}

/// What a calculation resolves to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalcKind {
    /// A `<number>`.
    Number,
    /// A `<length>`.
    Length,
    /// A `<percentage>`.
    Percentage,
    /// A `<length>` plus a `<percentage>` (which resolves to a length).
    LengthPercentage,
}

impl CalcKind {
    /// The kind of a sum or comparison of `self` and `other`, if they add.
    fn add(self, other: Self, percentages_are_lengths: bool) -> Option<Self> {
        use CalcKind as K;
        match (self, other) {
            (a, b) if a == b => Some(a),
            (
                K::Length | K::Percentage | K::LengthPercentage,
                K::Length | K::Percentage | K::LengthPercentage,
            ) if percentages_are_lengths => Some(K::LengthPercentage),
            _ => None,
        }
    }
}

/// A type-checked, simplified math function.
#[derive(Clone, Debug, PartialEq)]
pub struct Calc {
    root: CalcNode,
    kind: CalcKind,
}

/// The value categories a math function may resolve to in its context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Context {
    Number,
    Length,
    LengthPercentage,
}

impl Context {
    fn accepts(self, kind: CalcKind) -> bool {
        match self {
            Self::Number => kind == CalcKind::Number,
            Self::Length => kind == CalcKind::Length,
            Self::LengthPercentage => kind != CalcKind::Number,
        }
    }
}

/// Map NaN to zero and infinities to the largest finite values (§10.9).
pub(crate) fn censor(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else if value.is_infinite() {
        f64::MAX.copysign(value)
    } else {
        value
    }
}

/// Whether `name` is a math function this module parses.
pub(crate) fn is_math_function(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "calc" | "min" | "max" | "clamp"
    )
}

impl Calc {
    /// Parse a math function that resolves to a `<number>`, and evaluate it.
    ///
    /// The result is censored: NaN becomes 0 and infinities the largest
    /// finite values. The caller clamps it to the property's range.
    ///
    /// ```
    /// use rustic_css::{Calc, CssParser};
    /// let value = CssParser::default().component_value("calc(1 + 2 * 3)").unwrap().value.unwrap();
    /// assert_eq!(Calc::parse_number(&value), Some(7.0));
    /// ```
    pub fn parse_number(value: &Value) -> Option<f64> {
        let calc = parse(value, Context::Number)?;
        calc.resolve(|_| 0.0, None)
    }

    /// Parse a math function that resolves to a `<length>`.
    pub fn parse_length(value: &Value) -> Option<Self> {
        parse(value, Context::Length)
    }

    /// Parse a math function that resolves to a `<length-percentage>`.
    ///
    /// ```
    /// use rustic_css::{Calc, CssParser};
    /// let value = CssParser::default()
    ///     .component_value("calc(1in + 2em - 10% + 4px * 2)")
    ///     .unwrap()
    ///     .value
    ///     .unwrap();
    /// let calc = Calc::parse_length_percentage(&value).unwrap();
    /// assert_eq!(calc.to_string(), "calc(-10% + 2em + 104px)");
    /// ```
    pub fn parse_length_percentage(value: &Value) -> Option<Self> {
        parse(value, Context::LengthPercentage)
    }

    /// The simplified calculation tree.
    pub fn root(&self) -> &CalcNode {
        &self.root
    }

    /// What the calculation resolves to.
    pub fn kind(&self) -> CalcKind {
        self.kind
    }

    /// Whether the tree contains a percentage.
    pub fn has_percentage(&self) -> bool {
        let mut stack = vec![&self.root];
        while let Some(node) = stack.pop() {
            match node {
                CalcNode::Percentage(_) => return true,
                CalcNode::Number(_) | CalcNode::Length(_) => {}
                CalcNode::Sum(c) | CalcNode::Product(c) | CalcNode::Min(c) | CalcNode::Max(c) => {
                    stack.extend(c);
                }
                CalcNode::Negate(c) | CalcNode::Invert(c) => stack.push(c),
                CalcNode::Clamp(c) => stack.extend(c.iter()),
            }
        }
        false
    }

    /// Evaluate the calculation to a number, or to a length in pixels.
    ///
    /// `length_px` converts each length to pixels; `percent_basis` is what
    /// `100%` resolves to, or `None` if percentages cannot resolve yet (then a
    /// tree containing one gives `None`). The result is censored.
    pub fn resolve(
        &self,
        length_px: impl Fn(Length) -> f64,
        percent_basis: Option<f64>,
    ) -> Option<f64> {
        evaluate(&self.root, &length_px, percent_basis).map(censor)
    }

    /// The computed value of a `<length-percentage>` calculation: every
    /// length becomes pixels through `length_px` and the tree is simplified
    /// again. A tree that reduces to one value becomes a plain length or
    /// percentage; a mix of pixels and percentages stays a calculation.
    pub fn compute(&self, length_px: impl Fn(Length) -> f64) -> LengthPercentage {
        let mapped = map_lengths(&self.root, &length_px);
        Self::from_node(simplify(mapped), self.kind)
    }

    /// Wrap a node, unwrapping single values into plain values.
    fn from_node(root: CalcNode, kind: CalcKind) -> LengthPercentage {
        match root {
            CalcNode::Length(l) => LengthPercentage::Length(l),
            CalcNode::Percentage(p) => LengthPercentage::Percentage(p),
            root => LengthPercentage::Calc(Box::new(Self { root, kind })),
        }
    }

    /// `calc(100% - value)`, simplified (used to turn `right 10px` into a
    /// left offset).
    pub fn hundred_percent_minus(value: &LengthPercentage) -> LengthPercentage {
        let (node, kind) = match value {
            LengthPercentage::Length(l) => (CalcNode::Length(*l), CalcKind::Length),
            LengthPercentage::Percentage(p) => (CalcNode::Percentage(*p), CalcKind::Percentage),
            LengthPercentage::Calc(calc) => (calc.root.clone(), calc.kind),
        };
        let kind = CalcKind::Percentage
            .add(kind, true)
            .unwrap_or(CalcKind::LengthPercentage);
        let sum = CalcNode::Sum(vec![
            CalcNode::Percentage(100.0),
            CalcNode::Negate(Box::new(node)),
        ]);
        Self::from_node(simplify(sum), kind)
    }
}

// ---------------------------------------------------------------------------
// Parsing

struct Parser {
    nodes: usize,
    context: Context,
}

impl Parser {
    fn count(&mut self) -> Option<()> {
        self.nodes += 1;
        (self.nodes <= MAX_CALC_NODES).then_some(())
    }

    fn percentages_are_lengths(&self) -> bool {
        self.context == Context::LengthPercentage
    }

    /// A math function's arguments.
    fn function(
        &mut self,
        name: &str,
        arguments: &[Value],
        depth: usize,
    ) -> Option<(CalcNode, CalcKind)> {
        if depth >= MAX_CALC_DEPTH {
            return None;
        }
        self.count()?;
        let lower = name.to_ascii_lowercase();
        if lower == "calc" {
            return self.sum(arguments, depth + 1);
        }
        let mut args = Vec::new();
        let mut kind: Option<CalcKind> = None;
        for piece in arguments.split(|v| v.token() == Some(&Token::Comma)) {
            let (node, k) = self.sum(piece, depth + 1)?;
            kind = Some(match kind {
                None => k,
                Some(previous) => previous.add(k, self.percentages_are_lengths())?,
            });
            args.push(node);
        }
        let kind = kind?;
        let node = match lower.as_str() {
            "min" => CalcNode::Min(args),
            "max" => CalcNode::Max(args),
            "clamp" => {
                let [a, b, c]: [CalcNode; 3] = args.try_into().ok()?;
                CalcNode::Clamp(Box::new([a, b, c]))
            }
            _ => return None,
        };
        Some((node, kind))
    }

    /// `<calc-sum>`, which must consume all of `items`.
    fn sum(&mut self, items: &[Value], depth: usize) -> Option<(CalcNode, CalcKind)> {
        let mut cursor = Cursor { items, pos: 0 };
        cursor.skip_whitespace();
        let (first, mut kind) = self.product(&mut cursor, depth)?;
        let mut terms = vec![first];
        loop {
            let space_before = cursor.skip_whitespace();
            let Some(next) = cursor.next() else { break };
            let negate = match next.token() {
                Some(Token::Delim('+')) => false,
                Some(Token::Delim('-')) => true,
                _ => return None,
            };
            // `+` and `-` must have whitespace on both sides.
            if !space_before || !cursor.skip_whitespace() {
                return None;
            }
            let (term, term_kind) = self.product(&mut cursor, depth)?;
            kind = kind.add(term_kind, self.percentages_are_lengths())?;
            self.count()?;
            terms.push(if negate {
                CalcNode::Negate(Box::new(term))
            } else {
                term
            });
        }
        let node = if terms.len() == 1 {
            terms.pop().expect("one term")
        } else {
            CalcNode::Sum(terms)
        };
        Some((node, kind))
    }

    /// `<calc-product>`.
    fn product(&mut self, cursor: &mut Cursor<'_>, depth: usize) -> Option<(CalcNode, CalcKind)> {
        let (first, mut kind) = self.value(cursor.next()?, depth)?;
        let mut factors = vec![first];
        loop {
            let save = cursor.pos;
            cursor.skip_whitespace();
            let divide = match cursor.next().and_then(Value::token) {
                Some(Token::Delim('*')) => false,
                Some(Token::Delim('/')) => true,
                _ => {
                    cursor.pos = save;
                    break;
                }
            };
            cursor.skip_whitespace();
            let (factor, factor_kind) = self.value(cursor.next()?, depth)?;
            self.count()?;
            if divide {
                // The divisor must be a number.
                if factor_kind != CalcKind::Number {
                    return None;
                }
                factors.push(CalcNode::Invert(Box::new(factor)));
            } else {
                // One side of a multiplication must be a number.
                kind = match (kind, factor_kind) {
                    (CalcKind::Number, k) | (k, CalcKind::Number) => k,
                    _ => return None,
                };
                factors.push(factor);
            }
        }
        let node = if factors.len() == 1 {
            factors.pop().expect("one factor")
        } else {
            CalcNode::Product(factors)
        };
        Some((node, kind))
    }

    /// `<calc-value>`.
    fn value(&mut self, value: &Value, depth: usize) -> Option<(CalcNode, CalcKind)> {
        self.count()?;
        match &value.value {
            ComponentValue::Token(Token::Number(n)) => {
                Some((CalcNode::Number(n.value), CalcKind::Number))
            }
            ComponentValue::Token(Token::Percentage(n)) => {
                Some((CalcNode::Percentage(n.value), CalcKind::Percentage))
            }
            ComponentValue::Token(Token::Dimension(n, unit)) => {
                if self.context == Context::Number {
                    return None;
                }
                let unit = LengthUnit::from_name(unit)?;
                Some((
                    CalcNode::Length(Length {
                        value: n.value,
                        unit,
                    }),
                    CalcKind::Length,
                ))
            }
            ComponentValue::Token(Token::Ident(name)) => {
                let n = match name.to_ascii_lowercase().as_str() {
                    "e" => std::f64::consts::E,
                    "pi" => std::f64::consts::PI,
                    "infinity" => f64::INFINITY,
                    "-infinity" => f64::NEG_INFINITY,
                    "nan" => f64::NAN,
                    _ => return None,
                };
                Some((CalcNode::Number(n), CalcKind::Number))
            }
            ComponentValue::Block {
                kind: BlockKind::Paren,
                contents,
            } => {
                if depth >= MAX_CALC_DEPTH {
                    return None;
                }
                self.sum(contents, depth + 1)
            }
            ComponentValue::Function { name, arguments } if is_math_function(name) => {
                self.function(name, arguments, depth)
            }
            _ => None,
        }
    }
}

struct Cursor<'a> {
    items: &'a [Value],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// Skip whitespace; report whether there was any.
    fn skip_whitespace(&mut self) -> bool {
        let start = self.pos;
        while self.items.get(self.pos).is_some_and(Value::is_whitespace) {
            self.pos += 1;
        }
        self.pos > start
    }

    fn next(&mut self) -> Option<&'a Value> {
        let value = self.items.get(self.pos)?;
        self.pos += 1;
        Some(value)
    }
}

fn parse(value: &Value, context: Context) -> Option<Calc> {
    let ComponentValue::Function { name, arguments } = &value.value else {
        return None;
    };
    if !is_math_function(name) {
        return None;
    }
    let mut parser = Parser { nodes: 0, context };
    let (root, kind) = parser.function(name, arguments, 0)?;
    if !context.accepts(kind) {
        return None;
    }
    Some(Calc {
        root: simplify(root),
        kind,
    })
}

// ---------------------------------------------------------------------------
// Simplification (§10.10)

/// The unit a numeric leaf is expressed in, for combining like terms.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Unit {
    Number,
    Percentage,
    Length(LengthUnit),
}

fn unit_of(node: &CalcNode) -> Option<(Unit, f64)> {
    match node {
        CalcNode::Number(n) => Some((Unit::Number, *n)),
        CalcNode::Percentage(p) => Some((Unit::Percentage, *p)),
        CalcNode::Length(l) => Some((Unit::Length(l.unit), l.value)),
        _ => None,
    }
}

fn leaf(unit: Unit, value: f64) -> CalcNode {
    match unit {
        Unit::Number => CalcNode::Number(value),
        Unit::Percentage => CalcNode::Percentage(censor(value)),
        Unit::Length(unit) => CalcNode::Length(Length {
            value: censor(value),
            unit,
        }),
    }
}

/// `min`/`max` that propagate NaN, as math functions do.
fn nan_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

fn nan_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

fn clamp_value(low: f64, value: f64, high: f64) -> f64 {
    nan_max(low, nan_min(value, high))
}

/// Combine leaves with the same unit using `op`, keeping other nodes.
fn combine(children: Vec<CalcNode>, op: fn(f64, f64) -> f64) -> Vec<CalcNode> {
    let mut out: Vec<CalcNode> = Vec::with_capacity(children.len());
    for child in children {
        if let Some((unit, value)) = unit_of(&child) {
            if let Some(existing) = out
                .iter_mut()
                .find(|n| unit_of(n).is_some_and(|(u, _)| u == unit))
            {
                let (_, previous) = unit_of(existing).expect("leaf");
                *existing = leaf(unit, op(previous, value));
                continue;
            }
        }
        out.push(child);
    }
    out
}

fn simplify(node: CalcNode) -> CalcNode {
    match node {
        CalcNode::Number(_) | CalcNode::Percentage(_) => node,
        CalcNode::Length(l) => match l.to_px() {
            Some(px) => CalcNode::Length(Length::px(censor(px))),
            None => node,
        },
        CalcNode::Negate(child) => match simplify(*child) {
            CalcNode::Negate(inner) => *inner,
            child => match unit_of(&child) {
                Some((unit, value)) => leaf(unit, -value),
                None => CalcNode::Negate(Box::new(child)),
            },
        },
        CalcNode::Invert(child) => match simplify(*child) {
            CalcNode::Invert(inner) => *inner,
            CalcNode::Number(n) => CalcNode::Number(1.0 / n),
            child => CalcNode::Invert(Box::new(child)),
        },
        CalcNode::Sum(children) => {
            let mut flat = Vec::with_capacity(children.len());
            for child in children {
                match simplify(child) {
                    CalcNode::Sum(inner) => flat.extend(inner),
                    other => flat.push(other),
                }
            }
            let mut terms = combine(flat, |a, b| a + b);
            if terms.len() == 1 {
                terms.pop().expect("one term")
            } else {
                CalcNode::Sum(terms)
            }
        }
        CalcNode::Product(children) => {
            let mut factor = 1.0;
            let mut had_number = false;
            let mut rest = Vec::new();
            let mut stack: Vec<CalcNode> = children.into_iter().rev().collect();
            while let Some(child) = stack.pop() {
                match simplify(child) {
                    CalcNode::Number(n) => {
                        factor *= n;
                        had_number = true;
                    }
                    CalcNode::Product(inner) => stack.extend(inner.into_iter().rev()),
                    other => rest.push(other),
                }
            }
            match rest.len() {
                0 => CalcNode::Number(factor),
                1 => {
                    let only = rest.pop().expect("one factor");
                    if let Some((unit, value)) = unit_of(&only) {
                        return leaf(unit, value * factor);
                    }
                    if let CalcNode::Sum(terms) = &only {
                        if terms.iter().all(|t| unit_of(t).is_some()) {
                            return CalcNode::Sum(
                                terms
                                    .iter()
                                    .map(|t| {
                                        let (unit, value) = unit_of(t).expect("leaf");
                                        leaf(unit, value * factor)
                                    })
                                    .collect(),
                            );
                        }
                    }
                    // A number factor stays even when it is 1, so that a
                    // serialization that rounds a factor to `1` reparses to
                    // the same tree shape.
                    if had_number {
                        CalcNode::Product(vec![CalcNode::Number(factor), only])
                    } else {
                        only
                    }
                }
                _ => {
                    let mut factors = Vec::with_capacity(rest.len() + 1);
                    if had_number {
                        factors.push(CalcNode::Number(factor));
                    }
                    factors.extend(rest);
                    CalcNode::Product(factors)
                }
            }
        }
        CalcNode::Min(children) | CalcNode::Max(children) if children.is_empty() => {
            CalcNode::Number(f64::NAN)
        }
        CalcNode::Min(children) => {
            let mut args = combine(children.into_iter().map(simplify).collect(), nan_min);
            if args.len() == 1 {
                args.pop().expect("one argument")
            } else {
                CalcNode::Min(args)
            }
        }
        CalcNode::Max(children) => {
            let mut args = combine(children.into_iter().map(simplify).collect(), nan_max);
            if args.len() == 1 {
                args.pop().expect("one argument")
            } else {
                CalcNode::Max(args)
            }
        }
        CalcNode::Clamp(children) => {
            let [low, value, high] = *children;
            let (low, value, high) = (simplify(low), simplify(value), simplify(high));
            match (unit_of(&low), unit_of(&value), unit_of(&high)) {
                (Some((a, l)), Some((b, v)), Some((c, h))) if a == b && b == c => {
                    leaf(a, clamp_value(l, v, h))
                }
                _ => CalcNode::Clamp(Box::new([low, value, high])),
            }
        }
    }
}

fn map_lengths(node: &CalcNode, length_px: &dyn Fn(Length) -> f64) -> CalcNode {
    let map = |children: &[CalcNode]| children.iter().map(|c| map_lengths(c, length_px)).collect();
    match node {
        CalcNode::Length(l) => CalcNode::Length(Length::px(censor(length_px(*l)))),
        // Computed values are finite everywhere, numbers included.
        CalcNode::Number(n) => CalcNode::Number(censor(*n)),
        CalcNode::Percentage(_) => node.clone(),
        CalcNode::Sum(c) => CalcNode::Sum(map(c)),
        CalcNode::Product(c) => CalcNode::Product(map(c)),
        CalcNode::Min(c) => CalcNode::Min(map(c)),
        CalcNode::Max(c) => CalcNode::Max(map(c)),
        CalcNode::Negate(c) => CalcNode::Negate(Box::new(map_lengths(c, length_px))),
        CalcNode::Invert(c) => CalcNode::Invert(Box::new(map_lengths(c, length_px))),
        CalcNode::Clamp(c) => CalcNode::Clamp(Box::new([
            map_lengths(&c[0], length_px),
            map_lengths(&c[1], length_px),
            map_lengths(&c[2], length_px),
        ])),
    }
}

fn evaluate(
    node: &CalcNode,
    length_px: &dyn Fn(Length) -> f64,
    percent: Option<f64>,
) -> Option<f64> {
    let all = |children: &[CalcNode]| -> Option<Vec<f64>> {
        children
            .iter()
            .map(|c| evaluate(c, length_px, percent))
            .collect()
    };
    Some(match node {
        CalcNode::Number(n) => *n,
        CalcNode::Percentage(p) => percent? * p / 100.0,
        CalcNode::Length(l) => length_px(*l),
        CalcNode::Sum(c) => all(c)?.into_iter().sum(),
        CalcNode::Product(c) => all(c)?.into_iter().product(),
        CalcNode::Negate(c) => -evaluate(c, length_px, percent)?,
        CalcNode::Invert(c) => 1.0 / evaluate(c, length_px, percent)?,
        CalcNode::Min(c) => all(c)?.into_iter().reduce(nan_min)?,
        CalcNode::Max(c) => all(c)?.into_iter().reduce(nan_max)?,
        CalcNode::Clamp(c) => {
            let low = evaluate(&c[0], length_px, percent)?;
            let value = evaluate(&c[1], length_px, percent)?;
            let high = evaluate(&c[2], length_px, percent)?;
            clamp_value(low, value, high)
        }
    })
}

// ---------------------------------------------------------------------------
// Serialization (§10.12)

/// Sort order for a sum's or product's children: numbers, percentages,
/// dimensions by unit name, then everything else in its original order.
fn sort_key(node: &CalcNode) -> (u8, &'static str) {
    match node {
        CalcNode::Number(_) => (0, ""),
        CalcNode::Percentage(_) => (1, ""),
        CalcNode::Length(l) => (2, l.unit.name()),
        _ => (3, ""),
    }
}

fn sorted(children: &[CalcNode]) -> Vec<&CalcNode> {
    let mut out: Vec<&CalcNode> = children.iter().collect();
    // A stable sort keeps "everything else" in order.
    out.sort_by(|a, b| {
        sort_key(a)
            .partial_cmp(&sort_key(b))
            .unwrap_or(Ordering::Equal)
    });
    out
}

fn write_leaf_number(f: &mut fmt::Formatter<'_>, value: f64) -> fmt::Result {
    if value.is_nan() {
        f.write_str("NaN")
    } else if value == f64::INFINITY {
        f.write_str("infinity")
    } else if value == f64::NEG_INFINITY {
        f.write_str("-infinity")
    } else {
        write_number(f, value)
    }
}

/// Whether a leaf serializes with a minus sign (a value that rounds to `0`
/// does not, so `- 0px` never appears and reparses differently).
fn is_negative_leaf(node: &CalcNode) -> bool {
    unit_of(node).is_some_and(|(_, v)| {
        let mut text = String::new();
        v < 0.0 && write_number(&mut text, v).is_ok() && text.starts_with('-')
    })
}

fn write_node(f: &mut fmt::Formatter<'_>, node: &CalcNode, top: bool) -> fmt::Result {
    let (open, close) = if top { ("", "") } else { ("(", ")") };
    match node {
        CalcNode::Number(n) => write_leaf_number(f, *n),
        CalcNode::Percentage(p) => {
            write_number(f, *p)?;
            f.write_str("%")
        }
        CalcNode::Length(l) => write!(f, "{l}"),
        CalcNode::Negate(c) => {
            f.write_str("(-1 * ")?;
            write_node(f, c, false)?;
            f.write_str(")")
        }
        CalcNode::Invert(c) => {
            f.write_str("(1 / ")?;
            write_node(f, c, false)?;
            f.write_str(")")
        }
        CalcNode::Sum(children) => {
            f.write_str(open)?;
            for (i, child) in sorted(children).into_iter().enumerate() {
                if i == 0 {
                    write_node(f, child, false)?;
                } else if let CalcNode::Negate(inner) = child {
                    f.write_str(" - ")?;
                    write_node(f, inner, false)?;
                } else if is_negative_leaf(child) {
                    f.write_str(" - ")?;
                    let (unit, value) = unit_of(child).expect("leaf");
                    write_node(f, &leaf(unit, -value), false)?;
                } else {
                    f.write_str(" + ")?;
                    write_node(f, child, false)?;
                }
            }
            f.write_str(close)
        }
        CalcNode::Product(children) => {
            f.write_str(open)?;
            for (i, child) in sorted(children).into_iter().enumerate() {
                match child {
                    CalcNode::Invert(inner) if i > 0 => {
                        f.write_str(" / ")?;
                        write_node(f, inner, false)?;
                    }
                    _ => {
                        if i > 0 {
                            f.write_str(" * ")?;
                        }
                        write_node(f, child, false)?;
                    }
                }
            }
            f.write_str(close)
        }
        CalcNode::Min(children) | CalcNode::Max(children) => {
            f.write_str(if matches!(node, CalcNode::Min(_)) {
                "min("
            } else {
                "max("
            })?;
            for (i, child) in children.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write_node(f, child, true)?;
            }
            f.write_str(")")
        }
        CalcNode::Clamp(children) => {
            f.write_str("clamp(")?;
            for (i, child) in children.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write_node(f, child, true)?;
            }
            f.write_str(")")
        }
    }
}

impl fmt::Display for Calc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.root {
            CalcNode::Min(_) | CalcNode::Max(_) | CalcNode::Clamp(_) => {
                write_node(f, &self.root, true)
            }
            root => {
                f.write_str("calc(")?;
                write_node(f, root, true)?;
                f.write_str(")")
            }
        }
    }
}
