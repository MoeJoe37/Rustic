//! sfnt parsing, metrics, outlines, matching and hostile-input behavior over
//! the bundled original test fonts.

use rustic_text::bundled::{TEST_FONTS, test_database};
use rustic_text::{
    EmRatios, Face, FaceQuery, FaceStyle, FamilyRef, FontDatabase, FontError, FontLimits, GlyphId,
    Tag,
};

fn font(index: usize) -> Vec<u8> {
    TEST_FONTS[index].1.to_vec()
}

fn face(index: usize) -> Face {
    Face::parse(font(index), &FontLimits::default()).expect("bundled font parses")
}

const REGULAR: usize = 0;
const BOLD: usize = 1;
const ITALIC: usize = 2;
const CONDENSED: usize = 3;

fn be16(data: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([data[at], data[at + 1]])
}

fn be32(data: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

/// (record offset, table offset, table length) for `tag`, read independently
/// of the library.
fn table(data: &[u8], tag: &[u8; 4]) -> (usize, usize, usize) {
    let count = usize::from(be16(data, 4));
    (0..count)
        .map(|i| 12 + 16 * i)
        .find(|&r| &data[r..r + 4] == tag)
        .map(|r| (r, be32(data, r + 8) as usize, be32(data, r + 12) as usize))
        .expect("table present")
}

/// The byte range of glyph `id` inside the font, via `loca`.
fn glyph_range(data: &[u8], id: usize) -> (usize, usize) {
    let (_, head, _) = table(data, b"head");
    let long = be16(data, head + 50) == 1;
    let (_, loca, _) = table(data, b"loca");
    let (_, glyf, _) = table(data, b"glyf");
    let (start, end) = if long {
        (
            be32(data, loca + 4 * id) as usize,
            be32(data, loca + 4 * id + 4) as usize,
        )
    } else {
        (
            usize::from(be16(data, loca + 2 * id)) * 2,
            usize::from(be16(data, loca + 2 * id + 2)) * 2,
        )
    };
    (glyf + start, glyf + end)
}

#[test]
fn bundled_faces_describe_themselves() {
    let expected = [
        (
            vec!["Rustic Test"],
            400.0,
            FaceStyle::Normal,
            100.0,
            1000,
            7,
        ),
        (
            vec!["Rustic Test"],
            700.0,
            FaceStyle::Normal,
            100.0,
            1000,
            7,
        ),
        (
            vec!["Rustic Test"],
            400.0,
            FaceStyle::Italic,
            100.0,
            2048,
            6,
        ),
        (
            vec!["Rustic Test", "Rustic Test Condensed"],
            400.0,
            FaceStyle::Normal,
            75.0,
            1000,
            7,
        ),
    ];
    for (index, (families, weight, style, stretch, upem, glyphs)) in
        expected.into_iter().enumerate()
    {
        let face = face(index);
        assert_eq!(face.families(), families.as_slice(), "face {index}");
        assert_eq!(face.family(), families[0]);
        assert_eq!(face.weight(), weight);
        assert_eq!(face.style(), style);
        assert_eq!(face.stretch(), stretch);
        assert_eq!(face.units_per_em(), upem);
        assert_eq!(face.glyph_count(), glyphs);
    }
}

#[test]
fn metrics_come_from_os2_or_outlines() {
    let regular = face(REGULAR).metrics().to_owned();
    assert_eq!(
        (regular.ascent, regular.descent, regular.line_gap),
        (800, -200, 0)
    );
    assert_eq!(regular.x_height, Some(500));
    assert_eq!(regular.cap_height, Some(700));
    assert_eq!(regular.zero_advance, Some(550));
    assert_eq!(regular.ideograph_advance, Some(1000));
    assert_eq!(
        regular.em_ratios(),
        EmRatios {
            x_height: 0.5,
            cap_height: 0.7,
            zero_advance: 0.55,
            ideograph_advance: 1.0
        }
    );

    // OS/2 version 1: x-height and cap height from the `x`/`H` glyph tops;
    // no 水 glyph, so `ic` falls back to 1em.
    let italic = face(ITALIC).metrics().to_owned();
    assert_eq!(italic.x_height, Some(983));
    assert_eq!(italic.cap_height, Some(1413));
    assert_eq!(italic.zero_advance, Some(1106));
    assert_eq!(italic.ideograph_advance, None);
    let ratios = italic.em_ratios();
    assert_eq!(ratios.x_height, 983.0 / 2048.0);
    assert_eq!(ratios.cap_height, 1413.0 / 2048.0);
    assert_eq!(ratios.zero_advance, 1106.0 / 2048.0);
    assert_eq!(ratios.ideograph_advance, 1.0);

    let bold = face(BOLD).metrics().em_ratios();
    assert_eq!(
        (bold.x_height, bold.cap_height, bold.zero_advance),
        (0.52, 0.72, 0.6)
    );
    let condensed = face(CONDENSED).metrics().em_ratios();
    assert_eq!(
        (condensed.zero_advance, condensed.ideograph_advance),
        (0.45, 0.9)
    );
}

#[test]
fn missing_values_use_css_fallbacks() {
    let mut metrics = face(REGULAR).metrics().to_owned();
    metrics.x_height = None;
    metrics.cap_height = None;
    metrics.zero_advance = None;
    metrics.ideograph_advance = None;
    let ratios = metrics.em_ratios();
    // ex and ch: 0.5em; cap: the ascent; ic: 1em.
    assert_eq!(ratios.x_height, 0.5);
    assert_eq!(ratios.zero_advance, 0.5);
    assert_eq!(ratios.cap_height, 0.8);
    assert_eq!(ratios.ideograph_advance, 1.0);
    assert_eq!(
        rustic_text::FontMetrics::default().em_ratios(),
        EmRatios::FALLBACK
    );
}

#[test]
fn cmap_formats_4_and_12_agree() {
    let chars = [
        ' ',
        '0',
        'H',
        'w',
        'x',
        '\u{6C34}',
        'A',
        '\u{1F600}',
        '\u{FFFF}',
    ];
    let expected = [
        Some(1),
        Some(2),
        Some(3),
        Some(6),
        Some(4),
        Some(5),
        None,
        None,
        None,
    ];
    // Regular prefers its format 12 subtable.
    let regular = face(REGULAR);
    for (c, glyph) in chars.iter().zip(expected) {
        assert_eq!(regular.glyph_index(*c), glyph.map(GlyphId), "{c:?}");
    }
    // Hiding the (3,10) record leaves only format 4 (with an idRangeOffset
    // segment for `w`/`x`): the mapping must not change.
    let mut data = font(REGULAR);
    let (_, cmap, _) = table(&data, b"cmap");
    assert_eq!(be16(&data, cmap + 2), 2);
    data[cmap + 3] = 1;
    let format4 = Face::parse(data, &FontLimits::default()).unwrap();
    for (c, glyph) in chars.iter().zip(expected) {
        assert_eq!(format4.glyph_index(*c), glyph.map(GlyphId), "{c:?}");
    }
    // The italic face only has format 4 and no 水.
    let italic = face(ITALIC);
    assert_eq!(italic.glyph_index('w'), Some(GlyphId(5)));
    assert_eq!(italic.glyph_index('x'), Some(GlyphId(4)));
    assert_eq!(italic.glyph_index('\u{6C34}'), None);
}

#[test]
fn advances_cover_short_hmtx() {
    let regular = face(REGULAR);
    // Regular stores 6 long metrics for 7 glyphs; the composite reuses the last.
    let advances: Vec<u16> = (0..7)
        .map(|g| regular.advance(GlyphId(g)).unwrap())
        .collect();
    assert_eq!(advances, [500, 250, 550, 650, 500, 1000, 1000]);
    assert_eq!(
        regular.advance(GlyphId(7)),
        Err(FontError::GlyphOutOfRange(7))
    );
    let bold = face(BOLD);
    assert_eq!(bold.advance(GlyphId(6)), Ok(1080));
}

#[test]
fn outlines_decode_simple_and_composite_glyphs() {
    for index in [REGULAR, BOLD] {
        let face = face(index);
        let x_advance = f64::from(face.advance(face.glyph_index('x').unwrap()).unwrap());
        let space = face.outline(face.glyph_index(' ').unwrap()).unwrap();
        assert!(space.is_empty() && space.bounds().is_none());

        // `0`: two quadratic ovals alternating on/off-curve points.
        let zero = face.outline(face.glyph_index('0').unwrap()).unwrap();
        assert_eq!(zero.contours().len(), 2);
        let outer = &zero.contours()[0];
        assert_eq!(outer.len(), 8);
        assert!(
            outer
                .iter()
                .enumerate()
                .all(|(i, p)| p.on_curve == (i % 2 == 0))
        );
        let inner = &zero.contours()[1];
        assert!(!inner[0].on_curve, "the inner contour starts off-curve");
        let cap = f64::from(face.metrics().cap_height.unwrap());
        let b = zero.bounds().unwrap();
        assert_eq!((b.x_min, b.y_min, b.y_max), (50.0, 0.0, cap));

        // `H`: one 12-point polygon.
        let h = face.outline(face.glyph_index('H').unwrap()).unwrap();
        assert_eq!(h.contours().len(), 1);
        assert_eq!(h.contours()[0].len(), 12);
        assert!(h.contours()[0].iter().all(|p| p.on_curve));

        // `w`: two `x` components, the second offset by one advance.
        let x = face.outline(face.glyph_index('x').unwrap()).unwrap();
        let w = face.outline(face.glyph_index('w').unwrap()).unwrap();
        assert_eq!(w.contours().len(), 2);
        assert_eq!(w.contours()[0], x.contours()[0]);
        let shifted: Vec<_> = x.contours()[0]
            .iter()
            .map(|p| (p.x + x_advance, p.y, p.on_curve))
            .collect();
        let second: Vec<_> = w.contours()[1]
            .iter()
            .map(|p| (p.x, p.y, p.on_curve))
            .collect();
        assert_eq!(second, shifted);
    }
}

#[test]
fn database_matches_family_stretch_style_and_weight() {
    let db = test_database();
    let pick = |families: &[FamilyRef<'_>], weight: f64, style: FaceStyle, stretch: f64| {
        let face = db
            .select(
                families.iter().copied(),
                &FaceQuery {
                    weight,
                    style,
                    stretch,
                },
            )
            .expect("a face");
        (face.weight(), face.style(), face.stretch())
    };
    let family = [FamilyRef::Named("rustic TEST")];
    let normal = FaceStyle::Normal;
    assert_eq!(pick(&family, 400.0, normal, 100.0), (400.0, normal, 100.0));
    assert_eq!(pick(&family, 700.0, normal, 100.0), (700.0, normal, 100.0));
    // 600 > 500: heavier first.
    assert_eq!(pick(&family, 600.0, normal, 100.0), (700.0, normal, 100.0));
    // 450 is inside 400..=500 and nothing in 450..=500: lighter first.
    assert_eq!(pick(&family, 450.0, normal, 100.0), (400.0, normal, 100.0));
    assert_eq!(pick(&family, 100.0, normal, 100.0), (400.0, normal, 100.0));
    // Style narrows before weight: bold italic falls to the 400 italic.
    let italic = FaceStyle::Italic;
    assert_eq!(pick(&family, 700.0, italic, 100.0), (400.0, italic, 100.0));
    assert_eq!(
        pick(&family, 400.0, FaceStyle::Oblique, 100.0),
        (400.0, italic, 100.0)
    );
    // Stretch narrows first: condensed requests find the 75% face.
    assert_eq!(pick(&family, 700.0, normal, 75.0), (400.0, normal, 75.0));
    assert_eq!(pick(&family, 400.0, normal, 87.5), (400.0, normal, 75.0));
    assert_eq!(pick(&family, 400.0, normal, 150.0), (400.0, normal, 100.0));
    // The legacy family name reaches the condensed face only.
    let legacy = [FamilyRef::Named("Rustic Test Condensed")];
    assert_eq!(pick(&legacy, 700.0, italic, 200.0), (400.0, normal, 75.0));
    // Unknown names are skipped; generics map; the fallback catches the rest.
    let list = [FamilyRef::Named("Nope"), FamilyRef::Generic("monospace")];
    assert_eq!(pick(&list, 700.0, normal, 100.0), (700.0, normal, 100.0));
    assert_eq!(
        pick(&[FamilyRef::Named("Nope")], 400.0, normal, 100.0),
        (400.0, normal, 100.0)
    );

    let mut bare = FontDatabase::new();
    bare.load(font(BOLD), &FontLimits::default()).unwrap();
    let query = FaceQuery::default();
    assert!(bare.select([FamilyRef::Named("Nope")], &query).is_none());
    assert!(bare.select([FamilyRef::Generic("serif")], &query).is_none());
    // Any weight still finds the family's only face.
    assert_eq!(
        bare.select([FamilyRef::Named("Rustic Test")], &query)
            .map(Face::weight),
        Some(700.0)
    );
}

#[test]
fn every_truncation_fails_without_panicking() {
    for (name, bytes) in TEST_FONTS {
        for len in 0..bytes.len() {
            let result = Face::parse(bytes[..len].to_vec(), &FontLimits::default());
            // Only trailing table padding may be dropped.
            assert!(
                result.is_err() || bytes.len() - len < 4,
                "{name} parsed at {len} bytes"
            );
        }
    }
}

/// Deterministic xorshift: no third-party randomness.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn mutated_fonts_never_panic() {
    let mut rng = Rng(0x5eed_f0f7);
    let mut parsed = 0;
    for round in 0..4000 {
        let mut data = font(round % TEST_FONTS.len());
        for _ in 0..1 + rng.next() % 4 {
            let at = (rng.next() as usize) % data.len();
            data[at] = match rng.next() % 4 {
                0 => 0,
                1 => 0xFF,
                _ => rng.next() as u8,
            };
        }
        let Ok(face) = Face::parse(data, &FontLimits::default()) else {
            continue;
        };
        parsed += 1;
        let _ = face.metrics().em_ratios();
        for c in [' ', '0', 'H', 'w', 'x', '\u{6C34}', 'A'] {
            let _ = face.glyph_index(c);
        }
        for g in 0..=face.glyph_count() {
            let _ = face.advance(GlyphId(g));
            if let Ok(outline) = face.outline(GlyphId(g)) {
                let points: usize = outline.contours().iter().map(Vec::len).sum();
                assert!(points <= FontLimits::default().max_glyph_points);
            }
        }
    }
    assert!(parsed > 100, "mutations should leave many fonts loadable");
}

