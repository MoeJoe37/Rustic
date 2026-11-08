//! `<color>` values (CSS Color Level 4, the CSS-03 subset).
//!
//! Supported: named colors, `transparent`, `currentcolor`, hex colors (3, 4, 6
//! and 8 digits), `rgb()`/`rgba()` and `hsl()`/`hsla()` in legacy (comma) and
//! modern (space and `/ alpha`) syntax, `hwb()`, `lab()`, `lch()`, `oklab()`,
//! `oklch()` and `color()` with the predefined color spaces. `none` is
//! accepted where the modern syntax allows it. Math functions such as `calc()`,
//! relative colors, `color-mix()`, `light-dark()` and system colors are not
//! implemented yet and make the value invalid.
//!
//! sRGB forms (named, hex, `rgb`, `hsl`, `hwb`) are converted to sRGB channels
//! at parse time and serialize as `rgb()`/`rgba()`, as CSSOM specifies; the
//! other forms keep their own space and serialize as written, normalized.

use std::fmt::{self, Write};

use crate::{ComponentValue, Token, Value};

/// A parsed `<color>`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Color {
    /// `currentcolor`, resolved later against the `color` property.
    CurrentColor,
    /// An sRGB color: channels in `0..=255` (not rounded), alpha in `0..=1`.
    Rgb {
        /// Red.
        red: f64,
        /// Green.
        green: f64,
        /// Blue.
        blue: f64,
        /// Alpha.
        alpha: f64,
    },
    /// `lab()`, `lch()`, `oklab()` or `oklch()`: three channels (`None` for
    /// `none`), in the function's reference ranges, and alpha.
    Lab {
        /// Which function.
        space: LabSpace,
        /// Lightness, then a/chroma, then b/hue (degrees).
        channels: [Option<f64>; 3],
        /// Alpha in `0..=1`, `None` for `none`.
        alpha: Option<f64>,
    },
    /// `color(<space> c1 c2 c3 / alpha)`.
    Predefined {
        /// The predefined color space.
        space: PredefinedSpace,
        /// Channels as numbers (`100%` is `1`), `None` for `none`.
        channels: [Option<f64>; 3],
        /// Alpha in `0..=1`, `None` for `none`.
        alpha: Option<f64>,
    },
}

/// The CIE and Oklab color functions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LabSpace {
    /// `lab()`.
    Lab,
    /// `lch()`.
    Lch,
    /// `oklab()`.
    Oklab,
    /// `oklch()`.
    Oklch,
}

/// Predefined color spaces for `color()`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PredefinedSpace {
    /// `srgb`.
    Srgb,
    /// `srgb-linear`.
    SrgbLinear,
    /// `display-p3`.
    DisplayP3,
    /// `a98-rgb`.
    A98Rgb,
    /// `prophoto-rgb`.
    ProphotoRgb,
    /// `rec2020`.
    Rec2020,
    /// `xyz-d50`.
    XyzD50,
    /// `xyz-d65` (also written `xyz`).
    XyzD65,
}

impl PredefinedSpace {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "srgb" => Self::Srgb,
            "srgb-linear" => Self::SrgbLinear,
            "display-p3" => Self::DisplayP3,
            "a98-rgb" => Self::A98Rgb,
            "prophoto-rgb" => Self::ProphotoRgb,
            "rec2020" => Self::Rec2020,
            "xyz-d50" => Self::XyzD50,
            "xyz" | "xyz-d65" => Self::XyzD65,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Srgb => "srgb",
            Self::SrgbLinear => "srgb-linear",
            Self::DisplayP3 => "display-p3",
            Self::A98Rgb => "a98-rgb",
            Self::ProphotoRgb => "prophoto-rgb",
            Self::Rec2020 => "rec2020",
            Self::XyzD50 => "xyz-d50",
            Self::XyzD65 => "xyz-d65",
        }
    }
}

