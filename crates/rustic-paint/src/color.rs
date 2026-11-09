//! CSS Color 4 conversion of computed colors to 8-bit sRGB for painting.
//!
//! Out-of-gamut results are clipped per channel; CSS Color 4 gamut mapping
//! (OKLCh chroma reduction) is later work (TD-41). `none` channels are zero.

use rustic_css::{Color, LabSpace, PredefinedSpace};

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

type Matrix = [[f64; 3]; 3];

fn mul(m: &Matrix, v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

// Matrices from CSS Color Level 4 §18 (sample code).
const D50_TO_D65: Matrix = [
    [
        0.955_473_421_488_075,
        -0.023_098_454_948_764_71,
        0.063_259_243_200_570_72,
    ],
    [
        -0.028_369_709_333_863_7,
        1.009_995_398_081_304_1,
        0.021_041_441_191_917_323,
    ],
    [
        0.012_314_014_864_481_998,
        -0.020_507_649_298_898_964,
        1.330_365_926_242_124,
    ],
];
const XYZ_D65_TO_LINEAR_SRGB: Matrix = [
    [
        3.240_969_941_904_522_6,
        -1.537_383_177_570_094,
        -0.498_610_760_293_003_4,
    ],
    [
        -0.969_243_636_280_879_6,
        1.875_967_501_507_720_2,
        0.041_555_057_407_175_59,
    ],
    [
        0.055_630_079_696_993_66,
        -0.203_976_958_888_976_52,
        1.056_971_514_242_878_6,
    ],
];
const LINEAR_P3_TO_XYZ: Matrix = [
    [
        0.486_570_948_648_216_2,
        0.265_667_693_169_093_06,
        0.198_217_285_234_362_5,
    ],
    [
        0.228_974_564_069_748_8,
        0.691_738_521_836_506_4,
        0.079_286_914_093_745,
    ],
    [0.0, 0.045_113_381_858_902_64, 1.043_944_368_900_976],
];
const LINEAR_A98_TO_XYZ: Matrix = [
    [
        0.576_669_042_910_130_5,
        0.185_558_237_906_546_3,
        0.188_228_646_234_994_7,
    ],
    [
        0.297_344_975_250_536_05,
        0.627_363_566_255_466_1,
        0.075_291_458_493_997_88,
    ],
    [
        0.027_031_361_386_412_34,
        0.070_688_852_535_827_23,
        0.991_337_536_837_638_8,
    ],
];
const LINEAR_PROPHOTO_TO_XYZ_D50: Matrix = [
    [
        0.797_760_489_672_302_7,
        0.135_185_837_175_740_31,
        0.031_349_349_581_524_8,
    ],
    [
        0.288_071_128_229_293_4,
        0.711_843_217_810_101_4,
        0.000_085_653_960_605_259_02,
    ],
    [0.0, 0.0, 0.825_104_602_510_460_1],
];
const LINEAR_REC2020_TO_XYZ: Matrix = [
    [
        0.636_958_048_301_291_4,
        0.144_616_903_586_208_32,
        0.168_880_975_164_172_1,
    ],
    [
        0.262_700_212_011_267_1,
        0.677_998_071_518_870_8,
        0.059_301_716_469_861_96,
    ],
    [0.0, 0.028_072_693_049_087_428, 1.060_985_057_710_791],
];

fn srgb_decode(c: f64) -> f64 {
    let a = c.abs();
    let v = if a <= 0.04045 {
        a / 12.92
    } else {
        ((a + 0.055) / 1.055).powf(2.4)
    };
    v.copysign(c)
}

fn srgb_encode(c: f64) -> f64 {
    let a = c.abs();
    let v = if a <= 0.003_130_8 {
        a * 12.92
    } else {
        1.055 * a.powf(1.0 / 2.4) - 0.055
    };
    v.copysign(c)
}

fn lab_to_xyz_d50(l: f64, a: f64, b: f64) -> [f64; 3] {
    const KAPPA: f64 = 24_389.0 / 27.0;
    const EPSILON: f64 = 216.0 / 24_389.0;
    let f1 = (l + 16.0) / 116.0;
    let f0 = a / 500.0 + f1;
    let f2 = f1 - b / 200.0;
    let x = if f0 * f0 * f0 > EPSILON {
        f0 * f0 * f0
    } else {
        (116.0 * f0 - 16.0) / KAPPA
    };
    let y = if l > KAPPA * EPSILON {
        f1 * f1 * f1
    } else {
        l / KAPPA
    };
    let z = if f2 * f2 * f2 > EPSILON {
        f2 * f2 * f2
    } else {
        (116.0 * f2 - 16.0) / KAPPA
    };
    [
        x * (0.3457 / 0.3585),
        y,
        z * ((1.0 - 0.3457 - 0.3585) / 0.3585),
    ]
}

fn oklab_to_linear_srgb(l: f64, a: f64, b: f64) -> [f64; 3] {
    let l_ = l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
    let m_ = l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
    let s_ = l - 0.089_484_177_5 * a - 1.291_485_548 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    [
        4.076_741_662_1 * l3 - 3.307_711_591_3 * m3 + 0.230_969_929_2 * s3,
        -1.268_438_004_6 * l3 + 2.609_757_401_1 * m3 - 0.341_319_396_5 * s3,
        -0.004_196_086_3 * l3 - 0.703_418_614_7 * m3 + 1.707_614_701 * s3,
    ]
}

fn polar(lightness: f64, chroma: f64, hue_degrees: f64) -> [f64; 3] {
    let h = hue_degrees.to_radians();
    [lightness, chroma * h.cos(), chroma * h.sin()]
}

fn xyz_d65_to_srgb(xyz: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = mul(&XYZ_D65_TO_LINEAR_SRGB, xyz);
    [srgb_encode(r), srgb_encode(g), srgb_encode(b)]
}

fn predefined_to_srgb(space: PredefinedSpace, c: [f64; 3]) -> [f64; 3] {
    match space {
        PredefinedSpace::Srgb => c,
        PredefinedSpace::SrgbLinear => c.map(srgb_encode),
        PredefinedSpace::DisplayP3 => xyz_d65_to_srgb(mul(&LINEAR_P3_TO_XYZ, c.map(srgb_decode))),
        PredefinedSpace::A98Rgb => {
            let linear = c.map(|v| v.abs().powf(563.0 / 256.0).copysign(v));
            xyz_d65_to_srgb(mul(&LINEAR_A98_TO_XYZ, linear))
        }
        PredefinedSpace::ProphotoRgb => {
            let linear = c.map(|v| {
                let a = v.abs();
                let out = if a <= 16.0 / 512.0 {
                    a / 16.0
                } else {
                    a.powf(1.8)
                };
                out.copysign(v)
            });
            let d50 = mul(&LINEAR_PROPHOTO_TO_XYZ_D50, linear);
            xyz_d65_to_srgb(mul(&D50_TO_D65, d50))
        }
        PredefinedSpace::Rec2020 => {
            const ALPHA: f64 = 1.099_296_826_809_44;
            const BETA: f64 = 0.018_053_968_510_807;
            let linear = c.map(|v| {
                let a = v.abs();
                let out = if a < BETA * 4.5 {
                    a / 4.5
                } else {
                    ((a + ALPHA - 1.0) / ALPHA).powf(1.0 / 0.45)
                };
                out.copysign(v)
            });
            xyz_d65_to_srgb(mul(&LINEAR_REC2020_TO_XYZ, linear))
        }
        PredefinedSpace::XyzD50 => xyz_d65_to_srgb(mul(&D50_TO_D65, c)),
        PredefinedSpace::XyzD65 => xyz_d65_to_srgb(c),
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
    let (rgb, alpha) = match *color {
        Color::CurrentColor => return None,
        Color::Rgb {
            red,
            green,
            blue,
            alpha,
        } => ([red / 255.0, green / 255.0, blue / 255.0], alpha),
        Color::Lab {
            space,
            channels,
            alpha,
        } => {
            let [c0, c1, c2] = channels.map(|c| c.unwrap_or(0.0));
            let rgb = match space {
                LabSpace::Lab => xyz_d65_to_srgb(mul(&D50_TO_D65, lab_to_xyz_d50(c0, c1, c2))),
                LabSpace::Lch => {
                    let [l, a, b] = polar(c0, c1, c2);
                    xyz_d65_to_srgb(mul(&D50_TO_D65, lab_to_xyz_d50(l, a, b)))
                }
                LabSpace::Oklab => oklab_to_linear_srgb(c0, c1, c2).map(srgb_encode),
                LabSpace::Oklch => {
                    let [l, a, b] = polar(c0, c1, c2);
                    oklab_to_linear_srgb(l, a, b).map(srgb_encode)
                }
            };
            (rgb, alpha.unwrap_or(0.0))
        }
        Color::Predefined {
            space,
            channels,
            alpha,
        } => (
            predefined_to_srgb(space, channels.map(|c| c.unwrap_or(0.0))),
            alpha.unwrap_or(0.0),
        ),
    };
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
