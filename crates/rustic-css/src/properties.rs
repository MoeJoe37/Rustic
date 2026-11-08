//! Typed property values for the CSS-03 property set.
//!
//! [`parse_declaration`] turns a syntax-level [`Declaration`] into typed
//! longhand declarations, expanding shorthands. Unknown properties and invalid
//! values are reported, never guessed: a declaration that fails here is
//! dropped by the cascade exactly as CSS Syntax requires.
//!
//! Supported longhands: `display`, `color`, `background-color`, `margin-*`,
//! `padding-*`, `border-*-width`, `border-*-style`, `border-*-color`,
//! `width`, `height`, `min-width`, `min-height`, `max-width`, `max-height`,
//! `box-sizing`, `font-size`, `font-weight`, `font-style`, `font-family` and
//! `line-height`. Shorthands: `margin`, `padding`, `border-width`,
//! `border-style`, `border-color`, `border-top`/`-right`/`-bottom`/`-left`
//! and `border`. Every property also accepts the CSS-wide keywords.
//! Math functions (`calc()`, `min()`, `max()`, `clamp()`) are accepted in
//! every length, length-percentage and number slot (see [`crate::Calc`]).
//! `var()` substitution and the `font` shorthand are later tasks; their values
//! are rejected for now.

use std::fmt;

use crate::calc::{censor, is_math_function};
use crate::color::write_number;
use crate::serialize::serialize_string;
use crate::{
    Calc, Color, ComponentValue, Declaration, Length, LengthPercentage, LengthUnit, Token, Value,
};

/// The CSS-wide keywords (CSS Cascade Level 5 §7.3).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CssWideKeyword {
    /// `initial`.
    Initial,
    /// `inherit`.
    Inherit,
    /// `unset`.
    Unset,
    /// `revert`.
    Revert,
    /// `revert-layer`.
    RevertLayer,
}

impl CssWideKeyword {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "initial" => Self::Initial,
            "inherit" => Self::Inherit,
            "unset" => Self::Unset,
            "revert" => Self::Revert,
            "revert-layer" => Self::RevertLayer,
            _ => return None,
        })
    }

    /// The keyword as written in CSS.
    pub fn name(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Inherit => "inherit",
            Self::Unset => "unset",
            Self::Revert => "revert",
            Self::RevertLayer => "revert-layer",
        }
    }
}

macro_rules! keywords {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal,)* }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum $name {
            $(#[doc = concat!("`", $text, "`.")] $variant,)*
        }

        impl $name {
            fn from_name(name: &str) -> Option<Self> {
                match name.to_ascii_lowercase().as_str() {
                    $($text => Some(Self::$variant),)*
                    _ => None,
                }
            }

            /// The keyword as written in CSS.
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)*
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.name())
            }
        }
    };
}

keywords! {
    /// `display` values. Two-keyword forms are normalized to these.
    Display {
        None => "none",
        Contents => "contents",
        Block => "block",
        Inline => "inline",
        InlineBlock => "inline-block",
        FlowRoot => "flow-root",
        ListItem => "list-item",
        Flex => "flex",
        InlineFlex => "inline-flex",
        Grid => "grid",
        InlineGrid => "inline-grid",
        Table => "table",
        InlineTable => "inline-table",
        TableRowGroup => "table-row-group",
        TableHeaderGroup => "table-header-group",
        TableFooterGroup => "table-footer-group",
        TableRow => "table-row",
        TableCell => "table-cell",
        TableColumnGroup => "table-column-group",
        TableColumn => "table-column",
        TableCaption => "table-caption",
    }
}

keywords! {
    /// `border-style` values.
    BorderStyle {
        None => "none",
        Hidden => "hidden",
        Dotted => "dotted",
        Dashed => "dashed",
        Solid => "solid",
        Double => "double",
        Groove => "groove",
        Ridge => "ridge",
        Inset => "inset",
        Outset => "outset",
    }
}

keywords! {
    /// `box-sizing` values.
    BoxSizing {
        ContentBox => "content-box",
        BorderBox => "border-box",
    }
}

