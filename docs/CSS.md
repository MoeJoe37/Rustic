# CSS syntax, selectors and values

`rustic-css` implements [CSS Syntax Module Level 3](https://drafts.csswg.org/css-syntax-3/)
(Editor's Draft, reviewed 2026-11-08) with the block-contents changes from
[CSS Nesting](https://drafts.csswg.org/css-nesting-1/). It is original Rust with
no dependencies beyond `rustic-base`. Since CSS-02 it also parses
[Selectors Level 4](https://drafts.csswg.org/selectors-4/) syntax, specificity
and the `An+B` microsyntax, and the new `rustic-style` crate matches selectors
against a read-only DOM view (see [Selectors](#selectors)). CSS-03 added typed
values for colors, lengths, the box model, `display` and font basics (see
[Property values](#property-values)), CSS-04 the cascade (see
[Cascade](#cascade)), and CSS-05 computed values, the HTML user-agent
stylesheet, `<style>` attachment and a style dump (see
[Computed style](#computed-style)). CSS-06 is under way: media queries are
done (see [Media queries](#media-queries)); invalidation and custom properties
follow.

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
below), and so are the CSS Color Level 3/4 files (CSS-03). The Color 5
(`device-cmyk`) and byte-stream files are not vendored yet.

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
which the parser bounds, and each top-level match memoizes nested list results
per element, so nested `:is(… *)` levels cost polynomial rather than
exponential time (TD-21, paid with CSS-04). `:nth-*` positions are memoized
too, and a counted sibling's position is derived from the previous counted
sibling's, so matching every child of a long list is linear rather than
quadratic (TD-20, paid with CSS-05). A restyle (`style_tree`) and
`query_selector_all` share one cache for the whole walk.

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

## Property values

`Color::parse` reads a `<color>` from one component value: named colors,
`transparent`, `currentcolor`, 3/4/6/8-digit hex, `rgb()`/`rgba()` and
`hsl()`/`hsla()` in legacy and modern syntax, `hwb()`, `lab()`, `lch()`,
`oklab()`, `oklch()` and `color()` with the nine predefined spaces, with `none`
where the modern syntax allows it. sRGB forms convert to sRGB channels at parse
time and serialize as `rgb()`/`rgba()`; the others keep their space. Math
functions, relative colors, `color-mix()`, `light-dark()` and system colors are
rejected for now.

`parse_declaration` turns a syntax-level declaration into typed longhand
declarations, expanding shorthands and applying CSS-wide keywords (`initial`,
`inherit`, `unset`, `revert`, `revert-layer`) to every longhand. Unknown
properties return `UnknownProperty` and values outside the grammar return
`InvalidValue`, so the cascade drops them as CSS requires.

| Property | Grammar |
| --- | --- |
| `display` | One keyword (`none`, `contents`, `block`, `inline`, `inline-block`, `flow-root`, `list-item`, `flex`, `inline-flex`, `grid`, `inline-grid`, `table`, `inline-table` and the table-part keywords) or an outer/inner pair such as `inline flow-root` |
| `color`, `background-color`, `border-*-color` | `<color>` |
| `margin-*` | `auto` or `<length-percentage>` |
| `padding-*` | Nonnegative `<length-percentage>` |
| `width`, `height`, `min-width`, `min-height` | `auto` or nonnegative `<length-percentage>` |
| `max-width`, `max-height` | `none` or nonnegative `<length-percentage>` |
| `border-*-width` | `thin`, `medium`, `thick` or nonnegative `<length>` |
| `border-*-style` | `none`, `hidden`, `dotted`, `dashed`, `solid`, `double`, `groove`, `ridge`, `inset`, `outset` |
| `box-sizing` | `content-box`, `border-box` |
| `font-size` | Absolute/relative size keyword or nonnegative `<length-percentage>` |
| `font-weight` | `normal`, `bold`, `bolder`, `lighter` or a number from 1 to 1000 |
| `font-style` | `normal`, `italic`, `oblique` (angles later) |
| `font-family` | Comma-separated strings, identifier sequences and generic families |
| `line-height` | `normal`, nonnegative number or nonnegative `<length-percentage>` |
| Shorthands | `margin`, `padding`, `border-width`, `border-style`, `border-color` (1–4 values), `border-top`/`-right`/`-bottom`/`-left` and `border` (width, style and color in any order) |

Lengths accept the absolute units, the font-relative units (`em`, `rem`, `ex`,
`rex`, `ch`, `rch`, `cap`, `rcap`, `ic`, `ric`, `lh`, `rlh`) and the viewport
units including the small/large/dynamic variants, plus unitless zero. Every
longhand reports `initial_value()` and `inherited()`. `color`'s initial value is
`CanvasText` and `font-family`'s depends on the user agent; Rustic uses opaque
black and `serif` until system colors and font settings exist. `calc()` and the
other math functions, `var()`, the `font` shorthand and custom properties are
later tasks.

### Property value evidence

* The CSS Color files of css-parsing-tests (same pinned revision, 12 files):
  **all 8,041 cases pass**, 7,965 of them byte for byte. The other 76 differ only
  in the sixth decimal, because the expectations were computed with 32-bit
  floats, so the runner compares numbers with a 1e-4 relative tolerance.
* `tests/properties.rs`: 32 valid declarations with their expected longhands,
  22 invalid ones with their error, and a check that every longhand's initial
  value serializes and reparses to itself.
* The `css` fuzz target now also checks that every typed longhand it parses
  serializes to a fixed point.

## Cascade

`StyleSheet::parse` prepares a stylesheet for one origin (user agent, user or
author): style rules keep their parsed selector lists and typed longhand
declarations in order. Invalid selectors drop their rule, invalid or unknown
declarations drop themselves, and each drop is a `StyleDiagnostic` with a span.
`@namespace` rules before the first style rule build the namespace map.
`@media` rules, nested to any depth, are flattened in place: each style rule
records the media query lists it sits in (see [Media queries](#media-queries)).
Other at-rules (`@import`, `@layer`, `@supports`, ...) and nested style rules
are reported as unsupported for now, and a declaration directly inside a
top-level `@media` block is reported as misplaced. `parse_style_attribute`
reads a `style` attribute.

`Cascade::specified_style` picks each longhand's winner by, in order:

1. Origin and importance: important user agent, important user, important
   author, normal author, normal user, normal user agent.
2. Declarations from the element's `style` attribute over rule declarations.
3. Specificity of the most specific matching selector in the rule's list.
4. Order of appearance across sheets, rules and declarations.

`initial`, `inherit` and `unset` resolve as specified. `revert` rolls back to
the previous origin (author to user to user agent; in the user-agent origin it
acts like `unset`), and `revert-layer` does the same because cascade layers are
not implemented yet. Undeclared properties inherit or take their initial value.
`style_tree` styles a whole document subtree in tree order, inheriting from
each parent element.

The result is the *specified* value. Inheritance and `inherit` take the parent
element's *computed* value (see [Computed style](#computed-style)), so
`font-size: 2em` is inherited as pixels, not re-applied. Cascade layers,
shadow-tree encapsulation and the animation/transition origins are later
steps.

### Cascade evidence

* `rustic-style/tests/cascade.rs`: specificity and order, all six origin and
  importance tiers, style attributes (including important user styles beating
  them), dropped inline declarations, inheritance over several generations,
  every CSS-wide keyword including `revert` across origins, and stylesheet
  diagnostics with spans.
* The `css-cascade` fuzz target styles a fixed document with an arbitrary
  author sheet and checks that splitting the sheet in two, repeating it, or
  adding an empty user sheet never changes any element's specified style
  (400,000 executions before landing, no failures).

## Computed style

`ComputedStyle::compute` turns a specified style into computed values (CSS
Cascade 5 §4.4, with each property's "computed value" line):

| Property | Computed value |
| --- | --- |
| `font-size` | Pixels: keywords from `xx-small` 9px to `xxx-large` 48px with `medium` 16px; `larger`/`smaller` ×1.2 / ÷1.2 of the parent; percentages and `em` of the parent's size; `rem` of the root's (the root's own `rem` uses 16px) |
| Other lengths | Pixels: absolute units, font-relative units against the element's font size, `lh`/`rlh` against the computed line height, viewport units against `ComputeContext` (default 1280×720) |
| Percentages of the containing block | Kept as percentages for layout (widths, heights, margins, padding) |
| `line-height` | Percentages and lengths become pixels; numbers and `normal` stay, so children rescale them |
| `font-weight` | A number; `bolder`/`lighter` use the CSS Fonts 4 relative-weight table on the parent's weight |
| Colors | `currentcolor` becomes the element's `color` (for `color` itself, the parent's) |
| Border widths | Pixels (`thin` 1, `medium` 3, `thick` 5), and 0 when that side's style is `none` or `hidden` |
| `display` | Blockified on the root element |

Fonts are not loaded yet, so `ex` and `ch` use the specification's 0.5em
fallback, `cap` 0.7em and `ic` 1em, and `normal` line height counts as 1.2 for
`lh`. `style_tree` returns both the specified and computed style of each
element and inherits computed values.

### User-agent stylesheet and `<style>` elements

`HTML_USER_AGENT_STYLESHEET` is the supported subset of the HTML Rendering
section's sheet: hidden elements, block and list-item display, table display
types, body margin, paragraph, heading, list and blockquote spacing, heading
sizes and weights, and phrase elements (`b`, `strong`, `em`, `code`, ...).
Logical margins and padding are written as their left-to-right physical sides
until writing modes exist. `document_cascade` builds a cascade from that sheet
and every HTML `<style>` element in tree order (not inside templates or
foreign content). A `type` other than empty or `text/css` is skipped; a
`media` attribute restricts the sheet with `StyleSheet::restrict_to`, so it
applies only while its query list matches; each element is reported with its
status and diagnostics. `<link rel=stylesheet>` needs fetching and is not loaded.

### Style dump

`dump_computed_styles` prints one line per element (tag, `#id`, `.class`es)
and, indented below it, each computed value the element does not simply get by
default: an inherited property that differs from the parent's computed value,
or another property that differs from its initial value. Border colors are
omitted on sides with no border. Output is bounded by a byte limit.
`rustic dump-style <file>` loads a local HTML file with the same limits as
`dump-dom`, lists each `<style>` element's status and dropped constructs, then
prints the dump. The fixture `crates/rustic-cli/tests/fixtures/styled.html` and
its snapshot `styled.dump` cover it.

## Media queries

`rustic-css` parses [Media Queries Level 4](https://drafts.csswg.org/mediaqueries-4/)
(Editor's Draft, reviewed 2026-11-08) and the Level 5 features that need no
new value types (`parse_media_query_list`, `CssParser::media_query_list`):

| Area | Behavior |
| --- | --- |
| Queries | Comma-separated lists; `not`/`only` with a media type and an optional `and` condition, or a bare condition; `all`, `screen` and `print` match, other types (`tv`, ...) parse and never match; an empty list matches everything |
| Conditions | `not`, `and`, `or` (not mixed without parentheses; no `or` directly after a media type), parentheses nested up to `MAX_MEDIA_NESTING` (32) |
| Feature forms | `(name)`, `(name: value)`, `min-`/`max-` prefixes on range features, and range syntax `(name < v)`, `(v <= name)`, `(v1 < name <= v2)` with matching directions |
| Features | `width`, `height`, `device-width`, `device-height`, `aspect-ratio`, `device-aspect-ratio`, `orientation`, `resolution` (`dpi`, `dpcm`, `dppx`, `x`), `color`, `color-index`, `monochrome`, `grid`, `scan`, `update`, `overflow-block`, `overflow-inline`, `color-gamut`, `dynamic-range`, `video-dynamic-range`, `pointer`, `any-pointer`, `hover`, `any-hover`, `prefers-color-scheme`, `prefers-reduced-motion`, `prefers-reduced-transparency`, `prefers-reduced-data`, `prefers-contrast`, `forced-colors`, `inverted-colors`, `scripting`, `display-mode` |
| Errors | A malformed query becomes `not all` without affecting the rest of the list; an unknown feature, a value of the wrong type, or any other parenthesized or functional expression is `<general-enclosed>` and evaluates to unknown |
| Evaluation | Three-valued (Kleene) logic; unknown is false at the top level and stays unknown under `not`; relative lengths use the initial 16px font and the viewport |
| Serialization | CSSOM form: lowercase names, `(name: value)`, ratios as `a / b`, range tests as written, general-enclosed reserialized from its tokens |

`MediaEnvironment` describes the surface: media type, viewport and device
size, resolution, color depth, pointer and hover capabilities and the user
preferences. The default is a 1280×720 light-scheme desktop screen with a
mouse and no scripting (JavaScript does not run yet). A `Cascade` holds one
environment and caches each sheet's media list results; `set_environment`
re-evaluates them and returns whether any result changed, which is the signal
to restyle after a viewport resize or preference change. Container queries
are planned after CSS-06's invalidation work.

### Media query evidence

* `rustic-css/tests/media.rs`: 41 serialization cases (canonical forms,
  general-enclosed, invalid queries and a fixed-point check for each), bounded
  deep nesting, and 47 queries evaluated on a desktop, a phone and a print
  environment.
* `rustic-style/tests/cascade.rs`: nested `@media` rules flattened with their
  conditions, rules switching as the environment changes (and
  `set_environment` reporting no change when no query is affected), and
  `@media` diagnostics.
* The `css-media` fuzz target checks that serialization is a fixed point after
  one round and that the reparsed list matches the same four environments. It
  found two fixed-point bugs before landing (parentheses nested past the limit,
  and whitespace split by a comment), both fixed and kept as regression inputs;
  then 900,000 further executions found nothing.

### Computed style evidence

* `rustic-style/tests/computed.rs`: font-size keywords, percentages, `em`,
  `rem` and root `rem`; absolute, font-relative, `lh` and viewport units;
  line-height percentages versus numbers under inheritance; relative font
  weights; `currentcolor` and border widths with `none` styles; root
  blockification; a clean parse of the user-agent sheet; `<style>` attachment
  (type, media, templates, foreign content, diagnostics); and an exact dump.
* `rustic-style/tests/matching.rs`: cached `:nth-*` positions agree with fresh
  matching for nine selectors including nested `of S` lists, and four `:nth-*`
  selectors over a 20,000-element list stay linear.
* `rustic-cli/tests/cli.rs`: the `dump-style` snapshot at several chunk sizes,
  exact output budgets and URL rejection.
* The `css-cascade` fuzz target also checks that computed font sizes stay
  finite and non-negative, no computed value serializes as NaN or infinity, and
  the dump always succeeds (300,000 executions before landing, no failures).
