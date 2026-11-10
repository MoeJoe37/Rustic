//! Line boxes (CSS 2.2 §9.4.2, §10.8; CSS Text 3 §4.1.2, §5, §7): greedy
//! breaking at UAX #14 opportunities, float avoidance, white-space trimming
//! and hanging, bidi reordering (UAX #9 L1–L2), `text-align`/`text-indent`,
//! and `vertical-align` with baselines.
//!
//! Positions here use the block formatting context's coordinates; the flow
//! engine converts them to absolute fragments.

use rustic_css::{Direction, PropertyId as P, PropertyValue as V, TextAlign};
use rustic_text::GlyphId;
use rustic_text::bidi::{BidiInfo, Level};
use rustic_text::segment::BreakOpportunity;

use crate::exclusions::Exclusions;
use crate::flow::{FlowError, Work};
use crate::inline::{Align, FontBox, Ifc, Piece, Space};
use crate::intrinsic::Intrinsic;
use crate::sizing::lp;

const EPSILON: f64 = 1e-9;
/// Re-measurements of one line whose height revealed narrower float bands.
const MAX_BAND_RETRIES: usize = 4;

/// A placed line, in formatting-context coordinates.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RawLine {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub baseline: f64,
}

/// A placed run of glyphs from one text box, face and bidi level.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RawText {
    pub line: usize,
    pub text: usize,
    pub utf16: (usize, usize),
    pub x: f64,
    pub width: f64,
    pub top: f64,
    pub height: f64,
    pub baseline: f64,
    pub level: u8,
    pub face: usize,
    pub size: f64,
    pub glyphs: Vec<(GlyphId, f64, f64)>,
}

/// One inline box's fragment on one line.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RawInline {
    pub line: usize,
    pub ibox: usize,
    /// Border box (x, y, width, height).
    pub border: (f64, f64, f64, f64),
    pub padding: (f64, f64, f64, f64),
    pub content: (f64, f64, f64, f64),
    pub starts: bool,
    pub ends: bool,
}

/// An atomic inline's margin box position on a line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RawAtomic {
    pub line: usize,
    pub atomic: usize,
    /// Top-left corner of the margin box.
    pub x: f64,
    pub y: f64,
}

/// The lines of one inline formatting context.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LineSet {
    pub lines: Vec<RawLine>,
    pub texts: Vec<RawText>,
    pub inlines: Vec<RawInline>,
    pub atomics: Vec<RawAtomic>,
}

/// A chosen line: pieces `start..end` with their measured widths.
struct Break {
    start: usize,
    end: usize,
    widths: Vec<f64>,
    /// The first unbreakable segment did not fit.
    overflow: bool,
    forced: bool,
    /// Nothing that needs a line box (CSS 2.2 §9.4.2 empty lines).
    empty: bool,
    hyphen: Option<usize>,
}

