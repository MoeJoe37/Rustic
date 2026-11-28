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
//! two reaching the table's cells; `bordercolor` on tables. Body `text`,
//! `bgcolor`, `link` (reaching its links) and the margin attributes;
//! `font` `color`, `face` and `size`; `hr` `align`, `color`, `noshade`,
//! `size` and `width`; list `type` on `ol`, `ul` and `li`; `hspace`,
//! `vspace` and `align` on embedded content; `align` on `div`, `p`,
//! headings and `caption`; `wrap` on `pre`. Colors follow HTML's legacy
//! color parser. `rules`, `frame` and `background` are not mapped yet
//! (TD-179).

use crate::ElementView;

const HTML: &str = "http://www.w3.org/1999/xhtml";
const MATHML: &str = "http://www.w3.org/1998/Math/MathML";

/// MathML children styled by their position among their siblings
/// (MathML Core §3, approximated with CSS: TD-221): the fraction rule
/// under the numerator, script shifts and sizes, and the over script's
/// place in `munderover`. As hints, these keep position-dependent
/// selectors out of the UA sheet, so insertions elsewhere stay cheap; a
/// MathML child's hints are computed when it is styled.
fn mathml_position_hints<E: ElementView>(element: &E) -> String {
    let Some(parent) = element.parent_element() else {
        return String::new();
    };
    if parent.namespace() != Some(MATHML) {
        return String::new();
    }
    let mut position = 1;
    let mut sibling = element.previous_sibling_element();
    while let Some(s) = sibling {
        position += 1;
        if position > 3 {
            break;
        }
        sibling = s.previous_sibling_element();
    }
    let script = "font-size: 71%; ";
    match (parent.local_name(), position) {
        ("mfrac", 1) => {
            if attribute(&parent, "linethickness").is_some_and(|t| t.trim() == "0") {
                "padding-inline: 0.1em; ".to_owned()
            } else {
                "border-bottom: 0.06em solid; padding-inline: 0.1em; ".to_owned()
            }
        }
        ("msup", 2) | ("msubsup", 3) => format!("vertical-align: super; {script}"),
        ("msub", 2) | ("msubsup", 2) => format!("vertical-align: sub; {script}"),
        ("munderover", 3) => format!("order: -1; {script}"),
        ("munder" | "mover" | "munderover", 2) => script.to_owned(),
        ("mroot", 2) => "font-size: 50%; vertical-align: super; ".to_owned(),
        ("semantics", n) if n > 1 => "display: none; ".to_owned(),
        _ => String::new(),
    }
}

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
            | "bordercolor"
            | "text"
            | "link"
            | "color"
            | "face"
            | "size"
            | "noshade"
            | "type"
            | "hspace"
            | "vspace"
            | "wrap"
            | "marginwidth"
            | "marginheight"
            | "topmargin"
            | "bottommargin"
            | "leftmargin"
            | "rightmargin"
            | "href"
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

/// HTML "rules for parsing a legacy color value" (§2.3.6.4): CSS named
/// colors, then the lenient hex algorithm that turns any other text into
/// a color. The result is a validated name or `#rrggbb`.
fn legacy_color(value: &str) -> Option<String> {
    let v = value.trim_matches(is_space);
    if v.is_empty() || v.eq_ignore_ascii_case("transparent") {
        return None;
    }
    if v.chars().all(|c| c.is_ascii_alphabetic()) && rustic_css::Color::named(v).is_some() {
        return Some(v.to_ascii_lowercase());
    }
    let chars: Vec<char> = v.chars().collect();
    if chars.len() == 4 && chars[0] == '#' && chars[1..].iter().all(char::is_ascii_hexdigit) {
        let hex: String = chars[1..].iter().flat_map(|&c| [c, c]).collect();
        return Some(format!("#{}", hex.to_ascii_lowercase()));
    }
    // Code points above U+FFFF count as "00"; at most 128 code points.
    let mut text: Vec<char> = Vec::new();
    for c in chars {
        if u32::from(c) > 0xFFFF {
            text.extend(['0', '0']);
        } else {
            text.push(c);
        }
        if text.len() >= 128 {
            break;
        }
    }
    text.truncate(128);
    if text.first() == Some(&'#') {
        text.remove(0);
    }
    for c in &mut text {
        if !c.is_ascii_hexdigit() {
            *c = '0';
        }
    }
    while text.is_empty() || text.len() % 3 != 0 {
        text.push('0');
    }
    let length = text.len() / 3;
    let mut parts: Vec<&[char]> = text.chunks(length).collect();
    if length > 8 {
        for part in &mut parts {
            *part = &part[length - 8..];
        }
    }
    while parts[0].len() > 2 && parts.iter().all(|p| p[0] == '0') {
        for part in &mut parts {
            *part = &part[1..];
        }
    }
    let hex: String = parts
        .iter()
        .map(|p| {
            let digits: String = p.iter().take(2).collect();
            format!("{digits:0>2}")
        })
        .collect();
    Some(format!("#{}", hex.to_ascii_lowercase()))
}

