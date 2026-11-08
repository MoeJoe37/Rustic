//! Registered custom properties (CSS Properties and Values API Level 1,
//! Editor's Draft reviewed 2026-11-08): `@property` rules and the syntax
//! definitions their values are checked against.
//!
//! Supported syntax components: `<length>`, `<number>`, `<percentage>`,
//! `<length-percentage>`, `<color>`, `<integer>`, `<custom-ident>` and
//! literal identifiers, each with an optional `+` (space-separated list) or
//! `#` (comma-separated list) multiplier, combined with `|`; and the
//! universal syntax `*`. A syntax naming another data type (`<angle>`,
//! `<image>`, `<transform-list>`, ...) is not supported yet, and the
//! registration is dropped.

use std::fmt::{self, Write};

use crate::color::write_number;
use crate::properties::{CssWideKeyword, ident, length_percentage};
use crate::{
    Calc, Color, ComponentValue, CustomValue, Length, LengthPercentage, Token, Value,
    serialize_identifier,
};

/// A syntax component's data type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SyntaxType {
    /// `<length>`.
    Length,
    /// `<number>`.
    Number,
    /// `<percentage>`.
    Percentage,
    /// `<length-percentage>`.
    LengthPercentage,
    /// `<color>`.
    Color,
    /// `<integer>`.
    Integer,
    /// `<custom-ident>`.
    CustomIdent,
    /// A literal identifier (case-sensitive).
    Ident(String),
}

/// How many values a component takes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyntaxMultiplier {
    /// Exactly one.
    One,
    /// `+`: one or more, space-separated.
    Space,
    /// `#`: one or more, comma-separated.
    Comma,
}

/// One `|`-separated alternative of a syntax definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxComponent {
    /// The data type.
    pub kind: SyntaxType,
    /// The multiplier.
    pub multiplier: SyntaxMultiplier,
}

/// A registered property's syntax definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PropertySyntax {
    /// `*`: any token sequence, as for unregistered properties.
    Universal,
    /// Alternatives tried in order.
    Components(Vec<SyntaxComponent>),
}

/// One value of a registered property that matched a component.
#[derive(Clone, Debug, PartialEq)]
pub enum RegisteredItem {
    /// A `<length>` or `<length-percentage>`.
    Length(LengthPercentage),
    /// A `<number>`.
    Number(f64),
    /// A `<percentage>` (`50` for `50%`).
    Percentage(f64),
    /// An `<integer>`.
    Integer(f64),
    /// A `<color>`.
    Color(Color),
    /// A `<custom-ident>` or literal identifier.
    Ident(String),
}

/// A registered property's value, parsed against its syntax.
#[derive(Clone, Debug, PartialEq)]
pub struct RegisteredValue {
    /// The items, in order.
    pub items: Vec<RegisteredItem>,
    /// How they are separated.
    pub multiplier: SyntaxMultiplier,
}

fn is_custom_ident(name: &str) -> bool {
    CssWideKeyword::from_name(name).is_none() && !name.eq_ignore_ascii_case("default")
}

impl PropertySyntax {
    /// Parse a `syntax` descriptor's string; `None` if it is invalid or
    /// uses an unsupported data type.
    pub fn parse(source: &str) -> Option<Self> {
        let source = source.trim_matches(|c: char| c.is_ascii_whitespace());
        if source == "*" {
            return Some(Self::Universal);
        }
        let mut components = Vec::new();
        for part in source.split('|') {
            let part = part.trim_matches(|c: char| c.is_ascii_whitespace());
            let (body, multiplier) = match part.as_bytes().last() {
                Some(b'+') => (&part[..part.len() - 1], SyntaxMultiplier::Space),
                Some(b'#') => (&part[..part.len() - 1], SyntaxMultiplier::Comma),
                _ => (part, SyntaxMultiplier::One),
            };
            let kind = if let Some(name) = body.strip_prefix('<').and_then(|b| b.strip_suffix('>'))
            {
                match name {
                    "length" => SyntaxType::Length,
                    "number" => SyntaxType::Number,
                    "percentage" => SyntaxType::Percentage,
                    "length-percentage" => SyntaxType::LengthPercentage,
                    "color" => SyntaxType::Color,
                    "integer" => SyntaxType::Integer,
                    "custom-ident" => SyntaxType::CustomIdent,
                    _ => return None,
                }
            } else {
                let valid = !body.is_empty()
                    && !body.starts_with(|c: char| c.is_ascii_digit())
                    && !(body.starts_with('-')
                        && body[1..].starts_with(|c: char| c.is_ascii_digit()))
                    && body != "-"
                    && body.chars().all(|c| {
                        c.is_ascii_alphanumeric() || c == '-' || c == '_' || c >= '\u{80}'
                    })
                    && is_custom_ident(body);
                if !valid {
                    return None;
                }
                SyntaxType::Ident(body.to_owned())
            };
            components.push(SyntaxComponent { kind, multiplier });
        }
        Some(Self::Components(components))
    }

