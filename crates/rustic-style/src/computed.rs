//! Computed values (CSS Cascade Level 5 §4.4) for the supported longhands.
//!
//! Computing makes values absolute where the specification says so: lengths
//! become pixels (font-relative units against the font size, viewport units
//! against [`ComputeContext`]), font-size keywords and percentages resolve
//! against the parent, `bolder`/`lighter` resolve against the parent weight,
//! `currentcolor` resolves to the element's `color`, a border width is zero
//! when its style is `none` or `hidden`, and the root element's `display` is
//! blockified. Percentages of a containing block (widths, margins, padding)
//! stay percentages: they resolve during layout.
//!
//! `ex`, `ch`, `cap` and `ic` (and their root forms) use the metrics of the
//! element's first available font from [`ComputeContext::fonts`] (REN-03a);
//! without fonts they use the fallbacks in [`EmRatios::FALLBACK`] (`0.5em`
//! for `ex`/`ch`, `0.7em` for `cap`, `1em` for `ic`). `lh` uses the
//! element's computed line height with `normal` taken as `1.2`.

use std::fmt;
use std::sync::Arc;

use rustic_css::{
    BackgroundSize, BorderStyle, BorderWidth, Color, Direction, Display, FlexBasis, Float,
    FontSize, FontSizeKeyword, FontStretch, FontStyle, FontUnitRatios, FontWeight, Gap, Length,
    LengthPercentage, LengthPercentageOrAuto, LengthUnit, LineHeight, MaxSize, Overflow, Position,
    PositionComponent, PositionValue, PropertyId, PropertyValue, Size, Spacing, TextAlign,
    VerticalAlign, WritingMode,
};

use crate::fonts::{FontMetricsSource, FontQuery, INITIAL_FONT, unit_ratios};
use crate::{CustomProperties, EmRatios, SpecifiedStyle};

/// The environment computed values depend on.
#[derive(Clone)]
pub struct ComputeContext {
    /// Viewport width in CSS pixels (for `vw`, `vi`, `vmin`, `vmax`, ...).
    pub viewport_width: f64,
    /// Viewport height in CSS pixels (for `vh`, `vb`, `vmin`, `vmax`, ...).
    pub viewport_height: f64,
    /// Loaded fonts for `ex`, `ch`, `cap` and `ic`; `None` uses the CSS
    /// fallback ratios. Two contexts are equal only when they share the
    /// same source (`Arc` identity), since a source cannot be compared.
    pub fonts: Option<Arc<dyn FontMetricsSource>>,
}

impl fmt::Debug for ComputeContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComputeContext")
            .field("viewport_width", &self.viewport_width)
            .field("viewport_height", &self.viewport_height)
            .field("fonts", &self.fonts.is_some())
            .finish()
    }
}

impl PartialEq for ComputeContext {
    fn eq(&self, other: &Self) -> bool {
        self.viewport_width == other.viewport_width
            && self.viewport_height == other.viewport_height
            && match (&self.fonts, &other.fonts) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
}

impl Default for ComputeContext {
    /// A 1280×720 viewport, matching `rustic-engine`'s default, without fonts.
    fn default() -> Self {
        Self {
            viewport_width: 1280.0,
            viewport_height: 720.0,
            fonts: None,
        }
    }
}

impl ComputeContext {
    /// The default viewport with `fonts` as the metrics source.
    pub fn with_fonts(fonts: Arc<dyn FontMetricsSource>) -> Self {
        Self {
            fonts: Some(fonts),
            ..Self::default()
        }
    }

    /// The initial font's unit ratios, for [`MediaEnvironment::initial_font`]
    /// so media queries resolve `ex`, `ch`, `cap` and `ic` against the same
    /// fonts as computed styles.
    ///
    /// [`MediaEnvironment::initial_font`]: rustic_css::MediaEnvironment::initial_font
    pub fn initial_font_ratios(&self) -> FontUnitRatios {
        unit_ratios(self.em_ratios(&INITIAL_FONT))
    }

