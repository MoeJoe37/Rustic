//! `box-shadow` (CSS Backgrounds 3 §7), `filter` (Filter Effects 1 §5) and
//! `mix-blend-mode` and `isolation` (Compositing and Blending 1 §3).

use std::fmt;

use crate::properties::{ident, keywords, length, math_function};
use crate::{Calc, Color, ComponentValue, LengthPercentage, Token, Value};

/// One shadow of `box-shadow`, or the shadow of `drop-shadow()`.
#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    /// The color; `None` is `currentcolor`.
    pub color: Option<Color>,
    /// Horizontal offset (a length).
    pub x: LengthPercentage,
    /// Vertical offset (a length).
    pub y: LengthPercentage,
    /// Blur radius, nonnegative once computed.
    pub blur: Option<LengthPercentage>,
    /// Spread distance (not for `drop-shadow()`).
    pub spread: Option<LengthPercentage>,
    /// Whether the shadow is drawn inside the padding box.
    pub inset: bool,
}

impl Shadow {
    /// The same shadow with `f` applied to its offsets and spread and
    /// `blur` to its blur radius.
    pub fn map_lengths(
        &self,
        f: &dyn Fn(&LengthPercentage) -> LengthPercentage,
        blur: &dyn Fn(&LengthPercentage) -> LengthPercentage,
    ) -> Self {
        Self {
            color: self.color,
            x: f(&self.x),
            y: f(&self.y),
            blur: self.blur.as_ref().map(blur),
            spread: self.spread.as_ref().map(f),
            inset: self.inset,
        }
    }
}

impl fmt::Display for Shadow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(color) = &self.color {
            write!(f, "{color} ")?;
        }
        write!(f, "{} {}", self.x, self.y)?;
        if let Some(blur) = &self.blur {
            write!(f, " {blur}")?;
        }
        if let Some(spread) = &self.spread {
            write!(f, " {spread}")?;
        }
        if self.inset {
            f.write_str(" inset")?;
        }
        Ok(())
    }
}

/// A `<length>` (no percentage), including math functions resolving to one.
fn length_only(value: &Value) -> Option<LengthPercentage> {
    if math_function(value) {
        return Calc::parse_length(value).map(|c| LengthPercentage::Calc(Box::new(c)));
    }
    length(value).map(LengthPercentage::Length)
}

/// One shadow: a run of two to `max_lengths` lengths, with a color and (when
/// `allow_inset`) `inset` before or after it, each at most once.
fn shadow(parts: &[&Value], max_lengths: usize, allow_inset: bool) -> Option<Shadow> {
    let mut lengths = Vec::new();
    let mut lengths_done = false;
    let (mut color, mut inset) = (None, false);
    for part in parts {
        if let Some(l) = length_only(part) {
            if lengths_done {
                return None;
            }
            lengths.push(l);
            continue;
        }
        if !lengths.is_empty() {
            lengths_done = true;
        }
        if allow_inset && ident(part).is_some_and(|i| i.eq_ignore_ascii_case("inset")) {
            if inset {
                return None;
            }
            inset = true;
        } else if color.is_none() {
            color = Some(Color::parse(part)?);
        } else {
            return None;
        }
    }
    if lengths.len() < 2 || lengths.len() > max_lengths {
        return None;
    }
    let mut lengths = lengths.into_iter();
    let (x, y) = (lengths.next()?, lengths.next()?);
    let blur = lengths.next();
    // A negative blur radius is invalid; a math function clamps once it
    // resolves.
    if matches!(&blur, Some(LengthPercentage::Length(l)) if l.value < 0.0) {
        return None;
    }
    Some(Shadow {
        color,
        x,
        y,
        blur,
        spread: lengths.next(),
        inset,
    })
}

/// Parse `box-shadow`: `none` (an empty list) or comma-separated shadows.
pub(crate) fn box_shadow(parts: &[&Value]) -> Option<Vec<Shadow>> {
    if let [only] = parts {
        if ident(only).is_some_and(|i| i.eq_ignore_ascii_case("none")) {
            return Some(Vec::new());
        }
    }
    parts
        .split(|v| v.token() == Some(&Token::Comma))
        .map(|group| shadow(group, 4, true))
        .collect()
}

/// One `<filter-function>`.
#[derive(Clone, Debug, PartialEq)]
pub enum FilterFunction {
    /// `blur(<length>)`: the Gaussian standard deviation.
    Blur(LengthPercentage),
    /// `brightness(<amount>)`.
    Brightness(f64),
    /// `contrast(<amount>)`.
    Contrast(f64),
    /// `grayscale(<amount>)`.
    Grayscale(f64),
    /// `hue-rotate(<angle>)`, in degrees.
    HueRotate(f64),
    /// `invert(<amount>)`.
    Invert(f64),
    /// `opacity(<amount>)`.
    Opacity(f64),
    /// `saturate(<amount>)`.
    Saturate(f64),
    /// `sepia(<amount>)`.
    Sepia(f64),
    /// `drop-shadow(<color>? && <length>{2,3})`.
    DropShadow(Shadow),
    /// `url()`, a reference to an SVG filter.
    Url(String),
}

