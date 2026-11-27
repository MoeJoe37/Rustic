//! `@font-face` rules (CSS Fonts 4 §4).
//!
//! A rule names a family and lists sources; the style descriptors
//! (`font-weight`, `font-style`, `font-stretch`) and `unicode-range` say
//! which requests the face serves. The descriptors are parsed here; fetching
//! and decoding the sources is the embedder's work.

use std::sync::Arc;

use rustic_css::{AtRule, BlockItem, ComponentValue, Token, Value, parse_block_contents};
use rustic_url::Url;

/// The `font-style` descriptor, reduced to the slope classes face matching
/// uses (oblique angles are not distinguished).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontFaceStyle {
    /// `normal`.
    #[default]
    Normal,
    /// `italic`.
    Italic,
    /// `oblique` with any angle.
    Oblique,
}

/// One entry of the `src` descriptor.
#[derive(Clone, Debug, PartialEq)]
pub enum FontFaceSource {
    /// `url()`, with any supported `format()`. `url` is the reference
    /// resolved against the sheet's base URL ([`crate::StyleSheet::set_base_url`]),
    /// or `None` while it cannot be resolved.
    Url {
        /// The reference as written.
        href: Arc<str>,
        /// The resolved URL.
        url: Option<Url>,
    },
    /// `local(<family-name>)`: an installed face by full or PostScript name.
    Local(Arc<str>),
}

/// A valid `@font-face` rule.
#[derive(Clone, Debug, PartialEq)]
pub struct FontFaceRule {
    /// The `font-family` descriptor: the family name the face is used for.
    pub family: Arc<str>,
    /// Usable `src` entries in order; entries with an unsupported
    /// `format()` or `tech()` are dropped.
    pub sources: Vec<FontFaceSource>,
    /// The `font-weight` range, `low..=high` in `1..=1000`.
    pub weight: (f64, f64),
    /// The `font-style` descriptor.
    pub style: FontFaceStyle,
    /// The `font-stretch` range in percent.
    pub stretch: (f64, f64),
    /// The `unicode-range` descriptor as inclusive code point ranges
    /// (all of Unicode by default).
    pub unicode_range: Vec<(u32, u32)>,
    /// The conditional rules it sits in (indices into [`crate::StyleSheet::media`]).
    pub media: Vec<usize>,
}

impl FontFaceRule {
    /// Resolve `url()` sources against `base`.
    pub(crate) fn resolve(&mut self, base: Option<&Url>) {
        for source in &mut self.sources {
            if let FontFaceSource::Url { href, url } = source {
                *url = Url::parse(href, base).ok();
            }
        }
    }

    /// Whether `c` is in the rule's `unicode-range`.
    pub fn covers(&self, c: char) -> bool {
        let c = u32::from(c);
        self.unicode_range
            .iter()
            .any(|&(low, high)| low <= c && c <= high)
    }
}

fn significant(values: &[Value]) -> Vec<&Value> {
    values.iter().filter(|v| !v.is_whitespace()).collect()
}

/// Split at top-level commas.
fn commas(values: &[Value]) -> Vec<&[Value]> {
    values.split(|v| v.token() == Some(&Token::Comma)).collect()
}

fn ident(value: &Value) -> Option<&str> {
    match value.token()? {
        Token::Ident(name) => Some(name),
        _ => None,
    }
}

/// `<family-name>`: a string, or identifiers joined by single spaces.
fn family_name(values: &[Value]) -> Option<Arc<str>> {
    match significant(values).as_slice() {
        [one] if matches!(one.token(), Some(Token::String(_))) => match one.token() {
            Some(Token::String(s)) if !s.is_empty() => Some(s.as_str().into()),
            _ => None,
        },
        words if !words.is_empty() => {
            let mut out = Vec::new();
            for word in words {
                let name = ident(word)?;
                // CSS-wide keywords and `default` are not family names.
                if out.is_empty()
                    && words.len() == 1
                    && [
                        "inherit",
                        "initial",
                        "unset",
                        "revert",
                        "revert-layer",
                        "default",
                    ]
                    .iter()
                    .any(|k| name.eq_ignore_ascii_case(k))
                {
                    return None;
                }
                out.push(name);
            }
            Some(out.join(" ").into())
        }
        _ => None,
    }
}

/// The formats Rustic decodes (CSS Fonts 4 §4.3.1 `<font-format>`).
fn supported_format(name: &str) -> bool {
    ["woff", "woff2", "truetype", "opentype", "collection"]
        .iter()
        .any(|f| name.eq_ignore_ascii_case(f))
}

