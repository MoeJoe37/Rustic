//! Used content heights for the block-only reference, CSS 2.2 §10.5–10.7.

use rustic_css::{BoxSizing, LengthPercentage, PropertyId as P, PropertyValue as V};

use crate::intrinsic::Sizing;
use rustic_style::ComputedStyle;

use crate::{CssPx, MeasureError};

pub(crate) struct HeightConstraint {
    specified: Option<CssPx>,
    minimum: CssPx,
    maximum: Option<CssPx>,
}

fn resolve(value: &LengthPercentage, basis: Option<CssPx>) -> Result<Option<f64>, MeasureError> {
    let percentage = match value {
        LengthPercentage::Percentage(_) => true,
        LengthPercentage::Calc(calc) => calc.has_percentage(),
        LengthPercentage::Length(_) => false,
    };
    // A percentage against an indefinite containing height remains auto (or
    // the initial min/max bound). It never uses the parent's eventual auto height.
    if percentage && basis.is_none() {
        return Ok(None);
    }
    crate::sizing::lp(value, basis.map_or(0.0, CssPx::get)).map(Some)
}

fn auto(
    style: &ComputedStyle,
    property: P,
    basis: Option<CssPx>,
) -> Result<Option<f64>, MeasureError> {
    // Intrinsic keywords behave as the initial value in the block axis of a
    // block container (CSS Sizing 3 §3.1).
    match Sizing::read(style, property)?.length() {
        Some(value) => resolve(value, basis),
        None => Ok(None),
    }
}

impl HeightConstraint {
    pub(crate) fn new(
        style: &ComputedStyle,
        basis: Option<CssPx>,
        extras: CssPx,
    ) -> Result<Self, MeasureError> {
        let adjust = |value: f64| -> Result<CssPx, MeasureError> {
            let content = if matches!(style.get(P::BoxSizing), V::BoxSizing(BoxSizing::BorderBox)) {
                value.max(0.0) - extras.get()
            } else {
                value
            };
            Ok(CssPx::new(content.max(0.0))?)
        };
        let specified = auto(style, P::Height, basis)?.map(adjust).transpose()?;
        let minimum = adjust(auto(style, P::MinHeight, basis)?.unwrap_or(0.0))?;
        let maximum = auto(style, P::MaxHeight, basis)?.map(adjust).transpose()?;
        Ok(Self {
            specified,
            minimum,
            maximum,
        })
    }

    /// A constraint already solved elsewhere (absolutely positioned boxes):
    /// `Some` is a definite content height, `None` is content-based.
    pub(crate) fn solved(definite: Option<CssPx>) -> Self {
        Self {
            specified: definite,
            minimum: CssPx::ZERO,
            maximum: None,
        }
    }

    /// A content-based constraint with solved bounds (absolutely positioned
    /// boxes whose height depends on their content): flex and grid layout
    /// size their tracks and lines against the bounds.
    pub(crate) fn bounded(minimum: CssPx, maximum: Option<CssPx>) -> Self {
        Self {
            specified: None,
            minimum,
            maximum,
        }
    }

    pub(crate) fn used(&self, automatic: CssPx) -> CssPx {
        let tentative = self.specified.unwrap_or(automatic);
        let limited = self.maximum.map_or(
            tentative,
            |max| if tentative > max { max } else { tentative },
        );
        if limited < self.minimum {
            self.minimum
        } else {
            limited
        }
    }

    /// The specified content height, if definite.
    pub(crate) fn specified(&self) -> Option<CssPx> {
        self.specified
    }

    /// Used minimum content height.
    pub(crate) fn minimum(&self) -> CssPx {
        self.minimum
    }

    /// Used maximum content height, if any.
    pub(crate) fn maximum(&self) -> Option<CssPx> {
        self.maximum
    }

    pub(crate) fn is_auto(&self) -> bool {
        self.specified.is_none()
    }

    pub(crate) fn can_collapse_through(&self) -> bool {
        self.minimum == CssPx::ZERO && self.specified.is_none_or(|height| height == CssPx::ZERO)
    }

    pub(crate) fn definite(&self) -> Option<CssPx> {
        self.specified.map(|_| self.used(CssPx::ZERO))
    }
}
