# ADR 0043 — Original Encoding Standard implementation

Date: 2026-11-18
Status: Accepted for `url.encoding` (M6 entry)

## Context

M6 loads pages and stylesheets from servers, and real responses arrive in
legacy encodings: windows-1252, Shift_JIS, GBK, Big5, EUC-KR and the rest of
the WHATWG Encoding Standard's list. The URL parser also needs an encoder for
query strings in a document's encoding, and HTML forms need the legacy
"encode" hook. Until now Rustic decoded everything as UTF-8 (TD-28), and the
[ADR index](README.md) listed "own decoders or a reviewed crate" as an open
decision for `url.encoding`.

The candidates were:

1. **A reviewed crate** such as `encoding_rs`. It is mature and fast and is
   the implementation Gecko ships. It would be Rustic's second third-party
   dependency outside fonts, with its own SIMD paths and build features to
   review on every tier, including big-endian s390x and 32-bit targets.
2. **Original decoders over generated tables.** The standard defines each
   decoder and encoder as a short state machine over published indexes, in
   the same style as the URL and HTML algorithms Rustic already implements.
   The data is about 220 KB of indexes from one repository.

## Decision

Rustic implements the Encoding Standard itself, in a new leaf crate
`rustic-encoding` with no dependencies.

- **Data.** `tools/generate_encoding_data.py` reads `encodings.json` and the
  `index-*.txt` files of whatwg/encoding at a pinned commit, checks each
  input's SHA-256 against `crates/rustic-encoding/data/manifest.json`, and
  writes the label table, the single-byte indexes and two small indexes as
  Rust source and the five large CJK indexes as little-endian binary arrays
  read with `include_bytes!`. `--check` regenerates in memory and fails on any
  difference; `--repin` is the only way to move the revision.
- **Decoders** are the standard's handler algorithms one byte at a time, with
  "restore to ioQueue" as a small queue that the driver reads before the next
  input byte. Streaming in any chunk sizes gives the same output as one call.
  ASCII runs (and valid UTF-8 prefixes) are copied without the per-byte
  handler, which keeps the common case linear and fast. Both error modes are
  supported; fatal mode stops at the first error and resets the decoder.
- **Encoders** run per scalar value and return an unmappable value to the
  caller, which is exactly the "encode or fail" hook URL percent-encoding
  needs; the HTML "encode" hook writes `&#NNNN;` on top. Reverse indexes are
  built once per index on first use (a sorted vector behind `OnceLock`),
  using the standard's pointer rules for Shift_JIS and Big5.
- **Integration.** CSS stylesheets decode with CSS Syntax §3.2's fallback
  chain (`rustic_css::decode_stylesheet`). HTML encoding sniffing, `<meta>`
  prescan and the parser restart (`url.restart`), URL query encoding and the
  TextDecoder/TextEncoder bindings use this crate when they land.

## Consequences

- No new external dependency, and every target tier builds the same safe
  code. The binary grows by about 220 KB of index data plus the label table.
- Rustic owns correctness. The comparison below and the fuzz target are the
  evidence, and both must be rerun when the pinned revision moves.
- Throughput for legacy multi-byte encodings is a byte-at-a-time state
  machine. That is acceptable for M6 pages; vectorized decoding is an M12
  performance question, measured against the other engines.
- The encoders' reverse indexes allocate on first use (at most a few hundred
  KB in total) and live for the process. Engines that never encode a legacy
  encoding never build them.

## Validation

- `crates/rustic-encoding/tests/encoding.rs` covers labels, every decoder's
  error and restore paths, the standard's own examples (the ISO-2022-JP
  concatenation note, Big5's two-code-point pointers, gb18030 ranges), chunk
  independence, the hooks, and a round trip of every encodable scalar value
  in every encoding.
- `tools/encoding_reference.js` records Chromium's TextDecoder output for
  every one- or two-byte input plus random inputs, and its URL query
  encoding of every scalar value, in every encoding; the
  `compare_reference` example compares Rustic with it. At the pinned
  revision all 36 encoders agree on all 1,112,064 scalar values and on
  random strings. Decoders agree on 44.5 million inputs except three
  places where Chromium departs from the standard's text and Rustic follows
  the text: Big5 pointers 1133/1135/1164/1166, the EUC-JP JIS X 0212 flag
  after a failed sequence, and errors for bytes the ISO-2022-JP escape
  states restore. [ENCODING.md](../ENCODING.md#reference-comparison) has the
  details.
- The `encoding` fuzz target checks chunked against whole decoding, fatal
  against replacement mode, and decode(encode(text)) round trips.

## Revisit when

- A profile shows legacy decoding on a page's critical path (M12), or
- the standard adds an encoding or changes an algorithm, or
- a platform needs a transcoding service the engine cannot provide itself.
