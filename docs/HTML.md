# HTML input and tokenizer foundations

`rustic-html` implements HTML tokenization and tree construction in safe
Rust. It uses `rustic-encoding` for byte decoding and `rustic-dom` for
the checked tree operations.

## Implemented behavior

`Utf8Input` accepts explicitly UTF-8 byte chunks. Incomplete multibyte sequences
wait for subsequent bytes; malformed sequences become U+FFFD. EOF replaces a
remaining incomplete sequence once. The first decoded U+FEFF is stripped only
at the beginning of the stream. CR and CRLF normalize to LF across chunks;
later U+FEFF and null code points are preserved at this stage. Empty feeds have
no effect on decoding. The byte budget counts original bytes, including the
removed BOM and skipped LF bytes.

`HtmlTokenizer` integrates this decoder with iterative states for data, tag open,
end-tag open, tag names, attribute names/values, quoted/unquoted values and
self-closing syntax. ASCII uppercase in tag/attribute names is lowercased;
non-ASCII case and attribute values are preserved. Attributes stay in source
order and the first duplicate name wins. A slash inside an unquoted value is
data, as in `<a href=/path/>`; whitespace before the slash changes its role.
The self-closing flag is syntax, not a decision about HTML element closure.

Null characters remain null in data with a diagnostic; tag and attribute data
replace them with U+FFFD. Implemented malformed syntax has explicit recovery and
bounded diagnostics. End-tag attributes and self-closing flags are diagnosed
and discarded. An unfinished tag at EOF is discarded; a final `<` or `</`
becomes character data with a diagnostic. Successful completion emits one EOF
token, including for empty input.

Comments include the ordinary, nested-opening, malformed-closing and bogus
recovery states. Unknown or incomplete declaration openings become bogus comments
without losing buffered characters. In HTML context, `<![CDATA[` is diagnosed and
recovers as a bogus comment containing `[CDATA[`. When the tree builder allows
CDATA (`set_cdata_allowed`, for a non-HTML adjusted current node), the section's
characters become text until `]]>`; EOF inside it is diagnosed as `EofInCdata`.

Doctype tokens preserve missing versus empty names/identifiers, ASCII-lowercase
names, case-preserved public/system identifiers, and the tokenizer's force-quirks
flag. PUBLIC/SYSTEM keywords can span feeds. Errors after a completed system
identifier preserve the flag; other malformed forms use their specified recovery.
The flag is input to the separate document builder's mode selection, not itself
a computed quirks mode; see [tree construction](HTML_TREE.md).
System identifiers remain token data; resource loading belongs to the loader.

Processing instructions preserve case in accepted ASCII targets and retain
uninterpreted data. Leading data whitespace and closing `>`/`?>` delimiters are
excluded; additional question marks remain data. Null is preserved in instruction
data, as prescribed by that state's rule. Invalid targets and disallowed XML/
XML-stylesheet targets recover as bogus comments with the leading `?` retained.
Unfinished instructions are discarded at EOF with a diagnostic. No DOM processing
instruction or instruction execution is implemented yet.

Character tokens are grouped by a fixed UTF-8 byte limit and markup transitions,
independently of feed boundaries. A Unicode scalar is never split between tokens.
An individual scalar larger than the configured text chunk limit fails. Consumers
must allow adjacent character tokens; grouping has no HTML semantic meaning.
The tokenizer may retain the last text chunk until more input or EOF arrives.

Named and numeric character references expand in data and quoted/unquoted
attribute values. The complete 2,231-entry WHATWG name table includes 106 legacy
semicolonless spellings and two-scalar values. Matching uses the longest valid
name, preserves case, and handles the historical attribute exception when a
semicolonless name is followed by ASCII alphanumeric text or `=`. Unknown names
and malformed numeric prefixes preserve their literal spelling with the prescribed
diagnostics. A bare ampersand is ordinary data.

Decimal/hex references use checked accumulation, Unicode-range/surrogate/null
recovery, noncharacter/control diagnostics and legacy C1 remapping. Decoded
characters go directly to their destination: a decoded quote cannot close an
attribute and a decoded `<` cannot begin a tag. Numeric CR is output data and is
not fed through raw-input newline preprocessing again. Ampersands in tag/attribute
names, comments, doctype identifiers and instruction data remain literal.

