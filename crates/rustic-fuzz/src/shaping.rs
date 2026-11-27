//! Mutated Unicode text preserves grapheme ownership and finite glyphs.
use crate::Target;
use rustic_text::{
    FontDatabase, FontLimits,
    segment::grapheme_boundaries,
    shape::{ShapeOptions, shape},
};
use std::sync::OnceLock;
pub(crate) struct ShapingTarget;
fn fonts() -> &'static FontDatabase {
    static FONTS: OnceLock<FontDatabase> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut d = rustic_text::bundled::test_database();
        for bytes in [
            include_bytes!("../../rustic-text/tests/fonts/shaping/RusticCFFTest.otf").as_slice(),
            include_bytes!("../../rustic-text/tests/fonts/shaping/Arabic.ttf").as_slice(),
            include_bytes!("../../rustic-text/tests/fonts/shaping/Devanagari.ttf").as_slice(),
        ] {
            d.load(bytes.to_vec(), &FontLimits::default())
                .expect("pinned test font");
        }
        d
    })
}
/// Every committed Unicode 17 grapheme case becomes a shaping seed.
pub(crate) fn seeds() -> Vec<Vec<u8>> {
    include_str!("../../rustic-text/tests/fixtures/grapheme_cases.txt")
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|line| {
            let first = line.split(';').next()?;
            let text: String = first
                .split_whitespace()
                .filter_map(|s| u32::from_str_radix(s, 16).ok().and_then(char::from_u32))
                .collect();
            (!text.is_empty()).then(|| text.into_bytes())
        })
        .collect()
}
impl Target for ShapingTarget {
    fn name(&self) -> &'static str {
        "text-shaping"
    }
    fn max_len(&self) -> usize {
        4096
    }
    fn dictionary(&self) -> &'static [&'static [u8]] {
        const DICTIONARY: &[&[u8]] = &[
            b"fi",
            b"AV",
            "بب".as_bytes(),
            "لا".as_bytes(),
            "कि".as_bytes(),
            "क्ष".as_bytes(),
            "\u{200C}".as_bytes(),
            "\u{200D}".as_bytes(),
            "\u{301}".as_bytes(),
        ];
        DICTIONARY
    }
    fn check(&self, input: &[u8]) -> Result<(), String> {
        let text: Vec<_> = String::from_utf8_lossy(input).chars().collect();
        let boundaries = grapheme_boundaries(&text);
        let db = fonts();
        let chain: Vec<_> = (0..db.faces().len()).collect();
        let bits = crate::fingerprint(input);
        let options = ShapeOptions {
            size: 16.0,
            rtl: bits & 1 != 0,
            letter_spacing: if bits & 2 != 0 { 1.0 } else { 0.0 },
            word_spacing: if bits & 4 != 0 { 2.0 } else { 0.0 },
            vertical: bits & 8 != 0,
        };
        let out = shape(db, &chain, &text, &options).map_err(|e| e.to_string())?;
        let mut next = 0;
        for c in out {
            if c.range.start != next
                || c.range.end <= c.range.start
                || c.range.end > text.len()
                || !boundaries[c.range.start]
                || !boundaries[c.range.end]
                || !c.advance.is_finite()
            {
                return Err("cluster partition or grapheme invariant".into());
            }
            next = c.range.end;
            let face = db.face(c.face).ok_or("cluster face")?;
            for g in c.glyphs {
                if g.glyph.0 >= face.glyph_count()
                    || ![g.advance, g.x_offset, g.y_offset]
                        .iter()
                        .all(|x| x.is_finite())
                {
                    return Err("glyph bounds or finite position invariant".into());
                }
            }
        }
        if next != text.len() {
            return Err("source partition ended early".into());
        }
        Ok(())
    }
}
