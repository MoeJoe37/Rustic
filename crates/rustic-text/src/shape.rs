//! Bounded OpenType shaping: grapheme clusters, per-cluster font fallback,
//! horizontal or vertical (top-to-bottom) advances, bidi mirroring and CSS
//! letter/word spacing.
//!
//! HarfRust provides GSUB/GPOS and complex-script algorithms behind an
//! adapter. Rustic owns Unicode 17 grapheme boundaries, fallback, source
//! mapping, spacing, limits and cancellation (TD-86).

use std::ops::Range;

use crate::database::{FaceQuery, FamilyRef, FontDatabase, best_face};
use crate::error::FontError;
use crate::face::{Face, GlyphId};
use crate::segment::grapheme_boundaries;
use crate::unicode::mirrored;
use rustic_base::CancellationToken;

/// One glyph of a cluster, with its advance in CSS pixels.
///
/// In a vertical run ([`ShapeOptions::vertical`]) the pen moves down the
/// line and sits on the glyph's vertical origin (the center of its top);
/// the offsets then lead from that origin to the glyph's horizontal origin
/// (usually minus half the horizontal advance and minus the ascent).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    /// Glyph index in the cluster's face (0 is `.notdef`).
    pub glyph: GlyphId,
    /// Advance along the line in CSS pixels (downward in a vertical run).
    pub advance: f64,
    /// Horizontal offset from the pen, in CSS pixels.
    pub x_offset: f64,
    /// Vertical offset upward from the pen, in CSS pixels.
    pub y_offset: f64,
}

