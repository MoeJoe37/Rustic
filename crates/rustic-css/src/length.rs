//! `<length>` and `<percentage>` values (CSS Values and Units Level 4).

use std::fmt;

use crate::Calc;
use crate::color::write_number;

macro_rules! units {
    ($($variant:ident => $name:literal, $doc:literal;)*) => {
        /// A length unit.
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub enum LengthUnit {
            $(#[doc = $doc] $variant,)*
        }

        impl LengthUnit {
            /// The unit for a dimension's unit text, ASCII case-insensitively.
            pub fn from_name(name: &str) -> Option<Self> {
                let lower = name.to_ascii_lowercase();
                match lower.as_str() {
                    $($name => Some(Self::$variant),)*
                    _ => None,
                }
            }

            /// The canonical (lowercase) unit name.
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)*
                }
            }
        }
    };
}

units! {
    Px => "px", "CSS pixels.";
    Cm => "cm", "Centimeters (96px = 2.54cm).";
    Mm => "mm", "Millimeters.";
    Q => "q", "Quarter-millimeters.";
    In => "in", "Inches (96px).";
    Pt => "pt", "Points (1/72in).";
    Pc => "pc", "Picas (12pt).";
    Em => "em", "The element's font size.";
    Rem => "rem", "The root element's font size.";
    Ex => "ex", "The font's x-height.";
    Rex => "rex", "The root font's x-height.";
    Ch => "ch", "The font's `0` advance.";
    Rch => "rch", "The root font's `0` advance.";
    Cap => "cap", "The font's cap height.";
    Rcap => "rcap", "The root font's cap height.";
    Ic => "ic", "The font's CJK water ideograph advance.";
    Ric => "ric", "The root font's CJK water ideograph advance.";
    Lh => "lh", "The element's line height.";
    Rlh => "rlh", "The root element's line height.";
    Vw => "vw", "1% of the viewport width.";
    Vh => "vh", "1% of the viewport height.";
    Vi => "vi", "1% of the viewport inline size.";
    Vb => "vb", "1% of the viewport block size.";
    Vmin => "vmin", "The smaller of `vw` and `vh`.";
    Vmax => "vmax", "The larger of `vw` and `vh`.";
    Svw => "svw", "1% of the small viewport width.";
    Svh => "svh", "1% of the small viewport height.";
    Lvw => "lvw", "1% of the large viewport width.";
    Lvh => "lvh", "1% of the large viewport height.";
    Dvw => "dvw", "1% of the dynamic viewport width.";
    Dvh => "dvh", "1% of the dynamic viewport height.";
}

impl LengthUnit {
    /// CSS pixels per unit for absolute units; `None` for relative units.
    pub fn absolute_px(self) -> Option<f64> {
        Some(match self {
            Self::Px => 1.0,
            Self::Cm => 96.0 / 2.54,
            Self::Mm => 96.0 / 25.4,
            Self::Q => 96.0 / 101.6,
            Self::In => 96.0,
            Self::Pt => 96.0 / 72.0,
            Self::Pc => 16.0,
            _ => return None,
        })
    }
}

/// A `<length>`: a number and unit, as specified (not yet computed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Length {
    /// The number, always finite.
    pub value: f64,
    /// The unit. A unitless zero is stored as `0px`.
    pub unit: LengthUnit,
}

impl Length {
    /// A length in CSS pixels.
    pub fn px(value: f64) -> Self {
        Self {
            value,
            unit: LengthUnit::Px,
        }
    }

    /// The length in CSS pixels, if its unit is absolute.
    pub fn to_px(self) -> Option<f64> {
        self.unit.absolute_px().map(|per| self.value * per)
    }
}

impl fmt::Display for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_number(f, self.value)?;
        f.write_str(self.unit.name())
    }
}

/// A `<length-percentage>`.
#[derive(Clone, Debug, PartialEq)]
pub enum LengthPercentage {
    /// A length.
    Length(Length),
    /// A percentage, such as `50` for `50%`.
    Percentage(f64),
    /// A math function that did not simplify to a single length or
    /// percentage, such as `calc(50% - 2em)`. Its range is not checked when
    /// parsing: a negative result is clamped by whoever resolves it.
    Calc(Box<Calc>),
}

impl LengthPercentage {
    /// Zero pixels.
    pub const ZERO: Self = Self::Length(Length {
        value: 0.0,
        unit: LengthUnit::Px,
    });

    pub(crate) fn is_negative(&self) -> bool {
        match self {
            Self::Length(l) => l.value < 0.0,
            Self::Percentage(p) => *p < 0.0,
            Self::Calc(_) => false,
        }
    }
}

impl fmt::Display for LengthPercentage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length(l) => write!(f, "{l}"),
            Self::Percentage(p) => {
                write_number(f, *p)?;
                f.write_str("%")
            }
            Self::Calc(calc) => write!(f, "{calc}"),
        }
    }
}
