# JavaScript engine (JS-01 to JS-05)

`rustic-js` is Rustic's own ECMAScript implementation (M4, ADR 0001: no
existing JavaScript engine is used or embedded). JS-01 provides the front end:
UTF-16 source text, the lexical grammar, and a parser from scripts and modules
to an abstract syntax tree with source spans. JS-02 adds the runtime data
model (see [Runtime data model](#runtime-data-model-js-02)) and JS-03 its
garbage collector (see [Garbage collection](#garbage-collection-js-03)).
JS-04 compiles scripts to bytecode and runs them (see
[Bytecode and interpreter](#bytecode-and-interpreter-js-04)), including
generators (JS-08 part 1), promises with a job queue and async functions
(JS-06 part 1), async generators with `for await` (JS-08 part 2) and
regular expressions (JS-08 part 3), BigInt (JS-08 part 4), proxies with
mapped `arguments` objects (JS-08 part 5) and typed arrays (JS-08 part 6);
modules do not run yet.

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

At that revision all 23,123 files pass. The 167 expected failures there
were RegExp pattern early errors until the pattern parser paid TD-88, so
the list is empty. 709 files using proposal
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

| | `test/language` | `test/built-ins` (gated areas) |
| --- | ---: | ---: |
| Pass | 21,939 | 17,291 |
| Fail | 488 | 1,283 |
| Skipped (modules, proposals) | 1,405 | 111 |
| Pass rate of the files that run | 97.8% | 93.1% |

Async tests run since promises landed: after the script the runner drains
the job queue, and the test passes when `$DONE` printed
`Test262:AsyncTestComplete`. That moved about 4,650 `test/language` files
from skipped to run, most of which needed async generators or `for await`
(about 4,350); before it, 16,150 of 17,773 run files passed (90.9%), and
16,723 of 22,427 after it until async generators landed, and 20,966
until RegExp landed.

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
installed so far (`BUILT_IN_DIRS` in `tests/test262.rs`: Array, ArrayBuffer, Atomics, Boolean, DataView,
Date, Error, FinalizationRegistry, Function, GeneratorFunction,
GeneratorPrototype, Promise, AggregateError, AsyncFunction,
AsyncGeneratorFunction, AsyncGeneratorPrototype, the async iterator
prototypes, BigInt, Iterator, JSON, Map, Math, NativeErrors, Number,
Object, Proxy, Reflect, RegExp, Set, SharedArrayBuffer, String, Symbol, TypedArray, TypedArrayConstructors, WeakMap, WeakRef, WeakSet, the
iterator prototypes and the global functions). A sparse checkout must include them:
`git sparse-checkout add test/built-ins/Array ...`.

Both runs take about 40 seconds on one core. The files that pass are listed
in `tests/fixtures/test262_language_passing.txt` and
`tests/fixtures/test262_built_ins_passing.txt`. The gate compares exactly: a
file that stops passing fails the gate, and so does one that starts passing
until the lists are regenerated with `RUSTIC_TEST262_BLESS=1`. Run the gate in
release mode on a 64-bit host; the lists are not checked on other targets.

Most `test/language` failures are not interpreter bugs yet:

- About 150 `eval-code` files still fail: most declare `arguments` from an
  `eval` in parameter defaults, or are async.
- Modules account for most of the rest. The real assertion failures left are mostly parameter TDZ in
  defaults (`dflt-params-ref-later`) and function names from computed symbol
  keys.

In the built-ins, the failures are mostly proposals not implemented yet
(about 170 of the 217 `Iterator` failures), Unicode property escapes
(about 570 of the 655 `RegExp` failures, TD-110), or tests that reach for
Temporal or `$262.agent` (112 `Atomics` files). Skipped files are never
counted as passing. The script gate continues to report module files as
skipped. Their execution classifications are recorded by the separate module
gate below.

`$262.evalScript` runs UTF-16 script source in the current global environment
and preserves completion values and thrown objects. Its global declaration
tests cover lexical collisions, non-configurable properties, function
replacement and atomic rejection before any bindings are installed (§16.1.7).
Realm creation, explicit collection and test agents remain in TD-105.

## Fuzzing

The `js-parse` target of `rustic-fuzz` parses every input as a script, a
module and a strict script (the `js-heap` target is described under
[Garbage collection](#garbage-collection-js-03) and `js-run` under
[Bytecode and interpreter](#bytecode-and-interpreter-js-04)). It checks that nothing panics or overflows the
stack, that syntax error spans lie inside the source, that a successful parse
dumps deterministically, and that anything valid in strict mode is valid in
sloppy mode. Seeds are one snippet per line of
`crates/rustic-fuzz/corpus/js-parse/`.

The `js-regexp` target reads a flags byte, a pattern and a subject. It
checks that `Regex::validate` and `Regex::new` agree, that every capture lies
inside the subject, and that the subject matches the same way stored as
Latin-1 and as UTF-16; a poll budget cancels exponential patterns.

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

An `Agent` owns one `Heap` and the intrinsic objects of its
[realms](#realms-js-06). Every
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
  including exact BigInt/Number comparison, and `ToBigInt`, `ToBigInt64`,
  `ToBigUint64` and `ToIndex` (JS-08 part 4).
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
- **Generators** (JS-08 part 1, §27.5): `function*` declarations and
  expressions, generator methods in objects and classes, `yield`, `yield*`
  and `%GeneratorFunction%`.
  - A call runs the parameter and declaration instantiation, then
    `GeneratorStart` moves the frame into the generator object and returns
    it. The object keeps the frame's registers in one boxed slice, with the
    resume point, environment, `this` and function: about 100 bytes plus 8
    per register, and nothing else. No Rust stack and no separate value
    stack are kept per generator.
  - `next`, `throw` and `return` copy the registers back onto the agent's
    value stack and run the frame in a nested run until `yield` copies them
    out again. A resumption writes the sent value and the resume mode to
    the `yield`'s registers; compiled code after each `yield` then goes on,
    throws, or returns through any `finally` blocks with the ordinary exit
    path, so `return()` runs `finally` code that may itself yield.
  - `yield*` is a loop in bytecode around one `Delegate` op, which calls the
    inner `next`, `throw` or `return` and checks the result; inner results
    are passed out unchanged. A missing `throw` closes the inner iterator
    and throws a TypeError, as §15.5.5 requires.
  - A generator function's `prototype` (inheriting `%GeneratorPrototype%`,
    without `constructor`) is created on first use, as for other functions
    (TD-98). Generators are not constructors.
  - The collector traces a suspended generator's registers, environment and
    code (`suspended_generators_keep_their_frames_alive`).
  - A generator body runs nested under its `next` call, and native code on
    the Rust stack normally blocks collection. When `next` was called from
    a site that holds nothing unrooted (a bytecode call, an iterator step,
    a `yield*` step or the host) at the collectable level, the body
    collects at its own safe points, so `for (x of gen())` over an
    allocating generator stays within the heap. Called from anywhere else
    (a conversion, a callback of a native), the body does not collect
    (`generator_bodies_collect_at_their_safe_points`).
  - Test262: with generators, `test/language` went from 13,087 to 16,138 of
    17,773 files (90.8%; 16,150 once array destructuring closed its iterator
    on errors, TD-100), and the built-ins gate gained 268 files, mostly in
    `Iterator`; `GeneratorFunction` and `GeneratorPrototype` are gated.
- **Promises and async functions** (§27.2, §27.7): `Promise` with `then`,
  `catch`, `finally` and the statics `resolve`, `reject`, `all`,
  `allSettled`, `any`, `race`, `withResolvers` and `try`;
  `AggregateError`; `async` functions, arrows and methods with `await`,
  and `%AsyncFunction%` (`builtins/promise.rs`).
  - Jobs wait in a queue on the agent. The host runs them with
    `Agent::run_jobs()`, a microtask checkpoint; `js_run` and the Test262
    runner call it after the script. Jobs run at the host level, so their
    script collects at its safe points like any other.
  - A promise is an ordinary object with a boxed record: state, result
    and its reactions (40 bytes each). A promise made by `%Promise%` for
    `then` or an async function has no resolving function objects, since
    nothing can reach them; a job settles it directly. They are created
    only where script sees them: the executor of `new Promise`, a
    thenable's `then`, `withResolvers`, subclass constructors and the
    combinators' element functions (built-in closures whose few captured
    values sit in one boxed slice).
  - An async function starts like any call; its first op creates the
    promise and a state object. `await` attaches a reaction that refers
    to that state object (no closures), saves the registers as a
    generator does, and returns the promise to the caller. The job
    resumes the frame with the value or throws it. A catch-all handler
    rejects the promise on a throw, and `return` resolves it.
  - `HostPromiseRejectionTracker` (§27.2.1.9): a promise rejected without
    a handler, and a handler added to one reported earlier, are recorded as
    events that the host takes with `Agent::take_rejection_events()` after
    `run_jobs()`. A rejection handled before the host asks is withdrawn, so
    `Promise.reject(1).catch(f)` reports nothing. The list keeps its
    promises alive until it is taken; `Agent::promise_result` gives the
    reason.
- **Async generators and `for await`** (§27.6, §14.7.5): `async function*`
  declarations, expressions and methods, `yield*` over async and sync
  iterables, `for await`, `%AsyncGeneratorFunction%`,
  `%AsyncIteratorPrototype%` and `%AsyncFromSyncIteratorPrototype%`
  (`builtins/async_generator.rs`).
  - An async generator is a generator object whose record also holds its
    queue of requests (16 bytes each). `next`, `throw` and `return` queue
    a request with a new promise; if the generator is suspended, the
    driver (`Agent::async_generator_run`) runs the body. Each `yield`
    settles the head request and goes straight on to the next one if
    another is waiting; an `await` suspends the frame like an async
    function's, and its job resumes the driver. The end of the body
    settles the remaining requests, awaiting the value of a `return`.
  - The spec's awaits are compiled as `await` ops: on the operand of
    `yield` and `return`, on a `return` sent into a `yield`, and on each
    result of the inner iterator of `yield*` and `for await`. Closing a
    `for await` early calls `return` and awaits it; after a throw its
    errors are dropped.
  - Test262: `test/language` went from 16,723 to 20,966 of 22,427 run
    files (93.5%), and the four async iteration directories of
    `test/built-ins` pass except two realm tests.
- **Regular expressions** (JS-08 part 3, §22.2): `crates/rustic-js/src/regexp/`
  is the engine and `builtins/regexp.rs` the library.
  - The pattern parser covers the whole ES2025 grammar: Annex B in sloppy
    patterns, named groups (duplicates in separate alternatives), lookbehind,
    `v`-mode class set operations and `\q{}` strings, and modifier groups
    such as `(?i:…)`. The script parser validates every literal with it,
    so an invalid pattern is an early SyntaxError (TD-88 paid). Nesting
    deeper than 256 is a SyntaxError rather than a deep recursion.
  - Patterns compile to a small instruction list run by a backtracking
    matcher with explicit choice and trail stacks, so no pattern recurses
    in Rust. Loops over one character are a single instruction; counted
    loops keep their counters in registers and stop on empty iterations as
    §22.2.2.3.1 requires. Lookbehind compiles the same nodes to match
    backwards.
  - Case-insensitive matching closes each character class under the
    spec's `Canonicalize` once, at compile time, from the standard
    library's case tables: upper case without `u`, and simple case folding
    with `u` or `v`.
  - The matcher reads Latin-1 and UTF-16 strings in place, without copying.
    It polls the step budget and the cancellation token every 16,384
    steps, so a runaway pattern is stopped like a runaway loop; it can still
    take exponential time until then (TD-111). Its stacks are capped and
    overflowing them throws a RangeError.
  - The agent keeps the last 32 compiled patterns, so `new RegExp(s)` in a
    loop compiles once.
  - The library: the constructor with `RegExp.escape`, `exec`, `test`,
    `toString`, `compile`, the flag getters, `flags`, `source`, the five
    `Symbol` methods with `lastIndex` and `d`-flag indices, species, and
    `%RegExpStringIteratorPrototype%`. String `match`, `matchAll`,
    `replace`, `replaceAll`, `search` and `split` go through them.
  - Unicode property escapes (`\p{…}`) parse but do not compile yet
    (TD-110): a literal using one is rejected before the script runs.
  - Test262: `test/built-ins/RegExp` passes 1,224 of 1,879 files; apart
    from property escapes the failures need `$262` realms or a `print`
    global. `test/language` went from 20,966 to 21,220. Four bugs it found
    are covered by `regular_expressions` in `tests/interp.rs` and fixed: a
    negated class under `i` was complemented before its case closure,
    `\w` ignored modifier groups, `RegExp.escape` wrote `\u00a0` for
    `\xa0`, and `Object.prototype.toString` did not know RegExps.
- **BigInt** (JS-08 part 4, §6.1.6.2, §21.2): literals, literal property
  keys, every operator, the `BigInt` function with `asIntN` and `asUintN`,
  and `BigInt.prototype.toString(radix)`, `toLocaleString` and `valueOf`.
  - A BigInt is a sign and 32-bit limbs in its own arena: 32 bytes plus 4
    per limb, charged to the heap budget.
  - Division is Knuth's algorithm D, checked against multiplication over a
    deterministic sweep of operand sizes. The bitwise operators work on the
    two's complement form, so `-1n & 255n` is `255n`. `>>` rounds down.
  - Mixing a BigInt with a Number is a TypeError, as is `>>>` and unary
    `+`; dividing by `0n` and a negative exponent are RangeErrors.
  - A result may have at most 2²⁰ bits; past that the operator throws a
    RangeError before allocating (TD-112).
  - Test262: `test/built-ins/BigInt` passes 76 of 77 files (the other needs
    a second realm), and `test/language` went from 21,220 to 21,313.
- **Proxy** (JS-08 part 5, §10.5, §28.2): `new Proxy`, `Proxy.revocable`
  and all thirteen traps with their invariant checks
  (`builtins/proxy.rs`).
  - Objects that override internal methods carry one flag bit. The ordinary
    internal methods test it and hand such an object to `runtime/exotic.rs`,
    so ordinary objects pay one predictable branch and nothing else.
    `[[GetPrototypeOf]]`, `[[SetPrototypeOf]]`, `[[IsExtensible]]`,
    `[[PreventExtensions]]` and `[[OwnPropertyKeys]]` can now throw.
  - A proxy on a prototype chain intercepts inherited reads and writes, and
    the inline caches never record a lookup that passed one.
  - A proxy for a function is itself a function object, so calls, `new`,
    `extends` and `typeof` need nothing special. `IsArray` looks through
    proxies. Private names never pass through one.
  - Proxies can nest through targets, handlers and prototype chains 128
    deep in one operation; deeper is a RangeError instead of native stack
    growth.
  - `for-in` over a proxy asks its traps for all keys up front (TD-113).
  - Test262: `test/built-ins/Proxy` passes 273 of 310 files and
    `test/built-ins/Reflect` all 153; the 37 Proxy failures need a second
    realm.
- **Arguments objects** (§10.4.4): a sloppy function with a simple
  parameter list gets a mapped arguments object when it names `arguments`
  (or contains a direct `eval`). The compiler then keeps its parameters in
  the function's environment, and the object's mapped indices read and
  write those slots, so `arguments[0] = 1` changes the parameter and back.
  Deleting an index, making it an accessor or making it read-only ends its
  mapping. A function that never names `arguments` is compiled as before,
  with parameters in registers. Strict functions and functions with
  defaults, rests or patterns get the unmapped object, whose `callee`
  throws. `f.caller` and `f.arguments` are `null` for sloppy functions and
  throw for strict ones (TD-114).
- **Buffers and typed arrays** (JS-08 part 6, §25.1, §25.3, §23.2,
  §10.4.5): `ArrayBuffer` (resizable buffers, `slice`, `resize`, `transfer`,
  `transferToFixedLength`), `DataView` with every accessor, the eleven
  typed array constructors plus `Float16Array`, `%TypedArray%` with `from`
  and `of`, and all the prototype methods; `Math.f16round`.
  - A buffer is one byte vector charged to the heap budget, at most
    2³¹ − 1 bytes. A request the budget cannot hold is a catchable
    RangeError, and a buffer that becomes garbage returns its bytes at the
    next collection (64 MiB of 1 MiB arrays pass through a 4 MiB heap).
  - A typed array is 72 bytes plus a small boxed record (buffer, kind,
    offset, length) and stores nothing itself. Elements are little-endian
    on every target, so results do not depend on the host's byte order.
  - Indexed access follows the integer-indexed exotic rules: canonical
    numeric strings such as `"-0"` and `"1.5"` never reach the prototype
    chain, out-of-range writes do nothing, and a length-tracking view
    follows its resizable buffer. A detached or out-of-bounds array reads
    as length 0 and its methods throw.
  - Binary16 conversion rounds to nearest even and is checked against all
    65,536 bit patterns.
  - `Agent::new_array_buffer`, `array_buffer_bytes` and
    `detach_array_buffer` are the host's side.
  - Not yet: a fast path for element access (TD-115).
  - Test262: `ArrayBuffer` 193 of 221, `DataView` 541 of 560, `TypedArray`
    1,445 of 1,453 and `TypedArrayConstructors` 712 of 738; nearly all the
    failures need a second realm or the immutable buffer proposal. Six `DataView` files expect a length of 0 from a
    detached or out-of-bounds view, where the specification (§25.3.4.2)
    and this engine throw a TypeError.
- **SharedArrayBuffer and Atomics** (JS-08 part 7):
  - `SharedArrayBuffer` (§25.2) with `byteLength`, `slice`, growable
    buffers (`maxByteLength`, `growable`, `grow`) and species. It is the
    buffer record of `ArrayBuffer` with a `shared` flag: it cannot be
    detached, it only grows, and each kind rejects the other's methods.
    Typed arrays and `DataView` take either.
  - `Atomics` (§25.4): `load`, `store`, `add`, `sub`, `and`, `or`, `xor`,
    `exchange`, `compareExchange`, `isLockFree`, `pause`, `wait`,
    `waitAsync` and `notify`, on integer typed arrays over either kind of
    buffer. Conversions run before the access is revalidated, arithmetic
    wraps in the element type, and `compareExchange` compares raw
    elements.
  - An agent runs on one thread and no other agent can reach its memory
    yet (workers are M9), so the operations are atomic by construction and
    a shared buffer is shared between this agent's views only (TD-115).
    `Atomics.wait` therefore cannot be woken. By default it throws a
    TypeError, as on a browser's main thread; after
    `Agent::set_can_block(true)` a finite wait returns `"timed-out"`
    (moving a fixed clock forward) and an infinite one is cancelled.
    `Atomics.waitAsync` promises are woken by `Atomics.notify` in the same
    agent, oldest first, or time out when the host calls
    `Agent::expire_waiters` (`next_waiter_deadline` says when); pending
    waiters are collector roots.
  - A host decides whether script sees `SharedArrayBuffer` at all: the web
    requires cross-origin isolation (M6/M7 policy), which the bindings
    will enforce by removing the global.
  - Test262: `SharedArrayBuffer` 103 of 104 (the other needs a realm) and
    `Atomics` 268 of 389; 112 of the failures need `$262.agent` (other
    agents sharing the buffer) and 9 the immutable buffer proposal. The
    runner honors `CanBlockIsFalse` and expires `waitAsync` timeouts after
    the job queue drains.
- **Not yet compiled** (rejected with `CompileError::Unsupported` before
  anything runs, TD-97): modules and regular expression
  literals that use a property escape (TD-110).
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
first measurements from before JS-09, kept as history; the global loop is
about six times faster since (see the JS-09 section below).

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

## Realms (JS-06)

An agent starts with one realm and can hold more (§9.3). Each realm has its
own global object, global environment and set of intrinsics; the well-known
symbols and the `Symbol.for` registry belong to the agent and are shared.

```rust
let realm = agent.create_realm()?;          // the running realm stays
let global = agent.realm_global(realm);     // hand it to script as a value
let previous = agent.enter_realm(realm);    // host work in that realm
agent.evaluate("var x = 1")?;
agent.enter_realm(previous);
```

- Every object records the realm it was created in, in two bytes that were
  padding, so `Object` stays at 72 bytes. For a function that is its
  `[[Realm]]`.
- `Agent::intrinsics` and the global environment are the running realm's.
  Calling a function of another realm swaps them in and the return swaps
  them back, so a call within one realm pays one 16-bit comparison. A
  frame records its realm; generators and async functions resume in theirs.
- Bound functions and proxies run no code of their own and stay in the
  caller's realm, so a revoked proxy from another realm throws the caller's
  `TypeError`. A derived constructor checks its result after its own context
  is gone (§10.2.2), so those two errors are the caller's as well.
- `GetPrototypeFromConstructor` falls back to the intrinsic of the realm of
  `newTarget` (`GetFunctionRealm` looks through bound functions and
  proxies). The objects a realm is created with take consecutive indices in
  the same order in every realm, so the counterpart of an intrinsic is found
  by offset, with no table per realm. `ArraySpeciesCreate` ignores another
  realm's `%Array%`.
- A lazily created function `prototype` inherits from the intrinsics of the
  function's realm, whichever realm reads it first.
- A realm costs about what the first one does (119 KB of heap budget) and
  lives as long as its agent (TD-118). Creating one that does not fit leaves
  the agent unchanged.

The Test262 host object gains `$262.createRealm()`, which returns the new
realm's own `$262`. With it, 189 more files pass: 19 in `test/language`,
168 in the installed `test/built-ins` areas (which also gain
`Error.isError`) and 2 in the Intl selection. `test/language` passes 21,939
of 22,427 files run (97.8%) and the built-ins 17,290 of 18,574. The
cross-realm files still failing use `ShadowRealm` or the `Error.prototype.stack`
accessor, which are proposals.

## Static module loading (JS-06)

[`ModuleRecord`](../crates/rustic-js/src/module.rs) derives the static
linker inputs from the Module grammar. Requests preserve exact UTF-16
specifiers and treat attribute order as immaterial. The record retains the
first occurrence of each request, local import names, explicit exports and
star re-exports. Re-exported imports resolve to their defining binding or
namespace.

[`ModuleGraph`](../crates/rustic-js/src/module/graph.rs) calls the host to
resolve each specifier against the importing module's opaque key and to
supply source for each unique resolved key. The host owns any I/O, module
identity and import attribute policy. A graph is returned only after all
static requests have been loaded and named imports and re-exports validated.

Export discovery and resolution follow [ECMA-262 §16.2.1.7][modules] with
iterative traversal. A cycle terminates, two routes to the same binding
agree, and different bindings under the same star-export name are ambiguous.
Bare star exports do not forward the default export. The API checks module
count, total UTF-16 source size, resolution work and cancellation. The
source callback supplies an already constructed source buffer.

`Agent::evaluate_module_graph` compiles and links a fresh synchronous graph
instance and returns the entry module's namespace. Linking connects import
slots directly to the defining bindings. Every environment is created before
execution, and function declarations and `var` bindings are initialized before
any body runs. The VM then executes each dependency once in depth-first request
order, including cyclic graphs. Lexical bindings stay in their temporal dead
zone until their declarations run.

Namespaces have null prototypes and expose UTF-16-sorted export names through
live data descriptors. Ambiguous star exports are omitted. Import slots and
namespace properties trace their targets through garbage collection, and the
host may keep a namespace alive with `Agent::root`. See [ADR 0019](adr/0019-static-module-evaluation.md).

A module shares its null-prototype `import.meta` object with nested functions.
By default, the evaluator creates the empty object on first access.
`Agent::evaluate_module_graph_with_metadata` loads host-provided UTF-16 string
properties during instantiation for modules that reference `import.meta`.
The host receives each module's opaque key. Properties use the graph work
budget and the agent heap budget.

`Agent::start_module_evaluation` returns the entry namespace and a completion
promise. Top-level `await` and `for await` use the existing promise jobs and
suspended frames. An asynchronous dependency delays its importers while
independent siblings can run immediately. The host drives microtask checkpoints
and roots the returned handles across collection.

`import()` converts the specifier and options in the calling VM, then queues
the request for the explicit host loader. It resolves to the namespace only
after evaluation completes. The Test262 host caches evaluated and in-flight
modules by canonical fixture path and type. It reuses their live bindings
and completion promises across overlapping graphs.

The Test262 module runner selects files flagged `module`, reads relative
JavaScript, JSON, text and byte fixtures within the checkout, and distinguishes parse, resolution
and runtime failures. It uses the same harness, heap and step budgets as
the script runner. At revision `48221d7c1f7745dc13e420a2e850a248c0b2afb4`,
817 module files were selected. Of the 696 executed, all 696 passed. The remaining 121 were skipped for proposal features. All verdicts are pinned in
`tests/fixtures/test262_modules.txt`.

`ModuleGraph::load_sources` accepts JSON, text and bytes as synthetic modules with a
single default export. JSON uses the engine's JSON parser and invalid input
fails before any module body executes. Text becomes an exact UTF-16 string. Byte imports produce a Uint8Array
backed by an immutable ArrayBuffer.
Every import of the same resolved key shares the same binding and value.

```sh
cargo test --locked -p rustic-js --test module
RUSTIC_TEST262_DIR=/absolute/path/to/test262 \
  cargo test --locked --release -p rustic-js --test test262 test262_module_execution -- --ignored
cargo run --locked --release -p rustic-js --example test262 -- /path/to/test262 \
  test/language --modules --list module-verdicts.tsv
```

To update the fixture after a verified change, strip the `test/language/`
prefix and retain the first word of each verdict. Keep the counts, revision
and tracked limitations current.

[modules]: https://262.ecma-international.org/17.0/index.html#sec-source-text-module-records

## Limits and performance gate (JS-09)

`tests/limits.rs` is the gate. It holds the limits, the string rope
contract, the benchmark corpus and the heap-size report.

- **Limits.** Deep recursion throws a `RangeError` that script can catch, at
  exactly `VmLimits::max_frames`, and the stack is usable afterwards; the
  register stack is bounded the same way. A request that could never fit
  (`'x'.repeat(1e9)`, `new ArrayBuffer(1e9)`) is a catchable `RangeError`.
  Running out of the heap budget ends the run with
  `EvalError::OutOfMemory`, which script cannot catch: a `catch` block that
  needs memory to run cannot be relied on, so the host gets the error and
  the agent runs again once the garbage is collected. Endless loops,
  runaway recursion, generators and regular expressions stop on the
  cancellation token and on the step budget (`Agent::step_budget` reads what
  is left).
- **Ropes.** `Heap::concat` makes a rope when the result has 256 code units
  or more: the two halves are recorded and nothing is copied until
  something reads the units (`Heap::string`), so appending in a loop is
  linear (TD-101 paid). `Heap::string_len` answers `length` without a copy.
  A collection flattens every rope the program still holds, if the copy
  fits half of the free budget, and frees the chain behind it; a rope too
  large to copy (a string doubled onto itself) stays a rope. The copy is
  made behind `&self`, so it is charged late: the bytes count in
  `Heap::bytes_used` at once, a rope is refused when it is made unless its
  copy fits, and the next allocation or safe point fails if the heap is
  over its limit (TD-116).
- **Benchmark corpus.** `benchmarks/*.js`, eight original workloads, each a
  `run(n)` that returns a checksum: property access at one and at four
  shapes, calls and recursion, closure creation, string building, array
  push/iterate/sort, allocation churn, a JSON-like tree and a DOM-free
  "framework diff" loop. The tests pin each checksum (an independent model
  of the arithmetic workloads gives the same numbers), require the same
  result and the same number of safe points with inline caches off and
  with a collection at every safe point, and require that doubling `n`
  less than 2.5-folds both the safe points passed and the bytes allocated
  (`Heap::allocated_bytes`), so no workload is quadratic. Each also fits an
  8 MiB heap.
- **Heap-size report.** `tests/fixtures/heap_report.txt` lists the bytes
  charged for an empty object (72), an object with four properties (104), a
  dense array of 1,000 numbers (8,074), 1,000-character Latin-1 and UTF-16
  strings (1,025 and 2,025), closures (120, and 184 with one captured
  variable), a symbol, compiled code (232 bytes for an empty function and
  26 per instruction with its tables) and a new agent with the library
  (121,010; it was 105,954 before `SharedArrayBuffer` and `Atomics` were
  installed). The test compares a fresh report with the committed file on
  64-bit targets and names `RUSTIC_HEAP_REPORT_BLESS=1` to regenerate it; a
  line that grows needs an explanation in the same change.
- **Runner.** `cargo run --release -p rustic-js --example js_bench` runs
  each workload in a fresh agent (21 runs of `run(200000)` by default) and
  prints median and 95th-percentile time, peak heap
  (`Heap::peak_bytes`), collections and the total and longest collection
  pause (`Heap::set_gc_clock` times collections only when the host asks).

Results on the development machine (x86-64, Intel Core i7-13650HX, Linux
7.3, rustc 1.99.1, release build, one performance core):

| Workload | Median ms | p95 ms | Peak heap KiB | Collections | Total pause ms | Longest pause ms |
| --- | --- | --- | --- | --- | --- | --- |
| `alloc_churn` | 107.73 | 108.79 | 1,197 | 46 | 9.99 | 0.26 |
| `arrays` | 214.84 | 219.64 | 6,411 | 2 | 0.89 | 0.66 |
| `calls` | 31.90 | 32.08 | 108 | 0 | 0.00 | 0.00 |
| `closures` | 58.57 | 59.03 | 1,131 | 35 | 6.32 | 0.21 |
| `framework_diff` | 679.93 | 682.75 | 1,308 | 248 | 46.50 | 0.30 |
| `json_tree` | 438.33 | 448.98 | 95,963 | 7 | 32.63 | 15.46 |
| `property_access` | 18.20 | 18.26 | 108 | 0 | 0.00 | 0.00 |
| `strings` | 113.66 | 116.07 | 8,309 | 30 | 15.43 | 1.34 |

What the corpus found, all fixed in the same change:

- `charAt`, `charCodeAt`, `codePointAt` and `at` copied the whole string on
  every call, so a loop over a string was quadratic.
- Appending to a string copied it every time (TD-101).
- A `var` at the top level was never cached, because the global object is
  in dictionary mode and caches were keyed on shapes. A global site now
  remembers the dictionary entry and checks its name on every use: a read
  went from 75 ns to 9 ns and a write from 280 ns to 18 ns.
- `for-in` built a set of seen keys and walked every prototype's keys. When
  no prototype has an enumerable or indexed property, which is the usual
  case, the keys are the object's own.

What it measured and left open: collection is stop-the-world, and the
longest pause grows with the live heap, about 0.17 ms per MiB
(`json_tree`, 15 ms at 94 MiB). That is over the 5 ms target of
[PERFORMANCE.md](PERFORMANCE.md#javascript-runtime) above roughly 30 MiB of
live data, which is the measured trigger of TD-95; incremental marking is
`perf.gc` (M12). The native Linux ARM64 CI job runs the same release corpus
and writes its timings, processor, kernel, Rust version and source revision
to the job summary. The milestone still needs a successful native ARM64 run
and its recorded results.

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
  - A string pattern for `match`, `matchAll` or `search` is made into a
    RegExp, as the spec says.
  - `normalize` implements NFC, NFD, NFKC and NFKD with pinned Unicode
    17.0.0 data shared with the URL engine. Lone surrogates retain their
    UTF-16 code units and separate combining runs. The method bounds
    decomposition workspace and result length using the agent's available
    heap budget. Its native method and atom add 193 bytes to the measured
    fresh agent. Locale-dependent comparison and case mapping remain in TD-107.
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
- **eval.** Direct and indirect `eval` (§19.2.1).
  - A call of the bare name `eval` compiles to `DirectEval`. When the
    callee turns out to be `%eval%`, the string is parsed with the caller's
    strictness, compiled as eval code and run as a new frame on the
    caller's environment with its `this`; otherwise it is an ordinary call.
  - The resolver marks every binding visible from a direct `eval` captured,
    so the eval code finds them by name in environments. A function with a
    sloppy direct `eval` also keeps an environment for the `var`s the eval
    may add, and names it does not declare are looked up by name inside it.
    Functions without a direct `eval` compile exactly as before and pay
    nothing.
  - EvalDeclarationInstantiation checks that no `var` hoists past a `let`,
    `const` or `class` of the same name (a flag on lexical bindings; catch
    parameters are exempt, Annex B), checks global definability, then
    creates the bindings as deletable.
  - Not yet: declaring `arguments` from an `eval` in parameter defaults,
    which must throw; `super` inside eval code; the separate variable
    environment of a function whose parameters have expressions.
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
  - Test262 `test/built-ins/Iterator`: 467 of 684 files pass. Of the rest,
    about 170 test proposals not implemented yet (`zip`, `zipKeyed`,
    `chunks`, `windows`, `includes`, `join`), and a few need `$262`,
    `RegExp` or async.

Time on the development machine (release): 1 million `push` calls take 300
ms including the global loop around them (about 120 ns per call). A `map` over
1 million elements takes 40 ms beyond filling the array. Sorting 200,000
numbers takes 190 ms with a comparator and 145 ms with the default string
order. The methods take the generic path through the internal methods. A
dense-array fast path for `push`, `map` and `sort` is `perf.interpreter` (M12)
work; the JS-09 `arrays` workload is its baseline.

A native function can be a constructor, marked by `FLAG_NATIVE_CONSTRUCTOR`.
`new` calls it with a new target, which the function reads with
`Agent::new_target()`. Native constructors work as `extends` bases and
`super()` targets, and `Agent::construct_with` passes a new target through.

A new agent now holds 83 objects in 17,390 bytes, up from 32 objects in
6,477 bytes before JS-05. Most of the growth is native function objects.
Installing the rest of the library lazily is the lever if agents per page
start to matter (`perf.startup`, M12); the JS-09 heap report tracks the size.

With part 1, the Test262 `test/language` pass rate rose from 44.4% to 60.4%,
and with the part 2 pieces above to 70.0%. Eval took it to 73.6% and
generators to 90.8%.

## Intl locale identifiers, Locale objects and casing (JS-08)

`Intl.getCanonicalLocales` validates and deduplicates locale lists in request
order. The parser implements Unicode BCP 47 locale identifiers, canonical
casing and ordering, CLDR language, region and extension aliases, and
likely-region selection for deprecated territories. Locale-sensitive case
conversion applies the Unicode special casing rules for Azerbaijani,
Lithuanian and Turkish while preserving lone UTF-16 surrogates.

`Intl.Locale` implements the ECMA-402 §15 constructor, branded accessors,
`maximize`, `minimize` and seven information methods. Likely-subtag
changes preserve variants and extensions. Its preference methods read
pinned calendar, hour-cycle, collation, numbering, script-direction, week
and time-zone data. Time zones come from the canonical entries in IANA's
public-domain `zone1970.tab` for the explicit region.

The pinned Intl selection covers `Intl/getCanonicalLocales`, `Locale`
and both `String.prototype.toLocale*Case` directories. It passes 233 of 237
files in the original run. Later runs include realms, list formatting, and
collation. See [Intl collation](#intl-collation-js-08) for the current selection.

The data generator takes `supplementalMetadata.xml`, `likelySubtags.xml`,
`supplemental/supplementalData.xml`, `scriptMetadata.txt`, `core.zip` and
the listed `bcp47/` XML files from [CLDR release 48]. It also reads
Unicode 17.0.0 `PropList.txt` and `zone1970.tab` from [IANA tzdb 2026a].
The script verifies SHA-256 digests and checks the generated Rust with the
pinned formatter. The [Unicode license notice] covers the Unicode tables.

[CLDR release 48]: https://github.com/unicode-org/cldr/tree/release-48/common
[IANA tzdb 2026a]: https://www.iana.org/time-zones/releases/2026a
[Unicode license notice]: ../crates/rustic-js/data/CLDR-LICENSE.txt

```sh
python3 tools/generate_js_locale_data.py /path/to/pinned/locale-inputs --check
RUSTIC_TEST262_DIR=/path/to/test262 cargo test --locked --release -p rustic-js \
  --test test262 test262_locale_identifiers_and_case_mapping -- --ignored
```

## Intl list formatting (JS-08)

`Intl.ListFormat` implements `format`, `formatToParts`, `resolvedOptions`
and `supportedLocalesOf` with the nine CLDR 48 type/style combinations.
Locale lookup strips extensions for matching, walks parent tags and uses
`en-US` as Rustic's deterministic default. The best-fit option also
tries likely subtags when lookup fails. The formatter resolves inherited
CLDR templates at generation time and walks the parts in linear time.
It preserves each string's UTF-16 code units and closes the iterator when
it encounters a non-string element.

The combined pinned Intl selection passed 313 of 318 files, including
80 of 81 ListFormat cases. With [realms](#realms-js-06) it passes 315,
including every ListFormat case. Context-dependent conjunctions and
specialized best-fit matching remain open under TD-117.

## Intl collation (JS-08)

`Intl.Collator` and `String.prototype.localeCompare` share locale lookup and
Unicode `co`, `kn`, and `kf` negotiation. The comparison uses CLDR 48 root
weights and Unicode 17 canonical decomposition, preserves lone surrogates,
and supports German phonebook ordering, case ordering, sensitivity, variable
punctuation and decimal runs from every Unicode decimal script. Case and
accent sensitivity are applied to separate collation levels. Each instance
caches its bound `compare` function. The collector traces the callback and
its captured instance.

The pinned selection passes 388 of 393 Intl files, including 63 of 65
Collator files and all 10 `localeCompare` files. TD-107 records the remaining
tailorings and root collation gaps. The broader Intl service constructors
remain open.

Generate or verify the static tables with CLDR 48 `core.zip` and Unicode
17.0.0 `UnicodeData.txt`. The generator verifies the upstream SHA-256
digests. The [Unicode license notice] applies to these tables.

```sh
python3 tools/generate_js_collation.py /path/to/pinned/collation-inputs --check
```


## Intl plural rules (JS-08)

`Intl.PluralRules` resolves the locale and selects cardinal, ordinal, and
range categories with generated CLDR 48 rules. It rounds decimal coefficients
before deriving the plural operands, retaining significant and visible
fraction digits according to the options. Decimal strings and BigInts
retain their precision through the category calculation. Compact notation
uses the locale's pinned decimal scales.

The pinned Intl selection passes 440 of 446 files, including 52 of 53
PluralRules files. Its outstanding constructor test also exercises the
unfinished `Intl.NumberFormat` and `Intl.DateTimeFormat` constructors.

```sh
python3 tools/generate_js_plural_rules.py /path/to/pinned/core.zip --check
```
