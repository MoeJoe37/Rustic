//! Pinned CSS Color corpus: SimonSapin/css-parsing-tests `color_*.json`
//! (CC0-1.0, revision 203ce36bffd617db7f118c551e32794561fb273d).
//!
//! Each input is parsed as one component value and serialized. Numbers in the
//! expected serialization were produced with 32-bit floats, so numeric fields
//! are compared with a small tolerance; everything else must match exactly.

mod common;

use common::{Json, parse_json};
use rustic_css::{Color, CssParser};

macro_rules! corpus {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("fixtures/css-parsing-tests/", $name)))),*]
    };
}

const FILES: &[(&str, &str)] = corpus![
    "color_keywords_3.json",
    "color_keywords_4.json",
    "color_hexadecimal_3.json",
    "color_hexadecimal_4.json",
    "color_hsl_3.json",
    "color_hsl_4.json",
    "color_hwb_4.json",
    "color_lab_4.json",
    "color_lch_4.json",
    "color_oklab_4.json",
    "color_oklch_4.json",
    "color_function_4.json",
];

/// `(file, zero-based case index, reason)`.
const KNOWN_DIFFERENCES: &[(&str, usize, &str)] = &[];

fn serialize(input: &str) -> Option<String> {
    let value = CssParser::default().component_value(input).ok()?.value?;
    Color::parse(&value).map(|c| c.to_string())
}

/// Split into alternating text and number fields.
fn fields(s: &str) -> Vec<Result<f64, String>> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        let starts_number = c.is_ascii_digit()
            || ((c == '-' || c == '.') && text.chars().last().is_none_or(|p| !p.is_alphanumeric()));
        if starts_number {
            if !text.is_empty() {
                out.push(Err(std::mem::take(&mut text)));
            }
            let mut number = String::new();
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() || d == '.' || (number.is_empty() && d == '-') || d == 'e' {
                    number.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            match number.parse::<f64>() {
                Ok(n) => out.push(Ok(n)),
                Err(_) => text.push_str(&number),
            }
        } else {
            text.push(c);
            chars.next();
        }
    }
    if !text.is_empty() {
        out.push(Err(text));
    }
    out
}

fn close(actual: &str, expected: &str) -> bool {
    let (a, e) = (fields(actual), fields(expected));
    a.len() == e.len()
        && a.iter().zip(&e).all(|pair| match pair {
            (Ok(x), Ok(y)) => (x - y).abs() <= 1e-4 * y.abs().max(1.0),
            (Err(x), Err(y)) => x == y,
            _ => false,
        })
}

#[test]
fn pinned_color_corpus_matches() {
    let mut failures = Vec::new();
    let mut total = 0;
    for &(file, source) in FILES {
        let items = parse_json(source);
        for (index, pair) in items.chunks(2).enumerate() {
            total += 1;
            let Json::String(input) = &pair[0] else {
                panic!("{file} case {index}: input is not a string");
            };
            let expected = match &pair[1] {
                Json::String(s) => Some(s.as_str()),
                Json::Null => None,
                other => panic!("{file} case {index}: unexpected {other:?}"),
            };
            let actual = serialize(input);
            let ok = match (&actual, expected) {
                (Some(a), Some(e)) => close(a, e),
                (None, None) => true,
                _ => false,
            };
            let known = KNOWN_DIFFERENCES
                .iter()
                .any(|&(f, i, _)| f == file && i == index);
            if ok == known {
                failures.push(format!(
                    "{file} #{index} {input:?}: got {actual:?}, want {expected:?}{}",
                    if known {
                        " (listed as a known difference)"
                    } else {
                        ""
                    }
                ));
            }
        }
    }
    assert!(total > 7000, "corpus unexpectedly small: {total}");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures[..failures.len().min(60)].join("\n")
    );
}

#[test]
fn serialization_is_a_fixed_point() {
    for input in [
        "#ABCDEF80",
        "rgb(1 2 3 / 50%)",
        "hsl(120deg 100% 25%)",
        "hwb(0.5turn 10% 10%)",
        "lch(50% 30% 1.5rad / none)",
        "oklab(none 0.1 -0.1)",
        "color(xyz 1 none 50%)",
        "CurrentColor",
        "Transparent",
    ] {
        let once = serialize(input).unwrap_or_else(|| panic!("{input:?} is valid"));
        assert_eq!(
            serialize(&once).as_deref(),
            Some(once.as_str()),
            "{input:?}"
        );
    }
}

#[test]
fn unsupported_color_syntax_is_rejected() {
    for input in [
        "rgb(calc(1) 2 3)",
        "rgb(from red r g b)",
        "color-mix(in srgb, red, blue)",
        "light-dark(red, blue)",
        "Canvas",
        "#12345",
        "rgb(1, 2 3)",
        "rgb(1 2 3 4)",
        "rgb(10%, 20, 30)",
        "hwb(0, 0%, 0%)",
        "color(unknown 1 2 3)",
        "rgb(1deg 2 3)",
    ] {
        assert_eq!(serialize(input), None, "{input:?}");
    }
}