RCDATA expands references and recognizes only the appropriate end tag; RAWTEXT
keeps references/other markup literal. Script data implements the HTML escape,
escaped and double-escaped states, not JavaScript parsing or execution. A literal
`</script>` can close script data inside a JS-looking quoted string. Plaintext
keeps markup/references literal through EOF. All four replace null with U+FFFD
and the required diagnostic, while ordinary data retains its existing null rule.
Incomplete end-tag probes recover their original spelling; appropriate end tags
reuse the ordinary attribute/solidus recovery states. Escaped script EOF has its
own diagnostic. Probes stop retaining characters as soon as a match is impossible.

## Input drive and mode feedback

Use `write(bytes)` to submit bounded UTF-8 input and `next_token()` to read one
token at a time. After a start tag is delivered, `set_text_mode(TokenizerMode::...)`
can select the context before another input scalar is processed, including when
all remaining bytes are already queued. The tree builder owns that decision:
namespace, insertion mode and scripting flags cannot be inferred from a tag name
alone. The tokenizer does not automatically select modes for `title`/`style`/
`script`. A rejected mode change returns `InvalidModeChange` without mutation or
poisoning; the opportunity expires after more input/a token is processed.

`end()` closes byte submission without parsing ahead. Continue reading through
one EOF token; `next_token()` then returns `None` repeatedly. Before EOF, `None`
means more input is needed (including buffered partial text/probes). Another
write/end/finish after input closure returns `AlreadyFinished`, while queued input
can still be consumed. Cancellation/fatal parse errors poison future reads and
writes. `feed`/`finish` collect tokens for fixed-context consumers; use the pull
driver for feedback within a chunk. `with_context` supplies an explicit initial
mode and bounded last-start-tag name; `HtmlFragmentParser` uses it to start in
the context's state. It consumes UTF-8 bytes and is not a DOM-string parser.

The decoder's owned normalized output moves into a queued-input buffer when
possible. Consumed prefixes compact only after sufficient progress, avoiding a
full suffix copy for every small write. Input is not parsed past a returned token
boundary. Multiple emissions from one scalar/EOF recovery wait in a token queue.
The same budgets cover both drivers; reference/tag/text modules retain their
syntax responsibilities. See [ADR 0008](adr/0008-tokenizer-drive.md).

Diagnostics use zero-based **normalized Unicode scalar offsets**, with EOF at
the normalized stream length. They are neither raw input byte offsets nor DOM
UTF-16 offsets. Original-byte mapping, line/column information, decoder error
diagnostics and input control/noncharacter diagnostics remain future work.
Keyword lookahead failures point to the first candidate character; EOF is used
when an empty lookahead has no character. These offsets are stable across feeds.
Missing-semicolon diagnostics point immediately after the matched name/digits;
longest-match lookahead does not move that location to a later mismatch.

## Limits and stream lifecycle

| Resource | Default | Accounting |
| --- | --- | --- |
| Input | 8 MiB | Cumulative original bytes, before preprocessing |
| Tokens | 100,000 | Cumulative emitted tokens, including EOF |
| Markup token data | 64 KiB | UTF-8 payload per tag/comment/doctype/instruction, including discarded tag duplicates |
| Character chunk | 16 KiB | Maximum retained character-token bytes; positive and no larger than markup-data limit |
| Attributes | 128 | Occurrences per tag, including duplicates |
| Diagnostics | 1,024 | Retained recoverable conditions per stream; later ones are counted by `dropped_diagnostics()`, never fatal |

`TokenizerLimits` allows explicit caller budgets. Zero input/attribute/diagnostic
budgets are supported; zero tokens cannot successfully emit EOF. Markup accounting
includes comment data, combined doctype name/identifiers, and combined instruction
target/data. Converting an instruction target to a bogus comment charges the
additional leading `?`. Deferred comment delimiter characters are charged when
they become data. Declaration/doctype lookahead retains at most six ASCII bytes
apart from the current candidate; it is separate from a budgeted instruction
target. Punctuation/whitespace not stored as payload and ignored bogus-doctype
text are covered by the raw-input budget. Character-reference scratch retains at
most a 32-byte possible name plus its current candidate. Numeric digits are not
retained; overflow uses a fixed-size accumulator/flag. Expanded attribute/text
payloads consume the existing token/chunk budgets, and all reference errors use
the diagnostic budget. Last-start-tag context and a matching end-tag probe are
bounded by the markup-token byte limit; failed probes stream as literal text.
Script keyword probes retain at most six ASCII bytes. Queued normalized input is
bounded by cumulative raw input and decoding expansion; the token queue by the
cumulative token limit. These are logical resource limits rather than
total process memory guarantees. The decoder temporarily owns a copy of a feed
and its decoded output; replacement characters can expand input bytes. A returned
token vector is bounded by input and token budgets. Allocation failure is not a
recoverable contract. No content-controlled recursion is used.

