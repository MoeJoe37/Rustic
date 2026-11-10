//! Mutated TrueType fonts exercise the bounds-checked sfnt parser.

use crate::Target;
use rustic_text::bundled::TEST_FONTS;
use rustic_text::{Face, FontLimits, GlyphId};

pub(crate) struct SfntTarget;

/// Seeds: every bundled original test font, then its prefixes at 16-byte
/// steps (truncation inside every table).
pub(crate) fn seeds() -> impl Iterator<Item = Vec<u8>> {
    let whole = TEST_FONTS.iter().map(|(_, bytes)| bytes.to_vec());
    let prefixes = TEST_FONTS.iter().flat_map(|(_, bytes)| {
        (0..bytes.len())
            .step_by(16)
            .map(move |len| bytes[..len].to_vec())
    });
    whole.chain(prefixes).chain([
        include_bytes!("../../rustic-text/tests/fonts/shaping/RusticCFFTest.otf").to_vec(),
        include_bytes!("../../rustic-text/tests/fonts/shaping/RusticCFF2Test.otf").to_vec(),
        include_bytes!("../../rustic-text/tests/fonts/shaping/RusticCFFTest.woff").to_vec(),
        include_bytes!("../../rustic-text/tests/fonts/shaping/RusticCFFTest.woff2").to_vec(),
    ])
}

impl Target for SfntTarget {
    fn name(&self) -> &'static str {
        "text-sfnt"
    }
    fn max_len(&self) -> usize {
        8192
    }
    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"\x00\x01\x00\x00",
            b"true",
            b"OTTO",
            b"head",
            b"hhea",
            b"maxp",
            b"cmap",
            b"hmtx",
            b"loca",
            b"glyf",
            b"name",
            b"OS/2",
            b"GSUB",
            b"GPOS",
            b"CFF ",
            b"wOFF",
            b"wOF2",
            b"\xff\xff",
            b"\x00\x00",
            b"\x7f\xff",
            b"\x80\x00",
            b"\xff\xff\xff\xff",
            b"\x00\x0c",
            b"\x00\x04",
            b"\x00\x03\x00\x0a",
        ]
    }
    fn check(&self, input: &[u8]) -> Result<(), String> {
        let limits = FontLimits::default();
        let Ok(face) = Face::parse(input.to_vec(), &limits) else {
            return Ok(());
        };
        if face.families().is_empty() {
            return Err("a loaded face has no family name".into());
        }
        let ratios = face.metrics().em_ratios();
        for value in [
            ratios.x_height,
            ratios.cap_height,
            ratios.zero_advance,
            ratios.ideograph_advance,
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(format!("em ratio {value} is not finite and nonnegative"));
            }
        }
        for c in [
            ' ',
            '0',
            'H',
            'x',
            'w',
            '\u{6C34}',
            '\u{FFFF}',
            '\u{10FFFF}',
        ] {
            if let Some(glyph) = face.glyph_index(c) {
                if glyph.0 >= face.glyph_count() {
                    return Err(format!("{c:?} maps beyond the glyph count"));
                }
                face.advance(glyph)
                    .map_err(|e| format!("mapped glyph lacks an advance: {e}"))?;
            }
        }
        // Outlines of the first glyphs: errors are fine, budgets must hold.
        for g in 0..face.glyph_count().min(64) {
            if let Ok(outline) = face.outline(GlyphId(g)) {
                let points: usize = outline.contours().iter().map(Vec::len).sum();
                if points > limits.max_glyph_points {
                    return Err(format!("glyph {g} exceeds the point budget"));
                }
                if outline
                    .contours()
                    .iter()
                    .flatten()
                    .any(|p| !p.x.is_finite() || !p.y.is_finite())
                {
                    return Err(format!("glyph {g} has a non-finite point"));
                }
            }
        }
        // Exercise layout tables from accepted mutated fonts as well.
        let mut database = rustic_text::FontDatabase::new();
        if database.load(input.to_vec(), &limits).is_ok() {
            let options = rustic_text::shape::ShapeOptions {
                size: 16.0,
                rtl: false,
                letter_spacing: 0.0,
                word_spacing: 0.0,
            };
            let _ = rustic_text::shape::shape(
                &database,
                &[0],
                &['f', 'i', 'A', 'V', 'x', '\u{301}'],
                &options,
            );
        }
        Ok(())
    }
}