/// The CSS named colors (CSS Color Level 4 §6.1), sorted for binary search.
const NAMED_COLORS: &[(&str, [u8; 3])] = &[
    ("aliceblue", [240, 248, 255]),
    ("antiquewhite", [250, 235, 215]),
    ("aqua", [0, 255, 255]),
    ("aquamarine", [127, 255, 212]),
    ("azure", [240, 255, 255]),
    ("beige", [245, 245, 220]),
    ("bisque", [255, 228, 196]),
    ("black", [0, 0, 0]),
    ("blanchedalmond", [255, 235, 205]),
    ("blue", [0, 0, 255]),
    ("blueviolet", [138, 43, 226]),
    ("brown", [165, 42, 42]),
    ("burlywood", [222, 184, 135]),
    ("cadetblue", [95, 158, 160]),
    ("chartreuse", [127, 255, 0]),
    ("chocolate", [210, 105, 30]),
    ("coral", [255, 127, 80]),
    ("cornflowerblue", [100, 149, 237]),
    ("cornsilk", [255, 248, 220]),
    ("crimson", [220, 20, 60]),
    ("cyan", [0, 255, 255]),
    ("darkblue", [0, 0, 139]),
    ("darkcyan", [0, 139, 139]),
    ("darkgoldenrod", [184, 134, 11]),
    ("darkgray", [169, 169, 169]),
    ("darkgreen", [0, 100, 0]),
    ("darkgrey", [169, 169, 169]),
    ("darkkhaki", [189, 183, 107]),
    ("darkmagenta", [139, 0, 139]),
    ("darkolivegreen", [85, 107, 47]),
    ("darkorange", [255, 140, 0]),
    ("darkorchid", [153, 50, 204]),
    ("darkred", [139, 0, 0]),
    ("darksalmon", [233, 150, 122]),
    ("darkseagreen", [143, 188, 143]),
    ("darkslateblue", [72, 61, 139]),
    ("darkslategray", [47, 79, 79]),
    ("darkslategrey", [47, 79, 79]),
    ("darkturquoise", [0, 206, 209]),
    ("darkviolet", [148, 0, 211]),
    ("deeppink", [255, 20, 147]),
    ("deepskyblue", [0, 191, 255]),
    ("dimgray", [105, 105, 105]),
    ("dimgrey", [105, 105, 105]),
    ("dodgerblue", [30, 144, 255]),
    ("firebrick", [178, 34, 34]),
    ("floralwhite", [255, 250, 240]),
    ("forestgreen", [34, 139, 34]),
    ("fuchsia", [255, 0, 255]),
    ("gainsboro", [220, 220, 220]),
    ("ghostwhite", [248, 248, 255]),
    ("gold", [255, 215, 0]),
    ("goldenrod", [218, 165, 32]),
    ("gray", [128, 128, 128]),
    ("green", [0, 128, 0]),
    ("greenyellow", [173, 255, 47]),
    ("grey", [128, 128, 128]),
    ("honeydew", [240, 255, 240]),
    ("hotpink", [255, 105, 180]),
    ("indianred", [205, 92, 92]),
    ("indigo", [75, 0, 130]),
    ("ivory", [255, 255, 240]),
    ("khaki", [240, 230, 140]),
    ("lavender", [230, 230, 250]),
    ("lavenderblush", [255, 240, 245]),
    ("lawngreen", [124, 252, 0]),
    ("lemonchiffon", [255, 250, 205]),
    ("lightblue", [173, 216, 230]),
    ("lightcoral", [240, 128, 128]),
    ("lightcyan", [224, 255, 255]),
    ("lightgoldenrodyellow", [250, 250, 210]),
    ("lightgray", [211, 211, 211]),
    ("lightgreen", [144, 238, 144]),
    ("lightgrey", [211, 211, 211]),
    ("lightpink", [255, 182, 193]),
    ("lightsalmon", [255, 160, 122]),
    ("lightseagreen", [32, 178, 170]),
    ("lightskyblue", [135, 206, 250]),
    ("lightslategray", [119, 136, 153]),
    ("lightslategrey", [119, 136, 153]),
    ("lightsteelblue", [176, 196, 222]),
    ("lightyellow", [255, 255, 224]),
    ("lime", [0, 255, 0]),
    ("limegreen", [50, 205, 50]),
    ("linen", [250, 240, 230]),
    ("magenta", [255, 0, 255]),
    ("maroon", [128, 0, 0]),
    ("mediumaquamarine", [102, 205, 170]),
    ("mediumblue", [0, 0, 205]),
    ("mediumorchid", [186, 85, 211]),
    ("mediumpurple", [147, 112, 219]),
    ("mediumseagreen", [60, 179, 113]),
    ("mediumslateblue", [123, 104, 238]),
    ("mediumspringgreen", [0, 250, 154]),
    ("mediumturquoise", [72, 209, 204]),
    ("mediumvioletred", [199, 21, 133]),
    ("midnightblue", [25, 25, 112]),
    ("mintcream", [245, 255, 250]),
    ("mistyrose", [255, 228, 225]),
    ("moccasin", [255, 228, 181]),
    ("navajowhite", [255, 222, 173]),
    ("navy", [0, 0, 128]),
    ("oldlace", [253, 245, 230]),
    ("olive", [128, 128, 0]),
    ("olivedrab", [107, 142, 35]),
    ("orange", [255, 165, 0]),
    ("orangered", [255, 69, 0]),
    ("orchid", [218, 112, 214]),
    ("palegoldenrod", [238, 232, 170]),
    ("palegreen", [152, 251, 152]),
    ("paleturquoise", [175, 238, 238]),
    ("palevioletred", [219, 112, 147]),
    ("papayawhip", [255, 239, 213]),
    ("peachpuff", [255, 218, 185]),
    ("peru", [205, 133, 63]),
    ("pink", [255, 192, 203]),
    ("plum", [221, 160, 221]),
    ("powderblue", [176, 224, 230]),
    ("purple", [128, 0, 128]),
    ("rebeccapurple", [102, 51, 153]),
    ("red", [255, 0, 0]),
    ("rosybrown", [188, 143, 143]),
    ("royalblue", [65, 105, 225]),
    ("saddlebrown", [139, 69, 19]),
    ("salmon", [250, 128, 114]),
    ("sandybrown", [244, 164, 96]),
    ("seagreen", [46, 139, 87]),
    ("seashell", [255, 245, 238]),
    ("sienna", [160, 82, 45]),
    ("silver", [192, 192, 192]),
    ("skyblue", [135, 206, 235]),
    ("slateblue", [106, 90, 205]),
    ("slategray", [112, 128, 144]),
    ("slategrey", [112, 128, 144]),
    ("snow", [255, 250, 250]),
    ("springgreen", [0, 255, 127]),
    ("steelblue", [70, 130, 180]),
    ("tan", [210, 180, 140]),
    ("teal", [0, 128, 128]),
    ("thistle", [216, 191, 216]),
    ("tomato", [255, 99, 71]),
    ("turquoise", [64, 224, 208]),
    ("violet", [238, 130, 238]),
    ("wheat", [245, 222, 179]),
    ("white", [255, 255, 255]),
    ("whitesmoke", [245, 245, 245]),
    ("yellow", [255, 255, 0]),
    ("yellowgreen", [154, 205, 50]),
];

