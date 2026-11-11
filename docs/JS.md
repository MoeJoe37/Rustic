# JavaScript engine (JS-01 to JS-03)

`rustic-js` is Rustic's own ECMAScript implementation (M4, ADR 0001: no
existing JavaScript engine is used or embedded). JS-01 provides the front end:
UTF-16 source text, the lexical grammar, and a parser from scripts and modules
to an abstract syntax tree with source spans. JS-02 adds the runtime data
model (see [Runtime data model](#runtime-data-model-js-02)) and JS-03 its
garbage collector (see [Garbage collection](#garbage-collection-js-03)).
Nothing executes yet; the interpreter is JS-04.

## API

- `Source::from_str_limited` / `Source::from_utf16` hold source text as UTF-16
  code units, refusing text over a caller limit (and always over
  `MAX_SOURCE_UNITS`, so every offset fits in a `u32`). `Source::position`
  gives one-based lines and UTF-16 columns, as browsers report them.
- `parse_script` and `parse_module` return an `ast::Program` or a
  `ParseError`: `Syntax` (with a span and message), `TooDeep` or `Cancelled`.
- `Lexer` is public for tools and tests. The parser scans with the division
  goal and rescans a `/` as a regular expression where an expression starts.
- `ast::dump` renders a tree as an S-expression for tests and the
  `parse` example (`cargo run -p rustic-js --example parse -- -e 'a?.b'`).

## Scope

The full ECMAScript 2025 syntax is parsed, including classes with private
names, fields and static blocks; generators and async functions; optional
chaining; `import`/`export` with attributes and string names; top-level
`await`; numeric separators; BigInt; and templates with cooked and raw values.

Early errors are reported during the parse: redeclarations across var, lexical,
catch, parameter and function scopes; labels, `break` and `continue` targets;
`super`, `new.target`, `arguments` in class fields and static blocks; private
name declarations and uses; duplicate exports and undeclared exported names;
strict-mode restrictions (reserved words, `eval`/`arguments` bindings, `with`,
legacy octals, `delete` of names, duplicate parameters); `"use strict"` with
non-simple parameters; `yield` and `await` in their parameter and context
rules; `??` mixed with `||`/`&&`; and unary operators before `**`.

Arrow parameters and destructuring targets use the specification's cover
grammars: the parser reads an expression, records positions that are valid
only in one reading, and converts it when `=>` or `=` follows. It never
backtracks over tokens.

Annex B syntax is accepted in scripts, as browsers must: HTML-like comments,
legacy octal literals and escapes, sloppy-mode function declarations as `if`
bodies and labelled statements. Modules reject all of it.

## Limits

Parsing is bounded so hostile source cannot exhaust resources:

- Source length is checked before scanning.
- `ParseLimits::max_depth` (256) caps nested statements, expressions,
  patterns and functions.
- `ParseLimits::max_stack_bytes` (1 MiB) caps measured stack use, because
  frame sizes differ between builds (TD-90). The default is safe on any thread
  with 2 MiB of stack.
- Left-nested chains (`a + b + c ...`, `a.b.c ...`) are built iteratively, and
  dropping an expression tree never recurses per node, so a 200,000-operand
  chain parses and drops without stack growth.
- The cancellation token is polled between statements.

## Unicode data

`tools/generate_js_data.py` generates `crates/rustic-js/src/unicode/tables.rs`
(ID_Start, ID_Continue and the Space_Separator category) from Unicode 17.0.0
files at the same pinned `unicode-org/unicodetools` revision the text and IDNA
data use. Input digests and the output digest are pinned in
`crates/rustic-js/tests/fixtures/js_data_manifest.json`, and
`tools/check_repository.py` rejects hand-edited output. The data is under the
[Unicode License v3](../crates/rustic-js/UNICODE-LICENSE.md).

## Conformance

Test262 is not vendored. The parse-phase gate in `tests/test262.rs` runs every
file of `test/language` at the revision pinned in
`tests/fixtures/test262_expected_failures.txt`, in each mode its metadata
requires (sloppy and strict scripts, or module; `raw`, `onlyStrict` and
`noStrict` honored), and expects a `negative: phase: parse` file to fail in
every mode and every other file to parse:

```sh
git clone https://github.com/tc39/test262 /path/to/test262
git -C /path/to/test262 checkout 48221d7c1f7745dc13e420a2e850a248c0b2afb4
RUSTIC_TEST262_DIR=/path/to/test262 \
  cargo test --release -p rustic-js --test test262 -- --ignored
```

At that revision 22,956 of 23,123 files pass (99.3%). All 167 expected
failures are RegExp pattern early errors (TD-88). 709 files using proposal
features are skipped and counted separately (TD-89). The gate fails on any new
failure and on any expected failure that starts passing, so the list stays
exact. To regenerate it, run
`cargo run --release -p rustic-js --example test262_parse -- DIR OUT` and
keep the header lines.

This is parse-phase evidence only. Runtime and resolution phases, and the
published Test262 results the M4 gate requires, arrive with the runner in
JS-07. Parse-phase results must not be advertised as ECMAScript support.

## Fuzzing

The `js-parse` target of `rustic-fuzz` parses every input as a script, a
module and a strict script (the `js-heap` target is described under
[Garbage collection](#garbage-collection-js-03)). It checks that nothing panics or overflows the
stack, that syntax error spans lie inside the source, that a successful parse
dumps deterministically, and that anything valid in strict mode is valid in
sloppy mode. Seeds are one snippet per line of
`crates/rustic-fuzz/corpus/js-parse/`.

## Runtime data model (JS-02)

`rustic_js::runtime` holds values, objects and environments, laid out for a
small heap as [ADR 0015](adr/0015-js-value-and-memory-layout.md) records:

| Item | Cost |
| --- | --- |
| `Value` (any type) | 8 bytes, NaN-boxed; arena handles are `u32` |
| Object with no properties | 72 bytes (shape, prototype, flags, slot and element vectors, one pointer to rare state) |
| Each named property of an object built like others | 8 bytes (16 for an accessor); the shared shape describes it |
| Each dense array element | 8 bytes |
| String | 24 bytes (12 on 32-bit targets) plus 1 byte per code unit when all units are below 256, otherwise 2 |
| Binding in a declarative environment | 12 bytes |

An `Agent` owns one `Heap` and the realm's intrinsic objects. Every
allocation is charged to the heap's byte budget; exceeding it returns
`Abrupt::OutOfMemory`, which script cannot catch, and the heap is never
over-committed. Unreachable values are freed by the collector below.

- **Strings** (`JsStr`) are Latin-1 or UTF-16 with UTF-16 semantics.
  Property names are interned, so key comparison is an id compare, and array
  indices are a separate key kind that is never turned into a string.
- **Objects** start on a shared shape tree. Adding the same keys in the same
  order reuses the same shapes; deleting the newest key steps back up the
  tree. Changing attributes, deleting another key or exceeding 32 named
  properties moves the object to its own ordered dictionary. Indexed elements
  are a dense vector (holes allowed) until an index lands far past the end or
  an element gets non-default attributes; then they become a sparse map.
- **Internal methods** follow §10.1 for ordinary objects, including
  `ValidateAndApplyPropertyDescriptor`, prototype-cycle checks and
  `[[OwnPropertyKeys]]` ordering, plus the Array (`length`, `ArraySetLength`)
  and String (read-only indices) exotic objects. §7.3 operations such as
  `CreateDataPropertyOrThrow`, `GetMethod`, `SetIntegrityLevel` and
  `CreateArrayFromList` are on `Agent`.
- **Conversions and comparisons** (§7.1, §7.2): `ToPrimitive` with
  `@@toPrimitive`, `ToNumber` (the full StringNumericLiteral grammar,
  correctly rounded), `Number::toString` (shortest round-trip digits),
  `ToString`, `ToObject`, `ToPropertyKey`, `SameValue`, `===`, `==` and `<`,
  including exact BigInt/Number comparison. `ToBigInt` and BigInt arithmetic
  are not implemented yet (TD-92).
- **Environments** (§9.1): declarative, function, object (`with`, honoring
  `@@unscopables`) and global records, with the temporal dead zone stored as
  an internal "uninitialized" value. Reference Records implement `GetValue`,
  `PutValue` and `InitializeReferencedBinding`; completion records cover
  `UpdateEmpty` and `LoopContinues`.
- **Built-ins** are limited to what conversions call: `valueOf`/`toString` on
  `Object.prototype` and the wrapper prototypes, `Error.prototype.toString`,
  and the global value properties. Native functions are plain Rust `fn`s; the
  full library is JS-05.

`tests/runtime.rs` checks the spec rules above and pins the memory costs in
the table.

## Garbage collection (JS-03)

The collector is a nonmoving, stop-the-world mark/sweep over the arenas
([ADR 0016](adr/0016-js-garbage-collection.md)):

- **Roots** are the agent's interned names, well-known symbols and
  intrinsics, the `Roots` (values and environments) the caller passes to
  `Agent::collect_garbage`, and host roots made with `Agent::root`. A
  `HostRoot` carries a generation, so a released handle reads as `None` even
  after its slot is reused.
- **Marking** uses a worklist, never recursion, and keeps mark bits in
  temporary bit sets, so objects carry no collector header.
- **Sweeping** frees objects, strings, symbols, BigInts, environments, unused
  shapes (and their transitions) and unused atoms. Freed indices are reused
  lowest first, emptied arena tails are returned to the allocator, and the
  byte budget is reset to the exact footprint of what is live.
- **Safe points.** A handle in a Rust local is not a root, so allocation never
  collects by itself. Callers poll `Agent::should_collect` where they can list
  what they hold; the interpreter will do so between instructions. The trigger
  starts at 1 MiB and becomes twice the live footprint after each collection,
  capped at seven eighths of the heap limit (TD-94).
- **Safety.** Using a handle that was collected panics at once instead of
  reading another value.

`GcStats` reports bytes before and after and the cells freed per kind.
`tests/gc.rs` covers each root and reference kind, cycles, 200,000-deep
chains, shape and atom reclamation, index reuse and exact budget return. The
`js-heap` fuzz target checks random allocate/mutate/link/root/collect
sequences against a shadow model. Weak references and DOM host-object tracing
are the remaining part of JS-03 (TD-96); collection is not incremental
(TD-95).
