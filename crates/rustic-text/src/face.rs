//! The sfnt table directory and one parsed TrueType face.

use std::fmt;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use crate::cmap::CharMap;
use crate::error::{FontError, Tag};
use crate::glyf::{self, Loca, Outline};
use crate::metrics::FontMetrics;
use crate::reader::{Reader, size};

/// Bounds applied while loading a font and extracting outlines.
///
/// Fonts are attacker-controlled. Every limit failure rejects the font or
/// glyph with [`FontError::LimitExceeded`]; nothing is silently truncated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontLimits {
    /// Largest accepted font file, in bytes.
    pub max_font_bytes: usize,
    /// Most table records in the directory.
    pub max_tables: usize,
    /// Most points in one outline, composite components included.
    pub max_glyph_points: usize,
    /// Most components visited while flattening one composite glyph.
    pub max_components: usize,
    /// Deepest composite nesting.
    pub max_component_depth: usize,
    /// Longest accepted family name, in UTF-16 code units.
    pub max_name_units: usize,
}

impl Default for FontLimits {
    fn default() -> Self {
        Self {
            max_font_bytes: 32 * 1024 * 1024,
            max_tables: 256,
            max_glyph_points: 65_536,
            max_components: 1_024,
            max_component_depth: 8,
            max_name_units: 512,
        }
    }
}

/// A face's slope, from OS/2 `fsSelection` (or `head.macStyle`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FaceStyle {
    /// Upright.
    Normal,
    /// Italic.
    Italic,
    /// Oblique (a slanted upright design).
    Oblique,
}

/// A glyph index in one face.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlyphId(pub u16);

/// One parsed TrueType face. It owns its bytes; every table it uses was
/// bounds-checked and validated when it was created.
pub struct Face {
    data: Arc<Vec<u8>>,
    shaping: OnceLock<Option<harfrust::Font>>,
    tables: Tables,
    units_per_em: u16,
    glyph_count: u16,
    h_metrics: u16,
    loca: Option<Loca>,
    cmap: CharMap,
    families: Vec<String>,
    weight: f64,
    style: FaceStyle,
    stretch: f64,
    metrics: FontMetrics,
    limits: FontLimits,
}

impl fmt::Debug for Face {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Face")
            .field("families", &self.families)
            .field("weight", &self.weight)
            .field("style", &self.style)
            .field("stretch", &self.stretch)
            .field("glyphs", &self.glyph_count)
            .field("bytes", &self.data.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Default)]
struct Tables {
    head: Range<usize>,
    hhea: Range<usize>,
    maxp: Range<usize>,
    cmap: Range<usize>,
    hmtx: Range<usize>,
    loca: Range<usize>,
    glyf: Range<usize>,
    name: Range<usize>,
    os2: Option<Range<usize>>,
    cff: bool,
}

fn invalid(table: Tag, reason: &'static str) -> FontError {
    FontError::Invalid { table, reason }
}