impl FilterFunction {
    /// The same function with `f` applied to every length (offsets) and
    /// `blur` to blur radii.
    pub fn map_lengths(
        &self,
        f: &dyn Fn(&LengthPercentage) -> LengthPercentage,
        blur: &dyn Fn(&LengthPercentage) -> LengthPercentage,
    ) -> Self {
        match self {
            Self::Blur(r) => Self::Blur(blur(r)),
            Self::DropShadow(s) => Self::DropShadow(s.map_lengths(f, blur)),
            other => other.clone(),
        }
    }
}

impl fmt::Display for FilterFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let amount = |f: &mut fmt::Formatter<'_>, name: &str, v: f64| write!(f, "{name}({v})");
        match self {
            Self::Blur(r) => write!(f, "blur({r})"),
            Self::Brightness(v) => amount(f, "brightness", *v),
            Self::Contrast(v) => amount(f, "contrast", *v),
            Self::Grayscale(v) => amount(f, "grayscale", *v),
            Self::HueRotate(v) => write!(f, "hue-rotate({v}deg)"),
            Self::Invert(v) => amount(f, "invert", *v),
            Self::Opacity(v) => amount(f, "opacity", *v),
            Self::Saturate(v) => amount(f, "saturate", *v),
            Self::Sepia(v) => amount(f, "sepia", *v),
            Self::DropShadow(s) => write!(f, "drop-shadow({s})"),
            Self::Url(url) => {
                f.write_str("url(")?;
                crate::serialize::serialize_string(f, url)?;
                f.write_str(")")
            }
        }
    }
}

/// A nonnegative `<number>` or `<percentage>` as a number.
fn amount(value: &Value) -> Option<f64> {
    let v = match value.token()? {
        Token::Number(n) => n.value,
        Token::Percentage(n) => n.value / 100.0,
        _ => return None,
    };
    (v >= 0.0 && v.is_finite()).then_some(v)
}

/// Parse one filter function.
fn filter_function(value: &Value) -> Option<FilterFunction> {
    if let Some(crate::background::Image::Url(url)) = crate::background::image(value) {
        return Some(FilterFunction::Url(url));
    }
    let ComponentValue::Function { name, arguments } = &value.value else {
        return None;
    };
    let args: Vec<&Value> = arguments.iter().filter(|v| !v.is_whitespace()).collect();
    let name = name.to_ascii_lowercase();
    if name == "drop-shadow" {
        return shadow(&args, 3, false).map(FilterFunction::DropShadow);
    }
    let one = match args.as_slice() {
        [] => None,
        [v] => Some(*v),
        _ => return None,
    };
    let number = |default: f64| match one {
        None => Some(default),
        Some(v) => amount(v),
    };
    Some(match name.as_str() {
        "blur" => FilterFunction::Blur(match one {
            None => LengthPercentage::Length(crate::Length::px(0.0)),
            Some(v) => {
                let l = length_only(v)?;
                if matches!(&l, LengthPercentage::Length(l) if l.value < 0.0) {
                    return None;
                }
                l
            }
        }),
        "brightness" => FilterFunction::Brightness(number(1.0)?),
        "contrast" => FilterFunction::Contrast(number(1.0)?),
        "grayscale" => FilterFunction::Grayscale(number(1.0)?),
        "hue-rotate" => FilterFunction::HueRotate(match one {
            None => 0.0,
            Some(v) => crate::transform::angle(v)?,
        }),
        "invert" => FilterFunction::Invert(number(1.0)?),
        "opacity" => FilterFunction::Opacity(number(1.0)?),
        "saturate" => FilterFunction::Saturate(number(1.0)?),
        "sepia" => FilterFunction::Sepia(number(1.0)?),
        _ => return None,
    })
}

/// Parse `filter`: `none` (an empty list) or a space-separated list.
pub(crate) fn filter(parts: &[&Value]) -> Option<Vec<FilterFunction>> {
    if let [only] = parts {
        if ident(only).is_some_and(|i| i.eq_ignore_ascii_case("none")) {
            return Some(Vec::new());
        }
    }
    if parts.is_empty() {
        return None;
    }
    parts.iter().map(|v| filter_function(v)).collect()
}

keywords! {
    /// A `<blend-mode>` of `mix-blend-mode` (Compositing and Blending 1 §5).
    BlendMode {
        Normal => "normal",
        Multiply => "multiply",
        Screen => "screen",
        Overlay => "overlay",
        Darken => "darken",
        Lighten => "lighten",
        ColorDodge => "color-dodge",
        ColorBurn => "color-burn",
        HardLight => "hard-light",
        SoftLight => "soft-light",
        Difference => "difference",
        Exclusion => "exclusion",
        Hue => "hue",
        Saturation => "saturation",
        Color => "color",
        Luminosity => "luminosity",
        PlusLighter => "plus-lighter",
    }
}

keywords! {
    /// `isolation`.
    Isolation {
        Auto => "auto",
        Isolate => "isolate",
    }
}