fn parse(data: Vec<u8>) -> Result<Face, FontError> {
    Face::parse(data, &FontLimits::default())
}

#[test]
fn malformed_containers_and_tables_are_rejected() {
    let base = font(REGULAR);
    let mut data = base.clone();
    data[..4].copy_from_slice(b"OTTO");
    assert_eq!(
        parse(data).unwrap_err(),
        FontError::MissingTable(rustic_text::Tag(*b"CFF "))
    );
    let mut data = base.clone();
    data[..4].copy_from_slice(b"ttcf");
    assert_eq!(
        parse(data).unwrap_err(),
        FontError::Unsupported("font collections")
    );
    let mut data = base.clone();
    data[..4].copy_from_slice(b"wOF2");
    assert!(parse(data).is_err());
    let mut data = base.clone();
    data[..4].copy_from_slice(b"\x00\x02\x00\x00");
    assert_eq!(parse(data).unwrap_err(), FontError::NotSfnt);
    assert_eq!(parse(Vec::new()).unwrap_err(), FontError::NotSfnt);

    // Duplicate tag: rename `post` to `name`.
    let mut data = base.clone();
    let (post, _, _) = table(&data, b"post");
    data[post..post + 4].copy_from_slice(b"name");
    assert_eq!(
        parse(data).unwrap_err(),
        FontError::DuplicateTable(Tag::NAME)
    );

    // Missing required table: rename `glyf`.
    let mut data = base.clone();
    let (glyf, _, _) = table(&data, b"glyf");
    data[glyf..glyf + 4].copy_from_slice(b"gly_");
    assert_eq!(parse(data).unwrap_err(), FontError::MissingTable(Tag::GLYF));

    // A length running past the end of the file.
    let mut data = base.clone();
    let (hmtx, _, _) = table(&data, b"hmtx");
    data[hmtx + 12..hmtx + 16].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(
        parse(data).unwrap_err(),
        FontError::TableOutOfBounds(Tag::HMTX)
    );

    let (_, head, _) = table(&base, b"head");
    let mut data = base.clone();
    data[head + 18..head + 20].copy_from_slice(&0u16.to_be_bytes());
    assert!(matches!(
        parse(data),
        Err(FontError::Invalid {
            table: Tag::HEAD,
            ..
        })
    ));
    let mut data = base.clone();
    data[head + 12] ^= 1;
    assert!(matches!(
        parse(data),
        Err(FontError::Invalid {
            table: Tag::HEAD,
            ..
        })
    ));
    let mut data = base.clone();
    data[head + 51] = 2;
    assert!(matches!(
        parse(data),
        Err(FontError::Invalid {
            table: Tag::HEAD,
            ..
        })
    ));

    let (_, hhea, _) = table(&base, b"hhea");
    for count in [0u16, 8] {
        let mut data = base.clone();
        data[hhea + 34..hhea + 36].copy_from_slice(&count.to_be_bytes());
        assert!(matches!(
            parse(data),
            Err(FontError::Invalid {
                table: Tag::HHEA,
                ..
            })
        ));
    }

    let (_, maxp, _) = table(&base, b"maxp");
    let mut data = base.clone();
    data[maxp + 4..maxp + 6].copy_from_slice(&0u16.to_be_bytes());
    assert!(matches!(
        parse(data),
        Err(FontError::Invalid {
            table: Tag::MAXP,
            ..
        })
    ));
    // More glyphs than loca entries.
    let mut data = base.clone();
    data[maxp + 4..maxp + 6].copy_from_slice(&400u16.to_be_bytes());
    assert!(parse(data).is_err());

    let (_, os2, _) = table(&base, b"OS/2");
    let mut data = base.clone();
    data[os2 + 6..os2 + 8].copy_from_slice(&10u16.to_be_bytes());
    assert!(matches!(
        parse(data),
        Err(FontError::Invalid {
            table: Tag::OS2,
            ..
        })
    ));
}

