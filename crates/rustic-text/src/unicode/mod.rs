//! Unicode character properties for segmentation and bidi, generated from
//! the Unicode Character Database by `tools/generate_text_data.py`.

mod tables;

/// The Unicode version of every property table in this crate.
pub const UNICODE_VERSION: &str = tables::UNICODE_VERSION;

/// UAX #14 `Line_Break` property values.
#[allow(missing_docs, reason = "the UAX #14 class names are the documentation")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineBreakClass {
    Bk,
    Cr,
    Lf,
    Cm,
    Nl,
    Sg,
    Wj,
    Zw,
    Gl,
    Sp,
    Zwj,
    B2,
    Ba,
    Bb,
    Hy,
    Cb,
    Cl,
    Cp,
    Ex,
    In,
    Ns,
    Op,
    Qu,
    Is,
    Nu,
    Po,
    Pr,
    Sy,
    Ai,
    Ak,
    Al,
    Ap,
    As,
    Cj,
    Eb,
    Em,
    H2,
    H3,
    Hl,
    Hh,
    Id,
    Jl,
    Jv,
    Jt,
    Ri,
    Sa,
    Vf,
    Vi,
    Xx,
}

/// UAX #29 `Grapheme_Cluster_Break` property values (`Other` is the default).
#[allow(missing_docs, reason = "the UAX #29 value names are the documentation")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GraphemeClass {
    Other,
    Cr,
    Lf,
    Control,
    Extend,
    Zwj,
    RegionalIndicator,
    Prepend,
    Spacingmark,
    L,
    V,
    T,
    Lv,
    Lvt,
}

/// The `Indic_Conjunct_Break` property (UAX #44), used by UAX #29 GB9c.
#[allow(missing_docs, reason = "the UCD value names are the documentation")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IndicConjunctBreak {
    None,
    Consonant,
    Extend,
    Linker,
}

/// UAX #9 `Bidi_Class` property values.
#[allow(missing_docs, reason = "the UAX #9 class names are the documentation")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BidiClass {
    L,
    R,
    Al,
    En,
    Es,
    Et,
    An,
    Cs,
    Nsm,
    Bn,
    B,
    S,
    Ws,
    On,
    Lre,
    Lro,
    Rle,
    Rlo,
    Pdf,
    Lri,
    Rli,
    Fsi,
    Pdi,
}

impl BidiClass {
    /// Parse a UCD short name such as `AL` or `PDI`.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "L" => Self::L,
            "R" => Self::R,
            "AL" => Self::Al,
            "EN" => Self::En,
            "ES" => Self::Es,
            "ET" => Self::Et,
            "AN" => Self::An,
            "CS" => Self::Cs,
            "NSM" => Self::Nsm,
            "BN" => Self::Bn,
            "B" => Self::B,
            "S" => Self::S,
            "WS" => Self::Ws,
            "ON" => Self::On,
            "LRE" => Self::Lre,
            "LRO" => Self::Lro,
            "RLE" => Self::Rle,
            "RLO" => Self::Rlo,
            "PDF" => Self::Pdf,
            "LRI" => Self::Lri,
            "RLI" => Self::Rli,
            "FSI" => Self::Fsi,
            "PDI" => Self::Pdi,
            _ => return None,
        })
    }
}

fn lookup<T: Copy>(table: &[(u32, u32, T)], c: char) -> Option<T> {
    let c = u32::from(c);
    let i = table.partition_point(|&(_, end, _)| end < c);
    table
        .get(i)
        .filter(|&&(start, _, _)| start <= c)
        .map(|&(_, _, v)| v)
}

fn contains(table: &[(u32, u32)], c: char) -> bool {
    let c = u32::from(c);
    let i = table.partition_point(|&(_, end)| end < c);
    table.get(i).is_some_and(|&(start, _)| start <= c)
}

/// The `Line_Break` property of `c` (`XX` when unlisted).
pub fn line_break_class(c: char) -> LineBreakClass {
    lookup(&tables::LINE_BREAK, c).unwrap_or(LineBreakClass::Xx)
}

