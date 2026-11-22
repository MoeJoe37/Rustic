//! Reftests: render a test page and its references and compare the pixels.
//!
//! A test names its references with `<link rel=match href>` (must look the
//! same) and `<link rel=mismatch href>` (must look different), as in the
//! web-platform-tests reftest format. Every reference must hold. Pixel
//! tolerance comes from `<meta name=fuzzy content>` in the test:
//! `maxDifference=A;totalPixels=B` or the short `A;B` form, where each value
//! is a number `N` (meaning `0-N`) or an inclusive range `N-M`. A content
//! prefix `href:` limits the tolerance to the reference with exactly that
//! `href`. References load under the same local-only policy as stylesheets:
//! only files in the test's directory or below it. Reference chains (a
//! reference's own `rel=match` links) are not followed.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use rustic_dom::{Document, HTML_NAMESPACE};
use rustic_style::{DomElement, ElementView};
use rustic_url::Url;

use crate::stylesheets::{file_url, url_path};

/// An inclusive range of allowed values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Range {
    /// Smallest allowed value.
    pub min: u64,
    /// Largest allowed value.
    pub max: u64,
}

impl Range {
    const ZERO: Self = Self { min: 0, max: 0 };

    fn contains(self, value: u64) -> bool {
        (self.min..=self.max).contains(&value)
    }

    fn parse(text: &str) -> Option<Self> {
        let number = |s: &str| {
            let s = s.trim();
            (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                .then(|| s.parse::<u64>().ok())
                .flatten()
        };
        let (min, max) = match text.split_once('-') {
            Some((min, max)) => (number(min)?, number(max)?),
            None => (0, number(text)?),
        };
        (min <= max).then_some(Self { min, max })
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.min, self.max)
    }
}

/// Allowed differences between a test and a reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fuzzy {
    /// Allowed largest per-channel difference.
    pub max_difference: Range,
    /// Allowed count of differing pixels.
    pub total_pixels: Range,
}

impl Fuzzy {
    /// Exact match: no pixel may differ.
    pub const EXACT: Self = Self {
        max_difference: Range::ZERO,
        total_pixels: Range::ZERO,
    };

    /// Whether a difference is within the tolerance. Identical images are
    /// always within it, whatever the ranges say.
    pub fn allows(self, difference: rustic_raster::Difference) -> bool {
        difference.pixels == 0
            || (self
                .max_difference
                .contains(u64::from(difference.max_channel))
                && self.total_pixels.contains(difference.pixels))
    }

    /// Parse a fuzzy `content` value, returning the optional reference
    /// `href` it applies to.
    pub fn parse(content: &str) -> Option<(Option<String>, Self)> {
        let (href, values) = match content.split_once(':') {
            Some((href, values)) => (Some(href.trim().to_owned()), values),
            None => (None, content),
        };
        let mut parts = values.split(';');
        let (first, second) = (parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        let value = |part: &str, name: &str| {
            let part = part.trim();
            match part.split_once('=') {
                Some((key, value)) if key.trim() == name => Range::parse(value),
                Some(_) => None,
                None => Range::parse(part),
            }
        };
        Some((
            href,
            Self {
                max_difference: value(first, "maxDifference")?,
                total_pixels: value(second, "totalPixels")?,
            },
        ))
    }
}

/// Whether a reference must look the same as the test or different.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Relation {
    /// `rel=match`.
    Match,
    /// `rel=mismatch`.
    Mismatch,
}

impl fmt::Display for Relation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Match => "match",
            Self::Mismatch => "mismatch",
        })
    }
}

/// One reference named by a test.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reference {
    /// Match or mismatch.
    pub relation: Relation,
    /// The `href` as written.
    pub href: String,
    /// The local file it resolves to, or why it does not.
    pub path: Result<PathBuf, &'static str>,
    /// The tolerance for this reference.
    pub fuzzy: Fuzzy,
}