/// Greedy breaking from `start` within `available` pixels after `indent`;
/// `atomic` holds the outer width of each atomic inline.
fn break_line(
    ifc: &Ifc<'_>,
    start: usize,
    available: f64,
    indent: f64,
    atomic: &[f64],
    use_emergency: bool,
) -> Break {
    let n = ifc.pieces.len();
    let mut committed = indent;
    let mut pending = 0.0;
    let mut widths = Vec::new();
    let mut best: Option<(usize, usize)> = None;
    let mut emergency_best: Option<(usize, usize)> = None;
    let mut overflow = false;
    let mut seen_text = false;
    let mut needs_line = false;
    let finish =
        |end: usize, mut widths: Vec<f64>, overflow: bool, forced: bool, needs_line: bool| {
            let hyphen = if !forced && end < n && end > start && ifc.hyphens[end - 1].is_some() {
                Some(end - 1)
            } else {
                None
            };
            if let Some(k) = hyphen {
                if let Some(w) = widths.last_mut() {
                    *w += ifc.hyphens[k].as_ref().map_or(0.0, |h| h.width);
                }
            }
            Break {
                start,
                end,
                widths,
                overflow,
                forced,
                empty: !needs_line,
                hyphen,
            }
        };
    for k in start..n {
        let piece = &ifc.pieces[k];
        let space = piece.space();
        let mut w = match piece {
            Piece::Edge { width, .. } => *width,
            Piece::Cluster { width, .. } => *width,
            Piece::Atomic { atomic: a, .. } => atomic.get(*a).copied().unwrap_or(0.0),
        };
        if let Space::Tab(interval) = space {
            let x = committed + pending - indent;
            w = if interval > EPSILON {
                interval - x.rem_euclid(interval)
            } else {
                0.0
            };
        }
        match (piece, space) {
            (Piece::Cluster { .. }, Space::Collapsible) if !seen_text => w = 0.0,
            (Piece::Cluster { .. }, Space::Collapsible | Space::Hanging) => pending += w,
            (Piece::Edge { .. }, _) => committed += w,
            _ => {
                committed += pending + w;
                pending = 0.0;
            }
        }
        if let Piece::Cluster { .. } | Piece::Atomic { .. } = piece {
            if space != Space::Collapsible {
                seen_text = true;
                needs_line = true;
            }
        } else if w != 0.0 {
            needs_line = true;
        }
        widths.push(w);
        let content = !matches!(space, Space::Collapsible | Space::Hanging);
        if content && committed > available + EPSILON {
            if let Some((end, len)) = best.or(if use_emergency { emergency_best } else { None }) {
                widths.truncate(len);
                return finish(end, widths, false, false, true);
            }
            overflow = true;
        }
        match ifc.breaks[k] {
            Some(BreakOpportunity::Mandatory) => {
                return finish(k + 1, widths, overflow, true, true);
            }
            Some(BreakOpportunity::Allowed) if k + 1 < n => {
                if overflow {
                    return finish(k + 1, widths, true, false, needs_line);
                }
                let extra = ifc.hyphens[k].as_ref().map_or(0.0, |h| h.width);
                if committed + extra <= available + EPSILON {
                    best = Some((k + 1, widths.len()));
                }
            }
            _ => {}
        }
        if use_emergency && ifc.emergency[k] && k + 1 < n {
            if overflow {
                return finish(k + 1, widths, true, false, needs_line);
            }
            emergency_best = Some((k + 1, widths.len()));
        }
    }
    finish(n, widths, overflow, false, needs_line)
}

/// CSS 2.2 §10.3.5 intrinsic contributions: the widest unbreakable segment
/// and the widest line between forced breaks (plus a fixed `text-indent`).
pub(crate) fn intrinsic(ifc: &Ifc<'_>, work: &mut Work<'_>) -> Result<Intrinsic, FlowError> {
    let indent = match ifc.root.get(P::TextIndent) {
        V::LengthPercentage(v) if !matches!(v, rustic_css::LengthPercentage::Percentage(_)) => {
            lp(v, 0.0)?
        }
        _ => 0.0,
    };
    let mut result = Intrinsic::default();
    for (available, slot) in [(0.0, 0), (f64::INFINITY, 1)] {
        // Atomic inlines contribute their outer min-/max-content sizes.
        let atomic: Vec<f64> = ifc
            .atomics
            .iter()
            .map(|a| {
                if slot == 0 {
                    a.contribution.min
                } else {
                    a.contribution.max
                }
            })
            .collect();
        let mut start = 0;
        let mut first = true;
        while start < ifc.pieces.len() {
            let line = break_line(
                ifc,
                start,
                available,
                if first { indent } else { 0.0 },
                &atomic,
                false,
            );
            work.charge(line.end - start)?;
            let width = trimmed_width(ifc, &line) + if first { indent } else { 0.0 };
            if slot == 0 {
                result.min = result.min.max(width);
            } else {
                result.max = result.max.max(width);
            }
            first = false;
            start = line.end.max(start + 1);
        }
    }
    result.max = result.max.max(result.min);
    Ok(result)
}

/// Pieces of a line that are drawn: not leading/trailing collapsible spaces.
fn trailing(ifc: &Ifc<'_>, line: &Break) -> (Vec<bool>, Vec<bool>) {
    let len = line.end - line.start;
    let mut dropped = vec![false; len];
    let mut hanging = vec![false; len];
    // Leading collapsible spaces were measured as zero.
    for (i, k) in (line.start..line.end).enumerate() {
        match ifc.pieces[k].space() {
            Space::Collapsible => dropped[i] = true,
            Space::None if matches!(ifc.pieces[k], Piece::Edge { .. }) => {}
            _ => break,
        }
    }
    for (i, k) in (line.start..line.end).enumerate().rev() {
        match (&ifc.pieces[k], ifc.pieces[k].space()) {
            (Piece::Edge { .. }, _) | (_, Space::Forced) => {}
            (_, Space::Collapsible) => dropped[i] = true,
            (_, Space::Hanging) => hanging[i] = true,
            _ => break,
        }
    }
    (dropped, hanging)
}