Both input and tokenizer check shared cooperative cancellation, including on an
empty feed or EOF. After a resource or cancellation error,
the stream is poisoned and rejects further calls. Tokens returned by earlier
successful feeds remain final. A failing call returns no partial vector; the
consumer must terminate the document and discard partial parsing state. Internal
progress/diagnostics are not rolled back for retry. Successful EOF closes the
stream; another feed/finish returns `AlreadyFinished`.

## Deliberate remaining scope

The [inert document parser](HTML_TREE.md) now implements complete document tree
construction with scripting disabled, including foreign content and CDATA.
The fragment parsing algorithm is implemented over UTF-8 bytes.
The local document loader can select a legacy encoding and replay its
retained prefix before author code runs. The network loader applies
transport charset confidence and may reload once on a later declaration
while confidence remains tentative. See the [encoding guide](ENCODING.md)
and [local replay decision](adr/0044-local-html-encoding-replay.md).
The current API is an internal foundation, not a complete tokenizer/parser or a
stable embedding interface. DOC-05a's initial data/tag/comment scope is complete;
DOC-05b now has reference expansion and all four text modes; broader encoding work
remains open. Fragment parsing and the parser fuzz gate are complete (DOC-06c, DOC-08).

## Evidence and references

Fifty-three focused Rust integration tests cover exact limits, poisoning, cancellation,
attributes, error recovery and EOF. Input tests compare **all 65,536 two-byte
sequences** with an independent whole-buffer lossy decode/preprocessing oracle,
using both unbroken and split input. Further fixtures split BOMs, valid and
malformed multibyte sequences, CRLF, tags and attributes at every byte boundary
and feed one byte at a time; tokens and diagnostics must match the unbroken path.
Fixed expected token/diagnostic assertions test behavior separately from chunk
invariance. A separate corpus test checks **1,079 selected upstream cases** from
1,846 cases in five pinned html5lib tokenizer files. All selected token fields
and ordered error codes pass, including 779 expected doctypes, 268 comments and
three processing instructions. Empty output at instruction EOF is checked too.
The runner verifies one final EOF, coalesces adjacent character tokens for the
upstream format, runs 16,539 two-chunk splits and 1,079 one-byte-feed cases, and
compares all local diagnostic offsets across chunkings. A rustdoc example is
compiled too.

The fixed selection uses default data state, declaration/end-tag entry points,
and declaration/character output types. It excludes alternate contexts,
double-escaped data, surrogate scalars, ampersands conservatively, and unsupported
error codes including input control/noncharacter diagnostics. The 767 excluded
cases remain recorded; no implementation results drive selection. Upstream
line/column positions are not compared. This is a declaration corpus result,
not full tokenizer, HTML/WPT or browser conformance.

[The manifest](../crates/rustic-html/tests/fixtures/html5lib_manifest.json) records
the upstream revision `c5495c539ca8a79c3c5107f5f14010bcff9a35d5`, per-file indices,
source Git/SHA-256 digests, generated digest, counts and policy. The upstream
[MIT notice](../crates/rustic-html/tests/fixtures/HTML5LIB-LICENSE.md) is retained.
Normal tests are offline. To reproduce the fixture from that pinned upstream
checkout:

```sh
python3 tools/import_html_tests.py --source-dir /path/to/html5lib-tests --revision c5495c539ca8a79c3c5107f5f14010bcff9a35d5 --check
```

Omit `--check` only for a deliberate fixture update, with reviewed selection and
expected-result changes. Three Python tests cover importer selection, escaping,
field conversion and provenance; repository checks verify fixture integrity.

A second corpus test passes **all 4,626 cases** from the pinned `entities.test`,
`namedEntities.test` and `numericEntities.test` files: zero cases excluded by its
separate default-data-state/reference policy. It compares character data,
attribute values and ordered diagnostic kinds, checks 43,069 two-chunk splits and
4,626 one-byte runs, and verifies one EOF. Attribute dictionaries are compared by
name/value; separate tests enforce source order. The existing declaration subset
and its exclusions are unchanged. Upstream error line/column positions and
unimplemented tokenizer contexts are not conformance claims.

