//! Native-format equivalence, complex scripts and hostile adapter inputs.
use rustic_base::CancellationToken;
use rustic_text::{
    DiscoveryLimits, Face, FontDatabase, FontError, FontLimits,
    shape::{
        ShapeContext, ShapeError, ShapeLimits, ShapeOptions, shape, shape_with_context,
        shape_with_limits,
    },
};
const CFF: &[u8] = include_bytes!("fonts/shaping/RusticCFFTest.otf");
const CFF2: &[u8] = include_bytes!("fonts/shaping/RusticCFF2Test.otf");
const WOFF: &[u8] = include_bytes!("fonts/shaping/RusticCFFTest.woff");
const WOFF2: &[u8] = include_bytes!("fonts/shaping/RusticCFFTest.woff2");
const ARABIC: &[u8] = include_bytes!("fonts/shaping/Arabic.ttf");
const INDIC: &[u8] = include_bytes!("fonts/shaping/Devanagari.ttf");
fn options(rtl: bool) -> ShapeOptions {
    ShapeOptions {
        size: 1000.0,
        rtl,
        letter_spacing: 0.0,
        word_spacing: 0.0,
    }
}
fn database(data: &[u8]) -> FontDatabase {
    let mut d = FontDatabase::new();
    d.load(data.to_vec(), &FontLimits::default()).unwrap();
    d
}
fn shaped(d: &FontDatabase, text: &str, rtl: bool) -> Vec<rustic_text::shape::ShapedCluster> {
    shape(d, &[0], &text.chars().collect::<Vec<_>>(), &options(rtl)).unwrap()
}
fn ids(d: &FontDatabase, text: &str, rtl: bool) -> Vec<u16> {
    shaped(d, text, rtl)
        .iter()
        .flat_map(|c| c.glyphs.iter().map(|g| g.glyph.0))
        .collect()
}
#[test]
fn cff_cff2_and_web_wrappers_preserve_metrics_and_outlines() {
    let reference = Face::parse(CFF.to_vec(), &FontLimits::default()).unwrap();
    for data in [CFF, CFF2, WOFF, WOFF2] {
        let f = Face::parse(data.to_vec(), &FontLimits::default()).unwrap();
        assert_eq!(f.metrics(), reference.metrics());
        assert_eq!(f.family(), reference.family());
        for c in ['x', 'A', 'V', 'f', 'i'] {
            let glyph = f.glyph_index(c).unwrap();
            assert_eq!(f.advance(glyph), reference.advance(glyph));
            assert_eq!(f.outline(glyph).unwrap(), reference.outline(glyph).unwrap());
        }
    }
    let small = FontLimits {
        max_glyph_points: 2,
        ..FontLimits::default()
    };
    let f = Face::parse(CFF.to_vec(), &small).unwrap();
    assert!(matches!(
        f.outline(f.glyph_index('x').unwrap()),
        Err(FontError::LimitExceeded(_))
    ));
}
#[test]
fn gsub_gpos_and_tracking_have_measured_effects() {
    let d = database(CFF);
    let f = d.face(0).unwrap();
    let fi = shaped(&d, "fi", false);
    assert_eq!(fi.len(), 1);
    assert_eq!(fi[0].range, 0..2);
    assert_eq!(fi[0].glyphs.len(), 1);
    assert_eq!(fi[0].advance, 450.0);
    let av = shaped(&d, "AV", false);
    assert_eq!(av.iter().map(|c| c.advance).sum::<f64>(), 1120.0);
    let mark = shaped(&d, "x\u{301}", false);
    assert_eq!(mark.len(), 1);
    assert_eq!(mark[0].range, 0..2);
    assert_eq!(mark[0].glyphs.len(), 2);
    assert_eq!(mark[0].advance, 500.0);
    assert!(mark[0].glyphs[1].y_offset >= 600.0);
    let spaced = shape(
        &d,
        &[0],
        &['f', 'i'],
        &ShapeOptions {
            letter_spacing: 10.0,
            ..options(false)
        },
    )
    .unwrap();
    assert_eq!(spaced.len(), 2);
    assert_eq!(spaced.iter().map(|c| c.advance).sum::<f64>(), 520.0);
    assert_eq!(av[0].glyphs[0].glyph, f.glyph_index('A').unwrap());
}
#[test]
fn arabic_joining_ligatures_and_join_controls_preserve_source_ranges() {
    let d = database(ARABIC);
    let together = ids(&d, "بب", true);
    let alone = ids(&d, "ب", true);
    assert_eq!(together.len(), 2);
    assert_ne!(together[0], alone[0]);
    assert_ne!(together[1], alone[0]);
    assert_eq!(ids(&d, "لا", true).len(), 1);
    let nojoin = shaped(&d, "ب\u{200C}ب", true);
    assert_eq!(
        nojoin
            .iter()
            .flat_map(|c| c.glyphs.iter().map(|g| g.glyph.0))
            .collect::<Vec<_>>(),
        [alone[0], alone[0]]
    );
    assert_eq!(nojoin.first().unwrap().range.start, 0);
    assert_eq!(nojoin.last().unwrap().range.end, 3);
    let text: Vec<_> = "ب".chars().collect();
    let following: Vec<_> = "ب".chars().collect();
    let context = shape_with_context(
        &d,
        &[0],
        &text,
        &options(true),
        ShapeContext {
            before: &[],
            after: &following,
        },
        &ShapeLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    assert_eq!(context[0].glyphs[0].glyph.0, together[0]);
}
#[test]
fn indic_conjuncts_and_prebase_vowels_are_shaped_as_original_clusters() {
    let d = database(INDIC);
    let conjunct = shaped(&d, "क्ष", false);
    assert_eq!(conjunct.len(), 1);
    assert_eq!(conjunct[0].range, 0..3);
    assert!(conjunct[0].glyphs.len() < 3);
    let vowel = shaped(&d, "कि", false);
    assert_eq!(vowel.len(), 1);
    assert_eq!(vowel[0].range, 0..2);
    assert_eq!(vowel[0].glyphs.len(), 2);
    assert_ne!(
        vowel[0].glyphs[0].glyph,
        d.face(0).unwrap().glyph_index('क').unwrap()
    );
}
#[test]
fn fallback_chooses_whole_graphemes_across_loaded_families() {
    let mut d = database(CFF);
    d.load(ARABIC.to_vec(), &FontLimits::default()).unwrap();
    d.load(INDIC.to_vec(), &FontLimits::default()).unwrap();
    let chain = d.fallback_chain(
        [rustic_text::FamilyRef::Named("Rustic CFF Test")],
        &rustic_text::FaceQuery::default(),
    );
    assert_eq!(chain, [0, 1, 2]);
    let out = shape(
        &d,
        &chain,
        &"x ب कि".chars().collect::<Vec<_>>(),
        &options(false),
    )
    .unwrap();
    assert!(
        out.iter().any(|c| c.face == 0)
            && out.iter().any(|c| c.face == 1)
            && out.iter().any(|c| c.face == 2)
    );
}
#[test]
fn limits_invalid_values_and_cancellation_fail_without_partial_results() {
    let d = database(CFF);
    let token = CancellationToken::default();
    assert_eq!(
        shape_with_limits(
            &d,
            &[0],
            &['x', 'x'],
            &options(false),
            &ShapeLimits {
                max_chars: 1,
                max_glyphs: 99
            },
            &token
        ),
        Err(ShapeError::LimitExceeded)
    );
    assert_eq!(
        shape_with_limits(
            &d,
            &[0],
            &['x'],
            &options(false),
            &ShapeLimits {
                max_chars: 1,
                max_glyphs: 0
            },
            &token
        ),
        Err(ShapeError::LimitExceeded)
    );
    assert_eq!(
        shape(
            &d,
            &[0],
            &['x'],
            &ShapeOptions {
                size: f64::NAN,
                ..options(false)
            }
        ),
        Err(ShapeError::InvalidOptions)
    );
    token.cancel();
    assert_eq!(
        shape_with_limits(
            &d,
            &[0],
            &['x'],
            &options(false),
            &ShapeLimits::default(),
            &token
        ),
        Err(ShapeError::Cancelled)
    );
    assert_eq!(
        shape(&d, &[0], &vec!['x'; 4097], &options(false)),
        Err(ShapeError::LimitExceeded)
    );
    let long: Vec<_> = "x ".repeat(3000).chars().collect();
    assert_eq!(
        shape(&d, &[0], &long, &options(false))
            .unwrap()
            .last()
            .unwrap()
            .range
            .end,
        long.len()
    );
}
#[test]
fn malformed_wrappers_and_cff_tables_remain_bounded() {
    for bytes in [CFF, CFF2, WOFF, WOFF2] {
        for n in 0..bytes.len().min(120) {
            assert!(Face::parse(bytes[..n].to_vec(), &FontLimits::default()).is_err());
        }
        let small = FontLimits {
            max_font_bytes: bytes.len() - 1,
            ..FontLimits::default()
        };
        assert!(Face::parse(bytes.to_vec(), &small).is_err());
        for n in 0..bytes.len().min(300) {
            let mut mutant = bytes.to_vec();
            mutant[n] ^= 0xA5;
            if let Ok(f) = Face::parse(mutant, &FontLimits::default()) {
                let _ = f.outline(rustic_text::GlyphId(0));
            }
        }
    }
    for original in [WOFF, WOFF2] {
        let mut data = original.to_vec();
        data[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(
            Face::parse(data, &FontLimits::default()),
            Err(FontError::LimitExceeded(_))
        ));
    }
}
#[test]
fn discovery_is_sorted_bounded_and_atomic() {
    let dir = std::env::temp_dir().join(format!("rustic-discovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("b.woff2"), WOFF2).unwrap();
    std::fs::write(dir.join("a.otf"), CFF).unwrap();
    std::fs::write(dir.join("invalid.ttf"), b"invalid").unwrap();
    let mut d = FontDatabase::new();
    let small = DiscoveryLimits {
        max_faces: 1,
        ..DiscoveryLimits::default()
    };
    assert!(
        d.discover(std::slice::from_ref(&dir), &small, &FontLimits::default())
            .is_err()
    );
    assert!(d.faces().is_empty());
    let report = d
        .discover(
            std::slice::from_ref(&dir),
            &DiscoveryLimits::default(),
            &FontLimits::default(),
        )
        .unwrap();
    assert_eq!((report.loaded, report.rejected), (2, 1));
    assert_eq!(d.faces().len(), 2);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.join("a.otf"), dir.join("link.otf")).unwrap();
        let mut d = FontDatabase::new();
        assert_eq!(
            d.discover(
                std::slice::from_ref(&dir),
                &DiscoveryLimits::default(),
                &FontLimits::default()
            )
            .unwrap()
            .loaded,
            2
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
