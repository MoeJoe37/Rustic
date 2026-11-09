//! The Unicode Bidirectional Algorithm (UAX #9, Unicode 17.0.0): paragraph
//! levels, explicit embeddings and isolates, weak and neutral types with
//! bracket pairs, implicit levels, and the per-line rules L1–L2.
//!
//! The implementation keeps explicit stacks and index lists, so no input can
//! cause recursion; every phase is linear in the paragraph length except the
//! bracket-pair search, which UAX #9 bounds with a 63-entry stack.
//! Mirroring (L4) is exposed through [`crate::unicode::mirrored`]; L3
//! (combining marks) is not applied.

use std::ops::Range;

use crate::unicode::{BidiClass, bidi_class, paired_bracket};

/// A bidi embedding level (0–126). Even levels are left-to-right.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Level(u8);

/// The deepest explicit embedding level (BD2).
pub const MAX_DEPTH: u8 = 125;

impl Level {
    /// Left-to-right paragraph level.
    pub const LTR: Self = Self(0);
    /// Right-to-left paragraph level.
    pub const RTL: Self = Self(1);

    /// A level, or `None` above [`MAX_DEPTH`] + 1 (the highest implicit level).
    pub fn new(level: u8) -> Option<Self> {
        (level <= MAX_DEPTH + 1).then_some(Self(level))
    }

    /// The numeric level.
    pub const fn number(self) -> u8 {
        self.0
    }

    /// Whether text at this level runs right to left.
    pub const fn is_rtl(self) -> bool {
        self.0 % 2 == 1
    }

    fn next_odd(self) -> u8 {
        (self.0 + 1) | 1
    }

    fn next_even(self) -> u8 {
        (self.0 + 2) & !1
    }
}

/// One paragraph found by rule P1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paragraph {
    /// Character range, including its terminating paragraph separator.
    pub range: Range<usize>,
    /// The paragraph embedding level (P2–P3, or the requested base level).
    pub level: Level,
}

/// Resolved embedding levels for a text, before line breaking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BidiInfo {
    classes: Vec<BidiClass>,
    levels: Vec<Level>,
    removed: Vec<bool>,
    paragraphs: Vec<Paragraph>,
}

/// Matching bidi brackets are compared after this canonical mapping (BD16).
fn canonical_bracket(c: char) -> char {
    match c {
        '\u{2329}' => '\u{3008}',
        '\u{232A}' => '\u{3009}',
        other => other,
    }
}

fn strong_or_number(class: BidiClass) -> Option<BidiClass> {
    match class {
        BidiClass::L => Some(BidiClass::L),
        BidiClass::R | BidiClass::Al | BidiClass::En | BidiClass::An => Some(BidiClass::R),
        _ => None,
    }
}

fn is_isolate_initiator(class: BidiClass) -> bool {
    matches!(class, BidiClass::Lri | BidiClass::Rli | BidiClass::Fsi)
}

fn is_neutral_or_isolate(class: BidiClass) -> bool {
    matches!(
        class,
        BidiClass::B
            | BidiClass::S
            | BidiClass::Ws
            | BidiClass::On
            | BidiClass::Lri
            | BidiClass::Rli
            | BidiClass::Fsi
            | BidiClass::Pdi
    )
}

fn removed_by_x9(class: BidiClass) -> bool {
    matches!(
        class,
        BidiClass::Rle
            | BidiClass::Lre
            | BidiClass::Rlo
            | BidiClass::Lro
            | BidiClass::Pdf
            | BidiClass::Bn
    )
}

impl BidiInfo {
    /// Resolve `text`. `base` forces every paragraph's level (CSS
    /// `direction`); `None` uses rules P2–P3 per paragraph.
    pub fn new(text: &[char], base: Option<Level>) -> Self {
        let classes: Vec<BidiClass> = text.iter().map(|&c| bidi_class(c)).collect();
        Self::resolve(classes, Some(text), base)
    }

    /// Resolve a sequence of classes (no bracket pairs, as in BidiTest.txt).
    pub fn from_classes(classes: &[BidiClass], base: Option<Level>) -> Self {
        Self::resolve(classes.to_vec(), None, base)
    }

