//! Find in page (UI-01): `render --find TEXT [--find-index N]` marks every
//! case-insensitive match of TEXT in the laid-out text, the active one
//! (match N, counted from 0) more strongly, and reports
//! `find COUNT ACTIVE TOP BOTTOM` (the active match's top and bottom in
//! CSS pixels of the document) so the browser shell can scroll to it.
//!
//! Matches are searched within one text fragment (a run of one text box
//! on one line); a phrase broken across lines or elements is not found,
//! and a match's horizontal extent is estimated from its share of the
//! fragment's characters (TD-195).

use std::cell::RefCell;

use rustic_layout::{BlockLayout, BoxTree};

/// Most matches one search marks.
pub const MAX_MATCHES: usize = 1000;
/// Longest query, in characters.
pub const MAX_QUERY: usize = 256;

/// One search.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    /// What to look for (lowercased once).
    pub text: Vec<char>,
    /// The active match.
    pub index: usize,
}

thread_local! {
    static QUERY: RefCell<Option<Query>> = const { RefCell::new(None) };
}

/// Set (or clear) this thread's search for the next render.
pub fn set(query: Option<Query>) {
    QUERY.with(|slot| *slot.borrow_mut() = query);
}

/// This thread's search.
pub fn query() -> Option<Query> {
    QUERY.with(|slot| slot.borrow().clone())
}

/// Parse `--find`'s value.
pub fn parse_query(text: &str, index: usize) -> Result<Query, String> {
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    if text.is_empty() || text.len() > MAX_QUERY || text.iter().any(|c| c.is_control()) {
        return Err(format!(
            "--find requires 1 to {MAX_QUERY} printable characters"
        ));
    }
    Ok(Query { text, index })
}

/// A match: left, top, width, height in CSS pixels of the document.
pub type Rect = (f64, f64, f64, f64);

/// Every match of `query` in the laid-out text, in layout order.
pub fn matches(tree: &BoxTree, layout: &BlockLayout, query: &Query) -> Vec<Rect> {
    let mut found = Vec::new();
    for fragment in layout.texts() {
        let Some(source) = tree.boxes().get(fragment.source_box()) else {
            continue;
        };
        let Some(units) = source.text().get(fragment.text()) else {
            continue;
        };
        // Characters of the fragment, lowercased, each with its index in
        // the fragment's character sequence (lowercasing may expand).
        let chars: Vec<char> = char::decode_utf16(units.iter().copied())
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect();
        let mut folded: Vec<(char, usize)> = Vec::with_capacity(chars.len());
        for (at, c) in chars.iter().enumerate() {
            folded.extend(c.to_lowercase().map(|l| (l, at)));
        }
        let total = chars.len().max(1) as f64;
        let rect = fragment.rect();
        let (left, top) = (rect.origin().x.get(), rect.origin().y.get());
        let (width, height) = (rect.size().width().get(), rect.size().height().get());
        let n = query.text.len();
        let mut start = 0;
        while start + n <= folded.len() {
            if folded[start..start + n]
                .iter()
                .map(|&(c, _)| c)
                .eq(query.text.iter().copied())
            {
                let first = folded[start].1 as f64;
                let last = folded[start + n - 1].1 as f64 + 1.0;
                found.push((
                    left + width * first / total,
                    top,
                    width * (last - first) / total,
                    height,
                ));
                if found.len() >= MAX_MATCHES {
                    return found;
                }
                start += n;
            } else {
                start += 1;
            }
        }
    }
    found
}

