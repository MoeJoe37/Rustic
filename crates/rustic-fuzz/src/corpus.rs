//! Seed corpora: checked-in seeds, saved regressions and pinned upstream data.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::read_dir_inputs;

/// Repository root, derived from this crate's location at build time.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every seed for `target`, in a fixed order.
///
/// Sources: `crates/rustic-fuzz/regressions/<target>/` (minimized reproducers
/// of fixed defects), `crates/rustic-fuzz/corpus/<family>/`, and for HTML the
/// `#data` sections of the vendored html5lib tree-construction files, or for
/// URLs the inputs and bases of the pinned WPT URL cases.
pub fn seed_inputs(root: &Path, target: &str) -> io::Result<Vec<Vec<u8>>> {
    let fuzz = root.join("crates/rustic-fuzz");
    let mut seeds: Vec<Vec<u8>> = read_dir_inputs(&fuzz.join("regressions").join(target))?
        .into_iter()
        .map(|(_, bytes)| bytes)
        .collect();
    let family = if target.starts_with("html")
        || target == "layout-formatting"
        || target == "paint-raster"
    {
        "html"
    } else if target == "css-cascade" || target == "css-invalidation" {
        "css"
    } else {
        target
    };
    seeds.extend(
        read_dir_inputs(&fuzz.join("corpus").join(family))?
            .into_iter()
            .map(|(_, bytes)| bytes),
    );
    match family {
        "html" => seeds.extend(html5lib_data(root)?),
        "url" => seeds.extend(wpt_url_strings(root)?),
        "css" => seeds.extend(css_corpus_inputs(root)?),
        "css-selector" => seeds.extend(selector_inputs(root)?),
        "css-media" => seeds.extend(media_inputs(root)?),
        "text-shaping" => seeds.extend(crate::shaping::seeds()),
        "text-sfnt" => seeds.extend(crate::sfnt::seeds()),
        "codec-png" => seeds.extend(crate::png::seeds(root)?),
        "codec-jpeg" => seeds.extend(crate::images::JPEG.seeds(root)?),
        "codec-gif" => seeds.extend(crate::images::GIF.seeds(root)?),
        "codec-webp" => seeds.extend(crate::images::WEBP.seeds(root)?),
        "js-parse" => seeds.extend(crate::js::seeds(root)?),
        "js-heap" => seeds.extend(crate::js_heap::seeds()),
        _ => {}
    }
    Ok(seeds)
}