keywords! {
    /// `font-style` values (`oblique` angles are a later task).
    FontStyle {
        Normal => "normal",
        Italic => "italic",
        Oblique => "oblique",
    }
}

keywords! {
    /// `<absolute-size>` and `<relative-size>` keywords for `font-size`.
    FontSizeKeyword {
        XxSmall => "xx-small",
        XSmall => "x-small",
        Small => "small",
        Medium => "medium",
        Large => "large",
        XLarge => "x-large",
        XxLarge => "xx-large",
        XxxLarge => "xxx-large",
        Larger => "larger",
        Smaller => "smaller",
    }
}

keywords! {
    /// Generic font families.
    GenericFamily {
        Serif => "serif",
        SansSerif => "sans-serif",
        Monospace => "monospace",
        Cursive => "cursive",
        Fantasy => "fantasy",
        SystemUi => "system-ui",
        UiSerif => "ui-serif",
        UiSansSerif => "ui-sans-serif",
        UiMonospace => "ui-monospace",
        UiRounded => "ui-rounded",
        Math => "math",
        Emoji => "emoji",
        Fangsong => "fangsong",
    }
}

/// `auto | <length-percentage>`.
#[derive(Clone, Debug, PartialEq)]
pub enum LengthPercentageOrAuto {
    /// `auto`.
    Auto,
    /// A length or percentage.
    LengthPercentage(LengthPercentage),
}

/// `none | <length-percentage>`, for `max-width` and `max-height`.
#[derive(Clone, Debug, PartialEq)]
pub enum MaxSize {
    /// `none`.
    None,
    /// A length or percentage.
    LengthPercentage(LengthPercentage),
}

/// `thin | medium | thick | <length>`.
#[derive(Clone, Debug, PartialEq)]
pub enum BorderWidth {
    /// `thin` (1px).
    Thin,
    /// `medium` (3px).
    Medium,
    /// `thick` (5px).
    Thick,
    /// A nonnegative length.
    Length(Length),
    /// A math function resolving to a length; negative results clamp to 0.
    Calc(Box<Calc>),
}

/// `font-size`.
#[derive(Clone, Debug, PartialEq)]
pub enum FontSize {
    /// An absolute- or relative-size keyword.
    Keyword(FontSizeKeyword),
    /// A nonnegative length or percentage.
    LengthPercentage(LengthPercentage),
}

/// `font-weight`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FontWeight {
    /// `normal` (400).
    Normal,
    /// `bold` (700).
    Bold,
    /// `bolder`.
    Bolder,
    /// `lighter`.
    Lighter,
    /// A number in `1..=1000`.
    Number(f64),
}

/// One entry of `font-family`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FamilyName {
    /// A named family, from a string or a sequence of identifiers.
    Named(String),
    /// A generic family keyword.
    Generic(GenericFamily),
}

/// `line-height`.
#[derive(Clone, Debug, PartialEq)]
pub enum LineHeight {
    /// `normal`.
    Normal,
    /// A nonnegative number, multiplied by the font size.
    Number(f64),
    /// A nonnegative length or percentage.
    LengthPercentage(LengthPercentage),
}

/// A typed property value.
#[derive(Clone, Debug, PartialEq)]
pub enum PropertyValue {
    /// `display`.
    Display(Display),
    /// `color`, `background-color`, `border-*-color`.
    Color(Color),
    /// `margin-*`, `width`, `height`, `min-width`, `min-height`.
    LengthPercentageOrAuto(LengthPercentageOrAuto),
    /// `padding-*` (nonnegative).
    LengthPercentage(LengthPercentage),
    /// `max-width`, `max-height`.
    MaxSize(MaxSize),
    /// `border-*-width`.
    BorderWidth(BorderWidth),
    /// `border-*-style`.
    BorderStyle(BorderStyle),
    /// `box-sizing`.
    BoxSizing(BoxSizing),
    /// `font-size`.
    FontSize(FontSize),
    /// `font-weight`.
    FontWeight(FontWeight),
    /// `font-style`.
    FontStyle(FontStyle),
    /// `font-family`, in priority order.
    FontFamily(Vec<FamilyName>),
    /// `line-height`.
    LineHeight(LineHeight),
}

