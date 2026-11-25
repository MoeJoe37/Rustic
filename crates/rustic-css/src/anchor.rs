//! Values of CSS Anchor Positioning Level 1 (editor's draft reviewed
//! 2026-11-25): `anchor-name`, `position-anchor`, `position-area` and the
//! `anchor()` and `anchor-size()` functions in insets and sizes, plus
//! `view-transition-name` (CSS View Transitions Level 1 §3).
//!
//! `anchor()` and `anchor-size()` are accepted as the whole value of an
//! inset or size property, not inside math functions (TD-216). Such a value
//! keeps its fallback as the ordinary value of the property, so code that
//! does not resolve anchors sees the fallback (or `auto`).

use std::fmt;
use std::sync::Arc;

use crate::properties::{ident, keywords, length_percentage};
use crate::{ComponentValue, LengthPercentage, PropertyValue, Token, Value};

/// The `<anchor-side>` of `anchor()`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AnchorSide {
    /// `top`.
    Top,
    /// `right`.
    Right,
    /// `bottom`.
    Bottom,
    /// `left`.
    Left,
    /// `start`.
    Start,
    /// `end`.
    End,
    /// `self-start`.
    SelfStart,
    /// `self-end`.
    SelfEnd,
    /// `center`.
    Center,
    /// `inside`.
    Inside,
    /// `outside`.
    Outside,
    /// A `<percentage>` along the axis, from the start edge.
    Percentage(f64),
}

impl AnchorSide {
    fn parse(value: &Value) -> Option<Self> {
        if let Some(Token::Percentage(n)) = value.token() {
            return Some(Self::Percentage(n.value));
        }
        Some(match ident(value)?.to_ascii_lowercase().as_str() {
            "top" => Self::Top,
            "right" => Self::Right,
            "bottom" => Self::Bottom,
            "left" => Self::Left,
            "start" => Self::Start,
            "end" => Self::End,
            "self-start" => Self::SelfStart,
            "self-end" => Self::SelfEnd,
            "center" => Self::Center,
            "inside" => Self::Inside,
            "outside" => Self::Outside,
            _ => return None,
        })
    }
}

impl fmt::Display for AnchorSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Top => "top",
            Self::Right => "right",
            Self::Bottom => "bottom",
            Self::Left => "left",
            Self::Start => "start",
            Self::End => "end",
            Self::SelfStart => "self-start",
            Self::SelfEnd => "self-end",
            Self::Center => "center",
            Self::Inside => "inside",
            Self::Outside => "outside",
            Self::Percentage(p) => {
                crate::color::write_number(f, *p)?;
                return f.write_str("%");
            }
        })
    }
}

keywords! {
    /// The `<anchor-size>` of `anchor-size()`.
    AnchorSizeKeyword {
        Width => "width",
        Height => "height",
        Block => "block",
        Inline => "inline",
        SelfBlock => "self-block",
        SelfInline => "self-inline",
    }
}

/// `anchor(<anchor-name>? <anchor-side>, <length-percentage>?)` or
/// `anchor-size(<anchor-name>? <anchor-size>?, <length-percentage>?)`.
#[derive(Clone, Debug, PartialEq)]
pub enum AnchorFunction {
    /// `anchor()`: an edge of the anchor box.
    Edge {
        /// The anchor name; `None` uses `position-anchor`.
        name: Option<Arc<str>>,
        /// The edge.
        side: AnchorSide,
        /// The fallback when the anchor cannot be resolved.
        fallback: Option<LengthPercentage>,
    },
    /// `anchor-size()`: a dimension of the anchor box.
    Size {
        /// The anchor name; `None` uses `position-anchor`.
        name: Option<Arc<str>>,
        /// The dimension; `None` is the axis of the property.
        size: Option<AnchorSizeKeyword>,
        /// The fallback when the anchor cannot be resolved.
        fallback: Option<LengthPercentage>,
    },
}

impl AnchorFunction {
    /// The anchor name the function names, if any.
    pub fn name(&self) -> Option<&Arc<str>> {
        match self {
            Self::Edge { name, .. } | Self::Size { name, .. } => name.as_ref(),
        }
    }