/// Mark `matches` on premultiplied RGBA `pixels` of a `width × height`
/// surface painted at `factor` device pixels per CSS pixel and scrolled to
/// `scroll`: multiplied by yellow, the active one by orange, so text stays
/// readable over it.
pub fn highlight(
    pixels: &mut [u8],
    (width, height): (u32, u32),
    matches: &[Rect],
    active: Option<usize>,
    scroll: (f64, f64),
    factor: f64,
) {
    for (index, &(x, y, w, h)) in matches.iter().enumerate() {
        let tint: [u16; 3] = if Some(index) == active {
            [255, 150, 50]
        } else {
            [255, 235, 60]
        };
        let device = |v: f64| (v * factor).round();
        let left = device(x - scroll.0).clamp(0.0, f64::from(width)) as u32;
        let right = device(x + w - scroll.0).clamp(0.0, f64::from(width)) as u32;
        let top = device(y - scroll.1).clamp(0.0, f64::from(height)) as u32;
        let bottom = device(y + h - scroll.1).clamp(0.0, f64::from(height)) as u32;
        for row in top..bottom {
            for column in left..right {
                let at = ((row * width + column) * 4) as usize;
                let Some(pixel) = pixels.get_mut(at..at + 4) else {
                    continue;
                };
                // Over the white canvas the window shows: treat missing
                // coverage as white so a transparent page highlights too.
                let alpha = u16::from(pixel[3]);
                for (channel, tint) in pixel[..3].iter_mut().zip(tint) {
                    let over_white = u16::from(*channel) + (255 - alpha);
                    *channel = (over_white.min(255) * tint / 255) as u8;
                }
                pixel[3] = 255;
            }
        }
    }
}

/// The `find` line for `matches`: count, the active match (clamped into
/// range) and its top and bottom.
pub fn report(matches: &[Rect], index: usize) -> (Option<usize>, String) {
    if matches.is_empty() {
        return (None, "find 0 0 0 0\n".to_owned());
    }
    let active = index % matches.len();
    let (_, top, _, height) = matches[active];
    (
        Some(active),
        format!(
            "find {} {} {} {}\n",
            matches.len(),
            active,
            top.max(0.0).round(),
            (top + height).max(0.0).round()
        ),
    )
}

/// The live frame's find report: count, active match, its top and bottom.
pub fn found(matches: &[Rect], active: Option<usize>) -> (u32, u32, f64, f64) {
    let count = u32::try_from(matches.len()).unwrap_or(u32::MAX);
    match active.and_then(|index| matches.get(index).map(|m| (index, m))) {
        Some((index, &(_, top, _, height))) => (
            count,
            u32::try_from(index).unwrap_or(0),
            top.max(0.0),
            (top + height).max(0.0),
        ),
        None => (count, 0, 0.0, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_lowercased_and_bounded() {
        assert_eq!(parse_query("AbC", 2).unwrap().text, vec!['a', 'b', 'c']);
        assert!(parse_query("", 0).is_err());
        assert!(parse_query("a\nb", 0).is_err());
        assert!(parse_query(&"x".repeat(MAX_QUERY + 1), 0).is_err());
    }

    #[test]
    fn highlights_tint_only_the_match_and_stay_in_bounds() {
        let mut pixels = [255u8, 255, 255, 255].repeat(4 * 2);
        // A match covering x 1..3 on row 0 at scale 1, the active one.
        highlight(
            &mut pixels,
            (4, 2),
            &[(1.0, 0.0, 2.0, 1.0), (-50.0, -50.0, 500.0, 500.0)],
            Some(0),
            (0.0, 0.0),
            1.0,
        );
        // Everything is covered by the second (huge) match, clamped to the
        // surface; pixel 1 was tinted orange first, then yellow.
        assert_eq!(&pixels[4..8], &[255, 138, 11, 255]);
        assert_eq!(&pixels[0..4], &[255, 235, 60, 255]);
        let (active, line) = report(&[(0.0, 10.0, 5.0, 12.0), (0.0, 40.0, 5.0, 12.0)], 3);
        assert_eq!(active, Some(1));
        assert_eq!(line, "find 2 1 40 52\n");
        assert_eq!(report(&[], 0), (None, "find 0 0 0 0\n".to_owned()));
    }
}