fn trimmed_width(ifc: &Ifc<'_>, line: &Break) -> f64 {
    let (dropped, hanging) = trailing(ifc, line);
    line.widths
        .iter()
        .enumerate()
        .filter(|&(i, _)| !dropped[i] && !hanging[i])
        .map(|(_, w)| w)
        .sum()
}

/// Inputs that do not change between the lines of one context.
pub(crate) struct LineArea<'e> {
    pub left: f64,
    pub width: f64,
    pub top: f64,
    pub exclusions: Option<&'e Exclusions>,
}

/// Lay out every line; returns the lines and the bottom of the last one.
pub(crate) fn layout(
    ifc: &Ifc<'_>,
    area: &LineArea<'_>,
    work: &mut Work<'_>,
) -> Result<(LineSet, f64), FlowError> {
    let indent = match ifc.root.get(P::TextIndent) {
        V::LengthPercentage(v) => lp(v, area.width)?,
        _ => return Err(FlowError::InvalidSnapshot),
    };
    let align = match ifc.root.get(P::TextAlign) {
        V::TextAlign(a) => *a,
        _ => return Err(FlowError::InvalidSnapshot),
    };
    let atomic: Vec<f64> = ifc.atomics.iter().map(|a| a.size.0).collect();
    let mut set = LineSet::default();
    let mut y = area.top;
    let mut start = 0;
    let mut first = true;
    let (left, right) = (area.left, area.left + area.width);
    while start < ifc.pieces.len() {
        let mut guess = ifc.root_font.line_height.max(EPSILON);
        let mut retries = 0;
        loop {
            let band = match area.exclusions {
                Some(e) if !e.is_empty() => e.band(y, y + guess, left, right, work)?,
                _ => crate::exclusions::Band {
                    left,
                    right,
                    intruded: false,
                },
            };
            let available = (band.right - band.left).max(0.0);
            let line = break_line(
                ifc,
                start,
                available,
                if first { indent } else { 0.0 },
                &atomic,
                true,
            );
            work.charge(line.end - start)?;
            if line.overflow && band.intruded {
                if let Some(e) = area.exclusions {
                    if let Some(next) = e.next_bottom(y, y + guess, work)? {
                        y = next;
                        continue;
                    }
                }
            }
            if line.empty {
                start = line.end.max(start + 1);
                break;
            }
            let placed = place(
                ifc,
                &line,
                band.left,
                available,
                y,
                if first { indent } else { 0.0 },
                align,
                set.lines.len(),
                work,
            )?;
            if placed.0.height > guess + EPSILON && retries < MAX_BAND_RETRIES {
                if let Some(e) = area.exclusions.filter(|e| !e.is_empty()) {
                    let taller = e.band(y, y + placed.0.height, left, right, work)?;
                    if taller.left != band.left || taller.right != band.right {
                        guess = placed.0.height;
                        retries += 1;
                        continue;
                    }
                }
            }
            let (line_box, texts, inlines, atomics) = placed;
            y += line_box.height;
            set.lines.push(line_box);
            set.texts.extend(texts);
            set.inlines.extend(inlines);
            set.atomics.extend(atomics);
            first = false;
            start = line.end.max(start + 1);
            break;
        }
    }
    Ok((set, y))
}

