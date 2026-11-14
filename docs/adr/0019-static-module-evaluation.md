# 0019 — Traced environments for static modules

Date 2026-11-14
Status accepted for static graphs with synchronous or asynchronous evaluation

## Context

The static linker already resolves an exported name to its defining binding.
A module may update that binding long after initialization, and a closure or
namespace reachable from the host can keep it alive. In a cycle, function
declarations and `var` must be available before any module body runs, while
lexical bindings remain uninitialized.

## Decision

Compile two bytecode entry points per module and use one environment layout
for both. The first entry point initializes functions and `var`. The second
executes statements. Allocate all environments and namespaces and link their
entries before running either entry point.

Trace namespace-to-environment and import-to-binding edges with the same
collector as closures. Root the instance table through the VM while evaluation
runs. After evaluation, ordinary heap reachability governs the lifetime of
exports, with host roots available for values retained outside the VM.

The host loads the graph through explicit callbacks. Each standalone evaluation
creates fresh instances for its graph, and each body runs at most once within
that evaluation. The unpublished entry point provides no ambient I/O.

The asynchronous entry point returns the entry namespace and a completion
promise. An iterative depth-first traversal identifies cyclic components and
records pending dependencies. Module bodies with top-level await use the VM's
existing async frames. Promise reactions own a traced heap record for the graph
while a body is suspended. A reaction propagates rejection through waiting
importers or runs newly available bodies in evaluation order.

## Alternatives considered

Copying imported values into local slots would leave stale values after
writes. Generating JavaScript accessors for namespace properties would expose
accessor descriptors where ECMAScript requires live data descriptors.
Keeping all graph instances as permanent agent roots would retain unreachable
graphs.

## Validation and revisit trigger

The module regressions cover shared live values, initialization order, cyclic
lexical reads, key ordering, property descriptors, and reachability under GC
stress. Async regressions add sibling execution, resumption after host-driven
promise resolution and rejection across cycles. Revisit instance lifetime when
host-driven dynamic imports can reuse previously loaded modules.
