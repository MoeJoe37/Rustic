# CSS syntax

`rustic-css` implements [CSS Syntax Module Level 3](https://drafts.csswg.org/css-syntax-3/)
(Editor's Draft, reviewed 2026-11-08) with the block-contents changes from
[CSS Nesting](https://drafts.csswg.org/css-nesting-1/). It is original Rust with
no dependencies beyond `rustic-base`, and it has no selector, property, cascade
or computed-value semantics. Those are CSS-02 onward.

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
whitespace as the specification says. Colour, An+B and byte-stream files are not
vendored yet; they belong to later tasks.

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