macro_rules! property_ids {
    ($($variant:ident => $name:literal, $inherited:literal;)*) => {
        /// A supported longhand property.
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum PropertyId {
            $(#[doc = concat!("`", $name, "`.")] $variant,)*
        }

        impl PropertyId {
            /// Every supported longhand, in a stable order.
            pub const ALL: &'static [PropertyId] = &[$(Self::$variant,)*];

            /// The longhand named `name`, ASCII case-insensitively.
            pub fn from_name(name: &str) -> Option<Self> {
                match name.to_ascii_lowercase().as_str() {
                    $($name => Some(Self::$variant),)*
                    _ => None,
                }
            }

            /// The property name.
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)*
                }
            }

            /// Whether the property is inherited by default.
            pub fn inherited(self) -> bool {
                match self {
                    $(Self::$variant => $inherited,)*
                }
            }
        }
    };
}

property_ids! {
    Display => "display", false;
    Color => "color", true;
    BackgroundColor => "background-color", false;
    MarginTop => "margin-top", false;
    MarginRight => "margin-right", false;
    MarginBottom => "margin-bottom", false;
    MarginLeft => "margin-left", false;
    PaddingTop => "padding-top", false;
    PaddingRight => "padding-right", false;
    PaddingBottom => "padding-bottom", false;
    PaddingLeft => "padding-left", false;
    BorderTopWidth => "border-top-width", false;
    BorderRightWidth => "border-right-width", false;
    BorderBottomWidth => "border-bottom-width", false;
    BorderLeftWidth => "border-left-width", false;
    BorderTopStyle => "border-top-style", false;
    BorderRightStyle => "border-right-style", false;
    BorderBottomStyle => "border-bottom-style", false;
    BorderLeftStyle => "border-left-style", false;
    BorderTopColor => "border-top-color", false;
    BorderRightColor => "border-right-color", false;
    BorderBottomColor => "border-bottom-color", false;
    BorderLeftColor => "border-left-color", false;
    Width => "width", false;
    Height => "height", false;
    MinWidth => "min-width", false;
    MinHeight => "min-height", false;
    MaxWidth => "max-width", false;
    MaxHeight => "max-height", false;
    BoxSizing => "box-sizing", false;
    FontSize => "font-size", true;
    FontWeight => "font-weight", true;
    FontStyle => "font-style", true;
    FontFamily => "font-family", true;
    LineHeight => "line-height", true;
}

impl PropertyId {
    /// The property's initial value.
    ///
    /// `color`'s initial value is `CanvasText` and `font-family`'s depends on
    /// the user agent; Rustic uses opaque black and `serif` until system
    /// colors and font settings exist.
    pub fn initial_value(self) -> PropertyValue {
        use PropertyId as P;
        let zero = LengthPercentage::ZERO;
        match self {
            P::Display => PropertyValue::Display(Display::Inline),
            P::Color => PropertyValue::Color(Color::rgb(0, 0, 0)),
            P::BackgroundColor => PropertyValue::Color(Color::named("transparent").expect("named")),
            P::MarginTop | P::MarginRight | P::MarginBottom | P::MarginLeft => {
                PropertyValue::LengthPercentageOrAuto(LengthPercentageOrAuto::LengthPercentage(
                    zero,
                ))
            }
            P::PaddingTop | P::PaddingRight | P::PaddingBottom | P::PaddingLeft => {
                PropertyValue::LengthPercentage(zero)
            }
            P::BorderTopWidth | P::BorderRightWidth | P::BorderBottomWidth | P::BorderLeftWidth => {
                PropertyValue::BorderWidth(BorderWidth::Medium)
            }
            P::BorderTopStyle | P::BorderRightStyle | P::BorderBottomStyle | P::BorderLeftStyle => {
                PropertyValue::BorderStyle(BorderStyle::None)
            }
            P::BorderTopColor | P::BorderRightColor | P::BorderBottomColor | P::BorderLeftColor => {
                PropertyValue::Color(Color::CurrentColor)
            }
            P::Width | P::Height | P::MinWidth | P::MinHeight => {
                PropertyValue::LengthPercentageOrAuto(LengthPercentageOrAuto::Auto)
            }
            P::MaxWidth | P::MaxHeight => PropertyValue::MaxSize(MaxSize::None),
            P::BoxSizing => PropertyValue::BoxSizing(BoxSizing::ContentBox),
            P::FontSize => PropertyValue::FontSize(FontSize::Keyword(FontSizeKeyword::Medium)),
            P::FontWeight => PropertyValue::FontWeight(FontWeight::Normal),
            P::FontStyle => PropertyValue::FontStyle(FontStyle::Normal),
            P::FontFamily => {
                PropertyValue::FontFamily(vec![FamilyName::Generic(GenericFamily::Serif)])
            }
            P::LineHeight => PropertyValue::LineHeight(LineHeight::Normal),
        }
    }
}

