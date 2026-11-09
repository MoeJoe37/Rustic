//! Used content heights for the block-only reference, CSS 2.2 §10.5–10.7.

use rustic_css::{
    BoxSizing, LengthPercentage, LengthPercentageOrAuto, MaxSize, PropertyId as P,
    PropertyValue as V,
};
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
    match style.get(property) {
        V::LengthPercentageOrAuto(LengthPercentageOrAuto::Auto) => Ok(None),
        V::LengthPercentageOrAuto(LengthPercentageOrAuto::LengthPercentage(value)) => {
            resolve(value, basis)
        }
        _ => Err(MeasureError::InvalidComputedValue),
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
        let maximum = match style.get(P::MaxHeight) {
            V::MaxSize(MaxSize::None) => None,
            V::MaxSize(MaxSize::LengthPercentage(value)) => {
                resolve(value, basis)?.map(adjust).transpose()?
            }
            _ => return Err(MeasureError::InvalidComputedValue),
        };
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