#[test]
fn cmap_subtables_are_validated() {
    let base = font(ITALIC); // format 4 only
    let (_, cmap, _) = table(&base, b"cmap");
    let sub = cmap + be32(&base, cmap + 8) as usize;
    let segments = usize::from(be16(&base, sub + 6)) / 2;
    // Swap the first two end codes: unsorted segments.
    let mut data = base.clone();
    let (a, b) = (sub + 14, sub + 16);
    let (x, y) = (be16(&data, a), be16(&data, b));
    data[a..a + 2].copy_from_slice(&y.to_be_bytes());
    data[b..b + 2].copy_from_slice(&x.to_be_bytes());
    assert!(matches!(
        parse(data),
        Err(FontError::Invalid {
            table: Tag::CMAP,
            ..
        })
    ));
    // Remove the final 0xFFFF end code.
    let mut data = base.clone();
    let last = sub + 14 + 2 * (segments - 1);
    data[last..last + 2].copy_from_slice(&0xFFFEu16.to_be_bytes());
    assert!(parse(data).is_err());
    // Odd segment count.
    let mut data = base.clone();
    data[sub + 7] |= 1;
    assert!(parse(data).is_err());
    // No usable subtable: change the record's platform to Macintosh.
    let mut data = base.clone();
    data[cmap + 4..cmap + 6].copy_from_slice(&1u16.to_be_bytes());
    assert!(matches!(parse(data), Err(FontError::Unsupported(_))));

    // Format 12 groups out of order (Regular's second subtable).
    let base = font(REGULAR);
    let (_, cmap, _) = table(&base, b"cmap");
    let sub = cmap + be32(&base, cmap + 16) as usize;
    assert_eq!(be16(&base, sub), 12);
    let mut data = base.clone();
    data[sub + 16..sub + 20].copy_from_slice(&0x40u32.to_be_bytes());
    assert!(matches!(
        parse(data),
        Err(FontError::Invalid {
            table: Tag::CMAP,
            ..
        })
    ));
    let mut data = base.clone();
    data[sub + 12..sub + 16].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(parse(data).is_err());
}