/// A declared longhand value: a CSS-wide keyword or a typed value.
#[derive(Clone, Debug, PartialEq)]
pub enum DeclaredValue {
    /// A CSS-wide keyword, resolved by the cascade.
    CssWide(CssWideKeyword),
    /// A typed value.
    Value(PropertyValue),
}

/// One typed longhand declaration.
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyDeclaration {
    /// The longhand.
    pub property: PropertyId,
    /// Its declared value.
    pub value: DeclaredValue,
    /// Whether the source declaration was `!important`.
    pub important: bool,
}

/// Why a declaration was not accepted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PropertyError {
    /// The property is not supported (or does not exist).
    UnknownProperty,
    /// The value does not match the property's grammar (or uses syntax that
    /// is not implemented yet, such as `var()`).
    InvalidValue,
}

impl fmt::Display for PropertyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnknownProperty => "unsupported property",
            Self::InvalidValue => "invalid property value",
        })
    }
}

impl std::error::Error for PropertyError {}

/// Parse a declaration into typed longhand declarations.
///
/// A shorthand expands to all of its longhands; a CSS-wide keyword applies
/// to each of them.
///
/// ```
/// use rustic_css::{CssParser, PropertyId, parse_declaration};
/// let declaration = CssParser::default().declaration("margin: 1px auto !important").unwrap().value.unwrap();
/// let longhands = parse_declaration(&declaration).unwrap();
/// assert_eq!(longhands.len(), 4);
/// assert_eq!(longhands[1].property, PropertyId::MarginRight);
/// assert_eq!(longhands[1].value.to_string(), "auto");
/// assert!(longhands.iter().all(|d| d.important));
/// ```
pub fn parse_declaration(
    declaration: &Declaration,
) -> Result<Vec<PropertyDeclaration>, PropertyError> {
    let parts: Vec<&Value> = declaration
        .value
        .iter()
        .filter(|v| !v.is_whitespace())
        .collect();
    let name = declaration.name.to_ascii_lowercase();
    let wide = match parts.as_slice() {
        [only] => match only.token() {
            Some(Token::Ident(ident)) => CssWideKeyword::from_name(ident),
            _ => None,
        },
        _ => None,
    };
    let longhands = longhands_of(&name).ok_or(PropertyError::UnknownProperty)?;
    let important = declaration.important;
    let make = |property, value| PropertyDeclaration {
        property,
        value,
        important,
    };
    if let Some(keyword) = wide {
        return Ok(longhands
            .iter()
            .map(|&p| make(p, DeclaredValue::CssWide(keyword)))
            .collect());
    }
    if parts.is_empty() {
        return Err(PropertyError::InvalidValue);
    }
    let values = if longhands.len() == 1 {
        vec![longhand(longhands[0], &parts).ok_or(PropertyError::InvalidValue)?]
    } else {
        shorthand(&name, &parts).ok_or(PropertyError::InvalidValue)?
    };
    Ok(longhands
        .iter()
        .zip(values)
        .map(|(&p, v)| make(p, DeclaredValue::Value(v)))
        .collect())
}

