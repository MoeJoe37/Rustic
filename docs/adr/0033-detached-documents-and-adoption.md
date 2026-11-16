# ADR 0033 — Detached documents and transactional adoption

**Status** Accepted for WEB-03.

## Context

The [DOM Standard's Document and DOMImplementation interfaces](https://dom.spec.whatwg.org/#interface-document)
create documents outside the active browsing context. A script can keep a
detached node alive and still observe its owner document. An adoption changes
the storage identity of each moved node and preserves its script wrapper.

## Decision

The page registers one renderer-owned document and at most 16 auxiliary
documents. Each uses the active document's node, text, and depth limits.
Document format identifies the factory's MIME type and controls HTML case
folding. Each DOMImplementation is a branded wrapper for its associated
document. It shares the document wrapper cache and tracing rules while
remaining distinct from a Node.

At a VM safe point the coordinator traces all registered arenas together.
The active document is a host root. For an auxiliary document, reaching
the document, any of its nodes, or its DOMImplementation marks its connected
tree. A detached node retains both its own tree and its owner document.
Unmarked documents are removed from the registry along with their metadata
and collection cache entries. Detached documents have a null defaultView.

Cross-document copy and move paths reuse the DOM's iterative subtree plan.
The binding charges the script turn before copying. For insertion, the DOM
stages the destination nodes and validates the parent, reference, document
order and depth before committing the move. On failure it releases the staged
nodes and keeps the source unchanged. A foreign fragment keeps its own identity
and transfers just its children as a forest. The coordinator reserves storage
before the move and rekeys surviving wrappers and node-owned edges as the DOM
commits. Cached child collections use the JS wrapper as their key. Binding
mutations resolve node identities after any argument coercion.

## Verification

Binding tests cover document construction and namespaces, shallow and deep
copies, live wrapper identity, coercion that adopts a node, hierarchy failures,
and reclamation of detached document cycles under GC stress. DOM tests cover
insertion at the exact node limit and rollback of a rejected forest. The engine
test moves nodes between a detached document and the parsed page and checks
live class matching as the owner document changes.