/// Visual left-to-right order and levels of a line's drawn pieces.
fn visual_order(ifc: &Ifc<'_>, line: &Break, drawn: &[usize]) -> (Vec<usize>, Vec<Level>) {
    let para = if ifc.direction == Direction::Rtl {
        Level::RTL
    } else {
        Level::LTR
    };
    let chars: Vec<std::ops::Range<usize>> = (line.start..line.end)
        .filter_map(|k| ifc.pieces[k].chars().cloned())
        .collect();
    let (c0, c1) = match (chars.first(), chars.last()) {
        (Some(a), Some(b)) => (a.start, b.end),
        _ => (0, 0),
    };
    let line_levels = if c1 > c0 {
        ifc.bidi.line_levels(c0..c1)
    } else {
        Vec::new()
    };
    let level_at = |c: usize| line_levels.get(c.wrapping_sub(c0)).copied().unwrap_or(para);
    let mut levels = Vec::with_capacity(drawn.len());
    for (i, &k) in drawn.iter().enumerate() {
        let level_of = |j: usize| ifc.pieces[j].chars().map(|c| level_at(c.start));
        levels.push(match &ifc.pieces[k] {
            Piece::Cluster { chars, .. } | Piece::Atomic { chars, .. } => level_at(chars.start),
            Piece::Edge { open: true, .. } => {
                drawn[i..].iter().find_map(|&j| level_of(j)).unwrap_or(para)
            }
            Piece::Edge { open: false, .. } => drawn[..i]
                .iter()
                .rev()
                .find_map(|&j| level_of(j))
                .unwrap_or(para),
        });
    }
    (BidiInfo::reorder(&levels), levels)
}

type PlacedLine = (RawLine, Vec<RawText>, Vec<RawInline>, Vec<RawAtomic>);