    fn em_ratios(&self, font: &FontQuery<'_>) -> EmRatios {
        self.fonts
            .as_ref()
            .and_then(|source| source.em_ratios(font))
            .unwrap_or(EmRatios::FALLBACK)
    }
}

/// The computed value of every supported longhand for one element.
///
/// Values use the [`PropertyValue`] types, normalized: lengths are in `px`,
/// font sizes are pixel lengths, font weights are numbers and colors are
/// never `currentcolor`.
#[derive(Clone, Debug, PartialEq)]
pub struct ComputedStyle {
    values: Vec<PropertyValue>,
    pub(crate) custom: CustomProperties,
    font_ratios: EmRatios,
    /// Insets and sizes given by `anchor()` or `anchor-size()`, with their
    /// fallbacks computed; [`Self::get`] returns the fallback value.
    anchored: Vec<(PropertyId, rustic_css::AnchorFunction)>,
}

fn index(property: PropertyId) -> usize {
    PropertyId::ALL
        .iter()
        .position(|&p| p == property)
        .expect("every PropertyId is listed in ALL")
}

/// Font sizes for the absolute-size keywords (CSS Fonts Level 4 §2.5), with
/// `medium` at 16px.
fn keyword_px(keyword: FontSizeKeyword, parent_px: f64) -> f64 {
    match keyword {
        FontSizeKeyword::XxSmall => 9.0,
        FontSizeKeyword::XSmall => 10.0,
        FontSizeKeyword::Small => 13.0,
        FontSizeKeyword::Medium => 16.0,
        FontSizeKeyword::Large => 18.0,
        FontSizeKeyword::XLarge => 24.0,
        FontSizeKeyword::XxLarge => 32.0,
        FontSizeKeyword::XxxLarge => 48.0,
        FontSizeKeyword::Larger => parent_px * 1.2,
        FontSizeKeyword::Smaller => parent_px / 1.2,
    }
}

/// `bolder` and `lighter` (CSS Fonts Level 4 §2.2, relative weights table).
fn relative_weight(weight: FontWeight, parent: f64) -> f64 {
    match weight {
        FontWeight::Normal => 400.0,
        FontWeight::Bold => 700.0,
        FontWeight::Number(n) => n,
        FontWeight::Bolder if parent < 350.0 => 400.0,
        FontWeight::Bolder if parent < 550.0 => 700.0,
        FontWeight::Bolder if parent < 900.0 => 900.0,
        FontWeight::Bolder => parent,
        FontWeight::Lighter if parent < 100.0 => parent,
        FontWeight::Lighter if parent < 550.0 => 100.0,
        FontWeight::Lighter if parent < 750.0 => 400.0,
        FontWeight::Lighter => 700.0,
    }
}

/// What relative lengths resolve against for one element.
struct Basis {
    font_px: f64,
    root_font_px: f64,
    font: EmRatios,
    root_font: EmRatios,
    line_px: f64,
    root_line_px: f64,
    viewport: (f64, f64),
}

impl Basis {
    fn px(&self, length: Length) -> f64 {
        if let Some(px) = length.to_px() {
            return px;
        }
        let v = length.value;
        let (w, h) = self.viewport;
        let (font, root) = (&self.font, &self.root_font);
        let result = match length.unit {
            LengthUnit::Em => v * self.font_px,
            LengthUnit::Rem => v * self.root_font_px,
            LengthUnit::Ex => v * self.font_px * font.x_height,
            LengthUnit::Rex => v * self.root_font_px * root.x_height,
            LengthUnit::Ch => v * self.font_px * font.zero_advance,
            LengthUnit::Rch => v * self.root_font_px * root.zero_advance,
            LengthUnit::Cap => v * self.font_px * font.cap_height,
            LengthUnit::Rcap => v * self.root_font_px * root.cap_height,
            LengthUnit::Ic => v * self.font_px * font.ideograph_advance,
            LengthUnit::Ric => v * self.root_font_px * root.ideograph_advance,
            LengthUnit::Lh => v * self.line_px,
            LengthUnit::Rlh => v * self.root_line_px,
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
            // Absolute units returned above.
            _ => v,
        };
        // Keep values finite even for saturated inputs.
        if result.is_finite() {
            result
        } else {
            f64::MAX.copysign(result)
        }
    }

    fn length(&self, length: Length) -> Length {
        Length::px(self.px(length))
    }