    /// Parse `value` against the syntax: `None` if it does not match, and
    /// for [`PropertySyntax::Universal`] (whose values stay tokens).
    pub fn parse_value(&self, value: &CustomValue) -> Option<RegisteredValue> {
        let Self::Components(components) = self else {
            return None;
        };
        let parts: Vec<&Value> = value
            .values()
            .iter()
            .filter(|v| !v.is_whitespace())
            .collect();
        components.iter().find_map(|c| c.parse(&parts))
    }

    /// Whether `value` matches the syntax (any value matches `*`).
    pub fn matches(&self, value: &CustomValue) -> bool {
        *self == Self::Universal || self.parse_value(value).is_some()
    }
}

impl SyntaxComponent {
    fn parse(&self, parts: &[&Value]) -> Option<RegisteredValue> {
        let items = match self.multiplier {
            SyntaxMultiplier::One => match parts {
                [only] => vec![self.item(only)?],
                _ => return None,
            },
            SyntaxMultiplier::Space => {
                if parts.is_empty() {
                    return None;
                }
                parts
                    .iter()
                    .map(|p| self.item(p))
                    .collect::<Option<Vec<_>>>()?
            }
            SyntaxMultiplier::Comma => parts
                .split(|v| v.token() == Some(&Token::Comma))
                .map(|segment| match segment {
                    [only] => self.item(only),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?,
        };
        Some(RegisteredValue {
            items,
            multiplier: self.multiplier,
        })
    }

    fn item(&self, value: &Value) -> Option<RegisteredItem> {
        let is_math = matches!(&value.value, ComponentValue::Function { name, .. }
            if crate::calc::is_math_function(name));
        match &self.kind {
            SyntaxType::Length => {
                let lp = if is_math {
                    LengthPercentage::Calc(Box::new(Calc::parse_length(value)?))
                } else {
                    length_percentage(value)?
                };
                (!matches!(lp, LengthPercentage::Percentage(_)))
                    .then_some(RegisteredItem::Length(lp))
            }
            SyntaxType::LengthPercentage => length_percentage(value).map(RegisteredItem::Length),
            SyntaxType::Number => match value.token() {
                Some(Token::Number(n)) => Some(RegisteredItem::Number(n.value)),
                _ if is_math => Calc::parse_number(value).map(RegisteredItem::Number),
                _ => None,
            },
            SyntaxType::Integer => match value.token() {
                Some(Token::Number(n)) if n.integer => Some(RegisteredItem::Integer(n.value)),
                _ if is_math => {
                    Calc::parse_number(value).map(|n| RegisteredItem::Integer(n.round()))
                }
                _ => None,
            },
            SyntaxType::Percentage => match value.token() {
                Some(Token::Percentage(n)) => Some(RegisteredItem::Percentage(n.value)),
                _ => None,
            },
            SyntaxType::Color => Color::parse(value).map(RegisteredItem::Color),
            SyntaxType::CustomIdent => {
                let name = ident(value)?;
                is_custom_ident(name).then(|| RegisteredItem::Ident(name.to_owned()))
            }
            SyntaxType::Ident(expected) => {
                (ident(value)? == expected).then(|| RegisteredItem::Ident(expected.clone()))
            }
        }
    }
}

impl RegisteredValue {
    /// Whether any length uses a unit relative to the element's own font or
    /// line height (`em`, `ex`, `cap`, `ch`, `ic`, `lh`), and whether any
    /// uses `lh`; with `root`, the root-relative units count too.
    pub fn font_relative(&self, root: bool) -> (bool, bool) {
        use crate::LengthUnit as U;
        let mut font = false;
        let mut line = false;
        let mut visit = |length: Length| match length.unit {
            U::Em | U::Ex | U::Cap | U::Ch | U::Ic => font = true,
            U::Lh => {
                font = true;
                line = true;
            }
            U::Rem | U::Rex | U::Rcap | U::Rch | U::Ric if root => font = true,
            U::Rlh if root => {
                font = true;
                line = true;
            }
            _ => {}
        };
        for item in &self.items {
            if let RegisteredItem::Length(lp) = item {
                for_each_length(lp, &mut visit);
            }
        }
        (font, line)
    }

    /// Whether every length is absolute (an `initial-value` must be
    /// computationally independent).
    pub fn is_computationally_independent(&self) -> bool {
        let mut independent = true;
        for item in &self.items {
            if let RegisteredItem::Length(lp) = item {
                for_each_length(lp, &mut |l: Length| independent &= l.to_px().is_some());
            }
        }
        independent
    }

    /// Replace every length with `compute(length)`'s result.
    pub fn map_lengths(&mut self, compute: &dyn Fn(&LengthPercentage) -> LengthPercentage) {
        for item in &mut self.items {
            if let RegisteredItem::Length(lp) = item {
                *lp = compute(lp);
            }
        }
    }
}

fn for_each_length(lp: &LengthPercentage, visit: &mut dyn FnMut(Length)) {
    match lp {
        LengthPercentage::Length(l) => visit(*l),
        LengthPercentage::Percentage(_) => {}
        LengthPercentage::Calc(calc) => {
            let mut stack = vec![calc.root()];
            while let Some(node) = stack.pop() {
                use crate::CalcNode as N;
                match node {
                    N::Length(l) => visit(*l),
                    N::Number(_) | N::Percentage(_) => {}
                    N::Sum(c) | N::Product(c) | N::Min(c) | N::Max(c) => stack.extend(c),
                    N::Negate(c) | N::Invert(c) => stack.push(c),
                    N::Clamp(c) => stack.extend(c.iter()),
                }
            }
        }
    }
}

impl fmt::Display for RegisteredItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length(lp) => write!(f, "{lp}"),
            Self::Number(n) | Self::Integer(n) => write_number(f, *n),
            Self::Percentage(p) => {
                write_number(f, *p)?;
                f.write_char('%')
            }
            Self::Color(c) => write!(f, "{c}"),
            Self::Ident(name) => serialize_identifier(f, name),
        }
    }
}

