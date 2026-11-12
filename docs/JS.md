# JavaScript engine (JS-01 to JS-05)

`rustic-js` is Rustic's own ECMAScript implementation (M4, ADR 0001: no
existing JavaScript engine is used or embedded). JS-01 provides the front end:
UTF-16 source text, the lexical grammar, and a parser from scripts and modules
to an abstract syntax tree with source spans. JS-02 adds the runtime data
model (see [Runtime data model](#runtime-data-model-js-02)) and JS-03 its
garbage collector (see [Garbage collection](#garbage-collection-js-03)).
JS-04 compiles scripts to bytecode and runs them (see
[Bytecode and interpreter](#bytecode-and-interpreter-js-04)); generators,
async functions, modules and regular expressions do not run yet.

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

This is parse-phase evidence only and must not be advertised as ECMAScript
support. What runs is measured by the execution gate below.

### Test262 execution (JS-07)

`rustic_js::test262::execute` runs a file as Test262's `INTERPRETING.md`
specifies: a fresh agent for each mode, the harness files `assert.js` and
`sta.js` plus the file's `includes`, then the test, as a sloppy script and
again with `"use strict";` in front (`onlyStrict`, `noStrict` and `raw`
honored). A `negative` file must fail in the phase it names with the error it
names. Every run has a 64 MiB heap and a budget of 5 million safe points
(loop iterations and calls), so a runaway test fails identically on every
machine and no wall clock is involved. A panic inside the engine is caught
and counted as a failure.

```sh
RUSTIC_TEST262_DIR=/path/to/test262 \
  cargo test --release -p rustic-js --test test262 -- --ignored
cargo run --release -p rustic-js --example test262 -- /path/to/test262 \
  test/language --list results.tsv
```

The example prints pass, fail and skip counts per directory, and `--list`
writes the verdict and reason for every file. At the pinned revision:

| | `test/language` | `test/built-ins` (JS-05 areas) |
| --- | ---: | ---: |
| Pass | 12,450 | 10,502 |
| Fail | 5,323 | 1,351 |
| Skipped (modules, async, proposals) | 6,059 | 102 |
| Pass rate of the files that run | 70.0% | 88.6% |

The `test/language` files that pass were 7,708 on the first run, 7,895 after
four interpreter bugs it found were fixed, and 10,726 after JS-05 part 1.
The four bugs are covered by `test262_findings` in `tests/interp.rs`:

- A `var` inside a block at the top of a script was never declared as a
  global.
- Assignments inside `with` looked up their target after computing the
  value.
- `x = (function () {})` left the function unnamed.
- Closures in parameter defaults could see the body's `var`s. The body now
  gets its own scope when the parameters contain expressions, as §10.2.11
  requires.

The built-ins gate runs the `test/built-ins` directories of the library areas
installed so far (`BUILT_IN_DIRS` in `tests/test262.rs`: Array, Boolean,
Date, Error, FinalizationRegistry, Function, Iterator, JSON, Map, Math, NativeErrors, Number,
Object, Reflect, Set, String, Symbol, WeakMap, WeakRef, WeakSet, the
iterator prototypes and the global functions). A sparse checkout must include them:
`git sparse-checkout add test/built-ins/Array ...`.

Both runs take about 40 seconds on one core. The files that pass are listed
in `tests/fixtures/test262_language_passing.txt` and
`tests/fixtures/test262_built_ins_passing.txt`. The gate compares exactly: a
file that stops passing fails the gate, and so does one that starts passing
until the lists are regenerated with `RUSTIC_TEST262_BLESS=1`. Run the gate in
release mode on a 64-bit host; the lists are not checked on other targets.

Most `test/language` failures are not interpreter bugs yet:

- About 3,700 files need generators or async functions (JS-08).
- About 780 need `eval` (TD-106).
- About 240 need regular expressions.
- Modules, BigInt arithmetic, `Proxy` and typed arrays account for most of
  the rest.

In the built-ins, the failures are mostly tests that are written with
generators, test proposals not implemented yet (of the 361 `Iterator`
failures, about 180 and 170), or reach for `RegExp`, `Proxy`, Temporal, typed arrays or `$262`. Skipped files are never counted as
passing. Module and async files join with JS-06 (TD-105).

## Fuzzing

The `js-parse` target of `rustic-fuzz` parses every input as a script, a
module and a strict script (the `js-heap` target is described under
[Garbage collection](#garbage-collection-js-03) and `js-run` under
[Bytecode and interpreter](#bytecode-and-interpreter-js-04)). It checks that nothing panics or overflows the
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

- **Weak objects.** `Agent::new_weak_ref`, `new_weak_map`, `new_weak_set` and
  `new_finalization_registry` create the internal slots of the weak built-ins
  (their constructors and methods are JS-05). A weak target does not keep its
  value alive and reads `undefined` once it is collected. A weak-map entry is
  an ephemeron: its value is marked only once its key is, repeated to a
  fixpoint, so a key reachable only from its own value is collected. A
  registry keeps held values alive, and `registry_take_finalized` returns
  those whose targets died, for the host to run cleanup jobs.
- **Host objects.** `Agent::new_host_object(proto, data)` makes an object the
  embedder knows by a `u64`. `collect_garbage_with` takes a `HostTracer`,
  which adds roots, reports the values each reachable host object keeps
  alive, and is told which host objects were collected. `rustic-bindings`
  uses this for DOM node wrappers ([DOM.md](DOM.md#garbage-collection-doc-09-js-03)).
- **Stress mode.** `Heap::set_gc_stress(true)` makes `should_collect` answer
  yes at every safe point.

`GcStats` reports bytes before and after and the cells freed per kind.
`tests/gc.rs` covers each root and reference kind, cycles, 200,000-deep
chains, shape and atom reclamation, index reuse, exact budget return, weak
references, ephemeron chains, registries and host tracing. The `js-heap` fuzz
target checks random allocate/mutate/link/root/collect sequences against a
shadow model. Collection is not incremental (TD-95) and native functions
cannot collect mid-run (TD-94).

## Bytecode and interpreter (JS-04)

`Agent::evaluate(source)` parses, compiles and runs a script and returns its
completion value or an `EvalError` (`Parse`, `Compile`, `Throw`,
`OutOfMemory`, `Cancelled`, `SourceTooLong`). `Agent::compile_script` and
`Agent::run_script` do the two halves separately, and `Agent::call_from_host`
calls a function value. The design is recorded in
[ADR 0017](adr/0017-js-bytecode-interpreter.md).

- **Compiler** (`vm/scope.rs`, `vm/compile.rs`). A scope pass resolves every
  identifier before code is emitted: a binding that no inner function captures
  is a register, and a captured one is a numbered slot of a heap environment.
  Only globals and names inside `with` are looked up by name. Code is
  register bytecode with 12-byte instructions, stored once per function in a
  collected arena.
- **Supported syntax.** `var`/`let`/`const` with temporal dead zones, all
  operators including `**`, `??`, `?.` on members and logical assignment,
  destructuring with defaults and rests, template literals, object literals
  with getters, setters, methods, computed keys, spread and `__proto__`,
  arrays with holes and spread, functions with defaults and rest parameters,
  arrow functions, closures, `new` and `new.target`, `this` rules for strict
  and sloppy code, `arguments`, `if`/`switch`/loops/labels, `for-in`,
  `for-of` with iterator closing, `try`/`catch`/`finally`, `with`, `delete`,
  `typeof`, `in` and `instanceof`.
- **Classes.** Declarations and expressions, `extends` (including
  `extends null` and plain functions), constructors with `super()`, the
  default constructors, `super.x` reads, writes and calls, methods, getters,
  setters, statics, computed keys, instance and static fields, and static
  blocks. A class is a scope holding its own name and the class itself; in a
  derived constructor `this` is a binding in its temporal dead zone until
  `super()` returns, shared with the arrow functions inside, so the existing
  TDZ check gives the required `ReferenceError`. Instance fields are one
  function per class, run when `this` is bound. `new.target` works in arrow
  functions the same way.
- **Private names.** `#x` fields, methods, getters and setters, instance
  and static, and `#x in o`. A private name is a symbol that reflection
  never reports. A private field is an ordinary shape property keyed by that
  symbol, so it costs the same 8 bytes as a public field and reads at the
  same speed (2 million `this.#x = this.#x + 1` calls take 339 ms, the same
  as with `this.x`). Private methods and accessors are not stored per
  object: the functions live once in the class scope, and each instance
  carries one brand (a single private property, added before the fields)
  that every use checks. Static private methods check that the receiver is
  the class itself. The compiler resolves each `#x` to the innermost class
  that declares it, so the kind of access is known when the code is
  compiled.
- **`super` in object literals.** A method, getter or setter of an object
  literal remembers the literal as its home object (a field every closure
  already has), and `super.x` starts at that object's prototype. Arrow
  functions take the home of the function that creates them.
- **Tagged templates and optional calls.** A tag receives the site's
  template object: a frozen array of the cooked strings with a frozen `raw`
  array, built on first use and then kept by the compiled code, so every
  evaluation of one site passes the same object and allocates nothing.
  `f?.()`, `o.m?.()` and `(o?.m)()` short-circuit without evaluating their
  arguments and keep the receiver.
- **Not yet compiled** (rejected with `CompileError::Unsupported` before
  anything runs, TD-97): generators, async functions, modules, regular
  expression literals and BigInt literals. Built-in constructors
  cannot be extended until they exist (JS-05). BigInt arithmetic throws a `TypeError`
  (TD-92).
- **Calls** never recurse in Rust between script functions: frames and
  registers live on vectors in the agent. `VmLimits` bounds frames (10,000),
  stack values (2²⁰, 8 bytes each) and script nested through native code
  (96 levels and 1 MiB of Rust stack). Exceeding a limit throws a `RangeError`
  that script can catch.
- **Safe points** are loop back edges and calls. They poll the
  `CancellationToken` set with `Agent::set_cancellation`, count down
  `Agent::set_step_budget` (a deterministic limit for tests and fuzzing), and
  collect garbage when the heap asks. Cancellation and out-of-memory cannot
  be caught by script.
- **Inline caches.** Each `o.name` and global site remembers one shape: an
  own slot, a slot on a prototype, or a property add with its shape
  transition. Any change to an object used as a prototype, any collection and
  any new global lexical binding start a new epoch that invalidates all
  entries, so hits need no chain walk. `Agent::set_inline_caches(false)`
  turns them off for differential tests.
- **Iteration.** `for-of` over an array, while the array iterator's `next` is
  untouched, reads elements directly and allocates no result objects. The
  built-ins installed so far are only the ones the language needs: array and
  string iterators. Everything else is JS-05.

Memory per value on a 64-bit build: an object with two properties is 88
bytes; a closure is 120 bytes, because a function's `prototype` object is
created only when something first reads it; and the scope environment of a
closure that captures one variable is 64 bytes (56 plus 8 per variable),
since environments share their name lists with the compiled code.
`closures_and_their_scopes_stay_small` holds these bounds. Compiled code is
shared by all closures of a function.

Time on the development machine (x86-64, release build, best of five,
`cargo run --release -p rustic-js --example js_run -- file.js`). These are
first measurements, not a gate; JS-09 adds the benchmark corpus.

| Script | Time |
| --- | --- |
| `fib(30)` (1.3 million calls) | 175 ms |
| 10 million iterations of integer arithmetic in a function | 249 ms |
| The same loop on global variables | 551 ms |
| 2 million `new P(i, i + 1)` with two property reads each | 335 ms |
| 5 million array elements summed with `for-of` after 1 million indexed writes | 151 ms |
| 3 million calls of a counter closure | 240 ms |

The `js-run` fuzz target runs each input three ways (caches on, caches off,
and collecting at every safe point) with a 2 MiB heap and a step budget, and
requires the same outcome each time and a usable agent afterwards. Its first
runs found a loop that could not be cancelled and a function that ran off the
end of its code; both are fixed and kept as seeds or regressions.

## Built-ins (JS-05)

JS-05 installs the standard library in parts. Each part is one module under
`crates/rustic-js/src/runtime/builtins/`. A function is a native function
object: a Rust `fn(&mut Agent, this, args)`, its interned name and its
`length`, about 115 bytes. Tables of `method(name, fn, length)` entries keep
installation to a loop, and `install_constructor` links a constructor to its
prototype.

Part 1 holds what other built-ins and the Test262 harness lean on:

- **Error.** `Error` and the native errors (`TypeError`, `RangeError`,
  `ReferenceError`, `SyntaxError`, `EvalError`, `URIError`), with `cause`.
  The native error constructors inherit from `Error`.
- **Object.** The constructor and its 22 static methods (`keys`, `entries`,
  `assign`, `defineProperty`, `freeze`, `getOwnPropertyDescriptors`,
  `fromEntries` and the others; `groupBy` waits for iterators). `Object.prototype` gets `hasOwnProperty`,
  `isPrototypeOf`, `propertyIsEnumerable`, `toLocaleString` and the
  `__proto__` accessor.
- **Function.** `call`, `apply`, `bind` and `[Symbol.hasInstance]` on
  `Function.prototype`, and its `toString`. Source text is not kept, so
  `toString` uses the `NativeFunction` form for every function.
  - A bound function is a native function that holds its target, its `this`
    and its arguments, and it costs 8 bytes per bound argument.
  - The `Function` constructor parses its parameters and body as one
    function expression in the global scope. Text that tries to close the
    function early parses as something else and is a SyntaxError.
- **Globals.** `isFinite`, `isNaN`, `parseFloat`, `parseInt` (any radix),
  and the `Boolean`, `Number`, `String` and `Symbol` constructors.

Part 2 adds the rest of the everyday library, beginning with:

- **Array.** The constructor with `from`, `of`, `isArray` and
  `[Symbol.species]`, and every `Array.prototype` method of ES2025
  (`toSorted`, `with` and the other copying methods included), plus
  `[Symbol.unscopables]`.
  - The methods follow the specification through the object internal
    methods, so they work on array-likes, holes, accessors and subclasses.
  - `sort` is a stable merge sort. With no comparator, each element's string
    is computed once.
  - `flat` keeps its own stack, so deep nesting cannot overflow the native
    stack.
  - `join` treats an array that is already being joined as empty, as
    browsers do, so a cycle ends.
- **Iterators.** `keys` and `entries` iterators, and `%IteratorPrototype%`
  with `[Symbol.iterator]`.
- **Math.** Every function and constant. `Math.random` is xorshift64*,
  seeded from the clock, and `Agent::set_random_seed` makes it repeatable
  for tests and fuzzing.
- **Symbol.** The well-known symbols as properties of `Symbol`, `Symbol.for`
  and `Symbol.keyFor` with a registry object, and `description`.
- **Number.** The constants, `isFinite`/`isInteger`/`isNaN`/`isSafeInteger`
  and `toString(radix)`.
  - `toFixed`, `toExponential` and `toPrecision` round the exact binary value
    with ties away from zero, as the specification requires. They work from
    the number's exact decimal expansion, because Rust's formatting rounds
    ties to even: `(2.5).toFixed(0)` is `"3"`.
  - Radix strings print as many digits as tell the number apart from its
    neighbors, as browsers do.
- **String.** `fromCharCode`, `fromCodePoint`, `raw` and every
  `String.prototype` method on UTF-16 code units, including `at`, `padStart`,
  `replaceAll`, `isWellFormed`, `toWellFormed`, `substr` and the
  `trimLeft`/`trimRight` aliases.
  - Case mapping uses Rust's Unicode tables over each well-formed stretch, so
    `ß` uppercases to `SS` and a final sigma lowercases to `ς`. Lone
    surrogates are kept.
  - `match`, `matchAll`, `replace`, `replaceAll`, `search` and `split` call an
    object argument's `Symbol.match`/`replace`/`search`/`split` method.
  - A string pattern for `match`, `matchAll` or `search` needs RegExp
    (JS-08) and throws a TypeError until then.
  - `normalize` is missing (TD-107).
- **JSON.** `parse` with a reviver and `stringify` with replacer functions,
  property lists, indentation, `toJSON` and cycle detection.
  - Parsing and the reviver walk keep their own stacks, so 50,000 nested
    arrays parse.
  - `stringify` recurses and throws a RangeError when the run's native stack
    budget is spent.
- **Map and Set.** Every method, the ES2025 set methods (`union`,
  `intersection`, `difference`, `symmetricDifference`, `isSubsetOf`,
  `isSupersetOf`, `isDisjointFrom`), `Map.groupBy`, `Object.groupBy`, and the
  `getOrInsert`/`getOrInsertComputed` methods of the upsert proposal.
  - Entries live in one insertion-ordered vector with a hash index on
    SameValueZero (`runtime/table.rs`), about 40 bytes each.
  - A deleted entry leaves a hole, so an open iterator keeps its place and
    sees entries added after it.
  - Holes are squeezed out once they outnumber the live entries and no
    iterator or `forEach` holds a position. The collector recounts the open
    iterators of every table, so an iterator abandoned half-way stops holding
    its table back after the next collection.
- **Weak collections.** `WeakMap`, `WeakSet`, `WeakRef` and
  `FinalizationRegistry` on the JS-03 ephemeron tables. A registry records
  registrations and collects held values when targets die. Its cleanup
  callback waits for host jobs (JS-06).
- **Reflect.** All thirteen functions.
- **Date.** The constructor, `Date.parse`, `Date.UTC`, `Date.now` and every
  `Date.prototype` method, with Annex B `getYear`, `setYear` and
  `toGMTString` (`builtins/date.rs`).
  - The time value sits in the object's primitive slot. Calendar fields come
    from integer civil-date arithmetic on the day number, so a getter is a
    few integer divisions and needs no tables.
  - `Date.parse` reads the Date Time String Format (expanded years included)
    and the forms `toString` and `toUTCString` write, so every string a Date
    produces parses back to the same time.
  - Local time is UTC plus one fixed offset, `Agent::set_time_zone_offset`.
    There is no time zone database and so no daylight saving time (TD-108).
    `Agent::set_clock` fixes the current time for tests and fuzzing.
  - The `toLocale…String` methods return the `toString` forms; Intl is
    JS-08.
  - Test262 `test/built-ins/Date`: 595 of 610 files pass. The rest need
    regular expressions, other realms (`$262`) or Temporal.
- **Iterator helpers.** The `Iterator` constructor (abstract, for
  subclasses), `Iterator.from`, `Iterator.concat`, and on
  `Iterator.prototype`: `map`, `filter`, `take`, `drop`, `flatMap`,
  `reduce`, `toArray`, `forEach`, `some`, `every`, `find`, with the
  `constructor` and `@@toStringTag` accessors (`builtins/iterator.rs`).
  - The specification writes each lazy helper as a generator. Here a helper
    keeps the generator's variables in one boxed struct (about 80 bytes),
    and `next` runs one step as a plain function, so a helper needs no frame
    or stack of its own. A re-entrant `next` sees the running state and
    throws, as a running generator does.
  - `return` closes the inner iterator of `flatMap` first, then the
    underlying one; a throwing callback closes the underlying iterator.
  - Test262 `test/built-ins/Iterator`: every file that runs without
    generators passes (317 of 678). Of the rest, about 180 are written with
    generators (JS-08), about 170 test proposals not implemented yet (`zip`,
    `zipKeyed`, `chunks`, `windows`, `includes`, `join`), and a few need
    `$262` or `RegExp`.

Time on the development machine (release): 1 million `push` calls take 300
ms including the global loop around them (about 120 ns per call). A `map` over
1 million elements takes 40 ms beyond filling the array. Sorting 200,000
numbers takes 190 ms with a comparator and 145 ms with the default string
order. The methods take the generic path through the internal methods. A
dense-array fast path for `push`, `map` and `sort` is JS-09 work.

A native function can be a constructor, marked by `FLAG_NATIVE_CONSTRUCTOR`.
`new` calls it with a new target, which the function reads with
`Agent::new_target()`. Native constructors work as `extends` bases and
`super()` targets, and `Agent::construct_with` passes a new target through.

A new agent now holds 83 objects in 17,390 bytes, up from 32 objects in
6,477 bytes before JS-05. Most of the growth is native function objects.
Installing the rest of the library lazily is the JS-09 lever if agents per
page start to matter.

With part 1, the Test262 `test/language` pass rate rose from 44.4% to 60.4%,
and with the part 2 pieces above to 70.0%.
