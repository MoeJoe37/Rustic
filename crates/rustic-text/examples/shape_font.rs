//! Bounded glyph dump for tools/compare_shaping.py; positions in font units.
use rustic_text::{
    FontDatabase, FontLimits,
    shape::{ShapeOptions, shape},
};
use std::io::{Read, Write};
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: shape_font FONT ltr|rtl TEXT".into());
    }
    if args[2].chars().count() > 4096 {
        return Err("text limit".into());
    }
    let limits = FontLimits::default();
    let mut bytes = Vec::new();
    std::fs::File::open(&args[0])?
        .take(limits.max_font_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    let mut db = FontDatabase::new();
    db.load(bytes, &limits)?;
    let options = ShapeOptions {
        size: f64::from(db.face(0).ok_or("font")?.units_per_em()),
        rtl: args[1] == "rtl",
        letter_spacing: 0.0,
        word_spacing: 0.0,
    };
    let mut clusters = shape(&db, &[0], &args[2].chars().collect::<Vec<_>>(), &options)?;
    if options.rtl {
        clusters.reverse();
    }
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for c in clusters {
        for g in c.glyphs {
            writeln!(
                out,
                "{} {} {} {}",
                g.glyph.0, g.advance, g.x_offset, g.y_offset
            )?;
        }
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
