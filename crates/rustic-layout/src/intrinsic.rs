//! Intrinsic sizes and sizing keywords (CSS Sizing 3 §4–§5).
//!
//! Every box carries a content-box [`Intrinsic`] min-content/max-content
//! inline size, computed bottom up before layout. Width-like properties are
//! read through [`Sizing`], which resolves lengths, percentages and the
//! `min-content`, `max-content`, `fit-content` and `fit-content(<lp>)`
//! keywords against those sizes.

use rustic_css::{BoxSizing, LengthPercentage, MaxSize, PropertyId as P, PropertyValue as V, Size};
use rustic_style::ComputedStyle;

use crate::MeasureError;
use crate::sizing::{box_edges, lp};

/// Content-box intrinsic inline sizes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Intrinsic {
    pub min: f64,
    pub max: f64,
}

impl Intrinsic {
    /// CSS 2.2 §10.3.5 shrink-to-fit / CSS Sizing 3 fit-content.
    pub(crate) fn fit(self, available: f64) -> f64 {
        self.min.max(available).min(self.max)
    }
}

/// A borrowed width/height-like value: `width`, `min-width`, `max-width`,
/// their block-axis counterparts and `flex-basis`'s size.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Sizing<'a> {
    /// `auto` (or `none` for maximums).
    Initial,
    Length(&'a LengthPercentage),
    MinContent,
    MaxContent,
    FitContent(Option<&'a LengthPercentage>),
}

impl<'a> Sizing<'a> {
    pub(crate) fn of_size(size: &'a Size) -> Self {
        match size {
            Size::Auto => Self::Initial,
            Size::LengthPercentage(v) => Self::Length(v),
            Size::MinContent => Self::MinContent,
            Size::MaxContent => Self::MaxContent,
            Size::FitContent(v) => Self::FitContent(v.as_ref()),
        }
    }

    /// Read a `width`-like (`Size`) or `max-width`-like (`MaxSize`) property.
    pub(crate) fn read(style: &'a ComputedStyle, property: P) -> Result<Self, MeasureError> {
        Ok(match style.get(property) {
            V::Size(size) => Self::of_size(size),
            V::MaxSize(MaxSize::None) => Self::Initial,
            V::MaxSize(MaxSize::LengthPercentage(v)) => Self::Length(v),
            V::MaxSize(MaxSize::MinContent) => Self::MinContent,
            V::MaxSize(MaxSize::MaxContent) => Self::MaxContent,
            V::MaxSize(MaxSize::FitContent(v)) => Self::FitContent(v.as_ref()),
            _ => return Err(MeasureError::InvalidComputedValue),
        })
    }

    /// Whether the value is an intrinsic sizing keyword.
    pub(crate) fn is_keyword(self) -> bool {
        matches!(
            self,
            Self::MinContent | Self::MaxContent | Self::FitContent(_)
        )
    }

    /// The keyword-free part only: a length/percentage, or `None` for
    /// `auto`/`none` and (in the block axis) the keywords, which behave as
    /// the initial value there (CSS Sizing 3 §3.1).
    pub(crate) fn length(self) -> Option<&'a LengthPercentage> {
        match self {
            Self::Length(v) => Some(v),
            _ => None,
        }
    }
}

/// Whether a length-percentage contains a percentage.
pub(crate) fn has_percentage(value: &LengthPercentage) -> bool {
    match value {
        LengthPercentage::Percentage(_) => true,
        LengthPercentage::Calc(calc) => calc.has_percentage(),
        LengthPercentage::Length(_) => false,
    }
}

/// A length with no percentage, or `None` (percentages are cyclic for
/// intrinsic contributions and behave as their initial value).
pub(crate) fn fixed(value: &LengthPercentage) -> Result<Option<f64>, MeasureError> {
    if has_percentage(value) {
        Ok(None)
    } else {
        lp(value, 0.0).map(Some)
    }
}

/// Resolves width-like values to content-box sizes for one box.
pub(crate) struct Resolver<'a> {
    style: &'a ComputedStyle,
    /// Percentage basis; `None` makes percentages behave as the initial value.
    basis: Option<f64>,
    /// Horizontal padding plus border, for `box-sizing:border-box`.
    extras: f64,
    inner: Intrinsic,
}

impl<'a> Resolver<'a> {
    pub(crate) fn new(
        style: &'a ComputedStyle,
        basis: Option<f64>,
        extras: f64,
        inner: Intrinsic,
    ) -> Self {
        Self {
            style,
            basis,
            extras,
            inner,
        }
    }

