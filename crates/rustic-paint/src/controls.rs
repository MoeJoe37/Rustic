//! The painted parts of native form controls that CSS boxes cannot
//! express: the value bar of `progress` and `meter` (HTML §4.10.13–14),
//! the track and thumb of `input type=range`, the swatch of
//! `input type=color` and the mark of a checked checkbox or radio button.
//!
//! Each part is a rounded rectangle in the control's content box, painted
//! after its background and before its border. The state comes from
//! content attributes (`value`, `checked`), except that a checkbox's or
//! radio button's live checkedness, when the host supplied it to the box
//! tree ([`rustic_layout::BoxTree::checked`]), wins over `checked`.

use rustic_dom::{Document, HTML_NAMESPACE, NodeId, NodeKind};
use rustic_layout::{CssPoint, CssPx, CssRect, CssSize};

use crate::color::Srgba;

/// One part: its rectangle, color and corner radius.
pub(crate) type Part = (CssRect, Srgba, f64);

const ACCENT: Srgba = rgb(0x00, 0x75, 0xff);
const TRACK: Srgba = rgb(0xef, 0xef, 0xef);
const OPTIMUM: Srgba = rgb(0x10, 0x7c, 0x10);
const SUBOPTIMAL: Srgba = rgb(0xff, 0xb9, 0x00);
const WORSE: Srgba = rgb(0xd1, 0x34, 0x38);

const fn rgb(r: u8, g: u8, b: u8) -> Srgba {
    Srgba { r, g, b, a: 255 }
}

/// The parts of the control `node`, whose content box is `content`.
/// `checked` is the input's live checkedness when the host knows it (else
/// its `checked` attribute decides).
pub(crate) fn parts(
    document: &Document,
    node: NodeId,
    content: CssRect,
    checked: Option<bool>,
) -> Vec<Part> {
    let Ok(NodeKind::Element(element)) = document.node(node).map(|n| n.kind()) else {
        return Vec::new();
    };
    if element.name().namespace() != Some(HTML_NAMESPACE) {
        return Vec::new();
    }
    let attribute = |name: &str| {
        element
            .attributes()
            .iter()
            .find(|a| a.name().namespace().is_none() && a.name().local() == name)
            .map(|a| String::from_utf16_lossy(a.value().as_units()))
    };
    let number = |name: &str| attribute(name).as_deref().and_then(parse_float);
    let (x, y) = (content.origin().x.get(), content.origin().y.get());
    let (w, h) = (content.size().width().get(), content.size().height().get());
    let rect = |x: f64, y: f64, w: f64, h: f64| {
        let origin = CssPoint {
            x: CssPx::new(x).ok()?,
            y: CssPx::new(y).ok()?,
        };
        CssRect::new(origin, CssSize::new(w.max(0.0), h.max(0.0)).ok()?).ok()
    };
    let mut out = Vec::new();
    let mut push = |r: Option<CssRect>, color: Srgba, radius: f64| {
        if let Some(r) = r {
            out.push((r, color, radius));
        }
    };
    match element.name().local() {
        "progress" => {
            // Indeterminate without a valid value.
            if let Some(value) = number("value") {
                let max = number("max").filter(|&m| m > 0.0).unwrap_or(1.0);
                let fraction = (value.clamp(0.0, max)) / max;
                push(rect(x, y, w * fraction, h), ACCENT, 0.0);
            }
        }
        "meter" => {
            let m = Meter::new(
                number("min"),
                number("max"),
                number("value"),
                number("low"),
                number("high"),
                number("optimum"),
            );
            push(rect(x, y, w * m.fraction(), h), m.color(), 0.0);
        }
        "input" => {
            let kind = attribute("type").unwrap_or_default().to_ascii_lowercase();
            let checked = checked.unwrap_or_else(|| attribute("checked").is_some());
            match kind.as_str() {
                "range" => {
                    let min = number("min").unwrap_or(0.0);
                    let max = number("max").unwrap_or(100.0).max(min);
                    let value = number("value")
                        .unwrap_or(min + (max - min) / 2.0)
                        .clamp(min, max);
                    let fraction = if max > min {
                        (value - min) / (max - min)
                    } else {
                        0.0
                    };
                    let thumb = h.min(16.0);
                    let at = x + thumb / 2.0 + fraction * (w - thumb).max(0.0);
                    let track = 4.0_f64.min(h);
                    let top = y + (h - track) / 2.0;
                    push(rect(x, top, w, track), TRACK, track / 2.0);
                    push(rect(x, top, at - x, track), ACCENT, track / 2.0);
                    push(
                        rect(at - thumb / 2.0, y + (h - thumb) / 2.0, thumb, thumb),
                        ACCENT,
                        thumb / 2.0,
                    );
                }
                "color" => {
                    let color = attribute("value")
                        .as_deref()
                        .and_then(simple_color)
                        .unwrap_or(rgb(0, 0, 0));
                    push(rect(x, y, w, h), color, 0.0);
                }
                "checkbox" if checked => push(rect(x, y, w, h), ACCENT, 2.0),
                "radio" if checked => {
                    let inset = (w.min(h) / 4.0).max(0.0);
                    push(rect(x, y, w, h), ACCENT, w.min(h) / 2.0);
                    let inner = w.min(h) - 2.0 * inset;
                    push(
                        rect(x + inset, y + inset, inner, inner),
                        Srgba::WHITE,
                        inner / 2.0,
                    );
                }
                _ => {}
            }
        }
        _ => {}
    }
    out
}

