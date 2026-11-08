# ADR 0010: Template contents as parser-owned fragment roots

- Date: 2026-11-08
- Status: Accepted internal contract; extended by [ADR 0011](0011-fragments-cloning-adoption.md)
  (inserting contents now moves their children, as the DOM does)

## Context

Complete tree construction must insert template children into a template's
contents, a separate `DocumentFragment` that is not part of the template's child
list. `rustic-dom` previously had no fragment node. Template nesting is
content-controlled: each nested template lives in its parent's contents, so
nesting is not bounded by the document's parent-child edge depth.

## Decision

`rustic-dom` adds `NodeKind::DocumentFragment` and a single constructor,
`Document::create_template_contents`, which atomically creates and links an
empty fragment to an HTML `template` element. The element exposes it through
`Element::template_contents`. A contents fragment is a separate tree root: it can
hold ordinary children, but it cannot be inserted, detached or destroyed on its
own, `walk` does not cross into it, and it is destroyed with its template.

Destruction splices each contents fragment in as a final child before continuing
the existing allocation-free walk, so nested templates of any depth are destroyed
without recursion. `dump_tree` prints contents after the template's attributes
using an explicit stack of walkers bounded by live nodes.

## Alternatives and consequences

Representing contents as ordinary children would break child-list semantics and
every consumer that walks the tree. A side table in the parser would let the
completed document lose its contents. A general public fragment API would need
DOM insertion semantics (moving fragment children) and cross-document adoption
that are not designed yet; it remains future DOM work.

Consumers that need template contents must ask for them explicitly. Future
cloning, adoption into an inert template document, and script-visible
`DocumentFragment` objects require their own reviewed design.

## Validation and revisit trigger

DOM tests cover rejected constructors, rejected moves of a contents root, walk
exclusion, dump layout, complete destruction accounting and 20,000-deep nesting.
Parser tests and the html5lib corpus compare template contents. Revisit when
public fragments, cloning or the template contents owner document are added.
