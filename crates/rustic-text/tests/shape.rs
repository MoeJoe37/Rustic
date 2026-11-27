//! Shaping over the bundled test faces, which map only `x` and the space.

use rustic_text::bundled::test_database;
use rustic_text::shape::{ShapeError, ShapeOptions, is_invisible, is_word_separator, shape};
use rustic_text::{FaceQuery, FaceStyle, FamilyRef, FontDatabase, GlyphId};

const REGULAR: usize = 0;
const BOLD: usize = 1;
const ITALIC: usize = 2;
const CONDENSED: usize = 3;

fn options() -> ShapeOptions {
    ShapeOptions {
        size: 10.0,
        rtl: false,
        letter_spacing: 0.0,
        word_spacing: 0.0,
        vertical: false,
    }
}

fn chars(text: &str) -> Vec<char> {
    text.chars().collect()
}

/// Advance of `c` in `face` at 10px, computed from the face directly.
fn advance(db: &FontDatabase, face: usize, c: char) -> f64 {
    let face = db.face(face).expect("face");
    let glyph = face.glyph_index(c).unwrap_or(GlyphId(0));
    f64::from(face.advance(glyph).expect("advance")) * 10.0 / f64::from(face.units_per_em())
}

#[test]
fn fallback_chain_resolves_families_queries_and_the_fallback() {
    let db = test_database();
    let normal = FaceQuery::default();
    let bold = FaceQuery {
        weight: 700.0,
        ..FaceQuery::default()
    };
    let italic = FaceQuery {
        style: FaceStyle::Italic,
        ..FaceQuery::default()
    };
    let missing = [FamilyRef::Named("Missing"), FamilyRef::Generic("serif")];
    assert_eq!(db.fallback_chain(missing, &normal), [REGULAR]);
    assert_eq!(db.fallback_chain(missing, &bold), [BOLD]);
    assert_eq!(db.fallback_chain(missing, &italic), [ITALIC]);
    // Unknown families alone still reach the fallback family.
    assert_eq!(
        db.fallback_chain([FamilyRef::Named("Missing")], &normal),
        [REGULAR]
    );
    assert!(
        FontDatabase::new()
            .fallback_chain(missing, &normal)
            .is_empty()
    );
    assert!(db.face(4).is_none());
}

#[test]
fn clusters_take_one_face_and_unmapped_text_uses_notdef() {
    let db = test_database();
    let text = chars("x\u{301}a x");
    let clusters = shape(&db, &[CONDENSED, REGULAR], &text, &options()).unwrap();
    // x + combining acute is one grapheme cluster kept on one face.
    assert_eq!(
        clusters
            .iter()
            .map(|c| (c.range.clone(), c.face, c.glyphs.len()))
            .collect::<Vec<_>>(),
        [
            (0..2, CONDENSED, 2),
            (2..3, CONDENSED, 1),
            (3..4, CONDENSED, 1),
            (4..5, CONDENSED, 1)
        ]
    );
    let first = &clusters[0];
    assert_ne!(first.glyphs[0].glyph, GlyphId(0));
    assert_eq!(first.glyphs[1].glyph, GlyphId(0));
    assert_eq!(clusters[1].glyphs[0].glyph, GlyphId(0));
    assert_eq!(first.glyphs[0].advance, advance(&db, CONDENSED, 'x'));
    assert_eq!(
        first.advance,
        first.glyphs.iter().map(|g| g.advance).sum::<f64>()
    );
    // Faces scale by their own units per em (the italic face uses 2048).
    let italic = shape(&db, &[ITALIC], &chars("x"), &options()).unwrap();
    assert_eq!(italic[0].advance, advance(&db, ITALIC, 'x'));
}

