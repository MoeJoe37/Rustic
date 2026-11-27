//! `@font-face` loading for layout and paint (CSS Fonts 4 §4).
//!
//! The document's applicable `@font-face` rules are read in cascade order.
//! For each rule the first `src` entry that yields a font is used: a
//! `url()` fetched under the same policy as images (local files in the
//! document's directory or below, or the network session, as a CORS-mode
//! `font` request), or a `local()` name matched against the installed
//! families. The face then answers to the rule's family, style ranges and
//! `unicode-range` only. Fonts load before the first layout, as if every
//! rule had `font-display: block` with an unbounded block period (TD-212);
//! a rule whose sources all fail is skipped and text falls back.

use std::collections::HashMap;
use std::sync::Arc;

use rustic_style::{FontFaceRule, FontFaceSource, FontFaceStyle};
use rustic_text::{Face, FaceDescriptors, FaceStyle, FontDatabase, FontLimits};
use rustic_url::Url;

/// Budgets for the web fonts of one document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WebFontLimits {
    /// Distinct font resources fetched; later ones are skipped.
    pub(crate) max_fonts: usize,
    /// Total encoded bytes fetched across all fonts.
    pub(crate) max_bytes: usize,
}

impl Default for WebFontLimits {
    fn default() -> Self {
        Self {
            max_fonts: 64,
            max_bytes: 32 * 1024 * 1024,
        }
    }
}

fn style(style: FontFaceStyle) -> FaceStyle {
    match style {
        FontFaceStyle::Normal => FaceStyle::Normal,
        FontFaceStyle::Italic => FaceStyle::Italic,
        FontFaceStyle::Oblique => FaceStyle::Oblique,
    }
}

/// Parse a fetched font: the first face of a collection, else the font.
fn parse(bytes: Vec<u8>, limits: &FontLimits) -> Option<Face> {
    let bytes = match rustic_text::collection_len(&bytes) {
        Some(_) => rustic_text::collection_face(&bytes, 0, limits).ok()?,
        None => bytes,
    };
    Face::parse(bytes, limits).ok()
}

/// `base` plus the faces of `rules`, or `None` when no rule loads a face.
/// `fetch(url, max_bytes)` returns a resource's bytes.
pub(crate) fn database(
    rules: &[Arc<FontFaceRule>],
    base: &FontDatabase,
    limits: WebFontLimits,
    mut fetch: impl FnMut(&Url, usize) -> Option<Vec<u8>>,
) -> Option<FontDatabase> {
    let font_limits = FontLimits::default();
    let mut fetched: HashMap<String, Option<Face>> = HashMap::new();
    let mut spent = 0usize;
    let mut faces = Vec::new();
    for rule in rules {
        let mut face = None;
        for source in &rule.sources {
            face = match source {
                FontFaceSource::Url { url: Some(url), .. } => {
                    let key = url.serialize();
                    if let Some(known) = fetched.get(&key) {
                        known.clone()
                    } else if fetched.len() >= limits.max_fonts {
                        None
                    } else {
                        let loaded = fetch(url, limits.max_bytes - spent).and_then(|bytes| {
                            spent += bytes.len().min(limits.max_bytes - spent);
                            parse(bytes, &font_limits)
                        });
                        fetched.insert(key, loaded.clone());
                        loaded
                    }
                }
                FontFaceSource::Url { url: None, .. } => None,
                // Installed faces are matched by family name, not by their
                // full or PostScript names (TD-212).
                FontFaceSource::Local(name) => base
                    .faces()
                    .iter()
                    .find(|f| {
                        !f.is_web() && f.families().iter().any(|n| n.eq_ignore_ascii_case(name))
                    })
                    .cloned(),
            };
            if face.is_some() {
                break;
            }
        }
        if let Some(face) = face {
            faces.push(face.with_descriptors(&FaceDescriptors {
                family: rule.family.to_string(),
                weight: rule.weight,
                style: style(rule.style),
                stretch: rule.stretch,
                unicode_range: rule.unicode_range.clone(),
            }));
        }
    }
    if faces.is_empty() {
        return None;
    }
    let mut database = base.clone();
    // The last matching rule wins, and equal web faces keep insertion order.
    for face in faces.into_iter().rev() {
        database.add_web_face(face);
    }
    Some(database)
}
