# Fuzz regressions

Minimized reproducers of fixed defects live in `<target>/` subdirectories,
named as the harness saved them (`<kind>-<fingerprint>.bin`). Every campaign
replays them first, and `tests/harness.rs` replays them on every test run. Add
the fix and the reproducer in the same change, with a note in the commit
message describing the violated contract.

| Reproducer | Contract | Fix |
| --- | --- | --- |
| `css-media/violation-076bafb4550d968d.bin` | Media query serialization must be a fixed point; parentheses nested past `MAX_MEDIA_NESTING` around general-enclosed collapsed on reparse | CSS-06: parentheses around general-enclosed stay general-enclosed |
| `css-media/violation-0639aa11e8f78f12.bin` | `@container` prelude serialization must be a fixed point; a `style()` query's parentheses around general-enclosed collapsed on reparse | CSS-06: parentheses around general-enclosed style queries stay general-enclosed |
| `css-media/violation-87bace5f31c6386a.bin` | `@container` prelude serialization must be a fixed point; a `style()` query's nesting limit counted the media parentheses around it, which serialization can drop | CSS-06: `style()` counts its own nesting from zero |
| `css-media/violation-c97cedf85e7049bb.bin` | `@container` prelude serialization must be a fixed point; a nesting limit hit inside `style()` kept the inner parentheses as general-enclosed while the outer ones collapsed | CSS-06: a `style()` nested past the limit is general-enclosed as a whole |
| `css-media/violation-cfd9fe1a97a41aa2.bin` | Media query serialization must be a fixed point; a nesting limit hit under `not` kept inner parentheses as general-enclosed while the outer ones collapsed | CSS-06: a query nested past `MAX_MEDIA_NESTING` is invalid as a whole |
| `css-media/violation-d6d25ca1ecbf13e5.bin` | Media query serialization must be a fixed point; whitespace split by a comment inside general-enclosed doubled and then merged on reparse | CSS-06: consecutive whitespace values serialize as one space |
| `css-media/violation-8bb9f829bb829b62.bin` | A media query must match the same after a serialize/parse round trip; `(device-width: 2e-8px)` serialized as `0px` and then matched a zero-width device | M7 CI fix: media feature values are read at their serialized precision (six decimals) |
| `html-fragment/panic-f8f44a43672ff812.bin` | Tree construction must not panic; a `selectedcontent` inside its own selected option had its children replaced at end of file and the parser destroyed a still-open element | M7 CI fix: replaced `selectedcontent` children the parser still refers to are only detached |
| `paint-raster/violation-3bbada6275055942.bin` | The fuzz invariant allowed one background per box; an inline box with a border split around a block correctly paints one per fragment (CSS 2 §9.2.1.1) | M7 CI fix: the invariant allows one background per inline fragment |
| `codec-jpeg/panic-e46234fe1b3136ec.bin` | JPEG decoding must not panic; a component with a non-integral sampling ratio (for example 3:2) took the identity path and indexed past its plane | REN-05d: only components at full resolution skip resampling |
| `js-run/panic-001c54ab9f56eb66.bin` | Running script must not panic; a function whose last statement was a `return` inside an `if` had no implicit return for the skipped branch to land on, and ran off the end of its code | JS-04: every function body ends with an implicit return |