    /// A computed `<length-percentage>`: lengths become pixels, percentages
    /// stay, and a math function is simplified with pixel lengths. When
    /// `nonnegative`, a result that is a single negative value clamps to
    /// zero; a mix of pixels and percentages is clamped once it resolves.
    fn lp(&self, value: &LengthPercentage, nonnegative: bool) -> LengthPercentage {
        let computed = match value {
            LengthPercentage::Length(l) => LengthPercentage::Length(self.length(*l)),
            LengthPercentage::Percentage(p) => LengthPercentage::Percentage(*p),
            LengthPercentage::Calc(calc) => calc.compute(|l| self.px(l)),
        };
        match computed {
            LengthPercentage::Length(l) if nonnegative && l.value < 0.0 => {
                LengthPercentage::Length(Length::px(0.0))
            }
            LengthPercentage::Percentage(p) if nonnegative && p < 0.0 => {
                LengthPercentage::Percentage(0.0)
            }
            other => other,
        }
    }
}

/// Compute the lengths inside a sizing value.
fn compute_size(size: &Size, lp: &dyn Fn(&LengthPercentage) -> LengthPercentage) -> Size {
    match size {
        Size::LengthPercentage(v) => Size::LengthPercentage(lp(v)),
        Size::FitContent(Some(v)) => Size::FitContent(Some(lp(v))),
        other => other.clone(),
    }
}

/// Longhands whose lengths may be negative; every other length clamps at 0.
const NEGATIVE_LENGTHS: &[PropertyId] = &[
    PropertyId::MarginTop,
    PropertyId::MarginRight,
    PropertyId::MarginBottom,
    PropertyId::MarginLeft,
    PropertyId::Top,
    PropertyId::Right,
    PropertyId::Bottom,
    PropertyId::Left,
    PropertyId::TextIndent,
    PropertyId::OutlineOffset,
];

/// `overflow-x`/`-y` (CSS Overflow 3 §3.1): if one axis is `visible` or
/// `clip` and the other is not, `visible` computes to `auto` and `clip` to
/// `hidden`.
fn overflow_pair(x: Overflow, y: Overflow) -> (Overflow, Overflow) {
    let passive = |o: Overflow| matches!(o, Overflow::Visible | Overflow::Clip);
    let fix = |o: Overflow| match o {
        Overflow::Visible => Overflow::Auto,
        Overflow::Clip => Overflow::Hidden,
        other => other,
    };
    if passive(x) != passive(y) {
        (fix(x), fix(y))
    } else {
        (x, y)
    }
}

/// `start`/`end` against a direction, for `match-parent`.
fn absolute_align(align: TextAlign, direction: Direction) -> TextAlign {
    match (align, direction) {
        (TextAlign::Start, Direction::Ltr) | (TextAlign::End, Direction::Rtl) => TextAlign::Left,
        (TextAlign::Start, Direction::Rtl) | (TextAlign::End, Direction::Ltr) => TextAlign::Right,
        (other, _) => other,
    }
}

fn line_px(line_height: &PropertyValue, font_px: f64) -> f64 {
    match line_height {
        PropertyValue::LineHeight(LineHeight::Number(n)) => n * font_px,
        PropertyValue::LineHeight(LineHeight::LengthPercentage(LengthPercentage::Length(l))) => {
            l.to_px().unwrap_or(font_px * 1.2)
        }
        _ => font_px * 1.2,
    }
}

fn blockify(display: Display) -> Display {
    match display {
        Display::Inline
        | Display::InlineBlock
        | Display::Contents
        | Display::TableRowGroup
        | Display::TableHeaderGroup
        | Display::TableFooterGroup
        | Display::TableRow
        | Display::TableCell
        | Display::TableColumnGroup
        | Display::TableColumn
        | Display::TableCaption => Display::Block,
        Display::InlineFlex => Display::Flex,
        Display::InlineGrid => Display::Grid,
        Display::InlineTable => Display::Table,
        other => other,
    }
}

fn font_px_of(style: &ComputedStyle) -> f64 {
    match style.get(PropertyId::FontSize) {
        PropertyValue::FontSize(FontSize::LengthPercentage(LengthPercentage::Length(l))) => {
            l.to_px().unwrap_or(16.0)
        }
        _ => 16.0,
    }
}

impl ComputedStyle {
    /// Style for an anonymous flow box. Inherited properties come from the
    /// enclosing non-anonymous box; other properties take their initial value,
    /// with the supplied formatting display. Registered custom properties obey
    /// their inheritance flag. This does not run selectors or the cascade.
    pub fn anonymous_flow(&self, display: Display) -> Self {
        let mut style = Self::compute(
            &SpecifiedStyle::anonymous(self, display),
            Some(self),
            None,
            false,
            &ComputeContext::default(),
        );
        // Every font property inherits, so the first available font does too.
        style.font_ratios = self.font_ratios;
        style
    }

    /// The computed initial values (the style a root inherits from).
    pub fn initial() -> Self {
        compute_values(
            &SpecifiedStyle::initial(),
            None,
            None,
            false,
            &ComputeContext::default(),
        )
    }

    /// The computed value of `property`.
    pub fn get(&self, property: PropertyId) -> &PropertyValue {
        &self.values[index(property)]
    }

    /// The `anchor()` or `anchor-size()` function an inset or size was
    /// given by. Layout resolves it for absolutely positioned boxes;
    /// [`Self::get`] holds the value used when it does not resolve.
    pub fn anchor_function(&self, property: PropertyId) -> Option<&rustic_css::AnchorFunction> {
        self.anchored
            .iter()
            .find(|(p, _)| *p == property)
            .map(|(_, f)| f)
    }

    /// Whether any inset or size uses an anchor function.
    pub fn has_anchor_functions(&self) -> bool {
        !self.anchored.is_empty()
    }

    /// The element's `writing-mode`.
    pub fn writing_mode(&self) -> WritingMode {
        match self.get(PropertyId::WritingMode) {
            PropertyValue::WritingMode(mode) => *mode,
            _ => WritingMode::HorizontalTb,
        }
    }

