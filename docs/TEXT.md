# Text segmentation and bidi (REN-03b, part 1)

`rustic-text` now carries the Unicode algorithms that line layout needs, all
from scratch in safe Rust with no dependencies. Line boxes, shaping and text
fragments build on them in later REN-03b work; see [LAYOUT.md](LAYOUT.md).

## Unicode data

`tools/generate_text_data.py` generates
`crates/rustic-text/src/unicode/tables.rs` from Unicode 17.0.0 files at the
same pinned `unicode-org/unicodetools` revision the IDNA data uses (unicode.org
itself is blocked from Rustic's cloud sandboxes; the GitHub mirror is
identical). Inputs: `UnicodeData.txt`, `LineBreak.txt`, `EastAsianWidth.txt`,
`emoji/emoji-data.txt`, `auxiliary/GraphemeBreakProperty.txt`,
`DerivedCoreProperties.txt` (InCB), `extracted/DerivedBidiClass.txt`,
`BidiBrackets.txt`, `BidiMirroring.txt` and the four conformance test files.
Every input's SHA-256 and every output's digest are pinned in
`crates/rustic-text/tests/fixtures/text_data_manifest.json`;
`tools/check_repository.py` rejects hand-edited outputs. The generator fails
on property values it does not know, so a Unicode upgrade cannot silently
drop a class. The tables are sorted ranges searched by binary search. The data
is under the [Unicode License v3](../crates/rustic-text/UNICODE-LICENSE.md).

## Segmentation

`segment::grapheme_boundaries` implements UAX #29 extended grapheme clusters
(GB3–GB999, including GB9c Indic conjuncts and GB11 emoji ZWJ sequences).
`segment::line_breaks` implements the default UAX #14 rules LB1–LB31 in their
Unicode 17 form (LB15c/15d, LB19a East Asian quotes, LB20a, LB21a, the
regular-expression LB25 and LB28a Brahmic syllables), returning
`Prohibited`/`Allowed`/`Mandatory` per boundary. Both take decoded scalar
values and return one entry per boundary, so callers keep their own UTF-8 or
UTF-16 offset maps. Each is one forward pass; regional-indicator runs are
counted once and other lookbehind stops at the first non-matching unit, so
long runs stay linear (tested with 200,000-character runs).

No tailoring is applied: the CSS `line-break`, `word-break`,
`overflow-wrap` and `hyphens` properties, dictionary breaking for `SA`
scripts and language-specific rules are not implemented (TD-47).

## Bidi

`bidi::BidiInfo` implements UAX #9: P1 paragraph splitting, P2–P3 (or a forced
base level for CSS `direction`), X1–X10 with the 125-level stack, isolates
and overflow counters, W1–W7, N0 bracket pairs (63-entry stack, canonical
equivalents of U+2329/U+232A), N1–N2, I1–I2, and per line L1
(`line_levels`) and L2 (`reorder`). `unicode::mirrored` gives the L4 mirror
glyph. X9-removed characters keep the preceding character's level so they
never split a run. Rule L3 (combining marks) is not applied (TD-47).

## Evidence

- All 19,338 `LineBreakTest.txt` and 766 `GraphemeBreakTest.txt` cases are
  committed and pass.
- The bidi tests are too large to commit whole (8 MB and 7 MB). Every 64th
  `BidiTest.txt` line (7,670 lines, 12,022 checks across paragraph levels) and
  every 16th `BidiCharacterTest.txt` case (5,732) are committed and pass.
  The full files pass with
  `RUSTIC_UCD_DIR=<dir> cargo test -p rustic-text --test bidi -- --ignored`
  (run on 2026-11-09: 490,846 class lines and 91,707 character cases).
- Unit tests cover mandatory breaks, empty input, paragraph splitting, L1
  trailing whitespace, 300-deep embeddings and long bracketed input.

Specification basis, reviewed 2026-11-09: UAX #14 revision for Unicode 17.0.0,
UAX #29 revision for 17.0.0 and UAX #9 revision for 17.0.0. These are the
Unicode conformance suites, not CSS Text or WPT results.