fn html5lib_data(root: &Path) -> io::Result<Vec<Vec<u8>>> {
    let directory = root.join("crates/rustic-html/tests/fixtures/tree");
    let mut files: Vec<PathBuf> = fs::read_dir(&directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<_>>()?;
    files.retain(|path| path.extension().is_some_and(|ext| ext == "dat"));
    files.sort();
    let mut seeds = Vec::new();
    for file in files {
        let source = fs::read_to_string(&file)?;
        for block in source.split("#data\n").skip(1) {
            let data = block.split("\n#errors").next().unwrap_or("");
            seeds.push(data.as_bytes().to_vec());
        }
    }
    Ok(seeds)
}

/// Input strings of the vendored css-parsing-tests files (every even entry).
fn css_corpus_inputs(root: &Path) -> io::Result<Vec<Vec<u8>>> {
    let directory = root.join("crates/rustic-css/tests/fixtures/css-parsing-tests");
    let mut seeds = Vec::new();
    for entry in read_dir_inputs(&directory)? {
        let (path, bytes) = entry;
        if path.extension().is_none_or(|ext| ext != "json") || path.ends_with("manifest.json") {
            continue;
        }
        let source = String::from_utf8_lossy(&bytes);
        // Inputs sit at the start of a line as `"...",`; expectations are nested.
        for line in source.lines() {
            if let Some(rest) = line.strip_prefix('"') {
                if let Some((value, _)) = json_string_body(rest) {
                    seeds.push(value.into_bytes());
                }
            }
        }
    }
    Ok(seeds)
}

/// One selector per line of `corpus/css-selector/*.sel`, plus every pinned
/// `An+B` input wrapped in `:nth-child()`.
fn selector_inputs(root: &Path) -> io::Result<Vec<Vec<u8>>> {
    let fuzz = root.join("crates/rustic-fuzz/corpus/css-selector");
    let mut seeds = Vec::new();
    for (_, bytes) in read_dir_inputs(&fuzz)? {
        let source = String::from_utf8_lossy(&bytes);
        seeds.extend(source.lines().map(|line| line.as_bytes().to_vec()));
    }
    let anb = root.join("crates/rustic-css/tests/fixtures/css-parsing-tests/An+B.json");
    for line in fs::read_to_string(anb)?.lines() {
        if let Some((value, _)) = line.strip_prefix('"').and_then(json_string_body) {
            seeds.push(format!(":nth-child({value})").into_bytes());
        }
    }
    Ok(seeds)
}

/// One media query list per line of `corpus/css-media/*.mq`.
fn media_inputs(root: &Path) -> io::Result<Vec<Vec<u8>>> {
    let fuzz = root.join("crates/rustic-fuzz/corpus/css-media");
    let mut seeds = Vec::new();
    for (_, bytes) in read_dir_inputs(&fuzz)? {
        let source = String::from_utf8_lossy(&bytes);
        seeds.extend(source.lines().map(|line| line.as_bytes().to_vec()));
    }
    Ok(seeds)
}

/// Decode a JSON string body up to its closing quote (`\uXXXX` pairs included).
fn json_string_body(source: &str) -> Option<(String, &str)> {
    let mut value = String::new();
    let mut chars = source.char_indices();
    while let Some((index, c)) = chars.next() {
        match c {
            '"' => return Some((value, &source[index + 1..])),
            '\\' => match chars.next()?.1 {
                'n' => value.push('\n'),
                't' => value.push('\t'),
                'r' => value.push('\r'),
                'f' => value.push('\u{c}'),
                'u' => {
                    let hex: String = (0..4)
                        .filter_map(|_| chars.next().map(|(_, c)| c))
                        .collect();
                    let unit = u32::from_str_radix(&hex, 16).ok()?;
                    value.push(char::from_u32(unit).unwrap_or('\u{fffd}'));
                }
                other => value.push(other),
            },
            c => value.push(c),
        }
    }
    None
}

fn wpt_url_strings(root: &Path) -> io::Result<Vec<Vec<u8>>> {
    let source = fs::read_to_string(root.join("crates/rustic-url/tests/fixtures/wpt_cases.rs"))?;
    let mut seeds = Vec::new();
    for marker in ["input: \"", "base: Some(\""] {
        let mut rest = source.as_str();
        while let Some(at) = rest.find(marker) {
            rest = &rest[at + marker.len()..];
            if let Some((value, after)) = rust_string_literal(rest) {
                seeds.push(value.into_bytes());
                rest = after;
            }
        }
    }
    Ok(seeds)
}

/// Decode the body of a generated Rust string literal up to its closing quote.
fn rust_string_literal(source: &str) -> Option<(String, &str)> {
    let mut value = String::new();
    let mut chars = source.char_indices();
    while let Some((index, c)) = chars.next() {
        match c {
            '"' => return Some((value, &source[index + 1..])),
            '\\' => match chars.next()?.1 {
                'n' => value.push('\n'),
                't' => value.push('\t'),
                'r' => value.push('\r'),
                '0' => value.push('\0'),
                'u' => {
                    let mut hex = String::new();
                    chars.next().filter(|(_, c)| *c == '{')?;
                    for (_, c) in chars.by_ref() {
                        if c == '}' {
                            break;
                        }
                        hex.push(c);
                    }
                    value.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                other => value.push(other),
            },
            c => value.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_generated_literals() {
        let (value, rest) = rust_string_literal(r#"a\tb\u{1F600}\"c\\", base"#).unwrap();
        assert_eq!(value, "a\tb\u{1F600}\"c\\");
        assert_eq!(rest, ", base");
        assert!(rust_string_literal("unterminated").is_none());
    }
}