/// Read and check the table directory.
fn directory(data: &[u8], limits: &FontLimits) -> Result<Tables, FontError> {
    let mut r = Reader::new(data, Tag::DIRECTORY);
    match r.u32().map_err(|_| FontError::NotSfnt)? {
        0x0001_0000 | 0x7472_7565 | 0x4F54_544F => {}
        0x7474_6366 => return Err(FontError::Unsupported("font collections")),
        0x774F_4646 | 0x774F_4632 => return Err(FontError::Unsupported("WOFF/WOFF2 wrapping")),
        _ => return Err(FontError::NotSfnt),
    }
    let count = usize::from(r.u16()?);
    if count > limits.max_tables {
        return Err(FontError::LimitExceeded("table count"));
    }
    r.skip(6)?;
    let mut seen: Vec<Tag> = Vec::with_capacity(count);
    let mut tables = Tables::default();
    let mut found = [false; 8];
    for _ in 0..count {
        let tag = r.tag()?;
        r.u32()?; // checksum: not enforced, as in browsers' sanitizers
        let offset = size(r.u32()?);
        let length = size(r.u32()?);
        if seen.contains(&tag) {
            return Err(FontError::DuplicateTable(tag));
        }
        seen.push(tag);
        let end = offset
            .checked_add(length)
            .filter(|&end| end <= data.len())
            .ok_or(FontError::TableOutOfBounds(tag))?;
        let range = offset..end;
        let slot = match tag {
            Tag::HEAD => Some((0, &mut tables.head)),
            Tag::HHEA => Some((1, &mut tables.hhea)),
            Tag::MAXP => Some((2, &mut tables.maxp)),
            Tag::CMAP => Some((3, &mut tables.cmap)),
            Tag::HMTX => Some((4, &mut tables.hmtx)),
            Tag::LOCA => Some((5, &mut tables.loca)),
            Tag::GLYF => Some((6, &mut tables.glyf)),
            Tag::NAME => Some((7, &mut tables.name)),
            Tag([b'C', b'F', b'F', b' ']) | Tag([b'C', b'F', b'F', b'2']) => {
                tables.cff = true;
                None
            }
            Tag::OS2 => {
                tables.os2 = Some(range.clone());
                None
            }
            _ => None,
        };
        if let Some((index, field)) = slot {
            *field = range;
            found[index] = true;
        }
    }
    const REQUIRED: [Tag; 8] = [
        Tag::HEAD,
        Tag::HHEA,
        Tag::MAXP,
        Tag::CMAP,
        Tag::HMTX,
        Tag::LOCA,
        Tag::GLYF,
        Tag::NAME,
    ];
    if data.get(..4) != Some(b"OTTO") && tables.cff {
        return Err(invalid(Tag::DIRECTORY, "outline flavor mismatch"));
    }
    if data.get(..4) == Some(b"OTTO") {
        if !tables.cff {
            return Err(FontError::MissingTable(Tag(*b"CFF ")));
        }
        found[5] = true;
        found[6] = true;
    }
    if let Some(i) = found.iter().position(|&f| !f) {
        return Err(FontError::MissingTable(REQUIRED[i]));
    }
    Ok(tables)
}

struct Head {
    units_per_em: u16,
    long_loca: bool,
    mac_style: u16,
}

fn head(table: &[u8]) -> Result<Head, FontError> {
    let mut r = Reader::new(table, Tag::HEAD);
    if r.u16()? != 1 {
        return Err(invalid(Tag::HEAD, "unknown major version"));
    }
    r.skip(10)?; // minor, fontRevision, checkSumAdjustment
    if r.u32()? != 0x5F0F_3CF5 {
        return Err(invalid(Tag::HEAD, "bad magic number"));
    }
    r.u16()?; // flags
    let units_per_em = r.u16()?;
    if !(16..=16_384).contains(&units_per_em) {
        return Err(invalid(Tag::HEAD, "unitsPerEm outside 16..=16384"));
    }
    r.skip(16 + 8)?; // created, modified, bounding box
    let mac_style = r.u16()?;
    r.skip(4)?; // lowestRecPPEM, fontDirectionHint
    let long_loca = match r.i16()? {
        0 => false,
        1 => true,
        _ => return Err(invalid(Tag::HEAD, "indexToLocFormat is not 0 or 1")),
    };
    if r.i16()? != 0 {
        return Err(invalid(Tag::HEAD, "unknown glyphDataFormat"));
    }
    Ok(Head {
        units_per_em,
        long_loca,
        mac_style,
    })
}

struct Os2 {
    weight: u16,
    width: u16,
    selection: u16,
    typo: (i16, i16, i16),
    x_height: Option<i16>,
    cap_height: Option<i16>,
}

