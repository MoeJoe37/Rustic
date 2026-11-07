# HTML input and tokenizer foundations

`rustic-html` is an original, safe Rust implementation depending only on
`rustic-base`. It currently tokenizes a declared data/tag/attribute subset. It
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

Character tokens are grouped by a fixed UTF-8 byte limit and markup transitions,
independently of feed boundaries. A Unicode scalar is never split between tokens.
An individual scalar larger than the configured text chunk limit fails. Consumers
must allow adjacent character tokens; grouping has no HTML semantic meaning.
The tokenizer may retain the last text chunk until more input or EOF arrives.

Diagnostics use zero-based **normalized Unicode scalar offsets**, with EOF at
the normalized stream length. They are neither raw input byte offsets nor DOM
UTF-16 offsets. Original-byte mapping, line/column information, decoder error
diagnostics and input control/noncharacter diagnostics remain future work.

## Limits and stream lifecycle

| Resource | Default | Accounting |
| --- | --- | --- |
| Input | 8 MiB | Cumulative original bytes, before preprocessing |
| Tokens | 100,000 | Cumulative emitted tokens, including EOF |
| Tag data | 64 KiB | UTF-8 bytes appended to names/values per tag, including discarded duplicates |
| Character chunk | 16 KiB | Maximum retained character-token bytes; positive and no larger than tag-data limit |
| Attributes | 128 | Occurrences per tag, including duplicates |
| Diagnostics | 1,024 | Retained recoverable conditions per stream |

`TokenizerLimits` allows explicit caller budgets. Zero input/attribute/diagnostic
budgets are supported; zero tokens cannot successfully emit EOF. Tag accounting
excludes punctuation/whitespace that is not stored as names/values; the raw-input
budget still bounds such input. These are logical resource limits rather than
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
or attribute names are ordinary name data. Comments, doctypes, bogus comments,
processing instructions and other declaration paths return
`UnsupportedDeclaration`. Unsupported syntax fails rather than producing a
silently altered token stream.

RCDATA, RAWTEXT, script data and plaintext modes are not implemented. There is no
tree builder to select those modes; a tokenized `script` or `style` start tag
does not activate special handling. Encoding sniffing, legacy decoders, restart,
parser/script reentry, foreign content and complete HTML recovery remain planned.
The current API is an internal foundation, not a complete tokenizer/parser or a
stable embedding interface. DOC-05a remains partial until comment states land;
DOC-05b, DOC-06 and the corpus/fuzz gate remain open.

## Evidence and references

Seventeen Rust integration tests cover exact limits, poisoning, cancellation,
attributes, error recovery and EOF. Input tests compare **all 65,536 two-byte
sequences** with an independent whole-buffer lossy decode/preprocessing oracle,
using both unbroken and split input. Further fixtures split BOMs, valid and
malformed multibyte sequences, CRLF, tags and attributes at every byte boundary
and feed one byte at a time; tokens and diagnostics must match the unbroken path.
Fixed expected token/diagnostic assertions test behavior separately from chunk
invariance. A rustdoc example is compiled too.

```sh
cargo test --locked -p rustic-html
cargo run --locked -p rustic-html --example tokenize
```

The example tokenizes a small tag/attribute/text stream in three-byte feeds. It
does not build a document. No upstream HTML corpus or HTML/WPT conformance result
is claimed yet.

Implementation review: **2026-11-07**, against the WHATWG HTML Living Standard
last updated **2026-11-06**: [input preprocessing](https://html.spec.whatwg.org/multipage/parsing.html#preprocessing-the-input-stream),
[data state](https://html.spec.whatwg.org/multipage/parsing.html#data-state),
[tag open](https://html.spec.whatwg.org/multipage/parsing.html#tag-open-state),
[tag names](https://html.spec.whatwg.org/multipage/parsing.html#tag-name-state) and
[attribute states](https://html.spec.whatwg.org/multipage/parsing.html#before-attribute-name-state).
UTF-8 behavior uses the Rust standard library's validated decoding primitives;
Rustic owns the incremental buffering and HTML preprocessing/state machine.
No browser engine or external parser implementation is used.
