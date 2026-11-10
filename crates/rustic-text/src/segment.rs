//! Text segmentation: UAX #29 extended grapheme clusters and UAX #14 line
//! break opportunities, both for Unicode 17.0.0 without tailoring.
//!
//! Both functions take decoded scalar values and return one entry per
//! boundary position (`0..=len`), so callers keep their own mapping from
//! character indexes back to UTF-8 or UTF-16 offsets. Each is one forward
//! pass with bounded lookaround; no input can trigger recursion or
//! superlinear work.

use crate::unicode::{
    GraphemeClass as G, IndicConjunctBreak as Incb, LineBreakClass as Lb, grapheme_class,
    indic_conjunct_break, is_east_asian_wide, is_extended_pictographic, is_final_quote,
    is_initial_quote, is_sa_mark, is_unassigned_pictographic, line_break_class,
};

/// Extended grapheme cluster boundaries (UAX #29 §3.1.1).
///
/// The result has `text.len() + 1` entries; entry `i` is true when a
/// cluster boundary lies before `text[i]` (the last entry is the end of
/// text). Empty text has the single boundary `[true]`.
pub fn grapheme_boundaries(text: &[char]) -> Vec<bool> {
    let mut out = Vec::with_capacity(text.len() + 1);
    out.push(true);
    let Some(&first) = text.first() else {
        return out;
    };
    let mut prev = grapheme_class(first);
    // ExtPict Extend* ends at the previous character (GB11 prefix).
    let mut pictographic = is_extended_pictographic(first);
    // ExtPict Extend* ZWJ ends at the previous character.
    let mut pictographic_zwj = false;
    // Regional indicators ending at the previous character.
    let mut regional = usize::from(prev == G::RegionalIndicator);
    // GB9c: 0 none, 1 consonant then extend/linker, 2 the same with a linker.
    let mut conjunct = u8::from(indic_conjunct_break(first) == Incb::Consonant);
    for &c in &text[1..] {
        let cur = grapheme_class(c);
        let incb = indic_conjunct_break(c);
        let pict = is_extended_pictographic(c);
        let boundary = match (prev, cur) {
            (G::Cr, G::Lf) => false,
            (G::Control | G::Cr | G::Lf, _) | (_, G::Control | G::Cr | G::Lf) => true,
            (G::L, G::L | G::V | G::Lv | G::Lvt)
            | (G::Lv | G::V, G::V | G::T)
            | (G::Lvt | G::T, G::T)
            | (_, G::Extend | G::Zwj | G::Spacingmark)
            | (G::Prepend, _) => false,
            _ if incb == Incb::Consonant && conjunct == 2 => false,
            (G::Zwj, _) if pict && pictographic_zwj => false,
            (G::RegionalIndicator, G::RegionalIndicator) => regional % 2 == 0,
            _ => true,
        };
        out.push(boundary);
        pictographic_zwj = pictographic && cur == G::Zwj;
        pictographic = pict || (pictographic && cur == G::Extend);
        regional = if cur == G::RegionalIndicator {
            regional + 1
        } else {
            0
        };
        conjunct = match incb {
            Incb::Consonant => 1,
            Incb::Linker if conjunct > 0 => 2,
            Incb::Extend if conjunct > 0 => conjunct,
            _ => 0,
        };
        prev = cur;
    }
    out.push(true);
    out
}

/// A line break opportunity before a character (UAX #14 §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BreakOpportunity {
    /// A break is not allowed here (`×`).
    Prohibited,
    /// A break is allowed here (`÷`).
    Allowed,
    /// A break is required here (`!`), after a hard line break character.
    Mandatory,
}

impl BreakOpportunity {
    /// Whether a line may end here.
    pub fn is_break(self) -> bool {
        self != Self::Prohibited
    }
}

/// LB1: resolve `AI`, `SG`, `XX`, `SA` and `CJ` without tailoring.
fn resolved_class(c: char) -> Lb {
    match line_break_class(c) {
        Lb::Ai | Lb::Sg | Lb::Xx => Lb::Al,
        Lb::Sa if is_sa_mark(c) => Lb::Cm,
        Lb::Sa => Lb::Al,
        Lb::Cj => Lb::Ns,
        other => other,
    }
}

/// One LB9 combining sequence: a base and the marks it absorbed.
#[derive(Clone, Copy)]
struct Unit {
    class: Lb,
    base: char,
    start: usize,
    ends_with_zwj: bool,
    /// Regional indicator units ending here (LB30a), counted once.
    regional_run: usize,
}