#[test]
fn hostile_glyphs_hit_limits_not_recursion() {
    // Point the composite `w` (glyph 6) at itself: a reference cycle.
    let mut data = font(REGULAR);
    let (start, _) = glyph_range(&data, 6);
    assert_eq!(be16(&data, start) as i16, -1);
    let first_component = start + 10;
    data[first_component + 2..first_component + 4].copy_from_slice(&6u16.to_be_bytes());
    let face = parse(data).unwrap();
    assert_eq!(
        face.outline(GlyphId(6)).unwrap_err(),
        FontError::LimitExceeded("composite glyph depth")
    );

    // A tight point budget rejects the outline instead of truncating it.
    let limits = FontLimits {
        max_glyph_points: 15,
        ..FontLimits::default()
    };
    let face = Face::parse(font(REGULAR), &limits).unwrap();
    assert_eq!(
        face.outline(GlyphId(2)).unwrap_err(),
        FontError::LimitExceeded("outline points")
    );
    let limits = FontLimits {
        max_glyph_points: 16,
        max_components: 1,
        ..FontLimits::default()
    };
    let face = Face::parse(font(REGULAR), &limits).unwrap();
    assert_eq!(face.outline(GlyphId(2)).unwrap().contours().len(), 2);
    assert_eq!(
        face.outline(GlyphId(6)).unwrap_err(),
        FontError::LimitExceeded("composite glyph components")
    );

    // Point-matching composites are explicitly unsupported.
    let mut data = font(REGULAR);
    let (start, _) = glyph_range(&data, 6);
    data[start + 11] &= !0x02;
    assert_eq!(
        parse(data).unwrap().outline(GlyphId(6)).unwrap_err(),
        FontError::Unsupported("composite point matching")
    );

    // A flag repeat running past the last point.
    let mut data = font(REGULAR);
    let (start, _) = glyph_range(&data, 4); // `x`: one contour, 4 points
    let flags = start + 10 + 2 + 2;
    data[flags] |= 0x08;
    data[flags + 1] = 200;
    let face = parse(data).unwrap();
    assert!(matches!(
        face.outline(GlyphId(4)),
        Err(FontError::Invalid {
            table: Tag::GLYF,
            ..
        })
    ));

    // Decreasing loca offsets.
    let mut data = font(BOLD);
    let (_, loca, _) = table(&data, b"loca");
    data[loca + 8..loca + 12].copy_from_slice(&0u32.to_be_bytes());
    let face = parse(data).unwrap();
    assert!(face.outline(GlyphId(1)).is_err());
    assert_eq!(
        face.outline(GlyphId(99)).unwrap_err(),
        FontError::GlyphOutOfRange(99)
    );
}

#[test]
fn file_and_table_limits_apply() {
    let data = font(REGULAR);
    let limits = FontLimits {
        max_font_bytes: data.len() - 1,
        ..FontLimits::default()
    };
    assert_eq!(
        Face::parse(data.clone(), &limits).unwrap_err(),
        FontError::LimitExceeded("font size")
    );
    let limits = FontLimits {
        max_font_bytes: data.len(),
        max_tables: 9,
        ..FontLimits::default()
    };
    assert_eq!(
        Face::parse(data.clone(), &limits).unwrap_err(),
        FontError::LimitExceeded("table count")
    );
    let limits = FontLimits {
        max_tables: 10,
        max_name_units: 4,
        ..FontLimits::default()
    };
    assert!(matches!(
        Face::parse(data, &limits),
        Err(FontError::Invalid {
            table: Tag::NAME,
            ..
        })
    ));
}

#[test]
fn errors_render_fixed_text() {
    assert_eq!(Tag(*b"OS/2").to_string(), "OS/2");
    assert_eq!(Tag([0, b'a', 0xff, b' ']).to_string(), "\\x00a\\xff ");
    assert_eq!(
        FontError::Truncated(Tag::CMAP).to_string(),
        "font table cmap is truncated"
    );
}