    fn resolve(classes: Vec<BidiClass>, text: Option<&[char]>, base: Option<Level>) -> Self {
        let n = classes.len();
        let mut info = Self {
            levels: vec![Level::LTR; n],
            removed: classes.iter().map(|&c| removed_by_x9(c)).collect(),
            classes,
            paragraphs: Vec::new(),
        };
        let matching = matching_pdis(&info.classes);
        let mut start = 0;
        while start < n {
            let end = info.classes[start..]
                .iter()
                .position(|&c| c == BidiClass::B)
                .map_or(n, |p| start + p + 1);
            let level =
                base.unwrap_or_else(
                    || match first_strong(&info.classes, &matching, start, end) {
                        Some(BidiClass::R | BidiClass::Al) => Level::RTL,
                        _ => Level::LTR,
                    },
                );
            info.paragraphs.push(Paragraph {
                range: start..end,
                level,
            });
            info.resolve_paragraph(start..end, level, text, &matching);
            start = end;
        }
        info
    }

    fn resolve_paragraph(
        &mut self,
        range: Range<usize>,
        para: Level,
        text: Option<&[char]>,
        matching: &[Option<usize>],
    ) {
        let mut types = self.classes[range.clone()].to_vec();
        let base = range.start;
        explicit_levels(
            &self.classes,
            range.clone(),
            &mut types,
            &mut self.levels[range.clone()],
            para,
            matching,
        );
        let removed = &self.removed[range.clone()];
        let levels = &mut self.levels[range.clone()];
        let original = &self.classes[range.clone()];
        for sequence in isolating_run_sequences(original, removed, levels, matching, base) {
            resolve_sequence(
                &sequence, original, &mut types, levels, removed, para, text, base,
            );
        }
        // I1, I2
        for i in 0..levels.len() {
            if removed[i] {
                continue;
            }
            let level = levels[i];
            levels[i] = Level(match (level.is_rtl(), types[i]) {
                (false, BidiClass::R) => level.0 + 1,
                (false, BidiClass::An | BidiClass::En) => level.0 + 2,
                (true, BidiClass::L | BidiClass::En | BidiClass::An) => level.0 + 1,
                _ => level.0,
            });
        }
        // Removed characters take the preceding level (or the paragraph's),
        // so they never split a run when a line is reordered.
        let mut last = para;
        for i in 0..levels.len() {
            if removed[i] {
                levels[i] = last;
            } else {
                last = levels[i];
            }
        }
    }

    /// Resolved levels before line breaking. Characters removed by X9
    /// (explicit formatting and BN) carry the level of the preceding one.
    pub fn levels(&self) -> &[Level] {
        &self.levels
    }

    /// Whether X9 removed character `i` (it has no visual position).
    pub fn is_removed(&self, i: usize) -> bool {
        self.removed[i]
    }

    /// The paragraphs, in order.
    pub fn paragraphs(&self) -> &[Paragraph] {
        &self.paragraphs
    }

    /// The paragraph containing character `i`.
    pub fn paragraph_at(&self, i: usize) -> Option<&Paragraph> {
        let k = self.paragraphs.partition_point(|p| p.range.end <= i);
        self.paragraphs.get(k).filter(|p| p.range.contains(&i))
    }

    /// Levels for one line after rule L1: separators, trailing whitespace
    /// and whitespace before separators return to the paragraph level.
    /// `line` must lie within one paragraph.
    pub fn line_levels(&self, line: Range<usize>) -> Vec<Level> {
        let mut levels = self.levels[line.clone()].to_vec();
        let para = self
            .paragraph_at(line.start)
            .map_or(Level::LTR, |p| p.level);
        // Scan backwards: whitespace runs that end at a separator or the
        // line end are reset together with that separator.
        let mut trailing = true;
        for i in (0..levels.len()).rev() {
            let class = self.classes[line.start + i];
            match class {
                BidiClass::S | BidiClass::B => {
                    levels[i] = para;
                    trailing = true;
                }
                BidiClass::Ws
                | BidiClass::Fsi
                | BidiClass::Lri
                | BidiClass::Rli
                | BidiClass::Pdi
                    if trailing =>
                {
                    levels[i] = para;
                }
                _ if trailing && self.removed[line.start + i] => levels[i] = para,
                _ => trailing = false,
            }
        }
        levels
    }

    /// Visual order (rule L2) of a line's characters: entry `k` is the
    /// index, relative to the line start, of the `k`th character from the
    /// left. Removed characters are kept at their resolved levels.
    pub fn reorder(levels: &[Level]) -> Vec<usize> {
        let mut order: Vec<usize> = (0..levels.len()).collect();
        let (Some(&highest), Some(&lowest)) = (levels.iter().max(), levels.iter().min()) else {
            return order;
        };
        let lowest_odd = lowest.0 | 1;
        let mut level = highest.0;
        while level >= lowest_odd && level > 0 {
            let mut i = 0;
            while i < order.len() {
                if levels[order[i]].0 >= level {
                    let start = i;
                    while i < order.len() && levels[order[i]].0 >= level {
                        i += 1;
                    }
                    order[start..i].reverse();
                } else {
                    i += 1;
                }
            }
            level -= 1;
        }
        order
    }
}

