//! Media queries (Media Queries Level 4, with the Level 5 features that need
//! no new value types): parsing, CSSOM serialization and evaluation.
//!
//! A malformed query in a list becomes `not all`, as the specification
//! requires; the rest of the list is unaffected. A parenthesized expression
//! that is not a known feature with a valid value (an unknown feature name,
//! a value of the wrong type) is `<general-enclosed>` and evaluates to
//! *unknown*, which counts as false at the top level and stays unknown under
//! `not`.

use std::fmt::{self, Write};

use crate::color::write_number;
use crate::container::{StyleQuery, parse_style_function};
use crate::serialize::write_values;
use crate::{BlockKind, ComponentValue, Length, LengthUnit, Token, Value, serialize_identifier};

/// How deeply media conditions may nest in parentheses before the rest is
/// treated as `<general-enclosed>` (unknown).
pub const MAX_MEDIA_NESTING: usize = 32;

/// A comma-separated media query list. An empty list matches everything.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaQueryList {
    /// The queries; the list matches if any of them does.
    pub queries: Vec<MediaQuery>,
}

/// One media query.
#[derive(Clone, Debug, PartialEq)]
pub enum MediaQuery {
    /// A query that failed to parse; it behaves (and serializes) as `not all`.
    Invalid,
    /// A media type with an optional condition, or a bare condition.
    Query {
        /// `not` or `only` before the media type.
        qualifier: Option<MediaQualifier>,
        /// The media type; `None` for a bare `<media-condition>`.
        media_type: Option<MediaType>,
        /// The condition after `and` (or the whole query when there is no
        /// media type).
        condition: Option<MediaCondition>,
    },
}

/// `not` or `only`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaQualifier {
    /// Negates the query.
    Not,
    /// Hides the query from legacy user agents; no effect on evaluation.
    Only,
}

/// A media type. Deprecated and unknown types parse but never match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MediaType {
    /// `all`.
    All,
    /// `screen`.
    Screen,
    /// `print`.
    Print,
    /// Any other identifier, lowercased (`tv`, `handheld`, ...).
    Other(String),
}

/// A media condition: features combined with `not`, `and` and `or`.
#[derive(Clone, Debug, PartialEq)]
pub enum MediaCondition {
    /// `not <media-in-parens>`.
    Not(Box<MediaCondition>),
    /// Two or more operands joined by `and`.
    And(Vec<MediaCondition>),
    /// Two or more operands joined by `or`.
    Or(Vec<MediaCondition>),
    /// A media feature test.
    Feature(MediaFeature),
    /// `style(...)`, in container queries only.
    Style(StyleQuery),
    /// `<general-enclosed>`: a function or parenthesized block that is not a
    /// valid feature or condition. Always evaluates to unknown.
    Unknown(Value),
}

/// A known media feature (Media Queries 4 §4 and 5 §6–§12).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaFeatureName {
    /// `width`.
    Width,
    /// `height`.
    Height,
    /// `device-width` (deprecated).
    DeviceWidth,
    /// `device-height` (deprecated).
    DeviceHeight,
    /// `aspect-ratio`.
    AspectRatio,
    /// `device-aspect-ratio` (deprecated).
    DeviceAspectRatio,
    /// `orientation`.
    Orientation,
    /// `resolution`.
    Resolution,
    /// `scan`.
    Scan,
    /// `grid`.
    Grid,
    /// `update`.
    Update,
    /// `overflow-block`.
    OverflowBlock,
    /// `overflow-inline`.
    OverflowInline,
    /// `color`.
    Color,
    /// `color-index`.
    ColorIndex,
    /// `monochrome`.
    Monochrome,
    /// `color-gamut`.
    ColorGamut,
    /// `dynamic-range`.
    DynamicRange,
    /// `video-dynamic-range`.
    VideoDynamicRange,
    /// `pointer`.
    Pointer,
    /// `any-pointer`.
    AnyPointer,
    /// `hover`.
    Hover,
    /// `any-hover`.
    AnyHover,
    /// `prefers-color-scheme`.
    PrefersColorScheme,
    /// `prefers-reduced-motion`.
    PrefersReducedMotion,
    /// `prefers-reduced-transparency`.
    PrefersReducedTransparency,
    /// `prefers-reduced-data`.
    PrefersReducedData,
    /// `prefers-contrast`.
    PrefersContrast,
    /// `forced-colors`.
    ForcedColors,
    /// `inverted-colors`.
    InvertedColors,
    /// `scripting`.
    Scripting,
    /// `display-mode`.
    DisplayMode,
    /// `inline-size`, in container queries only.
    InlineSize,
    /// `block-size`, in container queries only.
    BlockSize,
}

/// Which features a condition may test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FeatureSet {
    /// `@media` and `media` attributes.
    Media,
    /// `@container`: size features and `style()`.
    Container,
}

/// What values a feature takes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FeatureType {
    Length,
    Ratio,
    Resolution,
    Integer,
    /// `grid`: `0` or `1`, no range forms.
    Boolean,
    Keywords(&'static [&'static str]),
}

impl FeatureType {
    fn is_range(self) -> bool {
        matches!(
            self,
            Self::Length | Self::Ratio | Self::Resolution | Self::Integer
        )
    }
}

