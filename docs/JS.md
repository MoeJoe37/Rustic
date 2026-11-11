# JavaScript: lexer and parser (JS-01)

`rustic-js` is Rustic's own ECMAScript implementation (M4, ADR 0001: no
existing JavaScript engine is used or embedded). JS-01 provides the front end:
UTF-16 source text, the lexical grammar, and a parser from scripts and modules
to an abstract syntax tree with source spans. Nothing executes yet. Values,
the heap and the interpreter follow in JS-02 to JS-04.

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
module and a strict script. It checks that nothing panics or overflows the
stack, that syntax error spans lie inside the source, that a successful parse
dumps deterministically, and that anything valid in strict mode is valid in
sloppy mode. Seeds are one snippet per line of
`crates/rustic-fuzz/corpus/js-parse/`.
