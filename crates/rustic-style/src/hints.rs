//! HTML presentational hints (HTML §15.3), as author-level declarations
//! with zero specificity that lose to every author rule.
//!
//! [`presentational_hints`] turns an element's attributes into CSS text
//! that the cascade parses like a `style` attribute. Every value is
//! validated and re-serialized here (numbers formatted by Rustic, colors
//! only as checked names or hex digits), so attribute text never reaches
//! the CSS parser.
//!
//! Mapped: `width`/`height` on `img`, `iframe`, `video`, `embed`, `object`,
//! `table`, `td`, `th`, `col`, `colgroup` (`height` also on `tr`); `bgcolor`
//! on tables, rows, row groups and cells; `align` and `valign` on rows, row
//! groups and cells; `nowrap` on cells; `align` on tables (floats or auto
//! margins); `cellspacing`, `cellpadding` and `border` on tables, the latter
//! two reaching the table's cells. `rules`, `frame`, `bordercolor`, body
//! colors and the other legacy attributes are not mapped yet (TD-179).

use crate::ElementView;

const HTML: &str = "http://www.w3.org/1999/xhtml";

/// Attributes whose change can change hints on the element or its
/// descendants (table attributes reach the cells).
pub(crate) fn is_hint_attribute(local: &str) -> bool {
    matches!(
        local,
        "width"
            | "height"
            | "bgcolor"
            | "align"
            | "valign"
            | "nowrap"
            | "cellspacing"
            | "cellpadding"
            | "border"
    )
}

fn attribute<E: ElementView>(element: &E, name: &str) -> Option<String> {
    element
        .attributes()
        .find(|a| a.namespace.is_none() && a.local.eq_ignore_ascii_case(name))
        .map(|a| String::from_utf16_lossy(a.value))
}

fn is_html<E: ElementView>(element: &E) -> bool {
    element.namespace() == Some(HTML)
}

fn is_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

/// HTML "rules for parsing non-negative integers".
fn non_negative_integer(value: &str) -> Option<u32> {
    let rest = value.trim_start_matches(is_space);
    let rest = rest.strip_prefix('+').unwrap_or(rest);
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    Some(
        digits
            .parse::<u64>()
            .map_or(u32::MAX, |n| n.min(u64::from(u32::MAX)) as u32),
    )
}

/// A parsed dimension: pixels or a percentage.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Dimension {
    Pixels(f64),
    Percent(f64),
}

impl Dimension {
    fn css(self) -> String {
        match self {
            Self::Pixels(n) => format!("{}px", number(n)),
            Self::Percent(n) => format!("{}%", number(n)),
        }
    }
    fn is_zero(self) -> bool {
        matches!(self, Self::Pixels(n) | Self::Percent(n) if n == 0.0)
    }
}

fn number(n: f64) -> String {
    let rounded = (n * 1000.0).round() / 1000.0;
    format!("{rounded}")
}

/// HTML "rules for parsing dimension values".
fn dimension(value: &str) -> Option<Dimension> {
    let rest = value.trim_start_matches(is_space);
    let integer: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if integer.is_empty() {
        return None;
    }
    let mut n: f64 = integer.parse().ok()?;
    let mut rest = &rest[integer.len()..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits: String = fraction.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            n += format!("0.{digits}").parse::<f64>().ok()?;
        }
        rest = &fraction[digits.len()..];
    }
    if !n.is_finite() || n > 1e7 {
        return None;
    }
    Some(if rest.starts_with('%') {
        Dimension::Percent(n)
    } else {
        Dimension::Pixels(n)
    })
}

/// A subset of HTML's "rules for parsing a legacy color value": names
/// (ASCII letters only, left to the CSS parser to accept) and 3- or
/// 6-digit hex with or without `#`.
fn legacy_color(value: &str) -> Option<String> {
    let v = value.trim_matches(is_space);
    if v.is_empty() || v.eq_ignore_ascii_case("transparent") {
        return None;
    }
    let hex = v.strip_prefix('#').unwrap_or(v);
    if matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(format!("#{hex}"));
    }
    if v.len() <= 32 && v.chars().all(|c| c.is_ascii_alphabetic()) {
        return Some(v.to_ascii_lowercase());
    }
    None
}

fn text_align(value: &str) -> Option<&'static str> {
    let v = value.trim_matches(is_space);
    [
        ("left", "left"),
        ("right", "right"),
        ("center", "center"),
        ("middle", "center"),
        ("justify", "justify"),
    ]
    .into_iter()
    .find(|(k, _)| v.eq_ignore_ascii_case(k))
    .map(|(_, css)| css)
}

fn vertical_align(value: &str) -> Option<&'static str> {
    let v = value.trim_matches(is_space);
    ["top", "middle", "bottom", "baseline"]
        .into_iter()
        .find(|k| v.eq_ignore_ascii_case(k))
}