macro_rules! features {
    ($($variant:ident => $name:literal, $kind:expr;)*) => {
        impl MediaFeatureName {
            /// Every known feature.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)*];

            /// The feature's name.
            pub fn name(self) -> &'static str {
                match self { $(Self::$variant => $name,)* }
            }

            fn kind(self) -> FeatureType {
                match self { $(Self::$variant => $kind,)* }
            }
        }
    };
}

use FeatureType::{Boolean, Integer, Keywords, Ratio, Resolution};

features! {
    Width => "width", FeatureType::Length;
    Height => "height", FeatureType::Length;
    DeviceWidth => "device-width", FeatureType::Length;
    DeviceHeight => "device-height", FeatureType::Length;
    AspectRatio => "aspect-ratio", Ratio;
    DeviceAspectRatio => "device-aspect-ratio", Ratio;
    Orientation => "orientation", Keywords(&["portrait", "landscape"]);
    Resolution => "resolution", Resolution;
    Scan => "scan", Keywords(&["interlace", "progressive"]);
    Grid => "grid", Boolean;
    Update => "update", Keywords(&["none", "slow", "fast"]);
    OverflowBlock => "overflow-block", Keywords(&["none", "scroll", "paged"]);
    OverflowInline => "overflow-inline", Keywords(&["none", "scroll"]);
    Color => "color", Integer;
    ColorIndex => "color-index", Integer;
    Monochrome => "monochrome", Integer;
    ColorGamut => "color-gamut", Keywords(&["srgb", "p3", "rec2020"]);
    DynamicRange => "dynamic-range", Keywords(&["standard", "high"]);
    VideoDynamicRange => "video-dynamic-range", Keywords(&["standard", "high"]);
    Pointer => "pointer", Keywords(&["none", "coarse", "fine"]);
    AnyPointer => "any-pointer", Keywords(&["none", "coarse", "fine"]);
    Hover => "hover", Keywords(&["none", "hover"]);
    AnyHover => "any-hover", Keywords(&["none", "hover"]);
    PrefersColorScheme => "prefers-color-scheme", Keywords(&["light", "dark"]);
    PrefersReducedMotion => "prefers-reduced-motion", Keywords(&["no-preference", "reduce"]);
    PrefersReducedTransparency => "prefers-reduced-transparency", Keywords(&["no-preference", "reduce"]);
    PrefersReducedData => "prefers-reduced-data", Keywords(&["no-preference", "reduce"]);
    PrefersContrast => "prefers-contrast", Keywords(&["no-preference", "less", "more", "custom"]);
    ForcedColors => "forced-colors", Keywords(&["none", "active"]);
    InvertedColors => "inverted-colors", Keywords(&["none", "inverted"]);
    Scripting => "scripting", Keywords(&["none", "initial-only", "enabled"]);
    DisplayMode => "display-mode", Keywords(&["fullscreen", "standalone", "minimal-ui", "browser", "picture-in-picture"]);
    InlineSize => "inline-size", FeatureType::Length;
    BlockSize => "block-size", FeatureType::Length;
}

impl MediaFeatureName {
    /// The feature named `name` (ASCII case-insensitive), whether a media or
    /// a container feature.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|f| f.name().eq_ignore_ascii_case(name))
    }

    /// Whether container queries can test the feature.
    pub fn is_container_feature(self) -> bool {
        matches!(
            self,
            Self::Width
                | Self::Height
                | Self::InlineSize
                | Self::BlockSize
                | Self::AspectRatio
                | Self::Orientation
        )
    }

    fn in_set(self, set: FeatureSet) -> bool {
        match set {
            FeatureSet::Media => !matches!(self, Self::InlineSize | Self::BlockSize),
            FeatureSet::Container => self.is_container_feature(),
        }
    }
}

/// A `<resolution>` unit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolutionUnit {
    /// Dots per inch.
    Dpi,
    /// Dots per centimetre.
    Dpcm,
    /// Dots per CSS pixel.
    Dppx,
    /// `x`, an alias of `dppx`.
    X,
}

impl ResolutionUnit {
    fn from_name(name: &str) -> Option<Self> {
        [Self::Dpi, Self::Dpcm, Self::Dppx, Self::X]
            .into_iter()
            .find(|u| u.name().eq_ignore_ascii_case(name))
    }

    fn name(self) -> &'static str {
        match self {
            Self::Dpi => "dpi",
            Self::Dpcm => "dpcm",
            Self::Dppx => "dppx",
            Self::X => "x",
        }
    }

    fn dppx(self, value: f64) -> f64 {
        match self {
            Self::Dpi => value / 96.0,
            Self::Dpcm => value * 2.54 / 96.0,
            Self::Dppx | Self::X => value,
        }
    }
}

/// A media feature value.
#[derive(Clone, Debug, PartialEq)]
pub enum MediaFeatureValue {
    /// A `<length>`.
    Length(Length),
    /// A `<ratio>`: two non-negative numbers.
    Ratio(f64, f64),
    /// A `<resolution>`.
    Resolution(f64, ResolutionUnit),
    /// An `<integer>`.
    Integer(f64),
    /// A keyword, lowercased.
    Keyword(&'static str),
}

/// A comparison in a range test, read as `feature <op> value`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaComparison {
    /// `feature < value`.
    Lt,
    /// `feature <= value`.
    Le,
    /// `feature > value`.
    Gt,
    /// `feature >= value`.
    Ge,
    /// `feature = value`.
    Eq,
}

