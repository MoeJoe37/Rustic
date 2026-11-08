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
`<link rel=stylesheet>` and `@import` load over a caller-controlled fetcher
(see [External stylesheets](#external-stylesheets)).

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
| Shadow trees | `:host` and `:host(<compound>)`, which match only the host, from its shadow tree's sheets (see [Shadow trees](#shadow-trees)) |
| Rejected for now | `:has()`, `:host-context()`, form-state and language pseudo-classes, other pseudo-elements (including `::slotted()` and `::part()`), pseudo-classes after a pseudo-element, the nesting selector `&` and the column combinator `\|\|` (CSS-07) |

Specificity follows Selectors Level 4 §17: `:is()`, `:not()` and
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

Lengths accept the absolute units, the font-relative units (`em`, `rem`, `ex`,
`rex`, `ch`, `rch`, `cap`, `rcap`, `ic`, `ric`, `lh`, `rlh`) and the viewport
units including the small/large/dynamic variants, plus unitless zero. Every
longhand reports `initial_value()` and `inherited()`. `color`'s initial value is
`CanvasText` and `font-family`'s depends on the user agent; Rustic uses opaque
black and `serif` until system colors and font settings exist. `var()` and
custom properties are later tasks (CSS-06).

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
[Media queries](#media-queries)) and its cascade layer (below). Other at-rules
(`@supports`, ...) and nested style rules are reported as unsupported for
now (so is `@import` by `StyleSheet::parse`; `StyleSheet::parse_with_imports`
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

Fonts are not loaded yet, so `ex` and `ch` use the specification's 0.5em
fallback, `cap` 0.7em and `ic` 1em, and `normal` line height counts as 1.2 for
`lh`. `style_tree` returns both the specified and computed style of each
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
