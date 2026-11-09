//! Font metrics in font units and the em ratios CSS font-relative units use.

/// Metrics read from a face's tables, in font units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FontMetrics {
    /// Font units per em.
    pub units_per_em: u16,
    /// Ascent above the baseline (`hhea`, or OS/2 typo metrics when
    /// `USE_TYPO_METRICS` is set).
    pub ascent: i16,
    /// Descent, usually negative.
    pub descent: i16,
    /// Recommended extra line gap.
    pub line_gap: i16,
    /// x-height: OS/2 `sxHeight`, else the `x` glyph's top. `None` when
    /// neither is available or positive.
    pub x_height: Option<i16>,
    /// Cap height: OS/2 `sCapHeight`, else the `H` glyph's top.
    pub cap_height: Option<i16>,
    /// Advance of `0` (U+0030), for `ch`.
    pub zero_advance: Option<u16>,
    /// Advance of 水 (U+6C34), for `ic`.
    pub ideograph_advance: Option<u16>,
}

/// The font-relative unit sizes CSS needs, as fractions of the font size
/// (1.0 = 1em). Always finite and nonnegative.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EmRatios {
    /// `ex`: the x-height.
    pub x_height: f64,
    /// `cap`: the cap height.
    pub cap_height: f64,
    /// `ch`: the advance of `0`.
    pub zero_advance: f64,
    /// `ic`: the advance of 水.
    pub ideograph_advance: f64,
}

impl EmRatios {
    /// Ratios without any font: CSS Values 4 §6.1.1 fallbacks of 0.5em for
    /// `ex` and `ch` and 1em for `ic`. With no font there is no ascent to
    /// fall back to for `cap`, so it keeps Rustic's earlier 0.7em.
    pub const FALLBACK: Self = Self {
        x_height: 0.5,
        cap_height: 0.7,
        zero_advance: 0.5,
        ideograph_advance: 1.0,
    };
}

impl FontMetrics {
    /// The em ratios for CSS units, with CSS Values 4 §6.1.1 fallbacks for
    /// missing values: 0.5em for `ex` and `ch`, 1em for `ic`, and the font's
    /// ascent for `cap`.
    pub fn em_ratios(&self) -> EmRatios {
        if self.units_per_em == 0 {
            return EmRatios::FALLBACK;
        }
        let em = f64::from(self.units_per_em);
        let ratio = |v: f64| v / em;
        EmRatios {
            x_height: self.x_height.map_or(0.5, |v| ratio(f64::from(v))),
            cap_height: self
                .cap_height
                .map_or(ratio(f64::from(self.ascent.max(0))), |v| {
                    ratio(f64::from(v))
                }),
            zero_advance: self.zero_advance.map_or(0.5, |v| ratio(f64::from(v))),
            ideograph_advance: self.ideograph_advance.map_or(1.0, |v| ratio(f64::from(v))),
        }
    }
}