    /// The element's `direction`.
    pub fn direction(&self) -> Direction {
        match self.get(PropertyId::Direction) {
            PropertyValue::Direction(direction) => *direction,
            _ => Direction::Ltr,
        }
    }

    /// The physical longhand `property` sets on this element: itself, or
    /// a logical longhand's mapping for the element's writing mode.
    pub fn physical(&self, property: PropertyId) -> PropertyId {
        property.to_physical(self.writing_mode(), self.direction())
    }

    /// Replace the computed value of `property` (layout uses this to
    /// substitute resolved anchor positions into a copy of the style).
    pub fn set(&mut self, property: PropertyId, value: PropertyValue) {
        self.values[index(property)] = value;
    }

    /// The computed custom properties (registered ones that do not inherit
    /// included).
    pub fn custom_properties(&self) -> &CustomProperties {
        &self.custom
    }

    /// The computed font size in pixels.
    pub fn font_size_px(&self) -> f64 {
        font_px_of(self)
    }

    /// The em ratios of this element's first available font, used for its
    /// `ex`, `ch`, `cap` and `ic` units ([`EmRatios::FALLBACK`] without
    /// fonts).
    pub fn font_metrics(&self) -> EmRatios {
        self.font_ratios
    }

    /// `property: value` lines for every longhand whose computed value differs
    /// from the computed initial value, in [`PropertyId::ALL`] order, then
    /// every custom property in name order. Logical longhands, which repeat
    /// their physical counterparts' values, are left out.
    pub fn dump_non_initial(&self) -> String {
        let initial = Self::initial();
        let mut out = String::new();
        for (&property, value) in PropertyId::ALL.iter().zip(&self.values) {
            if !property.is_logical() && value != initial.get(property) {
                out.push_str(property.name());
                out.push_str(": ");
                out.push_str(&value.to_string());
                out.push('\n');
            }
        }
        out.push_str(
            &self
                .custom
                .dump_differences(&CustomProperties::default(), ""),
        );
        out
    }

