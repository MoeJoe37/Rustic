# ADR 0086 — Wasm memory64

Date: 2026-11-25
Status: Accepted for the M9 unshared memory profile

## Context

Wasm modules can declare 64-bit and 32-bit memories together. The
address width determines the operand type for each access and the
valid range of declared page counts. JavaScript uses BigInt page
counts with a 64-bit Memory object.

## Decision

Store the address width in the validation index space and on the
allocated memory. The validator uses each memory's width to check
access operands, data offsets, immediates and size and growth.
The length operand for a mixed-width copy uses i32. A passive data
segment always uses i32 for its source offset and byte count.

The interpreter represents integer addresses as unsigned u64 values.
It checks effective-address addition and transfer ranges before
converting to host indices. A declared memory64 maximum may reach
2^48 pages, while allocation and growth are bounded by the same
byte pool used for memory32. Linking checks width, current size,
and declared maximum. The Window API converts i64 page counts to
BigInt and checks the unsigned 64-bit range.

## Alternatives considered

Separate backing stores for each width would duplicate allocation
accounting and the fixed-buffer generation lifecycle. Both widths use
one store with validation and conversion at their boundaries.

## Consequences and revisit trigger

Browser owners retain the 8 MiB memory cap, including when linking
modules with both memory widths. The byte pool also makes the
profile usable on hosts with 32-bit indices. Revisit physical
allocation strategies when a measured workload needs a larger
memory budget. Shared memories and table64 have independent work.

## Validation

The native tests exercise mixed copies in both directions, active
and passive data, arithmetic overflow and failed growth. The
Window regression covers BigInt conversion, brands, import width,
buffer identity, detachment, and live data across the JS boundary.

[Core memory type validation](https://webassembly.github.io/spec/core/valid/types.html#memory-types),
[Core memory instruction validation](https://webassembly.github.io/spec/core/valid/instructions.html#memory-instructions),
[JavaScript Memory interface](https://www.w3.org/TR/2026/CRD-wasm-js-api-2-20261123/#memories)