/// Longhands of a property name (one for a longhand, several for a shorthand).
fn longhands_of(name: &str) -> Option<&'static [PropertyId]> {
    use PropertyId as P;
    const MARGIN: &[P] = &[P::MarginTop, P::MarginRight, P::MarginBottom, P::MarginLeft];
    const PADDING: &[P] = &[
        P::PaddingTop,
        P::PaddingRight,
        P::PaddingBottom,
        P::PaddingLeft,
    ];
    const BORDER_WIDTH: &[P] = &[
        P::BorderTopWidth,
        P::BorderRightWidth,
        P::BorderBottomWidth,
        P::BorderLeftWidth,
    ];
    const BORDER_STYLE: &[P] = &[
        P::BorderTopStyle,
        P::BorderRightStyle,
        P::BorderBottomStyle,
        P::BorderLeftStyle,
    ];
    const BORDER_COLOR: &[P] = &[
        P::BorderTopColor,
        P::BorderRightColor,
        P::BorderBottomColor,
        P::BorderLeftColor,
    ];
    const BORDER: &[P] = &[
        P::BorderTopWidth,
        P::BorderRightWidth,
        P::BorderBottomWidth,
        P::BorderLeftWidth,
        P::BorderTopStyle,
        P::BorderRightStyle,
        P::BorderBottomStyle,
        P::BorderLeftStyle,
        P::BorderTopColor,
        P::BorderRightColor,
        P::BorderBottomColor,
        P::BorderLeftColor,
    ];
    const BORDER_TOP: &[P] = &[P::BorderTopWidth, P::BorderTopStyle, P::BorderTopColor];
    const BORDER_RIGHT: &[P] = &[
        P::BorderRightWidth,
        P::BorderRightStyle,
        P::BorderRightColor,
    ];
    const BORDER_BOTTOM: &[P] = &[
        P::BorderBottomWidth,
        P::BorderBottomStyle,
        P::BorderBottomColor,
    ];
    const BORDER_LEFT: &[P] = &[P::BorderLeftWidth, P::BorderLeftStyle, P::BorderLeftColor];
    Some(match name {
        "margin" => MARGIN,
        "padding" => PADDING,
        "border-width" => BORDER_WIDTH,
        "border-style" => BORDER_STYLE,
        "border-color" => BORDER_COLOR,
        "border" => BORDER,
        "border-top" => BORDER_TOP,
        "border-right" => BORDER_RIGHT,
        "border-bottom" => BORDER_BOTTOM,
        "border-left" => BORDER_LEFT,
        _ => {
            let id = PropertyId::from_name(name)?;
            let index = PropertyId::ALL.iter().position(|&p| p == id)?;
            &PropertyId::ALL[index..=index]
        }
    })
}

fn ident(value: &Value) -> Option<&str> {
    match value.token()? {
        Token::Ident(ident) => Some(ident),
        _ => None,
    }
}

fn keyword(value: &Value, expected: &str) -> bool {
    ident(value).is_some_and(|i| i.eq_ignore_ascii_case(expected))
}

fn length(value: &Value) -> Option<Length> {
    match value.token()? {
        Token::Dimension(n, unit) => Some(Length {
            value: n.value,
            unit: LengthUnit::from_name(unit)?,
        }),
        Token::Number(n) if n.value == 0.0 => Some(Length::px(0.0)),
        _ => None,
    }
}

fn math_function(value: &Value) -> bool {
    matches!(&value.value, ComponentValue::Function { name, .. } if is_math_function(name))
}

/// A `<length-percentage>`, including math functions. A math function stays
/// a [`LengthPercentage::Calc`] even when it simplifies to one value, so it
/// serializes as written and its range is checked only once it resolves.
fn length_percentage(value: &Value) -> Option<LengthPercentage> {
    if math_function(value) {
        return Calc::parse_length_percentage(value).map(|c| LengthPercentage::Calc(Box::new(c)));
    }
    match value.token()? {
        Token::Percentage(n) => Some(LengthPercentage::Percentage(n.value)),
        _ => length(value).map(LengthPercentage::Length),
    }
}