/// One `src` entry; `Ok(None)` for a valid entry Rustic cannot use.
fn source(values: &[Value]) -> Result<Option<FontFaceSource>, ()> {
    let parts = significant(values);
    let (first, rest) = parts.split_first().ok_or(())?;
    let href: Arc<str> = match &first.value {
        ComponentValue::Token(Token::Url(href)) => href.as_str().into(),
        ComponentValue::Function { name, arguments } if name.eq_ignore_ascii_case("url") => {
            match significant(arguments).as_slice() {
                [one] => match one.token() {
                    Some(Token::String(href)) => href.as_str().into(),
                    _ => return Err(()),
                },
                _ => return Err(()),
            }
        }
        ComponentValue::Function { name, arguments } if name.eq_ignore_ascii_case("local") => {
            if !rest.is_empty() {
                return Err(());
            }
            return Ok(Some(FontFaceSource::Local(
                family_name(arguments).ok_or(())?,
            )));
        }
        _ => return Err(()),
    };
    let mut usable = true;
    for part in rest {
        let ComponentValue::Function { name, arguments } = &part.value else {
            return Err(());
        };
        let words = significant(arguments);
        if name.eq_ignore_ascii_case("format") {
            let [format] = words.as_slice() else {
                return Err(());
            };
            let format = match format.token() {
                Some(Token::String(s) | Token::Ident(s)) => s,
                _ => return Err(()),
            };
            usable &= supported_format(format);
        } else if name.eq_ignore_ascii_case("tech") {
            // Color, variation and feature technologies are not decoded.
            usable = false;
        } else {
            return Err(());
        }
    }
    Ok(usable.then_some(FontFaceSource::Url { href, url: None }))
}

fn number(value: &Value) -> Option<f64> {
    match value.token()? {
        Token::Number(n) => Some(n.value),
        _ => None,
    }
}

/// `font-weight`: `auto` or one or two of `normal`, `bold`, `<number [1,1000]>`.
fn weight(values: &[Value]) -> Option<(f64, f64)> {
    let one = |v: &Value| match ident(v) {
        Some(k) if k.eq_ignore_ascii_case("normal") => Some(400.0),
        Some(k) if k.eq_ignore_ascii_case("bold") => Some(700.0),
        Some(_) => None,
        None => number(v).filter(|n| (1.0..=1000.0).contains(n)),
    };
    match significant(values).as_slice() {
        [auto] if ident(auto).is_some_and(|k| k.eq_ignore_ascii_case("auto")) => {
            Some((400.0, 400.0))
        }
        [a] => one(a).map(|a| (a, a)),
        [a, b] => {
            let (a, b) = (one(a)?, one(b)?);
            Some((a.min(b), a.max(b)))
        }
        _ => None,
    }
}

/// `font-stretch`: `auto` or one or two keywords or percentages.
fn stretch(values: &[Value]) -> Option<(f64, f64)> {
    const KEYWORDS: [(&str, f64); 9] = [
        ("ultra-condensed", 50.0),
        ("extra-condensed", 62.5),
        ("condensed", 75.0),
        ("semi-condensed", 87.5),
        ("normal", 100.0),
        ("semi-expanded", 112.5),
        ("expanded", 125.0),
        ("extra-expanded", 150.0),
        ("ultra-expanded", 200.0),
    ];
    let one = |v: &Value| match v.token()? {
        Token::Ident(k) => KEYWORDS
            .iter()
            .find(|(name, _)| k.eq_ignore_ascii_case(name))
            .map(|&(_, p)| p),
        Token::Percentage(n) if n.value >= 0.0 => Some(n.value),
        _ => None,
    };
    match significant(values).as_slice() {
        [auto] if ident(auto).is_some_and(|k| k.eq_ignore_ascii_case("auto")) => {
            Some((100.0, 100.0))
        }
        [a] => one(a).map(|a| (a, a)),
        [a, b] => {
            let (a, b) = (one(a)?, one(b)?);
            Some((a.min(b), a.max(b)))
        }
        _ => None,
    }
}

/// `font-style`: `auto | normal | italic | oblique [<angle>{1,2}]?`.
fn style(values: &[Value]) -> Option<FontFaceStyle> {
    let words = significant(values);
    let (first, rest) = words.split_first()?;
    let first = ident(first)?;
    if first.eq_ignore_ascii_case("oblique") {
        let angle = |v: &&Value| matches!(v.token(), Some(Token::Dimension(_, u)) if ["deg", "rad", "grad", "turn"].iter().any(|a| u.eq_ignore_ascii_case(a)));
        return (rest.len() <= 2 && rest.iter().all(angle)).then_some(FontFaceStyle::Oblique);
    }
    if !rest.is_empty() {
        return None;
    }
    if first.eq_ignore_ascii_case("normal") || first.eq_ignore_ascii_case("auto") {
        Some(FontFaceStyle::Normal)
    } else if first.eq_ignore_ascii_case("italic") {
        Some(FontFaceStyle::Italic)
    } else {
        None
    }
}

