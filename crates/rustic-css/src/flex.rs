//! Sizing keywords (CSS Sizing 3 §3), flexbox properties (CSS Flexible Box
//! Layout 1 §5–§8) and the box alignment values flex layout uses (CSS Box
//! Alignment 3 §4–§8), reviewed 2026-11-09.
//!
//! `width`, `height`, `min-width` and `min-height` take [`Size`];
//! `max-width`/`max-height` take [`crate::MaxSize`], which carries the same
//! keywords. `flex-basis` is `content | <'width'>`. Alignment properties share
//! one [`Alignment`] value whose keyword set is checked per property.

use std::fmt;

use crate::properties::{ident, keywords, nonnegative};
use crate::{ComponentValue, LengthPercentage, Value};

/// `auto | <length-percentage [0,∞]> | min-content | max-content |
/// fit-content | fit-content(<length-percentage [0,∞]>)`.
///
/// Bare `fit-content` is CSS Sizing 4; it uses the available space as the
/// argument. Layout treats the keywords as `auto` in the block axis of a
/// block container (CSS Sizing 3 §3.1).
#[derive(Clone, Debug, PartialEq)]
pub enum Size {
    /// `auto`.
    Auto,
    /// A nonnegative length or percentage.
    LengthPercentage(LengthPercentage),
    /// `min-content`.
    MinContent,
    /// `max-content`.
    MaxContent,
    /// `fit-content` (`None`) or `fit-content(<length-percentage>)`.
    FitContent(Option<LengthPercentage>),
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::LengthPercentage(lp) => write!(f, "{lp}"),
            Self::MinContent => f.write_str("min-content"),
            Self::MaxContent => f.write_str("max-content"),
            Self::FitContent(None) => f.write_str("fit-content"),
            Self::FitContent(Some(lp)) => write!(f, "fit-content({lp})"),
        }
    }
}

/// An intrinsic sizing keyword shared by [`Size`] and [`crate::MaxSize`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SizeKeyword {
    Min,
    Max,
    Fit(Option<LengthPercentage>),
}

/// `min-content`, `max-content`, `fit-content` or `fit-content(<lp>)`.
pub(crate) fn size_keyword(value: &Value) -> Option<SizeKeyword> {
    if let ComponentValue::Function { name, arguments } = &value.value {
        if !name.eq_ignore_ascii_case("fit-content") {
            return None;
        }
        let parts: Vec<&Value> = arguments.iter().filter(|v| !v.is_whitespace()).collect();
        let [argument] = parts.as_slice() else {
            return None;
        };
        return nonnegative(argument).map(|lp| SizeKeyword::Fit(Some(lp)));
    }
    Some(match ident(value)?.to_ascii_lowercase().as_str() {
        "min-content" => SizeKeyword::Min,
        "max-content" => SizeKeyword::Max,
        "fit-content" => SizeKeyword::Fit(None),
        _ => return None,
    })
}

/// Parse a [`Size`].
pub(crate) fn size(value: &Value) -> Option<Size> {
    if ident(value).is_some_and(|i| i.eq_ignore_ascii_case("auto")) {
        return Some(Size::Auto);
    }
    if let Some(keyword) = size_keyword(value) {
        return Some(match keyword {
            SizeKeyword::Min => Size::MinContent,
            SizeKeyword::Max => Size::MaxContent,
            SizeKeyword::Fit(lp) => Size::FitContent(lp),
        });
    }
    nonnegative(value).map(Size::LengthPercentage)
}

keywords! {
    /// `flex-direction` values.
    FlexDirection {
        Row => "row",
        RowReverse => "row-reverse",
        Column => "column",
        ColumnReverse => "column-reverse",
    }
}

impl FlexDirection {
    /// Whether the main axis is the inline (horizontal) axis.
    pub fn is_row(self) -> bool {
        matches!(self, Self::Row | Self::RowReverse)
    }

    /// Whether main-start and main-end are swapped.
    pub fn is_reverse(self) -> bool {
        matches!(self, Self::RowReverse | Self::ColumnReverse)
    }
}

keywords! {
    /// `flex-wrap` values.
    FlexWrap {
        Nowrap => "nowrap",
        Wrap => "wrap",
        WrapReverse => "wrap-reverse",
    }
}