fn nonnegative(value: &Value) -> Option<LengthPercentage> {
    length_percentage(value).filter(|lp| !lp.is_negative())
}

fn lp_or_auto(value: &Value, allow_negative: bool) -> Option<LengthPercentageOrAuto> {
    if keyword(value, "auto") {
        return Some(LengthPercentageOrAuto::Auto);
    }
    let lp = length_percentage(value)?;
    (allow_negative || !lp.is_negative()).then_some(LengthPercentageOrAuto::LengthPercentage(lp))
}

fn border_width(value: &Value) -> Option<BorderWidth> {
    if let Some(name) = ident(value) {
        return match name.to_ascii_lowercase().as_str() {
            "thin" => Some(BorderWidth::Thin),
            "medium" => Some(BorderWidth::Medium),
            "thick" => Some(BorderWidth::Thick),
            _ => None,
        };
    }
    if math_function(value) {
        return Calc::parse_length(value).map(|c| BorderWidth::Calc(Box::new(c)));
    }
    length(value)
        .filter(|l| l.value >= 0.0)
        .map(BorderWidth::Length)
}

fn display(parts: &[&Value]) -> Option<Display> {
    let names: Vec<String> = parts
        .iter()
        .map(|v| ident(v).map(str::to_ascii_lowercase))
        .collect::<Option<_>>()?;
    match names.as_slice() {
        [one] => Display::from_name(one),
        [a, b] => {
            let (outer, inner) = if matches!(a.as_str(), "block" | "inline") {
                (a.as_str(), b.as_str())
            } else {
                (b.as_str(), a.as_str())
            };
            Some(match (outer, inner) {
                ("block", "flow") => Display::Block,
                ("inline", "flow") => Display::Inline,
                ("block", "flow-root") => Display::FlowRoot,
                ("inline", "flow-root") => Display::InlineBlock,
                ("block", "flex") => Display::Flex,
                ("inline", "flex") => Display::InlineFlex,
                ("block", "grid") => Display::Grid,
                ("inline", "grid") => Display::InlineGrid,
                ("block", "table") => Display::Table,
                ("inline", "table") => Display::InlineTable,
                ("block", "list-item") => Display::ListItem,
                _ => return None,
            })
        }
        _ => None,
    }
}

fn font_family(parts: &[&Value]) -> Option<Vec<FamilyName>> {
    let mut families = Vec::new();
    for entry in parts.split(|v| v.token() == Some(&Token::Comma)) {
        let family = match entry {
            [] => return None,
            [single] => match single.token()? {
                Token::String(name) => FamilyName::Named(name.clone()),
                Token::Ident(name) => match GenericFamily::from_name(name) {
                    Some(generic) => FamilyName::Generic(generic),
                    None if CssWideKeyword::from_name(name).is_some()
                        || name.eq_ignore_ascii_case("default") =>
                    {
                        return None;
                    }
                    None => FamilyName::Named(name.clone()),
                },
                _ => return None,
            },
            several => {
                let words: Vec<&str> = several.iter().map(|v| ident(v)).collect::<Option<_>>()?;
                FamilyName::Named(words.join(" "))
            }
        };
        families.push(family);
    }
    Some(families)
}

fn number(value: &Value) -> Option<f64> {
    match value.token()? {
        Token::Number(n) => Some(n.value),
        _ => None,
    }
}

/// A `<number>` in `min..=max`. A literal outside the range is invalid; a
/// math function's result is clamped to it instead (CSS Values 4 §10.9).
fn number_in(value: &Value, min: f64, max: f64) -> Option<f64> {
    if math_function(value) {
        return Calc::parse_number(value).map(|n| censor(n).clamp(min, max));
    }
    number(value).filter(|n| (min..=max).contains(n))
}