/// The `font` `size` attribute as a CSS keyword (HTML §15.3.4 "rules for
/// parsing a legacy font size").
fn font_size(value: &str) -> Option<&'static str> {
    let rest = value.trim_start_matches(is_space);
    let (mode, rest) = match rest.chars().next()? {
        '+' => (1, &rest[1..]),
        '-' => (-1, &rest[1..]),
        _ => (0, rest),
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let n: i64 = digits.parse::<i64>().unwrap_or(i64::MAX).min(100);
    let size = match mode {
        1 => 3 + n,
        -1 => 3 - n,
        _ => n,
    }
    .clamp(1, 7);
    const SIZES: [&str; 7] = [
        "x-small",
        "small",
        "medium",
        "large",
        "x-large",
        "xx-large",
        "xxx-large",
    ];
    usize::try_from(size - 1).ok().map(|index| SIZES[index])
}

/// A `font` `face` list as quoted CSS family names; names keep only
/// letters, digits, spaces, hyphens and underscores.
fn font_families(value: &str) -> Option<String> {
    let names: Vec<String> = value
        .split(',')
        .map(|name| name.trim_matches(is_space))
        .filter(|name| {
            !name.is_empty()
                && name.len() <= 64
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        })
        .map(|name| format!("\"{name}\""))
        .take(16)
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}