impl Color {
    /// Opaque sRGB from 8-bit channels.
    pub fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self::Rgb {
            red: red.into(),
            green: green.into(),
            blue: blue.into(),
            alpha: 1.0,
        }
    }

    /// A named color or `transparent`, ASCII case-insensitively.
    pub fn named(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        if lower == "transparent" {
            return Some(Self::Rgb {
                red: 0.0,
                green: 0.0,
                blue: 0.0,
                alpha: 0.0,
            });
        }
        let index = NAMED_COLORS
            .binary_search_by(|(n, _)| (*n).cmp(&lower))
            .ok()?;
        let [r, g, b] = NAMED_COLORS[index].1;
        Some(Self::rgb(r, g, b))
    }

    /// Parse one component value as a `<color>`.
    ///
    /// ```
    /// use rustic_css::{Color, CssParser};
    /// let value = CssParser::default().component_value("hsl(120 100% 25% / 50%)").unwrap();
    /// let color = Color::parse(&value.value.unwrap()).unwrap();
    /// assert_eq!(color.to_string(), "rgba(0, 127.5, 0, 0.5)");
    /// ```
    pub fn parse(value: &Value) -> Option<Self> {
        match &value.value {
            ComponentValue::Token(Token::Ident(name)) => {
                if name.eq_ignore_ascii_case("currentcolor") {
                    Some(Self::CurrentColor)
                } else {
                    Self::named(name)
                }
            }
            ComponentValue::Token(Token::Hash { value, .. }) => hex(value),
            ComponentValue::Function { name, arguments } => {
                function(&name.to_ascii_lowercase(), arguments)
            }
            _ => None,
        }
    }
}