/// Parse a longhand value from its non-whitespace component values.
fn longhand(property: PropertyId, parts: &[&Value]) -> Option<PropertyValue> {
    use PropertyId as P;
    if property == P::Display {
        return display(parts).map(PropertyValue::Display);
    }
    if property == P::FontFamily {
        return font_family(parts).map(PropertyValue::FontFamily);
    }
    let [value] = parts else {
        return None;
    };
    Some(match property {
        P::Display | P::FontFamily => unreachable!("handled above"),
        P::Color
        | P::BackgroundColor
        | P::BorderTopColor
        | P::BorderRightColor
        | P::BorderBottomColor
        | P::BorderLeftColor => PropertyValue::Color(Color::parse(value)?),
        P::MarginTop | P::MarginRight | P::MarginBottom | P::MarginLeft => {
            PropertyValue::LengthPercentageOrAuto(lp_or_auto(value, true)?)
        }
        P::PaddingTop | P::PaddingRight | P::PaddingBottom | P::PaddingLeft => {
            PropertyValue::LengthPercentage(nonnegative(value)?)
        }
        P::BorderTopWidth | P::BorderRightWidth | P::BorderBottomWidth | P::BorderLeftWidth => {
            PropertyValue::BorderWidth(border_width(value)?)
        }
        P::BorderTopStyle | P::BorderRightStyle | P::BorderBottomStyle | P::BorderLeftStyle => {
            PropertyValue::BorderStyle(BorderStyle::from_name(ident(value)?)?)
        }
        P::Width | P::Height | P::MinWidth | P::MinHeight => {
            PropertyValue::LengthPercentageOrAuto(lp_or_auto(value, false)?)
        }
        P::MaxWidth | P::MaxHeight => PropertyValue::MaxSize(if keyword(value, "none") {
            MaxSize::None
        } else {
            MaxSize::LengthPercentage(nonnegative(value)?)
        }),
        P::BoxSizing => PropertyValue::BoxSizing(BoxSizing::from_name(ident(value)?)?),
        P::FontSize => PropertyValue::FontSize(match ident(value) {
            Some(name) => FontSize::Keyword(FontSizeKeyword::from_name(name)?),
            None => FontSize::LengthPercentage(nonnegative(value)?),
        }),
        P::FontWeight => PropertyValue::FontWeight(match ident(value) {
            Some(name) => match name.to_ascii_lowercase().as_str() {
                "normal" => FontWeight::Normal,
                "bold" => FontWeight::Bold,
                "bolder" => FontWeight::Bolder,
                "lighter" => FontWeight::Lighter,
                _ => return None,
            },
            None => FontWeight::Number(number_in(value, 1.0, 1000.0)?),
        }),
        P::FontStyle => PropertyValue::FontStyle(FontStyle::from_name(ident(value)?)?),
        P::LineHeight => PropertyValue::LineHeight(if keyword(value, "normal") {
            LineHeight::Normal
        } else if let Some(n) = number_in(value, 0.0, f64::MAX) {
            LineHeight::Number(n)
        } else if number(value).is_some() {
            return None;
        } else {
            LineHeight::LengthPercentage(nonnegative(value)?)
        }),
    })
}

/// Expand `top [right [bottom [left]]]` to four values.
fn four<T: Clone>(items: Vec<T>) -> Option<Vec<T>> {
    let mut items = items;
    match items.len() {
        1 => {
            let v = items[0].clone();
            items.extend([v.clone(), v.clone(), v]);
        }
        2 => {
            items.push(items[0].clone());
            items.push(items[1].clone());
        }
        3 => items.push(items[1].clone()),
        4 => {}
        _ => return None,
    }
    Some(items)
}