/// `flex-basis`: `content | <'width'>`.
#[derive(Clone, Debug, PartialEq)]
pub enum FlexBasis {
    /// `content`: size from the item's content.
    Content,
    /// A `width`-like value; `auto` defers to the main size property.
    Size(Size),
}

impl fmt::Display for FlexBasis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Content => f.write_str("content"),
            Self::Size(size) => write!(f, "{size}"),
        }
    }
}

pub(crate) fn flex_basis(value: &Value) -> Option<FlexBasis> {
    if ident(value).is_some_and(|i| i.eq_ignore_ascii_case("content")) {
        return Some(FlexBasis::Content);
    }
    size(value).map(FlexBasis::Size)
}

/// `row-gap` and `column-gap`: `normal | <length-percentage [0,∞]>`.
#[derive(Clone, Debug, PartialEq)]
pub enum Gap {
    /// `normal` (zero in flex layout).
    Normal,
    /// A nonnegative length or percentage.
    LengthPercentage(LengthPercentage),
}

impl fmt::Display for Gap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Normal => f.write_str("normal"),
            Self::LengthPercentage(lp) => write!(f, "{lp}"),
        }
    }
}

pub(crate) fn gap(value: &Value) -> Option<Gap> {
    if ident(value).is_some_and(|i| i.eq_ignore_ascii_case("normal")) {
        return Some(Gap::Normal);
    }
    nonnegative(value).map(Gap::LengthPercentage)
}

keywords! {
    /// Alignment keywords used by `justify-content`, `align-content`,
    /// `align-items`, `align-self`, `justify-items` and `justify-self`.
    /// Which keywords a property accepts is checked when parsing; the
    /// `legacy` forms are `justify-items` only.
    AlignKeyword {
        Auto => "auto",
        Normal => "normal",
        Stretch => "stretch",
        Baseline => "baseline",
        LastBaseline => "last baseline",
        Center => "center",
        Start => "start",
        End => "end",
        SelfStart => "self-start",
        SelfEnd => "self-end",
        FlexStart => "flex-start",
        FlexEnd => "flex-end",
        FlowStart => "flow-start",
        FlowEnd => "flow-end",
        Left => "left",
        Right => "right",
        SpaceBetween => "space-between",
        SpaceAround => "space-around",
        SpaceEvenly => "space-evenly",
        Legacy => "legacy",
        LegacyLeft => "legacy left",
        LegacyRight => "legacy right",
        LegacyCenter => "legacy center",
    }
}

/// `safe` or `unsafe` (`<overflow-position>`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverflowPosition {
    /// `safe`: overflow falls back to `start`.
    Safe,
    /// `unsafe`: honor the alignment even if content overflows.
    Unsafe,
}

/// A box alignment value: a keyword and an optional overflow position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Alignment {
    /// The alignment keyword (`first baseline` is [`AlignKeyword::Baseline`]).
    pub keyword: AlignKeyword,
    /// `safe`/`unsafe`, only with a positional keyword.
    pub overflow: Option<OverflowPosition>,
}

impl Alignment {
    /// A keyword with no overflow position.
    pub const fn new(keyword: AlignKeyword) -> Self {
        Self {
            keyword,
            overflow: None,
        }
    }
}

impl fmt::Display for Alignment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.overflow {
            Some(OverflowPosition::Safe) => f.write_str("safe ")?,
            Some(OverflowPosition::Unsafe) => f.write_str("unsafe ")?,
            None => {}
        }
        f.write_str(self.keyword.name())
    }
}

/// Which alignment property is being parsed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AlignProperty {
    JustifyContent,
    AlignContent,
    AlignItems,
    AlignSelf,
    JustifyItems,
    JustifySelf,
}

