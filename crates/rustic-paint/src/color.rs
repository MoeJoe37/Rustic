//! CSS Color 4 conversion of computed colors to 8-bit sRGB for painting.
//!
//! The conversions live in `rustic-css` (`Color::to_srgb`). Out-of-gamut results are clipped per channel; CSS Color 4 gamut mapping
//! (OKLCh chroma reduction) is later work (TD-41). `none` channels are zero.

use rustic_css::Color;
#[cfg(test)]
use rustic_css::{LabSpace, PredefinedSpace};

/// An 8-bit sRGB color with straight (not premultiplied) alpha.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Srgba {
    /// Red, 0–255.
    pub r: u8,
    /// Green, 0–255.
    pub g: u8,
    /// Blue, 0–255.
    pub b: u8,
    /// Alpha, 0 (transparent) to 255 (opaque).
    pub a: u8,
}

impl Srgba {
    /// Opaque white, the initial canvas color.
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };

    /// Whether painting this color changes nothing.
    pub const fn is_transparent(self) -> bool {
        self.a == 0
    }
}

fn byte(unit: f64) -> u8 {
    // NaN (impossible from finite inputs, but cheap to rule out) maps to 0.
    let scaled = (unit * 255.0).round();
    if scaled.is_nan() {
        0
    } else {
        scaled.clamp(0.0, 255.0) as u8
    }
}

/// Convert a computed color to 8-bit sRGB, clipping out-of-gamut channels.
///
/// Returns `None` for `currentcolor`, which computed values never contain.
pub fn srgba(color: &Color) -> Option<Srgba> {
    let (rgb, alpha) = color.to_srgb()?;
    Some(Srgba {
        r: byte(rgb[0]),
        g: byte(rgb[1]),
        b: byte(rgb[2]),
        a: byte(alpha),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lab(space: LabSpace, l: f64, a: f64, b: f64) -> Srgba {
        srgba(&Color::Lab {
            space,
            channels: [Some(l), Some(a), Some(b)],
            alpha: Some(1.0),
        })
        .unwrap()
    }

    fn rgb(r: u8, g: u8, b: u8) -> Srgba {
        Srgba { r, g, b, a: 255 }
    }

    #[test]
    fn rgb_rounds_and_clips() {
        let c = Color::Rgb {
            red: 127.5,
            green: 300.0,
            blue: -4.0,
            alpha: 0.5,
        };
        assert_eq!(
            srgba(&c),
            Some(Srgba {
                r: 128,
                g: 255,
                b: 0,
                a: 128
            })
        );
        assert_eq!(srgba(&Color::CurrentColor), None);
    }

    #[test]
    fn reference_colors_match_css_color_4_examples() {
        // White and black are exact in every space.
        assert_eq!(lab(LabSpace::Lab, 100.0, 0.0, 0.0), rgb(255, 255, 255));
        assert_eq!(lab(LabSpace::Oklab, 1.0, 0.0, 0.0), rgb(255, 255, 255));
        assert_eq!(lab(LabSpace::Oklch, 0.0, 0.0, 0.0), rgb(0, 0, 0));
        // sRGB red expressed in other spaces (values from CSS Color 4).
        assert_eq!(
            lab(LabSpace::Lab, 54.290_5, 80.804_9, 69.890_7),
            rgb(255, 0, 0)
        );
        assert_eq!(
            lab(LabSpace::Oklch, 0.627_955, 0.257_683, 29.233_9),
            rgb(255, 0, 0)
        );
        assert_eq!(
            lab(LabSpace::Lch, 54.290_5, 106.837, 40.853_8),
            rgb(255, 0, 0)
        );
        let p3 = |c: [f64; 3]| {
            srgba(&Color::Predefined {
                space: PredefinedSpace::DisplayP3,
                channels: c.map(Some),
                alpha: None,
            })
            .unwrap()
        };
        // `none` alpha is zero; P3 red is out of sRGB gamut and clips.
        assert_eq!(
            p3([1.0, 0.0, 0.0]),
            Srgba {
                r: 255,
                g: 0,
                b: 0,
                a: 0
            }
        );
        // sRGB grey round-trips through every predefined space's white point.
        for space in [
            PredefinedSpace::Srgb,
            PredefinedSpace::SrgbLinear,
            PredefinedSpace::DisplayP3,
            PredefinedSpace::A98Rgb,
            PredefinedSpace::ProphotoRgb,
            PredefinedSpace::Rec2020,
        ] {
            let white = srgba(&Color::Predefined {
                space,
                channels: [Some(1.0); 3],
                alpha: Some(1.0),
            })
            .unwrap();
            assert_eq!(white, rgb(255, 255, 255), "{space:?}");
        }
    }
}
