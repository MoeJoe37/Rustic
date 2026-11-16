# ADR 0025 — Pinned DOM IDL metadata

Date: 2026-11-16
Status: Accepted for the WEB-02 text interface generator

## Context

The conversion layer in ADR 0024 needs reusable type and argument metadata
for the upcoming DOM bindings. Recopying signatures into individual
binding methods would let defaults and conversion attributes drift from
the standard.

## Decision

Pin four complete interface blocks from WHATWG DOM revision
`e4f4f4f0ace83fe6c52ab68e047351ce7f2dcd69`.
The source slice covers `CharacterData`, `Text`, `CDATASection` and
`Comment`. It keeps the declared `Node` parent reference for later
binding work. The manifest records the full source, selected slice and
upstream license digests.
[Pinned DOM Standard source](https://github.com/whatwg/dom/blob/e4f4f4f0ace83fe6c52ab68e047351ce7f2dcd69/dom.bs)

`tools/generate_dom_idl.py` accepts the selected Window interface syntax
and rejects unsupported declarations, overloads, unknown parents and
inheritance cycles. It generates typed Rust metadata covering declared
members, argument order, optional empty-string defaults, readonly
attributes, conversion policies and `[NewObject]` results. Metadata-driven
argument conversion delegates to ADR 0024. Realm installation and actual
page API exposure belong to the binding layer.

## Alternatives and consequences

Emitting page-facing functions in this step would need wrapper lifetime,
error and exposure policies that WEB-02 is still implementing. The
checked-in descriptor table lets the wrapper layer consume the same
signature source. The repository check regenerates the table offline and
compares the bytes. Normal Rust builds compile the committed module.

## Validation and revisit trigger

The generator verifies the vendored slice offline. An optional full
source check verifies its checksum and re-extracts the exact pinned
blocks. Tests cover malformed input and stale metadata. Rust tests
exercise the emitted signatures, including using generated argument
conversions to replace UTF-16 data in an actual DOM node. Extend the
accepted syntax when another pinned interface family needs it.
