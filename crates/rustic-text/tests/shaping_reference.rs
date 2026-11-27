//! Committed independent native HarfBuzz 10.2.0 glyph/position expectations.
use rustic_text::{
    FontDatabase, FontLimits,
    shape::{ShapeOptions, shape},
};
#[test]
fn native_harfbuzz_glyphs_advances_and_offsets_match() {
    let mut cases = 0;
    for line in include_str!("fixtures/shaping_reference.txt")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let parts: Vec<_> = line.split('\t').collect();
        assert_eq!(parts.len(), 4);
        let bytes = match parts[0] {
            "RusticCFFTest.otf" => include_bytes!("fonts/shaping/RusticCFFTest.otf").as_slice(),
            "Arabic.ttf" => include_bytes!("fonts/shaping/Arabic.ttf").as_slice(),
            "Devanagari.ttf" => include_bytes!("fonts/shaping/Devanagari.ttf").as_slice(),
            _ => panic!("unknown reference font"),
        };
        let mut db = FontDatabase::new();
        db.load(bytes.to_vec(), &FontLimits::default()).unwrap();
        let text: Vec<_> = parts[2]
            .split(',')
            .map(|c| char::from_u32(u32::from_str_radix(c, 16).unwrap()).unwrap())
            .collect();
        let options = ShapeOptions {
            size: f64::from(db.face(0).unwrap().units_per_em()),
            rtl: parts[1] == "rtl",
            letter_spacing: 0.0,
            word_spacing: 0.0,
            vertical: false,
        };
        let mut clusters = shape(&db, &[0], &text, &options).unwrap();
        if options.rtl {
            clusters.reverse();
        }
        let actual: Vec<_> = clusters
            .iter()
            .flat_map(|c| {
                c.glyphs
                    .iter()
                    .map(|g| [f64::from(g.glyph.0), g.advance, g.x_offset, g.y_offset])
            })
            .collect();
        let expected: Vec<_> = parts[3]
            .split(';')
            .map(|g| {
                let n: Vec<f64> = g.split(',').map(|v| v.parse().unwrap()).collect();
                [n[0], n[1], n[2], n[3]]
            })
            .collect();
        assert_eq!(actual, expected, "{} {} {}", parts[0], parts[1], parts[2]);
        cases += 1;
    }
    assert_eq!(cases, 30);
}
