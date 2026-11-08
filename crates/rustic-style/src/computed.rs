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
//! Font metrics are not available before font loading (M3), so `ex` and `ch`
//! use the specification's fallback of `0.5em`, `cap` uses `0.7em`, and `ic`
//! uses `1em`; `lh` uses the element's computed line height with `normal`
//! taken as `1.2`.

use rustic_css::{
    BorderStyle, BorderWidth, Color, Display, FontSize, FontSizeKeyword, FontWeight, Length,
    LengthPercentage, LengthPercentageOrAuto, LengthUnit, LineHeight, MaxSize, PropertyId,
    PropertyValue,
};

use crate::SpecifiedStyle;

/// The environment computed values depend on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ComputeContext {
    /// Viewport width in CSS pixels (for `vw`, `vi`, `vmin`, `vmax`, ...).
    pub viewport_width: f64,
    /// Viewport height in CSS pixels (for `vh`, `vb`, `vmin`, `vmax`, ...).
    pub viewport_height: f64,
}

impl Default for ComputeContext {
    /// A 1280×720 viewport, matching `rustic-engine`'s default.
    fn default() -> Self {
        Self {
            viewport_width: 1280.0,
            viewport_height: 720.0,
        }
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
    line_px: f64,
    root_line_px: f64,
    viewport: ComputeContext,
}

impl Basis {
    fn px(&self, length: Length) -> f64 {
        if let Some(px) = length.to_px() {
            return px;
        }
        let v = length.value;
        let (w, h) = (self.viewport.viewport_width, self.viewport.viewport_height);
        let result = match length.unit {
            LengthUnit::Em => v * self.font_px,
            LengthUnit::Rem => v * self.root_font_px,
            LengthUnit::Ex | LengthUnit::Ch => v * self.font_px * 0.5,
            LengthUnit::Rex | LengthUnit::Rch => v * self.root_font_px * 0.5,
            LengthUnit::Cap => v * self.font_px * 0.7,
            LengthUnit::Rcap => v * self.root_font_px * 0.7,
            LengthUnit::Ic => v * self.font_px,
            LengthUnit::Ric => v * self.root_font_px,
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

    fn lp(&self, value: LengthPercentage) -> LengthPercentage {
        match value {
            LengthPercentage::Length(l) => LengthPercentage::Length(self.length(l)),
            percentage => percentage,
        }
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

    /// The computed font size in pixels.
    pub fn font_size_px(&self) -> f64 {
        font_px_of(self)
    }

    /// `property: value` lines for every longhand whose computed value differs
    /// from the computed initial value, in [`PropertyId::ALL`] order.
    pub fn dump_non_initial(&self) -> String {
        let initial = Self::initial();
        let mut out = String::new();
        for (&property, value) in PropertyId::ALL.iter().zip(&self.values) {
            if value != initial.get(property) {
                out.push_str(property.name());
                out.push_str(": ");
                out.push_str(&value.to_string());
                out.push('\n');
            }
        }
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

    // Font size first: other lengths resolve against it. Its own relative
    // units resolve against the parent (and `rem` on the root against the
    // initial 16px).
    let root_font_px = root.map_or(parent_font_px, font_px_of);
    let parent_basis = Basis {
        font_px: parent_font_px,
        root_font_px,
        line_px: parent.map_or(parent_font_px * 1.2, |p| {
            line_px(p.get(P::LineHeight), parent_font_px)
        }),
        root_line_px: root.map_or(parent_font_px * 1.2, |r| {
            line_px(r.get(P::LineHeight), font_px_of(r))
        }),
        viewport: *cx,
    };
    let font_px = match specified.get(P::FontSize) {
        PropertyValue::FontSize(FontSize::Keyword(k)) => keyword_px(*k, parent_font_px),
        PropertyValue::FontSize(FontSize::LengthPercentage(LengthPercentage::Percentage(p))) => {
            parent_font_px * p / 100.0
        }
        PropertyValue::FontSize(FontSize::LengthPercentage(LengthPercentage::Length(l))) => {
            parent_basis.px(*l)
        }
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
        line_px: font_px * 1.2,
        root_line_px: parent_basis.root_line_px,
        viewport: *cx,
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
            _ => 3.0,
        };
        PropertyValue::BorderWidth(BorderWidth::Length(Length::px(px)))
    };

    let values = PropertyId::ALL
        .iter()
        .map(|&property| {
            let value = specified.get(property);
            match property {
                P::Display => match value {
                    PropertyValue::Display(d) if is_root => PropertyValue::Display(blockify(*d)),
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
                _ => match value {
                    PropertyValue::LengthPercentageOrAuto(
                        LengthPercentageOrAuto::LengthPercentage(lp),
                    ) => PropertyValue::LengthPercentageOrAuto(
                        LengthPercentageOrAuto::LengthPercentage(basis.lp(*lp)),
                    ),
                    PropertyValue::LengthPercentage(lp) => {
                        PropertyValue::LengthPercentage(basis.lp(*lp))
                    }
                    PropertyValue::MaxSize(MaxSize::LengthPercentage(lp)) => {
                        PropertyValue::MaxSize(MaxSize::LengthPercentage(basis.lp(*lp)))
                    }
                    other => other.clone(),
                },
            }
        })
        .collect();
    ComputedStyle { values }
}