fn os2(table: &[u8]) -> Result<Os2, FontError> {
    let mut r = Reader::new(table, Tag::OS2);
    let version = r.u16()?;
    let needed = match version {
        0 => 78,
        1 => 86,
        _ => 96,
    };
    if table.len() < needed {
        return Err(FontError::Truncated(Tag::OS2));
    }
    r.i16()?; // xAvgCharWidth
    let weight = r.u16()?;
    let width = r.u16()?;
    let mut r = Reader::at(table, 62, Tag::OS2)?;
    let selection = r.u16()?;
    r.skip(4)?; // first/last char index
    let typo = (r.i16()?, r.i16()?, r.i16()?);
    let (x_height, cap_height) = if version >= 2 {
        let mut r = Reader::at(table, 86, Tag::OS2)?;
        (Some(r.i16()?), Some(r.i16()?))
    } else {
        (None, None)
    };
    Ok(Os2 {
        weight,
        width,
        selection,
        typo,
        x_height,
        cap_height,
    })
}

/// `usWidthClass` 1–9 as a `font-stretch` percentage (OpenType §OS/2).
fn stretch_of(width: u16) -> Result<f64, FontError> {
    Ok(match width {
        1 => 50.0,
        2 => 62.5,
        3 => 75.0,
        4 => 87.5,
        5 => 100.0,
        6 => 112.5,
        7 => 125.0,
        8 => 150.0,
        9 => 200.0,
        _ => return Err(invalid(Tag::OS2, "usWidthClass outside 1..=9")),
    })
}

/// Family names: typographic (16) first, then legacy (1), Windows Unicode
/// records preferred, US English first. Duplicates are dropped.
fn family_names(table: &[u8], limits: &FontLimits) -> Result<Vec<String>, FontError> {
    let mut r = Reader::new(table, Tag::NAME);
    let format = r.u16()?;
    if format > 1 {
        return Err(invalid(Tag::NAME, "unknown format"));
    }
    let count = usize::from(r.u16()?);
    let storage = usize::from(r.u16()?);
    // (rank, name)
    let mut candidates: Vec<(u32, String)> = Vec::new();
    for _ in 0..count {
        let platform = r.u16()?;
        let encoding = r.u16()?;
        let language = r.u16()?;
        let name_id = r.u16()?;
        let length = usize::from(r.u16()?);
        let offset = usize::from(r.u16()?);
        let id_rank = match name_id {
            16 => 0,
            1 => 1,
            _ => continue,
        };
        let utf16 = match (platform, encoding) {
            (3, 1 | 10) | (0, _) => true,
            (1, 0) => false,
            _ => continue,
        };
        let start = storage + offset;
        let bytes = table
            .get(start..start + length)
            .ok_or(FontError::Truncated(Tag::NAME))?;
        let units = if utf16 { length / 2 } else { length };
        if units > limits.max_name_units || (utf16 && length % 2 != 0) {
            continue;
        }
        let name = if utf16 {
            char::decode_utf16(
                bytes
                    .chunks_exact(2)
                    .map(|b| u16::from_be_bytes([b[0], b[1]])),
            )
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect::<String>()
        } else {
            // Mac Roman: only its ASCII subset is decoded.
            bytes
                .iter()
                .map(|&b| {
                    if b.is_ascii() {
                        char::from(b)
                    } else {
                        char::REPLACEMENT_CHARACTER
                    }
                })
                .collect()
        };
        if name.trim().is_empty() {
            continue;
        }
        let platform_rank = match platform {
            3 => 0,
            0 => 1,
            _ => 2,
        };
        let language_rank = u32::from(!(platform == 3 && language == 0x409));
        candidates.push((id_rank * 8 + platform_rank * 2 + language_rank, name));
    }
    candidates.sort_by_key(|(rank, _)| *rank);
    let mut names: Vec<String> = Vec::new();
    for (_, name) in candidates {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            names.push(name);
        }
    }
    if names.is_empty() {
        return Err(invalid(Tag::NAME, "no usable family name"));
    }
    Ok(names)
}

