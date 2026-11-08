//! CSS 2.2 §10.3.3/10.4 block widths, with CSS Sizing border-box adjustment.

use crate::{CssPx, GeometryError};
use rustic_css::{
    BorderWidth, BoxSizing, Direction, Display, Float, LengthPercentage, LengthPercentageOrAuto,
    LengthUnit, MaxSize, Position, PropertyId as P, PropertyValue as V,
};
use rustic_style::ComputedStyle;
use std::{error::Error, fmt};

/// Physical box edges, in CSS pixels. Margins can be negative.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges {
    /// Top edge.
    pub top: CssPx,
    /// Right edge.
    pub right: CssPx,
    /// Bottom edge.
    pub bottom: CssPx,
    /// Left edge.
    pub left: CssPx,
}

/// Used horizontal dimensions of one non-replaced block. No vertical flow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockMeasure {
    /// Nonnegative content width.
    pub content_width: CssPx,
    /// Used margins (vertical auto margins are zero).
    pub margin: Edges,
    /// Nonnegative padding, percentages against containing-block width.
    pub padding: Edges,
    /// Nonnegative computed border widths.
    pub border: Edges,
}

/// A block measurement failure; no partial measurement is returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeasureError {
    /// The style belongs to a different formatting context or positioning mode.
    UnsupportedFormatting,
    /// A value was not normalized to computed pixels/percentages.
    InvalidComputedValue,
    /// Arithmetic exceeded the checked geometry range.
    Geometry(GeometryError),
}

impl From<GeometryError> for MeasureError {
    fn from(value: GeometryError) -> Self {
        Self::Geometry(value)
    }
}

impl fmt::Display for MeasureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedFormatting => {
                "width measurement requires a non-floated normal-flow block"
            }
            Self::InvalidComputedValue => "width measurement requires computed CSS values",
            Self::Geometry(_) => "block dimensions exceed the geometry budget",
        })
    }
}
impl Error for MeasureError {}

fn lp(value: &LengthPercentage, basis: f64) -> Result<f64, MeasureError> {
    let n = match value {
        LengthPercentage::Length(l) if l.unit == LengthUnit::Px => l.value,
        LengthPercentage::Percentage(p) => basis * (p / 100.0),
        LengthPercentage::Calc(calc) => {
            let valid = std::cell::Cell::new(true);
            let resolved = calc
                .resolve(
                    |l| {
                        if l.unit == LengthUnit::Px {
                            l.value
                        } else {
                            valid.set(false);
                            0.0
                        }
                    },
                    Some(basis),
                )
                .ok_or(MeasureError::InvalidComputedValue)?;
            if !valid.get() {
                return Err(MeasureError::InvalidComputedValue);
            }
            resolved
        }
        _ => return Err(MeasureError::InvalidComputedValue),
    };
    Ok(CssPx::new(n)?.get())
}

fn auto(style: &ComputedStyle, property: P, basis: f64) -> Result<Option<f64>, MeasureError> {
    match style.get(property) {
        V::LengthPercentageOrAuto(LengthPercentageOrAuto::Auto) => Ok(None),
        V::LengthPercentageOrAuto(LengthPercentageOrAuto::LengthPercentage(v)) => {
            Ok(Some(lp(v, basis)?))
        }
        _ => Err(MeasureError::InvalidComputedValue),
    }
}

fn padding(style: &ComputedStyle, property: P, basis: f64) -> Result<CssPx, MeasureError> {
    match style.get(property) {
        V::LengthPercentage(v) => Ok(CssPx::new(lp(v, basis)?.max(0.0))?),
        _ => Err(MeasureError::InvalidComputedValue),
    }
}

fn border(style: &ComputedStyle, property: P) -> Result<CssPx, MeasureError> {
    match style.get(property) {
        V::BorderWidth(BorderWidth::Length(l)) if l.unit == LengthUnit::Px && l.value >= 0.0 => {
            Ok(CssPx::new(l.value)?)
        }
        _ => Err(MeasureError::InvalidComputedValue),
    }
}

