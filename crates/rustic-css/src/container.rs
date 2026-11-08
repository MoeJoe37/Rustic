//! Container queries (CSS Conditional Rules Level 5 §6, Editor's Draft
//! reviewed 2026-11-08): `@container` preludes, size features and
//! `style()` queries.
//!
//! A container condition reuses [`MediaCondition`] with the container
//! feature set: `width`, `height`, `inline-size`, `block-size`,
//! `aspect-ratio` and `orientation`, plus [`MediaCondition::Style`].
//! Evaluation is three-valued, as for media queries; a size feature on an
//! axis without a known size is unknown.

use std::fmt::{self, Write};

use crate::media::{
    FeatureSet, contains_bad_token, ident, is_keyword, nesting_depth, non_whitespace, paren_block,
    parse_condition,
};
use crate::serialize::write_values;
use crate::{
    CssWideKeyword, CustomValue, Length, LengthUnit, MAX_MEDIA_NESTING, MediaComparison,
    MediaCondition, MediaFeature, MediaFeatureName, MediaFeatureValue, MediaPrefix, Token, Value,
    serialize_identifier,
};

/// A `style()` query.
#[derive(Clone, Debug, PartialEq)]
pub enum StyleQuery {
    /// `not (query)`.
    Not(Box<StyleQuery>),
    /// Two or more operands joined by `and`.
    And(Vec<StyleQuery>),
    /// Two or more operands joined by `or`.
    Or(Vec<StyleQuery>),
    /// `name: value`, or `name` alone (true when the property has a value
    /// other than its initial one). Names keep their case.
    Feature {
        /// The property name.
        name: String,
        /// The value to compare with, if any.
        value: Option<CustomValue>,
    },
    /// `<general-enclosed>`: always unknown.
    Unknown(Value),
}

/// One condition of an `@container` prelude: an optional container name and
/// an optional query (at least one of the two is present).
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerCondition {
    /// The container name to look for, case-sensitive.
    pub name: Option<String>,
    /// The query the container must satisfy.
    pub query: Option<MediaCondition>,
}

/// An `@container` prelude: the rule applies when any condition holds.
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerQueryList {
    /// The conditions, in order (never empty).
    pub conditions: Vec<ContainerCondition>,
}

/// Which axes a query needs sizes on, to pick a container that has them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ContainerAxes {
    /// `width`, `inline-size` (horizontal writing mode assumed).
    pub inline: bool,
    /// `height`, `block-size`, and both axes for `aspect-ratio` and
    /// `orientation`.
    pub block: bool,
}

/// What a container query is evaluated against: the container's content
/// box size per axis (`None` where unknown or not a size container), its
/// font sizes for relative units, and the viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContainerFacts {
    /// Content-box width in CSS pixels.
    pub width: Option<f64>,
    /// Content-box height in CSS pixels.
    pub height: Option<f64>,
    /// The container's computed font size, for `em` and friends.
    pub font_size_px: f64,
    /// The root's computed font size, for `rem` and friends.
    pub root_font_size_px: f64,
    /// Viewport width in CSS pixels.
    pub viewport_width: f64,
    /// Viewport height in CSS pixels.
    pub viewport_height: f64,
}

/// Parse an `@container` prelude; `None` if it is invalid (the rule is then
/// dropped).
pub fn parse_container_query_list(values: &[Value]) -> Option<ContainerQueryList> {
    if contains_bad_token(values) {
        return None;
    }
    let items = non_whitespace(values);
    let conditions = items
        .split(|v| matches!(v.token(), Some(Token::Comma)))
        .map(container_condition)
        .collect::<Option<Vec<_>>>()?;
    (!conditions.is_empty()).then_some(ContainerQueryList { conditions })
}

/// A condition nested deeper than [`MAX_MEDIA_NESTING`] is invalid.
fn container_condition(items: &[&Value]) -> Option<ContainerCondition> {
    if nesting_depth(items) > MAX_MEDIA_NESTING {
        return None;
    }
    let mut pos = 0;
    let name = match items.first().and_then(|v| ident(v)) {
        Some(n) if is_container_name(n) => {
            pos = 1;
            Some(n.to_owned())
        }
        _ => None,
    };
    let query = if pos == items.len() {
        None
    } else {
        let query = parse_condition(items, &mut pos, true, 0, FeatureSet::Container)?;
        if pos != items.len() {
            return None;
        }
        Some(query)
    };
    (name.is_some() || query.is_some()).then_some(ContainerCondition { name, query })
}

