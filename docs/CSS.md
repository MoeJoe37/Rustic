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
[Computed style](#computed-style)). CSS-06 is under way: media queries (see
[Media queries](#media-queries)) and incremental invalidation (see
[Invalidation](#invalidation)) are done; custom properties follow.
`<link rel=stylesheet>` and `@import` load over a caller-controlled fetcher
(see [External stylesheets](#external-stylesheets)).
CSS-07a completed the required selectors: `:has()`, form states, `:lang()`,
`:dir()`, the nesting selector `&` with nested style rules, and pseudo-element
states.

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
| Logical | `:is()` and `:where()` (forgiving lists), `:not()`, and `:has()` with an unforgiving relative selector list (`:has(> img, + p)`); `:has()` may not nest, even through `:is()`, and takes no pseudo-elements; nesting is limited to 32 levels |
| Links and states | `:any-link`, `:link` (every link, since history is never exposed), `:visited` (never matches); `:hover`, `:active`, `:focus`, `:focus-visible`, `:focus-within` and `:target` come from the element view and are off until an interaction model exists |
| Form states | `:enabled`, `:disabled`, `:checked`, `:indeterminate`, `:default`, `:required`, `:optional`, `:read-only`, `:read-write`, `:placeholder-shown` (HTML §4.16.3; see [Element states](#element-states)) |
| Language and direction | `:lang()` with idents or strings (extended filtering, so `:lang(de-DE)` matches `de-Latn-DE` and `"*-CH"` works); `:dir()` with any ident, where only `ltr` and `rtl` can match |
| Nesting | `&` anywhere in a compound (after any type selector). In a nested style rule it is the parent's selectors as `:is()`, with the parent's highest specificity; elsewhere it is `:scope` with zero specificity |
| Pseudo-elements | `::before`, `::after`, `::first-line`, `::first-letter` (also with one colon), `::marker`, `::placeholder`, `::file-selector-button`, `::backdrop`, and the highlight pseudo-elements `::selection`, `::target-text`, `::spelling-error` and `::grammar-error`, last in a top-level selector only. Tree-abiding ones may be followed by user-action pseudo-classes (`::before:hover`, also inside `:is()`/`:where()`/`:not()`); highlight ones by nothing |
| Shadow trees | `:host` and `:host(<compound>)`, which match only the host, from its shadow tree's sheets (see [Shadow trees](#shadow-trees)) |
| Rejected for now | Constraint-validation states (`:valid`, `:invalid`, `:in-range`, `:out-of-range`, `:user-valid`, `:user-invalid`), `:open`, `:modal`, `:defined`, media-state pseudo-classes, `:host-context()`, `::part()`, `::slotted()`, `::highlight()`, `::cue` and the column combinator `\|\|` |

Specificity follows Selectors Level 4 §17: `:is()`, `:not()`, `:has()` and
`:nth-child(of S)` take their most specific argument, `:where()` adds
nothing, and `:host(S)` is a pseudo-class plus its argument. `Display` serializes selectors as CSSOM does (identifiers and strings
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

### Nested style rules

`parse_nested_selector_list` reads a nested rule's selectors (CSS Nesting,
Editor's Draft reviewed 2026-11-08). Each entry is relative: one that starts
with a combinator or contains no `&` gets `& ` and the combinator in front, so
`> li` becomes `& > li` and serializes that way. The parent's pseudo-element
selectors are dropped from `&`. Every `&` shares the parent list through an
`Arc`, so a deep chain of nested rules stays linear in size, and the parent's
own nesting depth counts against the 32-level limit.

`StyleSheet::parse` expands nested style rules in the order the cascade sees
them: the rule with the declarations before its first nested rule, then each
nested rule (depth-first), and every later run of declarations as a "nested
declarations rule" with the parent's selectors. A nested `@media` adds its
media condition, and its declarations and relative rules keep the parent's
selectors. Other nested at-rules, and nested rules in `style` attributes, are
dropped with `UnsupportedNestedRule` until those at-rules exist.

### Element states

`rustic-style` derives HTML element states from content attributes and the
tree, since there is no user interaction or script yet (HTML Standard,
reviewed 2026-11-08):

| State | Rule |
| --- | --- |
| `:enabled` / `:disabled` | Buttons, inputs, selects, textareas, optgroups, options and fieldsets; actually disabled through `disabled`, a disabled `optgroup` parent (options), or a disabled fieldset ancestor unless inside its first `legend` child |
| `:checked` | Checkboxes and radios with `checked` (only the last checked radio of a group, as inserting it unchecks the others) and selected options: in a single-line select, the last option with `selected`, or the first non-disabled option when none has it |
| `:indeterminate` | Radios whose group has no checked radio, and `progress` without `value` (a checkbox is indeterminate only through script) |
| `:default` | Each form's first submit button, and checkboxes, radios and options with `checked`/`selected` |
| `:required` / `:optional` | Selects, textareas and inputs whose type takes `required`, with or without it |
| `:read-write` / `:read-only` | Mutable text-like inputs and textareas (not `readonly`, not disabled) and `contenteditable` editing hosts and their editable descendants; everything else is read-only |
| `:placeholder-shown` | Text-like inputs with `placeholder` whose sanitized value is empty (`number` values that are not valid floats sanitize to empty) and textareas with `placeholder` and no text |
| Content language | The nearest `xml:lang`, or `lang` on an HTML, SVG or MathML element, else the last `<meta http-equiv=content-language>` without a comma; `lang=""` means unknown and matches only `:lang("")` |
| Directionality | `dir=ltr`/`rtl`; `dir=auto` and `bdi` use the first strong character (inputs and textareas their value; other elements their text, skipping `bdi`, `script`, `style`, `textarea` and elements with `dir`) and inherit when there is none; `input type=tel` is `ltr`; otherwise the parent's, `ltr` at the root |

Radio groups use the form owner from the `form` attribute or the nearest form
ancestor; parser association of non-descendant controls is not recorded yet
(TD-31). Strong characters come from the pinned Unicode 17.0.0 Bidi_Class
table shared with IDNA (TD-29). Whole-tree facts (IDs, radio groups, default
buttons, the pragma-set language) are built once per match or restyle on first
use, and directionality is memoized per element.

`:has()` arguments with one compound are memoized per (compound, element), so
`div:has(.leaf)` over a 3,000-deep chain matches in linear time; arguments with
inner combinators walk the anchor's subtree or following siblings (TD-30).
Pseudo-element states ask the element view, whose default is "no state".

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
* `tests/selectors_level4.rs` (CSS-07a): 27 more valid selectors with
  serialization and specificity, 24 invalid ones, typed values, 10 nested
  selectors resolved against a parent (each a serialization fixed point) and
  the nesting limit through `&` chains.
* `rustic-style/tests/selectors_level4.rs`: `:has()` in every combinator
  form, the single-compound fast paths checked against the general walk on a
  300-element generated tree, a 3,000-deep linear-time case, every form state
  on one form, `:lang()` extended filtering, inheritance and the pragma-set
  language, `:dir()` with `auto`, `bdi`, `tel` inputs and textareas, nested
  style rules through the cascade, and pseudo-element states.
* The `css-selector` fuzz target checks that every valid selector list
  serializes to a fixed point with the same specificities and selects the same
  elements of a fixed document in HTML, quirks and XML modes. 600,000 executions
  before landing found no defects; with CSS-07a's syntax, seeds and a fixture
  with form controls, `dir=auto` and `lang`, 400,000 more (plus 40,000
  `css-cascade` runs with nested rules) found none.

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
| `font-variant-caps` | `normal`, `small-caps`, `all-small-caps`, `petite-caps`, `all-petite-caps`, `unicase`, `titling-caps` |
| `font-stretch` | Width keyword (`ultra-condensed` … `ultra-expanded`) or nonnegative percentage |
| `position` | `static`, `relative`, `absolute`, `fixed`, `sticky` |
| `top`, `right`, `bottom`, `left` | `auto` or `<length-percentage>` |
| `z-index` | `auto` or `<integer>` (saturating to 32 bits; math functions round) |
| `float` | `none`, `left`, `right`, `inline-start`, `inline-end` |
| `clear` | `none`, `left`, `right`, `both`, `inline-start`, `inline-end` |
| `overflow-x`, `overflow-y` | `visible`, `hidden`, `clip`, `scroll`, `auto` |
| `visibility` | `visible`, `hidden`, `collapse` |
| `opacity` | `<number>` or `<percentage>` (stored as a number; any value, clamped when computed) |
| `vertical-align` | `baseline`, `sub`, `super`, `text-top`, `text-bottom`, `middle`, `top`, `bottom` or `<length-percentage>` |
| `direction` | `ltr`, `rtl` |
| `text-align` | `start`, `end`, `left`, `right`, `center`, `justify`, `match-parent` |
| `text-indent` | `<length-percentage>` (`hanging` and `each-line` later) |
| `text-transform` | `none` or `[capitalize \| uppercase \| lowercase] \|\| full-width \|\| full-size-kana` |
| `letter-spacing`, `word-spacing` | `normal` or `<length-percentage>` |
| `white-space` | `normal`, `pre`, `nowrap`, `pre-wrap`, `break-spaces`, `pre-line` (a longhand for now, TD-27) |
| `text-decoration-line` | `none` or any of `underline`, `overline`, `line-through`, `blink` |
| `text-decoration-style` | `solid`, `double`, `dotted`, `dashed`, `wavy` |
| `text-decoration-color` | `<color>` |
| `background-image` | Comma-separated `none` or `url()` (gradients and other images later; URLs stay unresolved, TD-26) |
| `background-position` | Comma-separated `<bg-position>`: 1–4 values with keywords, offsets and side-relative offsets (`right 10px bottom 20%`) |
| `background-size` | Comma-separated `cover`, `contain` or one or two of `auto` / nonnegative `<length-percentage>` |
| `background-repeat` | Comma-separated `repeat-x`, `repeat-y` or one or two of `repeat`, `space`, `round`, `no-repeat` |
| `background-attachment` | Comma-separated `scroll`, `fixed`, `local` |
| `background-origin`, `background-clip` | Comma-separated `border-box`, `padding-box`, `content-box` |
| Shorthands | `margin`, `padding`, `border-width`, `border-style`, `border-color`, `inset` (1–4 values), `border-top`/`-right`/`-bottom`/`-left` and `border` (width, style and color in any order), `overflow` (one or two values), `text-decoration` (line, style and color in any order), `font` and `background` (below) |

The `font` shorthand (CSS Fonts 4 §2.8) takes up to four of a style, `small-caps`,
a weight and a width keyword (or `normal`) in any order, then the size, an
optional `/ line-height` and the family list, and resets the seven font
longhands; system font keywords (`caption`, `menu`, ...) are not supported yet.
The `background` shorthand (CSS Backgrounds 3 §3.10) takes comma-separated
layers of image, position with optional `/ size`, repeat, attachment and one or
two boxes (one box sets both origin and clip) in any order, with a color only
in the last layer, and sets all eight background longhands with one entry per
layer. These are the CSS-03c catalog.

REN-02c and REN-08 add sizing keywords and flexbox values: `width`,
`height`, `min-width` and `min-height` take `auto | <length-percentage> |
min-content | max-content | fit-content | fit-content(<length-percentage>)`,
and `max-width`/`max-height` take the same keywords after `none`. The flex
longhands are `flex-direction`, `flex-wrap`, `flex-grow`, `flex-shrink`
(non-negative numbers), `flex-basis` (`content | <'width'>`), `order`
(integer), `justify-content`, `align-content`, `align-items`, `align-self`
(CSS Box Alignment 3 keywords with `safe`/`unsafe` and `[first | last]?
baseline`, checked per property) and `row-gap`/`column-gap` (`normal |
<length-percentage [0,∞]>`). The `flex` shorthand follows CSS Flexbox 1 §7.1
(`none`, `auto`, an omitted basis of `0`, and a unitless zero read as a
factor unless two factors precede it),
`flex-flow` takes direction and wrap in any order, and `gap` sets both gaps.
Children of flex and grid containers are blockified at computed-value time.
`tests/flex.rs` covers valid, round-trip and invalid values.

Lengths accept the absolute units, the font-relative units (`em`, `rem`, `ex`,
`rex`, `ch`, `rch`, `cap`, `rcap`, `ic`, `ric`, `lh`, `rlh`) and the viewport
units including the small/large/dynamic variants, plus unitless zero. Every
longhand reports `initial_value()` and `inherited()`. `color`'s initial value is
`CanvasText` and `font-family`'s depends on the user agent; Rustic uses opaque
black and `serif` until system colors and font settings exist. Custom
properties and `var()` are described under [Custom properties](#custom-properties).

### Math functions

`calc()`, `min()`, `max()` and `clamp()` (CSS-03b, [CSS Values and Units
Level 4 §10](https://drafts.csswg.org/css-values-4/#math), Editor's Draft
reviewed 2026-11-08) are accepted wherever a supported longhand takes a
`<length>`, `<length-percentage>` or `<number>`. `Calc` parses a function into
a calculation tree with `+ - * /`, parentheses, nested math functions and the
constants `e`, `pi`, `infinity`, `-infinity` and `NaN`; type-checks it against
its context (a length plus a percentage is allowed only where percentages are
lengths, a number never mixes with a dimension); and simplifies it per §10.10:
absolute lengths become `px`, like units in a sum combine, numbers multiply
into the other factor, and `min()`/`max()`/`clamp()` arguments with the same
unit reduce. Specified values serialize per §10.12 (`calc(1in + 2em - 10% + 4px * 2)`
becomes `calc(-10% + 2em + 104px)`), with children sorted as numbers,
percentages, then dimensions by unit.

| Behavior | Rule |
| --- | --- |
| Whitespace | `+` and `-` need whitespace on both sides (`calc(1px+2px)` is invalid) |
| Products | One side of `*` and the right side of `/` must be a `<number>`; typed division such as `10px / 2px` is rejected for now |
| Range | Not checked when parsing: `width: calc(-10px)` is valid and clamps to `0px` when computed; margins keep negative results |
| Numbers | `font-weight` and `line-height` numbers are evaluated when parsed and clamped to the property's range, so `font-weight: calc(2000)` is `1000` (the specified value serializes as the number, not `calc(1000)`) |
| Degenerate values | Dimensions are censored when produced: NaN becomes 0 and infinities the largest finite value; numbers may be infinite inside a tree and are censored when used or computed |
| Limits | At most 32 nested functions/parentheses (`MAX_CALC_DEPTH`) and 1,024 nodes (`MAX_CALC_NODES`); deeper or larger expressions make the declaration invalid |
| Not yet | `round()`, `mod()`, `rem()`, trigonometric, exponential and sign functions, `none` in `clamp()`, math functions inside colors, angles and times |

At computed-value time every length in a tree becomes pixels and the tree is
simplified again: a single value becomes a plain length or percentage
(`calc(2em + 3px)` with a 10px font is `23px`), while a mix of pixels and
percentages stays a calculation (`calc(50% + 20px)`) for layout to resolve.
`font-size` and `line-height` percentages resolve against the parent's font
size and the element's own font size, so their calculations always compute to
pixels.

### Property value evidence

* The CSS Color files of css-parsing-tests (same pinned revision, 12 files):
  **all 8,041 cases pass**, 7,965 of them byte for byte. The other 76 differ only
  in the sixth decimal, because the expectations were computed with 32-bit
  floats, so the runner compares numbers with a 1e-4 relative tolerance.
* `tests/properties.rs`: 44 valid declarations with their expected longhands,
  26 invalid ones with their error, and a check that every longhand's initial
  value serializes and reparses to itself.
* The `css` fuzz target now also checks that every typed longhand it parses
  serializes to a fixed point.
* `tests/catalog.rs` (CSS-03c): 75 valid declarations of the new longhands
  and shorthands with their expected longhands, a fixed-point check of every
  expected longhand, and 60 invalid declarations.
* `tests/calc.rs` (CSS-03b): 28 simplification/serialization cases, 30
  invalid expressions, number evaluation and censoring, resolution against
  lengths and percentages, the exact depth and node limits, and a
  serialization fixed-point check for values that round. The `css` fuzz
  target's dictionary and seeds include math functions, so its fixed-point
  check covers them.

## Cascade

`StyleSheet::parse` prepares a stylesheet for one origin (user agent, user or
author): style rules keep their parsed selector lists and typed longhand
declarations in order. Invalid selectors drop their rule, invalid or unknown
declarations drop themselves, and each drop is a `StyleDiagnostic` with a span.
`@namespace` rules before the first style rule build the namespace map.
`@media` and `@layer` rules, nested in each other to the parser's
block-nesting limit and read with an explicit stack, are flattened in place:
each style rule records the media query lists it sits in (see
[Media queries](#media-queries)) and its cascade layer (below). Nested style
rules expand after their parent (see [Nested style rules](#nested-style-rules)).
Other at-rules
(`@supports`, ...) are reported as unsupported for now (so is `@import` by `StyleSheet::parse`; `StyleSheet::parse_with_imports`
returns the valid ones for loading), and a declaration directly inside a top-level `@media` or
`@layer` block is reported as misplaced. `parse_style_attribute` reads a
`style` attribute.

`Cascade::specified_style` (or `specified_style_with` for elements in shadow
trees and animated values) picks each longhand's winner by, in order (CSS
Cascade Level 5 §6.1, reviewed 2026-11-08 against the editor's draft):

1. Origin and importance: transitions, important user agent, important user,
   important author, animations, normal author, normal user, normal user agent.
2. Encapsulation context: between the element's own tree and the shadow tree
   it hosts, the outer context wins for normal declarations and the inner one
   for important declarations.
3. Declarations from the element's `style` attribute over rule declarations.
4. Cascade layer: the later layer wins for normal declarations, the earlier
   for important ones; unlayered declarations form the final layer.
5. Specificity of the most specific matching selector in the rule's list.
6. Order of appearance across sheets, rules and declarations.

`initial`, `inherit` and `unset` resolve as specified. `revert` rolls back to
the previous origin (author to user to user agent; in the user-agent origin it
acts like `unset`), counting the animation and transition origins as author.
`revert-layer` rolls back as if nothing existed between the layer's important
and normal levels, inclusive; a `style` attribute is its own layer after the
unlayered one, an important one skips only style-attribute and animation
declarations, and the animation origin is its own layer. Undeclared properties
inherit or take their initial value.

The result is the *specified* value. Inheritance and `inherit` take the parent
element's *computed* value (see [Computed style](#computed-style)), so
`font-size: 2em` is inherited as pixels, not re-applied.

### Cascade layers

`@layer a, b.c;` declares layers; `@layer name { ... }` and anonymous
`@layer { ... }` blocks hold rules and nest to the parser's block-nesting limit
(read with an explicit stack). A `<layer-name>` is dot-separated identifiers
with no whitespace; CSS-wide keywords are reserved, and a block naming more
than one layer is invalid (`InvalidLayerRule`). A declaration directly in a
layer block is `MisplacedDeclaration`. Layer statements may precede
`@namespace`; layer blocks may not. Each sheet lists its layers fully qualified
in order of first declaration (`StyleSheet::layers`), and each rule names its
layer by index. The cascade merges those lists per origin and tree, in sheet
order, into a layer tree: sublayers sort before their parent's own rules,
siblings in declaration order. Anonymous layers are unique to their sheet.
A layer declared inside an `@media` block counts in the layer order even when
the query does not match (TD-25). `@import ... layer()` waits for `@import`.

### Animation and transition origins

`StyleInputs` carries animation and transition values for an element, and the
cascade sorts them into their origins. Rustic has no animation engine yet, so
the engine passes none; the tiers, `revert` and `revert-layer` behave as above
once it does.

### Shadow trees

Every sheet in a `Cascade` belongs to a `TreeContext`: `push` adds a
document-tree sheet and `push_in` one for a shadow tree (`shadow_tree_context`
derives a shadow root's context from its node). User-agent and user sheets style
every tree; an author sheet styles the elements of its own tree and, through
`:host` and `:host(<compound>)`, the tree's host. Inside a shadow tree the host
is featureless: it matches only `:host` forms (also within `:is()` and
`:where()`), combinators reach it from the tree's top-level elements, and
nothing beyond it. `document_cascade` puts each `<style>` inside a shadow tree
in that tree's context, and `style_tree` styles in shadow-including tree order:
a shadow tree right after its host, its top-level elements inheriting from the
host. Slots are not assigned, so the host's light children inherit from the
host rather than from a slot (TD-23); `::slotted()`, `::part()` and
`:host-context()` are rejected. Each tree has its own layer order.

### Cascade evidence

* `rustic-style/tests/cascade.rs`: specificity and order, all six origin and
  importance tiers, style attributes (including important user styles beating
  them), dropped inline declarations, inheritance over several generations,
  every CSS-wide keyword including `revert` across origins, and stylesheet
  diagnostics with spans.
* `rustic-style/tests/layers.rs`: layer order by first declaration across
  sheets, unlayered and style-attribute precedence, the important reversal,
  sublayers and dotted names, distinct anonymous layers, per-origin order,
  every `revert-layer` case above, twelve malformed `@layer` forms, 250 nested
  layer blocks, and the animation and transition origins with `revert` and
  `revert-layer`.
* `rustic-style/tests/shadow.rs`: per-tree sheets, `:host` against outer
  normal and important rules, the featureless host (`*`, `:not()`, `div p` and
  document-tree `:host` never reach it), per-tree layer order, nested shadow
  trees, `:host()` parsing, specificity and serialization, and inheritance from
  the host.
* The `css-cascade` fuzz target styles a fixed document with an arbitrary
  author sheet and checks that splitting the sheet in two, repeating it, or
  adding an empty user sheet never changes any element's specified style
  (400,000 executions before landing, no failures). Sheets with anonymous
  layers skip the split and repeat checks, since those layers belong to their
  sheet; `@layer` fragments are in the dictionary.

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
| `float`, `display` | `float` is `none` for `position: absolute`/`fixed`; floated and absolutely positioned boxes are blockified like the root |
| `overflow-x`, `overflow-y` | If one axis is `visible`/`clip` and the other is not, `visible` becomes `auto` and `clip` becomes `hidden` |
| `opacity` | Clamped to 0–1 |
| `vertical-align` | Lengths and percentages (of the line height) become pixels |
| `text-align` | `match-parent` takes the parent's value with `start`/`end` resolved against the parent's `direction` |
| `text-indent`, `letter-spacing`, `word-spacing`, `top`/`right`/`bottom`/`left` | Lengths become pixels (negative allowed); percentages stay |
| `text-decoration-color` | `currentcolor` becomes the element's `color` |
| `font-stretch` | A percentage (`condensed` is 75%) |
| `background-position` | Each layer becomes left and top offsets: `center` is 50%, `right 10px` is `calc(100% - 10px)` |
| `background-size` | Lengths become pixels |
| Math functions | Lengths become pixels and the tree is simplified again; one value becomes a plain length or percentage clamped to the property's range, a pixel/percentage mix stays a calculation (CSS-03b) |

`ex`, `ch`, `cap` and `ic` (and `rex`, `rch`, `rcap`, `ric`) use the metrics
of the element's first available font when `ComputeContext::fonts` supplies a
`FontMetricsSource` (REN-03a, see [LAYOUT.md](LAYOUT.md#font-adapter-ren-03a)):
x-height, the advance of `0`, cap height and the advance of 水. Units inside
`font-size` use the parent's font, root units the root's, and the root's
parent is the initial `serif` font at weight 400. Without a source or a
matching font they keep the specification's 0.5em fallback for `ex` and `ch`,
0.7em for `cap` and 1em for `ic`. `normal` line height counts as 1.2 for `lh`.
Media and container queries still use the fallback ratios (TD-43). `style_tree` returns both the specified and computed style of each
element and inherits computed values.

### User-agent stylesheet and `<style>` elements

`HTML_USER_AGENT_STYLESHEET` is the supported subset of the HTML Rendering
section's sheet: hidden elements, block and list-item display, table display
types, body margin, paragraph, heading, list and blockquote spacing, heading
sizes and weights, and phrase elements (`b`, `strong`, `em`, `code`, ...),
plus since CSS-03c `white-space` for `pre`-like elements and `nobr`, link and
`u`/`ins`/`s`/`strike`/`del` decorations, `sub`/`sup` and table-part vertical
alignment, `dir` attribute directions and the table `text-indent` reset.
Logical margins and padding are written as their left-to-right physical sides
until writing modes exist. `document_cascade` builds a cascade from that sheet
and every HTML `<style>` element in tree order (not inside templates or
foreign content). A `type` other than empty or `text/css` is skipped; a
`media` attribute restricts the sheet with `StyleSheet::restrict_to`, so it
applies only while its query list matches; each element is reported with its
status and diagnostics.
`document_cascade` fetches nothing: `<link rel=stylesheet>` elements and
`@import`s are reported as blocked. See
[External stylesheets](#external-stylesheets) for loading them.

### External stylesheets

`document_cascade_with` adds `<link rel=stylesheet>` and `@import` to the same
tree-order walk (HTML §4.2.4 and §4.6.7.17, CSS Cascade Level 5 §2.1). The
style system never opens files or sockets: the embedder passes a
`StylesheetFetcher` that owns the policy (which URLs may load, how bytes are
read and decoded), and a `StyleLoader` that enforces budgets shared by the
whole document.

| Case | Behavior |
| --- | --- |
| Base URL | The first `<base href>` resolved against the document URL, else the document URL; relative URLs with no base are reported invalid |
| `<link>` | HTML namespace, `rel` contains `stylesheet` (ASCII case-insensitive), nonempty `href`; `alternate` or `disabled` is reported and not applied; `type` is checked and `media` restricts the sheet like `<style>` |
| `@import` | Only before every rule except `@charset`, `@layer` statements and other `@import`s; `url(...)`, `url("...")` or a string. Misplaced or malformed rules drop with `InvalidImportRule` |
| Import conditions | A media list restricts the imported sheet and everything it imports (with `StyleSheet::restrict_to`), like a `media` attribute does for a whole `<link>` tree; `layer` and `supports()` are reported and the sheet is not loaded until cascade layers and `@supports` exist |
| Order | Imported sheets precede the importing sheet, depth first, so the importing sheet wins ties; the whole group sits at the element's tree position |
| URL resolution | An import resolves against the URL of the sheet that contains it |
| Cycles | An import whose URL (fragment ignored) is already on the import chain is refused; repeating a sheet off the chain loads it again |
| Budgets | `StyleLoadLimits`: 64 fetched sheets (failures count), 16 import levels, 8 MiB of sheet text counted at every inclusion; a fetcher returning more than it was offered counts as too large |
| Failure | Every refusal is a per-sheet status (`InvalidUrl`, `Cycle`, `DepthLimit`, `LoadLimit`, `Fetch(..)`, ...), never fatal, and leaves the rest of the cascade intact |
| Caching | A URL is fetched at most once per loader |

The import tree is walked with an explicit stack, so even a 5,000-deep chain
with unlimited budgets loads without recursion.

`rustic dump-style` uses a local-only fetcher: the document's URL is the
`file:` URL of its canonical path, and a sheet loads only if its `file:` URL
names a regular file inside the document's directory tree, checked both
lexically and after resolving symbolic links. Network, `data:` and other-host
URLs and paths that leave the directory are blocked. Files are read in
chunks within the remaining byte budget and decoded as UTF-8 (a BOM is
stripped; `@charset` and legacy encodings are TD-28). The dump lists each
`@link` with its `href` as written and each `@import` indented by depth, with
statuses and dropped constructs. `--max-stylesheets`, `--max-stylesheet-bytes`
and `--max-import-depth` set the budgets.

### Style dump

`dump_computed_styles` prints one line per element (tag, `#id`, `.class`es)
and, indented below it, each computed value the element does not simply get by
default: an inherited property that differs from the parent's computed value,
or another property that differs from its initial value. Border colors are
omitted on sides with no border, and `text-decoration-color` when no
decoration line is set. Output is bounded by a byte limit.
`rustic dump-style <file>` loads a local HTML file with the same limits as
`dump-dom`, lists each `<style>` element's status and dropped constructs, then
prints the dump. The fixture `crates/rustic-cli/tests/fixtures/styled.html` and
its snapshot `styled.dump` cover it.

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
* `rustic-style/tests/external.rs`: link and `<style>` interleaving, link
  `rel`/`type`/`media`/`disabled`/`alternate` handling, `<base href>`,
  missing base URLs, blocked loads without a fetcher, nested import order and
  relative resolution, import conditions and placement, self and mutual import
  cycles, fetch caching, exact depth, sheet-count and byte budgets, a fetcher
  that ignores its budget, a 5,000-deep import chain, and diagnostic spans
  inside imported sheets.
* `rustic-cli/tests/cli.rs` and `stylesheets.rs` unit tests: the `linked.dump`
  snapshot (BOM, cycles, blocked parent-directory and network URLs, missing
  files) at several chunk sizes, the three budget options, symbolic links out
  of the directory (Unix), and `file:` URL to path round trips with
  percent-encoded separators and NUL rejected.
* The `css-cascade` fuzz target also checks that computed font sizes stay
  finite and non-negative, no computed value serializes as NaN or infinity, and
  the dump always succeeds (300,000 executions before landing, no failures).

## Media queries

`rustic-css` parses [Media Queries Level 4](https://drafts.csswg.org/mediaqueries-4/)
(Editor's Draft, reviewed 2026-11-08) and the Level 5 features that need no
new value types (`parse_media_query_list`, `CssParser::media_query_list`):

| Area | Behavior |
| --- | --- |
| Queries | Comma-separated lists; `not`/`only` with a media type and an optional `and` condition, or a bare condition; `all`, `screen` and `print` match, other types (`tv`, ...) parse and never match; an empty list matches everything |
| Conditions | `not`, `and`, `or` (not mixed without parentheses; no `or` directly after a media type), parentheses nested up to `MAX_MEDIA_NESTING` (32); a query nested deeper is invalid as a whole |
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
share the condition grammar; see [Container queries](#container-queries).

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

## Container queries

`@container` rules follow [CSS Conditional Rules Level 5 §6](https://drafts.csswg.org/css-conditional-5/#container-queries)
(Editor's Draft, reviewed 2026-11-08). `parse_container_query_list` reads a
prelude as a comma-separated list of conditions. Each condition is an optional
container name and an optional query, and at least one of the two must be
present. A query uses the media condition grammar (`not`, `and`, `or`,
parentheses, range syntax) with the container feature set. A media feature
such as `(color)` is `<general-enclosed>` in a container query, and
`inline-size` is `<general-enclosed>` in a media query. An invalid prelude
drops the rule with `InvalidContainerRule`. The rule also nests inside style
rules, `@media` and `@layer`, and they nest inside it.

| Area | Behavior |
| --- | --- |
| Size features | `width`, `height`, `inline-size`, `block-size`, `aspect-ratio`, `orientation`; `inline-size` is `width` and `block-size` is `height`, since writing modes are not supported yet |
| Style queries | `style(--name: value)`, `style(--name)` (true when the property has a value), and `not`/`and`/`or` of parenthesized style queries. Values are compared by serialization after substituting `var()` against the container. Standard properties in `style()` are unknown (browsers do not support them either) |
| Properties | `container-type: normal \| size \| inline-size`, `container-name: none \| <custom-ident>+` (case-sensitive; not `and`, `or`, `not`, `default` or CSS-wide keywords) and the `container` shorthand `<name> [/ <type>]?` |
| Container selection | The nearest ancestor (never the element itself) whose `container-name` lists the condition's name, if it has one, and whose `container-type` has every axis the query tests (`inline-size` gives the inline axis, `size` both). Every element is a style container. A condition with no eligible container is false |
| Lengths | `em`/`ex`/`ch`/`cap`/`lh` use the container's font size, the `r*` units the root's, and viewport units the cascade's media environment |
| Evaluation | Three-valued, as for media queries; the rule applies when any condition is true |

Sizes come from layout, which arrives with M3. Until then the caller supplies
content-box sizes per element: `style_tree_with_sizes` takes a map, and
`StyledDocument::set_container_size` records or forgets one, restyling that
element's subtree when it changes. A size feature tested on an element with
no supplied size is unknown, so plain `style_tree` applies no size-based
container rule. Container query length units (`cqw`, `cqi`, ...), `scroll-state()`
queries, and the anonymous containment that size containers would impose on
layout remain open.

With `@container` rules in the cascade, an element whose computed style
changes during a restyle recomputes its whole subtree. A descendant may query
it even when the elements in between keep equal styles, for example when they
override the custom property it tests.

### Container query evidence

* `rustic-css/tests/container.rs`: prelude parsing and serialization
  (names, lists, `style()`, general-enclosed, invalid preludes); container
  features kept out of media queries; 24 evaluations including unknown
  sizes; the axes each query needs; and the container properties.
* `rustic-style/tests/containers.rs`: nearest-eligible container selection by
  type and name; `em` against the container; an element never being its own
  container; style queries against the parent or a named ancestor;
  `@container` nested in a style rule; diagnostics; and, through
  `StyledDocument`, size changes and container style changes (also past an
  ancestor that overrides the property) matching a full restyle.
* The `css-media` fuzz target also checks every input as an `@container`
  prelude: serialization is a fixed point and the reparsed list has the same
  names, axes and results. It found three fixed-point bugs before landing,
  all about parentheses near the nesting limit. They are fixed by keeping
  parentheses around general-enclosed style queries, by counting `style()`
  nesting from zero, and by treating a query nested past `MAX_MEDIA_NESTING`
  as invalid, which also fixed a latent media query case. All four inputs
  are kept as regressions, and 800,000 further executions found nothing. The `css-invalidation` target gained container size
  mutations and container property values, and the `css` seeds gained
  `corpus/css/container.css`.

## Invalidation

`StyledDocument` keeps every element's style from a full `style_tree` walk and
restyles incrementally. The caller reports each DOM change after making it
(`note` with a `Mutation`: an attribute, a child list, character data, or
everything), and `restyle` recomputes only what those changes can affect,
reusing every other element's style. An `InvalidationMap` built from the
cascade's selectors decides the scope:

| Change | Restyled |
| --- | --- |
| Attribute no selector tests | Nothing (`style` restyles its own element) |
| Attribute tested only in subject compounds (`.x`, `#id`, `[a]`, `:any-link`'s `href`) | The element |
| Attribute tested in an ancestor or sibling position | The element's subtree, and with `+`/`~` the following siblings' subtrees |
| Children inserted, removed or moved | Inserted and moved elements with their subtrees (found because they have no style or a new style parent); with structural selectors (`:nth-*`, `:*-child`, `:*-of-type`, `:empty`) or sibling combinators also the siblings and the parent |
| Text | The parent for `:empty`; ancestor structural tests also reach descendant and following-sibling subjects |
| Attribute or content a `:has()` argument tests | The possible anchors: the changed element's ancestors as far as the relative selectors reach (`> x > y` reaches two levels, any descendant combinator reaches the root), with their preceding siblings when a relative selector uses `+`/`~`; anchor subtrees instead when a `:has()` is outside the subject compound. A test in a nested list's ancestor position (`:has(:is(.p .q))`) also restyles the changed element's subtree |
| `lang` | The element's subtree (`:lang()`); a `<meta http-equiv=content-language>` change restyles everything |
| `dir`, an input's `type` or `value`, or text and children under a `dir=auto` or `bdi` element | The element's subtree and the outermost `dir=auto` ancestor's subtree (`:dir()`) |
| A form-control attribute (`disabled`, `readonly`, `contenteditable`, `placeholder`, ...) | The element's subtree; `checked`, `name`, `type`, `form`, `id` and `selected`, and any child-list change, also restyle every `input`, `button` and `option` (radio groups, default buttons, selected options) |
| Text in a `textarea`, children of a `fieldset`, `select` or `optgroup` | The textarea; the container's children with their subtrees |
| Any reported mutation with an `:nth-child(... of S)` filter | Everything, preserving sibling-count/filter dependencies until targeted counting scopes exist (TD-36) |
| Stylesheets or the media environment (`cascade_changed`) | Everything |
| A container size (`set_container_size`) | The element's subtree |

During the walk an element whose computed style changed restyles its children
(inheritance, and `inherit` on non-inherited properties), which stops where
the children's styles come out equal; a changed root style restyles everything
(`rem`, `rlh`). An element state used outside a subject compound
(`input:checked + label`) restyles the changed elements' subtrees and following
siblings instead of the elements alone. A state inside a nested list's ancestor
position of a `:has()` argument still restyles everything. The `Element
states` facts in [Selectors](#element-states) list what each state reads.

Structural tests in ancestor compounds dirty descendant subjects and, with
sibling combinators, following siblings even when the ancestor itself retains
the same computed style. Review regressions cover child-list and text changes
and filtered sibling membership; see [the M2 review](IMPLEMENTATION.md#m2-review--structural-invalidation).

The result always equals a fresh `style_tree` walk (`matches_full_restyle`
checks it). A `<style>` element's text or a loaded external sheet changes the
cascade itself: rebuild it and call `cascade_changed`. A newly attached shadow
root is `Mutation::Everything`.

### Invalidation evidence

* `rustic-style/tests/invalidation.rs`: twenty scenarios, each checked against a
  full restyle after every change: unrelated attributes restyle nothing and
  `style` one element; subject versus ancestor classes; inherited changes
  stopping where children's styles stay equal; sibling combinators; inserted,
  moved and removed elements; structural selectors and `:empty`; text; form
  states, radio groups and `:has()`; root font size and `rem`; custom
  properties used through `var()`; and a media environment change. The
  TD-32 scenarios pad the page with 40 unrelated elements and require fewer
  than 20 recomputed: `:has()` anchors (descendant, child, `+`/`~`, outside the
  subject, nested ancestor tests, insertion and removal, `:empty`), radio
  groups and default buttons, fieldsets and editing hosts, a textarea's
  placeholder, `lang` and `dir=auto` text. Planting each of seven bugs in the
  targeting (dropping preceding siblings, the `dir=auto` root, the pragma
  check, ...) fails one of them.
* The `css-invalidation` fuzz target styles a fixed document (forms, lists,
  `dir=auto`, links, a dialog) with an arbitrary sheet, applies up to 96
  scripted attribute, insertion, removal, move and text mutations, and checks
  the incremental styles against a full restyle every third step and at the
  end (320,000 executions before landing, no failures).

## Custom properties

A declaration whose name starts with `--` is a custom property. Its name keeps
its case, and its value keeps its component values with the surrounding
whitespace trimmed. An empty value is allowed. The value is rejected at parse
time only for a malformed `var()`, a bad string, a bad URL or an unmatched
closing bracket. CSS-wide keywords apply as usual (`initial` makes the value
guaranteed-invalid). `parse_declaration` returns these as
`PropertyName::Custom` with `DeclaredValue::Custom`.

An ordinary declaration whose value contains `var()` anywhere, including inside
other functions and with any letter case, is accepted at parse time when every
`var()` has the form `var(--name)` or `var(--name, fallback)`. Each longhand,
including every longhand of a shorthand, gets the same
`DeclaredValue::Unparsed`. That value is parsed against the declared property's
grammar after substitution.

For each element, the cascade picks the winning declaration of each custom
property (with `revert` and `revert-layer` as usual). Custom properties all
inherit, so an element that declares none shares its parent's map. Declared
values that contain `var()` are resolved in dependency order using Tarjan's
algorithm without recursion. Every member of a dependency cycle becomes
guaranteed-invalid; references in fallbacks count as dependencies. A property
that only refers to a cycle member is not part of the cycle, so its fallback
applies. Then each `Unparsed` longhand is substituted and parsed. Any failure
makes the longhand invalid at computed-value time, and it acts as `unset`. The
failures are a missing reference without a fallback, a grammar mismatch and an
exceeded limit. Substituted tokens keep their own units, so `2em` set on an
ancestor resolves against the font size of the element that uses it.

Two limits bound the work. Each computation gets a budget of `MAX_SUBSTITUTED_VALUES`
(65,536) component values, which stops exponential doubling chains. One
computation covers an element's custom property map or one `var()` longhand.
Within a restyle the results are shared (TD-33, paid): elements with the same
parent custom properties and the same winning custom declarations share one
map, and a longhand is substituted once per declaration, longhand and map. A
result depends only on these inputs, so sharing never changes a style. A substituted value may nest at most
`MAX_SUBSTITUTION_DEPTH` (128) functions and blocks. Substitution never merges
tokens: `var(--a)var(--a)` serializes with an empty comment between the copies.

Computed custom properties live on `ComputedStyle::custom_properties()`. Both
style dumps list them after the longhands in name order. `dump-style` lists them
where they differ from the parent, cutting values at 256 bytes. Not yet
supported: `env()`, `attr()`, animating registered properties, and CSS-wide
keywords produced by substitution, which are currently invalid instead of
applying.

### Registered custom properties

An `@property --name { syntax: ...; inherits: ...; initial-value: ... }` rule
registers a custom property (CSS Properties and Values API Level 1). It is
valid at the top level and inside `@layer` blocks, and the last valid
registration of a name in the cascade wins (`Cascade::registration`).
`syntax` (a string) and `inherits` (`true` or `false`) are required.
`initial-value` is required unless the syntax is `*`; it must match the
syntax, contain no `var()` and be computationally independent (no relative
lengths). Unknown descriptors are ignored. Anything else drops the rule with
an `InvalidPropertyRule` diagnostic, as does a rule inside `@media`,
`@container` or a style rule. Valid rules are in `StyleSheet::properties`.

Supported syntax components are `<length>`, `<number>`, `<percentage>`,
`<length-percentage>`, `<color>`, `<integer>`, `<custom-ident>` and literal
identifiers, each with an optional `+` or `#` multiplier, joined with `|`,
plus the universal `*`. A syntax that uses another data type such as
`<angle>` is not supported yet, so its rule is dropped.

A registered property starts from its initial value at the root. A
non-inherited one takes its initial value on every element that does not
declare it, and `unset` follows `inherits`. After substitution, a value is
checked against the syntax and stored in canonical form (`0` becomes `0px`,
colors become `rgb()`). A value that does not match is invalid at
computed-value time and acts as `unset`. Relative lengths compute to pixels
against the element that declares them, so descendants inherit pixels.

A registered value with `em`, `ex`, `cap`, `ch`, `ic` or `lh` depends on the
element's `font-size`, and `lh` also on `line-height`; on the root, the
root-relative units count too. When `font-size` or `line-height` uses such a
property through `var()`, directly or through other custom properties, the
dependency graph has a cycle. Every custom property in the cycle and the
longhand then act as `unset`. Unregistered values stay tokens and never form
these cycles.

### Custom property evidence

* `rustic-css/tests/custom.rs`: name case, trimming, empty values, nested
  blocks and escapes; `var()` in shorthands and nested functions; references
  and fallbacks; token separation; the size and depth limits.
* `rustic-css/tests/properties.rs`: malformed `var()` and invalid custom
  values are rejected, and an unknown property with `var()` stays unknown.
* `rustic-style/tests/variables.rs`: inheritance and substitution into
  shorthands and `calc()`; case-sensitive names; fallbacks and
  invalid-at-computed-value-time (`unset`); CSS-wide keywords; cycles, self
  references and references to cycle members; style attributes; late
  resolution of relative units; exponential expansion; dumps; and a
  2,000-paragraph doubling chain that must share one computed map (TD-33).
* `rustic-style/tests/invalidation.rs`: changing an ancestor's custom
  property restyles the elements that use it, matching a full restyle.
* `rustic-css/tests/registered.rs`: syntax parsing and serialization,
  rejected syntaxes, value matching and canonical forms, font-relative units
  and computational independence.
* `rustic-style/tests/registered.rs`: initial values, non-inheritance,
  invalid and substituted values, CSS-wide keywords, pixel computation of
  relative lengths, `em`, `lh` and root `rem` cycles through `font-size` and
  `line-height`, invalid `@property` rules and the last registration winning.
* `rustic-style/src/variables.rs` unit tests: component order and a
  200,000-node cycle without recursion.
* The `css`, `css-cascade` and `css-invalidation` fuzz targets cover the new
  `corpus/css/variables.css` and `corpus/css/registered.css` seeds and the
  custom-property mutation values.
  The `css` target's typed-declaration round trip now includes custom and
  `var()` declarations.

## Conformance gate

`css.conformance` is a declared gate, not a whole-WPT claim. The
[web-platform-tests](https://github.com/web-platform-tests/wpt) parsing pages
call `test_valid_value`, `test_invalid_value`, `test_valid_selector` and
`test_invalid_selector` with string literals. `tools/import_wpt_css_tests.py`
reads those calls statically, never running script, from a pinned checkout
(`cb97c571c3b1947e08b8615eeb4028a2794b23a4`, BSD-3-Clause, license vendored)
for the selector, pseudo-element and property directories of M2's scope. It
writes `crates/rustic-css/tests/fixtures/wpt-css/cases.json` and a manifest with
per-file digests. A call whose arguments are not plain literals is counted as
skipped (219), never guessed. `tools/check_repository.py` verifies the digests.

`tests/wpt_conformance.rs` runs every case through `rustic-css`:

* A selector case must parse (or be rejected) and serialize as WPT expects.
* A value case is in scope when the property is supported. A longhand must be
  accepted (or rejected) and serialize as expected; a shorthand checks
  acceptance only, because serializing a shorthand from its longhands is not
  implemented.
* `.tentative` files test proposals and are never in scope.

Of 2,916 cases, 1,455 are in scope and **1,115 pass** (invalid-selector
102/104, invalid-value 470/471, valid-selector 108/199, valid-value 435/681).
The other 1,461 are out of scope: properties `rustic-css` does not support yet,
and tentative files. Both counts are pinned in the gate, so supporting or
dropping a property changes them deliberately. Every other in-scope case is
listed in `expected_failures.json` with a reason,
and the list is a ratchet: an unlisted failure fails the gate, and so does a
listed case that now passes, or a listed case that no longer exists. The
largest groups of known failures are features outside M2's declared scope:

| Reason | Cases |
| --- | --- |
| Multi-keyword `display` with `flow`, `list-item`, `run-in` or `ruby` | 65 |
| `alpha()`, `contrast-color()`, `light-dark()` and system colors | 87 |
| `::scroll-button()`, `::part()`, `::slotted()`, `:state()`, `:heading` | 89 |
| Math functions serialize folded (`2`, not `calc(2)`), and percentages in numeric math functions | 28 |
| Named colors serialize as `rgb()`; family names are always quoted | 12 |
| Other unimplemented values (`oblique <angle>`, `text-indent` keywords, `cross-fade()`, ...) | 59 |

Before the gate, four serialization bugs it found were fixed: `[|a]` and
`*|*` without a default namespace now drop the redundant prefix, a lone `*`
before a pseudo-element is omitted, and `background-size` writes `<w> auto`.
Keyword-like words (`cursive serif`) in an unquoted family name are now
rejected.
