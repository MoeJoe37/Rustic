# ADR 0079 — Explicit WebAssembly function imports

Date: 2026-11-24
Status: Accepted for the M9 numeric embedding boundary

## Context

A validated module can declare function imports and put them in tables or
export them. The runtime needs to link those imports before start executes.
The browser bridge will later supply JavaScript values and the capabilities
available to the importing realm.

## Decision

The embedder supplies borrowed `FunctionImport` bindings when instantiating
a module. Each binding names a module and field, has a scalar numeric
signature, and holds a Rust callback. Linking checks exact names and
signatures before allocating instance state or invoking start. Functions share
an import-first index space. The same bound function can be called directly,
through a table, through a tail call or as an export.

The interpreter charges for argument and result transfers and checks their
capacity before entering the callback. It passes the invocation cancellation
token to the embedder and rechecks it when the callback returns. Returned
values are checked before entering the guest stack. The embedder owns the
callback's execution policy and the capabilities it captures.

## Consequences and revisit trigger

The host controls names and lifetimes with explicit bindings. A guest name
only selects a supplied binding. Imported memory, tables, globals, tags, and
reference-valued host calls require an explicit ownership and identity
design. Revisit this boundary when the JavaScript adapter needs rooted
values, exception translation, or shared state between module instances.

## Evidence

The integration suite checks linked direct and indirect calls, re-export,
start, signature and name failures, transfer budgets, cancellation and
host traps. The binary fuzz target supplies a deterministic numeric import.
A pinned core replay exercises bulk table copies whose entries refer to
imported numeric functions.