/// A `<container-name>`: a `<custom-ident>` other than `none`, `and`, `or`,
/// `not` and `default`.
fn is_container_name(name: &str) -> bool {
    !["none", "and", "or", "not", "default"]
        .iter()
        .any(|r| name.eq_ignore_ascii_case(r))
        && CssWideKeyword::from_name(name).is_none()
}

fn trim(mut values: &[Value]) -> &[Value] {
    while values.first().is_some_and(Value::is_whitespace) {
        values = &values[1..];
    }
    while values.last().is_some_and(Value::is_whitespace) {
        values = &values[..values.len() - 1];
    }
    values
}

/// Parse a `style()` function's arguments. Arguments nested deeper than
/// [`MAX_MEDIA_NESTING`] make the function general-enclosed as a whole, so
/// the result never depends on where a nesting limit cut it off.
pub(crate) fn parse_style_function(arguments: &[Value]) -> Option<StyleQuery> {
    let items: Vec<&Value> = arguments.iter().collect();
    if nesting_depth(&items) > MAX_MEDIA_NESTING {
        return None;
    }
    parse_style_query(arguments, 0)
}

/// Parse a `style()` function's arguments, `depth` parentheses deep inside
/// it (counted separately from the media condition around it, so the
/// result does not depend on that context).
fn parse_style_query(arguments: &[Value], depth: usize) -> Option<StyleQuery> {
    let raw = trim(arguments);
    let items = non_whitespace(raw);
    let first = *items.first()?;
    if is_keyword(first, "not") || paren_block(first).is_some() {
        let mut pos = 0;
        let query = style_condition(&items, &mut pos, depth)?;
        return (pos == items.len()).then_some(query);
    }
    // A feature: `name` or `name: value`.
    let name = ident(first)?.to_owned();
    if items.len() == 1 {
        return Some(StyleQuery::Feature { name, value: None });
    }
    let colon = raw.iter().position(|v| v.token() == Some(&Token::Colon))?;
    if !raw[1..colon].iter().all(Value::is_whitespace) {
        return None;
    }
    let value = CustomValue::new(&raw[colon + 1..])?;
    Some(StyleQuery::Feature {
        name,
        value: Some(value),
    })
}

fn style_condition(items: &[&Value], pos: &mut usize, depth: usize) -> Option<StyleQuery> {
    if items.get(*pos).is_some_and(|v| is_keyword(v, "not")) {
        *pos += 1;
        return style_in_parens(items, pos, depth).map(|q| StyleQuery::Not(Box::new(q)));
    }
    let first = style_in_parens(items, pos, depth)?;
    let joiner = match items.get(*pos) {
        Some(v) if is_keyword(v, "and") => "and",
        Some(v) if is_keyword(v, "or") => "or",
        _ => return Some(first),
    };
    let mut operands = vec![first];
    while items.get(*pos).is_some_and(|v| is_keyword(v, joiner)) {
        *pos += 1;
        operands.push(style_in_parens(items, pos, depth)?);
    }
    Some(if joiner == "and" {
        StyleQuery::And(operands)
    } else {
        StyleQuery::Or(operands)
    })
}

fn style_in_parens(items: &[&Value], pos: &mut usize, depth: usize) -> Option<StyleQuery> {
    let value = *items.get(*pos)?;
    let query = if let Some(contents) = paren_block(value) {
        // Past the nesting limit the block is general-enclosed.
        let nested = (depth < MAX_MEDIA_NESTING)
            .then(|| parse_style_query(contents, depth + 1))
            .flatten();
        match nested {
            // `((x))` around general-enclosed is itself general-enclosed;
            // keeping the outer block preserves the parentheses.
            Some(StyleQuery::Unknown(_)) | None => StyleQuery::Unknown(value.clone()),
            Some(query) => query,
        }
    } else if matches!(value.value, crate::ComponentValue::Function { .. }) {
        StyleQuery::Unknown(value.clone())
    } else {
        return None;
    };
    *pos += 1;
    Some(query)
}