    /// Compute an element's style from its specified style.
    ///
    /// `parent` is the parent element's computed style and `root` the root
    /// element's (both `None` for the root itself); `is_root` enables root
    /// blockification.
    pub fn compute(
        specified: &SpecifiedStyle,
        parent: Option<&ComputedStyle>,
        root: Option<&ComputedStyle>,
        is_root: bool,
        cx: &ComputeContext,
    ) -> Self {
        compute_values(specified, parent, root, is_root, cx)
    }
}

fn compute_values(
    specified: &SpecifiedStyle,
    parent: Option<&ComputedStyle>,
    root: Option<&ComputedStyle>,
    is_root: bool,
    cx: &ComputeContext,
) -> ComputedStyle {
    use PropertyId as P;
    let parent_font_px = parent.map_or(16.0, font_px_of);
    let parent_weight = match parent.map(|p| p.get(P::FontWeight)) {
        Some(PropertyValue::FontWeight(FontWeight::Number(n))) => *n,
        _ => 400.0,
    };
    let parent_color = match parent.map(|p| p.get(P::Color)) {
        Some(PropertyValue::Color(c)) => *c,
        _ => Color::rgb(0, 0, 0),
    };

    // The first available font's metrics. The root's parent is the initial
    // font: the initial `serif` family at weight 400, normal style/stretch.
    let parent_ratios = match parent {
        Some(p) => p.font_ratios,
        None => cx.em_ratios(&INITIAL_FONT),
    };
    let font = font_query(specified, parent_weight);
    let font_ratios = match parent {
        Some(p) if font == font_query_of(p) => p.font_ratios,
        _ => cx.em_ratios(&font),
    };
    let viewport = (cx.viewport_width, cx.viewport_height);

    // Font size first: other lengths resolve against it. Its own relative
    // units resolve against the parent (and `rem` on the root against the
    // initial 16px).
    let root_font_px = root.map_or(parent_font_px, font_px_of);
    let parent_basis = Basis {
        font_px: parent_font_px,
        root_font_px,
        font: parent_ratios,
        root_font: root.map_or(parent_ratios, |r| r.font_ratios),
        line_px: parent.map_or(parent_font_px * 1.2, |p| {
            line_px(p.get(P::LineHeight), parent_font_px)
        }),
        root_line_px: root.map_or(parent_font_px * 1.2, |r| {
            line_px(r.get(P::LineHeight), font_px_of(r))
        }),
        viewport,
    };
    let font_px = match specified.get(P::FontSize) {
        PropertyValue::FontSize(FontSize::Keyword(k)) => keyword_px(*k, parent_font_px),
        PropertyValue::FontSize(FontSize::LengthPercentage(LengthPercentage::Percentage(p))) => {
            parent_font_px * p / 100.0
        }
        PropertyValue::FontSize(FontSize::LengthPercentage(LengthPercentage::Length(l))) => {
            parent_basis.px(*l)
        }
        PropertyValue::FontSize(FontSize::LengthPercentage(LengthPercentage::Calc(calc))) => calc
            .resolve(|l| parent_basis.px(l), Some(parent_font_px))
            .unwrap_or(parent_font_px)
            .max(0.0),
        _ => parent_font_px,
    };
    // On the root, `rem` and `rlh` in other properties use its own values.
    let root_font_px = if root.is_none() {
        font_px
    } else {
        root_font_px
    };
    let mut basis = Basis {
        font_px,
        root_font_px,
        font: font_ratios,
        root_font: root.map_or(font_ratios, |r| r.font_ratios),
        line_px: font_px * 1.2,
        root_line_px: parent_basis.root_line_px,
        viewport,
    };
    let line_height = match specified.get(P::LineHeight) {
        PropertyValue::LineHeight(LineHeight::LengthPercentage(LengthPercentage::Percentage(
            p,
        ))) => PropertyValue::LineHeight(LineHeight::LengthPercentage(LengthPercentage::Length(
            Length::px(font_px * p / 100.0),
        ))),
        PropertyValue::LineHeight(LineHeight::LengthPercentage(LengthPercentage::Length(l))) => {
            // `lh` in line-height itself refers to the parent's line height.
            let l = if matches!(l.unit, LengthUnit::Lh) {
                Length::px(l.value * parent_basis.line_px)
            } else {
                basis.length(*l)
            };
            PropertyValue::LineHeight(LineHeight::LengthPercentage(LengthPercentage::Length(l)))
        }
        PropertyValue::LineHeight(LineHeight::LengthPercentage(LengthPercentage::Calc(calc))) => {
            let px = calc
                .resolve(
                    |l| {
                        if matches!(l.unit, LengthUnit::Lh) {
                            l.value * parent_basis.line_px
                        } else {
                            basis.px(l)
                        }
                    },
                    Some(font_px),
                )
                .unwrap_or(font_px * 1.2)
                .max(0.0);
            PropertyValue::LineHeight(LineHeight::LengthPercentage(LengthPercentage::Length(
                Length::px(px),
            )))
        }
        other => other.clone(),
    };
    basis.line_px = line_px(&line_height, font_px);
    if root.is_none() {
        basis.root_line_px = basis.line_px;
    }

    let color = match specified.get(P::Color) {
        PropertyValue::Color(Color::CurrentColor) => parent_color,
        PropertyValue::Color(c) => *c,
        _ => parent_color,
    };
    let resolve_color = |value: &PropertyValue| match value {
        PropertyValue::Color(Color::CurrentColor) => PropertyValue::Color(color),
        other => other.clone(),
    };
    let style_of = |property: PropertyId| specified.get(property);
    let border_width = |width: PropertyId, style: PropertyId| {
        let none = matches!(
            style_of(style),
            PropertyValue::BorderStyle(BorderStyle::None | BorderStyle::Hidden)
        );
        let px = match style_of(width) {
            _ if none => 0.0,
            PropertyValue::BorderWidth(BorderWidth::Thin) => 1.0,
            PropertyValue::BorderWidth(BorderWidth::Thick) => 5.0,
            PropertyValue::BorderWidth(BorderWidth::Length(l)) => basis.px(*l),
            PropertyValue::BorderWidth(BorderWidth::Calc(calc)) => {
                calc.resolve(|l| basis.px(l), None).unwrap_or(0.0).max(0.0)
            }
            _ => 3.0,
        };
        PropertyValue::BorderWidth(BorderWidth::Length(Length::px(px)))
    };

    // `outline-width` resolves like a border width whose style is not `none`.
    let outline_width = |width: PropertyId| {
        let px = match style_of(width) {
            PropertyValue::BorderWidth(BorderWidth::Thin) => 1.0,
            PropertyValue::BorderWidth(BorderWidth::Thick) => 5.0,
            PropertyValue::BorderWidth(BorderWidth::Length(l)) => basis.px(*l),
            PropertyValue::BorderWidth(BorderWidth::Calc(calc)) => {
                calc.resolve(|l| basis.px(l), None).unwrap_or(0.0).max(0.0)
            }
            _ => 3.0,
        };
        PropertyValue::BorderWidth(BorderWidth::Length(Length::px(px)))
    };

    // Absolutely positioned and floated boxes are blockified (CSS Display 3
    // §2.7), and absolute positioning makes `float` compute to `none` (CSS 2
    // §9.7).
    let positioned = matches!(
        specified.get(P::Position),
        PropertyValue::Position(Position::Absolute | Position::Fixed)
    );
    let floated =
        !positioned && !matches!(specified.get(P::Float), PropertyValue::Float(Float::None));
    // Children of a flex or grid container are blockified too (CSS Display
    // 3 §2.7, CSS Flexbox 1 §4); `display:contents` generates no box. The
    // parent is the DOM parent's style, so a child promoted through a
    // `display:contents` parent is not blockified (TD-48).
    let container_child = parent.is_some_and(|p| {
        matches!(
            p.get(P::Display),
            PropertyValue::Display(
                Display::Flex | Display::InlineFlex | Display::Grid | Display::InlineGrid
            )
        )
    });
    let overflow = match (specified.get(P::OverflowX), specified.get(P::OverflowY)) {
        (PropertyValue::Overflow(x), PropertyValue::Overflow(y)) => Some(overflow_pair(*x, *y)),
        _ => None,
    };

    let mut anchored = Vec::new();
    let mut values: Vec<PropertyValue> = PropertyId::ALL
        .iter()
        .map(|&property| {
            if property.is_logical() {
                // Replaced by the physical counterpart's value below.
                return PropertyValue::Number(0.0);
            }
            let mut value = specified.get(property);
            if let PropertyValue::Anchored(a) = value {
                anchored.push((property, a.function.map_fallback(|lp| basis.lp(lp, false))));
                value = &a.fallback;
            }
            match property {
                P::Display => match value {
                    PropertyValue::Display(d) if is_root || positioned || floated => {
                        PropertyValue::Display(blockify(*d))
                    }
                    PropertyValue::Display(d)
                        if container_child && !matches!(d, Display::Contents | Display::None) =>
                    {
                        PropertyValue::Display(blockify(*d))
                    }
                    other => other.clone(),
                },
                P::Float if positioned => PropertyValue::Float(Float::None),
                // Times compute to seconds.
                _ if matches!(value, PropertyValue::Animation(_)) => match value {
                    PropertyValue::Animation(list) => PropertyValue::Animation(list.computed()),
                    other => other.clone(),
                },
                P::OverflowX | P::OverflowY => match overflow {
                    Some((x, y)) => {
                        PropertyValue::Overflow(if property == P::OverflowX { x } else { y })
                    }
                    None => value.clone(),
                },
                P::Opacity => match value {
                    PropertyValue::Opacity(n) => PropertyValue::Opacity(n.clamp(0.0, 1.0)),
                    other => other.clone(),
                },
                P::TextAlign => match value {
                    PropertyValue::TextAlign(TextAlign::MatchParent) => {
                        let (align, direction) = match parent {
                            Some(p) => (p.get(P::TextAlign), p.get(P::Direction)),
                            None => (
                                &PropertyValue::TextAlign(TextAlign::Start),
                                &PropertyValue::Direction(Direction::Ltr),
                            ),
                        };
                        match (align, direction) {
                            (PropertyValue::TextAlign(a), PropertyValue::Direction(d)) => {
                                PropertyValue::TextAlign(absolute_align(*a, *d))
                            }
                            _ => PropertyValue::TextAlign(TextAlign::Start),
                        }
                    }
                    other => other.clone(),
                },
                P::TextDecorationColor | P::OutlineColor => resolve_color(value),
                // CSS UI 4 §5.2: computes to 0 when `outline-style` is `none`.
                P::OutlineWidth => {
                    if matches!(
                        style_of(P::OutlineStyle),
                        PropertyValue::OutlineStyle(rustic_css::OutlineStyle::None)
                    ) {
                        PropertyValue::BorderWidth(BorderWidth::Length(Length::px(0.0)))
                    } else {
                        outline_width(property)
                    }
                }
                P::FontStretch => match value {
                    PropertyValue::FontStretch(FontStretch::Keyword(k)) => {
                        PropertyValue::FontStretch(FontStretch::Percentage(k.percentage()))
                    }
                    other => other.clone(),
                },
                P::VerticalAlign => match value {
                    // Percentages refer to the element's line height.
                    PropertyValue::VerticalAlign(VerticalAlign::LengthPercentage(lp)) => {
                        let px = match lp {
                            LengthPercentage::Percentage(p) => basis.line_px * p / 100.0,
                            LengthPercentage::Length(l) => basis.px(*l),
                            LengthPercentage::Calc(calc) => calc
                                .resolve(|l| basis.px(l), Some(basis.line_px))
                                .unwrap_or(0.0),
                        };
                        PropertyValue::VerticalAlign(VerticalAlign::LengthPercentage(
                            LengthPercentage::Length(Length::px(px)),
                        ))
                    }
                    other => other.clone(),
                },
                P::LetterSpacing | P::WordSpacing => match value {
                    PropertyValue::Spacing(Spacing::LengthPercentage(lp)) => {
                        PropertyValue::Spacing(Spacing::LengthPercentage(basis.lp(lp, false)))
                    }
                    other => other.clone(),
                },
                P::BackgroundPosition => match value {
                    PropertyValue::BackgroundPosition(positions) => {
                        PropertyValue::BackgroundPosition(
                            positions
                                .iter()
                                .map(|p| PositionValue {
                                    x: PositionComponent::Offset(basis.lp(&p.x.to_offset(), false)),
                                    y: PositionComponent::Offset(basis.lp(&p.y.to_offset(), false)),
                                })
                                .collect(),
                        )
                    }
                    other => other.clone(),
                },
                P::BackgroundSize => match value {
                    PropertyValue::BackgroundSize(sizes) => PropertyValue::BackgroundSize(
                        sizes
                            .iter()
                            .map(|size| match size {
                                BackgroundSize::Explicit(w, h) => BackgroundSize::Explicit(
                                    w.as_ref().map(|lp| basis.lp(lp, true)),
                                    h.as_ref().map(|lp| basis.lp(lp, true)),
                                ),
                                other => other.clone(),
                            })
                            .collect(),
                    ),
                    other => other.clone(),
                },
                P::Color => PropertyValue::Color(color),
                P::BackgroundColor
                | P::BorderTopColor
                | P::BorderRightColor
                | P::BorderBottomColor
                | P::BorderLeftColor => resolve_color(value),
                P::BorderTopWidth => border_width(property, P::BorderTopStyle),
                P::BorderRightWidth => border_width(property, P::BorderRightStyle),
                P::BorderBottomWidth => border_width(property, P::BorderBottomStyle),
                P::BorderLeftWidth => border_width(property, P::BorderLeftStyle),
                P::FontSize => PropertyValue::FontSize(FontSize::LengthPercentage(
                    LengthPercentage::Length(Length::px(font_px)),
                )),
                P::FontWeight => match value {
                    PropertyValue::FontWeight(w) => PropertyValue::FontWeight(FontWeight::Number(
                        relative_weight(*w, parent_weight),
                    )),
                    other => other.clone(),
                },
                P::LineHeight => line_height.clone(),
                _ => {
                    let nonnegative = !NEGATIVE_LENGTHS.contains(&property);
                    match value {
                        PropertyValue::LengthPercentageOrAuto(
                            LengthPercentageOrAuto::LengthPercentage(lp),
                        ) => PropertyValue::LengthPercentageOrAuto(
                            LengthPercentageOrAuto::LengthPercentage(basis.lp(lp, nonnegative)),
                        ),
                        PropertyValue::LengthPercentage(lp) => {
                            PropertyValue::LengthPercentage(basis.lp(lp, nonnegative))
                        }
                        PropertyValue::MaxSize(MaxSize::LengthPercentage(lp)) => {
                            PropertyValue::MaxSize(MaxSize::LengthPercentage(
                                basis.lp(lp, nonnegative),
                            ))
                        }
                        PropertyValue::MaxSize(MaxSize::FitContent(Some(lp))) => {
                            PropertyValue::MaxSize(MaxSize::FitContent(Some(
                                basis.lp(lp, nonnegative),
                            )))
                        }
                        PropertyValue::Size(size) => {
                            PropertyValue::Size(compute_size(size, &|lp| basis.lp(lp, nonnegative)))
                        }
                        PropertyValue::FlexBasis(FlexBasis::Size(size)) => {
                            PropertyValue::FlexBasis(FlexBasis::Size(compute_size(size, &|lp| {
                                basis.lp(lp, nonnegative)
                            })))
                        }
                        PropertyValue::Gap(Gap::LengthPercentage(lp)) => {
                            PropertyValue::Gap(Gap::LengthPercentage(basis.lp(lp, nonnegative)))
                        }
                        PropertyValue::Grid(grid) => {
                            PropertyValue::Grid(grid.map_lengths(&|lp| basis.lp(lp, true)))
                        }
                        PropertyValue::Transform(list) => {
                            PropertyValue::Transform(rustic_css::TransformList(
                                list.0
                                    .iter()
                                    .map(|f| f.map_lengths(&|lp| basis.lp(lp, false)))
                                    .collect(),
                            ))
                        }
                        PropertyValue::IndividualTransform(rustic_css::IndividualTransform(
                            Some(f),
                        )) => PropertyValue::IndividualTransform(rustic_css::IndividualTransform(
                            Some(f.map_lengths(&|lp| basis.lp(lp, false))),
                        )),
                        PropertyValue::TransformOrigin(p, z) => PropertyValue::TransformOrigin(
                            PositionValue {
                                x: PositionComponent::Offset(basis.lp(&p.x.to_offset(), false)),
                                y: PositionComponent::Offset(basis.lp(&p.y.to_offset(), false)),
                            },
                            basis.lp(z, false),
                        ),
                        PropertyValue::PerspectiveOrigin(p) => {
                            PropertyValue::PerspectiveOrigin(PositionValue {
                                x: PositionComponent::Offset(basis.lp(&p.x.to_offset(), false)),
                                y: PositionComponent::Offset(basis.lp(&p.y.to_offset(), false)),
                            })
                        }
                        PropertyValue::Perspective(Some(d)) => {
                            PropertyValue::Perspective(Some(basis.lp(d, true)))
                        }
                        PropertyValue::BorderSpacing(h, v) => {
                            PropertyValue::BorderSpacing(basis.lp(h, true), basis.lp(v, true))
                        }
                        PropertyValue::ClipPath(rustic_css::ClipPath::Shape(shape, b)) => {
                            PropertyValue::ClipPath(rustic_css::ClipPath::Shape(
                                shape.map_lengths(&|lp| basis.lp(lp, false)),
                                *b,
                            ))
                        }
                        PropertyValue::BoxShadow(shadows) => PropertyValue::BoxShadow(
                            shadows
                                .iter()
                                .map(|shadow| rustic_css::Shadow {
                                    color: Some(match shadow.color {
                                        None | Some(Color::CurrentColor) => color,
                                        Some(c) => c,
                                    }),
                                    ..shadow.map_lengths(&|lp| basis.lp(lp, false), &|lp| {
                                        basis.lp(lp, true)
                                    })
                                })
                                .collect(),
                        ),
                        PropertyValue::Filter(functions) => PropertyValue::Filter(
                            functions
                                .iter()
                                .map(|function| {
                                    let mapped = function
                                        .map_lengths(&|lp| basis.lp(lp, false), &|lp| {
                                            basis.lp(lp, true)
                                        });
                                    match mapped {
                                        rustic_css::FilterFunction::DropShadow(shadow) => {
                                            rustic_css::FilterFunction::DropShadow(
                                                rustic_css::Shadow {
                                                    color: Some(match shadow.color {
                                                        None | Some(Color::CurrentColor) => color,
                                                        Some(c) => c,
                                                    }),
                                                    ..shadow
                                                },
                                            )
                                        }
                                        other => other,
                                    }
                                })
                                .collect(),
                        ),
                        PropertyValue::BorderRadius(r) => {
                            PropertyValue::BorderRadius(rustic_css::CornerRadius {
                                horizontal: basis.lp(&r.horizontal, true),
                                vertical: basis.lp(&r.vertical, true),
                            })
                        }
                        other => other.clone(),
                    }
                }
            }
        })
        .collect();
    copy_logical(&mut values);
    ComputedStyle {
        values,
        // Registered lengths compute to pixels, against this element.
        custom: specified
            .custom_properties()
            .absolutize(&|lp| basis.lp(lp, false)),
        font_ratios,
        anchored,
    }
}

/// Give each logical longhand its physical counterpart's computed value,
/// for the element's own `writing-mode` and `direction` (CSS Logical 1 §4).
pub(crate) fn copy_logical(values: &mut [PropertyValue]) {
    let mode = match values[index(PropertyId::WritingMode)] {
        PropertyValue::WritingMode(mode) => mode,
        _ => WritingMode::HorizontalTb,
    };
    let direction = match values[index(PropertyId::Direction)] {
        PropertyValue::Direction(direction) => direction,
        _ => Direction::Ltr,
    };
    for (i, &property) in PropertyId::ALL.iter().enumerate() {
        if property.is_logical() {
            values[i] = values[index(property.to_physical(mode, direction))].clone();
        }
    }
}

/// The font properties of a specified style, with the weight resolved
/// against the parent's computed weight.
fn font_query(specified: &SpecifiedStyle, parent_weight: f64) -> FontQuery<'_> {
    use PropertyId as P;
    FontQuery {
        families: match specified.get(P::FontFamily) {
            PropertyValue::FontFamily(families) => families,
            _ => &[],
        },
        weight: match specified.get(P::FontWeight) {
            PropertyValue::FontWeight(w) => relative_weight(*w, parent_weight),
            _ => parent_weight,
        },
        style: match specified.get(P::FontStyle) {
            PropertyValue::FontStyle(style) => *style,
            _ => FontStyle::Normal,
        },
        stretch: stretch_percentage(specified.get(P::FontStretch)),
    }
}

/// The font properties of a computed style.
fn font_query_of(style: &ComputedStyle) -> FontQuery<'_> {
    use PropertyId as P;
    FontQuery {
        families: match style.get(P::FontFamily) {
            PropertyValue::FontFamily(families) => families,
            _ => &[],
        },
        weight: match style.get(P::FontWeight) {
            PropertyValue::FontWeight(FontWeight::Number(n)) => *n,
            _ => 400.0,
        },
        style: match style.get(P::FontStyle) {
            PropertyValue::FontStyle(style) => *style,
            _ => FontStyle::Normal,
        },
        stretch: stretch_percentage(style.get(P::FontStretch)),
    }
}

fn stretch_percentage(value: &PropertyValue) -> f64 {
    match value {
        PropertyValue::FontStretch(FontStretch::Keyword(k)) => k.percentage(),
        PropertyValue::FontStretch(FontStretch::Percentage(p)) => *p,
        _ => 100.0,
    }
}