[Reference corpus provenance](../crates/rustic-html/tests/fixtures/references_manifest.json)
records the same pinned upstream revision, source/generated digests, indices and
counts. Reproduce it with:

```sh
python3 tools/import_html_tests.py --source-dir /path/to/html5lib-tests --revision c5495c539ca8a79c3c5107f5f14010bcff9a35d5 --suite references --check
```

The runtime name table is generated from the vendored WHATWG
[entities.json](../crates/rustic-html/data/entities.json), snapshot 2026-11-07.
[Its manifest](../crates/rustic-html/data/entities_manifest.json) pins the canonical
source URL, source/table SHA-256 digests, counts and maximum name size; the
[WHATWG notice](../crates/rustic-html/data/WHATWG-LICENSE.md) preserves source-code
licensing/attribution. This imports standard character data, not another engine's
parser. `python3 tools/import_html_entities.py --check` reproduces it offline.
Updating data requires an explicit snapshot date and reviewed source/table/notice
changes. Repository checks regenerate/compare it; Python tests reject duplicate
keys, invalid scalars/names, mismatched values and missing legacy equivalents.

A third corpus covers text states: **109 selected source records**, expanded into
**282 mode/context runs**, from 1,865 records across seven pinned files. It checks
last-start-tag context, double-escaped Unicode input/output, 1,902 two-chunk splits
and 282 one-byte runs. Non-text initial states, unsupported input diagnostic codes
and non-scalar inputs remain outside the declared selection. The manifest records
omitted states from selected multi-state cases; no failure-driven exclusions were
added. [Text corpus provenance](../crates/rustic-html/tests/fixtures/text_manifest.json)
records all indices, states, counts and digests. Reproduce with `--suite text` in
the same import command. Older declaration/reference selections remain unchanged.

The upstream tokenizer fixtures provide decoded Unicode streams. The harness
encodes them with an explicit transport BOM, which the byte decoder removes;
this preserves an intentional leading U+FEFF in the fixture as content. Dedicated
raw-byte input tests separately verify BOM stripping and every byte split. The
runner compares token fields/ordered diagnostic kinds and local offset invariance;
upstream line/column positions and full parser conformance remain unclaimed.

```sh
cargo test --locked -p rustic-html
cargo test --locked -p rustic-html --test corpus -- --nocapture
cargo run --locked -p rustic-html --example tokenize
cargo run --locked -p rustic-html --example text_modes
```

The example tokenizes a doctype, comment, processing instruction and
tag/attribute/text/reference stream in three-byte feeds. It does not build a document.
The `text_modes` example drives explicit feedback for a known HTML fixture; that
small selection helper is not a substitute for tree construction.

Implementation review: **2026-11-07**, against the WHATWG HTML Living Standard
last updated **2026-11-06**: [input preprocessing](https://html.spec.whatwg.org/multipage/parsing.html#preprocessing-the-input-stream),
[data state](https://html.spec.whatwg.org/multipage/parsing.html#data-state),
[tag open](https://html.spec.whatwg.org/multipage/parsing.html#tag-open-state),
[tag names](https://html.spec.whatwg.org/multipage/parsing.html#tag-name-state) and
[attribute states](https://html.spec.whatwg.org/multipage/parsing.html#before-attribute-name-state),
[declarations/comments](https://html.spec.whatwg.org/multipage/parsing.html#markup-declaration-open-state),
[doctypes](https://html.spec.whatwg.org/multipage/parsing.html#doctype-state) and
[processing instructions](https://html.spec.whatwg.org/multipage/parsing.html#processing-instruction-open-state).
Reference handling follows [character-reference states](https://html.spec.whatwg.org/multipage/parsing.html#character-reference-state)
and the [named-reference table](https://html.spec.whatwg.org/multipage/named-characters.html).
Text states follow [RCDATA](https://html.spec.whatwg.org/multipage/parsing.html#rcdata-state),
[RAWTEXT](https://html.spec.whatwg.org/multipage/parsing.html#rawtext-state),
[script data](https://html.spec.whatwg.org/multipage/parsing.html#script-data-state) and
[plaintext](https://html.spec.whatwg.org/multipage/parsing.html#plaintext-state).
The shared encoding crate uses Rust's validated UTF-8 prefix checks and
original streaming decoders. HTML applies its own input preprocessing.
[ENCODING.md](ENCODING.md)
