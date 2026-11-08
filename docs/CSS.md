# CSS syntax and selectors

`rustic-css` implements [CSS Syntax Module Level 3](https://drafts.csswg.org/css-syntax-3/)
(Editor's Draft, reviewed 2026-11-08) with the block-contents changes from
[CSS Nesting](https://drafts.csswg.org/css-nesting-1/). It is original Rust with
no dependencies beyond `rustic-base`. Since CSS-02 it also parses
[Selectors Level 4](https://drafts.csswg.org/selectors-4/) syntax, specificity
and the `An+B` microsyntax, and the new `rustic-style` crate matches selectors
against a read-only DOM view (see [Selectors](#selectors)). Properties, the
cascade and computed values are CSS-03 onward.

## Implemented behavior

| Area | Behavior |
| --- | --- |
| Preprocessing | CRLF, CR and FF read as LF and NUL reads as U+FFFD, applied on the fly so spans stay byte offsets into the caller's string |
| Tokens | Ident, function, at-keyword, hash (with the "id" flag), string, bad-string, url, bad-url, delim, number, percentage, dimension (value, integer/number type flag and source representation), whitespace, CDO/CDC and punctuation |
| Escapes | Hex escapes of up to six digits with one trailing whitespace; zero, surrogates and values above U+10FFFF become U+FFFD; escape at EOF becomes U+FFFD with a diagnostic |
| Recovery | Unclosed comments, strings, URLs and blocks; newline in a string (bad-string); bad URL remnants with escaped `)`; invalid escapes |
| Component values | Functions and `{}`/`[]`/`()` blocks are grouped with an explicit stack; unmatched closing brackets stay as tokens |
| Entry points | Stylesheet (skips top-level CDO/CDC), rule list, one rule, one declaration, block contents, one component value and component-value list |
| Rules | Qualified rules and at-rules keep their `{}` block as raw component values; `parse_block_contents` reads a block later as declarations, at-rules and nested qualified rules |
| Declarations | Leading/trailing whitespace removed; `!important` matched ASCII case-insensitively; a `{}` block mixed with other values makes it a nested rule instead, except for custom properties (`--*`) |

Numbers parse with Rust's correctly rounded float conversion and saturate to
`±f64::MAX`, so values are always finite. Unicode-range tokens are not produced:
CSS Syntax removed them, and a grammar that needs `U+` ranges reparses ordinary
tokens. Byte-stream decoding (`@charset`, BOM and protocol encodings) is not
implemented; input is a Rust string.

## Limits and failure

`CssLimits` bounds input bytes (default 8 MiB), tokens (1,000,000), open
functions/blocks (256) and kept diagnostics (1,024; later ones are counted in
`dropped_diagnostics`). Exceeding a byte, token or nesting limit, or a
cancellation request, returns a `CssError` with no partial result. The nesting
limit also bounds the depth of the result tree, so dropping it cannot overflow
the stack. All algorithms are iterative.

## Evidence

The [css-parsing-tests](https://github.com/SimonSapin/css-parsing-tests) corpus
(CC0-1.0) is vendored at `203ce36bffd617db7f118c551e32794561fb273d` for the eight
CSS Syntax entry-point files, with a digest manifest checked by
`tools/import_css_tests.py --check`. Of 149 cases, **139 pass**, 9 are excluded
because they expect unicode-range tokens, and 1 is a recorded known difference:
`declaration_list.json` case 8 expects the 2021 "list of declarations" recovery,
which CSS Nesting replaced (the same input passes in `blocks_contents.json`).

The runner applies documented adaptations, because the corpus follows
rust-cssparser's representation in places: EOF-in-string/URL and "invalid"
markers inside lists are diagnostics here, the obsolete match/column tokens
(`~=`, `||`, ...) are two delims, and declaration values drop surrounding
whitespace as the specification says. `An+B.json` is vendored for CSS-02 (see
below); colour and byte-stream files are not vendored yet and belong to later
tasks.

Eight focused tests cover byte spans across CRLF/comments/NUL, escape
replacement, bad strings and URLs, EOF recovery with spans, number
representation and saturation, raw blocks with later nested parsing and
custom properties, exact limits, cancellation and the single-entry-point errors.
The `css` fuzz target checks that token spans are ordered, nonempty, on character
boundaries and cover all non-comment input, and that nested block parsing is
total and deterministic.

```sh
cargo test --locked -p rustic-css
python3 tools/import_css_tests.py --check
```

## Selectors

`parse_selector_list` (or `CssParser::selector_list`) reads a selector list
from component values, such as a style rule's prelude, with a `NamespaceMap`
for `@namespace` prefixes. A top-level list is not forgiving: one invalid
selector invalidates it. Unsupported syntax is rejected with a
`SelectorErrorKind` and a byte span instead of being accepted and silently never
matching.

| Area | Supported |
| --- | --- |
| Simple selectors | Type and universal with `ns\|`, `*\|` and `\|` prefixes and the default namespace; `#id`; `.class`; `[attr]` with `=`, `~=`, `\|=`, `^=`, `$=`, `*=` and the `i`/`s` flags |
| Combinators | Descendant, `>`, `+`, `~` |
| Structural | `:root`, `:empty`, `:scope`, `:first-child`, `:last-child`, `:only-child`, `:first-of-type`, `:last-of-type`, `:only-of-type`, `:nth-child(An+B [of S])`, `:nth-last-child(An+B [of S])`, `:nth-of-type()`, `:nth-last-of-type()` |
| Logical | `:is()` and `:where()` (forgiving lists), `:not()`; nesting is limited to 32 levels |
| Links and states | `:any-link`, `:link` (every link, since history is never exposed), `:visited` (never matches); `:hover`, `:active`, `:focus`, `:focus-visible`, `:focus-within` and `:target` come from the element view and are off until an interaction model exists |
| Pseudo-elements | `::before`, `::after`, `::first-line`, `::first-letter` (also with one colon) and `::marker`, last in a top-level selector only |
| Rejected for now | `:has()`, form-state and language pseudo-classes, other pseudo-elements, pseudo-classes after a pseudo-element, the nesting selector `&` and the column combinator `\|\|` (CSS-07) |

Specificity follows Selectors Level 4 §17: `:is()`, `:not()` and
`:nth-child(of S)` take their most specific argument, and `:where()` adds
nothing. `Display` serializes selectors as CSSOM does (identifiers and strings
escaped, `An+B` normalized, an implicit `*` dropped), and serializing again is a
fixed point.

### Matching

`rustic-style` matches over any `ElementView`: a copyable read-only handle that
can reach parent and sibling elements, attributes and its `:empty` state.
`DomElement` implements it for `rustic-dom` documents, and `query_selector_all`
walks a subtree in tree order. `MatchContext` carries the document facts that
change matching: in an HTML document, type and attribute names match HTML
elements ASCII case-insensitively and HTML's legacy attribute list compares
values case-insensitively; in quirks mode IDs and classes match
case-insensitively; and `:scope` uses the scoping root when one is given.

The matcher runs right to left with an explicit stack and the "restart from the
closest descendant or sibling combinator" pruning, so a long descendant chain
never backtracks exponentially and a selector with tens of thousands of
compounds never recurses. Recursion happens only into pseudo-class arguments,
which the parser bounds. `:nth-*` pseudo-classes count siblings on each test
without caching (TD-20).

### Selector evidence

* `An+B.json` from css-parsing-tests (same pinned revision): **128 of 128**
  cases pass.
* `tests/selectors.rs`: 41 valid selectors with expected serialization and
  specificity (each also checked as a serialization fixed point), 30 invalid
  selectors with their error kind, namespaces, nesting limits and error spans.
* `rustic-style/tests/matching.rs`: matching against parsed HTML pages,
  including combinators, every attribute operator and case rule, SVG names,
  namespaces, quirks mode, XML mode, `:scope`, pseudo-element subjects, a
  brute-force check of `:nth-child()` over 91 `An+B` values, and pathological
  selectors (a 40-compound descendant chain over 100 nested elements and a
  20,000-compound selector).
* The `css-selector` fuzz target checks that every valid selector list
  serializes to a fixed point with the same specificities and selects the same
  elements of a fixed document in HTML, quirks and XML modes. 600,000 executions
  before landing found no defects.

```sh
cargo test --locked -p rustic-css -p rustic-style
cargo run --release -p rustic-fuzz -- run --target css-selector --iterations 200000
```