/// The references and tolerances of a parsed test page.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Manifest {
    /// References in tree order.
    pub references: Vec<Reference>,
    /// `<meta name=fuzzy>` values that could not be parsed.
    pub invalid_fuzzy: Vec<String>,
    /// Read in a renderer: the broker serves (and checks) each reference.
    pub confined: bool,
}

pub(crate) fn attribute(element: DomElement<'_>, name: &str) -> Option<String> {
    element
        .attributes()
        .find(|a| a.namespace.is_none() && a.local == name)
        .map(|a| String::from_utf16_lossy(a.value))
}

pub(crate) fn is_html(element: DomElement<'_>, name: &str) -> bool {
    element.local_name() == name && element.namespace() == Some(HTML_NAMESPACE)
}

fn link_relation(rel: &str) -> Option<Relation> {
    let tokens = || rel.split(['\t', '\n', '\x0c', '\r', ' ']);
    if tokens().any(|t| t.eq_ignore_ascii_case("match")) {
        Some(Relation::Match)
    } else if tokens().any(|t| t.eq_ignore_ascii_case("mismatch")) {
        Some(Relation::Mismatch)
    } else {
        None
    }
}

/// Read the references of `document`, a test loaded from the canonical
/// file `test`. Each `href` resolves against the document base URL (the
/// first `<base href>`, else the test's `file:` URL) and must name a file
/// inside the test's directory once symbolic links resolve. A `confined`
/// renderer has no file system: it checks containment lexically and the
/// broker, which serves the reference, checks it again after symbolic
/// links resolve (TD-171).
pub fn manifest(document: &Document, test: &Path, confined: bool) -> Manifest {
    let document_url = file_url(test);
    let root = test.parent().map(Path::to_path_buf);
    let mut base = None;
    let mut links = Vec::new();
    let mut fuzzy = Vec::new();
    let mut manifest = Manifest {
        confined,
        ..Manifest::default()
    };
    let Ok(walk) = document.walk(document.root()) else {
        return manifest;
    };
    for (id, _) in walk {
        let Some(element) = DomElement::new(document, id) else {
            continue;
        };
        if is_html(element, "base") && base.is_none() {
            if let Some(href) = attribute(element, "href") {
                base = Some(Url::parse(&href, document_url.as_ref()).ok());
            }
        } else if is_html(element, "link") {
            let relation = attribute(element, "rel").and_then(|rel| link_relation(&rel));
            if let (Some(relation), Some(href)) = (relation, attribute(element, "href")) {
                links.push((relation, href));
            }
        } else if is_html(element, "meta")
            && attribute(element, "name").is_some_and(|n| n.eq_ignore_ascii_case("fuzzy"))
        {
            if let Some(content) = attribute(element, "content") {
                match Fuzzy::parse(&content) {
                    Some(value) => fuzzy.push(value),
                    None => manifest.invalid_fuzzy.push(content),
                }
            }
        }
    }
    // A `<base>` whose href does not parse leaves the document URL in place.
    let base = base.flatten().or(document_url);
    for (relation, href) in links {
        // A tolerance for this exact href wins over a general one; later
        // entries win over earlier ones.
        let tolerance = fuzzy
            .iter()
            .rev()
            .find(|(target, _)| target.as_deref() == Some(href.as_str()))
            .or_else(|| fuzzy.iter().rev().find(|(target, _)| target.is_none()))
            .map_or(Fuzzy::EXACT, |(_, value)| *value);
        let path = if confined {
            resolve_lexical(&href, base.as_ref(), root.as_deref())
        } else {
            resolve(&href, base.as_ref(), root.as_deref())
        };
        manifest.references.push(Reference {
            relation,
            href,
            path,
            fuzzy: tolerance,
        });
    }
    manifest
}