/// BD9: the matching PDI of every isolate initiator, stopping at paragraph ends.
fn matching_pdis(classes: &[BidiClass]) -> Vec<Option<usize>> {
    let mut matching = vec![None; classes.len()];
    let mut open: Vec<usize> = Vec::new();
    for (i, &class) in classes.iter().enumerate() {
        match class {
            c if is_isolate_initiator(c) => open.push(i),
            BidiClass::Pdi => {
                if let Some(start) = open.pop() {
                    matching[start] = Some(i);
                }
            }
            BidiClass::B => open.clear(),
            _ => {}
        }
    }
    matching
}

/// P2: the first strong class in `start..end`, skipping isolated content.
fn first_strong(
    classes: &[BidiClass],
    matching: &[Option<usize>],
    start: usize,
    end: usize,
) -> Option<BidiClass> {
    let mut i = start;
    while i < end {
        match classes[i] {
            c @ (BidiClass::L | BidiClass::R | BidiClass::Al) => return Some(c),
            c if is_isolate_initiator(c) => i = matching[i]?,
            BidiClass::B => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

#[derive(Clone, Copy)]
struct Status {
    level: Level,
    override_class: Option<BidiClass>,
    isolate: bool,
}

/// X1–X8 for one paragraph; `types` receives overrides.
fn explicit_levels(
    all: &[BidiClass],
    range: Range<usize>,
    types: &mut [BidiClass],
    levels: &mut [Level],
    para: Level,
    matching: &[Option<usize>],
) {
    let base = range.start;
    let classes = &all[range.clone()];
    let mut stack = Vec::with_capacity(usize::from(MAX_DEPTH) + 2);
    stack.push(Status {
        level: para,
        override_class: None,
        isolate: false,
    });
    let (mut overflow_isolates, mut overflow_embeddings, mut valid_isolates) =
        (0usize, 0usize, 0usize);
    for i in 0..classes.len() {
        let top = *stack.last().unwrap_or(&Status {
            level: para,
            override_class: None,
            isolate: false,
        });
        let class = classes[i];
        match class {
            BidiClass::Rle | BidiClass::Lre | BidiClass::Rlo | BidiClass::Lro => {
                levels[i] = top.level;
                let rtl = matches!(class, BidiClass::Rle | BidiClass::Rlo);
                let next = if rtl {
                    top.level.next_odd()
                } else {
                    top.level.next_even()
                };
                if next <= MAX_DEPTH && overflow_isolates == 0 && overflow_embeddings == 0 {
                    stack.push(Status {
                        level: Level(next),
                        override_class: match class {
                            BidiClass::Rlo => Some(BidiClass::R),
                            BidiClass::Lro => Some(BidiClass::L),
                            _ => None,
                        },
                        isolate: false,
                    });
                } else if overflow_isolates == 0 {
                    overflow_embeddings += 1;
                }
            }
            BidiClass::Rli | BidiClass::Lri | BidiClass::Fsi => {
                levels[i] = top.level;
                if let Some(o) = top.override_class {
                    types[i] = o;
                }
                let rtl = match class {
                    BidiClass::Rli => true,
                    BidiClass::Lri => false,
                    _ => {
                        let end = matching[base + i].unwrap_or(range.end);
                        matches!(
                            first_strong(all, matching, base + i + 1, end),
                            Some(BidiClass::R | BidiClass::Al)
                        )
                    }
                };
                let next = if rtl {
                    top.level.next_odd()
                } else {
                    top.level.next_even()
                };
                if next <= MAX_DEPTH && overflow_isolates == 0 && overflow_embeddings == 0 {
                    valid_isolates += 1;
                    stack.push(Status {
                        level: Level(next),
                        override_class: None,
                        isolate: true,
                    });
                } else {
                    overflow_isolates += 1;
                }
            }
            BidiClass::Pdi => {
                if overflow_isolates > 0 {
                    overflow_isolates -= 1;
                } else if valid_isolates > 0 {
                    overflow_embeddings = 0;
                    while stack.last().is_some_and(|s| !s.isolate) {
                        stack.pop();
                    }
                    stack.pop();
                    valid_isolates -= 1;
                }
                let top = *stack.last().unwrap_or(&top);
                levels[i] = top.level;
                if let Some(o) = top.override_class {
                    types[i] = o;
                }
            }
            BidiClass::Pdf => {
                levels[i] = top.level;
                if overflow_isolates > 0 {
                } else if overflow_embeddings > 0 {
                    overflow_embeddings -= 1;
                } else if !top.isolate && stack.len() >= 2 {
                    stack.pop();
                }
            }
            BidiClass::B => levels[i] = para,
            BidiClass::Bn => levels[i] = top.level,
            _ => {
                levels[i] = top.level;
                if let Some(o) = top.override_class {
                    types[i] = o;
                }
            }
        }
    }
}

/// BD13: isolating run sequences as lists of paragraph-relative indexes,
/// excluding characters removed by X9.
fn isolating_run_sequences(
    classes: &[BidiClass],
    removed: &[bool],
    levels: &[Level],
    matching: &[Option<usize>],
    base: usize,
) -> Vec<Vec<usize>> {
    // Level runs over retained characters.
    let mut runs: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut run_level = None;
    for i in 0..classes.len() {
        if removed[i] {
            continue;
        }
        if run_level != Some(levels[i]) && !current.is_empty() {
            runs.push(std::mem::take(&mut current));
        }
        run_level = Some(levels[i]);
        current.push(i);
    }
    if !current.is_empty() {
        runs.push(current);
    }
    // Which run starts with each matching PDI.
    let mut run_of_start = vec![usize::MAX; classes.len()];
    for (r, run) in runs.iter().enumerate() {
        run_of_start[run[0]] = r;
    }
    let mut matched_pdi = vec![false; classes.len()];
    for m in matching[base..base + classes.len()].iter().flatten() {
        if let Some(slot) = matched_pdi.get_mut(m - base) {
            *slot = true;
        }
    }
    let mut sequences = Vec::new();
    for r in 0..runs.len() {
        let first = runs[r][0];
        // A run starting with a PDI that matches an initiator continues a sequence.
        if classes[first] == BidiClass::Pdi && matched_pdi[first] {
            continue;
        }
        let mut sequence = Vec::new();
        let mut run = r;
        loop {
            sequence.extend_from_slice(&runs[run]);
            let last = *runs[run].last().unwrap_or(&first);
            if !is_isolate_initiator(classes[last]) {
                break;
            }
            let Some(pdi) = matching[base + last] else {
                break;
            };
            let next = run_of_start.get(pdi - base).copied().unwrap_or(usize::MAX);
            if next == usize::MAX {
                break;
            }
            run = next;
        }
        sequences.push(sequence);
    }
    sequences
}

#[allow(
    clippy::too_many_arguments,
    reason = "one UAX #9 phase over shared paragraph state"
)]
fn resolve_sequence(
    seq: &[usize],
    original: &[BidiClass],
    types: &mut [BidiClass],
    levels: &[Level],
    removed: &[bool],
    para: Level,
    text: Option<&[char]>,
    base: usize,
) {
    let (Some(&first), Some(&last)) = (seq.first(), seq.last()) else {
        return;
    };
    let level = levels[first];
    let class_of = |l: Level| {
        if l.is_rtl() {
            BidiClass::R
        } else {
            BidiClass::L
        }
    };
    let before = (0..first)
        .rev()
        .find(|&k| !removed[k])
        .map_or(para, |k| levels[k]);
    let after = if is_isolate_initiator(original[last]) {
        para
    } else {
        (last + 1..levels.len())
            .find(|&k| !removed[k])
            .map_or(para, |k| levels[k])
    };
    let sos = class_of(level.max(before));
    let eos = class_of(level.max(after));

    // W1: NSM takes the previous type (ON after an isolate initiator or PDI).
    let mut prev = sos;
    for &i in seq {
        if types[i] == BidiClass::Nsm {
            types[i] = if is_isolate_initiator(prev) || prev == BidiClass::Pdi {
                BidiClass::On
            } else {
                prev
            };
        }
        prev = types[i];
    }
    // W2: EN after AL becomes AN; W3: AL becomes R.
    let mut strong = sos;
    for &i in seq {
        match types[i] {
            c @ (BidiClass::L | BidiClass::R | BidiClass::Al) => strong = c,
            BidiClass::En if strong == BidiClass::Al => types[i] = BidiClass::An,
            _ => {}
        }
    }
    for &i in seq {
        if types[i] == BidiClass::Al {
            types[i] = BidiClass::R;
        }
    }
    // W4: a single separator between two numbers of the same kind.
    for k in 1..seq.len().saturating_sub(1) {
        let (p, c, n) = (types[seq[k - 1]], types[seq[k]], types[seq[k + 1]]);
        types[seq[k]] = match (p, c, n) {
            (BidiClass::En, BidiClass::Es | BidiClass::Cs, BidiClass::En) => BidiClass::En,
            (BidiClass::An, BidiClass::Cs, BidiClass::An) => BidiClass::An,
            _ => c,
        };
    }
    // W5: ET runs adjacent to EN become EN.
    let mut k = 0;
    while k < seq.len() {
        if types[seq[k]] != BidiClass::Et {
            k += 1;
            continue;
        }
        let start = k;
        while k < seq.len() && types[seq[k]] == BidiClass::Et {
            k += 1;
        }
        let touches = (start > 0 && types[seq[start - 1]] == BidiClass::En)
            || (k < seq.len() && types[seq[k]] == BidiClass::En);
        if touches {
            for &i in &seq[start..k] {
                types[i] = BidiClass::En;
            }
        }
    }
    // W6: remaining separators and terminators become ON.
    for &i in seq {
        if matches!(types[i], BidiClass::Es | BidiClass::Et | BidiClass::Cs) {
            types[i] = BidiClass::On;
        }
    }
    // W7: EN after L becomes L.
    let mut strong = sos;
    for &i in seq {
        match types[i] {
            c @ (BidiClass::L | BidiClass::R) => strong = c,
            BidiClass::En if strong == BidiClass::L => types[i] = BidiClass::L,
            _ => {}
        }
    }

    let embedding = class_of(level);
    if let Some(text) = text {
        resolve_brackets(seq, original, types, text, base, sos, embedding);
    }

    // N1, N2
    let mut k = 0;
    while k < seq.len() {
        if !is_neutral_or_isolate(types[seq[k]]) {
            k += 1;
            continue;
        }
        let start = k;
        while k < seq.len() && is_neutral_or_isolate(types[seq[k]]) {
            k += 1;
        }
        let leading = if start == 0 {
            sos
        } else {
            strong_or_number(types[seq[start - 1]]).unwrap_or(embedding)
        };
        let trailing = if k == seq.len() {
            eos
        } else {
            strong_or_number(types[seq[k]]).unwrap_or(embedding)
        };
        let resolved = if leading == trailing {
            leading
        } else {
            embedding
        };
        for &i in &seq[start..k] {
            types[i] = resolved;
        }
    }
}

/// N0: paired brackets take the direction of their content or context.
fn resolve_brackets(
    seq: &[usize],
    original: &[BidiClass],
    types: &mut [BidiClass],
    text: &[char],
    base: usize,
    sos: BidiClass,
    embedding: BidiClass,
) {
    const MAX_STACK: usize = 63;
    // BD16: positions in `seq` of matched pairs.
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let mut stack: Vec<(char, usize)> = Vec::new();
    for (k, &i) in seq.iter().enumerate() {
        if types[i] != BidiClass::On {
            continue;
        }
        let c = text[base + i];
        let Some((pair, open)) = paired_bracket(c) else {
            continue;
        };
        if open {
            if stack.len() == MAX_STACK {
                break;
            }
            stack.push((canonical_bracket(pair), k));
        } else if let Some(depth) = stack
            .iter()
            .rposition(|&(closer, _)| closer == canonical_bracket(c))
        {
            pairs.push((stack[depth].1, k));
            stack.truncate(depth);
        }
    }
    pairs.sort_unstable();
    for (open, close) in pairs {
        let mut found_embedding = false;
        let mut found_opposite = false;
        for &i in &seq[open + 1..close] {
            match strong_or_number(types[i]) {
                Some(c) if c == embedding => found_embedding = true,
                Some(_) => found_opposite = true,
                None => {}
            }
        }
        let resolved = if found_embedding {
            embedding
        } else if found_opposite {
            let context = seq[..open]
                .iter()
                .rev()
                .find_map(|&i| strong_or_number(types[i]))
                .unwrap_or(sos);
            if context != embedding {
                context
            } else {
                embedding
            }
        } else {
            continue;
        };
        for k in [open, close] {
            types[seq[k]] = resolved;
            // NSMs that followed the bracket originally follow it again.
            for &i in &seq[k + 1..] {
                if original[i] != BidiClass::Nsm {
                    break;
                }
                types[i] = resolved;
            }
        }
    }
}