    /// The fallback length, if any.
    pub fn fallback(&self) -> Option<&LengthPercentage> {
        match self {
            Self::Edge { fallback, .. } | Self::Size { fallback, .. } => fallback.as_ref(),
        }
    }

    /// The same function with its fallback mapped (to compute lengths).
    pub fn map_fallback(&self, map: impl Fn(&LengthPercentage) -> LengthPercentage) -> Self {
        let mut out = self.clone();
        match &mut out {
            Self::Edge { fallback, .. } | Self::Size { fallback, .. } => {
                *fallback = fallback.as_ref().map(map);
            }
        }
        out
    }

    /// Parse `anchor()` (when `size` is false) or `anchor-size()`.
    fn parse(value: &Value, size: bool) -> Option<Self> {
        let ComponentValue::Function { name, arguments } = &value.value else {
            return None;
        };
        let expected = if size { "anchor-size" } else { "anchor" };
        if !name.eq_ignore_ascii_case(expected) {
            return None;
        }
        let args: Vec<&Value> = arguments.iter().filter(|v| !v.is_whitespace()).collect();
        let (head, fallback) = match args.iter().position(|v| v.token() == Some(&Token::Comma)) {
            Some(comma) => {
                let [one] = &args[comma + 1..] else {
                    return None;
                };
                (&args[..comma], Some(length_percentage(one)?))
            }
            None => (&args[..], None),
        };
        let mut anchor = None;
        let mut rest = Vec::new();
        for value in head {
            match ident(value) {
                Some(n) if n.starts_with("--") && anchor.is_none() => {
                    anchor = Some(Arc::from(n));
                }
                _ => rest.push(*value),
            }
        }
        if size {
            let size = match rest.as_slice() {
                [] => None,
                [one] => Some(AnchorSizeKeyword::from_name(ident(one)?)?),
                _ => return None,
            };
            Some(Self::Size {
                name: anchor,
                size,
                fallback,
            })
        } else {
            let [side] = rest.as_slice() else {
                return None;
            };
            Some(Self::Edge {
                name: anchor,
                side: AnchorSide::parse(side)?,
                fallback,
            })
        }
    }
}

impl fmt::Display for AnchorFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (function, name, fallback) = match self {
            Self::Edge { name, fallback, .. } => ("anchor(", name, fallback),
            Self::Size { name, fallback, .. } => ("anchor-size(", name, fallback),
        };
        f.write_str(function)?;
        let mut wrote = false;
        if let Some(name) = name {
            crate::serialize::serialize_identifier(f, name)?;
            wrote = true;
        }
        let rest = match self {
            Self::Edge { side, .. } => Some(side.to_string()),
            Self::Size { size, .. } => size.map(|s| s.name().to_owned()),
        };
        if let Some(rest) = rest {
            if wrote {
                f.write_str(" ")?;
            }
            f.write_str(&rest)?;
        }
        if let Some(fallback) = fallback {
            write!(f, ", {fallback}")?;
        }
        f.write_str(")")
    }
}

/// An inset or size whose value is `anchor()` or `anchor-size()`.
#[derive(Clone, Debug, PartialEq)]
pub struct Anchored {
    /// The function.
    pub function: AnchorFunction,
    /// The value the property takes when the function cannot be resolved:
    /// the fallback, or `auto` (`none` for maximum sizes).
    pub fallback: PropertyValue,
}

impl fmt::Display for Anchored {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.function)
    }
}

/// The grid cells a `position-area` covers in one axis: the cell before
/// the anchor, the anchor's own cell, and the cell after it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AreaSpan {
    /// The cell from the containing block's start edge to the anchor.
    pub start: bool,
    /// The anchor's cell.
    pub center: bool,
    /// The cell from the anchor to the containing block's end edge.
    pub end: bool,
}

impl AreaSpan {
    /// All three cells (`span-all`).
    pub const ALL: Self = Self {
        start: true,
        center: true,
        end: true,
    };

    const fn of(start: bool, center: bool, end: bool) -> Self {
        Self { start, center, end }
    }