impl MediaComparison {
    pub(crate) fn flipped(self) -> Self {
        match self {
            Self::Lt => Self::Gt,
            Self::Le => Self::Ge,
            Self::Gt => Self::Lt,
            Self::Ge => Self::Le,
            Self::Eq => Self::Eq,
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Eq => "=",
        }
    }

    pub(crate) fn holds(self, ordering: std::cmp::Ordering) -> bool {
        use std::cmp::Ordering::{Equal, Greater, Less};
        match self {
            Self::Lt => ordering == Less,
            Self::Le => ordering != Greater,
            Self::Gt => ordering == Greater,
            Self::Ge => ordering != Less,
            Self::Eq => ordering == Equal,
        }
    }
}

/// A `min-` or `max-` prefix on a plain feature test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaPrefix {
    /// `min-`: the feature is at least the value.
    Min,
    /// `max-`: the feature is at most the value.
    Max,
}

/// A media feature test.
#[derive(Clone, Debug, PartialEq)]
pub enum MediaFeature {
    /// `(name)`: true unless the feature's value is zero or `none`.
    Boolean(MediaFeatureName),
    /// `(name: value)`, `(min-name: value)` or `(max-name: value)`.
    Plain {
        /// The feature.
        name: MediaFeatureName,
        /// `min-` or `max-`, for range features.
        prefix: Option<MediaPrefix>,
        /// The value.
        value: MediaFeatureValue,
    },
    /// `(name < value)`, `(value <= name)` or `(v1 < name < v2)`, stored as
    /// one or two `name <op> value` comparisons in source order.
    Range {
        /// The feature.
        name: MediaFeatureName,
        /// The comparisons.
        comparisons: Vec<(MediaComparison, MediaFeatureValue)>,
    },
}

// ---------------------------------------------------------------------------
// Parsing

pub(crate) fn ident(value: &Value) -> Option<&str> {
    match value.token()? {
        Token::Ident(name) => Some(name),
        _ => None,
    }
}

pub(crate) fn is_keyword(value: &Value, keyword: &str) -> bool {
    ident(value).is_some_and(|i| i.eq_ignore_ascii_case(keyword))
}

pub(crate) fn paren_block(value: &Value) -> Option<&[Value]> {
    match &value.value {
        ComponentValue::Block {
            kind: BlockKind::Paren,
            contents,
        } => Some(contents),
        _ => None,
    }
}

pub(crate) fn non_whitespace(values: &[Value]) -> Vec<&Value> {
    values.iter().filter(|v| !v.is_whitespace()).collect()
}

pub(crate) fn contains_bad_token(values: &[Value]) -> bool {
    values.iter().any(|v| match &v.value {
        ComponentValue::Token(Token::BadString | Token::BadUrl) => true,
        ComponentValue::Token(_) => false,
        ComponentValue::Function { arguments, .. } => contains_bad_token(arguments),
        ComponentValue::Block { contents, .. } => contains_bad_token(contents),
    })
}

/// Parse a media query list from component values, such as an `@media`
/// prelude or a `media` attribute's tokens.
pub fn parse_media_query_list(values: &[Value]) -> MediaQueryList {
    let items = non_whitespace(values);
    if items.is_empty() {
        return MediaQueryList {
            queries: Vec::new(),
        };
    }
    let queries = items
        .split(|v| matches!(v.token(), Some(Token::Comma)))
        .map(|query| parse_query(query).unwrap_or(MediaQuery::Invalid))
        .collect();
    MediaQueryList { queries }
}

fn media_type(name: &str) -> Option<MediaType> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "only" | "not" | "and" | "or" | "layer" => return None,
        "all" => MediaType::All,
        "screen" => MediaType::Screen,
        "print" => MediaType::Print,
        _ => MediaType::Other(lower),
    })
}

/// The deepest function and block nesting in `items` (1 for a block with
/// only tokens inside), computed without recursion.
pub(crate) fn nesting_depth(items: &[&Value]) -> usize {
    let mut deepest = 0;
    let mut stack: Vec<(&Value, usize)> = items.iter().map(|v| (*v, 0)).collect();
    while let Some((value, depth)) = stack.pop() {
        let inner = match &value.value {
            ComponentValue::Function { arguments, .. } => arguments,
            ComponentValue::Block { contents, .. } => contents,
            ComponentValue::Token(_) => continue,
        };
        deepest = deepest.max(depth + 1);
        stack.extend(inner.iter().map(|v| (v, depth + 1)));
    }
    deepest
}

