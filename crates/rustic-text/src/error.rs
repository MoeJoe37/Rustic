//! Font parsing errors and table tags.

use std::fmt;

/// A four-byte OpenType table tag such as `head` or `OS/2`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tag(pub [u8; 4]);

impl Tag {
    /// `head`.
    pub const HEAD: Tag = Tag(*b"head");
    /// `hhea`.
    pub const HHEA: Tag = Tag(*b"hhea");
    /// `maxp`.
    pub const MAXP: Tag = Tag(*b"maxp");
    /// `cmap`.
    pub const CMAP: Tag = Tag(*b"cmap");
    /// `hmtx`.
    pub const HMTX: Tag = Tag(*b"hmtx");
    /// `OS/2`.
    pub const OS2: Tag = Tag(*b"OS/2");
    /// `name`.
    pub const NAME: Tag = Tag(*b"name");
    /// `loca`.
    pub const LOCA: Tag = Tag(*b"loca");
    /// `glyf`.
    pub const GLYF: Tag = Tag(*b"glyf");
    /// The table directory itself, for errors before any table is read.
    pub const DIRECTORY: Tag = Tag(*b"sfnt");
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &byte in &self.0 {
            if byte.is_ascii_graphic() || byte == b' ' {
                write!(f, "{}", char::from(byte))?;
            } else {
                write!(f, "\\x{byte:02x}")?;
            }
        }
        Ok(())
    }
}

/// Why a font or glyph was rejected. Every error is recoverable: a rejected
/// font is simply not loaded, and parsing never panics on hostile bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontError {
    /// The data ended inside a structure of this table.
    Truncated(Tag),
    /// The data is not a TrueType-flavored sfnt font.
    NotSfnt,
    /// A recognized but unsupported container or format.
    Unsupported(&'static str),
    /// A required table is absent.
    MissingTable(Tag),
    /// The same table tag appears twice in the directory.
    DuplicateTable(Tag),
    /// A table record points outside the font data.
    TableOutOfBounds(Tag),
    /// A field holds a value the format does not allow.
    Invalid {
        /// The table holding the field.
        table: Tag,
        /// What was wrong, as fixed engine text.
        reason: &'static str,
    },
    /// A glyph id at or beyond the font's glyph count.
    GlyphOutOfRange(u32),
    /// A configured [`crate::FontLimits`] bound was exceeded.
    LimitExceeded(&'static str),
}

impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated(tag) => write!(f, "font table {tag} is truncated"),
            Self::NotSfnt => f.write_str("data is not an sfnt font"),
            Self::Unsupported(what) => write!(f, "unsupported font feature: {what}"),
            Self::MissingTable(tag) => write!(f, "required font table {tag} is missing"),
            Self::DuplicateTable(tag) => write!(f, "font table {tag} appears twice"),
            Self::TableOutOfBounds(tag) => write!(f, "font table {tag} lies outside the font"),
            Self::Invalid { table, reason } => write!(f, "invalid font table {table}: {reason}"),
            Self::GlyphOutOfRange(id) => write!(f, "glyph {id} is out of range"),
            Self::LimitExceeded(what) => write!(f, "font limit exceeded: {what}"),
        }
    }
}

impl std::error::Error for FontError {}