    fn keyword(self, start: &str, end: &str) -> String {
        match (self.start, self.center, self.end) {
            (true, false, false) => start.to_owned(),
            (false, false, true) => end.to_owned(),
            (false, true, false) => "center".to_owned(),
            (true, true, false) => format!("span-{start}"),
            (false, true, true) => format!("span-{end}"),
            _ => "span-all".to_owned(),
        }
    }
}

/// A `position-area` grid region (logical keywords resolve as
/// `horizontal-tb`, `ltr`: TD-216).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PositionArea {
    /// The horizontal cells.
    pub x: AreaSpan,
    /// The vertical cells.
    pub y: AreaSpan,
}

#[derive(Clone, Copy, PartialEq)]
enum AreaAxis {
    X,
    Y,
    Either,
}

fn area_keyword(name: &str) -> Option<(AreaAxis, AreaSpan)> {
    let lower = name.to_ascii_lowercase();
    let (span, base) = match lower.strip_prefix("span-") {
        Some(base) => (true, base),
        None => (false, lower.as_str()),
    };
    let (axis, at_start) = match base {
        "left" | "x-start" | "inline-start" | "x-self-start" | "self-inline-start" => {
            (AreaAxis::X, true)
        }
        "right" | "x-end" | "inline-end" | "x-self-end" | "self-inline-end" => (AreaAxis::X, false),
        "top" | "y-start" | "block-start" | "y-self-start" | "self-block-start" => {
            (AreaAxis::Y, true)
        }
        "bottom" | "y-end" | "block-end" | "y-self-end" | "self-block-end" => (AreaAxis::Y, false),
        "start" | "self-start" => (AreaAxis::Either, true),
        "end" | "self-end" => (AreaAxis::Either, false),
        "center" if !span => return Some((AreaAxis::Either, AreaSpan::of(false, true, false))),
        "all" if span => return Some((AreaAxis::Either, AreaSpan::ALL)),
        _ => return None,
    };
    let cells = match (span, at_start) {
        (false, true) => AreaSpan::of(true, false, false),
        (false, false) => AreaSpan::of(false, false, true),
        (true, true) => AreaSpan::of(true, true, false),
        (true, false) => AreaSpan::of(false, true, true),
    };
    Some((axis, cells))
}

impl PositionArea {
    fn parse(parts: &[&Value]) -> Option<Self> {
        let keywords: Vec<(AreaAxis, AreaSpan)> = parts
            .iter()
            .map(|v| area_keyword(ident(v)?))
            .collect::<Option<_>>()?;
        match keywords.as_slice() {
            [(AreaAxis::X, x)] => Some(Self {
                x: *x,
                y: AreaSpan::ALL,
            }),
            [(AreaAxis::Y, y)] => Some(Self {
                x: AreaSpan::ALL,
                y: *y,
            }),
            [(AreaAxis::Either, both)] => Some(Self { x: *both, y: *both }),
            [(a, first), (b, second)] => {
                use AreaAxis::{Either, X, Y};
                let (x, y) = match (a, b) {
                    (X, Y) | (X, Either) | (Either, Y) => (first, second),
                    // Two ambiguous keywords name the block axis first.
                    (Y, X) | (Y, Either) | (Either, X) | (Either, Either) => (second, first),
                    _ => return None,
                };
                Some(Self { x: *x, y: *y })
            }
            _ => None,
        }
    }
}

impl fmt::Display for PositionArea {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let x = self.x.keyword("left", "right");
        let y = self.y.keyword("top", "bottom");
        match (x.as_str(), y.as_str()) {
            ("span-all", "span-all") => f.write_str("span-all"),
            ("center", "center") => f.write_str("center"),
            (x, "span-all") if x != "center" => f.write_str(x),
            ("span-all", y) if y != "center" => f.write_str(y),
            (x, y) => write!(f, "{y} {x}"),
        }
    }
}