/// One extended grapheme cluster rendered with one face.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapedCluster {
    /// Character range in the shaped text.
    pub range: Range<usize>,
    /// Index of the face in [`FontDatabase::faces`].
    pub face: usize,
    /// Glyphs in visual order within the cluster; invisible characters have none.
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
    /// Shape top to bottom for upright vertical text: vertical advances
    /// (`vmtx`, else the ascent plus descent) and the `vert`/`vrt2`
    /// alternates. `rtl` is ignored.
    pub vertical: bool,
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
        // Additional loaded families participate in cluster fallback.
        for face in self.faces() {
            if chain
                .iter()
                .any(|&i| self.faces()[i].family().eq_ignore_ascii_case(face.family()))
            {
                continue;
            }
            push(
                best_face(self.family_faces(face.family()), query),
                &mut chain,
            );
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
/// Public input and output bounds for one shaping operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShapeLimits {
    /// Most Unicode scalars accepted.
    pub max_chars: usize,
    /// Most glyphs returned.
    pub max_glyphs: usize,
}
impl Default for ShapeLimits {
    fn default() -> Self {
        Self {
            max_chars: 65_536,
            max_glyphs: 262_144,
        }
    }
}
/// Neighboring source text used for joining at style/node boundaries.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShapeContext<'a> {
    /// Text before this run, in logical order.
    pub before: &'a [char],
    /// Text after this run, in logical order.
    pub after: &'a [char],
}
/// Shape a run with default limits and no neighboring context.
pub fn shape(
    database: &FontDatabase,
    chain: &[usize],
    text: &[char],
    options: &ShapeOptions,
) -> Result<Vec<ShapedCluster>, ShapeError> {
    shape_with_limits(
        database,
        chain,
        text,
        options,
        &ShapeLimits::default(),
        &CancellationToken::default(),
    )
}
/// Shape with explicit resource and cancellation limits.
pub fn shape_with_limits(
    database: &FontDatabase,
    chain: &[usize],
    text: &[char],
    options: &ShapeOptions,
    limits: &ShapeLimits,
    cancel: &CancellationToken,
) -> Result<Vec<ShapedCluster>, ShapeError> {
    shape_with_context(
        database,
        chain,
        text,
        options,
        ShapeContext::default(),
        limits,
        cancel,
    )
}
/// Shape with neighboring context, retaining original local source indexes.
pub fn shape_with_context(
    database: &FontDatabase,
    chain: &[usize],
    text: &[char],
    options: &ShapeOptions,
    context: ShapeContext<'_>,
    limits: &ShapeLimits,
    cancel: &CancellationToken,
) -> Result<Vec<ShapedCluster>, ShapeError> {
    cancel.check().map_err(|_| ShapeError::Cancelled)?;
    if text.len() > limits.max_chars {
        return Err(ShapeError::LimitExceeded);
    }
    if !options.size.is_finite()
        || options.size < 0.0
        || !options.letter_spacing.is_finite()
        || !options.word_spacing.is_finite()
    {
        return Err(ShapeError::InvalidOptions);
    }
    let faces: Vec<_> = chain
        .iter()
        .filter_map(|&i| database.face(i).map(|f| (i, f)))
        .collect();
    let boundaries = grapheme_boundaries(text);
    let mut owner = vec![0; text.len()];
    let mut assignments = Vec::new();
    let mut start = 0;
    let mut script = harfrust::Script::COMMON;
    for end in 1..=text.len() {
        if !boundaries[end] {
            continue;
        }
        owner[start..end].fill(start);
        let visible: Vec<_> = text[start..end]
            .iter()
            .copied()
            .filter(|&c| !is_invisible(c))
            .collect();
        let mapped = |f: &Face, c: char| {
            f.glyph_index(c).is_some()
                || (options.rtl && mirrored(c).is_some_and(|m| f.glyph_index(m).is_some()))
        };
        let face = faces
            .iter()
            .find(|(_, f)| visible.iter().all(|&c| mapped(f, c)))
            .or_else(|| {
                faces
                    .iter()
                    .find(|(_, f)| visible.first().is_some_and(|&c| mapped(f, c)))
            })
            .or(faces.first());
        if face.is_none() && !visible.is_empty() {
            return Err(ShapeError::NoFont);
        }
        let current = text[start..end]
            .iter()
            .map(|&c| harfrust::unicode::script(u32::from(c)))
            .find(|s| *s != harfrust::Script::COMMON && *s != harfrust::Script::INHERITED);
        if let Some(s) = current {
            script = s;
        }
        assignments.push((start, end, face.map_or(usize::MAX, |(i, _)| *i), script));
        start = end;
    }
    // Resolve leading common/inherited clusters from the following script.
    if let Some(s) = assignments
        .iter()
        .find(|a| a.3 != harfrust::Script::COMMON)
        .map(|a| a.3)
    {
        for a in &mut assignments {
            if a.3 != harfrust::Script::COMMON {
                break;
            }
            a.3 = s;
        }
    }
    let breaks = crate::segment::line_breaks(text);
    let mut result = Vec::new();
    let mut a = 0;
    let mut glyph_count = 0usize;
    while a < assignments.len() {
        cancel.check().map_err(|_| ShapeError::Cancelled)?;
        let (start, _, index, script) = assignments[a];
        let mut b = a + 1;
        while b < assignments.len() && assignments[b].2 == index && assignments[b].3 == script {
            b += 1;
        }
        if assignments[b - 1].1 - start > 4096 {
            b = (a + 1..b)
                .rev()
                .find(|&k| {
                    assignments[k - 1].1 - start <= 4096
                        && breaks[assignments[k - 1].1]
                            != crate::segment::BreakOpportunity::Prohibited
                })
                .ok_or(ShapeError::LimitExceeded)?;
        }
        let end = assignments[b - 1].1;
        if index == usize::MAX {
            for &(s, e, _, _) in &assignments[a..b] {
                result.push(ShapedCluster {
                    range: s..e,
                    face: index,
                    glyphs: Vec::new(),
                    advance: 0.0,
                });
            }
            a = b;
            continue;
        }
        let face = database.face(index).ok_or(ShapeError::NoFont)?;
        let font = face.shaping_font().ok_or(ShapeError::OpenType)?;
        let shaper = harfrust::ShaperFont::new(font);
        let mut buffer = harfrust::Buffer::new();
        for (i, &c) in text[start..end].iter().enumerate() {
            // C0/C1 and paragraph separators have no glyph in the public
            // contract. A zero-width boundary retains their shaping position.
            let c = if matches!(u32::from(c),0..=0x1F|0x7F..=0x9F|0x2028..=0x2029) {
                '\u{200B}'
            } else {
                c
            };
            buffer.push(u32::from(c), i as u32);
        }
        buffer.set_direction(if options.vertical {
            harfrust::Direction::TopToBottom
        } else if options.rtl {
            harfrust::Direction::RightToLeft
        } else {
            harfrust::Direction::LeftToRight
        });
        buffer.set_script(Some(script));
        buffer.set_flags(harfrust::BufferFlags::REMOVE_DEFAULT_IGNORABLES);
        let pre = if start == 0 {
            context.before
        } else {
            &text[..start]
        };
        let post = if end == text.len() {
            context.after
        } else {
            &text[end..]
        };
        buffer.set_pre_context_codepoints(
            &pre[pre.len().saturating_sub(8)..]
                .iter()
                .map(|&c| u32::from(c))
                .collect::<Vec<_>>(),
        );
        buffer.set_post_context_codepoints(
            &post[..post.len().min(8)]
                .iter()
                .map(|&c| u32::from(c))
                .collect::<Vec<_>>(),
        );
        let features: Vec<_> = if options.letter_spacing != 0.0 {
            [b"liga", b"clig", b"dlig", b"hlig"]
                .iter()
                .map(|t| harfrust::Feature::new(harfrust::Tag::new(t), 0, ..))
                .collect()
        } else {
            Vec::new()
        };
        harfrust::shape(
            &shaper,
            &mut buffer,
            harfrust::ShapeOptions::new().features(&features),
        )
        .map_err(|_| ShapeError::OpenType)?;
        cancel.check().map_err(|_| ShapeError::Cancelled)?;
        glyph_count = glyph_count
            .checked_add(buffer.len())
            .filter(|&n| n <= limits.max_glyphs)
            .ok_or(ShapeError::LimitExceeded)?;
        let mut grouped = std::collections::BTreeMap::<usize, Vec<ShapedGlyph>>::new();
        grouped.entry(start).or_default();
        for &(s, e, _, _) in &assignments[a..b] {
            if text[s..e].iter().all(|&c| is_invisible(c)) {
                grouped.entry(s).or_default();
            }
        }
        let scale = options.size / f64::from(face.units_per_em());
        for (g, pos) in buffer.glyph_infos().iter().zip(buffer.glyph_positions()) {
            if g.glyph_id >= u32::from(face.glyph_count()) {
                return Err(ShapeError::OpenType);
            }
            let mut source = start + g.cluster as usize;
            if source >= end {
                return Err(ShapeError::OpenType);
            }
            if is_invisible(text[source]) {
                source = (source..end)
                    .find(|&i| !is_invisible(text[i]))
                    .unwrap_or(source);
            }
            let source = owner[source];
            let mut advance = if options.vertical {
                -f64::from(pos.y_advance) * scale
            } else {
                f64::from(pos.x_advance) * scale
            };
            if is_word_separator(text[source]) {
                advance += options.word_spacing;
            }
            let glyph = ShapedGlyph {
                glyph: GlyphId(g.glyph_id as u16),
                advance,
                x_offset: f64::from(pos.x_offset) * scale,
                y_offset: f64::from(pos.y_offset) * scale,
            };
            if [glyph.advance, glyph.x_offset, glyph.y_offset]
                .iter()
                .any(|v| !v.is_finite())
            {
                return Err(ShapeError::InvalidOptions);
            }
            grouped.entry(source).or_default().push(glyph);
        }
        let keys: Vec<_> = grouped.keys().copied().collect();
        for (i, (&source, glyphs)) in grouped.iter().enumerate() {
            let end = keys.get(i + 1).copied().unwrap_or(end);
            let advance = glyphs.iter().map(|g| g.advance).sum::<f64>()
                + if glyphs.is_empty() {
                    0.0
                } else {
                    options.letter_spacing
                };
            if !advance.is_finite() {
                return Err(ShapeError::InvalidOptions);
            }
            result.push(ShapedCluster {
                range: source..end,
                face: index,
                glyphs: glyphs.clone(),
                advance,
            });
        }
        a = b;
    }
    Ok(result)
}

/// A shaping failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShapeError {
    /// Visible text with no face to draw it.
    NoFont,
    /// Non-finite options or output.
    InvalidOptions,
    /// Backend rejected malformed layout data.
    OpenType,
    /// Input, output or bounded backend run budget exhausted.
    LimitExceeded,
    /// Cooperative cancellation was requested.
    Cancelled,
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
            Self::InvalidOptions => f.write_str("invalid shaping options"),
            Self::OpenType => f.write_str("OpenType shaping failed"),
            Self::LimitExceeded => f.write_str("shaping limit exceeded"),
            Self::Cancelled => f.write_str("shaping cancelled"),
            Self::NoFont => f.write_str("no font is available for visible text"),
            Self::Font(e) => write!(f, "shaping failed: {e}"),
        }
    }
}

impl std::error::Error for ShapeError {}