/// A query nested deeper than [`MAX_MEDIA_NESTING`] is invalid as a whole,
/// so that where a nesting limit cuts it off never depends on parentheses
/// that serialization drops.
fn parse_query(items: &[&Value]) -> Option<MediaQuery> {
    if nesting_depth(items) > MAX_MEDIA_NESTING {
        return None;
    }
    // A bare condition (including `not (...)`).
    let bare = items.first().is_some_and(|v| {
        ident(v).is_none()
            || (is_keyword(v, "not") && items.get(1).is_some_and(|n| ident(n).is_none()))
    });
    if bare {
        let mut pos = 0;
        let condition = parse_condition(items, &mut pos, true, 0, FeatureSet::Media)?;
        return (pos == items.len()).then_some(MediaQuery::Query {
            qualifier: None,
            media_type: None,
            condition: Some(condition),
        });
    }
    let mut pos = 0;
    let qualifier = match items.first().and_then(|v| ident(v)) {
        Some(q) if q.eq_ignore_ascii_case("not") => Some(MediaQualifier::Not),
        Some(q) if q.eq_ignore_ascii_case("only") => Some(MediaQualifier::Only),
        _ => None,
    };
    if qualifier.is_some() {
        pos += 1;
    }
    let media_type = media_type(ident(items.get(pos)?)?)?;
    pos += 1;
    let condition = if pos == items.len() {
        None
    } else {
        if !is_keyword(items[pos], "and") {
            return None;
        }
        pos += 1;
        let condition = parse_condition(items, &mut pos, false, 0, FeatureSet::Media)?;
        if pos != items.len() {
            return None;
        }
        Some(condition)
    };
    Some(MediaQuery::Query {
        qualifier,
        media_type: Some(media_type),
        condition,
    })
}

pub(crate) fn parse_condition(
    items: &[&Value],
    pos: &mut usize,
    allow_or: bool,
    depth: usize,
    set: FeatureSet,
) -> Option<MediaCondition> {
    if items.get(*pos).is_some_and(|v| is_keyword(v, "not")) {
        *pos += 1;
        return parse_in_parens(items, pos, depth, set).map(|c| MediaCondition::Not(Box::new(c)));
    }
    let first = parse_in_parens(items, pos, depth, set)?;
    let joiner = match items.get(*pos) {
        Some(v) if is_keyword(v, "and") => "and",
        Some(v) if allow_or && is_keyword(v, "or") => "or",
        _ => return Some(first),
    };
    let mut operands = vec![first];
    while items.get(*pos).is_some_and(|v| is_keyword(v, joiner)) {
        *pos += 1;
        operands.push(parse_in_parens(items, pos, depth, set)?);
    }
    Some(if joiner == "and" {
        MediaCondition::And(operands)
    } else {
        MediaCondition::Or(operands)
    })
}

fn parse_in_parens(
    items: &[&Value],
    pos: &mut usize,
    depth: usize,
    set: FeatureSet,
) -> Option<MediaCondition> {
    let value = *items.get(*pos)?;
    let result = if let Some(contents) = paren_block(value) {
        let inner = non_whitespace(contents);
        let mut inner_pos = 0;
        // Past the nesting limit a nested condition is left unparsed and
        // treated as `<general-enclosed>`, which keeps recursion bounded.
        let nested = if depth < MAX_MEDIA_NESTING {
            parse_condition(&inner, &mut inner_pos, true, depth + 1, set)
        } else {
            None
        };
        match nested {
            // `((x))` around general-enclosed is itself general-enclosed;
            // keeping the outer block preserves the parentheses.
            Some(MediaCondition::Unknown(_)) if inner_pos == inner.len() => {
                MediaCondition::Unknown(value.clone())
            }
            Some(condition) if inner_pos == inner.len() => condition,
            _ => match parse_feature(&inner, set) {
                Some(feature) => MediaCondition::Feature(feature),
                None if contains_bad_token(contents) => return None,
                None => MediaCondition::Unknown(value.clone()),
            },
        }
    } else if let ComponentValue::Function { name, arguments } = &value.value {
        if contains_bad_token(arguments) {
            return None;
        }
        let style = (set == FeatureSet::Container && name.eq_ignore_ascii_case("style"))
            .then(|| parse_style_function(arguments))
            .flatten();
        match style {
            Some(query) => MediaCondition::Style(query),
            None => MediaCondition::Unknown(value.clone()),
        }
    } else {
        return None;
    };
    *pos += 1;
    Some(result)
}

/// A comparison operator starting at `items[pos]`, with its length in items.
fn comparison(items: &[&Value], pos: usize) -> Option<(MediaComparison, usize)> {
    let delim = |i: usize| match items.get(i)?.token()? {
        Token::Delim(c) => Some(*c),
        _ => None,
    };
    let first = delim(pos)?;
    // `<=` and `>=` are two adjacent delim tokens.
    let with_equals = delim(pos + 1) == Some('=')
        && items[pos].span.range().end == items[pos + 1].span.range().start;
    match (first, with_equals) {
        ('<', true) => Some((MediaComparison::Le, 2)),
        ('>', true) => Some((MediaComparison::Ge, 2)),
        ('<', false) => Some((MediaComparison::Lt, 1)),
        ('>', false) => Some((MediaComparison::Gt, 1)),
        ('=', _) => Some((MediaComparison::Eq, 1)),
        _ => None,
    }
}

/// `value` as it reads back from its serialization (six decimals), so a
/// query evaluates the same before and after a serialize/parse round trip:
/// `(device-width: 2e-8px)` serializes as `0px`, and so matches as `0px`.
fn serialized(value: f64) -> f64 {
    let mut text = String::new();
    if write_number(&mut text, value).is_err() {
        return value;
    }
    text.parse().unwrap_or(value)
}