/// The nearest `table` ancestor of a cell, row or row group, if it is an
/// HTML table reached through table-structure elements only.
fn owning_table<E: ElementView>(element: &E) -> Option<E> {
    let mut current = element.parent_element()?;
    for _ in 0..3 {
        if !is_html(&current) {
            return None;
        }
        match current.local_name() {
            "table" => return Some(current),
            "tr" | "tbody" | "thead" | "tfoot" => current = current.parent_element()?,
            _ => return None,
        }
    }
    None
}

/// The table `border` attribute's width: absent → `None`; unparsable or
/// empty → 1 (HTML §15.3.8).
fn table_border<E: ElementView>(table: &E) -> Option<u32> {
    attribute(table, "border").map(|v| non_negative_integer(&v).unwrap_or(1))
}

/// CSS declarations for `element`'s presentational hints, or an empty
/// string.
pub(crate) fn presentational_hints<E: ElementView>(element: &E) -> String {
    if !is_html(element) {
        return String::new();
    }
    let mut css = String::new();
    let mut push = |property: &str, value: &str| {
        css.push_str(property);
        css.push_str(": ");
        css.push_str(value);
        css.push_str("; ");
    };
    let name = element.local_name().to_ascii_lowercase();
    let dims = |push: &mut dyn FnMut(&str, &str), skip_zero: bool, height: bool| {
        let props: &[&str] = if height {
            &["width", "height"]
        } else {
            &["width"]
        };
        for p in props {
            if let Some(d) = attribute(element, p).as_deref().and_then(dimension) {
                if !(skip_zero && d.is_zero()) {
                    push(p, &d.css());
                }
            }
        }
    };
    let color = |push: &mut dyn FnMut(&str, &str)| {
        if let Some(c) = attribute(element, "bgcolor")
            .as_deref()
            .and_then(legacy_color)
        {
            push("background-color", &c);
        }
    };
    let aligns = |push: &mut dyn FnMut(&str, &str)| {
        if let Some(a) = attribute(element, "align").as_deref().and_then(text_align) {
            push("text-align", a);
        }
        if let Some(v) = attribute(element, "valign")
            .as_deref()
            .and_then(vertical_align)
        {
            push("vertical-align", v);
        }
    };
    match name.as_str() {
        "img" | "iframe" | "video" | "embed" | "object" => dims(&mut push, false, true),
        "table" => {
            dims(&mut push, true, true);
            color(&mut push);
            if let Some(n) = attribute(element, "cellspacing")
                .as_deref()
                .and_then(non_negative_integer)
            {
                push("border-spacing", &format!("{n}px"));
            }
            if let Some(n) = table_border(element).filter(|&n| n > 0) {
                push("border", &format!("{n}px outset gray"));
            }
            match attribute(element, "align")
                .map(|a| a.trim_matches(is_space).to_ascii_lowercase())
                .as_deref()
            {
                Some("left") => push("float", "left"),
                Some("right") => push("float", "right"),
                Some("center") => {
                    push("margin-left", "auto");
                    push("margin-right", "auto");
                }
                _ => {}
            }
        }
        "td" | "th" => {
            dims(&mut push, true, true);
            color(&mut push);
            aligns(&mut push);
            if attribute(element, "nowrap").is_some() {
                push("white-space", "nowrap");
            }
            if let Some(table) = owning_table(element) {
                if let Some(n) = attribute(&table, "cellpadding")
                    .as_deref()
                    .and_then(non_negative_integer)
                {
                    push("padding", &format!("{n}px"));
                }
                if table_border(&table).is_some_and(|n| n > 0) {
                    push("border", "1px inset gray");
                }
            }
        }
        "tr" => {
            if let Some(d) = attribute(element, "height").as_deref().and_then(dimension) {
                if !d.is_zero() {
                    push("height", &d.css());
                }
            }
            color(&mut push);
            aligns(&mut push);
        }
        "tbody" | "thead" | "tfoot" => {
            color(&mut push);
            aligns(&mut push);
        }
        "col" | "colgroup" => dims(&mut push, true, false),
        _ => {}
    }
    css
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_follow_the_html_rules() {
        assert_eq!(dimension(" 120"), Some(Dimension::Pixels(120.0)));
        assert_eq!(dimension("50%"), Some(Dimension::Percent(50.0)));
        assert_eq!(dimension("12.5px"), Some(Dimension::Pixels(12.5)));
        assert_eq!(dimension("x10"), None);
        assert_eq!(dimension(""), None);
        assert_eq!(non_negative_integer(" +7 apples"), Some(7));
        assert_eq!(non_negative_integer("-1"), None);
    }

    #[test]
    fn legacy_colors_never_pass_arbitrary_text() {
        assert_eq!(legacy_color("#ffcc00").as_deref(), Some("#ffcc00"));
        assert_eq!(legacy_color("fc0").as_deref(), Some("#fc0"));
        assert_eq!(legacy_color(" Red ").as_deref(), Some("red"));
        assert_eq!(legacy_color("red; display: none"), None);
        assert_eq!(legacy_color("url(x)"), None);
        assert_eq!(legacy_color("transparent"), None);
    }
}