#[allow(clippy::too_many_arguments, reason = "one line's geometry inputs")]
fn place(
    ifc: &Ifc<'_>,
    line: &Break,
    band_left: f64,
    available: f64,
    y: f64,
    indent: f64,
    align: TextAlign,
    index: usize,
    work: &mut Work<'_>,
) -> Result<PlacedLine, FlowError> {
    let (dropped, hanging) = trailing(ifc, line);
    let drawn: Vec<usize> = (line.start..line.end)
        .filter(|&k| !dropped[k - line.start])
        .collect();
    work.charge(drawn.len())?;
    let width_of = |k: usize| line.widths[k - line.start];
    let content: f64 = drawn
        .iter()
        .filter(|&&k| !hanging[k - line.start])
        .map(|&k| width_of(k))
        .sum::<f64>()
        + indent;
    let rtl = if matches!(
        ifc.root.get(P::UnicodeBidi),
        V::UnicodeBidi(rustic_css::UnicodeBidi::Plaintext)
    ) {
        (line.start..line.end)
            .find_map(|k| ifc.pieces[k].chars())
            .and_then(|c| ifc.bidi.paragraph_at(c.start))
            .is_some_and(|p| p.level.is_rtl())
    } else {
        ifc.direction == Direction::Rtl
    };
    let free = available - content;
    let last = line.forced || line.end >= ifc.pieces.len();
    let justify: Vec<usize> = if align == TextAlign::Justify && !last && free > EPSILON {
        drawn
            .iter()
            .copied()
            .filter(|&k| {
                !hanging[k - line.start]
                    && matches!(
                        ifc.pieces[k],
                        Piece::Cluster {
                            justifiable: true,
                            ..
                        }
                    )
            })
            .collect()
    } else {
        Vec::new()
    };
    let extra = if justify.is_empty() {
        0.0
    } else {
        free / justify.len() as f64
    };
    let mut justified = vec![false; line.end - line.start];
    for &k in &justify {
        justified[k - line.start] = true;
    }
    let offset = if !justify.is_empty() {
        0.0
    } else if free < 0.0 {
        // Overflowing content is start-aligned (CSS Text 3 §7.1).
        if rtl { free } else { 0.0 }
    } else {
        match (align, rtl) {
            (TextAlign::Center, _) => free / 2.0,
            (TextAlign::Left, _) => 0.0,
            (TextAlign::Right, _) => free,
            (TextAlign::Start | TextAlign::MatchParent | TextAlign::Justify, false)
            | (TextAlign::End, true) => 0.0,
            (TextAlign::Start | TextAlign::MatchParent | TextAlign::Justify, true)
            | (TextAlign::End, false) => free,
        }
    };

    // Vertical metrics (CSS 2.2 §10.8.1): offsets from each alignment
    // root's baseline, positive downwards.
    // Alignment subjects: inline boxes `0..nb`, then atomic inlines.
    let nb = ifc.boxes.len();
    let na = ifc.atomics.len();
    let mut present = vec![false; nb];
    let mut atomic_present = vec![false; na];
    let mark = |mut b: Option<usize>, present: &mut Vec<bool>, work: &mut Work<'_>| {
        while let Some(i) = b {
            if present[i] {
                break;
            }
            present[i] = true;
            b = ifc.boxes[i].parent;
            work.charge(1)?;
        }
        Ok::<(), FlowError>(())
    };
    for &k in &drawn {
        match &ifc.pieces[k] {
            Piece::Cluster { text, .. } => mark(ifc.texts[*text].parent, &mut present, work)?,
            Piece::Edge { ibox, .. } => mark(Some(*ibox), &mut present, work)?,
            Piece::Atomic { atomic, .. } => {
                atomic_present[*atomic] = true;
                mark(ifc.atomics[*atomic].parent, &mut present, work)?;
            }
        }
    }
    let layout_top = |f: &FontBox| -(f.ascent + f.half_leading());
    let layout_bottom = |f: &FontBox| f.descent + f.half_leading();
    // Alignment groups: None is the root inline box; Some(b) a top/bottom box.
    let mut offset_of = vec![0.0; nb + na];
    let mut group_of: Vec<Option<usize>> = vec![None; nb + na];
    let mut root_extent = (layout_top(&ifc.root_font), layout_bottom(&ifc.root_font));
    let mut groups: Vec<(usize, f64, f64)> = Vec::new();
    for b in 0..nb {
        if !present[b] {
            continue;
        }
        let ib = &ifc.boxes[b];
        let (po, pg, pf) = match ib.parent {
            Some(p) => (offset_of[p], group_of[p], ifc.boxes[p].font),
            None => (0.0, None, ifc.root_font),
        };
        let f = ib.font;
        let (o, g) = match ib.align {
            Align::Shift(s) => (po + s, pg),
            Align::TextTop => (po - pf.ascent + f.ascent + f.half_leading(), pg),
            Align::TextBottom => (po + pf.descent - f.descent - f.half_leading(), pg),
            Align::Middle => (po - pf.x_height / 2.0 - (f.descent - f.ascent) / 2.0, pg),
            Align::Top | Align::Bottom => (0.0, Some(b)),
        };
        offset_of[b] = o;
        group_of[b] = g;
        let (t, bo) = (o + layout_top(&f), o + layout_bottom(&f));
        match g {
            None => root_extent = (root_extent.0.min(t), root_extent.1.max(bo)),
            Some(root) => match groups.iter_mut().find(|e| e.0 == root) {
                Some(e) => {
                    e.1 = e.1.min(t);
                    e.2 = e.2.max(bo);
                }
                None => groups.push((root, t, bo)),
            },
        }
    }
    // Atomic inlines align their margin boxes (CSS 2.2 §10.8.1), with the
    // baseline the flow engine found (or the bottom margin edge).
    for a in 0..na {
        if !atomic_present[a] {
            continue;
        }
        work.charge(1)?;
        let at = &ifc.atomics[a];
        let (po, pg, pf) = match at.parent {
            Some(p) => (offset_of[p], group_of[p], ifc.boxes[p].font),
            None => (0.0, None, ifc.root_font),
        };
        let height = at.size.1;
        let (above, below) = (at.baseline, height - at.baseline);
        let (o, g) = match at.align {
            Align::Shift(s) => (po + s, pg),
            Align::TextTop => (po - pf.ascent + above, pg),
            Align::TextBottom => (po + pf.descent - below, pg),
            Align::Middle => (po - pf.x_height / 2.0 + above - height / 2.0, pg),
            Align::Top | Align::Bottom => (0.0, Some(nb + a)),
        };
        offset_of[nb + a] = o;
        group_of[nb + a] = g;
        let (t, bo) = (o - above, o + below);
        match g {
            None => root_extent = (root_extent.0.min(t), root_extent.1.max(bo)),
            Some(root) => match groups.iter_mut().find(|e| e.0 == root) {
                Some(e) => {
                    e.1 = e.1.min(t);
                    e.2 = e.2.max(bo);
                }
                None => groups.push((root, t, bo)),
            },
        }
    }
    let align_of = |s: usize| {
        if s < nb {
            ifc.boxes[s].align
        } else {
            ifc.atomics[s - nb].align
        }
    };
    let mut height = root_extent.1 - root_extent.0;
    for &(_, t, b) in &groups {
        height = height.max(b - t);
    }
    let height = height.max(0.0);
    let root_baseline = y - root_extent.0;
    let baseline_of = |b: Option<usize>| -> f64 {
        match b {
            None => root_baseline,
            Some(b) => {
                let group = match group_of[b] {
                    None => root_baseline,
                    Some(g) => {
                        let (_, t, bo) = groups
                            .iter()
                            .copied()
                            .find(|e| e.0 == g)
                            .unwrap_or((g, 0.0, 0.0));
                        match align_of(g) {
                            Align::Bottom => y + height - bo,
                            _ => y - t,
                        }
                    }
                };
                group + offset_of[b]
            }
        }
    };
    let relative = |mut b: Option<usize>| {
        let (mut dx, mut dy) = (0.0, 0.0);
        while let Some(i) = b {
            dx += ifc.boxes[i].relative.0;
            dy += ifc.boxes[i].relative.1;
            b = ifc.boxes[i].parent;
        }
        (dx, dy)
    };

    // Horizontal placement in visual order.
    let (order, levels) = visual_order(ifc, line, &drawn);
    let mut x = band_left + offset + if rtl { 0.0 } else { indent };
    let mut texts: Vec<RawText> = Vec::new();
    let mut atomics: Vec<RawAtomic> = Vec::new();
    let mut extents: Vec<Option<(f64, f64)>> = vec![None; nb];
    let mut edges: Vec<(bool, bool)> = vec![(false, false); nb];
    for &v in &order {
        let k = drawn[v];
        let level = levels[v];
        let mut w = width_of(k);
        if justified[k - line.start] {
            w += extra;
        }
        let owner = match &ifc.pieces[k] {
            Piece::Cluster { text, .. } => ifc.texts[*text].parent,
            Piece::Edge { ibox, .. } => Some(*ibox),
            Piece::Atomic { atomic, .. } => ifc.atomics[*atomic].parent,
        };
        let mut b = owner;
        while let Some(i) = b {
            work.charge(1)?;
            let e = extents[i].get_or_insert((x, x + w));
            e.0 = e.0.min(x);
            e.1 = e.1.max(x + w);
            b = ifc.boxes[i].parent;
        }
        match &ifc.pieces[k] {
            Piece::Atomic { atomic, .. } => {
                let at = &ifc.atomics[*atomic];
                let (dx, dy) = relative(at.parent);
                atomics.push(RawAtomic {
                    line: index,
                    atomic: *atomic,
                    x: x + dx,
                    y: baseline_of(Some(nb + *atomic)) - at.baseline + dy,
                });
            }
            Piece::Edge { ibox, open, .. } => {
                let ltr = ifc.boxes[*ibox].direction == Direction::Ltr;
                // (left edge on this line, right edge on this line)
                if *open == ltr {
                    edges[*ibox].0 = true;
                } else {
                    edges[*ibox].1 = true;
                }
            }
            Piece::Cluster {
                chars,
                text,
                face,
                glyphs,
                ..
            } => {
                let item = &ifc.texts[*text];
                let parent = item.parent;
                let font = parent.map_or(ifc.root_font, |p| ifc.boxes[p].font);
                let (dx, dy) = relative(parent);
                let baseline = baseline_of(parent) + dy;
                let src = (
                    ifc.sources[chars.start].start,
                    ifc.sources[chars.end - 1].end,
                );
                let hyphen_width = if line.hyphen == Some(k) {
                    ifc.hyphens[k].as_ref().map_or(0.0, |h| h.width)
                } else {
                    0.0
                };
                let text_width = w - hyphen_width;
                let mut pen = x + dx + if level.is_rtl() { hyphen_width } else { 0.0 };
                let mut placed: Vec<(GlyphId, f64, f64)> = Vec::with_capacity(glyphs.len());
                for g in glyphs {
                    placed.push((g.glyph, pen + g.x_offset, baseline - g.y_offset));
                    pen += g.advance;
                }
                let joins = texts.last().is_some_and(|t: &RawText| {
                    t.text == *text
                        && t.face == *face
                        && t.level == level.number()
                        && (t.x + t.width - (x + dx)).abs() < EPSILON
                });
                if joins {
                    if let Some(t) = texts.last_mut() {
                        t.width += text_width;
                        t.utf16 = (t.utf16.0.min(src.0), t.utf16.1.max(src.1));
                        t.glyphs.extend(placed);
                    }
                } else {
                    texts.push(RawText {
                        line: index,
                        text: *text,
                        utf16: src,
                        x: x + dx,
                        width: text_width,
                        top: baseline - font.ascent,
                        height: font.ascent + font.descent,
                        baseline,
                        level: level.number(),
                        face: *face,
                        size: item.font.size,
                        glyphs: placed,
                    });
                }
            }
        }
        if line.hyphen == Some(k) {
            if let Some(h) = &ifc.hyphens[k] {
                let item = &ifc.texts[h.text];
                let parent = item.parent;
                let (dx, dy) = relative(parent);
                let font = parent.map_or(ifc.root_font, |p| ifc.boxes[p].font);
                let baseline = baseline_of(parent) + dy;
                let mut pen = if level.is_rtl() {
                    x + dx
                } else {
                    x + w - h.width + dx
                };
                let mut glyphs = Vec::new();
                for g in &h.glyphs {
                    glyphs.push((g.glyph, pen + g.x_offset, baseline - g.y_offset));
                    pen += g.advance;
                }
                let source = ifc.sources[h.at];
                let start = if ifc.chars[h.at] == '\u{AD}' {
                    source.start
                } else {
                    source.end
                };
                texts.push(RawText {
                    line: index,
                    text: h.text,
                    utf16: (start, source.end),
                    x: if level.is_rtl() {
                        x + dx
                    } else {
                        x + w - h.width + dx
                    },
                    width: h.width,
                    top: baseline - font.ascent,
                    height: font.ascent + font.descent,
                    baseline,
                    level: level.number(),
                    face: h.face,
                    size: item.font.size,
                    glyphs,
                });
            }
        }
        x += w;
    }
    let mut inlines = Vec::new();
    for b in 0..nb {
        let Some((x0, x1)) = extents[b] else {
            continue;
        };
        let ib = &ifc.boxes[b];
        let (left, right) = edges[b];
        let (dx, dy) = relative(Some(b));
        let bx0 = x0 + if left { ib.margin.0 } else { 0.0 } + dx;
        let bx1 = (x1 - if right { ib.margin.1 } else { 0.0 } + dx).max(bx0);
        let px0 = bx0 + if left { ib.border.0 } else { 0.0 };
        let px1 = (bx1 - if right { ib.border.1 } else { 0.0 }).max(px0);
        let cx0 = px0 + if left { ib.padding.0 } else { 0.0 };
        let cx1 = (px1 - if right { ib.padding.1 } else { 0.0 }).max(cx0);
        let baseline = baseline_of(Some(b)) + dy;
        let ctop = baseline - ib.font.ascent;
        let cheight = ib.font.ascent + ib.font.descent;
        let ptop = ctop - ib.padding_v.0;
        let pheight = cheight + ib.padding_v.0 + ib.padding_v.1;
        let btop = ptop - ib.border_v.0;
        let bheight = pheight + ib.border_v.0 + ib.border_v.1;
        let ltr = ib.direction == Direction::Ltr;
        inlines.push(RawInline {
            line: index,
            ibox: b,
            border: (bx0, btop, bx1 - bx0, bheight),
            padding: (px0, ptop, px1 - px0, pheight),
            content: (cx0, ctop, cx1 - cx0, cheight),
            starts: if ltr { left } else { right },
            ends: if ltr { right } else { left },
        });
    }
    Ok((
        RawLine {
            x: band_left,
            y,
            width: available,
            height,
            baseline: root_baseline,
        },
        texts,
        inlines,
        atomics,
    ))
}

/// Shift every coordinate of a line set.
pub(crate) fn translate(set: &mut LineSet, dx: f64, dy: f64) {
    for l in &mut set.lines {
        l.x += dx;
        l.y += dy;
        l.baseline += dy;
    }
    for t in &mut set.texts {
        t.x += dx;
        t.top += dy;
        t.baseline += dy;
        for g in &mut t.glyphs {
            g.1 += dx;
            g.2 += dy;
        }
    }
    for f in &mut set.inlines {
        for r in [&mut f.border, &mut f.padding, &mut f.content] {
            r.0 += dx;
            r.1 += dy;
        }
    }
    for a in &mut set.atomics {
        a.x += dx;
        a.y += dy;
    }
}