const DOTTED_CIRCLE: char = '\u{25CC}';

/// UAX #14 line break opportunities with the default (untailored) rules.
///
/// The result has `text.len() + 1` entries; entry `i` describes the
/// boundary before `text[i]`. Entry 0 is always [`BreakOpportunity::Prohibited`]
/// (LB2) and the last is always [`BreakOpportunity::Mandatory`] (LB3).
pub fn line_breaks(text: &[char]) -> Vec<BreakOpportunity> {
    line_breaks_tailored(text, false)
}

/// UAX #14 with CSS Text 3 break-all letter/number classes treated as ID.
/// Grapheme boundaries are enforced by the layout caller.
pub fn line_breaks_for_break_all(text: &[char]) -> Vec<BreakOpportunity> {
    line_breaks_tailored(text, true)
}
fn line_breaks_tailored(text: &[char], break_all: bool) -> Vec<BreakOpportunity> {
    let mut out = vec![BreakOpportunity::Prohibited; text.len() + 1];
    if let Some(last) = out.last_mut() {
        *last = BreakOpportunity::Mandatory;
    }
    // LB9/LB10: attach CM/ZWJ to the preceding base unless it is a break,
    // space or ZW; a mark that stays alone is treated as AL.
    let mut units: Vec<Unit> = Vec::with_capacity(text.len());
    for (i, &c) in text.iter().enumerate() {
        let class = resolved_class(c);
        let class = if break_all && matches!(class, Lb::Al | Lb::Hl | Lb::Nu | Lb::Sa) {
            Lb::Id
        } else {
            class
        };
        if matches!(class, Lb::Cm | Lb::Zwj) {
            if let Some(unit) = units.last_mut() {
                if !matches!(
                    unit.class,
                    Lb::Bk | Lb::Cr | Lb::Lf | Lb::Nl | Lb::Sp | Lb::Zw
                ) {
                    unit.ends_with_zwj = class == Lb::Zwj;
                    continue;
                }
            }
            units.push(Unit {
                class: Lb::Al,
                base: c,
                start: i,
                ends_with_zwj: class == Lb::Zwj,
                regional_run: 0,
            });
            continue;
        }
        let regional_run = match units.last() {
            Some(prev) if class == Lb::Ri => prev.regional_run + 1,
            None if class == Lb::Ri => 1,
            _ => 0,
        };
        units.push(Unit {
            class,
            base: c,
            start: i,
            ends_with_zwj: false,
            regional_run,
        });
    }
    let rules = Rules { units: &units };
    for u in 1..units.len() {
        out[units[u].start] = rules.decide(u);
    }
    out
}

struct Rules<'u> {
    units: &'u [Unit],
}

