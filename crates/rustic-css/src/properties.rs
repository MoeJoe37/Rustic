//! Typed property values for the CSS-03 and CSS-03c property sets.
//!
//! [`parse_declaration`] turns a syntax-level [`Declaration`] into typed
//! longhand declarations, expanding shorthands. Unknown properties and invalid
//! values are reported, never guessed: a declaration that fails here is
//! dropped by the cascade exactly as CSS Syntax requires.
//!
//! Supported longhands: `display`, `color`, `background-color`, `margin-*`,
//! `padding-*`, `border-*-width`, `border-*-style`, `border-*-color`,
//! `width`, `height`, `min-width`, `min-height`, `max-width`, `max-height`,
//! `box-sizing`, `font-size`, `font-weight`, `font-style`, `font-family`,
//! `line-height` (CSS-03) and, since CSS-03c, `font-variant-caps`,
//! `font-stretch`, `position`, `top`/`right`/`bottom`/`left`, `z-index`,
//! `float`, `clear`, `overflow-x`/`-y`, `visibility`, `opacity`,
//! `vertical-align`, `direction`, `text-align`, `text-indent`,
//! `text-transform`, `letter-spacing`, `word-spacing`, `white-space`,
//! `text-decoration-line`/`-style`/`-color` and the list-valued
//! `background-image`/`-position`/`-size`/`-repeat`/`-attachment`/`-origin`/
//! `-clip`. Shorthands: `margin`, `padding`, `border-width`, `border-style`,
//! `border-color`, `border-top`/`-right`/`-bottom`/`-left`, `border`, `inset`,
//! `overflow`, `text-decoration`, `font` and `background`. Every property also
//! accepts the CSS-wide keywords.
//!
//! Math functions (`calc()`, `min()`, `max()`, `clamp()`) are accepted in
//! every length, length-percentage and number slot (see [`crate::Calc`]).
//! `var()` substitution is a later task; such values are rejected for now.

use std::fmt;

use crate::background::{
    self, Attachment, BackgroundRepeat, BackgroundSize, Image, PositionValue, VisualBox, write_list,
};
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
            pub(crate) fn from_name(name: &str) -> Option<Self> {
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

pub(crate) use keywords;

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

keywords! {
    /// `position` values.
    Position {
        Static => "static",
        Relative => "relative",
        Absolute => "absolute",
        Fixed => "fixed",
        Sticky => "sticky",
    }
}

keywords! {
    /// `float` values.
    Float {
        None => "none",
        Left => "left",
        Right => "right",
        InlineStart => "inline-start",
        InlineEnd => "inline-end",
    }
}

keywords! {
    /// `clear` values.
    Clear {
        None => "none",
        Left => "left",
        Right => "right",
        Both => "both",
        InlineStart => "inline-start",
        InlineEnd => "inline-end",
    }
}

keywords! {
    /// `overflow-x` and `overflow-y` values.
    Overflow {
        Visible => "visible",
        Hidden => "hidden",
        Clip => "clip",
        Scroll => "scroll",
        Auto => "auto",
    }
}

keywords! {
    /// `visibility` values.
    Visibility {
        Visible => "visible",
        Hidden => "hidden",
        Collapse => "collapse",
    }
}

keywords! {
    /// `text-align` values.
    TextAlign {
        Start => "start",
        End => "end",
        Left => "left",
        Right => "right",
        Center => "center",
        Justify => "justify",
        MatchParent => "match-parent",
    }
}

keywords! {
    /// `direction` values.
    Direction {
        Ltr => "ltr",
        Rtl => "rtl",
    }
}

keywords! {
    /// `white-space` values (the CSS Text 3 keywords; the Level 4 longhands
    /// it expands to are not separate properties yet).
    WhiteSpace {
        Normal => "normal",
        Pre => "pre",
        Nowrap => "nowrap",
        PreWrap => "pre-wrap",
        BreakSpaces => "break-spaces",
        PreLine => "pre-line",
    }
}

keywords! {
    /// `text-decoration-style` values.
    TextDecorationStyle {
        Solid => "solid",
        Double => "double",
        Dotted => "dotted",
        Dashed => "dashed",
        Wavy => "wavy",
    }
}

keywords! {
    /// `font-variant-caps` values.
    FontVariantCaps {
        Normal => "normal",
        SmallCaps => "small-caps",
        AllSmallCaps => "all-small-caps",
        PetiteCaps => "petite-caps",
        AllPetiteCaps => "all-petite-caps",
        Unicase => "unicase",
        TitlingCaps => "titling-caps",
    }
}

keywords! {
    /// `font-stretch` (`<font-width>`) keywords.
    FontStretchKeyword {
        UltraCondensed => "ultra-condensed",
        ExtraCondensed => "extra-condensed",
        Condensed => "condensed",
        SemiCondensed => "semi-condensed",
        Normal => "normal",
        SemiExpanded => "semi-expanded",
        Expanded => "expanded",
        ExtraExpanded => "extra-expanded",
        UltraExpanded => "ultra-expanded",
    }
}

impl FontStretchKeyword {
    /// The keyword's width as a percentage (CSS Fonts 4 §2.4).
    pub fn percentage(self) -> f64 {
        match self {
            Self::UltraCondensed => 50.0,
            Self::ExtraCondensed => 62.5,
            Self::Condensed => 75.0,
            Self::SemiCondensed => 87.5,
            Self::Normal => 100.0,
            Self::SemiExpanded => 112.5,
            Self::Expanded => 125.0,
            Self::ExtraExpanded => 150.0,
            Self::UltraExpanded => 200.0,
        }
    }
}

keywords! {
    /// `vertical-align` keywords.
    VerticalAlignKeyword {
        Baseline => "baseline",
        Sub => "sub",
        Super => "super",
        TextTop => "text-top",
        TextBottom => "text-bottom",
        Middle => "middle",
        Top => "top",
        Bottom => "bottom",
    }
}

keywords! {
    /// The case transform of `text-transform`.
    TextCase {
        Capitalize => "capitalize",
        Uppercase => "uppercase",
        Lowercase => "lowercase",
    }
}

/// `z-index`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZIndex {
    /// `auto`.
    Auto,
    /// An integer (math functions round to the nearest integer; values
    /// saturate to the `i32` range).
    Integer(i32),
}