/// The value of an anchor positioning or view transition property.
#[derive(Clone, Debug, PartialEq)]
pub enum AnchorValue {
    /// `anchor-name`: dashed idents; empty for `none`.
    Names(Vec<Arc<str>>),
    /// `position-anchor`: a dashed ident, or `None` for `auto`.
    Default(Option<Arc<str>>),
    /// `position-area`: `None` for `none`.
    Area(Option<PositionArea>),
    /// `view-transition-name`: a custom ident, or `None` for `none`.
    TransitionName(Option<Arc<str>>),
}

impl fmt::Display for AnchorValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let write_name =
            |f: &mut fmt::Formatter<'_>, name: &Option<Arc<str>>, none: &str| match name {
                Some(name) => crate::serialize::serialize_identifier(f, name),
                None => f.write_str(none),
            };
        match self {
            Self::Names(names) if names.is_empty() => f.write_str("none"),
            Self::Names(names) => {
                for (i, name) in names.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    crate::serialize::serialize_identifier(f, name)?;
                }
                Ok(())
            }
            Self::Default(name) => write_name(f, name, "auto"),
            Self::Area(None) => f.write_str("none"),
            Self::Area(Some(area)) => write!(f, "{area}"),
            Self::TransitionName(name) => write_name(f, name, "none"),
        }
    }
}

fn dashed(value: &Value) -> Option<Arc<str>> {
    ident(value)
        .filter(|n| n.starts_with("--") && n.len() > 2)
        .map(Arc::from)
}

/// Parse `anchor-name`, `position-anchor`, `position-area` or
/// `view-transition-name`.
pub(crate) fn longhand(property: crate::PropertyId, parts: &[&Value]) -> Option<PropertyValue> {
    use crate::PropertyId as P;
    let keyword = |word: &str| matches!(parts, [one] if ident(one).is_some_and(|n| n.eq_ignore_ascii_case(word)));
    let value = match property {
        P::AnchorName => {
            if keyword("none") {
                AnchorValue::Names(Vec::new())
            } else {
                let mut names = Vec::new();
                let mut expect_name = true;
                for part in parts {
                    if expect_name {
                        names.push(dashed(part)?);
                    } else if part.token() != Some(&Token::Comma) {
                        return None;
                    }
                    expect_name = !expect_name;
                }
                if expect_name {
                    return None;
                }
                AnchorValue::Names(names)
            }
        }
        P::PositionAnchor => AnchorValue::Default(if keyword("auto") {
            None
        } else {
            let [one] = parts else {
                return None;
            };
            Some(dashed(one)?)
        }),
        P::PositionArea => AnchorValue::Area(if keyword("none") {
            None
        } else {
            Some(PositionArea::parse(parts)?)
        }),
        P::ViewTransitionName => AnchorValue::TransitionName(if keyword("none") {
            None
        } else {
            let [one] = parts else {
                return None;
            };
            let name = ident(one)?;
            if crate::CssWideKeyword::from_name(name).is_some()
                || name.eq_ignore_ascii_case("auto")
                || name.eq_ignore_ascii_case("default")
            {
                return None;
            }
            Some(Arc::from(name))
        }),
        _ => return None,
    };
    Some(PropertyValue::Anchor(value))
}

/// The initial value of an anchor positioning or view transition property.
pub(crate) fn initial(property: crate::PropertyId) -> Option<AnchorValue> {
    use crate::PropertyId as P;
    Some(match property {
        P::AnchorName => AnchorValue::Names(Vec::new()),
        P::PositionAnchor => AnchorValue::Default(None),
        P::PositionArea => AnchorValue::Area(None),
        P::ViewTransitionName => AnchorValue::TransitionName(None),
        _ => return None,
    })
}

/// Parse `anchor()` in an inset property or `anchor-size()` in a size
/// property (`fallback` builds the property's value from the fallback).
pub(crate) fn anchored(
    value: &Value,
    size: bool,
    fallback: impl Fn(Option<&LengthPercentage>) -> Option<PropertyValue>,
) -> Option<PropertyValue> {
    let function = AnchorFunction::parse(value, size)?;
    let fallback = fallback(function.fallback())?;
    Some(PropertyValue::Anchored(Box::new(Anchored {
        function,
        fallback,
    })))
}