impl Rules<'_> {
    fn class(&self, u: usize) -> Option<Lb> {
        self.units.get(u).map(|unit| unit.class)
    }

    /// The class of the last non-space unit at or before `u`.
    fn before_spaces(&self, u: usize) -> Option<(usize, Lb)> {
        let mut k = u;
        loop {
            let class = self.units[k].class;
            if class != Lb::Sp {
                return Some((k, class));
            }
            k = k.checked_sub(1)?;
        }
    }

    /// Whether `NU (SY | IS)*` ends at unit `u`.
    fn numeric_prefix(&self, u: usize) -> bool {
        let mut k = u;
        loop {
            match self.units[k].class {
                Lb::Nu => return true,
                Lb::Sy | Lb::Is => match k.checked_sub(1) {
                    Some(prev) => k = prev,
                    None => return false,
                },
                _ => return false,
            }
        }
    }

    fn east_asian(&self, u: usize) -> bool {
        is_east_asian_wide(self.units[u].base)
    }

    fn aksara(&self, u: usize) -> bool {
        matches!(self.units[u].class, Lb::Ak | Lb::As) || self.units[u].base == DOTTED_CIRCLE
    }

    fn aksara_base(&self, u: usize) -> bool {
        self.units[u].class == Lb::Ak || self.units[u].base == DOTTED_CIRCLE
    }

    /// The opportunity before unit `u` (`u >= 1`), rules LB4–LB31 in order.
    fn decide(&self, u: usize) -> BreakOpportunity {
        use BreakOpportunity::{Allowed, Mandatory, Prohibited};
        let a = self.units[u - 1].class;
        let b = self.units[u].class;
        let next = self.class(u + 1);
        // LB4, LB5
        if a == Lb::Bk || (a == Lb::Cr && b != Lb::Lf) || matches!(a, Lb::Lf | Lb::Nl) {
            return Mandatory;
        }
        if a == Lb::Cr {
            return Prohibited;
        }
        // LB6, LB7
        if matches!(b, Lb::Bk | Lb::Cr | Lb::Lf | Lb::Nl | Lb::Sp | Lb::Zw) {
            return Prohibited;
        }
        let behind = self.before_spaces(u - 1);
        // LB8: ZW SP* ÷
        if behind.is_some_and(|(_, c)| c == Lb::Zw) {
            return Allowed;
        }
        // LB8a: ZWJ ×
        if self.units[u - 1].ends_with_zwj {
            return Prohibited;
        }
        // LB11, LB12, LB12a
        if a == Lb::Wj
            || b == Lb::Wj
            || a == Lb::Gl
            || (b == Lb::Gl && !matches!(a, Lb::Sp | Lb::Ba | Lb::Hy | Lb::Hh))
        {
            return Prohibited;
        }
        // LB13: × CL, × CP, × EX, × SY
        if matches!(b, Lb::Cl | Lb::Cp | Lb::Ex | Lb::Sy) {
            return Prohibited;
        }
        // LB14: OP SP* ×
        if behind.is_some_and(|(_, c)| c == Lb::Op) {
            return Prohibited;
        }
        // LB15a: (sot | BK | CR | LF | NL | OP | QU | GL | SP | ZW) [Pi & QU] SP* ×
        if let Some((k, Lb::Qu)) = behind {
            if is_initial_quote(self.units[k].base)
                && (k == 0
                    || matches!(
                        self.units[k - 1].class,
                        Lb::Bk
                            | Lb::Cr
                            | Lb::Lf
                            | Lb::Nl
                            | Lb::Op
                            | Lb::Qu
                            | Lb::Gl
                            | Lb::Sp
                            | Lb::Zw
                    ))
            {
                return Prohibited;
            }
        }
        // LB15b: × [Pf & QU] (SP | GL | WJ | CL | QU | CP | EX | IS | SY | BK | CR | LF | NL | ZW | eot)
        if b == Lb::Qu
            && is_final_quote(self.units[u].base)
            && next.is_none_or(|c| {
                matches!(
                    c,
                    Lb::Sp
                        | Lb::Gl
                        | Lb::Wj
                        | Lb::Cl
                        | Lb::Qu
                        | Lb::Cp
                        | Lb::Ex
                        | Lb::Is
                        | Lb::Sy
                        | Lb::Bk
                        | Lb::Cr
                        | Lb::Lf
                        | Lb::Nl
                        | Lb::Zw
                )
            })
        {
            return Prohibited;
        }
        // LB15c: SP ÷ IS NU; LB15d: × IS
        if b == Lb::Is {
            return if a == Lb::Sp && next == Some(Lb::Nu) {
                Allowed
            } else {
                Prohibited
            };
        }
        // LB16: (CL | CP) SP* × NS; LB17: B2 SP* × B2
        if (b == Lb::Ns && behind.is_some_and(|(_, c)| matches!(c, Lb::Cl | Lb::Cp)))
            || (b == Lb::B2 && behind.is_some_and(|(_, c)| c == Lb::B2))
        {
            return Prohibited;
        }
        // LB18: SP ÷
        if a == Lb::Sp {
            return Allowed;
        }
        // LB19: × [QU - Pi]; [QU - Pf] ×
        if (b == Lb::Qu && !is_initial_quote(self.units[u].base))
            || (a == Lb::Qu && !is_final_quote(self.units[u - 1].base))
        {
            return Prohibited;
        }
        // LB19a
        if b == Lb::Qu
            && (!self.east_asian(u - 1)
                || self.units.get(u + 1).is_none()
                || !self.east_asian(u + 1))
        {
            return Prohibited;
        }
        if a == Lb::Qu && (!self.east_asian(u) || u < 2 || !self.east_asian(u - 2)) {
            return Prohibited;
        }
        // LB20: ÷ CB, CB ÷
        if a == Lb::Cb || b == Lb::Cb {
            return Allowed;
        }
        // LB20a: (sot | BK | CR | LF | NL | SP | ZW | CB | GL) (HY | HH) × (AL | HL)
        if matches!(a, Lb::Hy | Lb::Hh)
            && matches!(b, Lb::Al | Lb::Hl)
            && (u < 2
                || matches!(
                    self.units[u - 2].class,
                    Lb::Bk | Lb::Cr | Lb::Lf | Lb::Nl | Lb::Sp | Lb::Zw | Lb::Cb | Lb::Gl
                ))
        {
            return Prohibited;
        }
        // LB21, LB21a, LB21b, LB22
        if matches!(b, Lb::Ba | Lb::Hh | Lb::Hy | Lb::Ns | Lb::In)
            || a == Lb::Bb
            || (matches!(a, Lb::Hy | Lb::Hh)
                && b != Lb::Hl
                && u >= 2
                && self.units[u - 2].class == Lb::Hl)
            || (a == Lb::Sy && b == Lb::Hl)
        {
            return Prohibited;
        }
        // LB23, LB23a, LB24
        let alpha = |c: Lb| matches!(c, Lb::Al | Lb::Hl);
        if (alpha(a) && b == Lb::Nu)
            || (a == Lb::Nu && alpha(b))
            || (a == Lb::Pr && matches!(b, Lb::Id | Lb::Eb | Lb::Em))
            || (matches!(a, Lb::Id | Lb::Eb | Lb::Em) && b == Lb::Po)
            || (matches!(a, Lb::Pr | Lb::Po) && alpha(b))
            || (alpha(a) && matches!(b, Lb::Pr | Lb::Po))
        {
            return Prohibited;
        }
        // LB25 (the regular-expression form of UAX #14 since 15.1)
        if matches!(b, Lb::Po | Lb::Pr)
            && (self.numeric_prefix(u - 1)
                || (matches!(a, Lb::Cl | Lb::Cp) && u >= 2 && self.numeric_prefix(u - 2)))
        {
            return Prohibited;
        }
        if matches!(a, Lb::Po | Lb::Pr)
            && (b == Lb::Nu
                || (b == Lb::Op
                    && (next == Some(Lb::Nu)
                        || (next == Some(Lb::Is) && self.class(u + 2) == Some(Lb::Nu)))))
        {
            return Prohibited;
        }
        if b == Lb::Nu && (matches!(a, Lb::Hy | Lb::Is) || self.numeric_prefix(u - 1)) {
            return Prohibited;
        }
        // LB26, LB27: Korean syllable blocks
        let hangul = |c: Lb| matches!(c, Lb::Jl | Lb::Jv | Lb::Jt | Lb::H2 | Lb::H3);
        if (a == Lb::Jl && matches!(b, Lb::Jl | Lb::Jv | Lb::H2 | Lb::H3))
            || (matches!(a, Lb::Jv | Lb::H2) && matches!(b, Lb::Jv | Lb::Jt))
            || (matches!(a, Lb::Jt | Lb::H3) && b == Lb::Jt)
            || (hangul(a) && b == Lb::Po)
            || (a == Lb::Pr && hangul(b))
        {
            return Prohibited;
        }
        // LB28: (AL | HL) × (AL | HL)
        if alpha(a) && alpha(b) {
            return Prohibited;
        }
        // LB28a: Brahmic orthographic syllables
        if (a == Lb::Ap && self.aksara(u))
            || (self.aksara(u - 1) && matches!(b, Lb::Vf | Lb::Vi))
            || (a == Lb::Vi && u >= 2 && self.aksara(u - 2) && self.aksara_base(u))
            || (self.aksara(u - 1) && self.aksara(u) && next == Some(Lb::Vf))
        {
            return Prohibited;
        }
        // LB29: IS × (AL | HL)
        if a == Lb::Is && alpha(b) {
            return Prohibited;
        }
        // LB30: (AL | HL | NU) × [OP - EA]; [CP - EA] × (AL | HL | NU)
        let word = |c: Lb| matches!(c, Lb::Al | Lb::Hl | Lb::Nu);
        if (word(a) && b == Lb::Op && !self.east_asian(u))
            || (a == Lb::Cp && !self.east_asian(u - 1) && word(b))
        {
            return Prohibited;
        }
        // LB30a: pairs of regional indicators
        if a == Lb::Ri && b == Lb::Ri && self.units[u - 1].regional_run % 2 == 1 {
            return Prohibited;
        }
        // LB30b: EB × EM; [ExtPict & Cn] × EM
        if b == Lb::Em && (a == Lb::Eb || is_unassigned_pictographic(self.units[u - 1].base)) {
            return Prohibited;
        }
        // LB31
        Allowed
    }
}