/// Resolve `href` to a canonical file inside `root` (lexically, then after
/// symbolic links resolve), or say why it cannot be read.
pub(crate) fn resolve(
    href: &str,
    base: Option<&Url>,
    root: Option<&Path>,
) -> Result<PathBuf, &'static str> {
    let path = resolve_lexical(href, base, root)?;
    let root = root.ok_or("test has no directory")?;
    // Lexical containment first, then again after symbolic links resolve.
    let canonical = fs::canonicalize(&path).map_err(|_| "not found")?;
    if !canonical.starts_with(root) {
        return Err("outside the test's directory");
    }
    Ok(canonical)
}

/// Resolve `href` to a file inside `root` by its path alone, before any
/// symbolic link resolves.
pub(crate) fn resolve_lexical(
    href: &str,
    base: Option<&Url>,
    root: Option<&Path>,
) -> Result<PathBuf, &'static str> {
    let url = Url::parse(href, base).map_err(|_| "invalid URL")?;
    let path = url_path(&url).ok_or("not a local file")?;
    let root = root.ok_or("test has no directory")?;
    let url_root = file_url(root).and_then(|url| url_path(&url));
    if !url_root.is_some_and(|url_root| path.starts_with(url_root)) {
        return Err("outside the test's directory");
    }
    Ok(path)
}

/// The verdict for one reference given the measured difference.
pub fn passes(reference: &Reference, difference: Option<rustic_raster::Difference>) -> bool {
    let same = difference.is_some_and(|d| reference.fuzzy.allows(d));
    match reference.relation {
        Relation::Match => same,
        Relation::Mismatch => !same,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(min: u64, max: u64) -> Range {
        Range { min, max }
    }

    #[test]
    fn parses_fuzzy_forms() {
        let fuzzy = |max_difference, total_pixels| Fuzzy {
            max_difference,
            total_pixels,
        };
        assert_eq!(
            Fuzzy::parse("maxDifference=1-2;totalPixels=0-300"),
            Some((None, fuzzy(range(1, 2), range(0, 300))))
        );
        assert_eq!(
            Fuzzy::parse(" 4 ; 10-20 "),
            Some((None, fuzzy(range(0, 4), range(10, 20))))
        );
        assert_eq!(
            Fuzzy::parse("ref.html:maxDifference=3;totalPixels=5"),
            Some((Some("ref.html".into()), fuzzy(range(0, 3), range(0, 5))))
        );
        for bad in [
            "",
            "4",
            "1;2;3",
            "2-1;3",
            "maxDifference=;totalPixels=1",
            "totalPixels=1;maxDifference=1",
            "-1;2",
            "1;+2",
            "99999999999999999999;1",
        ] {
            assert_eq!(Fuzzy::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn tolerance_and_relations() {
        use rustic_raster::Difference;
        let diff = |pixels, max_channel| {
            Some(Difference {
                pixels,
                max_channel,
            })
        };
        let reference = |relation, fuzzy| Reference {
            relation,
            href: String::new(),
            path: Err("unused"),
            fuzzy,
        };
        let exact = reference(Relation::Match, Fuzzy::EXACT);
        assert!(passes(&exact, diff(0, 0)));
        assert!(!passes(&exact, diff(1, 1)));
        assert!(!passes(&exact, None));
        let loose = Fuzzy {
            max_difference: range(2, 4),
            total_pixels: range(1, 10),
        };
        let fuzzy = reference(Relation::Match, loose);
        assert!(passes(&fuzzy, diff(0, 0)));
        assert!(passes(&fuzzy, diff(10, 4)));
        assert!(!passes(&fuzzy, diff(10, 1)));
        assert!(!passes(&fuzzy, diff(11, 3)));
        let mismatch = reference(Relation::Mismatch, loose);
        assert!(!passes(&mismatch, diff(0, 0)));
        assert!(!passes(&mismatch, diff(5, 3)));
        assert!(passes(&mismatch, diff(5, 200)));
        assert!(passes(&mismatch, None));
    }

    #[test]
    fn link_relations_are_token_lists() {
        assert_eq!(link_relation("MATCH"), Some(Relation::Match));
        assert_eq!(link_relation("help\tmismatch"), Some(Relation::Mismatch));
        assert_eq!(link_relation("matches"), None);
    }
}