fn hex(digits: &str) -> Option<Color> {
    let nibbles: Vec<u8> = digits
        .chars()
        .map(|c| c.to_digit(16).and_then(|d| u8::try_from(d).ok()))
        .collect::<Option<_>>()?;
    let channel = |i: usize, wide: bool| -> f64 {
        if wide {
            f64::from(nibbles[2 * i] * 16 + nibbles[2 * i + 1])
        } else {
            f64::from(nibbles[i] * 17)
        }
    };
    let (wide, has_alpha) = match nibbles.len() {
        3 => (false, false),
        4 => (false, true),
        6 => (true, false),
        8 => (true, true),
        _ => return None,
    };
    Some(Color::Rgb {
        red: channel(0, wide),
        green: channel(1, wide),
        blue: channel(2, wide),
        alpha: if has_alpha {
            channel(3, wide) / 255.0
        } else {
            1.0
        },
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Item {
    Number(f64),
    Percent(f64),
    /// Degrees.
    Angle(f64),
    None,
}

fn item(value: &Value) -> Option<Item> {
    Some(match value.token()? {
        Token::Number(n) => Item::Number(n.value),
        Token::Percentage(n) => Item::Percent(n.value),
        Token::Dimension(n, unit) => {
            let per_unit = match unit.to_ascii_lowercase().as_str() {
                "deg" => 1.0,
                "grad" => 0.9,
                "rad" => 180.0 / std::f64::consts::PI,
                "turn" => 360.0,
                _ => return None,
            };
            Item::Angle(n.value * per_unit)
        }
        Token::Ident(ident) if ident.eq_ignore_ascii_case("none") => Item::None,
        _ => return None,
    })
}

/// Function arguments: three channels, optional alpha, and whether the
/// legacy comma syntax was used.
struct Arguments {
    channels: [Item; 3],
    alpha: Option<Item>,
    legacy: bool,
}

fn arguments(values: &[Value]) -> Option<Arguments> {
    let parts: Vec<&Value> = values.iter().filter(|v| !v.is_whitespace()).collect();
    let legacy = parts.iter().any(|v| v.token() == Some(&Token::Comma));
    let mut items = Vec::new();
    let mut alpha = None;
    if legacy {
        // item (, item){2,3}
        for (index, part) in parts.iter().enumerate() {
            if index % 2 == 1 {
                if part.token() != Some(&Token::Comma) {
                    return None;
                }
            } else {
                items.push(item(part)?);
            }
        }
        if parts.len() % 2 == 0 {
            return None; // Trailing comma.
        }
        if items.len() == 4 {
            alpha = items.pop();
        }
    } else {
        let slash = parts
            .iter()
            .position(|v| v.token() == Some(&Token::Delim('/')));
        let (channels, rest) = match slash {
            Some(i) => (&parts[..i], Some(&parts[i + 1..])),
            None => (&parts[..], None),
        };
        for part in channels {
            items.push(item(part)?);
        }
        if let Some(rest) = rest {
            let [only] = rest else {
                return None;
            };
            alpha = Some(item(only)?);
        }
    }
    let channels: [Item; 3] = items.try_into().ok()?;
    if legacy && (channels.contains(&Item::None) || alpha == Some(Item::None)) {
        return None;
    }
    Some(Arguments {
        channels,
        alpha,
        legacy,
    })
}

/// `<alpha-value> | none`, clamped to `0..=1`; `None` means `none`.
fn alpha(item: Option<Item>) -> Option<Option<f64>> {
    Some(match item {
        None => Some(1.0),
        Some(Item::Number(n)) => Some(n.clamp(0.0, 1.0)),
        Some(Item::Percent(p)) => Some((p / 100.0).clamp(0.0, 1.0)),
        Some(Item::None) => None,
        Some(Item::Angle(_)) => return None,
    })
}

/// A number or percentage channel, where `100%` maps to `full`.
fn channel(item: Item, full: f64) -> Option<Option<f64>> {
    match item {
        Item::Number(n) => Some(Some(n)),
        Item::Percent(p) => Some(Some(p / 100.0 * full)),
        Item::None => Some(None),
        Item::Angle(_) => None,
    }
}

/// A hue: a number (degrees) or angle.
fn hue(item: Item) -> Option<Option<f64>> {
    match item {
        Item::Number(n) | Item::Angle(n) => Some(Some(n)),
        Item::None => Some(None),
        Item::Percent(_) => None,
    }
}

fn srgb(red: f64, green: f64, blue: f64, alpha: Option<f64>) -> Color {
    Color::Rgb {
        red: red.clamp(0.0, 255.0),
        green: green.clamp(0.0, 255.0),
        blue: blue.clamp(0.0, 255.0),
        alpha: alpha.unwrap_or(0.0),
    }
}

/// CSS Color 4 §7.1 `hslToRgb`, returning channels in `0..=1`.
fn hsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> [f64; 3] {
    let hue = hue.rem_euclid(360.0);
    let f = |n: f64| {
        let k = (n + hue / 30.0) % 12.0;
        let a = saturation * lightness.min(1.0 - lightness);
        lightness - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    [f(0.0), f(8.0), f(4.0)]
}

fn function(name: &str, values: &[Value]) -> Option<Color> {
    if name == "color" {
        return predefined(values);
    }
    let Arguments {
        channels: [c0, c1, c2],
        alpha: alpha_item,
        legacy,
    } = arguments(values)?;
    let alpha = alpha(alpha_item)?;
    match name {
        "rgb" | "rgba" => {
            if legacy {
                let numbers = [c0, c1, c2].iter().all(|c| matches!(c, Item::Number(_)));
                let percents = [c0, c1, c2].iter().all(|c| matches!(c, Item::Percent(_)));
                if !numbers && !percents {
                    return None;
                }
            }
            let [r, g, b] = [
                channel(c0, 255.0)?,
                channel(c1, 255.0)?,
                channel(c2, 255.0)?,
            ];
            Some(srgb(
                r.unwrap_or(0.0),
                g.unwrap_or(0.0),
                b.unwrap_or(0.0),
                alpha,
            ))
        }
        "hsl" | "hsla" => {
            if legacy && !(matches!(c1, Item::Percent(_)) && matches!(c2, Item::Percent(_))) {
                return None;
            }
            let h = hue(c0)?.unwrap_or(0.0);
            let s = channel(c1, 100.0)?.unwrap_or(0.0).max(0.0) / 100.0;
            let l = channel(c2, 100.0)?.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0;
            let [r, g, b] = hsl_to_rgb(h, s, l);
            Some(srgb(r * 255.0, g * 255.0, b * 255.0, alpha))
        }
        "hwb" if !legacy => {
            let h = hue(c0)?.unwrap_or(0.0);
            let w = channel(c1, 100.0)?.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0;
            let b = channel(c2, 100.0)?.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0;
            if w + b >= 1.0 {
                let gray = w / (w + b) * 255.0;
                return Some(srgb(gray, gray, gray, alpha));
            }
            let pure = hsl_to_rgb(h, 1.0, 0.5);
            let scale = 1.0 - w - b;
            let [r, g, bl] = pure.map(|c| (c * scale + w) * 255.0);
            Some(srgb(r, g, bl, alpha))
        }
        "lab" | "lch" | "oklab" | "oklch" if !legacy => {
            let (space, l_full, ab_full) = match name {
                "lab" => (LabSpace::Lab, 100.0, 125.0),
                "lch" => (LabSpace::Lch, 100.0, 150.0),
                "oklab" => (LabSpace::Oklab, 1.0, 0.4),
                _ => (LabSpace::Oklch, 1.0, 0.4),
            };
            let polar = matches!(space, LabSpace::Lch | LabSpace::Oklch);
            let l = channel(c0, l_full)?;
            let ab = channel(c1, ab_full)?;
            let third = if polar {
                hue(c2)?.map(|h| h.rem_euclid(360.0))
            } else {
                channel(c2, ab_full)?
            };
            Some(Color::Lab {
                space,
                channels: [l, ab, third],
                alpha,
            })
        }
        _ => None,
    }
}

fn predefined(values: &[Value]) -> Option<Color> {
    let first = values.iter().position(|v| !v.is_whitespace())?;
    let Some(Token::Ident(name)) = values[first].token() else {
        return None;
    };
    let space = PredefinedSpace::from_name(name)?;
    let Arguments {
        channels,
        alpha: alpha_item,
        legacy,
    } = arguments(&values[first + 1..])?;
    if legacy || channels.iter().any(|c| matches!(c, Item::Angle(_))) {
        return None;
    }
    let [c0, c1, c2] = channels;
    Some(Color::Predefined {
        space,
        channels: [channel(c0, 1.0)?, channel(c1, 1.0)?, channel(c2, 1.0)?],
        alpha: alpha(alpha_item)?,
    })
}

/// Write a number with at most six decimals, without trailing zeros or `-0`.
pub(crate) fn write_number(out: &mut impl Write, value: f64) -> fmt::Result {
    let text = format!("{value:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" || text.is_empty() {
        out.write_char('0')
    } else {
        out.write_str(text)
    }
}

fn write_channel(out: &mut impl Write, value: Option<f64>) -> fmt::Result {
    match value {
        Some(v) => write_number(out, v),
        None => out.write_str("none"),
    }
}

fn write_alpha(out: &mut impl Write, alpha: Option<f64>) -> fmt::Result {
    if alpha != Some(1.0) {
        out.write_str(" / ")?;
        write_channel(out, alpha)?;
    }
    Ok(())
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::CurrentColor => f.write_str("currentcolor"),
            Self::Rgb {
                red,
                green,
                blue,
                alpha,
            } => {
                f.write_str(if alpha == 1.0 { "rgb(" } else { "rgba(" })?;
                for (index, c) in [red, green, blue].into_iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write_number(f, c)?;
                }
                if alpha != 1.0 {
                    f.write_str(", ")?;
                    write_number(f, alpha)?;
                }
                f.write_char(')')
            }
            Self::Lab {
                space,
                channels,
                alpha,
            } => {
                f.write_str(match space {
                    LabSpace::Lab => "lab(",
                    LabSpace::Lch => "lch(",
                    LabSpace::Oklab => "oklab(",
                    LabSpace::Oklch => "oklch(",
                })?;
                for (index, c) in channels.into_iter().enumerate() {
                    if index > 0 {
                        f.write_char(' ')?;
                    }
                    write_channel(f, c)?;
                }
                write_alpha(f, alpha)?;
                f.write_char(')')
            }
            Self::Predefined {
                space,
                channels,
                alpha,
            } => {
                write!(f, "color({}", space.name())?;
                for c in channels {
                    f.write_char(' ')?;
                    write_channel(f, c)?;
                }
                write_alpha(f, alpha)?;
                f.write_char(')')
            }
        }
    }
}