// ---------------------------------------------------------------------------
// Evaluation

impl ContainerCondition {
    /// The axes the query needs sizes on.
    pub fn axes(&self) -> ContainerAxes {
        let mut axes = ContainerAxes::default();
        if let Some(query) = &self.query {
            collect_axes(query, &mut axes);
        }
        axes
    }
}

/// Recursion is bounded by the parser's nesting limit.
fn collect_axes(condition: &MediaCondition, axes: &mut ContainerAxes) {
    use MediaFeatureName as F;
    match condition {
        MediaCondition::Not(inner) => collect_axes(inner, axes),
        MediaCondition::And(operands) | MediaCondition::Or(operands) => {
            for operand in operands {
                collect_axes(operand, axes);
            }
        }
        MediaCondition::Feature(feature) => {
            let name = match feature {
                MediaFeature::Boolean(name)
                | MediaFeature::Plain { name, .. }
                | MediaFeature::Range { name, .. } => *name,
            };
            match name {
                F::Width | F::InlineSize => axes.inline = true,
                F::Height | F::BlockSize => axes.block = true,
                _ => {
                    axes.inline = true;
                    axes.block = true;
                }
            }
        }
        MediaCondition::Style(_) | MediaCondition::Unknown(_) => {}
    }
}

impl ContainerFacts {
    fn length_px(&self, length: Length) -> f64 {
        if let Some(px) = length.to_px() {
            return px;
        }
        let v = length.value;
        let (font, root) = (self.font_size_px, self.root_font_size_px);
        let (w, h) = (self.viewport_width, self.viewport_height);
        match length.unit {
            LengthUnit::Em | LengthUnit::Ic => v * font,
            LengthUnit::Rem | LengthUnit::Ric => v * root,
            LengthUnit::Ex | LengthUnit::Ch => v * font / 2.0,
            LengthUnit::Rex | LengthUnit::Rch => v * root / 2.0,
            LengthUnit::Cap => v * font * 0.7,
            LengthUnit::Rcap => v * root * 0.7,
            LengthUnit::Lh => v * font * 1.2,
            LengthUnit::Rlh => v * root * 1.2,
            LengthUnit::Vw
            | LengthUnit::Vi
            | LengthUnit::Svw
            | LengthUnit::Lvw
            | LengthUnit::Dvw => v * w / 100.0,
            LengthUnit::Vh
            | LengthUnit::Vb
            | LengthUnit::Svh
            | LengthUnit::Lvh
            | LengthUnit::Dvh => v * h / 100.0,
            LengthUnit::Vmin => v * w.min(h) / 100.0,
            LengthUnit::Vmax => v * w.max(h) / 100.0,
            _ => v,
        }
    }

    fn size(&self, name: MediaFeatureName) -> Option<f64> {
        use MediaFeatureName as F;
        match name {
            F::Width | F::InlineSize => self.width,
            F::Height | F::BlockSize => self.height,
            _ => None,
        }
    }

    fn compare(
        &self,
        name: MediaFeatureName,
        value: &MediaFeatureValue,
    ) -> Option<std::cmp::Ordering> {
        match value {
            MediaFeatureValue::Length(l) => self.size(name)?.partial_cmp(&self.length_px(*l)),
            MediaFeatureValue::Ratio(a, b) if name == MediaFeatureName::AspectRatio => {
                let (w, h) = (self.width?, self.height?);
                (w * b).partial_cmp(&(h * a))
            }
            _ => None,
        }
    }