/// A feature value of `kind` at the start of `items`, and how many items it
/// used.
fn feature_value(kind: FeatureType, items: &[&Value]) -> Option<(MediaFeatureValue, usize)> {
    let first = items.first()?.token()?;
    match kind {
        FeatureType::Length => match first {
            Token::Dimension(n, unit) => Some((
                MediaFeatureValue::Length(Length {
                    value: serialized(n.value),
                    unit: LengthUnit::from_name(unit)?,
                }),
                1,
            )),
            Token::Number(n) if n.value == 0.0 => {
                Some((MediaFeatureValue::Length(Length::px(0.0)), 1))
            }
            _ => None,
        },
        FeatureType::Ratio => {
            let number = |t: &Token| match t {
                Token::Number(n) if n.value >= 0.0 => Some(serialized(n.value)),
                _ => None,
            };
            let a = number(first)?;
            let slash = items
                .get(1)
                .and_then(|v| v.token())
                .is_some_and(|t| *t == Token::Delim('/'));
            if slash {
                let b = number(items.get(2)?.token()?)?;
                Some((MediaFeatureValue::Ratio(a, b), 3))
            } else {
                Some((MediaFeatureValue::Ratio(a, 1.0), 1))
            }
        }
        FeatureType::Resolution => match first {
            Token::Dimension(n, unit) if n.value >= 0.0 => Some((
                MediaFeatureValue::Resolution(
                    serialized(n.value),
                    ResolutionUnit::from_name(unit)?,
                ),
                1,
            )),
            _ => None,
        },
        FeatureType::Integer => match first {
            Token::Number(n) if n.integer => Some((MediaFeatureValue::Integer(n.value), 1)),
            _ => None,
        },
        FeatureType::Boolean => match first {
            Token::Number(n) if n.integer && (n.value == 0.0 || n.value == 1.0) => {
                Some((MediaFeatureValue::Integer(n.value), 1))
            }
            _ => None,
        },
        FeatureType::Keywords(keywords) => {
            let Token::Ident(name) = first else {
                return None;
            };
            let keyword = keywords.iter().find(|k| k.eq_ignore_ascii_case(name))?;
            Some((MediaFeatureValue::Keyword(keyword), 1))
        }
    }
}

/// The whole of `items` as one value of `kind`.
fn whole_value(kind: FeatureType, items: &[&Value]) -> Option<MediaFeatureValue> {
    let (value, used) = feature_value(kind, items)?;
    (used == items.len()).then_some(value)
}

fn parse_feature(items: &[&Value], set: FeatureSet) -> Option<MediaFeature> {
    let named = |raw: &str| MediaFeatureName::from_name(raw).filter(|n| n.in_set(set));
    // `(name)` and `(name: value)`.
    if let Some(raw) = items.first().and_then(|v| ident(v)) {
        if items.len() == 1 {
            return named(raw).map(MediaFeature::Boolean);
        }
        if matches!(items[1].token(), Some(Token::Colon)) {
            let lower = raw.to_ascii_lowercase();
            let (prefix, base) = if let Some(base) = lower.strip_prefix("min-") {
                (Some(MediaPrefix::Min), base)
            } else if let Some(base) = lower.strip_prefix("max-") {
                (Some(MediaPrefix::Max), base)
            } else {
                (None, lower.as_str())
            };
            let name = named(base)?;
            if prefix.is_some() && !name.kind().is_range() {
                return None;
            }
            let value = whole_value(name.kind(), &items[2..])?;
            return Some(MediaFeature::Plain {
                name,
                prefix,
                value,
            });
        }
        // `(name <op> value)`.
        let name = named(raw)?;
        if !name.kind().is_range() {
            return None;
        }
        let (op, len) = comparison(items, 1)?;
        let value = whole_value(name.kind(), &items[1 + len..])?;
        return Some(MediaFeature::Range {
            name,
            comparisons: vec![(op, value)],
        });
    }
    // `(value <op> name)` and `(value <op> name <op> value)`: try each
    // feature position.
    let name_at = items.iter().position(|v| ident(v).is_some())?;
    let name = named(ident(items[name_at])?)?;
    let kind = name.kind();
    if !kind.is_range() {
        return None;
    }
    let (left, used) = feature_value(kind, items)?;
    let (op1, len1) = comparison(items, used)?;
    if used + len1 != name_at {
        return None;
    }
    let mut comparisons = vec![(op1.flipped(), left)];
    let rest = name_at + 1;
    if rest < items.len() {
        let (op2, len2) = comparison(items, rest)?;
        let right = whole_value(kind, &items[rest + len2..])?;
        let ascending = matches!(op1, MediaComparison::Lt | MediaComparison::Le);
        let descending = matches!(op1, MediaComparison::Gt | MediaComparison::Ge);
        let ok = (ascending && matches!(op2, MediaComparison::Lt | MediaComparison::Le))
            || (descending && matches!(op2, MediaComparison::Gt | MediaComparison::Ge));
        if !ok {
            return None;
        }
        comparisons.push((op2, right));
    }
    Some(MediaFeature::Range { name, comparisons })
}

// ---------------------------------------------------------------------------
// Evaluation

/// A pointing device's accuracy (`pointer`, `any-pointer`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointerAccuracy {
    /// No pointing device.
    None,
    /// A coarse pointer, such as a finger.
    Coarse,
    /// A fine pointer, such as a mouse.
    Fine,
}

/// The user's color scheme preference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorScheme {
    /// Light.
    Light,
    /// Dark.
    Dark,
}