/// A `meter`'s values after the HTML §4.10.14 clamping steps.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Meter {
    min: f64,
    max: f64,
    value: f64,
    low: f64,
    high: f64,
    optimum: f64,
}

impl Meter {
    fn new(
        min: Option<f64>,
        max: Option<f64>,
        value: Option<f64>,
        low: Option<f64>,
        high: Option<f64>,
        optimum: Option<f64>,
    ) -> Self {
        let min = min.unwrap_or(0.0);
        let max = max.unwrap_or(1.0).max(min);
        let value = value.unwrap_or(0.0).clamp(min, max);
        let low = low.unwrap_or(min).clamp(min, max);
        let high = high.unwrap_or(max).clamp(low, max);
        let optimum = optimum.unwrap_or((min + max) / 2.0).clamp(min, max);
        Self {
            min,
            max,
            value,
            low,
            high,
            optimum,
        }
    }

    fn fraction(self) -> f64 {
        if self.max > self.min {
            (self.value - self.min) / (self.max - self.min)
        } else {
            0.0
        }
    }

    /// The region the value is in: optimum, suboptimal or even less good.
    fn color(self) -> Srgba {
        let Self {
            value,
            low,
            high,
            optimum,
            ..
        } = self;
        let middle = (low..=high).contains(&value);
        if optimum < low {
            if value < low {
                OPTIMUM
            } else if middle {
                SUBOPTIMAL
            } else {
                WORSE
            }
        } else if optimum > high {
            if value > high {
                OPTIMUM
            } else if middle {
                SUBOPTIMAL
            } else {
                WORSE
            }
        } else if middle {
            OPTIMUM
        } else {
            SUBOPTIMAL
        }
    }
}

/// The HTML rules for parsing floating-point number values: a leading
/// number after ASCII whitespace (`-`, digits, a fraction, an exponent),
/// trailing text ignored.
fn parse_float(value: &str) -> Option<f64> {
    let value = value.trim_start_matches(['\t', '\n', '\x0c', '\r', ' ']);
    let bytes = value.as_bytes();
    let mut end = 0;
    if bytes.first() == Some(&b'-') {
        end = 1;
    }
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let whole = digits(end);
    end += whole;
    let mut fraction = 0;
    if bytes.get(end) == Some(&b'.') {
        fraction = digits(end + 1);
        if fraction > 0 {
            end += 1 + fraction;
        }
    }
    if whole == 0 && fraction == 0 {
        return None;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut at = end + 1;
        if matches!(bytes.get(at), Some(b'-' | b'+')) {
            at += 1;
        }
        let exponent = digits(at);
        if exponent > 0 {
            end = at + exponent;
        }
    }
    value[..end].parse::<f64>().ok().filter(|n| n.is_finite())
}

/// A valid simple color (`#rrggbb`, HTML §2.3.6), ASCII case-insensitive.
fn simple_color(value: &str) -> Option<Srgba> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(rgb(channel(0)?, channel(2)?, channel(4)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_follow_the_html_parsing_rules() {
        assert_eq!(parse_float("0.5"), Some(0.5));
        assert_eq!(parse_float("-2e1x"), Some(-20.0));
        assert_eq!(parse_float(".25"), Some(0.25));
        assert_eq!(parse_float("3."), Some(3.0));
        assert_eq!(parse_float("e5"), None);
        assert_eq!(parse_float(" 1"), Some(1.0));
        assert_eq!(parse_float("+1"), None);
    }

    #[test]
    fn meter_regions_follow_low_high_and_optimum() {
        let meter = |value, low, high, optimum| {
            Meter::new(Some(0.0), Some(10.0), Some(value), low, high, optimum)
        };
        assert_eq!(meter(5.0, None, None, None).color(), OPTIMUM);
        assert_eq!(meter(2.0, Some(3.0), Some(7.0), None).color(), SUBOPTIMAL);
        assert_eq!(meter(9.0, Some(3.0), Some(7.0), Some(1.0)).color(), WORSE);
        assert_eq!(meter(1.0, Some(3.0), Some(7.0), Some(1.0)).color(), OPTIMUM);
        // max below min becomes min; the value clamps into the range.
        let m = Meter::new(Some(4.0), Some(2.0), Some(9.0), None, None, None);
        assert_eq!((m.max, m.value, m.fraction()), (4.0, 4.0, 0.0));
    }

    #[test]
    fn only_six_digit_hex_is_a_simple_color() {
        assert_eq!(simple_color("#FF8000"), Some(rgb(255, 128, 0)));
        assert_eq!(simple_color("#f80"), None);
        assert_eq!(simple_color("red"), None);
    }
}