    fn feature(&self, feature: &MediaFeature) -> Option<bool> {
        use MediaFeatureName as F;
        match feature {
            MediaFeature::Boolean(F::AspectRatio) => Some(self.width? > 0.0 && self.height? > 0.0),
            MediaFeature::Boolean(F::Orientation) => {
                self.width?;
                self.height?;
                Some(true)
            }
            MediaFeature::Boolean(name) => Some(self.size(*name)? > 0.0),
            MediaFeature::Plain {
                name: F::Orientation,
                value: MediaFeatureValue::Keyword(k),
                ..
            } => {
                let portrait = self.height? >= self.width?;
                Some((*k == "portrait") == portrait)
            }
            MediaFeature::Plain {
                name,
                prefix,
                value,
            } => {
                let ordering = self.compare(*name, value)?;
                Some(match prefix {
                    Some(MediaPrefix::Min) => MediaComparison::Ge.holds(ordering),
                    Some(MediaPrefix::Max) => MediaComparison::Le.holds(ordering),
                    None => MediaComparison::Eq.holds(ordering),
                })
            }
            MediaFeature::Range { name, comparisons } => {
                let mut result = true;
                for (op, value) in comparisons {
                    result &= op.holds(self.compare(*name, value)?);
                }
                Some(result)
            }
        }
    }
}

/// Kleene `and`/`or` over `operands`.
fn fold(results: impl Iterator<Item = Option<bool>>, and: bool) -> Option<bool> {
    let mut result = Some(and);
    for r in results {
        match r {
            Some(b) if b != and => return Some(b),
            None => result = None,
            Some(_) => {}
        }
    }
    result
}

impl MediaCondition {
    /// Evaluate a container condition against `facts`, with `style`
    /// answering each `style()` feature. Three-valued: `None` is unknown.
    pub fn evaluate_container(
        &self,
        facts: &ContainerFacts,
        style: &dyn Fn(&str, Option<&CustomValue>) -> Option<bool>,
    ) -> Option<bool> {
        match self {
            Self::Not(inner) => inner.evaluate_container(facts, style).map(|b| !b),
            Self::And(operands) => fold(
                operands.iter().map(|o| o.evaluate_container(facts, style)),
                true,
            ),
            Self::Or(operands) => fold(
                operands.iter().map(|o| o.evaluate_container(facts, style)),
                false,
            ),
            Self::Feature(feature) => facts.feature(feature),
            Self::Style(query) => query.evaluate(style),
            Self::Unknown(_) => None,
        }
    }
}

impl StyleQuery {
    /// Evaluate with `feature` answering each `name[: value]` test.
    pub fn evaluate(
        &self,
        feature: &dyn Fn(&str, Option<&CustomValue>) -> Option<bool>,
    ) -> Option<bool> {
        match self {
            Self::Not(inner) => inner.evaluate(feature).map(|b| !b),
            Self::And(operands) => fold(operands.iter().map(|o| o.evaluate(feature)), true),
            Self::Or(operands) => fold(operands.iter().map(|o| o.evaluate(feature)), false),
            Self::Feature { name, value } => feature(name, value.as_ref()),
            Self::Unknown(_) => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Serialization

fn write_style_operand(f: &mut fmt::Formatter<'_>, operand: &StyleQuery) -> fmt::Result {
    match operand {
        StyleQuery::Unknown(_) => write!(f, "{operand}"),
        _ => write!(f, "({operand})"),
    }
}

impl fmt::Display for StyleQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Not(inner) => {
                f.write_str("not ")?;
                write_style_operand(f, inner)
            }
            Self::And(operands) | Self::Or(operands) => {
                let joiner = if matches!(self, Self::And(_)) {
                    " and "
                } else {
                    " or "
                };
                for (i, operand) in operands.iter().enumerate() {
                    if i > 0 {
                        f.write_str(joiner)?;
                    }
                    write_style_operand(f, operand)?;
                }
                Ok(())
            }
            Self::Feature { name, value } => {
                serialize_identifier(f, name)?;
                match value {
                    Some(value) => write!(f, ": {value}"),
                    None => Ok(()),
                }
            }
            Self::Unknown(value) => {
                let mut out = String::new();
                write_values(&mut out, std::slice::from_ref(value))?;
                f.write_str(&out)
            }
        }
    }
}

impl fmt::Display for ContainerCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(name) = &self.name {
            serialize_identifier(f, name)?;
            if self.query.is_some() {
                f.write_char(' ')?;
            }
        }
        match &self.query {
            Some(query) => write!(f, "{query}"),
            None => Ok(()),
        }
    }
}

impl fmt::Display for ContainerQueryList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, condition) in self.conditions.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{condition}")?;
        }
        Ok(())
    }
}
