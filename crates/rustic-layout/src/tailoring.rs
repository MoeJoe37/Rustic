//! CSS Text 3 §5 tailoring. Locale resources and Level 4 phrases: TD-87.
use crate::{
    flow::{FlowError, Work},
    inline::{Ifc, Piece, Space, wrap_style},
};
use rustic_css::{
    Hyphens, LineBreak, OverflowWrap, PropertyId as P, PropertyValue as V, WhiteSpace, WordBreak,
};
use rustic_style::ComputedStyle;
use rustic_text::{
    FontDatabase,
    hyphen::hyphenation_points,
    segment::{BreakOpportunity as B, grapheme_boundaries, line_breaks, line_breaks_for_break_all},
    shape::{ShapeOptions, ShapedGlyph, shape},
    unicode::{
        LineBreakClass as L, is_east_asian_ambiguous, is_east_asian_wide, line_break_class as lb,
    },
};
/// A glyph inserted only when its discretionary break is selected.
#[derive(Clone, Debug)]
pub(crate) struct Discretionary {
    pub text: usize,
    pub face: usize,
    pub glyphs: Vec<ShapedGlyph>,
    pub width: f64,
    pub at: usize,
}
fn values(s: &ComputedStyle) -> Result<(LineBreak, WordBreak, OverflowWrap, Hyphens), FlowError> {
    match (
        s.get(P::LineBreak),
        s.get(P::WordBreak),
        s.get(P::OverflowWrap),
        s.get(P::Hyphens),
    ) {
        (V::LineBreak(l), V::WordBreak(w), V::OverflowWrap(o), V::Hyphens(h)) => {
            Ok((*l, *w, *o, *h))
        }
        _ => Err(FlowError::InvalidSnapshot),
    }
}
fn letters(l: L) -> bool {
    matches!(
        l,
        L::Al
            | L::Hl
            | L::Nu
            | L::Id
            | L::Cj
            | L::Jl
            | L::Jv
            | L::Jt
            | L::H2
            | L::H3
            | L::Sa
            | L::Ak
            | L::As
    )
}
fn chinese_japanese(lang: Option<&str>) -> bool {
    lang.is_some_and(|l| {
        let p = l.split('-').next().unwrap_or("");
        p == "zh" || p == "ja"
    })
}
fn tailored(
    a: char,
    b: char,
    mode: LineBreak,
    lang: Option<&str>,
    previous_id: bool,
    base: B,
) -> B {
    if mode == LineBreak::Anywhere {
        return B::Allowed;
    }
    let loose = mode == LineBreak::Loose;
    let cj = chinese_japanese(lang);
    let mut allowed = base == B::Allowed;
    if matches!(b, '\u{301C}' | '\u{30A0}') {
        allowed = cj && mode != LineBreak::Strict;
    }
    if matches!(b, '\u{2010}' | '\u{2013}') {
        allowed = loose && previous_id;
    }
    if lb(b) == L::Cj
        || matches!(
            b,
            '\u{3005}' | '\u{303B}' | '\u{309D}' | '\u{309E}' | '\u{30FD}' | '\u{30FE}'
        )
        || lb(a) == L::In && lb(b) == L::In
    {
        allowed = loose;
    }
    if matches!(
        b,
        '\u{30FB}'
            | '\u{FF1A}'
            | '\u{FF1B}'
            | '\u{FF65}'
            | '\u{203C}'
            | '\u{2047}'
            | '\u{2048}'
            | '\u{2049}'
            | '\u{FF01}'
            | '\u{FF1F}'
    ) {
        allowed = loose && cj;
    }
    let wide = |c| is_east_asian_wide(c) || is_east_asian_ambiguous(c);
    if lb(b) == L::Po && wide(b) || lb(a) == L::Pr && wide(a) {
        allowed = loose && cj;
    }
    if allowed { B::Allowed } else { B::Prohibited }
}
/// Compute break flags and discretionary glyphs without changing source text.
pub(crate) fn apply(
    ifc: &mut Ifc<'_>,
    fonts: &FontDatabase,
    work: &mut Work<'_>,
) -> Result<(), FlowError> {
    work.charge(ifc.chars.len())?;
    let n = ifc.chars.len();
    let base = line_breaks(&ifc.chars);
    let all = line_breaks_for_break_all(&ifc.chars);
    let graphemes = grapheme_boundaries(&ifc.chars);
    let mut normal = vec![None; n + 1];
    let mut emergency = vec![false; n + 1];
    let mut hyphens = vec![false; n + 1];
    let mut auto = vec![false; n + 1];
    let mut k = 0;
    while k < n {
        if !ifc.chars[k].is_ascii_alphabetic() {
            k += 1;
            continue;
        }
        let start = k;
        while k < n && (ifc.chars[k].is_ascii_alphabetic() || ifc.chars[k] == '\u{AD}') {
            k += 1;
        }
        let chars = &ifc.chars[start..k];
        if !chars.contains(&'\u{AD}') {
            let word: String = chars.iter().collect();
            let item = &ifc.texts[ifc.sources[start].text];
            if let Some(lang) = item.language.as_deref() {
                for point in hyphenation_points(&word, lang) {
                    auto[start + point] = true;
                }
            }
        }
    }
    for e in 1..n {
        if base[e] == B::Mandatory {
            normal[e] = Some(B::Mandatory);
            continue;
        }
        if !graphemes[e] {
            continue;
        }
        let style = wrap_style(ifc, e - 1, e);
        let V::WhiteSpace(ws) = style.get(P::WhiteSpace) else {
            return Err(FlowError::InvalidSnapshot);
        };
        if matches!(ws, WhiteSpace::Nowrap | WhiteSpace::Pre) {
            continue;
        }
        let (line, word, overflow, hyphen) = values(style)?;
        if word == WordBreak::AutoPhrase {
            return Err(FlowError::Unsupported {
                node: ifc.texts[ifc.sources[e - 1].text].node,
                reason: crate::flow::UnsupportedFlow::TextLayout,
            });
        }
        let (a, b) = (ifc.chars[e - 1], ifc.chars[e]);
        let item = &ifc.texts[ifc.sources[e - 1].text];
        let initial = if word == WordBreak::BreakAll {
            all[e]
        } else {
            base[e]
        };
        let mut choice = tailored(
            a,
            b,
            line,
            item.language.as_deref(),
            lb(a) == L::Id || word == WordBreak::BreakAll && letters(lb(a)),
            initial,
        );
        if line != LineBreak::Anywhere
            && word == WordBreak::KeepAll
            && letters(lb(a))
            && letters(lb(b))
        {
            choice = B::Prohibited;
        }
        if *ws == WhiteSpace::BreakSpaces && a == ' ' {
            choice = B::Allowed;
        }
        let anywhere = overflow == OverflowWrap::Anywhere || word == WordBreak::BreakWord;
        if anywhere {
            choice = B::Allowed;
        }
        emergency[e] = overflow == OverflowWrap::BreakWord;
        if a == '\u{AD}' {
            choice = if hyphen == Hyphens::None {
                B::Prohibited
            } else {
                B::Allowed
            };
        }
        let hyphenated =
            hyphen != Hyphens::None && (a == '\u{AD}' || hyphen == Hyphens::Auto && auto[e]);
        if hyphenated && line != LineBreak::Anywhere && word != WordBreak::BreakAll {
            choice = B::Allowed;
            hyphens[e] = true;
        }
        normal[e] = (choice == B::Allowed).then_some(B::Allowed);
    }
    ifc.breaks = vec![None; ifc.pieces.len()];
    ifc.emergency = vec![false; ifc.pieces.len()];
    ifc.hyphens = vec![None; ifc.pieces.len()];
    for k in 0..ifc.pieces.len() {
        let next_close = matches!(ifc.pieces.get(k + 1), Some(Piece::Edge { open: false, .. }));
        let end = match &ifc.pieces[k] {
            Piece::Cluster { chars, space, .. } => {
                if *space == Space::Forced {
                    ifc.breaks[k] = Some(B::Mandatory);
                    continue;
                }
                chars.end
            }
            Piece::Atomic { chars, .. } => chars.end,
            Piece::Edge {
                open: false, at, ..
            } => *at,
            Piece::Edge { .. } => continue,
        };
        if next_close || end == 0 || end >= n {
            continue;
        }
        ifc.breaks[k] = normal[end];
        ifc.emergency[k] = emergency[end];
        if hyphens[end] {
            let source = ifc.sources[end - 1];
            let item = &ifc.texts[source.text];
            let hyphen = if item.chain.iter().any(|&i| {
                fonts
                    .face(i)
                    .is_some_and(|f| f.glyph_index('\u{2010}').is_some())
            }) {
                '\u{2010}'
            } else {
                '-'
            };
            let options = ShapeOptions {
                size: item.font.size,
                rtl: ifc.bidi.levels()[end - 1].is_rtl(),
                letter_spacing: 0.0,
                word_spacing: 0.0,
                vertical: false,
            };
            let shaped = shape(fonts, &item.chain, &[hyphen], &options)
                .map_err(|_| FlowError::InvalidSnapshot)?;
            let c = shaped
                .into_iter()
                .next()
                .ok_or(FlowError::InvalidSnapshot)?;
            ifc.hyphens[k] = Some(Discretionary {
                text: source.text,
                face: c.face,
                glyphs: c.glyphs,
                width: c.advance,
                at: end - 1,
            });
        }
    }
    Ok(())
}
