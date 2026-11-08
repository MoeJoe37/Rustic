# URL records, parsing and origins

## Implemented scope

`rustic-url` owns an original Rust URL record/parser and process-local origin
model. It depends only on `rustic-base`; no existing URL parser, browser engine,
JavaScript runtime or third-party Rust package supplies its implementation.
Parsing performs no DNS lookup, network access, file access or navigation.

Implemented in the initial DOC-02a slice:

- Immutable records with scheme, userinfo, optional host/port, structured path,
  and distinct absent/empty query and fragment values.
- `http`, `https`, `ws`, `wss` and `ftp` network URLs and default-port normalization.
- Percent-decoded special hosts, the non-strict domain parser (ASCII domains are
  lowercased; others go through UTS #46 ToASCII) and forbidden-domain checks.
- Original UTS #46 IDNA processing (`rustic_url::idna`): mapping, NFC, Punycode,
  ContextJ and the RFC 5893 Bidi Rule over generated Unicode 17.0.0 tables.
- Original short-form decimal/octal/hex IPv4 parsing and original IPv6 parsing/
  canonical compression, including strict dotted IPv4 tails inside IPv6.
- Generic non-special hierarchical URLs, opaque hosts and opaque paths.
- `file:` URLs (DOC-02b): empty and `localhost` hosts, UNC-style hosts, Windows
  drive letters (`C|` normalized to `C:`, drive letters in the host position,
  `..` never removing a lone drive letter) and resolution against file bases.
- Validation-error diagnostics (DOC-02b) through `UrlParser::parse_with_diagnostics`,
  using the standard's error names and byte offsets into the original input.
- Relative references, same-scheme special references, authority replacement,
  encoded dot-segment recovery and special-scheme backslash behavior.
- C0/space trimming, ASCII tab/newline preprocessing and component-specific UTF-8 percent encoding.
- Explicit serialization, credential-free serialization and a redacted `Debug` implementation.
- Input/output/segment limits, immutable base records and cooperative cancellation checkpoints.
- Structured tuple origins and distinct opaque origins whose serialized `null` strings never determine identity.

The implementation follows the [WHATWG URL Standard](https://url.spec.whatwg.org/),
reviewed at [revision 845bafca22fb0c5a42c4bf384a9bdea525a2f10d](https://github.com/whatwg/url/tree/845bafca22fb0c5a42c4bf384a9bdea525a2f10d),
including host parsing/serialization, basic URL parsing, URL serialization and
origin derivation. This is a partial implementation, not full URL conformance.

## Boundaries and pending work

`blob:` returns an explicit unsupported error. A special URL whose host fails
the domain parser is rejected with `InvalidHost`; there is no guessed host or
fallback authority. ASCII domains, including malformed A-labels such as `xn--a`,
follow the standard's non-strict rule and are only lowercased. Unicode
paths/queries/fragments use UTF-8 percent encoding; non-special opaque hosts do
not use IDNA.

Pending: strict (`beStrict`) domain validation for conformance checkers and the
`domain-to-ASCII` validation error, blob registry and origin rules,
legacy document query encodings,
URL setters and URLSearchParams. There is no public browser `URL` binding yet.
Successful parsing does not mean a scheme is navigable, secure, allowed by policy,
or supported by the future network service. FTP syntax support is not an FTP client.

## File URLs

File URLs follow the standard's file, file slash and file host states. The
rules are the same on every platform: a Windows drive letter is recognized on
Linux, macOS, Android and iOS too, because the URL Standard defines the syntax
independently of the host OS. Parsing never touches the file system. A file URL
always has a host, which is empty for local files; `localhost` becomes empty,
any other host (including IPv4/IPv6) is kept, as in `file://server/share`.
Relative references use only a `file:` base; a drive letter at the start of a
reference replaces the whole base path, and `/path` keeps the base's drive.

`Url::origin()` returns a fresh opaque origin for file URLs. The standard leaves
this implementation-defined, and opaque is the restrictive choice: two local
files are never same-origin by URL alone. Converting a file URL to a native path
(UNC shares, drive letters, percent-decoding to the OS encoding, Android content
providers or iOS sandboxes) is not URL parsing. It belongs to a later loader or
platform layer, which must apply its own access policy; the local `dump-dom`
loader still accepts paths, not file URLs.

## Validation diagnostics

`UrlParser::parse_with_diagnostics` returns a `ParseReport` holding exactly the
result `parse` would return plus `Diagnostic` values (a `ValidationError` kind and
a byte offset). Offsets index the original input before trimming and tab/newline
removal and are always on `char` boundaries. Errors detected inside a host
(IPv4, IPv6, forbidden code points) point at the start of that host. At most
`MAX_DIAGNOSTICS` (64) are kept; `truncated()` reports any overflow. Diagnostics
contain no input text, so they are safe to log; `ValidationError::name()` gives
the standard's name, such as `invalid-URL-unit`.

Reported kinds cover the URL parser (`invalid-URL-unit`, slash and reverse-solidus
errors, `invalid-credentials`, `host-missing`, port errors, missing scheme and both
file drive-letter errors), opaque and domain host code points, every IPv4 and IPv6
error, and `domain-to-ASCII` when percent-decoded host bytes are not UTF-8. The
remaining IDNA errors (`domain-to-ASCII` from IDNA processing, `domain-to-Unicode`)
arrive with the IDNA adapter; the enum is non-exhaustive for that reason. Every
expected failure in the pinned WPT subset produces at least one diagnostic.
Upstream data records no expected validation errors, so kinds and offsets are
pinned by Rustic's own tests in `tests/diagnostics.rs`, not by WPT.

## Usage and origin identity

```rust
use rustic_url::{Origin, Url};

let base = Url::parse("https://example.test/docs/index.html", None)?;
let asset = Url::parse("../images/logo.svg", Some(&base))?;
assert_eq!(asset.serialize(), "https://example.test/images/logo.svg");
assert_eq!(asset.origin(), base.origin());

let assigned = Origin::opaque();
let retained = assigned.clone();
assert_eq!(assigned, retained);
assert_ne!(assigned, Origin::opaque());
```

Tuple origins compare canonical scheme/host/normalized port. Credentials, path,
query and fragment do not enter that comparison. An origin is not a schemeful
site or registrable domain; public-suffix/site policy remains future work.

Every `Url::origin()` derivation for a non-tuple URL returns a fresh opaque origin,
as specified for those URL-origin cases. A navigation/document layer must assign
an origin once and retain/clone that value. `about:blank` document inheritance,
sandboxing and other navigation rules must not be inferred from a URL string.
Opaque origin identity is a private shared allocation with equality/hashing based
on identity, not the allocation's unit value. It is process-local and is never
serialized as a pointer or a transportable capability. IPC needs its own broker-
assigned mapping later. `Origin::serialize()` returning `null` does not authorize
access to another opaque origin.

`Url::serialize()` intentionally includes userinfo. Use explicit credential-free
serialization when appropriate, while remembering that queries/paths may still be
sensitive. `Debug` and parse-error messages omit credentials and private payloads.

## Resource and module contracts

`UrlParser` defaults to 64 KiB original input, 256 KiB serialized output and 4,096
retained path segments. The original input is checked before whitespace cleanup.
Base records are bounded before copying. Segment checks apply while constructing
the path, including empty segments; encoded dot segments can remove earlier ones.
These limits account for logical data, not total allocator capacity/process memory.
Temporary work is linear in the bounded input/base, including encoding expansion.
Cancellation is cooperative before/after parsing and between structural/path
components; it is not a watchdog or an interrupt for allocation.

| Module | Responsibility |
| --- | --- |
| `record` | Immutable representation, accessors, URL/path serialization and safe debug output |
| `parser` | Scheme/authority/path/file/reference orchestration, diagnostics, limits and checkpoints |
| `host` | Domain/opaque/address host representation, domain parser and host validation |
| `idna` | UTS #46 ToASCII/ToUnicode, NFC, Punycode, ContextJ and Bidi Rule; generated `tables` |
| `ipv4` / `ipv6` | Address algorithms and IPv6 canonical serialization |
| `encoding` | Component percent-encode sets and host percent decoding |
| `origin` | Tuple keys, opaque identity, equality/hashing and serialization |
| `error` | Input-independent errors and explicit unsupported features |
| `diagnostics` | Validation-error kinds, offsets and the bounded parse report |

Run the offline example:

```sh
cargo run --locked -p rustic-url --example resolve
```

## Pinned upstream evidence

The offline integration test runs **887 selected cases** from the 899 test records
in WPT's `url/resources/urltestdata.json` at
[revision 8c17bfeb3e66a2986be16b0e004f85ace68db647](https://github.com/web-platform-tests/wpt/blob/8c17bfeb3e66a2986be16b0e004f85ace68db647/url/resources/urltestdata.json).
The fixed selection rule is defined before running Rustic: every record except
blob-scheme cases and any record with a lone surrogate (which a Rust string
cannot hold; none exist at this revision). File cases joined with DOC-02b
(policy v2) and the 72 non-ASCII and percent-encoded non-ASCII cases joined once
IDNA landed (policy v3); all pass. Fixture literals escape every non-ASCII code
point as `\u{...}` so no bidi or invisible text enters the source.

| Count | Meaning |
| --- | --- |
| 899 | All upstream test records; section-description strings are not cases |
| 887 | Selected and executed with no unexpected failures |
| 617 | Expected successful parses checked field by field and reparsed for record stability |
| 270 | Expected parse rejections, each also checked to produce a diagnostic |
| 12 | Excluded: blob scheme (registry not implemented) |

Every successful selected case checks href, protocol, username/password, host,
hostname, port, pathname, search and hash, plus origin where supplied upstream.
This does not execute URLSearchParams expectations, setters, legacy encodings,
other WPT files or a full browser WPT harness. The 887-case result is a declared
subset, not a whole-suite pass percentage. Crashes or unexpected errors fail it;
cases are not dynamically skipped based on Rustic's results.

The [manifest](../crates/rustic-url/tests/fixtures/wpt_manifest.json) records the
revision, source Git blob/SHA-256, selected array indices, exclusion counts,
checked fields and generated fixture digest. Fixtures retain the upstream
[BSD license](../crates/rustic-url/tests/fixtures/WPT-LICENSE.md). Recreate/check
them from an exact downloaded source file with:

```sh
python3 tools/import_url_tests.py --source /path/to/urltestdata.json --revision 8c17bfeb3e66a2986be16b0e004f85ace68db647 --check
cargo test --locked -p rustic-url --test wpt -- --nocapture
```

Omit `--check` only for a deliberate fixture update. Review revision, selection,
digest, denominator and behavior changes together; do not shrink the selection
or rewrite expected results merely to hide a regression. Normal CI is offline
with respect to the fixture source and needs no upstream checkout.

## Unicode IDNA data

`rustic_url::idna` is an original UTS #46 implementation with the URL Standard's
non-strict options: CheckHyphens, UseSTD3ASCIIRules and VerifyDnsLength false,
CheckBidi and CheckJoiners true, nontransitional processing and no
IgnoreInvalidPunycode. Only non-ASCII domains reach it; the host parser applies
the empty-result and forbidden-domain-code-point checks afterwards, then the
IPv4 "ends in a number" rule, so full-width digits can still form an IPv4 host.

`src/idna/tables.rs` is generated, never hand-edited. It holds the IDNA mapping
status runs, canonical decompositions and primary composites with
Full_Composition_Exclusion applied, combining classes, General_Category=Mark,
Bidi_Class (with `@missing` defaults) and the Joining_Type values ContextJ uses.
Lookups are binary searches over sorted runs; Hangul uses the algorithmic
mapping. The inputs are Unicode 17.0.0 files from
[unicode-org/unicodetools 63aff0ca8dea894a057d04e7aef7ae1241757680](https://github.com/unicode-org/unicodetools/tree/63aff0ca8dea894a057d04e7aef7ae1241757680/unicodetools/data)
(a mirror of `unicode.org/Public`) plus WPT's `url/resources/toascii.json` at
[57bbaa12ff3515848cc0b3d3791791086a563047](https://github.com/web-platform-tests/wpt/blob/57bbaa12ff3515848cc0b3d3791791086a563047/url/resources/toascii.json).
The [manifest](../crates/rustic-url/tests/fixtures/idna_manifest.json) lists every
source URL with its SHA-256 and the digest of every generated file, and
`tools/check_repository.py` rejects a generated file that no longer matches it.
The data is under the [Unicode License v3](../crates/rustic-url/UNICODE-LICENSE.md).

Download the eight files named in the manifest into one directory, then:

```sh
python3 tools/generate_idna_data.py --source-dir /path/to/files --check
cargo test --locked -p rustic-url --test idna
```

Updating Unicode means changing the pinned revision/version in the generator,
running it with `--repin`, and reviewing both the table diff and the test results.
The generator rejects unknown statuses, overlapping or missing ranges and pool
overflows rather than guessing.

| Evidence | Result |
| --- | --- |
| Unicode `IdnaTestV2.txt` | All 6,389 well-formed cases pass for ToASCII and ToUnicode; 2 lines with unpaired surrogates cannot be expressed as Rust strings |
| Unicode `NormalizationTest.txt` | 37,671 NFC pairs generated; the 31,032 whose code points are IDNA-valid are checked through ToUnicode |
| WPT `toascii.json` | All 87 cases pass through `Url::parse` |

IdnaTestV2 statuses V2, V3, U1, A4_1, A4_2 and X4_2 are ignored because they
belong to options the URL Standard turns off (UTS #46 lists them per flag).

**Decision: Punycode work limits.** Punycode decoding and encoding are quadratic
in label length, and hosts are content-controlled. Rustic rejects a label whose
Punycode part exceeds 2,000 bytes or whose encoding input exceeds 1,000 code
points (`IdnaError::LabelTooLong`). DNS limits labels to 63 octets, so this only
rejects hosts that can never resolve; it is a deliberate, tested deviation from
the unbounded algorithm. Other work is linear apart from sorting each run of
combining marks, which is O(n log n).

