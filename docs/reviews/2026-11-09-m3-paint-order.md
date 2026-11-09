# M3 paint order and clipping checkpoint — 2026-11-09

Continues `feature` after REN-02b (`bd1ed3a`) and REN-03a (`09981e8`).

## Implemented

REN-04b: CSS 2.2 Appendix E paint order for the block reference (stacking
contexts, `z-index`, floats, positioned and fixed boxes), overflow clips from
the layout's containing-block clip chain, and opacity groups. `opacity` is no
longer an unsupported paint feature. [PAINT.md](../PAINT.md) is the contract.

## Safety and portability

The paint order is built with an explicit task stack, never recursion over
content. Group buffers are checked against the raster memory budget before
allocation and their passes are charged to the work budget. Compositing stays
`f64`-only, so the new layered golden checksum is the same on every target.

## Deliberate debt

TD-46: opacity groups allocate a whole surface rather than their bounds.

## Evidence

Six new integration tests in `crates/rustic-raster/tests/paint.rs`, the
extended `paint-raster` fuzz target (one background per box, balanced groups)
with a layered seed, and the full sandbox suite.
