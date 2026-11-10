# Text, fonts and inline policies (REN-03 completion)

Rustic owns Unicode 17 segmentation/bidi, CSS font selection, source mapping,
wrapping, inline layout and paint integration. Reviewed low-level libraries
provide OpenType shaping, CFF outlines and web-font decoding; their boundary,
versions and audit policy are in [ADR 0014](adr/0014-reviewed-font-adapters.md).
The original validated TrueType parser remains in use.

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

## Font formats and discovery

`Face::parse` accepts TrueType, CFF/CFF2-flavored OpenType, WOFF1 and WOFF2.
Table directories and required metrics/cmap/name tables are checked. Wrapper
headers, table counts, original/padded table sums, transformed data and output
sizes are bounded before publication. Default font bytes are capped at 32 MiB;
table records at 256, outline points at 65,536. CFF cubic paths are subdivided
iteratively at 0.125 font-unit tolerance with a 20-level cap. Bad input returns
`FontError`; output is never silently truncated. Collection/variation/color-font
integration and hinting remain TD-85.

`FontDatabase::discover` traverses only host-granted roots, sorted, without
following symlinks. Defaults cap 16,384 entries, depth 16, 512 faces and 128 MiB
of input and, separately, decoded data. A failure publishes no faces; invalid
individual fonts are counted as rejected. `system_font_roots()` suggests native
paths without opening them. Native font services and directory races are TD-85.
Neither layout nor shaping performs I/O or downloads fonts.

## Segmentation and CSS wrapping

`segment::grapheme_boundaries` implements UAX #29 GB3–GB999, including Indic
GB9c and emoji GB11. `segment::line_breaks` implements untailored Unicode 17
UAX #14 LB1–LB31 and returns one result per scalar boundary. These APIs retain
their original conformance data; CSS tailoring is a separate layout policy.

Layout applies `word-break:normal/break-all/keep-all/break-word`,
`line-break:auto/loose/normal/strict/anywhere`, `overflow-wrap` (including the
`word-wrap` alias), and `hyphens:none/manual/auto`, after white-space rules.
Break-all changes letter/number classes to ID; every selected boundary still
respects an original grapheme or larger shaped cluster. Keep-all suppresses
word-internal CJK breaks. CJK punctuation follows the required distinctions in
CSS Text 3 §5.2, with declared Chinese/Japanese language where required.
`auto` uses normal strictness. `anywhere` overrides nonbreaking punctuation;
white-space nowrap/pre still suppress soft wrapping.

Emergency `overflow-wrap:break-word` breaks only a word that cannot otherwise
fit and does not reduce min-content. `anywhere` and legacy `word-break:break-word`
do reduce min-content. Soft hyphens stay invisible until the chosen break;
the inserted glyph width participates in fitting and intrinsic sizing. Automatic
points use the pinned, licensed Knuth–Liang English resource under
`crates/rustic-text/data/hyphenation/`. HTML/XML language follows DOM ancestry,
including `display:contents` and selected subtree roots; empty language resets
knowledge. Only declared `en`/`en-US`, ASCII words up to 256 characters, left
minimum two/right minimum three are supported. Unknown languages do not guess.
Explicit soft hyphens take priority in their word. Wider resources, SA dictionary
breaking and Level 4 phrase segmentation remain TD-87. `auto-phrase` parses
but fails layout explicitly until its resources exist.

## Bidi scopes

`bidi::BidiInfo` implements UAX #9 paragraph, explicit embedding/isolate,
weak/neutral/bracket, implicit and per-line L1/L2 rules. `with_scopes` injects
virtual controls for CSS `unicode-bidi:embed/bidi-override/isolate/isolate-override/
plaintext`, resolves them, then maps levels back to the original character
indexes. Scopes must be nested ranges; crossings, depth over 125, over 65,536
scopes or expansion over two million scalars fail explicitly. Scopes reopen
across paragraph separators. HTML `dir`, `bdi` and `bdo` have UA rules. Plaintext
alignment follows each paragraph's actual base direction. DOM values and UTF-16
source ranges remain unchanged.

## OpenType shaping and positions

HarfRust applies GSUB/GPOS/kern, Arabic joining and Indic reordering. Rustic
chooses one face per Unicode 17 grapheme, forms face/script/direction runs, and
maps backend clusters to original grapheme owners. Ligatures may own several
graphemes. Clusters remain logical; their glyphs are in visual order. Joining
controls participate in shaping before default ignorables are removed. Fallback
tries the best face of requested families, then configured and additional loaded
families. No duplicate weights of an already selected family are appended.

`ShapedGlyph` retains advance and horizontal/vertical offsets. Layout carries
absolute glyph origins to paint; mark and cursive positioning reaches the
rasterizer. Nonzero letter spacing disables optional ligatures. Word spacing
is applied to separators and letter spacing after visible clusters.
`shape_with_limits` defaults to 65,536 input scalars and 262,144 output glyphs.
Backend runs are capped at 4,096 scalars; longer text splits only at existing
soft breaks or fails. Cancellation is checked before/after runs. Non-finite
options/positions and invalid output glyph IDs fail. `shape_with_context`
preserves joining across source-node boundaries using eight neighboring scalars.
Cross-source optional ligatures, reshaping at selected breaks and carets remain
TD-86. Backend temporary/cache memory remains TD-85.

## CLI and evidence

`--font FILE` grants a local TTF/OTF/WOFF/WOFF2 file to rendering and computed
font metrics (repeat up to 64; 32 MiB per input, 128 MiB aggregate input and
separately decoded data). The first family maps generics; other loaded families
provide fallback. Configuration is published only after all fonts validate.
Without the option the original bundled test fonts keep fixtures deterministic.

```sh
cargo run --locked -p rustic-cli -- render --font /path/to/font.woff2 --viewport 640x400 --output out.png page.html
cargo build --locked -p rustic-text --example shape_font
python3 tools/compare_shaping.py
```

Tests cover format/outline equivalence, ligatures/kerning/marks, Arabic/Indic,
fallback, limits/cancellation, wrapper/CFF mutations, discovery atomicity, CSS
policies, language inheritance, source scopes and paint positions. Thirty pinned
independent native HarfBuzz 10.2.0 cases compare glyph IDs/advances/offsets offline;
Unicode-version-sensitive cluster ownership is checked against Rustic's Unicode
17 grapheme data separately. Native HarfBuzz is test tooling only.

All 19,338 committed line-break and 766 grapheme cases remain covered, together
with the unchanged 5,732 bidi character and 7,670 bidi class samples. Full bidi
files can be run with `RUSTIC_UCD_DIR=<dir> cargo test -p rustic-text --test bidi
-- --ignored`; this does not imply full CSS Text WPT conformance. `text-shaping`
fuzzes source partitions/graphemes/glyph bounds/positions, while `text-sfnt`
mutates the formats and accepted GSUB/GPOS tables. Resource manifests preserve
licenses, exact sources and output digests. `tools/check_repository.py` verifies
them alongside the reviewed dependency graph.

Specification basis: Unicode 17 UAX #9/#14/#29; CSS Writing Modes 3 §2;
[CSS Text 3 CR draft, 14 August 2026](https://www.w3.org/TR/2026/CRD-css-text-3-20260814/)
§5 and shaping-boundary rules. M3 catalog completion covers the declared static
pipeline; native host acceptance is M8 and broader text conformance is M9.
