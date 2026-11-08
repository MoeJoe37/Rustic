# Standards and compatibility

## Policy

Rustic implements specified web behavior rather than a project-specific HTML or
JavaScript dialect. Subsets are inevitable during development; publish precisely
which algorithms, syntax, APIs, and test selections work. Standards links below
are authoritative references, not claims that Rustic is conformant today.
No HTML, CSS, JavaScript, or network behavior is implemented at M0.

Current M1 evidence includes the original [URL/parser and origin subset](URL.md)
with 887 pinned upstream URL-data cases, including file and non-ASCII URLs, plus named
validation diagnostics and UTS #46 IDNA checked against all well-formed Unicode
17.0.0 IdnaTestV2 cases and WPT `toascii.json`. This is not full browser WPT
conformance and does not implement blob or browser binding features.
HTML evidence now includes streaming input, tags/declarations and character
references, with 1,079 selected declaration cases and all 4,626 cases in three
pinned reference files; [HTML.md](HTML.md) records exact scope and provenance.
The text-mode corpus adds 109 selected records in 282 explicit RCDATA/RAWTEXT/
script/plaintext contexts. Tokenizer context selection is not HTML tree construction.

The full-product baseline targets complete required CSS/ECMAScript semantics and
broad real-site interoperability, including GPU-backed APIs, WebAssembly and the
Rustic MV2 ecosystem. See [PRODUCT.md](PRODUCT.md) for dated baseline selection,
complete feature accounting, competitive claims and vendor/OS eligibility limits.
Initial subsets are staging steps, not permanent replacements for mature standards.

For each task, record the specification section, revision/date, upstream test
revision, supported subset, known deviations, and regression fixtures. Prefer
standard algorithms over reverse engineering one browser's output. When browsers
disagree, use the normative text and upstream tests, reduce the case, and record
the discrepancy; don't silently hard-code a site-specific exception.

## Reference map

| Area | Primary references | First implementation scope |
| --- | --- | --- |
| Infrastructure | [Infra](https://infra.spec.whatwg.org/), [Encoding](https://encoding.spec.whatwg.org/) | Strings, names, byte decoding, error recovery |
| URLs and origins | [URL](https://url.spec.whatwg.org/), [HTML origins](https://html.spec.whatwg.org/multipage/browsers.html#origin) | Parsing, serialization, relative URLs, tuple/opaque identity |
| Documents | [DOM](https://dom.spec.whatwg.org/), [HTML syntax](https://html.spec.whatwg.org/multipage/syntax.html), [HTML parsing](https://html.spec.whatwg.org/multipage/parsing.html) | Nodes/mutation, tokenizer/tree builder, local document lifecycle |
| CSS syntax and selectors | [CSS Syntax 3](https://www.w3.org/TR/css-syntax-3/), [Selectors 4](https://www.w3.org/TR/selectors-4/) | Grammar/recovery and an explicit selector subset |
| Cascade and values | [Cascade 5](https://www.w3.org/TR/css-cascade-5/), [Values and Units 4](https://www.w3.org/TR/css-values-4/) | Origins/importance/inheritance, initial/computed/used values |
| Formatting | [CSS 2](https://www.w3.org/TR/CSS2/), [Display](https://www.w3.org/TR/css-display-3/), [Flexbox](https://www.w3.org/TR/css-flexbox-1/), [Grid](https://www.w3.org/TR/css-grid-1/) | Block/inline first; distinct formatting-context work after that |
| Text | [CSS Text](https://www.w3.org/TR/css-text-3/), [CSS Fonts](https://www.w3.org/TR/css-fonts-4/), [Unicode bidi](https://www.unicode.org/reports/tr9/), [text segmentation](https://www.unicode.org/reports/tr29/) | Font metrics, shaping adapter, bidi, wrapping, caret clusters |
| Language | [ECMAScript](https://tc39.es/ecma262/), [Intl](https://tc39.es/ecma402/), [Test262](https://github.com/tc39/test262) | Interpreter semantics and scoped conformance selections |
| WebAssembly | [Core](https://webassembly.github.io/spec/core/), [JS interface](https://webassembly.github.io/spec/js-api/), [Web API](https://webassembly.github.io/spec/web-api/) | Required original validator/interpreter, explicit imports and JS/Web integration |
| Bindings/execution | [Web IDL](https://webidl.spec.whatwg.org/), [HTML web APIs/event loops](https://html.spec.whatwg.org/multipage/webappapis.html) | Conversions, wrappers, tasks/jobs, reentry and lifecycle |
| Network | [Fetch](https://fetch.spec.whatwg.org/), [HTTP semantics](https://www.rfc-editor.org/rfc/rfc9110), [HTTP caching](https://www.rfc-editor.org/rfc/rfc9111), [TLS 1.3](https://www.rfc-editor.org/rfc/rfc8446) | Browser policy over a reviewed transport and TLS library |
| Web security | [CSP](https://www.w3.org/TR/CSP3/), [Mixed Content](https://www.w3.org/TR/mixed-content/), [Secure Contexts](https://www.w3.org/TR/secure-contexts/) | Enforce policies on every implemented loading/execution path |
| Persistence | [Storage](https://storage.spec.whatwg.org/), [IndexedDB](https://www.w3.org/TR/IndexedDB/), [Service Workers](https://www.w3.org/TR/service-workers/) | Partition keys/quotas first; transactions/offline later |
| Accessibility | [HTML AAM](https://www.w3.org/TR/html-aam-1.0/), [Core AAM](https://www.w3.org/TR/core-aam-1.2/), [Accessible Name](https://www.w3.org/TR/accname-1.2/) | Semantic tree, roles/names/focus and platform mapping |
| Interoperability | [WPT](https://github.com/web-platform-tests/wpt), [WPT documentation](https://web-platform-tests.org/) | Parser data first; reftests and testharness after runner integration |

Living standards and working drafts evolve. Implement only the normative features
selected for the milestone; proposals are not automatically part of the target.
Newer CSS modules may supersede parts of CSS 2; record the applicable module when
implementing an algorithm. Web security also requires platform and process design
that these specifications do not provide as a Rust implementation blueprint.

## Feature-status vocabulary

| Status | Meaning |
| --- | --- |
| Planned | Design/task only; API not advertised |
| Experimental | Implementation behind an explicit development flag; subset and test evidence documented |
| Partial | Specific exposed subset works; failures and unsupported semantics enumerated |
| Supported in declared scope | Required selected tests and product flows pass for a stated version/platform |
| Disabled | Implementation deliberately unavailable with a stated reason |

Add a machine-readable feature inventory once real subsystems exist. Each entry
will contain a stable feature ID, status, platforms, flags, spec links, milestone,
test selection, known failures, and owning task. Do not populate a giant inventory
with false “supported” flags just because an empty crate or binding exists.

## Conformance results and updates

Store upstream revisions in a checked-in manifest when importing suites. Keep
large checkouts outside normal source history; preserve license/notices for any
vendored test fixtures or fonts. A runner report includes commit/toolchain/OS,
suite revision, selection command, backend/fonts, selected total, executed total,
passes, failures, expected failures, skips, crashes and timeouts. Report both
suite-level and subtest counts where they differ.

Broad web conformance cannot be summarized honestly by a percentage from a tiny
handpicked subset. Retain unsupported selections in the denominator of a broad
report, or clearly label a subset report. Unexpected passes get investigated and
expectations updated; failures are never converted to “expected” as an automatic
way to make a PR green. Pin updates land separately with before/after evidence.
