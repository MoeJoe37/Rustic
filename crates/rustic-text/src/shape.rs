//! Simple shaping: grapheme clusters, per-cluster font fallback, horizontal
//! advances, bidi mirroring and CSS letter/word spacing.
//!
//! Each character maps through its face's `cmap` to one glyph with the
//! `hmtx` advance. There is no OpenType layout (GSUB/GPOS/`kern`), so
//! ligatures, contextual forms, mark positioning and complex scripts are not
//! shaped (TD-50). Clusters are UAX #29 extended grapheme clusters and never
//! split across faces.

use std::ops::Range;

use crate::database::{FaceQuery, FamilyRef, FontDatabase, best_face};
use crate::error::FontError;
use crate::face::{Face, GlyphId};
use crate::segment::grapheme_boundaries;
use crate::unicode::mirrored;

/// One glyph of a cluster, with its advance in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    /// Glyph index in the cluster's face (0 is `.notdef`).
    pub glyph: GlyphId,
    /// Horizontal advance in CSS pixels.
    pub advance: f64,
}

/// One extended grapheme cluster rendered with one face.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapedCluster {
    /// Character range in the shaped text.
    pub range: Range<usize>,
    /// Index of the face in [`FontDatabase::faces`].
    pub face: usize,
    /// Glyphs in logical order; invisible characters have none.
    pub glyphs: Vec<ShapedGlyph>,
    /// Total advance in CSS pixels, spacing included.
    pub advance: f64,
}

/// How to shape one run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeOptions {
    /// Font size in CSS pixels.
    pub size: f64,
    /// The run is at an odd bidi level, so mirrored characters use their
    /// `Bidi_Mirroring_Glyph` when the face has it (UAX #9 L4).
    pub rtl: bool,
    /// CSS `letter-spacing` in pixels, added after each visible cluster.
    pub letter_spacing: f64,
    /// CSS `word-spacing` in pixels, added to each word separator.
    pub word_spacing: f64,
}

/// Default-ignorable and control characters that draw nothing.
pub fn is_invisible(c: char) -> bool {
    matches!(
        u32::from(c),
        0x00..=0x1F
            | 0x7F..=0x9F
            | 0xAD
            | 0x34F
            | 0x61C
            | 0x115F..=0x1160
            | 0x17B4..=0x17B5
            | 0x180B..=0x180F
            | 0x200B..=0x200F
            | 0x2028..=0x202E
            | 0x2060..=0x206F
            | 0x3164
            | 0xFE00..=0xFE0F
            | 0xFEFF
            | 0xFFA0
            | 0xFFF0..=0xFFF8
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0000..=0xE0FFF
    )
}

/// CSS Text 3 word-separator characters, which receive `word-spacing`.
pub fn is_word_separator(c: char) -> bool {
    matches!(
        c,
        ' ' | '\u{A0}' | '\u{1361}' | '\u{10100}' | '\u{10101}' | '\u{1039F}' | '\u{1091F}'
    )
}

impl FontDatabase {
    /// The face of `index`.
    pub fn face(&self, index: usize) -> Option<&Face> {
        self.faces().get(index)
    }

    /// Face indexes to try in order for a CSS family list: the best face of
    /// each listed family that has faces, then of the fallback family. The
    /// first entry is the *first available font*. Duplicates are removed.
    pub fn fallback_chain<'a>(
        &self,
        families: impl IntoIterator<Item = FamilyRef<'a>>,
        query: &FaceQuery,
    ) -> Vec<usize> {
        let mut chain: Vec<usize> = Vec::new();
        let push = |face: Option<&Face>, chain: &mut Vec<usize>| {
            if let Some(face) = face {
                let index = self
                    .faces()
                    .iter()
                    .position(|f| std::ptr::eq(f, face))
                    .unwrap_or(usize::MAX);
                if index != usize::MAX && !chain.contains(&index) {
                    chain.push(index);
                }
            }
        };
        for family in families {
            let name = match family {
                FamilyRef::Named(name) => Some(name),
                FamilyRef::Generic(generic) => self.generic_family(generic),
            };
            if let Some(name) = name {
                push(best_face(self.family_faces(name), query), &mut chain);
            }
        }
        if let Some(fallback) = self.fallback_family() {
            push(best_face(self.family_faces(fallback), query), &mut chain);
        }
        chain
    }
}

/// Shape `text` with the faces of `chain` (indexes into `database`).
///
/// Each grapheme cluster uses the first face that maps all its visible
/// characters, else the first that maps its first visible character, else
/// the chain's first face with `.notdef`. With an empty chain only
/// invisible text can be shaped; visible text is [`ShapeError::NoFont`].
pub fn shape(
    database: &FontDatabase,
    chain: &[usize],
    text: &[char],
    options: &ShapeOptions,
) -> Result<Vec<ShapedCluster>, ShapeError> {
    let faces: Vec<(usize, &Face)> = chain
        .iter()
        .filter_map(|&i| database.face(i).map(|f| (i, f)))
        .collect();
    let boundaries = grapheme_boundaries(text);
    let mut clusters = Vec::new();
    let mut start = 0;
    for end in 1..=text.len() {
        if !boundaries[end] {
            continue;
        }
        let chars = &text[start..end];
        let visible: Vec<char> = chars
            .iter()
            .copied()
            .filter(|&c| !is_invisible(c))
            .collect();
        let mapped = |face: &Face, c: char| -> bool {
            face.glyph_index(c).is_some()
                || (options.rtl && mirrored(c).is_some_and(|m| face.glyph_index(m).is_some()))
        };
        let chosen = faces
            .iter()
            .find(|(_, f)| visible.iter().all(|&c| mapped(f, c)))
            .or_else(|| {
                faces
                    .iter()
                    .find(|(_, f)| visible.first().is_some_and(|&c| mapped(f, c)))
            })
            .or(faces.first());
        let Some(&(index, face)) = chosen else {
            if visible.is_empty() {
                clusters.push(ShapedCluster {
                    range: start..end,
                    face: usize::MAX,
                    glyphs: Vec::new(),
                    advance: 0.0,
                });
                start = end;
                continue;
            }
            return Err(ShapeError::NoFont);
        };
        let scale = options.size / f64::from(face.units_per_em());
        let mut glyphs = Vec::with_capacity(visible.len());
        let mut advance = 0.0;
        for &c in &visible {
            let mirror = options
                .rtl
                .then(|| mirrored(c))
                .flatten()
                .and_then(|m| face.glyph_index(m));
            let glyph = mirror.or_else(|| face.glyph_index(c)).unwrap_or(GlyphId(0));
            let mut width = f64::from(face.advance(glyph)?) * scale;
            if is_word_separator(c) {
                width += options.word_spacing;
            }
            advance += width;
            glyphs.push(ShapedGlyph {
                glyph,
                advance: width,
            });
        }
        if !visible.is_empty() {
            advance += options.letter_spacing;
        }
        clusters.push(ShapedCluster {
            range: start..end,
            face: index,
            glyphs,
            advance,
        });
        start = end;
    }
    Ok(clusters)
}

/// A shaping failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShapeError {
    /// Visible text with no face to draw it.
    NoFont,
    /// A face's tables failed while reading a glyph.
    Font(FontError),
}

impl From<FontError> for ShapeError {
    fn from(value: FontError) -> Self {
        Self::Font(value)
    }
}

impl std::fmt::Display for ShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoFont => f.write_str("no font is available for visible text"),
            Self::Font(e) => write!(f, "shaping failed: {e}"),
        }
    }
}

impl std::error::Error for ShapeError {}
