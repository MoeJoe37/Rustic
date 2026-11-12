//! JS-01 parser behaviour through the public API: tree shape, early errors,
//! automatic semicolon insertion, strict mode and resource limits.

use rustic_base::CancellationToken;
use rustic_js::ast::{ProgramKind, dump};
use rustic_js::{ParseError, ParseLimits, Source, parse_module, parse_script};

fn source(text: &str) -> Source {
    Source::from_str_limited(text, usize::MAX).expect("test source fits")
}

fn script(text: &str) -> Result<String, ParseError> {
    let program = parse_script(&source(text), &ParseLimits::default())?;
    assert_eq!(program.kind, ProgramKind::Script);
    Ok(dump(&program).expect("test trees are shallow"))
}

fn module(text: &str) -> Result<String, ParseError> {
    let program = parse_module(&source(text), &ParseLimits::default())?;
    assert_eq!(program.kind, ProgramKind::Module);
    Ok(dump(&program).expect("test trees are shallow"))
}

#[track_caller]
fn assert_script(text: &str, expected: &str) {
    match script(text) {
        Ok(tree) => assert_eq!(tree, expected, "for `{text}`"),
        Err(e) => panic!("`{text}` failed: {e}"),
    }
}

#[track_caller]
fn assert_script_error(text: &str) {
    if let Ok(tree) = script(text) {
        panic!("`{text}` parsed as {tree}");
    }
}

#[test]
fn trees_record_structure_and_precedence() {
    assert_script("var x = 1;", "(script (var (= x 1)))");
    assert_script("a?.b(c)", "(script (expr (chain (call (?. a b) c))))");
    assert_script("x => x * 2", "(script (expr (arrow (params x) (* x 2))))");
    assert_script(
        "let {a, ...b} = c",
        "(script (let (= (object-pattern a (... b)) c)))",
    );
    assert_script(
        "class A { #x; m() { return this.#x } }",
        "(script (class A (field #x _) (method m (function strict (params) (body (return (. this #x)))))))",
    );
    // `**` is right-associative and binds tighter than `*`.
    let power = script("a * b ** c ** d").unwrap();
    assert!(power.contains("(* a (** b (** c d)))"), "{power}");
    // Cover grammar: the same tokens become a pattern before `=`.
    let destructure = script("[a, {b}] = c").unwrap();
    assert!(destructure.contains("array-pattern"), "{destructure}");
}

#[test]
fn automatic_semicolon_insertion_follows_line_terminators() {
    assert_script("a\n++b", "(script (expr a) (expr (++prefix b)))");
    // `return` followed by a line break returns nothing.
    let tree = script("function f() { return\nx }").unwrap();
    assert!(tree.contains("(return)"), "{tree}");
    // No semicolon may be inserted between tokens on one line.
    assert_script_error("a b");
    // Nor in the header of a `for` statement.
    assert_script_error("for (a\nb\n) {}");
}

#[test]
fn early_errors_reject_invalid_programs() {
    for text in [
        "let x; let x;",
        "const y;",
        "break;",
        "continue;",
        "return;",
        "super();",
        "new.target;",
        "class A { constructor() {} constructor() {} }",
        "class B { #a; #a; }",
        "this.#undeclared;",
        "x = { a = 1 };",
        "a ?? b || c;",
        "-a ** b;",
        "a?.b = 1;",
        "a?.`t`;",
        "(a, a) => {};",
        "async (await) => {};",
        "function f(...a,) {}",
        "[...a, b] = c;",
        "({ __proto__: a, __proto__: b });",
        "/a/gg;",
        "1__0;",
        "0_1;",
        "08n;",
        "'\\u{110000}';",
        "label: label: x;",
        "for (let x of a, b) {}",
        "import x from 'y';",
        "export default 1;",
        "function f(a = 1) { 'use strict'; }",
    ] {
        assert_script_error(text);
    }
    // A duplicate `__proto__` is fine once the literal becomes a pattern.
    assert!(script("({ __proto__: a, __proto__: b } = c);").is_ok());
}

#[test]
fn strict_mode_comes_from_directives_classes_and_modules() {
    for text in [
        "with (x) y;",
        "010;",
        "'\\101';",
        "delete x;",
        "var eval;",
        "function f(a, a) {}",
        "var let = 1;",
        "yield = 1;",
    ] {
        assert!(script(text).is_ok(), "sloppy `{text}`");
        assert_script_error(&format!("'use strict'; {text}"));
        assert!(module(text).is_err(), "module `{text}`");
    }
    // An escaped directive is not a `use strict` directive.
    assert!(script("'use\\x20strict'; with (x) y;").is_ok());
    // Class bodies are strict.
    assert_script_error("class A { m() { with (x) y; } }");
    // A directive applies to the function it starts, including its name.
    assert_script_error("function eval() { 'use strict'; }");
}

#[test]
fn annex_b_syntax_is_script_only() {
    for text in [
        "<!-- comment\nx",
        "x\n--> comment",
        "if (a) function f() {}",
        "lbl: function f() {}",
    ] {
        assert!(script(text).is_ok(), "script `{text}`");
        assert!(module(text).is_err(), "module `{text}`");
    }
}

