//! CSS 2.2 §10.3.3/10.4 block widths, with CSS Sizing border-box adjustment.

use crate::intrinsic::{Intrinsic, Resolver, Sizing};
use crate::{CssPx, GeometryError};
use rustic_css::{
    BorderWidth, Direction, Display, Float, LengthPercentage, LengthPercentageOrAuto, LengthUnit,
    Position, PropertyId as P, PropertyValue as V,
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
    /// An intrinsic sizing keyword (`min-content`, `max-content`,
    /// `fit-content`) needs content sizes that this entry point lacks.
    IntrinsicSizeRequired,
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
            Self::IntrinsicSizeRequired => "an intrinsic sizing keyword needs content sizes",
        })
    }
}
impl Error for MeasureError {}

pub(crate) fn lp(value: &LengthPercentage, basis: f64) -> Result<f64, MeasureError> {
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

pub(crate) fn auto(
    style: &ComputedStyle,
    property: P,
    basis: f64,
) -> Result<Option<f64>, MeasureError> {
    match style.get(property) {
        V::LengthPercentageOrAuto(LengthPercentageOrAuto::Auto) => Ok(None),
        V::LengthPercentageOrAuto(LengthPercentageOrAuto::LengthPercentage(v)) => {
            Ok(Some(lp(v, basis)?))
        }
        _ => Err(MeasureError::InvalidComputedValue),
    }
}

pub(crate) fn padding(
    style: &ComputedStyle,
    property: P,
    basis: f64,
) -> Result<CssPx, MeasureError> {
    match style.get(property) {
        V::LengthPercentage(v) => Ok(CssPx::new(lp(v, basis)?.max(0.0))?),
        _ => Err(MeasureError::InvalidComputedValue),
    }
}

pub(crate) fn border(style: &ComputedStyle, property: P) -> Result<CssPx, MeasureError> {
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
    measure_in_band(
        style,
        containing_width,
        containing_direction,
        (0.0, 0.0),
        None,
        None,
    )
}

/// Padding and border edges; percentages resolve against `basis` (CSS 2.2 §8.4).
pub(crate) fn box_edges(style: &ComputedStyle, basis: f64) -> Result<(Edges, Edges), MeasureError> {
    Ok((
        Edges {
            top: padding(style, P::PaddingTop, basis)?,
            right: padding(style, P::PaddingRight, basis)?,
            bottom: padding(style, P::PaddingBottom, basis)?,
            left: padding(style, P::PaddingLeft, basis)?,
        },
        Edges {
            top: border(style, P::BorderTopWidth)?,
            right: border(style, P::BorderRightWidth)?,
            bottom: border(style, P::BorderBottomWidth)?,
            left: border(style, P::BorderLeftWidth)?,
        },
    ))
}

/// The normal-flow width equation inside a float-narrowed band.
///
/// `floors` are the distances by which floats intrude from the containing
/// block's left and right edges. A margin smaller than its floor is raised to
/// it (margins may overlap floats, borders may not); auto margins distribute
/// the remaining band. Percentages still resolve against the full width.
/// `inner` supplies content sizes for intrinsic sizing keywords; without it
/// a keyword fails with [`MeasureError::IntrinsicSizeRequired`].
pub(crate) fn measure_in_band(
    style: &ComputedStyle,
    containing_width: CssPx,
    containing_direction: Direction,
    floors: (f64, f64),
    inner: Option<Intrinsic>,
    forced: Option<f64>,
) -> Result<BlockMeasure, MeasureError> {
    if inner.is_none() {
        for p in [P::Width, P::MinWidth, P::MaxWidth] {
            if Sizing::read(style, p)?.is_keyword() {
                return Err(MeasureError::IntrinsicSizeRequired);
            }
        }
    }
    let basis = containing_width.get();
    let (pad, borders) = box_edges(style, basis)?;
    let (floor_left, floor_right) = floors;
    let band = basis - floor_left - floor_right;
    let extras = pad
        .left
        .checked_add(pad.right)?
        .checked_add(borders.left)?
        .checked_add(borders.right)?
        .get();
    // Inside an intruded band, a margin keeps only its part beyond the float.
    let lower =
        |m: Option<f64>, floor: f64| m.map(|m| if floor > 0.0 { (m - floor).max(0.0) } else { m });
    let left = lower(auto(style, P::MarginLeft, basis)?, floor_left);
    let right = lower(auto(style, P::MarginRight, basis)?, floor_right);
    let mut ml = left.unwrap_or(0.0);
    let mut mr = right.unwrap_or(0.0);
    // `fit-content` sizes into the band left after margins, padding and borders.
    let resolver = Resolver::new(style, Some(basis), extras, inner.unwrap_or_default());
    // A replaced box's used width is already constrained (CSS 2.2 §10.4).
    let (specified, minimum, maximum) = match forced {
        Some(w) => (Some(w), 0.0, None),
        None => resolver.widths(band - extras - ml - mr)?,
    };
    let available = CssPx::new(band - extras - ml - mr)?.get();
    let tentative = specified.unwrap_or(available.max(0.0));
    let width = maximum.map_or(tentative, |m| tentative.min(m)).max(minimum);
    let underflow = CssPx::new(band - extras - ml - mr - width)?.get();
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
            left: CssPx::new(ml + floor_left)?,
            right: CssPx::new(mr + floor_right)?,
            top: CssPx::new(auto(style, P::MarginTop, basis)?.unwrap_or(0.0))?,
            bottom: CssPx::new(auto(style, P::MarginBottom, basis)?.unwrap_or(0.0))?,
        },
    })
}