/// Whether `c` has East_Asian_Width F, W or H (UAX #14's `$EastAsian`).
pub fn is_east_asian_wide(c: char) -> bool {
    contains(&tables::EAST_ASIAN_FWH, c)
}

/// Whether `c` has East_Asian_Width Ambiguous, for CSS CJK tailoring.
pub fn is_east_asian_ambiguous(c: char) -> bool {
    contains(&tables::EAST_ASIAN_A, c)
}

/// Whether `c` is `Extended_Pictographic`.
pub fn is_extended_pictographic(c: char) -> bool {
    contains(&tables::EXTENDED_PICTOGRAPHIC, c)
}

pub(crate) fn is_unassigned_pictographic(c: char) -> bool {
    contains(&tables::PICTOGRAPHIC_UNASSIGNED, c)
}

/// `SA` characters with General_Category Mn or Mc (LB1 resolves them to CM).
pub(crate) fn is_sa_mark(c: char) -> bool {
    contains(&tables::SA_MARKS, c)
}

pub(crate) fn is_initial_quote(c: char) -> bool {
    contains(&tables::QUOTE_INITIAL, c)
}

pub(crate) fn is_final_quote(c: char) -> bool {
    contains(&tables::QUOTE_FINAL, c)
}

/// The `Grapheme_Cluster_Break` property of `c`.
pub fn grapheme_class(c: char) -> GraphemeClass {
    lookup(&tables::GRAPHEME, c).unwrap_or(GraphemeClass::Other)
}

pub(crate) fn indic_conjunct_break(c: char) -> IndicConjunctBreak {
    lookup(&tables::INDIC_CONJUNCT_BREAK, c).unwrap_or(IndicConjunctBreak::None)
}

/// The `Bidi_Class` property of `c` (unassigned code points use their
/// block's default).
pub fn bidi_class(c: char) -> BidiClass {
    lookup(&tables::BIDI_CLASS, c).unwrap_or(BidiClass::L)
}

/// A character's strong bidirectional direction (HTML `dir=auto`, `:dir()`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StrongDirection {
    /// `Bidi_Class` L.
    Ltr,
    /// `Bidi_Class` R or AL.
    Rtl,
}

/// The strong direction of `c`, or `None` for weak, neutral and explicit
/// formatting characters.
///
/// ```
/// use rustic_text::unicode::{StrongDirection, strong_direction};
/// assert_eq!(strong_direction('a'), Some(StrongDirection::Ltr));
/// assert_eq!(strong_direction('\u{5D0}'), Some(StrongDirection::Rtl));
/// assert_eq!(strong_direction('\u{627}'), Some(StrongDirection::Rtl));
/// assert_eq!(strong_direction('1'), None);
/// ```
pub fn strong_direction(c: char) -> Option<StrongDirection> {
    match bidi_class(c) {
        BidiClass::L => Some(StrongDirection::Ltr),
        BidiClass::R | BidiClass::Al => Some(StrongDirection::Rtl),
        _ => None,
    }
}

/// `Bidi_Paired_Bracket` and whether `c` is an opening bracket.
pub(crate) fn paired_bracket(c: char) -> Option<(char, bool)> {
    let i = tables::BIDI_BRACKETS.partition_point(|&(cp, _, _)| cp < u32::from(c));
    let &(cp, pair, open) = tables::BIDI_BRACKETS.get(i)?;
    (cp == u32::from(c)).then_some(())?;
    Some((char::from_u32(pair)?, open))
}

/// The `Bidi_Mirroring_Glyph` of `c`, used at odd levels (UAX #9 L4).
pub fn mirrored(c: char) -> Option<char> {
    let i = tables::BIDI_MIRRORING.partition_point(|&(cp, _)| cp < u32::from(c));
    let &(cp, mirror) = tables::BIDI_MIRRORING.get(i)?;
    (cp == u32::from(c)).then_some(())?;
    char::from_u32(mirror)
}