/// The nearest HTML `body` ancestor.
fn body_ancestor<E: ElementView>(element: &E) -> Option<E> {
    let mut current = element.parent_element();
    while let Some(e) = current {
        if is_html(&e) && e.local_name().eq_ignore_ascii_case("body") {
            return Some(e);
        }
        current = e.parent_element();
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
    if element.namespace() == Some(MATHML) {
        return mathml_position_hints(element);
    }
    if !is_html(element) {
        return String::new();
    }
    // A fieldset's first child legend is its rendered legend (§15.3.13),
    // which shrinks to fit.
    if element.local_name().eq_ignore_ascii_case("legend")
        && element.previous_sibling_element().is_none()
        && element
            .parent_element()
            .is_some_and(|p| is_html(&p) && p.local_name().eq_ignore_ascii_case("fieldset"))
    {
        return "inline-size: fit-content; ".to_owned();
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
            if let Some(c) = attribute(element, "bordercolor")
                .as_deref()
                .and_then(legacy_color)
            {
                push("border-color", &c);
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
        "body" => {
            if let Some(c) = attribute(element, "text").as_deref().and_then(legacy_color) {
                push("color", &c);
            }
            color(&mut push);
            for (attributes, property) in [
                (["marginheight", "topmargin"], "margin-top"),
                (["marginheight", "bottommargin"], "margin-bottom"),
                (["marginwidth", "leftmargin"], "margin-left"),
                (["marginwidth", "rightmargin"], "margin-right"),
            ] {
                let value = attributes.iter().rev().find_map(|a| {
                    attribute(element, a)
                        .as_deref()
                        .and_then(non_negative_integer)
                });
                if let Some(n) = value {
                    push(property, &format!("{n}px"));
                }
            }
        }
        "a" if attribute(element, "href").is_some() => {
            // A link takes its body's `link` color (visited links are not
            // tracked, so `vlink` never applies).
            if let Some(c) = body_ancestor(element)
                .and_then(|body| attribute(&body, "link"))
                .as_deref()
                .and_then(legacy_color)
            {
                push("color", &c);
            }
        }
        "font" => {
            if let Some(c) = attribute(element, "color")
                .as_deref()
                .and_then(legacy_color)
            {
                push("color", &c);
            }
            if let Some(f) = attribute(element, "face")
                .as_deref()
                .and_then(font_families)
            {
                push("font-family", &f);
            }
            if let Some(size) = attribute(element, "size").as_deref().and_then(font_size) {
                push("font-size", size);
            }
        }
        "hr" => {
            match attribute(element, "align")
                .map(|a| a.trim_matches(is_space).to_ascii_lowercase())
                .as_deref()
            {
                Some("left") => {
                    push("margin-left", "0");
                    push("margin-right", "auto");
                }
                Some("right") => {
                    push("margin-left", "auto");
                    push("margin-right", "0");
                }
                Some("center") => {
                    push("margin-left", "auto");
                    push("margin-right", "auto");
                }
                _ => {}
            }
            let color = attribute(element, "color")
                .as_deref()
                .and_then(legacy_color);
            if let Some(c) = &color {
                push("border-style", "solid");
                push("border-color", c);
                push("background-color", c);
            } else if attribute(element, "noshade").is_some() {
                push("border-style", "solid");
                push("border-color", "gray");
                push("background-color", "gray");
            }
            if let Some(n) = attribute(element, "size")
                .as_deref()
                .and_then(non_negative_integer)
            {
                if n <= 1 {
                    push("border-bottom-width", "0");
                } else {
                    push("height", &format!("{}px", n - 2));
                }
            }
            if let Some(d) = attribute(element, "width").as_deref().and_then(dimension) {
                if !d.is_zero() {
                    push("width", &d.css());
                }
            }
        }
        "ol" | "ul" | "li" => {
            let kind = attribute(element, "type");
            let kind = kind.as_deref().map(|t| t.trim_matches(is_space));
            let ordered = match kind {
                Some("1") => Some("decimal"),
                Some("a") => Some("lower-alpha"),
                Some("A") => Some("upper-alpha"),
                Some("i") => Some("lower-roman"),
                Some("I") => Some("upper-roman"),
                _ => None,
            };
            let unordered = kind.and_then(|t| {
                ["disc", "circle", "square", "none"]
                    .into_iter()
                    .find(|k| t.eq_ignore_ascii_case(k))
            });
            let chosen = match name.as_str() {
                "ol" => ordered,
                "ul" => unordered,
                _ => ordered.or(unordered),
            };
            if let Some(t) = chosen {
                push("list-style-type", t);
            }
        }
        "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            if let Some(a) = attribute(element, "align").as_deref().and_then(text_align) {
                push("text-align", a);
            }
        }
        "caption" => {
            match attribute(element, "align")
                .map(|a| a.trim_matches(is_space).to_ascii_lowercase())
                .as_deref()
            {
                Some("bottom") => push("caption-side", "bottom"),
                Some("top") => push("caption-side", "top"),
                _ => {}
            }
        }
        "pre" if attribute(element, "wrap").is_some() => push("white-space", "pre-wrap"),
        _ => {}
    }
    if matches!(
        name.as_str(),
        "img" | "iframe" | "embed" | "object" | "video"
    ) {
        for (attribute_name, sides) in [
            ("hspace", ["margin-left", "margin-right"]),
            ("vspace", ["margin-top", "margin-bottom"]),
        ] {
            if let Some(d) = attribute(element, attribute_name)
                .as_deref()
                .and_then(dimension)
            {
                for side in sides {
                    push(side, &d.css());
                }
            }
        }
        if name != "video" {
            match attribute(element, "align")
                .map(|a| a.trim_matches(is_space).to_ascii_lowercase())
                .as_deref()
            {
                Some("left") => push("float", "left"),
                Some("right") => push("float", "right"),
                Some("top") => push("vertical-align", "top"),
                Some("middle" | "absmiddle" | "abscenter") => push("vertical-align", "middle"),
                Some("bottom" | "baseline") => push("vertical-align", "baseline"),
                _ => {}
            }
        }
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
        // Without `#`, three digits are three components (HTML, not CSS).
        assert_eq!(legacy_color("fc0").as_deref(), Some("#0f0c00"));
        assert_eq!(legacy_color(" Red ").as_deref(), Some("red"));
        assert_eq!(legacy_color("transparent"), None);
        // The lenient hex algorithm: any other text becomes hex digits,
        // and the result is always a checked `#rrggbb`.
        assert_eq!(
            legacy_color("red; display: none").as_deref(),
            Some("#ed0000")
        );
        assert_eq!(legacy_color("url(x)").as_deref(), Some("#000000"));
        assert_eq!(legacy_color("chucknorris").as_deref(), Some("#c00000"));
        assert_eq!(legacy_color("#abcd").as_deref(), Some("#abcd00"));
        assert_eq!(legacy_color("#ffcc0").as_deref(), Some("#ffcc00"));
        assert_eq!(legacy_color("ninjaturtle").as_deref(), Some("#00a000"));
        assert_eq!(legacy_color("#1234567890").as_deref(), Some("#125690"));
    }

    #[test]
    fn font_sizes_and_faces_follow_the_legacy_rules() {
        assert_eq!(font_size("1"), Some("x-small"));
        assert_eq!(font_size("+2"), Some("x-large"));
        assert_eq!(font_size("-5"), Some("x-small"));
        assert_eq!(font_size("9"), Some("xxx-large"));
        assert_eq!(font_size("x"), None);
        assert_eq!(
            font_families("Arial, 'bad'; x, Times New Roman").as_deref(),
            Some("\"Arial\", \"Times New Roman\"")
        );
    }
}
