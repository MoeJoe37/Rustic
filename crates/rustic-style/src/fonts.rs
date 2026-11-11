//! Font metrics behind the font-relative units `ex`, `ch`, `cap` and `ic`
//! (CSS Values 4 §6.1.1).
//!
//! Computing a style asks the [`crate::ComputeContext`]'s
//! [`FontMetricsSource`] for the element's *first available font*, chosen
//! from its computed `font-family`, `font-weight`, `font-style` and
//! `font-stretch`. Without a source, or when no font matches, the CSS
//! fallbacks in [`EmRatios::FALLBACK`] apply.

use std::fmt::Debug;

use rustic_css::{FamilyName, FontStyle, FontUnitRatios, GenericFamily};
use rustic_text::{EmRatios, FaceQuery, FaceStyle, FamilyRef, FontDatabase};

/// The computed font properties a first available font is chosen from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontQuery<'a> {
    /// The computed `font-family` list.
    pub families: &'a [FamilyName],
    /// The computed `font-weight`.
    pub weight: f64,
    /// The computed `font-style`.
    pub style: FontStyle,
    /// The computed `font-stretch`, as a percentage.
    pub stretch: f64,
}

/// Supplies font metrics while computing styles. Implemented for
/// [`FontDatabase`]; tests and embedders can provide their own.
pub trait FontMetricsSource: Send + Sync + Debug {
    /// The em ratios of the first available font for `font`, or `None` when
    /// no font is available (the CSS fallbacks then apply).
    fn em_ratios(&self, font: &FontQuery<'_>) -> Option<EmRatios>;
}

impl FontMetricsSource for FontDatabase {
    fn em_ratios(&self, font: &FontQuery<'_>) -> Option<EmRatios> {
        let families = font.families.iter().map(|family| match family {
            FamilyName::Named(name) => FamilyRef::Named(name),
            FamilyName::Generic(generic) => FamilyRef::Generic(generic.name()),
        });
        let query = FaceQuery {
            weight: font.weight,
            style: match font.style {
                FontStyle::Normal => FaceStyle::Normal,
                FontStyle::Italic => FaceStyle::Italic,
                FontStyle::Oblique => FaceStyle::Oblique,
            },
            stretch: font.stretch,
        };
        self.select(families, &query)
            .map(|face| face.metrics().em_ratios())
    }
}

/// The initial font: the initial `serif` family at weight 400 with normal
/// style and stretch. It is the root element's parent font and the font
/// media queries resolve `ex`, `ch`, `cap` and `ic` against.
pub(crate) const INITIAL_FONT: FontQuery<'static> = FontQuery {
    families: &[FamilyName::Generic(GenericFamily::Serif)],
    weight: 400.0,
    style: FontStyle::Normal,
    stretch: 100.0,
};

/// `ratios` in the form media and container queries take.
pub(crate) fn unit_ratios(ratios: EmRatios) -> FontUnitRatios {
    FontUnitRatios {
        ex: ratios.x_height,
        ch: ratios.zero_advance,
        cap: ratios.cap_height,
        ic: ratios.ideograph_advance,
    }
}