/// Sizes of the font-relative units `ex`, `ch`, `cap` and `ic` as multiples
/// of the font size, taken from a font's metrics.
///
/// Media and container queries cannot see fonts, so the embedder (or the
/// style system, for containers) passes the ratios of the relevant font.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontUnitRatios {
    /// `ex`: the x-height.
    pub ex: f64,
    /// `ch`: the advance of `0`.
    pub ch: f64,
    /// `cap`: the cap height.
    pub cap: f64,
    /// `ic`: the advance of 水.
    pub ic: f64,
}

impl FontUnitRatios {
    /// The ratios without a font: CSS Values 4 fallbacks of 0.5em for `ex`
    /// and `ch`, 1em for `ic`, and 0.7em for `cap`.
    pub const FALLBACK: Self = Self {
        ex: 0.5,
        ch: 0.5,
        cap: 0.7,
        ic: 1.0,
    };
}

impl Default for FontUnitRatios {
    fn default() -> Self {
        Self::FALLBACK
    }
}

/// Everything media queries can test, for one rendering surface.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaEnvironment {
    /// `screen` or `print` (or `all`, which only `all` queries match).
    pub media_type: MediaType,
    /// Viewport width in CSS pixels.
    pub width: f64,
    /// Viewport height in CSS pixels.
    pub height: f64,
    /// Screen width in CSS pixels.
    pub device_width: f64,
    /// Screen height in CSS pixels.
    pub device_height: f64,
    /// Device pixels per CSS pixel.
    pub resolution: f64,
    /// Bits per color component (0 for monochrome devices).
    pub color: u32,
    /// Color lookup table entries (0 without a table).
    pub color_index: u32,
    /// Bits per pixel on monochrome devices.
    pub monochrome: u32,
    /// The primary pointer's accuracy.
    pub pointer: PointerAccuracy,
    /// The most accurate available pointer.
    pub any_pointer: PointerAccuracy,
    /// Whether the primary pointer can hover.
    pub hover: bool,
    /// Whether any pointer can hover.
    pub any_hover: bool,
    /// The preferred color scheme.
    pub color_scheme: ColorScheme,
    /// `prefers-reduced-motion: reduce`.
    pub reduced_motion: bool,
    /// `prefers-reduced-transparency: reduce`.
    pub reduced_transparency: bool,
    /// `prefers-reduced-data: reduce`.
    pub reduced_data: bool,
    /// `prefers-contrast` as its keyword.
    pub contrast: &'static str,
    /// `forced-colors: active`.
    pub forced_colors: bool,
    /// `inverted-colors: inverted`.
    pub inverted_colors: bool,
    /// `scripting` as its keyword (`none` until JavaScript runs).
    pub scripting: &'static str,
    /// `display-mode` as its keyword.
    pub display_mode: &'static str,
    /// `color-gamut` as its keyword.
    pub color_gamut: &'static str,
    /// `dynamic-range: high`.
    pub high_dynamic_range: bool,
    /// The initial font's unit ratios, for `ex`, `ch`, `cap` and `ic` in
    /// queries ([`FontUnitRatios::FALLBACK`] until fonts are known).
    pub initial_font: FontUnitRatios,
}

impl Default for MediaEnvironment {
    /// A 1280×720 desktop screen with a mouse, light scheme and no scripts.
    fn default() -> Self {
        Self::screen(1280.0, 720.0)
    }
}

impl MediaEnvironment {
    /// A desktop screen whose viewport and device are `width` × `height`.
    pub fn screen(width: f64, height: f64) -> Self {
        Self {
            media_type: MediaType::Screen,
            width,
            height,
            device_width: width,
            device_height: height,
            resolution: 1.0,
            color: 8,
            color_index: 0,
            monochrome: 0,
            pointer: PointerAccuracy::Fine,
            any_pointer: PointerAccuracy::Fine,
            hover: true,
            any_hover: true,
            color_scheme: ColorScheme::Light,
            reduced_motion: false,
            reduced_transparency: false,
            reduced_data: false,
            contrast: "no-preference",
            forced_colors: false,
            inverted_colors: false,
            scripting: "none",
            display_mode: "browser",
            color_gamut: "srgb",
            high_dynamic_range: false,
            initial_font: FontUnitRatios::FALLBACK,
        }
    }

    fn print(&self) -> bool {
        self.media_type == MediaType::Print
    }