fn shorthand(name: &str, parts: &[&Value]) -> Option<Vec<PropertyValue>> {
    let each = |f: &dyn Fn(&Value) -> Option<PropertyValue>| -> Option<Vec<PropertyValue>> {
        four(parts.iter().map(|v| f(v)).collect::<Option<Vec<_>>>()?)
    };
    match name {
        "margin" => each(&|v| lp_or_auto(v, true).map(PropertyValue::LengthPercentageOrAuto)),
        "padding" => each(&|v| nonnegative(v).map(PropertyValue::LengthPercentage)),
        "border-width" => each(&|v| border_width(v).map(PropertyValue::BorderWidth)),
        "border-style" => {
            each(&|v| BorderStyle::from_name(ident(v)?).map(PropertyValue::BorderStyle))
        }
        "border-color" => each(&|v| Color::parse(v).map(PropertyValue::Color)),
        "border" | "border-top" | "border-right" | "border-bottom" | "border-left" => {
            let (mut width, mut style, mut color) = (None, None, None);
            for v in parts {
                if width.is_none() {
                    if let Some(w) = border_width(v) {
                        width = Some(w);
                        continue;
                    }
                }
                if style.is_none() {
                    if let Some(s) = ident(v).and_then(BorderStyle::from_name) {
                        style = Some(s);
                        continue;
                    }
                }
                if color.is_none() {
                    if let Some(c) = Color::parse(v) {
                        color = Some(c);
                        continue;
                    }
                }
                return None;
            }
            let width = PropertyValue::BorderWidth(width.unwrap_or(BorderWidth::Medium));
            let style = PropertyValue::BorderStyle(style.unwrap_or(BorderStyle::None));
            let color = PropertyValue::Color(color.unwrap_or(Color::CurrentColor));
            let sides = if name == "border" { 4 } else { 1 };
            let mut out = Vec::with_capacity(3 * sides);
            for value in [width, style, color] {
                out.extend(std::iter::repeat_n(value, sides));
            }
            Some(out)
        }
        _ => None,
    }
}

impl fmt::Display for LengthPercentageOrAuto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::LengthPercentage(lp) => write!(f, "{lp}"),
        }
    }
}

impl fmt::Display for PropertyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Display(d) => write!(f, "{d}"),
            Self::Color(c) => write!(f, "{c}"),
            Self::LengthPercentageOrAuto(v) => write!(f, "{v}"),
            Self::LengthPercentage(lp) => write!(f, "{lp}"),
            Self::MaxSize(MaxSize::None) => f.write_str("none"),
            Self::MaxSize(MaxSize::LengthPercentage(lp)) => write!(f, "{lp}"),
            Self::BorderWidth(BorderWidth::Thin) => f.write_str("thin"),
            Self::BorderWidth(BorderWidth::Medium) => f.write_str("medium"),
            Self::BorderWidth(BorderWidth::Thick) => f.write_str("thick"),
            Self::BorderWidth(BorderWidth::Length(l)) => write!(f, "{l}"),
            Self::BorderWidth(BorderWidth::Calc(c)) => write!(f, "{c}"),
            Self::BorderStyle(s) => write!(f, "{s}"),
            Self::BoxSizing(b) => write!(f, "{b}"),
            Self::FontSize(FontSize::Keyword(k)) => write!(f, "{k}"),
            Self::FontSize(FontSize::LengthPercentage(lp)) => write!(f, "{lp}"),
            Self::FontWeight(FontWeight::Normal) => f.write_str("normal"),
            Self::FontWeight(FontWeight::Bold) => f.write_str("bold"),
            Self::FontWeight(FontWeight::Bolder) => f.write_str("bolder"),
            Self::FontWeight(FontWeight::Lighter) => f.write_str("lighter"),
            Self::FontWeight(FontWeight::Number(n)) => write_number(f, *n),
            Self::FontStyle(s) => write!(f, "{s}"),
            Self::FontFamily(families) => {
                for (index, family) in families.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    match family {
                        FamilyName::Named(name) => serialize_string(f, name)?,
                        FamilyName::Generic(generic) => f.write_str(generic.name())?,
                    }
                }
                Ok(())
            }
            Self::LineHeight(LineHeight::Normal) => f.write_str("normal"),
            Self::LineHeight(LineHeight::Number(n)) => write_number(f, *n),
            Self::LineHeight(LineHeight::LengthPercentage(lp)) => write!(f, "{lp}"),
        }
    }
}

impl fmt::Display for DeclaredValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CssWide(keyword) => f.write_str(keyword.name()),
            Self::Value(value) => write!(f, "{value}"),
        }
    }
}

impl fmt::Display for PropertyDeclaration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.property.name(), self.value)?;
        if self.important {
            f.write_str(" !important")?;
        }
        Ok(())
    }
}
