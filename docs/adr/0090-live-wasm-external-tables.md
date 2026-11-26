# ADR 0090 — Live WebAssembly external-reference table exports

Date: 2026-11-26
Status: Accepted for module-defined nullable external-reference tables in M9

## Context

A Window export of a module-defined `externref` table must share the guest
table's storage and preserve JavaScript identity. Values can outlive a table
entry when Wasm moves them into internal globals or keeps them in a frame
suspended during an imported call.

## Decision

The interpreter stores opaque host identities and distinguishes guest null
from every identity. The Window binding encodes JavaScript values losslessly
and decodes them only in the same heap. The instance's native state visitor
marks the identities reachable from guest globals, tables and retained
segments. The table wrapper reaches the instance through the page's existing
ephemeron.

Before an imported call, the interpreter exposes external identities in all
suspended guest locals and operands. The binding roots those identities until
the call returns and the interpreter releases its store borrows before
dispatch. Guest start runs before module exports become accessible and the
numeric-only import boundary supplies no external references.

Both address widths use the interpreter's existing full-width checks and
aggregate table budget. The JS heap reservation accounts for the native
entry representation. Frame scanning and host root preparation consume work
in their respective runtimes.

## Alternatives considered

A JavaScript mirror would have to track native mutations, including bulk
operations, and temporarily own every value reachable only from guest frames.
Rooting all values ever inserted would retain objects after guest state stops
referencing them. Visiting current edges allows normal cyclic collection.

## Consequences and revisit trigger

[ADR 0091](0091-wasm-external-table-imports.md) extends this design to
external-reference imports and roots start-time state. TD-208 tracks
cross-instance function identity and function-table imports.

## Validation

The Window integration test covers table32/table64, alias and value identity,
null versus undefined, guest/JS growth, bounds, and reentry with explicit
collection while guest frames are suspended. It checks reclamation after the
last table edge is overwritten.

[WebAssembly JavaScript Interface, tables](https://webassembly.github.io/spec/js-api/#tables)