    fn length_px(&self, length: Length) -> f64 {
        if let Some(px) = length.to_px() {
            return px;
        }
        // Relative units use the initial font (16px, `normal` line height
        // 1.2) and the viewport, as Media Queries 4 §1.3 requires.
        const INITIAL_FONT_PX: f64 = 16.0;
        let v = length.value;
        let (w, h) = (self.width, self.height);
        let font = self.initial_font;
        match length.unit {
            LengthUnit::Em | LengthUnit::Rem => v * INITIAL_FONT_PX,
            LengthUnit::Ex | LengthUnit::Rex => v * INITIAL_FONT_PX * font.ex,
            LengthUnit::Ch | LengthUnit::Rch => v * INITIAL_FONT_PX * font.ch,
            LengthUnit::Cap | LengthUnit::Rcap => v * INITIAL_FONT_PX * font.cap,
            LengthUnit::Ic | LengthUnit::Ric => v * INITIAL_FONT_PX * font.ic,
            LengthUnit::Lh | LengthUnit::Rlh => v * INITIAL_FONT_PX * 1.2,
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

    fn keyword(&self, name: MediaFeatureName) -> &'static str {
        use MediaFeatureName as F;
        let pointer = |p: PointerAccuracy| match p {
            PointerAccuracy::None => "none",
            PointerAccuracy::Coarse => "coarse",
            PointerAccuracy::Fine => "fine",
        };
        let flag = |on: bool, yes: &'static str, no: &'static str| if on { yes } else { no };
        match name {
            F::Orientation => flag(self.height >= self.width, "portrait", "landscape"),
            F::Scan => "progressive",
            F::Update => flag(self.print(), "none", "fast"),
            F::OverflowBlock => flag(self.print(), "paged", "scroll"),
            F::OverflowInline => flag(self.print(), "none", "scroll"),
            F::ColorGamut => self.color_gamut,
            F::DynamicRange | F::VideoDynamicRange => {
                flag(self.high_dynamic_range, "high", "standard")
            }
            F::Pointer => pointer(self.pointer),
            F::AnyPointer => pointer(self.any_pointer),
            F::Hover => flag(self.hover, "hover", "none"),
            F::AnyHover => flag(self.any_hover, "hover", "none"),
            F::PrefersColorScheme => flag(self.color_scheme == ColorScheme::Dark, "dark", "light"),
            F::PrefersReducedMotion => flag(self.reduced_motion, "reduce", "no-preference"),
            F::PrefersReducedTransparency => {
                flag(self.reduced_transparency, "reduce", "no-preference")
            }
            F::PrefersReducedData => flag(self.reduced_data, "reduce", "no-preference"),
            F::PrefersContrast => self.contrast,
            F::ForcedColors => flag(self.forced_colors, "active", "none"),
            F::InvertedColors => flag(self.inverted_colors, "inverted", "none"),
            F::Scripting => self.scripting,
            F::DisplayMode => self.display_mode,
            _ => "",
        }
    }

    /// The environment's value for a range feature as a number, and the
    /// value to compare it with.
    fn compare(
        &self,
        name: MediaFeatureName,
        value: &MediaFeatureValue,
    ) -> Option<std::cmp::Ordering> {
        use MediaFeatureName as F;
        let ratio = |w: f64, h: f64, a: f64, b: f64| (w * b).partial_cmp(&(h * a));
        match (name, value) {
            (F::Width | F::InlineSize, MediaFeatureValue::Length(l)) => {
                self.width.partial_cmp(&self.length_px(*l))
            }
            (F::Height | F::BlockSize, MediaFeatureValue::Length(l)) => {
                self.height.partial_cmp(&self.length_px(*l))
            }
            (F::DeviceWidth, MediaFeatureValue::Length(l)) => {
                self.device_width.partial_cmp(&self.length_px(*l))
            }
            (F::DeviceHeight, MediaFeatureValue::Length(l)) => {
                self.device_height.partial_cmp(&self.length_px(*l))
            }
            (F::AspectRatio, MediaFeatureValue::Ratio(a, b)) => {
                ratio(self.width, self.height, *a, *b)
            }
            (F::DeviceAspectRatio, MediaFeatureValue::Ratio(a, b)) => {
                ratio(self.device_width, self.device_height, *a, *b)
            }
            (F::Resolution, MediaFeatureValue::Resolution(v, unit)) => {
                self.resolution.partial_cmp(&unit.dppx(*v))
            }
            (F::Color, MediaFeatureValue::Integer(n)) => f64::from(self.color).partial_cmp(n),
            (F::ColorIndex, MediaFeatureValue::Integer(n)) => {
                f64::from(self.color_index).partial_cmp(n)
            }
            (F::Monochrome, MediaFeatureValue::Integer(n)) => {
                f64::from(self.monochrome).partial_cmp(n)
            }
            (F::Grid, MediaFeatureValue::Integer(n)) => 0.0f64.partial_cmp(n),
            _ => None,
        }
    }

    fn boolean(&self, name: MediaFeatureName) -> bool {
        use MediaFeatureName as F;
        match name {
            F::Width | F::InlineSize => self.width > 0.0,
            F::Height | F::BlockSize => self.height > 0.0,
            F::DeviceWidth => self.device_width > 0.0,
            F::DeviceHeight => self.device_height > 0.0,
            F::AspectRatio => self.width > 0.0 && self.height > 0.0,
            F::DeviceAspectRatio => self.device_width > 0.0 && self.device_height > 0.0,
            F::Resolution => self.resolution > 0.0,
            F::Color => self.color > 0,
            F::ColorIndex => self.color_index > 0,
            F::Monochrome => self.monochrome > 0,
            F::Grid => false,
            other => {
                let keyword = self.keyword(other);
                keyword != "none" && keyword != "no-preference"
            }
        }
    }

    fn feature(&self, feature: &MediaFeature) -> Option<bool> {
        match feature {
            MediaFeature::Boolean(name) => Some(self.boolean(*name)),
            MediaFeature::Plain {
                name,
                prefix,
                value,
            } => {
                if let MediaFeatureValue::Keyword(k) = value {
                    return Some(self.keyword(*name) == *k);
                }
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

    fn condition(&self, condition: &MediaCondition) -> Option<bool> {
        match condition {
            MediaCondition::Not(inner) => self.condition(inner).map(|b| !b),
            MediaCondition::And(operands) => {
                // Kleene logic: false wins over unknown.
                let mut result = Some(true);
                for operand in operands {
                    match self.condition(operand) {
                        Some(false) => return Some(false),
                        None => result = None,
                        Some(true) => {}
                    }
                }
                result
            }
            MediaCondition::Or(operands) => {
                let mut result = Some(false);
                for operand in operands {
                    match self.condition(operand) {
                        Some(true) => return Some(true),
                        None => result = None,
                        Some(false) => {}
                    }
                }
                result
            }
            MediaCondition::Feature(feature) => self.feature(feature),
            MediaCondition::Style(_) | MediaCondition::Unknown(_) => None,
        }
    }

    fn query(&self, query: &MediaQuery) -> bool {
        let MediaQuery::Query {
            qualifier,
            media_type,
            condition,
        } = query
        else {
            return false;
        };
        let type_matches = match media_type {
            None | Some(MediaType::All) => true,
            Some(MediaType::Other(_)) => false,
            Some(t) => *t == self.media_type,
        };
        let result = if type_matches {
            match condition {
                Some(c) => self.condition(c),
                None => Some(true),
            }
        } else {
            Some(false)
        };
        match (result, qualifier) {
            (None, _) => false,
            (Some(b), Some(MediaQualifier::Not)) => !b,
            (Some(b), _) => b,
        }
    }
}

impl MediaQueryList {
    /// Whether the list matches `environment`: empty lists match, otherwise
    /// any query must.
    pub fn matches(&self, environment: &MediaEnvironment) -> bool {
        self.queries.is_empty() || self.queries.iter().any(|q| environment.query(q))
    }
}

// ---------------------------------------------------------------------------
// Serialization (CSSOM §4.2)

impl fmt::Display for MediaQueryList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, query) in self.queries.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{query}")?;
        }
        Ok(())
    }
}

