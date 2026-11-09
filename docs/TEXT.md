# Text segmentation, bidi and shaping (REN-03b)

`rustic-text` carries the Unicode algorithms and the simple shaper that line
layout needs, all from scratch in safe Rust with no dependencies. Line boxes
and text fragments are built on them in `rustic-layout`; see
[LAYOUT.md](LAYOUT.md#inline-formatting-and-text-fragments-ren-03b).

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

## Shaping

`shape::shape(database, chain, text, options)` turns decoded text into
`ShapedCluster`s, one per UAX #29 extended grapheme cluster:

- **Font fallback per cluster.** `FontDatabase::fallback_chain(families,
  query)` resolves each `font-family` entry (generic families through the
  database's generic map) to its best face for the weight/style/stretch query,
  then appends the fallback family; its first entry is the CSS *first
  available font*. A cluster takes the first face that maps all its visible
  characters, else the first that maps its first character, else the chain's
  first face with `.notdef`. A cluster is never split across faces.
- **Glyphs and advances.** Each visible character maps through `cmap` to one
  glyph with its `hmtx` advance scaled to the used font size. Default
  ignorables (ZWJ/ZWNJ, variation selectors, bidi controls, ZWSP, soft hyphen
  and similar, `shape::is_invisible`) produce no glyph.
- **Mirroring.** In right-to-left runs a character with a `Bidi_Mirroring_Glyph`
  uses that glyph when the face has it (UAX #9 L4).
- **Spacing.** `letter-spacing` is added once after every visible cluster and
  `word-spacing` to every word-separator character (CSS Text 3 §8).

There is no OpenType layout: GSUB/GPOS/`kern` are not applied, so ligatures,
kerning, contextual forms, mark positioning and complex scripts such as Arabic
joining or Indic reordering are not shaped (TD-50). `shape` is pure and
width-independent, so layout shapes each run once.

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
- `tests/shape.rs` uses the bundled test faces (which map only `x` and the
  space): fallback-chain resolution, clusters kept on one face, `.notdef`,
  per-face scaling, invisible characters, missing fonts and spacing. The test
  faces have no bracket glyphs, so mirrored-glyph substitution itself is not
  yet exercised by a font that has them.

Specification basis, reviewed 2026-11-09: UAX #14 revision for Unicode 17.0.0,
UAX #29 revision for 17.0.0 and UAX #9 revision for 17.0.0. These are the
Unicode conformance suites, not CSS Text or WPT results.