/// Measure a horizontal, non-replaced block against a definite containing width.
///
/// `containing_direction` is the containing block's direction (CSS 2.2
/// §10.3.3), which selects the adjusted margin in over-constrained cases.
/// Width, min/max-width, box-sizing, percentages, calc and auto margins are
/// resolved here. The caller selects non-replaced boxes; intrinsic/replaced
/// sizing and vertical layout are separate algorithms. No font lookup, cascade,
/// clipping, positioning offsets or page resources are performed.
pub fn measure_block_width(
    style: &ComputedStyle,
    containing_width: CssPx,
    containing_direction: Direction,
) -> Result<BlockMeasure, MeasureError> {
    if containing_width.get() < 0.0 {
        return Err(GeometryError.into());
    }
    if !matches!(
        style.get(P::Display),
        V::Display(Display::Block | Display::FlowRoot)
    ) || !matches!(
        style.get(P::Position),
        V::Position(Position::Static | Position::Relative)
    ) || !matches!(style.get(P::Float), V::Float(Float::None))
    {
        return Err(MeasureError::UnsupportedFormatting);
    }
    let basis = containing_width.get();
    let pad = Edges {
        top: padding(style, P::PaddingTop, basis)?,
        right: padding(style, P::PaddingRight, basis)?,
        bottom: padding(style, P::PaddingBottom, basis)?,
        left: padding(style, P::PaddingLeft, basis)?,
    };
    let borders = Edges {
        top: border(style, P::BorderTopWidth)?,
        right: border(style, P::BorderRightWidth)?,
        bottom: border(style, P::BorderBottomWidth)?,
        left: border(style, P::BorderLeftWidth)?,
    };
    let extras = pad
        .left
        .checked_add(pad.right)?
        .checked_add(borders.left)?
        .checked_add(borders.right)?
        .get();
    let box_adjust = |n: f64| -> f64 {
        if matches!(style.get(P::BoxSizing), V::BoxSizing(BoxSizing::BorderBox)) {
            (n.max(0.0) - extras).max(0.0)
        } else {
            n.max(0.0)
        }
    };
    let left = auto(style, P::MarginLeft, basis)?;
    let right = auto(style, P::MarginRight, basis)?;
    let specified = auto(style, P::Width, basis)?.map(box_adjust);
    let minimum = box_adjust(auto(style, P::MinWidth, basis)?.unwrap_or(0.0));
    let maximum = match style.get(P::MaxWidth) {
        V::MaxSize(MaxSize::None) => None,
        V::MaxSize(MaxSize::LengthPercentage(v)) => Some(box_adjust(lp(v, basis)?)),
        _ => return Err(MeasureError::InvalidComputedValue),
    };
    let mut ml = left.unwrap_or(0.0);
    let mut mr = right.unwrap_or(0.0);
    let available = CssPx::new(basis - extras - ml - mr)?.get();
    let tentative = specified.unwrap_or(available.max(0.0));
    let width = maximum.map_or(tentative, |m| tentative.min(m)).max(minimum);
    let underflow = CssPx::new(basis - extras - ml - mr - width)?.get();
    let fixed = specified.is_some() || width != tentative;
    if fixed && underflow >= 0.0 && (left.is_none() || right.is_none()) {
        match (left, right) {
            (None, None) => {
                ml = underflow / 2.0;
                mr = ml;
            }
            (None, _) => ml = underflow,
            (_, None) => mr = underflow,
            _ => {}
        }
    } else {
        match containing_direction {
            Direction::Ltr => mr += underflow,
            Direction::Rtl => ml += underflow,
        }
    }
    Ok(BlockMeasure {
        content_width: CssPx::new(width)?,
        padding: pad,
        border: borders,
        margin: Edges {
            left: CssPx::new(ml)?,
            right: CssPx::new(mr)?,
            top: CssPx::new(auto(style, P::MarginTop, basis)?.unwrap_or(0.0))?,
            bottom: CssPx::new(auto(style, P::MarginBottom, basis)?.unwrap_or(0.0))?,
        },
    })
}