/// The source text of tokens, for the `<urange>` microsyntax
/// (CSS Syntax 3 §7.1), which spans ident, number and dimension tokens.
fn token_text(values: &[Value]) -> Option<String> {
    let mut out = String::new();
    for value in values {
        match value.token()? {
            Token::Ident(s) => out.push_str(s),
            Token::Number(n) => out.push_str(&n.repr),
            Token::Dimension(n, unit) => {
                out.push_str(&n.repr);
                out.push_str(unit);
            }
            Token::Delim(c) => out.push(*c),
            _ => return None,
        }
    }
    Some(out)
}

/// One `<urange>`: `U+hex`, `U+hex?…` or `U+hex-hex`.
fn urange(text: &str) -> Option<(u32, u32)> {
    let rest = text
        .strip_prefix("U+")
        .or_else(|| text.strip_prefix("u+"))?;
    let hex = |s: &str| {
        (!s.is_empty() && s.len() <= 6 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| u32::from_str_radix(s, 16).ok())
            .flatten()
    };
    let (low, high) = if let Some((a, b)) = rest.split_once('-') {
        (hex(a)?, hex(b)?)
    } else if rest.contains('?') {
        let digits = rest.trim_end_matches('?');
        let wild = rest.len() - digits.len();
        if rest.len() > 6 || digits.contains('?') {
            return None;
        }
        let base = if digits.is_empty() { 0 } else { hex(digits)? };
        let shift = 4 * u32::try_from(wild).ok()?;
        (base << shift, ((base + 1) << shift) - 1)
    } else {
        let one = hex(rest)?;
        (one, one)
    };
    (low <= high && low <= 0x10_FFFF).then_some((low, high.min(0x10_FFFF)))
}

fn unicode_range(values: &[Value]) -> Option<Vec<(u32, u32)>> {
    commas(values)
        .into_iter()
        .map(|part| {
            let words: Vec<Value> = significant(part).into_iter().cloned().collect();
            urange(&token_text(&words)?)
        })
        .collect()
}

/// Parse an `@font-face` rule; `None` if it lacks a valid `font-family` or
/// `src`. Invalid descriptors are ignored.
pub(crate) fn font_face_rule(at: &AtRule, media: Vec<usize>) -> Option<FontFaceRule> {
    if !significant(&at.prelude).is_empty() {
        return None;
    }
    let mut rule = FontFaceRule {
        family: Arc::from(""),
        sources: Vec::new(),
        weight: (400.0, 400.0),
        style: FontFaceStyle::Normal,
        stretch: (100.0, 100.0),
        unicode_range: vec![(0, 0x10_FFFF)],
        media,
    };
    let mut family = None;
    let mut sources = None;
    for item in parse_block_contents(at.block.as_deref()?, 0).value {
        let BlockItem::Declaration(d) = item else {
            continue;
        };
        if d.important {
            continue;
        }
        let value = &d.value;
        match d.name.to_ascii_lowercase().as_str() {
            "font-family" => {
                if let Some(name) = family_name(value) {
                    family = Some(name);
                }
            }
            "src" => {
                // An invalid entry invalidates the whole descriptor.
                let parsed: Result<Vec<_>, ()> = commas(value).into_iter().map(source).collect();
                if let Ok(parsed) = parsed {
                    sources = Some(parsed.into_iter().flatten().collect());
                }
            }
            "font-weight" => rule.weight = weight(value).unwrap_or(rule.weight),
            "font-style" => rule.style = style(value).unwrap_or(rule.style),
            "font-stretch" | "font-width" => {
                rule.stretch = stretch(value).unwrap_or(rule.stretch);
            }
            "unicode-range" => {
                if let Some(ranges) = unicode_range(value) {
                    rule.unicode_range = ranges;
                }
            }
            _ => {}
        }
    }
    rule.family = family?;
    rule.sources = sources?;
    rule.resolve(None);
    Some(rule)
}

#[cfg(test)]
mod tests {
    use super::urange;

    #[test]
    fn unicode_ranges_follow_the_urange_microsyntax() {
        assert_eq!(urange("U+26"), Some((0x26, 0x26)));
        assert_eq!(urange("u+0-7F"), Some((0, 0x7F)));
        assert_eq!(urange("U+4??"), Some((0x400, 0x4FF)));
        assert_eq!(urange("U+??????"), Some((0, 0x10_FFFF)));
        assert_eq!(urange("U+0025-00FF"), Some((0x25, 0xFF)));
        assert_eq!(urange("U+FF-0"), None);
        assert_eq!(urange("U+1?2"), None);
        assert_eq!(urange("26"), None);
    }
}
