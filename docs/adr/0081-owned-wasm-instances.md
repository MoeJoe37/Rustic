# ADR 0081 — Owned WebAssembly instances

Date: 2026-11-24
Status: Accepted for M9 numeric Window instances

## Context

A JavaScript function exported by Wasm can outlive the object that initiated
the instance. The native interpreter previously borrowed the validated module
and passive data, which fits Rust embedding calls with an explicit scope.
A persistent instance needs safe ownership across browser jobs and collection.

## Decision

The owned construction path consumes a validated module. Instruction bodies
and signatures move into the interpreter. The instance copies function export
names and passive bytes as preparation work. Active segments populate local
memory during construction. The borrowed path continues to share the original
code and can use Rust callback imports.

Window retains the owned interpreter in a branded host object. The exports
record is an ephemeron value keyed by the instance, and exported functions
trace their owning instance through private capture slots. Aliases of one
function index return the same function object. Argument conversion releases
the native borrow before JavaScript can run, so reentrant coercion can call the
instance. Wasm traps and link errors use the realm's intrinsic prototypes.

Keeping the original byte snapshot and revalidating it avoids a second
persistent copy of code on every Module. Sharing validated code would remove
that preparation step and needs accounting for allocations retained by both
modules and instances.

## Consequences and revisit trigger

The initial Window profile admits local instances with numeric function
exports. It reserves linear memory and table growth against the JS heap while
the instance is live. Source bytes remain available on the module for custom
section reflection. Instantiation validates that immutable snapshot again.
Move validated code into a shared owned module representation when
instantiation benchmarks or linked modules justify sharing it.

JavaScript imports, reference-valued calls and exposed memories, tables and
globals need identity, tracing and reentrancy rules that extend this profile.

## Evidence

Tests cover owned state after the source is released, active and passive data,
start functions, independent instances, export identity, reentrant conversion,
BigInt, multi-value results, traps and Promise overloads. Six pinned WPT
subtests exercise the exports getter and the Instance string tag.