impl fmt::Display for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => f.write_str("all"),
            Self::Screen => f.write_str("screen"),
            Self::Print => f.write_str("print"),
            Self::Other(name) => serialize_identifier(f, name),
        }
    }
}

impl fmt::Display for MediaQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::Query {
            qualifier,
            media_type,
            condition,
        } = self
        else {
            return f.write_str("not all");
        };
        match qualifier {
            Some(MediaQualifier::Not) => f.write_str("not ")?,
            Some(MediaQualifier::Only) => f.write_str("only ")?,
            None => {}
        }
        match (media_type, condition) {
            // After a media type, `or` needs parentheses.
            (Some(t), Some(c @ MediaCondition::Or(_))) => write!(f, "{t} and ({c})"),
            (Some(t), Some(c)) => write!(f, "{t} and {c}"),
            (Some(t), None) => write!(f, "{t}"),
            (None, Some(c)) => write!(f, "{c}"),
            (None, None) => f.write_str("all"),
        }
    }
}

fn write_operand(f: &mut fmt::Formatter<'_>, operand: &MediaCondition) -> fmt::Result {
    match operand {
        MediaCondition::Feature(_) | MediaCondition::Style(_) | MediaCondition::Unknown(_) => {
            write!(f, "{operand}")
        }
        _ => write!(f, "({operand})"),
    }
}

impl fmt::Display for MediaCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Not(inner) => {
                f.write_str("not ")?;
                write_operand(f, inner)
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
                    write_operand(f, operand)?;
                }
                Ok(())
            }
            Self::Feature(feature) => write!(f, "{feature}"),
            Self::Style(query) => write!(f, "style({query})"),
            Self::Unknown(value) => {
                let mut out = String::new();
                write_values(&mut out, std::slice::from_ref(value))?;
                f.write_str(&out)
            }
        }
    }
}

impl fmt::Display for MediaFeatureValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length(l) => write!(f, "{l}"),
            Self::Ratio(a, b) => {
                write_number(f, *a)?;
                f.write_str(" / ")?;
                write_number(f, *b)
            }
            Self::Resolution(v, unit) => {
                write_number(f, *v)?;
                f.write_str(unit.name())
            }
            Self::Integer(n) => write_number(f, *n),
            Self::Keyword(k) => f.write_str(k),
        }
    }
}

impl fmt::Display for MediaFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Boolean(name) => write!(f, "({})", name.name()),
            Self::Plain {
                name,
                prefix,
                value,
            } => {
                let prefix = match prefix {
                    Some(MediaPrefix::Min) => "min-",
                    Some(MediaPrefix::Max) => "max-",
                    None => "",
                };
                write!(f, "({prefix}{}: {value})", name.name())
            }
            Self::Range { name, comparisons } => {
                f.write_char('(')?;
                match comparisons.as_slice() {
                    [(op, value)] => write!(f, "{} {} {value}", name.name(), op.symbol())?,
                    [(op1, v1), (op2, v2)] => write!(
                        f,
                        "{v1} {} {} {} {v2}",
                        op1.flipped().symbol(),
                        name.name(),
                        op2.symbol()
                    )?,
                    _ => return Err(fmt::Error),
                }
                f.write_char(')')
            }
        }
    }
}
