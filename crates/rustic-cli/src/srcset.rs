//! Image source selection (HTML §4.8.4.3): `srcset` and `sizes` on `img`
//! and on the `source` elements of a `picture`.
//!
//! The image source set is parsed with the HTML microsyntax (URLs with `w`,
//! `x` or `h` descriptors), `sizes` picks the first source size whose media
//! condition matches the view, and the selected candidate is the one with
//! the smallest density at or above the view's device pixel ratio (else the
//! densest). A `picture` uses its first `source` whose `media` matches and
//! whose `type` is a format Rustic decodes. The selected density scales the
//! image's natural size (TD-217).

use rustic_css::{CssParser, MediaEnvironment, Token};
use rustic_dom::{Document, NodeId, NodeKind};

/// One image candidate: its URL and density in image pixels per CSS pixel.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Candidate {
    pub(crate) url: String,
    pub(crate) density: f64,
}

/// What the view looks like to source selection.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ViewInfo {
    /// Viewport width in CSS pixels.
    pub(crate) width: f64,
    /// Viewport height in CSS pixels.
    pub(crate) height: f64,
    /// Device pixels per CSS pixel.
    pub(crate) ratio: f64,
}

/// A candidate's descriptor before the source size is known.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Descriptor {
    Density(f64),
    Width(f64),
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')
}

/// HTML "parse a srcset attribute": `(url, descriptor)` pairs; candidates
/// with invalid or conflicting descriptors are dropped.
fn parse_srcset(input: &str) -> Vec<(String, Option<Descriptor>)> {
    let mut out = Vec::new();
    let mut rest = input;
    loop {
        rest = rest.trim_start_matches(|c: char| is_space(c) || c == ',');
        if rest.is_empty() {
            return out;
        }
        let end = rest.find(is_space).unwrap_or(rest.len());
        let mut url = &rest[..end];
        rest = &rest[end..];
        let mut descriptors = Vec::new();
        if url.ends_with(',') {
            url = url.trim_end_matches(',');
        } else {
            // Descriptors run to the next comma outside parentheses.
            let mut depth = 0usize;
            let mut end = rest.len();
            for (i, c) in rest.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => depth = depth.saturating_sub(1),
                    ',' if depth == 0 => {
                        end = i;
                        break;
                    }
                    _ => {}
                }
            }
            descriptors.extend(rest[..end].split(is_space).filter(|d| !d.is_empty()));
            rest = rest.get(end + 1..).unwrap_or("");
        }
        let mut descriptor = None;
        let mut valid = true;
        for d in descriptors {
            let (number, unit) = d.split_at(d.len().saturating_sub(1));
            let parsed = number.parse::<f64>().ok().filter(|n| n.is_finite());
            let next = match (unit, parsed) {
                ("w", Some(n)) if n > 0.0 && n.fract() == 0.0 => Some(Descriptor::Width(n)),
                ("x", Some(n)) if n >= 0.0 => Some(Descriptor::Density(n)),
                // Height descriptors only qualify a width; ignored.
                ("h", Some(n)) if n > 0.0 => continue,
                _ => None,
            };
            match (next, descriptor) {
                (Some(next), None) => descriptor = Some(next),
                _ => valid = false,
            }
        }
        if valid && !url.is_empty() {
            out.push((url.to_owned(), descriptor));
        }
    }
}

/// A source size length in CSS pixels (`px`, `em`, `rem`, `vw`, `vh`).
fn length(value: &rustic_css::Value, view: ViewInfo) -> Option<f64> {
    match value.token()? {
        Token::Dimension(n, unit) => {
            let scale = match unit.to_ascii_lowercase().as_str() {
                "px" => 1.0,
                "em" | "rem" => 16.0,
                "vw" => view.width / 100.0,
                "vh" => view.height / 100.0,
                _ => return None,
            };
            (n.value >= 0.0).then_some(n.value * scale)
        }
        Token::Number(n) if n.value == 0.0 => Some(0.0),
        _ => None,
    }
}

/// HTML "parse a sizes attribute": the first matching source size, else
/// `100vw`.
fn source_size(sizes: Option<&str>, view: ViewInfo) -> f64 {
    let fallback = view.width;
    let Some(sizes) = sizes else {
        return fallback;
    };
    let Ok(values) = CssParser::default().component_values(sizes) else {
        return fallback;
    };
    let mut environment = MediaEnvironment::screen(view.width, view.height);
    environment.resolution = view.ratio;
    for part in values.value.split(|v| v.token() == Some(&Token::Comma)) {
        let significant: Vec<&rustic_css::Value> =
            part.iter().filter(|v| !v.is_whitespace()).collect();
        let Some((&last, condition)) = significant.split_last() else {
            continue;
        };
        let Some(size) = length(last, view) else {
            continue;
        };
        if condition.is_empty() {
            return size;
        }
        let start = part.iter().position(|v| std::ptr::eq(v, condition[0]));
        let end = part.iter().position(|v| std::ptr::eq(v, last));
        let (Some(start), Some(end)) = (start, end) else {
            continue;
        };
        if rustic_css::parse_media_query_list(&part[start..end]).matches(&environment) {
            return size;
        }
    }
    fallback
}

