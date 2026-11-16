# ADR 0029 — Scripted node creation and character data

Date: 2026-11-16. Status: accepted for WEB-03.

## Context

The page host in ADR 0028 provides live wrappers for an existing document.
Factories, attribute changes, and character-data operations add conversions
that can execute JavaScript. A text split also needs a new wrapper and final
DOM state to become available together.
[Pinned DOM Standard source](https://github.com/whatwg/dom/blob/e4f4f4f0ace83fe6c52ab68e047351ce7f2dcd69/dom.bs)

## Decision

A binding checks the receiver and the required argument count, then converts
arguments in declared order without a DOM write borrow. This permits a
JavaScript getter, valueOf, or toString to call another binding. After
conversion the operation resolves its node identity again on the current
tree. Character-data signatures and Text and Comment constructor defaults
use the generated metadata from ADR 0025.

Node factories create detached nodes in the active document and release one
on wrapper allocation failure. Text splitting validates the offset, reserves
the suffix and a new node slot, and prepares the wrapper while the original
text is intact. The split then divides the original payload charge and links
the new Text node immediately after its source when attached. A failed
preflight changes neither the original text nor its siblings.

Attribute operations match qualified names while preserving the namespace
and prefix of an existing record. In the current HTML document, HTML element
attribute names fold ASCII case. Names use the existing scalar UTF-8 storage
and character data and attribute values use UTF-16. The storage limit for
names is TD-127. Additional documents, namespace-aware factories, and custom
element registries remain future work.

## Alternatives and consequences

Holding the tree through argument conversion would prevent reentrant
bindings. Allocating the suffix as an ordinary text node before shortening
the original would require additional payload budget for the same data.
Creating the wrapper after the split would expose an edited tree on
wrapper allocation failure.

## Validation

Tests cover factory and subtype identity, UTF-16 offsets, conversion order,
reentrant edits, namespaced attributes, and the same tree observed by the
renderer. The split test exercises the exact payload and node limits and
simulates a failed wrapper reservation. Mutation observers and range updates
belong to their respective future integrations.
