//! CSS Writing Modes 3 §2: virtual bidi controls, preserving source indexes.
use crate::bidi::{BidiInfo, Level, Paragraph};
use crate::unicode::{BidiClass, bidi_class};
use std::ops::Range;

/// CSS unicode-bidi behavior of an inline scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeStyle {
    /// Directional embedding.
    Embed,
    /// Override character directions.
    Override,
    /// Isolate from surrounding bidi resolution.
    Isolate,
    /// Isolate and override.
    IsolateOverride,
    /// Determine direction from the first strong character.
    Plaintext,
}
/// A nested range in original character indexes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BidiScope {
    /// Original range, excluding the virtual controls.
    pub range: Range<usize>,
    /// Direction for non-plaintext scopes.
    pub rtl: bool,
    /// Requested behavior.
    pub style: ScopeStyle,
}
/// Invalid scope nesting or exhausted expansion budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeError;
impl std::fmt::Display for ScopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid or oversized bidi scopes")
    }
}
impl std::error::Error for ScopeError {}
fn controls(s: &BidiScope) -> (&'static [char], &'static [char]) {
    match (s.style, s.rtl) {
        (ScopeStyle::Embed, false) => (&['\u{202A}'], &['\u{202C}']),
        (ScopeStyle::Embed, true) => (&['\u{202B}'], &['\u{202C}']),
        (ScopeStyle::Override, false) => (&['\u{202D}'], &['\u{202C}']),
        (ScopeStyle::Override, true) => (&['\u{202E}'], &['\u{202C}']),
        (ScopeStyle::Isolate, false) => (&['\u{2066}'], &['\u{2069}']),
        (ScopeStyle::Isolate, true) => (&['\u{2067}'], &['\u{2069}']),
        (ScopeStyle::IsolateOverride, false) => {
            (&['\u{2066}', '\u{202D}'], &['\u{202C}', '\u{2069}'])
        }
        (ScopeStyle::IsolateOverride, true) => {
            (&['\u{2067}', '\u{202E}'], &['\u{202C}', '\u{2069}'])
        }
        (ScopeStyle::Plaintext, _) => (&['\u{2068}'], &['\u{2069}']),
    }
}
/// Expanded Unicode resolution projected onto the original characters.
pub(crate) struct ScopedResolution {
    pub info: BidiInfo,
    pub indexes: Vec<usize>,
    pub paragraphs: Vec<Paragraph>,
}
pub(crate) fn resolve(
    text: &[char],
    base: Option<Level>,
    scopes: &[BidiScope],
) -> Result<ScopedResolution, ScopeError> {
    const LIMIT: usize = 2_000_000;
    if text.len() > LIMIT || scopes.len() > 65_536 {
        return Err(ScopeError);
    }
    let mut ordered: Vec<_> = scopes.iter().filter(|s| !s.range.is_empty()).collect();
    if scopes
        .iter()
        .any(|s| s.range.start > s.range.end || s.range.end > text.len())
    {
        return Err(ScopeError);
    }
    ordered.sort_by_key(|s| (s.range.start, std::cmp::Reverse(s.range.end)));
    let mut stack: Vec<&BidiScope> = Vec::new();
    for s in &ordered {
        while stack.last().is_some_and(|p| p.range.end <= s.range.start) {
            stack.pop();
        }
        if stack.last().is_some_and(|p| s.range.end > p.range.end) || stack.len() >= 125 {
            return Err(ScopeError);
        }
        stack.push(s);
    }
    stack.clear();
    let mut virtuals = Vec::new();
    let mut indexes = Vec::with_capacity(text.len());
    let mut next = 0;
    let append = |out: &mut Vec<char>, part: &[char]| -> Result<(), ScopeError> {
        if out.len().checked_add(part.len()).is_none_or(|n| n > LIMIT) {
            return Err(ScopeError);
        }
        out.extend_from_slice(part);
        Ok(())
    };
    for (i, &c) in text.iter().enumerate() {
        while stack.last().is_some_and(|s| s.range.end == i) {
            append(&mut virtuals, controls(stack.pop().ok_or(ScopeError)?).1)?;
        }
        while next < ordered.len() && ordered[next].range.start == i {
            let s = ordered[next];
            append(&mut virtuals, controls(s).0)?;
            stack.push(s);
            next += 1;
        }
        if bidi_class(c) == BidiClass::B {
            for s in stack.iter().rev() {
                append(&mut virtuals, controls(s).1)?;
            }
        }
        indexes.push(virtuals.len());
        append(&mut virtuals, &[c])?;
        if bidi_class(c) == BidiClass::B {
            for s in &stack {
                append(&mut virtuals, controls(s).0)?;
            }
        }
    }
    while let Some(s) = stack.pop() {
        append(&mut virtuals, controls(s).1)?;
    }
    let info = BidiInfo::new(&virtuals, base);
    let mut paragraphs = Vec::new();
    let mut start = 0;
    for (i, &c) in text.iter().enumerate() {
        if bidi_class(c) == BidiClass::B || i + 1 == text.len() {
            let level = info
                .paragraph_at(indexes[start])
                .map_or(Level::LTR, |p| p.level);
            paragraphs.push(Paragraph {
                range: start..i + 1,
                level,
            });
            start = i + 1;
        }
    }
    Ok(ScopedResolution {
        info,
        indexes,
        paragraphs,
    })
}