impl fmt::Display for RegisteredValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let separator = match self.multiplier {
            SyntaxMultiplier::Comma => ", ",
            _ => " ",
        };
        for (i, item) in self.items.iter().enumerate() {
            if i > 0 {
                f.write_str(separator)?;
            }
            write!(f, "{item}")?;
        }
        Ok(())
    }
}

impl fmt::Display for PropertySyntax {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::Components(components) = self else {
            return f.write_char('*');
        };
        for (i, c) in components.iter().enumerate() {
            if i > 0 {
                f.write_str(" | ")?;
            }
            match &c.kind {
                SyntaxType::Length => f.write_str("<length>")?,
                SyntaxType::Number => f.write_str("<number>")?,
                SyntaxType::Percentage => f.write_str("<percentage>")?,
                SyntaxType::LengthPercentage => f.write_str("<length-percentage>")?,
                SyntaxType::Color => f.write_str("<color>")?,
                SyntaxType::Integer => f.write_str("<integer>")?,
                SyntaxType::CustomIdent => f.write_str("<custom-ident>")?,
                SyntaxType::Ident(name) => f.write_str(name)?,
            }
            match c.multiplier {
                SyntaxMultiplier::One => {}
                SyntaxMultiplier::Space => f.write_char('+')?,
                SyntaxMultiplier::Comma => f.write_char('#')?,
            }
        }
        Ok(())
    }
}
