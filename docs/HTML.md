# HTML input and tokenizer foundations

`rustic-html` is an original, safe Rust implementation depending only on
`rustic-base`. It tokenizes data, tags/attributes and declarations in HTML context. It
does not construct a DOM, select encodings, load documents or render pages.

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
recovers as a bogus comment containing `[CDATA[`. Foreign-content CDATA awaits
the future tree builder's adjusted-current-node context.

Doctype tokens preserve missing versus empty names/identifiers, ASCII-lowercase
names, case-preserved public/system identifiers, and the tokenizer's force-quirks
flag. PUBLIC/SYSTEM keywords can span feeds. Errors after a completed system
identifier preserve the flag; other malformed forms use their specified recovery.
The flag is input to future document-mode selection, not a computed quirks mode.
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

Diagnostics use zero-based **normalized Unicode scalar offsets**, with EOF at
the normalized stream length. They are neither raw input byte offsets nor DOM
UTF-16 offsets. Original-byte mapping, line/column information, decoder error
diagnostics and input control/noncharacter diagnostics remain future work.
Keyword lookahead failures point to the first candidate character; EOF is used
when an empty lookahead has no character. These offsets are stable across feeds.

## Limits and stream lifecycle

| Resource | Default | Accounting |
| --- | --- | --- |
| Input | 8 MiB | Cumulative original bytes, before preprocessing |
| Tokens | 100,000 | Cumulative emitted tokens, including EOF |
| Markup token data | 64 KiB | UTF-8 payload per tag/comment/doctype/instruction, including discarded tag duplicates |
| Character chunk | 16 KiB | Maximum retained character-token bytes; positive and no larger than markup-data limit |
| Attributes | 128 | Occurrences per tag, including duplicates |
| Diagnostics | 1,024 | Retained recoverable conditions per stream |

`TokenizerLimits` allows explicit caller budgets. Zero input/attribute/diagnostic
budgets are supported; zero tokens cannot successfully emit EOF. Markup accounting
includes comment data, combined doctype name/identifiers, and combined instruction
target/data. Converting an instruction target to a bogus comment charges the
additional leading `?`. Deferred comment delimiter characters are charged when
they become data. Declaration/doctype lookahead retains at most six ASCII bytes
apart from the current candidate; it is separate from a budgeted instruction
target. Punctuation/whitespace not stored as payload and ignored bogus-doctype
text are covered by the raw-input budget. These are logical resource limits rather than
total process memory guarantees. The decoder temporarily owns a copy of a feed
and its decoded output; replacement characters can expand input bytes. A returned
token vector is bounded by input and token budgets. Allocation failure is not a
recoverable contract. No content-controlled recursion is used.

Both input and tokenizer check shared cooperative cancellation, including on an
empty feed or EOF. After a resource, cancellation or unsupported-feature error,
the stream is poisoned and rejects further calls. Tokens returned by earlier
successful feeds remain final. A failing call returns no partial vector; the
consumer must terminate the document and discard partial parsing state. Internal
progress/diagnostics are not rolled back for retry. Successful EOF closes the
stream; another feed/finish returns `AlreadyFinished`.

## Deliberate remaining scope

Character-reference entry in data or attribute values currently returns
`UnsupportedCharacterReference`, including a bare ampersand. Ampersands in tag
or attribute names are ordinary name data. Ampersands in comments, doctype
identifiers and processing-instruction data remain literal. Unsupported
character-reference syntax fails rather than producing a silently altered stream.

RCDATA, RAWTEXT, script data and plaintext modes are not implemented. There is no
tree builder to select those modes; a tokenized `script` or `style` start tag
does not activate special handling. Encoding sniffing, legacy decoders, restart,
parser/script reentry, foreign content and complete HTML recovery remain planned.
The current API is an internal foundation, not a complete tokenizer/parser or a
stable embedding interface. DOC-05a's initial data/tag/comment scope is complete;
DOC-05b, DOC-06 and the broader corpus/fuzz gate remain open.

## Evidence and references

Twenty-nine focused Rust integration tests cover exact limits, poisoning, cancellation,
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

```sh
cargo test --locked -p rustic-html
cargo test --locked -p rustic-html --test corpus -- --nocapture
cargo run --locked -p rustic-html --example tokenize
```

The example tokenizes a doctype, comment, processing instruction and
tag/attribute/text stream in three-byte feeds. It does not build a document.

Implementation review: **2026-11-07**, against the WHATWG HTML Living Standard
last updated **2026-11-06**: [input preprocessing](https://html.spec.whatwg.org/multipage/parsing.html#preprocessing-the-input-stream),
[data state](https://html.spec.whatwg.org/multipage/parsing.html#data-state),
[tag open](https://html.spec.whatwg.org/multipage/parsing.html#tag-open-state),
[tag names](https://html.spec.whatwg.org/multipage/parsing.html#tag-name-state) and
[attribute states](https://html.spec.whatwg.org/multipage/parsing.html#before-attribute-name-state),
[declarations/comments](https://html.spec.whatwg.org/multipage/parsing.html#markup-declaration-open-state),
[doctypes](https://html.spec.whatwg.org/multipage/parsing.html#doctype-state) and
[processing instructions](https://html.spec.whatwg.org/multipage/parsing.html#processing-instruction-open-state).
UTF-8 behavior uses the Rust standard library's validated decoding primitives;
Rustic owns the incremental buffering and HTML preprocessing/state machine.
No browser engine or external parser implementation is used.