    fn adjust(&self, n: f64) -> f64 {
        if matches!(
            self.style.get(P::BoxSizing),
            V::BoxSizing(BoxSizing::BorderBox)
        ) {
            (n.max(0.0) - self.extras).max(0.0)
        } else {
            n.max(0.0)
        }
    }

    fn length(&self, value: &LengthPercentage) -> Result<Option<f64>, MeasureError> {
        match self.basis {
            Some(basis) => lp(value, basis).map(|n| Some(self.adjust(n))),
            None => Ok(fixed(value)?.map(|n| self.adjust(n))),
        }
    }

    /// The content-box size `value` asks for, given the content-box
    /// `available` space for `fit-content`; `None` for `auto`/`none`.
    pub(crate) fn resolve(
        &self,
        value: Sizing<'_>,
        available: f64,
    ) -> Result<Option<f64>, MeasureError> {
        let inner = self.inner;
        Ok(match value {
            Sizing::Initial => None,
            Sizing::Length(v) => self.length(v)?,
            Sizing::MinContent => Some(inner.min),
            Sizing::MaxContent => Some(inner.max),
            Sizing::FitContent(None) => Some(inner.fit(available)),
            Sizing::FitContent(Some(v)) => Some(match self.length(v)? {
                Some(limit) => inner.max.min(inner.min.max(limit)),
                None => inner.fit(available),
            }),
        })
    }

    /// `(specified, minimum, maximum)` content-box widths.
    pub(crate) fn widths(
        &self,
        available: f64,
    ) -> Result<(Option<f64>, f64, Option<f64>), MeasureError> {
        let read = |p| Sizing::read(self.style, p);
        Ok((
            self.resolve(read(P::Width)?, available)?,
            self.resolve(read(P::MinWidth)?, available)?.unwrap_or(0.0),
            self.resolve(read(P::MaxWidth)?, available)?,
        ))
    }
}

/// Horizontal padding (fixed parts only) plus borders, for contributions.
pub(crate) fn fixed_extras(style: &ComputedStyle) -> Result<f64, MeasureError> {
    let mut extras = 0.0;
    for p in [P::PaddingLeft, P::PaddingRight] {
        if let V::LengthPercentage(v) = style.get(p) {
            extras += fixed(v)?.unwrap_or(0.0).max(0.0);
        } else {
            return Err(MeasureError::InvalidComputedValue);
        }
    }
    let (_, border) = box_edges(style, 0.0)?;
    Ok(extras + border.left.get() + border.right.get())
}

/// A margin's fixed part (`auto` and percentages contribute zero).
pub(crate) fn fixed_margin(style: &ComputedStyle, property: P) -> Result<f64, MeasureError> {
    match style.get(property) {
        V::LengthPercentageOrAuto(rustic_css::LengthPercentageOrAuto::Auto) => Ok(0.0),
        V::LengthPercentageOrAuto(rustic_css::LengthPercentageOrAuto::LengthPercentage(v)) => {
            Ok(fixed(v)?.unwrap_or(0.0))
        }
        _ => Err(MeasureError::InvalidComputedValue),
    }
}

/// Outer (margin-box) min/max-content contribution of a block-level box
/// (CSS Sizing 3 §5.1): its `width`/`min-width`/`max-width` applied to its
/// content-box intrinsic sizes, plus fixed padding, borders and margins.
pub(crate) fn outer_contribution(
    style: &ComputedStyle,
    inner: Intrinsic,
) -> Result<Intrinsic, MeasureError> {
    let extras = fixed_extras(style)?;
    let r = Resolver::new(style, None, extras, inner);
    // The min-content contribution sizes into zero space, the max-content
    // contribution into infinite space; `fit-content` follows.
    let contribution = |available: f64| -> Result<f64, MeasureError> {
        let (specified, minimum, maximum) = r.widths(available)?;
        let tentative = specified.unwrap_or(if available.is_finite() {
            inner.min
        } else {
            inner.max
        });
        Ok(maximum.map_or(tentative, |m| tentative.min(m)).max(minimum))
    };
    let margins = fixed_margin(style, P::MarginLeft)? + fixed_margin(style, P::MarginRight)?;
    let outer = |n: f64| (n + extras + margins).max(0.0);
    Ok(Intrinsic {
        min: outer(contribution(0.0)?),
        max: outer(contribution(f64::INFINITY)?),
    })
}
