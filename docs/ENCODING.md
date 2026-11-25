# Text encodings

`rustic-encoding` is Rustic's implementation of the
[WHATWG Encoding Standard](https://encoding.spec.whatwg.org/), chosen in
[ADR 0043](adr/0043-encoding-standard.md). It is a leaf crate: no
dependencies, no I/O, safe Rust only. It started the M6 entry work while M5
was still in progress, because nothing in it depends on scripting.

## What works

| Area | Status |
| --- | --- |
| Encodings | All 40: UTF-8, 28 single-byte encodings, GBK, gb18030, Big5, EUC-JP, ISO-2022-JP, Shift_JIS, EUC-KR, replacement, UTF-16BE/LE, x-user-defined |
| Labels | "Get an encoding" over all 228 labels: ASCII whitespace trimmed, ASCII case folded (`Encoding::for_label`, `for_label_bytes`) |
| Decoders | Every decoder, streaming in any chunk sizes, "replacement" and "fatal" modes (`Decoder`) |
| Window interfaces | `TextEncoder.encode()` and scalar-complete `encodeInto()`. `TextDecoder` uses the same decoders as the document loader, with API-specific label checks, BOM handling, and streaming state. Fixed-length typed arrays, `DataView`, and array buffers can supply bytes. [ADR 0075](adr/0075-web-text-encoding.md) |
| Encoders | Every encoder one scalar value at a time; unmappable values go back to the caller ("encode or fail"); ISO-2022-JP's end-of-queue escape (`Encoder::finish`) |
| Hooks | `decode` (BOM sniff, then the fallback encoding), `bom_sniff`, `encode` (HTML forms, `&#NNNN;`), `utf8_decode`, `utf8_decode_without_bom`, `utf8_decode_without_bom_or_fail`, "get an output encoding" |
| Stylesheets | CSS Syntax §3.2: BOM, then the transport `charset`, then a leading `@charset "…";`, then the referring document's encoding, then UTF-8 (`rustic_css::decode_stylesheet`). `dump-style`, `dump-layout` and `render` load local sheets through it |
| Local HTML | BOM or the first effective HTML charset declaration within the bounded 64 KiB prefix. The local host replays the prefix before executing scripts. The document's encoding supplies the environment fallback for local stylesheets and classic scripts. [ADR 0044](adr/0044-local-html-encoding-replay.md) |
| Network HTML | HTML encoding sniffing: BOM, then the response `charset` (both *certain*), then the `<meta>` prescan of the 64 KiB prefix, then the HTML §13.2.3.3 fallback for `--locale` (default `en`, windows-1252), both *tentative* (`sniff_declared_encoding`, `locale_fallback_encoding`). "Change the encoding": a later `<meta>` that names another encoding while the confidence is tentative loads the document again in it, once, with certain confidence (`url.restart`) |
| URL queries | `UrlParser::with_query_encoding` implements "percent-encode after encoding": the queries of `ftp`, `file`, `http` and `https` URLs use the document's output encoding, unmappable characters become `%26%23NNN%3B`, and `ws`/`wss`, non-special URLs and every other component stay UTF-8. `StyleLoader::with_document_encoding` applies it to `<base href>` and `<link href>`; `@import` stays UTF-8 |

## What does not work yet

| Missing | Where it lands |
| --- | --- |
| A local declaration beyond the bounded 64 KiB prefix is ignored | The local loader's current replay contract is [ADR 0044](adr/0044-local-html-encoding-replay.md). The network loader can reload once while encoding is tentative |
| Document-encoding query handling for `<script src>`, `<img src>` and module specifiers | Remaining URL/loader interoperability |
| `TextDecoderStream`, `TextEncoderStream` and worker exposure | M9 Streams and worker bindings |
| Form submission encodings (`application/x-www-form-urlencoded`, `multipart/form-data` in a legacy encoding) | NET-07 |
| Vectorized or table-driven fast paths for multi-byte encodings | M12, if a profile shows decoding on a critical path |

## Data

`tools/generate_encoding_data.py` reads `encodings.json`, `LICENSE` and the
`index-*.txt` files from [whatwg/encoding](https://github.com/whatwg/encoding)
at revision `be17ac5c49881336538acfc7b40dba3c8009144b` (the indexes dated
2024-09-18). It checks every input against the SHA-256 pins in
`crates/rustic-encoding/data/manifest.json` and writes:

- `src/tables.rs`: the encoding list and names, the sorted label table, the
  28 single-byte indexes (`[u16; 128]`), the 207 gb18030 ranges and the
  ISO-2022-JP katakana index, formatted with the pinned `rustfmt`;
- `data/index-{big5,euc-kr,gb18030,jis0208,jis0212}.bin`: dense little-endian
  arrays indexed by pointer, 0 for unmapped (no index maps a pointer to
  U+0000). Big5 uses 32-bit entries because it maps pointers outside the BMP;
- `data/WHATWG-LICENSE.txt`, the upstream notice. The indexes are CC BY 4.0,
  and BSD-3-Clause where incorporated into source code.

```sh
git clone https://github.com/whatwg/encoding /tmp/encoding-src
git -C /tmp/encoding-src checkout be17ac5c49881336538acfc7b40dba3c8009144b
python3 tools/generate_encoding_data.py --source-dir /tmp/encoding-src --check
```

Moving to a new revision: change `ENCODING_REVISION`, run with `--repin`,
review the diff of the tables and the manifest counts, then rerun the tests
and the reference comparison below.

## Design notes

- **Decoder driver.** Each decoder is the standard's handler: one byte or
  end-of-queue in, continue / one or two code points / error / finished
  out. "Restore to ioQueue" pushes bytes onto a queue that is read before the
  next input byte, so a sequence split across two `decode` calls behaves as
  if it arrived in one. With an empty queue and the decoder in its initial
  state, ASCII runs are copied directly, and UTF-8 copies the longest valid
  prefix that `core::str::from_utf8` finds.
- **Replacement.** The standard's replacement decoder returns *finished* on
  the second byte, ending the queue. Rustic ignores the rest of the stream
  instead, which has the same output and keeps a later chunk from producing
  a second U+FFFD.
- **Encoders.** Single-byte encoders scan their 128 entries. Multi-byte
  encoders binary-search a reverse index built on first use: (code point,
  first pointer) pairs, except that Shift_JIS skips pointers 8272–8835 and
  Big5 skips pointers below 5024 and takes the last pointer for U+2550,
  U+255E, U+2561, U+256A, U+5341 and U+5345. gb18030 checks the 18-entry
  Private Use side table first and refuses U+E5E5.
- **Limits.** Work is linear in the input. Output is at most three bytes of
  UTF-8 per input byte for decoders (two code points for Big5's four special
  pointers, which take two bytes) and at most `&#1114111;` per scalar value
  for the HTML encode hook. Callers bound the input; `dump-style` checks the
  decoded text against its byte budget.

## Evidence

- `cargo test -p rustic-encoding`: labels, each decoder's error and restore
  paths, the ISO-2022-JP concatenation example from the standard, chunk
  independence for every encoding, the hooks, encoder edge cases, and a
  round trip of every encodable scalar value in every encoding.
- `rustic_css` unit tests for the stylesheet fallback chain, and the
  `stylesheets_decode_with_their_charset_rule` CLI test: an ISO-8859-1 sheet
  with `@charset` matches a selector containing `é`, the same bytes without
  the rule do not.
- Fuzz target `encoding` (160 seeds; in the smoke job): chunked decoding
  equals one-call decoding, fatal mode fails exactly when replacement mode
  reports an error, and decode(encode(text)) returns the text whenever every
  scalar value is encodable.

- The local loader's `local_encoding_replays_before_the_first_script`
  regression checks byte-at-a-time reads, a declaration beyond byte 1024,
  an external classic, a stylesheet and a single execution of the script
  that appears before the declaration.

### Reference comparison

`node tools/encoding_reference.js FILE` records, in Playwright's Chromium,
TextDecoder output for every one-byte input (single-byte encodings) or every
two-byte input with three suffixes (multi-byte encodings) plus 30,000 random
inputs per encoding, and `<a href>` query encoding for every scalar value
and 5,000 random strings per encoding. `cargo run --release -p
rustic-encoding --example compare_reference -- FILE` compares Rustic with it.

Result on 2026-11-18 (Chromium 157.0.8103.78, revision above):
44,469,888 cases agree. Every encoder agrees on every scalar value. The 242
decoder differences are all places where Chromium departs from the
standard's text and Rustic follows the text:

| Encoding | Inputs | Standard (Rustic) | Chromium |
| --- | --- | --- | --- |
| Big5 | `88 62`, `88 64`, `88 A3`, `88 A5` | Two code points, e.g. U+00CA U+0304 | Two unrelated code units |
| EUC-JP | `8F` + lead + an invalid trail, then a JIS X 0208 pair | "Set EUC-JP jis0212 to false" happens on every path, so the next pair reads JIS X 0208 | The flag survives the error and the next pair reads JIS X 0212 |
| ISO-2022-JP | An escape that fails on a non-ASCII byte, e.g. `1B 28 81` | The restored byte is processed again and is an error in the ASCII state: U+FFFD, `(`, U+FFFD | The second U+FFFD is missing |

## Ownership and authority

The crate owns no resources beyond the reverse indexes, which are built once
per process behind `OnceLock` and are immutable afterwards. It never decides
which encoding a resource uses. The CSS, HTML, Fetch and Window callers
apply the rules for their own input.