/// `vertical-align`.
#[derive(Clone, Debug, PartialEq)]
pub enum VerticalAlign {
    /// A keyword.
    Keyword(VerticalAlignKeyword),
    /// A length or percentage of the line height (computed to a length).
    LengthPercentage(LengthPercentage),
}

/// `letter-spacing` and `word-spacing`.
#[derive(Clone, Debug, PartialEq)]
pub enum Spacing {
    /// `normal`.
    Normal,
    /// A length or percentage (negative values allowed).
    LengthPercentage(LengthPercentage),
}

/// `text-transform`: `none` when every part is off.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TextTransform {
    /// `capitalize`, `uppercase` or `lowercase`.
    pub case: Option<TextCase>,
    /// `full-width`.
    pub full_width: bool,
    /// `full-size-kana`.
    pub full_size_kana: bool,
}

/// `text-decoration-line`: `none` when every line is off.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TextDecorationLine {
    /// `underline`.
    pub underline: bool,
    /// `overline`.
    pub overline: bool,
    /// `line-through`.
    pub line_through: bool,
    /// `blink`.
    pub blink: bool,
}

impl TextDecorationLine {
    /// Whether no line is drawn.
    pub fn is_none(self) -> bool {
        self == Self::default()
    }
}

/// `font-stretch`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FontStretch {
    /// A keyword (computed to its percentage).
    Keyword(FontStretchKeyword),
    /// A nonnegative percentage.
    Percentage(f64),
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
    /// `color`, `background-color`, `border-*-color`,
    /// `text-decoration-color`.
    Color(Color),
    /// `margin-*`, `width`, `height`, `min-width`, `min-height`, `top`,
    /// `right`, `bottom`, `left`.
    LengthPercentageOrAuto(LengthPercentageOrAuto),
    /// `padding-*` (nonnegative), `text-indent`.
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
    /// `position`.
    Position(Position),
    /// `z-index`.
    ZIndex(ZIndex),
    /// `float`.
    Float(Float),
    /// `clear`.
    Clear(Clear),
    /// `overflow-x`, `overflow-y`.
    Overflow(Overflow),
    /// `visibility`.
    Visibility(Visibility),
    /// `opacity`, as a number (a percentage is divided by 100). Out-of-range
    /// values are clamped to `0..=1` when computed.
    Opacity(f64),
    /// `vertical-align`.
    VerticalAlign(VerticalAlign),
    /// `text-align`.
    TextAlign(TextAlign),
    /// `direction`.
    Direction(Direction),
    /// `text-transform`.
    TextTransform(TextTransform),
    /// `letter-spacing`, `word-spacing`.
    Spacing(Spacing),
    /// `white-space`.
    WhiteSpace(WhiteSpace),
    /// `text-decoration-line`.
    TextDecorationLine(TextDecorationLine),
    /// `text-decoration-style`.
    TextDecorationStyle(TextDecorationStyle),
    /// `font-variant-caps`.
    FontVariantCaps(FontVariantCaps),
    /// `font-stretch`.
    FontStretch(FontStretch),
    /// `background-image`, one entry per layer.
    BackgroundImage(Vec<Image>),
    /// `background-position`, one entry per layer.
    BackgroundPosition(Vec<PositionValue>),
    /// `background-size`, one entry per layer.
    BackgroundSize(Vec<BackgroundSize>),
    /// `background-repeat`, one entry per layer.
    BackgroundRepeat(Vec<BackgroundRepeat>),
    /// `background-attachment`, one entry per layer.
    BackgroundAttachment(Vec<Attachment>),
    /// `background-origin`, `background-clip`, one entry per layer.
    BackgroundBox(Vec<VisualBox>),
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
    FontVariantCaps => "font-variant-caps", true;
    FontStretch => "font-stretch", true;
    Position => "position", false;
    Top => "top", false;
    Right => "right", false;
    Bottom => "bottom", false;
    Left => "left", false;
    ZIndex => "z-index", false;
    Float => "float", false;
    Clear => "clear", false;
    OverflowX => "overflow-x", false;
    OverflowY => "overflow-y", false;
    Visibility => "visibility", true;
    Opacity => "opacity", false;
    VerticalAlign => "vertical-align", false;
    Direction => "direction", true;
    TextAlign => "text-align", true;
    TextIndent => "text-indent", true;
    TextTransform => "text-transform", true;
    LetterSpacing => "letter-spacing", true;
    WordSpacing => "word-spacing", true;
    WhiteSpace => "white-space", true;
    TextDecorationLine => "text-decoration-line", false;
    TextDecorationStyle => "text-decoration-style", false;
    TextDecorationColor => "text-decoration-color", false;
    BackgroundImage => "background-image", false;
    BackgroundPosition => "background-position", false;
    BackgroundSize => "background-size", false;
    BackgroundRepeat => "background-repeat", false;
    BackgroundAttachment => "background-attachment", false;
    BackgroundOrigin => "background-origin", false;
    BackgroundClip => "background-clip", false;
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
            P::FontVariantCaps => PropertyValue::FontVariantCaps(FontVariantCaps::Normal),
            P::FontStretch => {
                PropertyValue::FontStretch(FontStretch::Keyword(FontStretchKeyword::Normal))
            }
            P::Position => PropertyValue::Position(Position::Static),
            P::Top | P::Right | P::Bottom | P::Left => {
                PropertyValue::LengthPercentageOrAuto(LengthPercentageOrAuto::Auto)
            }
            P::ZIndex => PropertyValue::ZIndex(ZIndex::Auto),
            P::Float => PropertyValue::Float(Float::None),
            P::Clear => PropertyValue::Clear(Clear::None),
            P::OverflowX | P::OverflowY => PropertyValue::Overflow(Overflow::Visible),
            P::Visibility => PropertyValue::Visibility(Visibility::Visible),
            P::Opacity => PropertyValue::Opacity(1.0),
            P::VerticalAlign => {
                PropertyValue::VerticalAlign(VerticalAlign::Keyword(VerticalAlignKeyword::Baseline))
            }
            P::Direction => PropertyValue::Direction(Direction::Ltr),
            P::TextAlign => PropertyValue::TextAlign(TextAlign::Start),
            P::TextIndent => PropertyValue::LengthPercentage(zero),
            P::TextTransform => PropertyValue::TextTransform(TextTransform::default()),
            P::LetterSpacing | P::WordSpacing => PropertyValue::Spacing(Spacing::Normal),
            P::WhiteSpace => PropertyValue::WhiteSpace(WhiteSpace::Normal),
            P::TextDecorationLine => {
                PropertyValue::TextDecorationLine(TextDecorationLine::default())
            }
            P::TextDecorationStyle => {
                PropertyValue::TextDecorationStyle(TextDecorationStyle::Solid)
            }
            P::TextDecorationColor => PropertyValue::Color(Color::CurrentColor),
            P::BackgroundImage => PropertyValue::BackgroundImage(vec![Image::None]),
            P::BackgroundPosition => {
                PropertyValue::BackgroundPosition(vec![PositionValue::top_left()])
            }
            P::BackgroundSize => PropertyValue::BackgroundSize(vec![BackgroundSize::AUTO]),
            P::BackgroundRepeat => PropertyValue::BackgroundRepeat(vec![BackgroundRepeat::REPEAT]),
            P::BackgroundAttachment => {
                PropertyValue::BackgroundAttachment(vec![Attachment::Scroll])
            }
            P::BackgroundOrigin => PropertyValue::BackgroundBox(vec![VisualBox::PaddingBox]),
            P::BackgroundClip => PropertyValue::BackgroundBox(vec![VisualBox::BorderBox]),
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
    const INSET: &[P] = &[P::Top, P::Right, P::Bottom, P::Left];
    const OVERFLOW: &[P] = &[P::OverflowX, P::OverflowY];
    const TEXT_DECORATION: &[P] = &[
        P::TextDecorationLine,
        P::TextDecorationStyle,
        P::TextDecorationColor,
    ];
    const FONT: &[P] = &[
        P::FontStyle,
        P::FontVariantCaps,
        P::FontWeight,
        P::FontStretch,
        P::FontSize,
        P::LineHeight,
        P::FontFamily,
    ];
    const BACKGROUND: &[P] = &[
        P::BackgroundColor,
        P::BackgroundImage,
        P::BackgroundPosition,
        P::BackgroundSize,
        P::BackgroundRepeat,
        P::BackgroundAttachment,
        P::BackgroundOrigin,
        P::BackgroundClip,
    ];
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
        "inset" => INSET,
        "overflow" => OVERFLOW,
        "text-decoration" => TEXT_DECORATION,
        "font" => FONT,
        "background" => BACKGROUND,
        _ => {
            let id = PropertyId::from_name(name)?;
            let index = PropertyId::ALL.iter().position(|&p| p == id)?;
            &PropertyId::ALL[index..=index]
        }
    })
}