impl Face {
    /// Parse TrueType/CFF OpenType, WOFF1 or WOFF2. The whole table set
    /// used later is validated here; outlines are parsed on demand.
    pub fn parse(data: Vec<u8>, limits: &FontLimits) -> Result<Self, FontError> {
        if data.len() > limits.max_font_bytes {
            return Err(FontError::LimitExceeded("font size"));
        }
        let data = crate::containers::decode(data, limits)?;
        let tables = directory(&data, limits)?;
        let head = head(&data[tables.head.clone()])?;

        let mut r = Reader::new(&data[tables.maxp.clone()], Tag::MAXP);
        match r.u32()? {
            0x0000_5000 | 0x0001_0000 => {}
            _ => return Err(invalid(Tag::MAXP, "unknown version")),
        }
        let glyph_count = r.u16()?;
        if glyph_count == 0 {
            return Err(invalid(Tag::MAXP, "no glyphs"));
        }

        let mut r = Reader::new(&data[tables.hhea.clone()], Tag::HHEA);
        if r.u16()? != 1 {
            return Err(invalid(Tag::HHEA, "unknown major version"));
        }
        r.u16()?;
        let hhea_metrics = (r.i16()?, r.i16()?, r.i16()?);
        let mut r = Reader::at(&data[tables.hhea.clone()], 34, Tag::HHEA)?;
        let h_metrics = r.u16()?;
        if h_metrics == 0 || h_metrics > glyph_count {
            return Err(invalid(Tag::HHEA, "numberOfHMetrics outside 1..=numGlyphs"));
        }
        let hmtx_len = usize::from(h_metrics) * 4 + usize::from(glyph_count - h_metrics) * 2;
        if tables.hmtx.len() < hmtx_len {
            return Err(FontError::Truncated(Tag::HMTX));
        }

        let loca = if tables.cff {
            None
        } else {
            Some(Loca::parse(
                &data[tables.loca.clone()],
                head.long_loca,
                glyph_count,
            )?)
        };
        let cmap = CharMap::parse(&data[tables.cmap.clone()])?;
        let families = family_names(&data[tables.name.clone()], limits)?;

        let os2 = match &tables.os2 {
            Some(range) => Some(os2(&data[range.clone()])?),
            None => None,
        };
        let (weight, style, stretch) = match &os2 {
            Some(o) => {
                let weight = match o.weight {
                    0 => 400,
                    // Some old fonts use 1–9 for 100–900.
                    1..=9 => o.weight * 100,
                    w => w.min(1000),
                };
                let style = if o.selection & 0x0200 != 0 {
                    FaceStyle::Oblique
                } else if o.selection & 0x0001 != 0 {
                    FaceStyle::Italic
                } else {
                    FaceStyle::Normal
                };
                (f64::from(weight), style, stretch_of(o.width)?)
            }
            None => {
                let weight = if head.mac_style & 1 != 0 {
                    700.0
                } else {
                    400.0
                };
                let style = if head.mac_style & 2 != 0 {
                    FaceStyle::Italic
                } else {
                    FaceStyle::Normal
                };
                (weight, style, 100.0)
            }
        };

        let mut face = Self {
            data: Arc::new(data),
            shaping: OnceLock::new(),
            tables,
            units_per_em: head.units_per_em,
            glyph_count,
            h_metrics,
            loca,
            cmap,
            families,
            weight,
            style,
            stretch,
            metrics: FontMetrics::default(),
            limits: *limits,
        };
        // USE_TYPO_METRICS (fsSelection bit 7) selects the OS/2 values.
        let (ascent, descent, line_gap) = match &os2 {
            Some(o) if o.selection & 0x0080 != 0 => o.typo,
            _ => hhea_metrics,
        };
        let positive = |v: Option<i16>| v.filter(|&v| v > 0);
        let x_height = positive(os2.as_ref().and_then(|o| o.x_height))
            .or_else(|| positive(face.glyph_top('x')));
        let cap_height = positive(os2.as_ref().and_then(|o| o.cap_height))
            .or_else(|| positive(face.glyph_top('H')));
        face.metrics = FontMetrics {
            units_per_em: face.units_per_em,
            ascent,
            descent,
            line_gap,
            x_height,
            cap_height,
            zero_advance: face.char_advance('0'),
            ideograph_advance: face.char_advance('\u{6C34}'),
        };
        Ok(face)
    }

