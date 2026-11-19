# ADR 0047 — Mutation observers over a document journal

Date: 2026-11-19
Status: Accepted for WEB-07 (`dom.observers`); implemented

## Context

`MutationObserver` ([DOM §4.3](https://dom.spec.whatwg.org/#mutation-observers))
needs facts that exist only at the moment of a change: the moved nodes and
their siblings, an attribute's previous value, the old character data, and
which observers were registered on the target's inclusive ancestors. Changes
come from three places: page bindings, the HTML parser between scripts, and
host code such as checkbox activation. `rustic-dom` must stay ignorant of
JavaScript, and `rustic-bindings` must never hold a document borrow while
script runs ([ADR 0028](0028-scripted-document-collection.md)).

Registrations also create a lifetime question. The DOM says a node holds its
registered observers strongly while the observer's node list is weak, and
node identities change when a subtree is adopted into another document
([ADR 0011](0011-fragments-cloning-adoption.md)).

The alternatives were: queue records at each binding call site (misses
parser and host edits and duplicates the DOM's record rules in every
binding); give `rustic-dom` callbacks (a JS-free crate calling into script
mid-mutation, against the borrow rule); or record facts in the document and
interpret them in the bindings.

## Decision

1. **A document journal.** `Document::set_mutation_journal(true)` makes every
   tree, attribute and character-data mutation append a `MutationEntry`
   (`ChildList`, `Attribute`, `CharacterData`) in the order the DOM queues
   records: a move logs its removal before its insertion; a fragment logs
   its emptying; `splitText` logs the insertion before the data change;
   "replace all" logs one record and suppresses its inner insertion. Each
   entry is allocated before the tree changes, so a failed edit leaves both
   unchanged. A cross-document move logs both records in the destination's
   journal, naming moved nodes by their new IDs. A disabled journal costs
   one branch per edit.
2. **Interpretation in the bindings.** The first `observe()` enables the
   journal in every document of the page. When the outermost binding
   operation releases the page, and before every microtask checkpoint, the
   bindings drain the journals, walk each target's inclusive ancestors,
   apply the options, `attributeFilter` and old-value rules, append a
   `MutationRecord` to each interested observer and queue one notification
   microtask (`Agent::enqueue_microtask`, also used by `queueMicrotask`).
3. **Registrations are node edges.** A registration is a private array
   `[observer, options, attributeFilter, source]` that its node holds through
   `DomHeap::add_edge`. Adoption retargets it with the node, collection frees
   it with the node, and it keeps the observer alive exactly as long as the
   node is reachable. The observer keeps a list of its registrations (not
   their nodes); `disconnect` and transient expiry mark registrations dead,
   and a node drops a dead edge when a later mutation inspects it.
4. **Delivery.** Notification takes every pending observer in creation order
   (as engines do), empties its queue, drops its transient registrations and
   calls the callback with `(records, observer)` and `this` set to the
   observer. A throw is reported to the listener-error count and does not
   stop later observers.
5. **Parser scripts.** The scripted document performs a microtask checkpoint
   before each parser-inserted classic runs, as the HTML "`</script>`" steps
   require, so records for parser insertions arrive before the next script.

## Consequences

- One mechanism serves bindings, the parser and host edits, and no binding
  call site needs observer code.
- Records for host and parser edits find interested observers when the
  journal is drained, not at the instant of the edit. Between two script
  turns the tree is the parser's and nothing can observe the difference
  except a node the parser itself moves later in the same chunk
  ([TD-151](../TECH_DEBT.md#td-151--mutation-records-for-parser-and-host-edits-resolve-observers-when-drained)).
- A disconnected registration stays on its node until a mutation looks at it
  or the node is collected, and stays in a long-lived observer's list until
  `disconnect` ([TD-150](../TECH_DEBT.md#td-150--dead-mutation-observer-registrations-are-dropped-lazily)).
- Once any observer exists, the page records every edit and copies old
  character data and attribute values, even if no observer asks for them.

## Validation

`crates/rustic-dom/tests/journal.rs` checks each entry kind, move and
fragment order, failure atomicity and cross-document identities.
`crates/rustic-bindings/tests/mutation_observer.rs` covers delivery order
against promise jobs, option conversion order and validation, subtree,
filters and old values, re-observation, `takeRecords`, transient
registrations, creation order, exceptions, collection through nodes,
adoption and host edits, all under GC stress. The engine test
`mutation_observers.rs` checks parser insertions at every split. Eleven
upstream WPT files are in the pinned Window selection.

## Revisit

When Range, `normalize()`, `innerHTML` or custom elements land (each needs
its own record order), or when a profile shows journal cost on pages that
keep an observer for their whole lifetime.