#[test]
fn modules_parse_imports_exports_and_top_level_await() {
    let tree =
        module("import x, { y as z } from 'm'; export { x as default, z }; await 1;").unwrap();
    assert!(tree.starts_with("(module strict"), "{tree}");
    assert!(
        module("export * as ns from 'm'; import { 'a b' as c } from 'n' with { type: 'json' };")
            .is_ok()
    );
    for text in [
        "export { x };",
        "export { a }; export { a }; var a;",
        "export default 1; export default 2;",
        "import { a } from 'm'; let a;",
        "if (1) { import x from 'm'; }",
    ] {
        assert!(module(text).is_err(), "module `{text}`");
    }
    // `await` is an identifier in scripts.
    assert!(script("var await = 1;").is_ok());
    assert!(module("var await = 1;").is_err());
}

#[test]
fn errors_carry_utf16_positions() {
    let text = "let a = '😀';\nlet a;";
    let source = source(text);
    let Err(ParseError::Syntax(error)) = parse_script(&source, &ParseLimits::default()) else {
        panic!("redeclaration parsed");
    };
    let at = source.position(error.span.start);
    assert_eq!((at.line, at.column), (2, 5));
    assert_eq!(source.text(error.span), "a");
}

#[test]
fn nesting_beyond_the_limit_fails_without_overflowing() {
    let limits = ParseLimits {
        max_depth: 64,
        ..ParseLimits::default()
    };
    let deep = format!("{}1{}", "(".repeat(10_000), ")".repeat(10_000));
    assert!(matches!(
        parse_script(&source(&deep), &limits),
        Err(ParseError::TooDeep(_))
    ));
    let blocks = format!("{}{}", "{".repeat(10_000), "}".repeat(10_000));
    assert!(matches!(
        parse_script(&source(&blocks), &ParseLimits::default()),
        Err(ParseError::TooDeep(_))
    ));
    let shallow = format!("{}1{}", "[".repeat(32), "]".repeat(32));
    assert!(parse_script(&source(&shallow), &limits).is_ok());
    // The default limits allow realistic nesting in every build profile.
    let nested = format!("{}1{}", "([".repeat(64), "])".repeat(64));
    assert!(parse_script(&source(&nested), &ParseLimits::default()).is_ok());
}

#[test]
fn the_stack_guard_stops_nesting_before_the_depth_limit() {
    let limits = ParseLimits {
        max_depth: u32::MAX,
        max_stack_bytes: 64 * 1024,
        ..ParseLimits::default()
    };
    let deep = format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000));
    let Err(ParseError::TooDeep(span)) = parse_script(&source(&deep), &limits) else {
        panic!("deep nesting parsed");
    };
    assert!(span.start < 10_000, "stopped at {span}");
}

#[test]
fn long_left_chains_parse_and_drop_iteratively() {
    // 200 000 operands nest the tree that deep on the left; neither
    // parsing nor dropping it may recurse per operand.
    let chain = vec!["a"; 200_000].join(" + ");
    let program = parse_script(&source(&chain), &ParseLimits::default()).unwrap();
    assert!(
        dump(&program).is_err(),
        "dump refuses rather than truncating"
    );
    drop(program);
    let members = format!("a{}", ".b".repeat(200_000));
    assert!(parse_script(&source(&members), &ParseLimits::default()).is_ok());
}

#[test]
fn cancellation_stops_the_parse() {
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let limits = ParseLimits {
        cancellation,
        ..ParseLimits::default()
    };
    assert_eq!(
        parse_script(&source("a; b; c;"), &limits),
        Err(ParseError::Cancelled)
    );
}

#[test]
fn source_length_is_checked_before_scanning() {
    let error = Source::from_str_limited("abcdef", 4).unwrap_err();
    assert_eq!(error.limit, 4);
    // Astral characters count as two code units.
    assert!(Source::from_str_limited("😀😀", 3).is_err());
    assert!(Source::from_utf16(vec![0xD800, 0x61], 2).is_ok());
}

#[test]
fn regexp_literals_are_validated_at_parse_time() {
    // §13.2.7.2: a literal whose pattern or flags are invalid is an early
    // SyntaxError, so nothing of the script runs (TD-88).
    for text in [
        "/(/;",
        "/a**/;",
        "/[b-a]/;",
        "/(?<n>a)(?<n>b)/;",
        "/\\k<missing>/u;",
        "/\\u{110000}/u;",
        "/a/gg;",
        "/a/uv;",
        "/(?<=a)+/u;",
        "/{1}/u;",
    ] {
        assert_script_error(text);
    }
    for text in [
        "/(?<n>a)|(?<n>b)/;",
        "/a{/;",
        "/[\\p{L}--[a-z]]/v;",
        "/(?i:a)b/;",
        "/(?<=a)b/dgimsuy;",
    ] {
        assert!(script(text).is_ok(), "{text}");
    }
}