/// The candidates of a `srcset` (plus `src` as `1x` when no candidate uses
/// a width descriptor and none is `1x`), with densities resolved.
pub(crate) fn candidates(
    srcset: Option<&str>,
    sizes: Option<&str>,
    src: Option<&str>,
    view: ViewInfo,
) -> Vec<Candidate> {
    let parsed = srcset.map(parse_srcset).unwrap_or_default();
    let size = source_size(sizes, view).max(f64::MIN_POSITIVE);
    let widths = parsed
        .iter()
        .any(|(_, d)| matches!(d, Some(Descriptor::Width(_))));
    let mut out: Vec<Candidate> = parsed
        .into_iter()
        .map(|(url, d)| {
            let density = match d {
                Some(Descriptor::Width(w)) => w / size,
                Some(Descriptor::Density(x)) => x,
                None => 1.0,
            };
            Candidate { url, density }
        })
        .collect();
    if let Some(src) = src.filter(|s| !s.is_empty()) {
        if !widths && !out.iter().any(|c| c.density == 1.0) {
            out.push(Candidate {
                url: src.to_owned(),
                density: 1.0,
            });
        }
    }
    out
}

/// The candidate for the view: the least dense at or above the device
/// pixel ratio, else the densest.
pub(crate) fn select(candidates: &[Candidate], ratio: f64) -> Option<&Candidate> {
    let enough = candidates
        .iter()
        .filter(|c| c.density >= ratio)
        .min_by(|a, b| a.density.total_cmp(&b.density));
    enough.or_else(|| {
        candidates
            .iter()
            .max_by(|a, b| a.density.total_cmp(&b.density))
    })
}

fn html_element<'a>(
    document: &'a Document,
    id: NodeId,
    name: &str,
) -> Option<&'a rustic_dom::Element> {
    match document.node(id).ok()?.kind() {
        NodeKind::Element(e)
            if e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE)
                && e.name().local() == name =>
        {
            Some(e)
        }
        _ => None,
    }
}

fn attribute(element: &rustic_dom::Element, name: &str) -> Option<String> {
    element
        .attributes()
        .iter()
        .find(|a| a.name().namespace().is_none() && a.name().local() == name)
        .map(|a| String::from_utf16_lossy(a.value().as_units()))
}

/// Image types Rustic decodes, for `source type`.
fn supported_type(value: &str) -> bool {
    let essence = value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    matches!(
        essence.as_str(),
        "" | "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/apng"
    )
}

/// The selected URL and density for the `img` element `id`, or `None`
/// when it has no usable source.
pub(crate) fn img_source(document: &Document, id: NodeId, view: ViewInfo) -> Option<Candidate> {
    let img = html_element(document, id, "img")?;
    let mut environment = MediaEnvironment::screen(view.width, view.height);
    environment.resolution = view.ratio;
    let parent = document.node(id).ok()?.parent();
    if let Some(picture) = parent.filter(|&p| html_element(document, p, "picture").is_some()) {
        for child in document.children(picture).ok()? {
            if child == id {
                break;
            }
            let Some(source) = html_element(document, child, "source") else {
                continue;
            };
            let Some(srcset) = attribute(source, "srcset") else {
                continue;
            };
            if let Some(media) = attribute(source, "media") {
                let matches = CssParser::default()
                    .component_values(&media)
                    .map(|values| {
                        rustic_css::parse_media_query_list(&values.value).matches(&environment)
                    })
                    .unwrap_or(false);
                if !matches {
                    continue;
                }
            }
            if !attribute(source, "type").is_none_or(|t| supported_type(&t)) {
                continue;
            }
            let sizes = attribute(source, "sizes");
            let found = candidates(Some(&srcset), sizes.as_deref(), None, view);
            if let Some(chosen) = select(&found, view.ratio) {
                return Some(chosen.clone());
            }
        }
    }
    let srcset = attribute(img, "srcset");
    let sizes = attribute(img, "sizes");
    let src = attribute(img, "src");
    let found = candidates(srcset.as_deref(), sizes.as_deref(), src.as_deref(), view);
    select(&found, view.ratio).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: ViewInfo = ViewInfo {
        width: 800.0,
        height: 600.0,
        ratio: 2.0,
    };

    #[test]
    fn srcset_candidates_resolve_width_and_density_descriptors() {
        let parsed = parse_srcset(" a.png 1x, b.png 2x,c.png,  d.png 3x 4x, e,f.png 100w ");
        assert_eq!(
            parsed,
            vec![
                ("a.png".into(), Some(Descriptor::Density(1.0))),
                ("b.png".into(), Some(Descriptor::Density(2.0))),
                ("c.png".into(), None),
                // A comma inside a URL belongs to it.
                ("e,f.png".into(), Some(Descriptor::Width(100.0))),
            ]
        );
        let found = candidates(
            Some("s.png 400w, l.png 1600w"),
            Some("(max-width: 500px) 100vw, 400px"),
            None,
            VIEW,
        );
        assert_eq!(found[0].density, 1.0);
        assert_eq!(found[1].density, 4.0);
        assert_eq!(select(&found, 2.0).unwrap().url, "l.png");
        assert_eq!(select(&found, 1.0).unwrap().url, "s.png");
        // `src` joins as 1x for density sets only.
        let found = candidates(Some("hi.png 2x"), None, Some("lo.png"), VIEW);
        assert_eq!(select(&found, 1.0).unwrap().url, "lo.png");
        assert_eq!(select(&found, 3.0).unwrap().url, "hi.png");
        let narrow = ViewInfo {
            width: 300.0,
            ..VIEW
        };
        assert_eq!(
            source_size(Some("(max-width: 500px) 50vw, 400px"), narrow),
            150.0
        );
        assert_eq!(source_size(Some("bogus, 20em"), VIEW), 320.0);
        assert_eq!(source_size(None, VIEW), 800.0);
    }
}