#[test]
fn invisible_characters_draw_nothing_even_without_fonts() {
    let db = test_database();
    let clusters = shape(&db, &[REGULAR], &chars("x\u{200B}\u{AD}x"), &options()).unwrap();
    assert_eq!(
        clusters.iter().map(|c| c.glyphs.len()).collect::<Vec<_>>(),
        [1, 0, 0, 1]
    );
    assert_eq!(clusters[1].advance, 0.0);
    let empty = FontDatabase::new();
    let only_invisible = shape(&empty, &[], &chars("\u{200D}\u{FE0F}"), &options()).unwrap();
    assert!(
        only_invisible
            .iter()
            .all(|c| c.glyphs.is_empty() && c.advance == 0.0)
    );
    assert_eq!(
        shape(&empty, &[], &chars("x"), &options()),
        Err(ShapeError::NoFont)
    );
    assert_eq!(
        shape(&db, &[], &chars("x"), &options()),
        Err(ShapeError::NoFont)
    );
    assert!(is_invisible('\u{2066}') && !is_invisible('x') && !is_invisible(' '));
}

#[test]
fn letter_and_word_spacing_follow_css_text() {
    let db = test_database();
    let x = advance(&db, REGULAR, 'x');
    let space = advance(&db, REGULAR, ' ');
    let spaced = ShapeOptions {
        letter_spacing: 1.0,
        word_spacing: 2.0,
        vertical: false,
        ..options()
    };
    let clusters = shape(&db, &[REGULAR], &chars("x x\u{200B}"), &spaced).unwrap();
    assert_eq!(
        clusters.iter().map(|c| c.advance).collect::<Vec<_>>(),
        // Letter spacing follows each visible cluster; invisible ones get none.
        [x + 1.0, space + 2.0 + 1.0, x + 1.0, 0.0]
    );
    assert_eq!(clusters[1].glyphs[0].advance, space + 2.0);
    assert!(is_word_separator('\u{A0}') && !is_word_separator('\u{3000}'));
}

#[test]
fn right_to_left_runs_try_mirrored_glyphs_first() {
    let db = test_database();
    let rtl = ShapeOptions {
        rtl: true,
        ..options()
    };
    // The test faces have no brackets: mirroring falls back to the original
    // character, then to `.notdef`, without changing cluster ranges.
    let clusters = shape(&db, &[REGULAR], &chars("(x)"), &rtl).unwrap();
    assert_eq!(
        clusters.iter().map(|c| c.range.clone()).collect::<Vec<_>>(),
        [0..1, 1..2, 2..3]
    );
    assert_eq!(clusters[0].glyphs[0].glyph, GlyphId(0));
    assert_eq!(
        clusters[1].glyphs[0].glyph,
        shape(&db, &[REGULAR], &chars("x"), &options()).unwrap()[0].glyphs[0].glyph
    );
}

#[test]
fn invisible_controls_never_become_visible_notdef_glyphs() {
    let db = test_database();
    for c in ['\0', '\t', '\n', '\u{7F}', '\u{85}', '\u{2028}', '\u{2029}'] {
        let out = shape(&db, &[REGULAR], &[c, 'x', c], &options()).unwrap();
        assert_eq!(
            out.iter().map(|c| c.glyphs.len()).sum::<usize>(),
            1,
            "U+{:04X}",
            u32::from(c)
        );
    }
}

#[test]
fn vertical_runs_advance_down_from_the_vertical_origin() {
    let db = test_database();
    let face = db.face(REGULAR).expect("face");
    let m = face.metrics();
    let em = f64::from(face.units_per_em());
    let out = shape(
        &db,
        &[REGULAR],
        &chars("xx"),
        &ShapeOptions {
            vertical: true,
            ..options()
        },
    )
    .unwrap();
    assert_eq!(out.len(), 2);
    let g = out[0].glyphs[0];
    // Without `vmtx` the advance is the ascent plus descent (1em here), the
    // vertical origin is the top center, and the ink is centered in the
    // advance (HarfBuzz's fallback origin): yMax + (advance - height) / 2.
    let height = f64::from(i32::from(m.ascent) - i32::from(m.descent)) * 10.0 / em;
    assert!((g.advance - height).abs() < 1e-9, "{g:?}");
    assert!(
        (g.x_offset + advance(&db, REGULAR, 'x') / 2.0).abs() < 1e-9,
        "{g:?}"
    );
    let ink = face.outline(g.glyph).unwrap().bounds().unwrap();
    let (top, bottom) = (ink.y_max * 10.0 / em, ink.y_min * 10.0 / em);
    let origin = top + (height - (top - bottom)) / 2.0;
    assert!((g.y_offset + origin).abs() < 1e-6, "{g:?} {origin}");
}
