# URL records, parsing and origins

## Implemented scope

`rustic-url` owns an original Rust URL record/parser and process-local origin
model. It depends only on `rustic-base`; no existing URL parser, browser engine,
JavaScript runtime or third-party Rust package supplies its implementation.
Parsing performs no DNS lookup, network access, file access or navigation.

Implemented in the initial DOC-02a slice:

- Immutable records with scheme, userinfo, optional host/port, structured path,
  and distinct absent/empty query and fragment values.
- `http`, `https`, `ws`, `wss` and `ftp` network URLs, ASCII domains and default-port normalization.
- Percent-decoded special hosts, forbidden-host checks and lowercase ASCII domains.
- Original short-form decimal/octal/hex IPv4 parsing and original IPv6 parsing/
  canonical compression, including strict dotted IPv4 tails inside IPv6.
- Generic non-special hierarchical URLs, opaque hosts and opaque paths.
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

`file:` and `blob:` return explicit unsupported errors. Special URLs requiring
Unicode IDNA processing also return an unsupported error, with no guessed host
or fallback authority. ASCII A-labels are processed under the reviewed standard's
non-strict ASCII-domain behavior. Unicode paths/queries/fragments use UTF-8 percent
encoding; non-special opaque hosts do not use IDNA.

Pending: reviewed Unicode IDNA/data adapter, file/platform rules, blob registry
and origin rules, validation-error diagnostics, legacy document query encodings,
URL setters and URLSearchParams. There is no public browser `URL` binding yet.
Successful parsing does not mean a scheme is navigable, secure, allowed by policy,
or supported by the future network service. FTP syntax support is not an FTP client.

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
| `parser` | Scheme/authority/path/reference orchestration, limits and checkpoints |
| `host` | Domain/opaque/address host representation and host validation |
| `ipv4` / `ipv6` | Address algorithms and IPv6 canonical serialization |
| `encoding` | Component percent-encode sets and host percent decoding |
| `origin` | Tuple keys, opaque identity, equality/hashing and serialization |
| `error` | Input-independent errors and explicit unsupported features |

Run the offline example:

```sh
cargo run --locked -p rustic-url --example resolve
```

## Pinned upstream evidence

The offline integration test runs **685 selected cases** from the 899 test records
in WPT's `url/resources/urltestdata.json` at
[revision 8c17bfeb3e66a2986be16b0e004f85ace68db647](https://github.com/web-platform-tests/wpt/blob/8c17bfeb3e66a2986be16b0e004f85ace68db647/url/resources/urltestdata.json).
The fixed selection rule is defined before running Rustic: ASCII input/base,
excluding file/blob schemes and percent-encoded non-ASCII octets in any component.
The last rule is intentionally conservative and excludes some otherwise-supported
encoded paths to keep this subset independent of IDNA.

| Count | Meaning |
| --- | --- |
| 899 | All upstream test records; section-description strings are not cases |
| 685 | Selected and executed with no unexpected failures |
| 443 | Expected successful parses checked field by field and reparsed for record stability |
| 242 | Expected parse rejections |
| 214 | Excluded: 142 file/blob, 56 non-ASCII input/base, 16 encoded non-ASCII |

Every successful selected case checks href, protocol, username/password, host,
hostname, port, pathname, search and hash, plus origin where supplied upstream.
This does not execute URLSearchParams expectations, setters, legacy encodings,
other WPT files or a full browser WPT harness. The 685-case result is a declared
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