pub(crate) fn ident(value: &Value) -> Option<&str> {
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
pub(crate) fn length_percentage(value: &Value) -> Option<LengthPercentage> {
    if math_function(value) {
        return Calc::parse_length_percentage(value).map(|c| LengthPercentage::Calc(Box::new(c)));
    }
    match value.token()? {
        Token::Percentage(n) => Some(LengthPercentage::Percentage(n.value)),
        _ => length(value).map(LengthPercentage::Length),
    }
}

pub(crate) fn nonnegative(value: &Value) -> Option<LengthPercentage> {
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

/// The only value of a one-value list entry.
fn single<'a>(parts: &[&'a Value]) -> Option<&'a Value> {
    match parts {
        [one] => Some(*one),
        _ => None,
    }
}

/// Saturate a finite integer-valued number to `i32`.
fn saturate_i32(n: f64) -> i32 {
    // The clamp bounds the cast: every value in range converts exactly.
    n.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

fn z_index(value: &Value) -> Option<ZIndex> {
    if keyword(value, "auto") {
        return Some(ZIndex::Auto);
    }
    if math_function(value) {
        // Math functions in an `<integer>` slot round to the nearest
        // integer, halfway cases toward +infinity (CSS Values 4 §10.9).
        let n = censor(Calc::parse_number(value)?);
        return Some(ZIndex::Integer(saturate_i32((n + 0.5).floor())));
    }
    match value.token()? {
        Token::Number(n) if n.integer => Some(ZIndex::Integer(saturate_i32(n.value))),
        _ => None,
    }
}

fn opacity(value: &Value) -> Option<f64> {
    if math_function(value) {
        return Calc::parse_number(value);
    }
    match value.token()? {
        Token::Number(n) => Some(n.value),
        Token::Percentage(n) => Some(n.value / 100.0),
        _ => None,
    }
}

fn spacing(value: &Value) -> Option<Spacing> {
    if keyword(value, "normal") {
        return Some(Spacing::Normal);
    }
    length_percentage(value).map(Spacing::LengthPercentage)
}

fn font_stretch(value: &Value) -> Option<FontStretch> {
    if let Some(name) = ident(value) {
        return FontStretchKeyword::from_name(name).map(FontStretch::Keyword);
    }
    match value.token()? {
        Token::Percentage(n) if n.value >= 0.0 => Some(FontStretch::Percentage(n.value)),
        _ => None,
    }
}

/// Lowercase identifiers, or `None` if any part is not an identifier.
fn idents(parts: &[&Value]) -> Option<Vec<String>> {
    parts
        .iter()
        .map(|v| ident(v).map(str::to_ascii_lowercase))
        .collect()
}

/// Set `flag`, failing if it was already set.
fn once(flag: &mut bool) -> Option<()> {
    (!std::mem::replace(flag, true)).then_some(())
}

fn text_transform(parts: &[&Value]) -> Option<TextTransform> {
    let names = idents(parts)?;
    if names == ["none"] {
        return Some(TextTransform::default());
    }
    let mut t = TextTransform::default();
    for name in &names {
        match name.as_str() {
            "full-width" => once(&mut t.full_width)?,
            "full-size-kana" => once(&mut t.full_size_kana)?,
            other => {
                let case = TextCase::from_name(other)?;
                if t.case.replace(case).is_some() {
                    return None;
                }
            }
        }
    }
    (!names.is_empty()).then_some(t)
}

/// Add one `text-decoration-line` keyword; `None` if it is not one or repeats.
fn add_line(line: &mut TextDecorationLine, name: &str) -> Option<()> {
    match name {
        "underline" => once(&mut line.underline),
        "overline" => once(&mut line.overline),
        "line-through" => once(&mut line.line_through),
        "blink" => once(&mut line.blink),
        _ => None,
    }
}

fn decoration_line(parts: &[&Value]) -> Option<TextDecorationLine> {
    let names = idents(parts)?;
    if names == ["none"] {
        return Some(TextDecorationLine::default());
    }
    let mut line = TextDecorationLine::default();
    for name in &names {
        add_line(&mut line, name)?;
    }
    (!names.is_empty()).then_some(line)
}

/// `text-decoration`: `<line> || <style> || <color>`, where the line keywords
/// must be adjacent.
fn text_decoration(parts: &[&Value]) -> Option<Vec<PropertyValue>> {
    #[derive(PartialEq)]
    enum State {
        Unseen,
        Open,
        Closed,
    }
    let mut line = TextDecorationLine::default();
    let mut state = State::Unseen;
    let (mut style, mut color) = (None, None);
    for v in parts {
        let name = ident(v).map(str::to_ascii_lowercase);
        if let Some(name) = &name {
            if name == "none" && state == State::Unseen {
                state = State::Closed;
                continue;
            }
            let mut probe = line;
            if add_line(&mut probe, name).is_some() {
                if state == State::Closed {
                    return None;
                }
                line = probe;
                state = State::Open;
                continue;
            }
        }
        if state == State::Open {
            state = State::Closed;
        }
        if style.is_none() {
            if let Some(s) = name.as_deref().and_then(TextDecorationStyle::from_name) {
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
    Some(vec![
        PropertyValue::TextDecorationLine(line),
        PropertyValue::TextDecorationStyle(style.unwrap_or(TextDecorationStyle::Solid)),
        PropertyValue::Color(color.unwrap_or(Color::CurrentColor)),
    ])
}

/// The `font` shorthand (CSS Fonts 4 §2.8): up to four of style,
/// `small-caps`, weight and width keywords (or `normal`) in any order, then
/// the size, an optional `/ line-height` and the family list. System font
/// keywords (`caption`, `menu`, ...) are not supported yet.
fn font(parts: &[&Value]) -> Option<Vec<PropertyValue>> {
    let (mut style, mut caps, mut weight, mut stretch) = (None, None, None, None);
    let mut i = 0;
    while i < parts.len() && i < 4 {
        let v = parts[i];
        if let Some(name) = ident(v) {
            let lower = name.to_ascii_lowercase();
            if lower == "normal" {
            } else if let Some(s) = FontStyle::from_name(&lower).filter(|_| style.is_none()) {
                style = Some(s);
            } else if lower == "small-caps" && caps.is_none() {
                caps = Some(FontVariantCaps::SmallCaps);
            } else if let Some(w) = match lower.as_str() {
                "bold" => Some(FontWeight::Bold),
                "bolder" => Some(FontWeight::Bolder),
                "lighter" => Some(FontWeight::Lighter),
                _ => None,
            }
            .filter(|_| weight.is_none())
            {
                weight = Some(w);
            } else if let Some(k) =
                FontStretchKeyword::from_name(&lower).filter(|_| stretch.is_none())
            {
                stretch = Some(FontStretch::Keyword(k));
            } else {
                break;
            }
        } else if let Some(w) = number_in(v, 1.0, 1000.0).filter(|_| weight.is_none()) {
            weight = Some(FontWeight::Number(w));
        } else if number(v).is_some_and(|n| n != 0.0) {
            // A number that is not a valid weight (and not a unitless zero
            // size) makes the declaration invalid.
            return None;
        } else {
            break;
        }
        i += 1;
    }
    let size = longhand(PropertyId::FontSize, &[parts.get(i)?])?;
    i += 1;
    let mut line_height = PropertyValue::LineHeight(LineHeight::Normal);
    if parts.get(i).and_then(|v| v.token()) == Some(&Token::Delim('/')) {
        line_height = longhand(PropertyId::LineHeight, &[parts.get(i + 1)?])?;
        i += 2;
    }
    let family = font_family(&parts[i..])?;
    Some(vec![
        PropertyValue::FontStyle(style.unwrap_or(FontStyle::Normal)),
        PropertyValue::FontVariantCaps(caps.unwrap_or(FontVariantCaps::Normal)),
        PropertyValue::FontWeight(weight.unwrap_or(FontWeight::Normal)),
        PropertyValue::FontStretch(
            stretch.unwrap_or(FontStretch::Keyword(FontStretchKeyword::Normal)),
        ),
        size,
        line_height,
        PropertyValue::FontFamily(family),
    ])
}

/// Parse a longhand value from its non-whitespace component values.
fn longhand(property: PropertyId, parts: &[&Value]) -> Option<PropertyValue> {
    use PropertyId as P;
    if property == P::Display {
        return display(parts).map(PropertyValue::Display);
    }
    match property {
        P::FontFamily => return font_family(parts).map(PropertyValue::FontFamily),
        P::TextTransform => return text_transform(parts).map(PropertyValue::TextTransform),
        P::TextDecorationLine => {
            return decoration_line(parts).map(PropertyValue::TextDecorationLine);
        }
        P::BackgroundImage => {
            return background::list(parts, |p| background::image(single(p)?))
                .map(PropertyValue::BackgroundImage);
        }
        P::BackgroundPosition => {
            return background::list(parts, background::position)
                .map(PropertyValue::BackgroundPosition);
        }
        P::BackgroundSize => {
            return background::list(parts, background::size).map(PropertyValue::BackgroundSize);
        }
        P::BackgroundRepeat => {
            return background::list(parts, background::repeat)
                .map(PropertyValue::BackgroundRepeat);
        }
        P::BackgroundAttachment => {
            return background::list(parts, |p| Attachment::from_name(ident(single(p)?)?))
                .map(PropertyValue::BackgroundAttachment);
        }
        P::BackgroundOrigin | P::BackgroundClip => {
            return background::list(parts, |p| VisualBox::from_name(ident(single(p)?)?))
                .map(PropertyValue::BackgroundBox);
        }
        _ => {}
    }
    let [value] = parts else {
        return None;
    };
    let name = || ident(value);
    Some(match property {
        P::Display
        | P::FontFamily
        | P::TextTransform
        | P::TextDecorationLine
        | P::BackgroundImage
        | P::BackgroundPosition
        | P::BackgroundSize
        | P::BackgroundRepeat
        | P::BackgroundAttachment
        | P::BackgroundOrigin
        | P::BackgroundClip => unreachable!("handled above"),
        P::FontVariantCaps => PropertyValue::FontVariantCaps(FontVariantCaps::from_name(name()?)?),
        P::FontStretch => PropertyValue::FontStretch(font_stretch(value)?),
        P::Position => PropertyValue::Position(Position::from_name(name()?)?),
        P::Top | P::Right | P::Bottom | P::Left => {
            PropertyValue::LengthPercentageOrAuto(lp_or_auto(value, true)?)
        }
        P::ZIndex => PropertyValue::ZIndex(z_index(value)?),
        P::Float => PropertyValue::Float(Float::from_name(name()?)?),
        P::Clear => PropertyValue::Clear(Clear::from_name(name()?)?),
        P::OverflowX | P::OverflowY => PropertyValue::Overflow(Overflow::from_name(name()?)?),
        P::Visibility => PropertyValue::Visibility(Visibility::from_name(name()?)?),
        P::Opacity => PropertyValue::Opacity(opacity(value)?),
        P::VerticalAlign => PropertyValue::VerticalAlign(match name() {
            Some(n) => VerticalAlign::Keyword(VerticalAlignKeyword::from_name(n)?),
            None => VerticalAlign::LengthPercentage(length_percentage(value)?),
        }),
        P::Direction => PropertyValue::Direction(Direction::from_name(name()?)?),
        P::TextAlign => PropertyValue::TextAlign(TextAlign::from_name(name()?)?),
        P::TextIndent => PropertyValue::LengthPercentage(length_percentage(value)?),
        P::LetterSpacing | P::WordSpacing => PropertyValue::Spacing(spacing(value)?),
        P::WhiteSpace => PropertyValue::WhiteSpace(WhiteSpace::from_name(name()?)?),
        P::TextDecorationStyle => {
            PropertyValue::TextDecorationStyle(TextDecorationStyle::from_name(name()?)?)
        }
        P::TextDecorationColor => PropertyValue::Color(Color::parse(value)?),
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
        "inset" => each(&|v| lp_or_auto(v, true).map(PropertyValue::LengthPercentageOrAuto)),
        "overflow" => {
            let values: Vec<Overflow> = parts
                .iter()
                .map(|v| Overflow::from_name(ident(v)?))
                .collect::<Option<_>>()?;
            let (x, y) = match values.as_slice() {
                [both] => (*both, *both),
                [x, y] => (*x, *y),
                _ => return None,
            };
            Some(vec![PropertyValue::Overflow(x), PropertyValue::Overflow(y)])
        }
        "text-decoration" => text_decoration(parts),
        "font" => font(parts),
        "background" => background::shorthand(parts),
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
            Self::Position(p) => write!(f, "{p}"),
            Self::ZIndex(ZIndex::Auto) => f.write_str("auto"),
            Self::ZIndex(ZIndex::Integer(n)) => write!(f, "{n}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::Clear(v) => write!(f, "{v}"),
            Self::Overflow(v) => write!(f, "{v}"),
            Self::Visibility(v) => write!(f, "{v}"),
            Self::Opacity(n) => write_number(f, *n),
            Self::VerticalAlign(VerticalAlign::Keyword(k)) => write!(f, "{k}"),
            Self::VerticalAlign(VerticalAlign::LengthPercentage(lp)) => write!(f, "{lp}"),
            Self::TextAlign(v) => write!(f, "{v}"),
            Self::Direction(v) => write!(f, "{v}"),
            Self::TextTransform(t) => {
                let mut words: Vec<&str> = Vec::new();
                if let Some(case) = t.case {
                    words.push(case.name());
                }
                if t.full_width {
                    words.push("full-width");
                }
                if t.full_size_kana {
                    words.push("full-size-kana");
                }
                if words.is_empty() {
                    words.push("none");
                }
                f.write_str(&words.join(" "))
            }
            Self::Spacing(Spacing::Normal) => f.write_str("normal"),
            Self::Spacing(Spacing::LengthPercentage(lp)) => write!(f, "{lp}"),
            Self::WhiteSpace(v) => write!(f, "{v}"),
            Self::TextDecorationLine(line) => {
                let flags = [
                    (line.underline, "underline"),
                    (line.overline, "overline"),
                    (line.line_through, "line-through"),
                    (line.blink, "blink"),
                ];
                let words: Vec<&str> = flags
                    .iter()
                    .filter(|(on, _)| *on)
                    .map(|(_, w)| *w)
                    .collect();
                f.write_str(if words.is_empty() { "none" } else { "" })?;
                f.write_str(&words.join(" "))
            }
            Self::TextDecorationStyle(v) => write!(f, "{v}"),
            Self::FontVariantCaps(v) => write!(f, "{v}"),
            Self::FontStretch(FontStretch::Keyword(k)) => write!(f, "{k}"),
            Self::FontStretch(FontStretch::Percentage(p)) => {
                write_number(f, *p)?;
                f.write_str("%")
            }
            Self::BackgroundImage(v) => write_list(f, v),
            Self::BackgroundPosition(v) => write_list(f, v),
            Self::BackgroundSize(v) => write_list(f, v),
            Self::BackgroundRepeat(v) => write_list(f, v),
            Self::BackgroundAttachment(v) => write_list(f, v),
            Self::BackgroundBox(v) => write_list(f, v),
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