/// Parse an alignment value for `property` (CSS Box Alignment 3 §5–§6).
pub(crate) fn alignment(parts: &[&Value], property: AlignProperty) -> Option<Alignment> {
    use AlignKeyword as K;
    let names: Vec<String> = parts
        .iter()
        .map(|v| ident(v).map(str::to_ascii_lowercase))
        .collect::<Option<_>>()?;
    let words: Vec<&str> = names.iter().map(String::as_str).collect();
    let content = matches!(
        property,
        AlignProperty::JustifyContent | AlignProperty::AlignContent
    );
    let justify = matches!(
        property,
        AlignProperty::JustifyContent | AlignProperty::JustifyItems | AlignProperty::JustifySelf
    );
    // `legacy && [left | right | center]?` (CSS Box Alignment 3 §6.3).
    if property == AlignProperty::JustifyItems && words.contains(&"legacy") {
        let other: Vec<&str> = words.iter().copied().filter(|w| *w != "legacy").collect();
        return match (words.len(), other.as_slice()) {
            (1, []) => Some(Alignment::new(K::Legacy)),
            (2, ["left"]) => Some(Alignment::new(K::LegacyLeft)),
            (2, ["right"]) => Some(Alignment::new(K::LegacyRight)),
            (2, ["center"]) => Some(Alignment::new(K::LegacyCenter)),
            _ => None,
        };
    }
    // `<baseline-position>`: `[first | last]? baseline`.
    let baseline = match words.as_slice() {
        ["baseline"] | ["first", "baseline"] => Some(K::Baseline),
        ["last", "baseline"] => Some(K::LastBaseline),
        _ => None,
    };
    if let Some(keyword) = baseline {
        return (property != AlignProperty::JustifyContent).then_some(Alignment::new(keyword));
    }
    let (overflow, rest) = match words.as_slice() {
        ["safe", rest @ ..] => (Some(OverflowPosition::Safe), rest),
        ["unsafe", rest @ ..] => (Some(OverflowPosition::Unsafe), rest),
        rest => (None, rest),
    };
    let [word] = rest else {
        return None;
    };
    let keyword = match *word {
        "first" | "last" | "baseline" | "legacy" => return None,
        other => AlignKeyword::from_name(other)?,
    };
    let positional = match keyword {
        K::Center | K::Start | K::End | K::FlexStart | K::FlexEnd | K::FlowStart | K::FlowEnd => {
            true
        }
        K::Left | K::Right => justify,
        K::SelfStart | K::SelfEnd => !content,
        _ => false,
    };
    // `<overflow-position>? normal` for the self and items properties
    // (CSS Box Alignment 3, 2025 Editor's Draft).
    let normal_overflow = keyword == K::Normal && !content;
    if overflow.is_some() {
        return (positional || normal_overflow).then_some(Alignment { keyword, overflow });
    }
    let accepted = positional
        || match keyword {
            K::Normal | K::Stretch => true,
            K::Auto => matches!(
                property,
                AlignProperty::AlignSelf | AlignProperty::JustifySelf
            ),
            K::SpaceBetween | K::SpaceAround | K::SpaceEvenly => content,
            _ => false,
        };
    accepted.then_some(Alignment::new(keyword))
}

/// `place-content`, `place-items`, `place-self`: the `align-*` value then an
/// optional `justify-*` value; one value sets both (CSS Box Alignment 3
/// §5.5, §6.4, §6.5). `property` is the `align-*` longhand. A lone baseline
/// value sets `justify-content` to `start`, which takes no baseline.
pub(crate) fn place(
    parts: &[&Value],
    property: AlignProperty,
) -> Option<Vec<crate::PropertyValue>> {
    let justify = match property {
        AlignProperty::AlignContent => AlignProperty::JustifyContent,
        AlignProperty::AlignItems => AlignProperty::JustifyItems,
        _ => AlignProperty::JustifySelf,
    };
    for split in 1..=parts.len() {
        let Some(align) = alignment(&parts[..split], property) else {
            continue;
        };
        let rest = &parts[split..];
        let other = if rest.is_empty() {
            match alignment(&parts[..split], justify) {
                Some(v) => v,
                None if matches!(
                    align.keyword,
                    AlignKeyword::Baseline | AlignKeyword::LastBaseline
                ) && justify == AlignProperty::JustifyContent =>
                {
                    Alignment::new(AlignKeyword::Start)
                }
                None => continue,
            }
        } else {
            match alignment(rest, justify) {
                Some(v) => v,
                None => continue,
            }
        };
        return Some(vec![
            crate::PropertyValue::Alignment(align),
            crate::PropertyValue::Alignment(other),
        ]);
    }
    None
}