    /// Family names the face answers to: the typographic family (name id
    /// 16) first when present, then the legacy family (name id 1).
    pub fn families(&self) -> &[String] {
        &self.families
    }

    /// The primary family name.
    pub fn family(&self) -> &str {
        &self.families[0]
    }

    /// The weight, 1–1000 (`usWeightClass`, or 400/700 from `macStyle`).
    pub fn weight(&self) -> f64 {
        self.weight
    }

    /// The slope.
    pub fn style(&self) -> FaceStyle {
        self.style
    }

    /// The width as a `font-stretch` percentage.
    pub fn stretch(&self) -> f64 {
        self.stretch
    }

    /// Font units per em.
    pub fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    /// Number of glyphs.
    pub fn glyph_count(&self) -> u16 {
        self.glyph_count
    }

    /// Metrics derived from the face's tables when it loaded.
    pub fn metrics(&self) -> &FontMetrics {
        &self.metrics
    }

    /// The glyph for `c`, or `None` when the font does not map it. A
    /// mapping to a glyph id beyond the glyph count counts as unmapped.
    pub fn glyph_index(&self, c: char) -> Option<GlyphId> {
        let g = self
            .cmap
            .glyph(&self.data[self.tables.cmap.clone()], u32::from(c));
        u16::try_from(g)
            .ok()
            .filter(|&g| g != 0 && g < self.glyph_count)
            .map(GlyphId)
    }

    /// The horizontal advance of `glyph`, in font units.
    pub fn advance(&self, glyph: GlyphId) -> Result<u16, FontError> {
        if glyph.0 >= self.glyph_count {
            return Err(FontError::GlyphOutOfRange(u32::from(glyph.0)));
        }
        let index = glyph.0.min(self.h_metrics - 1);
        Reader::at(
            &self.data[self.tables.hmtx.clone()],
            usize::from(index) * 4,
            Tag::HMTX,
        )?
        .u16()
    }

    /// The outline of `glyph` in font units, composites flattened.
    pub fn outline(&self, glyph: GlyphId) -> Result<Outline, FontError> {
        if glyph.0 >= self.glyph_count {
            return Err(FontError::GlyphOutOfRange(u32::from(glyph.0)));
        }
        if self.tables.cff {
            return crate::cff::outline(&self.data, glyph, &self.limits);
        }
        glyf::outline(
            &self.loca.ok_or(FontError::MissingTable(Tag::LOCA))?,
            &self.data[self.tables.loca.clone()],
            &self.data[self.tables.glyf.clone()],
            glyph.0,
            &self.limits,
        )
    }

    /// Retained decoded sfnt bytes, for aggregate host font accounting.
    pub fn byte_len(&self) -> usize {
        self.data.len()
    }

    pub(crate) fn shaping_font(&self) -> Option<&harfrust::Font> {
        self.shaping
            .get_or_init(|| {
                let bytes: Arc<dyn AsRef<[u8]> + Send + Sync> = self.data.clone();
                harfrust::Font::new(harfrust::font::Blob::from(bytes), 0)
            })
            .as_ref()
    }

    fn char_advance(&self, c: char) -> Option<u16> {
        self.advance(self.glyph_index(c)?).ok()
    }

    /// The glyph header's yMax for `c`.
    fn glyph_top(&self, c: char) -> Option<i16> {
        let glyph = self.glyph_index(c)?;
        if self.tables.cff {
            return self
                .outline(glyph)
                .ok()?
                .bounds()
                .map(|b| b.y_max.round() as i16);
        }
        let glyf = &self.data[self.tables.glyf.clone()];
        let (start, end) = self
            .loca?
            .range(&self.data[self.tables.loca.clone()], glyf.len(), glyph.0)
            .ok()?;
        glyf::header_bounds(&glyf[start..end]).ok()?.map(|b| b[3])
    }
}
