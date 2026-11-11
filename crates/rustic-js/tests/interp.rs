//! JS-04: scripts compile to bytecode and run.

use rustic_base::CancellationToken;
use rustic_js::EvalError;
use rustic_js::runtime::{Abrupt, Agent, Value};

/// Run `source` and show its completion value; a thrown error shows as
/// `!Name: message`.
fn run(source: &str) -> String {
    let mut agent = Agent::new().expect("agent");
    run_in(&mut agent, source)
}

fn run_in(agent: &mut Agent, source: &str) -> String {
    match agent.evaluate(source) {
        Ok(value) => agent.display(value),
        Err(EvalError::Throw(value)) => format!("!{}", describe_error(agent, value)),
        Err(other) => format!("!!{other:?}"),
    }
}

fn describe_error(agent: &mut Agent, value: Value) -> String {
    match agent.to_string(value) {
        Ok(s) => agent.heap.string(s).to_text(),
        Err(_) => "<unprintable>".into(),
    }
}

thread_local! {
    static FAILURES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Compare a script's result, recording a mismatch instead of stopping,
/// so one run shows every failing case of a test.
fn check(source: &str, expected: &str) {
    let actual = std::panic::catch_unwind(|| run(source)).unwrap_or_else(|_| "<panic>".into());
    if actual != expected {
        FAILURES.with_borrow_mut(|f| {
            f.push(format!(
                "{source}\n    expected: {expected}\n    actual:   {actual}"
            ));
        });
    }
}

/// Fails the test at its end if any [`check`] did not match.
struct Report;

impl Drop for Report {
    fn drop(&mut self) {
        let failures = FAILURES.take();
        if !failures.is_empty() && !std::thread::panicking() {
            panic!("{} mismatches:\n{}", failures.len(), failures.join("\n"));
        }
    }
}

#[test]
fn literals_and_operators() {
    let _report = Report;
    check("1 + 2 * 3", "7");
    check("'a' + 1 + 2", "a12");
    check("1 + 2 + 'a'", "3a");
    check("7 % 3", "1");
    check("2 ** 10", "1024");
    check("1 / 0", "Infinity");
    check("-0 === 0", "true");
    check("5 & 3 | 8 ^ 1", "9");
    check("1 << 31", "-2147483648");
    check("-1 >>> 0", "4294967295");
    check("-8 >> 1", "-4");
    check("'b' > 'a'", "true");
    check("null == undefined", "true");
    check("null === undefined", "false");
    check("'1' == 1", "true");
    check(
        "typeof 1 + typeof 'x' + typeof undefined + typeof null",
        "numberstringundefinedobject",
    );
    check("typeof function(){}", "function");
    check("typeof nope", "undefined");
    check("!0", "true");
    check("~5", "-6");
    check("void 0", "undefined");
    check("`a${1 + 1}b${'c'}`", "a2bc");
    check("1 < 2 ? 'y' : 'n'", "y");
    check("null ?? 'd'", "d");
    check("0 || 'x'", "x");
    check("0 && 'x'", "0");
    check("(1, 2, 3)", "3");
    check("1 in [5]", "false");
    check("0 in [5]", "true");
    check("'x' in {x: 1}", "true");
    check("1 in 2", "!TypeError: right side of 'in' is not an object");
}

#[test]
fn variables_and_scopes() {
    let _report = Report;
    check("var x = 1; x = x + 1; x", "2");
    check("let x = 1; { let x = 2; } x", "1");
    check("const c = 5; c", "5");
    check(
        "const c = 5; c = 6",
        "!TypeError: assignment to a constant binding",
    );
    check(
        "{ const c = 5; c = 6 }",
        "!TypeError: assignment to constant variable c",
    );
    check(
        "{ const c = 5; (() => { c = 6 })() }",
        "!TypeError: assignment to constant variable c",
    );
    check(
        "x; let x = 1",
        "!ReferenceError: x cannot be accessed before initialization",
    );
    check(
        "{ y; let y = 1 }",
        "!ReferenceError: y cannot be accessed before initialization",
    );
    check("nope", "!ReferenceError: nope is not defined");
    check("x = 5; x", "5");
    check("'use strict'; x = 5", "!ReferenceError: x is not defined");
    check("var a = 1; var a = 2; a", "2");
    assert!(run("let a = 1; var a = 2;").starts_with("!!Parse(Syntax("));
    check("var x = 10; x += 5; x *= 2; x -= 1; x", "29");
    check("var x = null; x ??= 3; x ||= 4; x &&= 5; x", "5");
    check("var i = 0; i++ + ++i", "2");
    check("var i = 5; i--; --i; i", "3");
    check(
        "var [a, b = 2, ...c] = [1, undefined, 3, 4]; a + b + c.length",
        "5",
    );
    check(
        "var {p, q: {r}, ...rest} = {p: 1, q: {r: 2}, s: 3, t: 4}; p + r + rest.s + rest.t",
        "10",
    );
    check(
        "var a, b; [a, b] = [1, 2]; [a, b] = [b, a]; a * 10 + b",
        "21",
    );
    check("var o = {}; ({x: o.y} = {x: 7}); o.y", "7");
    check("globalThis.zz = 3; zz", "3");
    check("var v = 1; globalThis.v", "1");
    check("let l = 1; globalThis.l", "undefined");
}

#[test]
fn control_flow() {
    let _report = Report;
    check("var s = 0; for (var i = 0; i < 5; i++) s += i; s", "10");
    check(
        "var s = 0, i = 0; while (i < 5) { i++; if (i == 2) continue; if (i == 4) break; s += i; } s",
        "4",
    );
    check("var i = 0; do { i++ } while (i < 3); i", "3");
    check("var s = ''; for (var k in {a: 1, b: 2}) s += k; s", "ab");
    check("var s = 0; for (var v of [1, 2, 3]) s += v; s", "6");
    check(
        "var s = ''; for (const ch of 'h\\u{1F600}i') s += ch.length; s",
        "121",
    );
    check(
        "var s = ''; outer: for (var i = 0; i < 3; i++) { for (var j = 0; j < 3; j++) { if (j == 1) continue outer; if (i == 2) break outer; s += i + '' + j + ' '; } } s",
        "00 10 ",
    );
    check(
        "function f(x) { switch (x) { case 1: return 'one'; case 2: case 3: return 'few'; default: return 'many'; } } f(1) + f(3) + f(9)",
        "onefewmany",
    );
    check(
        "var s = ''; switch (2) { case 1: s += 'a'; case 2: s += 'b'; case 3: s += 'c'; break; case 4: s += 'd'; } s",
        "bc",
    );
    check(
        "var x = 0; if (x) x = 1; else if (x === 0) x = 2; else x = 3; x",
        "2",
    );
    check("a: { b: { break a; } }; 1", "1");
    check(
        "var s = ''; for (var k in 'ab') s += k; for (k in null) s += 'x'; s",
        "01",
    );
    check(
        "var o = {a: 1, b: 2, c: 3}; var s = ''; for (var k in o) { delete o.b; s += k; } s",
        "ac",
    );
    check(
        "function P() { this.own = 1 } P.prototype.inherited = 2; var s = ''; for (var k in new P) s += k + ' '; s",
        "own inherited ",
    );
}

#[test]
fn functions_and_closures() {
    let _report = Report;
    check("function add(a, b) { return a + b } add(2, 3)", "5");
    check("hoisted(); function hoisted() { return 1 }", "1");
    check(
        "var f = function fact(n) { return n <= 1 ? 1 : n * fact(n - 1) }; f(5)",
        "120",
    );
    check(
        "var f = (a, b = a + 1, ...rest) => a + b + rest.length; f(1) + f(1, 1, 1, 1)",
        "7",
    );
    check(
        "function counter() { var n = 0; return function () { return ++n } } var c = counter(); c(); c(); c()",
        "3",
    );
    check(
        "function make() { var n = 0; return {inc: () => ++n, get: () => n} } var a = make(), b = make(); a.inc(); a.inc(); b.inc(); a.get() * 10 + b.get()",
        "21",
    );
    check(
        "var out = []; for (let i = 0; i < 3; i++) out[i] = () => i; out[0]() + out[1]() + out[2]()",
        "3",
    );
    check(
        "var out = []; for (var i = 0; i < 3; i++) out[i] = () => i; out[0]() + out[1]() + out[2]()",
        "9",
    );
    check(
        "var out = []; for (const x of [1, 2, 3]) out[out.length] = () => x; out[0]() + out[1]() + out[2]()",
        "6",
    );
    check(
        "function f() { return arguments.length + arguments[1] } f(5, 6, 7)",
        "9",
    );
    check("function f(a) { return typeof a } f()", "undefined");
    check("function f() { return this } f() === globalThis", "true");
    check(
        "function f() { 'use strict'; return this } f()",
        "undefined",
    );
    check(
        "var o = {x: 4, m() { return this.x }, a: function () { return (() => this.x)() }}; o.m() + o.a()",
        "8",
    );
    check(
        "var o = {get v() { return 3 }, set v(x) { this.w = x }}; o.v = 9; o.v + o.w",
        "12",
    );
    check(
        "function f() {} f.name + f.length + (function (a, b) {}).length",
        "f02",
    );
    check(
        "var g = function () {}; var h = () => 1; g.name + h.name",
        "gh",
    );
    check(
        "(function () { return typeof inner; function inner() {} })()",
        "function",
    );
    check(
        "function outer() { var x = 1; function inner() { return x + y } var y = 2; return inner() } outer()",
        "3",
    );
    check("function f(a, a2) { a = 5; return a + a2 } f(1, 2)", "7");
    check(
        "function f({a, b}, [c]) { return a + b + c } f({a: 1, b: 2}, [3])",
        "6",
    );
    check("1()", "!TypeError: value is not a function");
    check("var o = {}; o.f()", "!TypeError: value is not a function");
    check(
        "undefined.x",
        "!TypeError: cannot access property x of undefined or null",
    );
    check("var f = (...a) => a.length; f(...[1, 2], 3, ...'ab')", "5");
    check(
        "function f(x) { return x && x.y } f(null) + '' + f({y: 2})",
        "null2",
    );
    check(
        "var o = {a: {b: null}}; o?.a?.b?.c + '' + o.z?.q.r + o.a.f?.()",
        "undefinedundefinedundefined",
    );
}

#[test]
fn constructors_and_prototypes() {
    let _report = Report;
    // A function whose last statement is a skipped `return`.
    check(
        "function C() { if (!new.target) return '' } typeof new C() + typeof C()",
        "objectstring",
    );
    check(
        "function f(x) { if (x) return 1 } '' + f(1) + f(0)",
        "1undefined",
    );
    check(
        "function P(x) { this.x = x } P.prototype.get = function () { return this.x }; new P(4).get()",
        "4",
    );
    check(
        "function P() {} var p = new P; (p instanceof P) + ' ' + (p.constructor === P) + ' ' + ({} instanceof P)",
        "true true false",
    );
    check("function P() { return {v: 1} } new P().v", "1");
    check("function P() { this.v = 2; return 5 } new P().v", "2");
    check(
        "function P() { return new.target === P } new P() + '' + P()",
        "[object Object]false",
    );
    check("new (() => 1)", "!TypeError: value is not a constructor");
    check(
        "var o = {m() {}}; new o.m",
        "!TypeError: value is not a constructor",
    );
    check(
        "var proto = {hi() { return 'hi' }}; var o = {__proto__: proto}; o.hi()",
        "hi",
    );
    check(
        "function A(...xs) { this.n = xs.length } new A(...[1, 2, 3]).n",
        "3",
    );
    check(
        "var o = {a: 1, ...{b: 2, c: 3}, ['d' + 1]: 4}; o.a + o.b + o.c + o.d1",
        "10",
    );
    check("var a = [1, , 3]; a.length + ' ' + (1 in a)", "3 false");
    check("var a = [1, ...[2, 3], 4]; a.length", "4");
    check("var o = {x: 1}; delete o.x; 'x' in o", "false");
    check("var a = []; a[3] = 1; a.length", "4");
    check("var s = 'abc'; s.length + s[1]", "3b");
}

#[test]
fn exceptions() {
    let _report = Report;
    check("try { throw 1 } catch (e) { e + 1 }", "2");
    check(
        "var r; try { nope() } catch (e) { r = typeof e + e.name } r",
        "objectReferenceError",
    );
    check("var s = ''; try { s += 'a' } finally { s += 'b' } s", "ab");
    check(
        "var s = ''; try { try { throw 'x' } finally { s += 'f' } } catch (e) { s += e } s",
        "fx",
    );
    check(
        "function f() { try { return 'try' } finally { g = 'ran' } } var g; f() + g",
        "tryran",
    );
    check(
        "function f() { try { return 'try' } finally { return 'finally' } } f()",
        "finally",
    );
    check(
        "function f() { try { throw 1 } catch (e) { return 'c' } finally { g += 1 } } var g = 0; f() + g",
        "c1",
    );
    check(
        "var s = ''; for (var i = 0; i < 3; i++) { try { if (i == 1) continue; s += i } finally { s += 'f' } } s",
        "0ff2f",
    );
    check(
        "var s = ''; for (var i = 0; i < 3; i++) { try { if (i == 1) break; s += i } finally { s += 'f' } } s",
        "0ff",
    );
    check(
        "function thrower() { throw new Error2('boom') } function Error2(m) { this.message = m } try { thrower() } catch ({message}) { message }",
        "boom",
    );
    check(
        "try { null.x } catch (e) { e.message }",
        "cannot access property x of undefined or null",
    );
    check("try { throw 1 } catch { 'no binding' }", "no binding");
    check(
        "function f() { try { throw 1 } finally { 2 } } try { f() } catch (e) { 'outer ' + e }",
        "outer 1",
    );
    check("throw 'up'", "!up");
    check(
        "function deep(n) { if (n == 0) throw 'bottom'; deep(n - 1) } try { deep(50) } catch (e) { e }",
        "bottom",
    );
    check(
        "var log = ''; var it = {[Symbol_iterator]() { return {next() { return {done: false, value: 1} }, return() { log += 'closed'; return {} }} }};",
        "!ReferenceError: Symbol_iterator is not defined",
    );
}

#[test]
fn with_statement_resolves_names_dynamically() {
    let _report = Report;
    check("var o = {x: 1}; var x = 5; with (o) { x = 2 } o.x + x", "7");
    check(
        "var o = {f() { return this === o }}; with (o) { f() }",
        "true",
    );
    check(
        "function outer() { var local = 3; with ({}) { return (function () { return local })() } } outer()",
        "3",
    );
    check("with ({a: 1}) { typeof a + typeof b }", "numberundefined");
}

#[test]
fn recursion_is_bounded_without_the_rust_stack() {
    let _report = Report;
    // Far deeper than the Rust stack could take if calls recursed.
    check(
        "function f(n) { return n == 0 ? 0 : 1 + f(n - 1) } f(9000)",
        "9000",
    );
    check(
        "function f() { return f() } try { f() } catch (e) { e.message }",
        "Maximum call stack size exceeded",
    );
    // Recursion through a getter nests native frames and is bounded too.
    check(
        "var o = {get g() { return this.g }}; try { o.g } catch (e) { e.message }",
        "Maximum call stack size exceeded",
    );
    // The stack is usable again afterwards.
    check(
        "function f() { return f() } try { f() } catch (e) {} function g(n) { return n ? g(n - 1) : 'ok' } g(100)",
        "ok",
    );
}

#[test]
fn cancellation_stops_an_endless_loop() {
    let mut agent = Agent::new().expect("agent");
    let token = CancellationToken::default();
    agent.set_cancellation(Some(token.clone()));
    token.cancel();
    assert_eq!(agent.evaluate("for (;;) {}"), Err(EvalError::Cancelled));
    assert_eq!(
        agent.evaluate("function f() { try { f() } catch (e) { f() } } f()"),
        Err(EvalError::Cancelled)
    );
    // Script cannot catch it.
    assert_eq!(
        agent.evaluate("try { while (true) {} } catch (e) { 1 } finally { 2 }"),
        Err(EvalError::Cancelled)
    );
    // A loop whose only back edge is a `continue`.
    for source in [
        "outer: while (true) { do { continue outer } while (false) }",
        "while (true) continue",
        "for (;;) continue",
        "do { continue } while (true)",
    ] {
        assert_eq!(
            agent.evaluate(source),
            Err(EvalError::Cancelled),
            "{source}"
        );
    }
    agent.set_cancellation(None);
    assert_eq!(run_in(&mut agent, "1 + 1"), "2");
}

#[test]
fn a_step_budget_cancels_deterministically() {
    let mut agent = Agent::new().expect("agent");
    agent.set_step_budget(Some(1000));
    assert_eq!(
        run_in(
            &mut agent,
            "var n = 0; for (var i = 0; i < 500; i++) n++; n"
        ),
        "500"
    );
    // 500 steps are left, shared by later runs; the body runs once more
    // before the back edge that finds none.
    assert_eq!(
        agent.evaluate("for (var i = 0; i < 501; i++) n++"),
        Err(EvalError::Cancelled)
    );
    assert_eq!(run_in(&mut agent, "n"), "1001");
    assert_eq!(
        agent.evaluate("function f() {} f()"),
        Err(EvalError::Cancelled)
    );
    agent.set_step_budget(None);
    assert_eq!(
        run_in(&mut agent, "for (var i = 0; i < 5000; i++) n++; n"),
        "6001"
    );
}

#[test]
fn results_do_not_depend_on_inline_caches() {
    let source = "function P(a) { this.a = a } P.prototype.get = function () { return this.a }; \
                  var t = 0; for (var i = 0; i < 300; i++) { var o = new P(i); o.b = i; t += o.get() + o.b } \
                  P.prototype.get = function () { return 1 }; t + new P(0).get()";
    let mut cached = Agent::new().expect("agent");
    let mut uncached = Agent::new().expect("agent");
    uncached.set_inline_caches(false);
    assert_eq!(run_in(&mut cached, source), "89701");
    assert_eq!(run_in(&mut uncached, source), "89701");
}

#[test]
fn out_of_memory_is_not_catchable() {
    let mut agent = Agent::with_heap_limit(2 * 1024 * 1024).expect("agent");
    let result = agent.evaluate(
        "var a = []; try { for (;;) a[a.length] = {x: a.length} } catch (e) { 'caught' }",
    );
    assert_eq!(result, Err(EvalError::OutOfMemory));
    let _ = Abrupt::OutOfMemory;
}

#[test]
fn garbage_is_collected_while_a_script_runs() {
    let mut agent = Agent::with_heap_limit(4 * 1024 * 1024).expect("agent");
    // Allocates far more than the heap limit in total; only the collector
    // makes this fit.
    let source = "var keep = []; for (var i = 0; i < 200000; i++) { var o = {i: i, s: 'x' + i}; if (i % 50000 == 0) keep[keep.length] = o; } keep.length + ' ' + keep[3].s";
    assert_eq!(run_in(&mut agent, source), "4 x150000");
    assert!(agent.heap.collections() > 0);
}

#[test]
fn collection_under_stress_keeps_live_values() {
    let mut agent = Agent::new().expect("agent");
    agent.heap.set_gc_stress(true);
    let source = "function make(n) { var list = []; for (var i = 0; i < n; i++) list[i] = {v: 'item' + i, f: () => i}; return list }
        var all = make(20); var s = '';
        for (const o of all) s += o.v.length;
        function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2) }
        s + ' ' + fib(12) + ' ' + all[19].f()";
    assert_eq!(run_in(&mut agent, source), "55555555556666666666 144 20");
}

#[test]
fn globals_persist_across_scripts() {
    let mut agent = Agent::new().expect("agent");
    assert_eq!(
        run_in(
            &mut agent,
            "var a = 1; let b = 2; function inc() { return ++a }"
        ),
        "undefined"
    );
    assert_eq!(run_in(&mut agent, "inc(); a + b"), "4");
    assert_eq!(
        run_in(&mut agent, "let b = 3"),
        "!SyntaxError: redeclaration of b"
    );
    // A global lexical binding shadows a property a site already cached.
    assert_eq!(
        run_in(
            &mut agent,
            "function read() { return shadow } globalThis.shadow = 'prop'; read()"
        ),
        "prop"
    );
    assert_eq!(
        run_in(&mut agent, "let shadow = 'lexical'; read()"),
        "lexical"
    );
}

#[test]
fn inline_caches_survive_shape_changes() {
    let _report = Report;
    check(
        "function get(o) { return o.x } var a = {x: 1}, b = {y: 0, x: 2}, c = {x: 3}; get(a) + get(b) + get(c) + get(a)",
        "7",
    );
    check(
        "function set(o, v) { o.x = v } var a = {x: 1}, b = {y: 0, x: 2}; set(a, 5); set(b, 6); set(a, 7); a.x + b.x",
        "13",
    );
    check(
        "function get(o) { return o.x } var o = {x: 1}; get(o); delete o.x; get(o) + '' + get({get x() { return 'acc' }})",
        "undefinedacc",
    );
    check(
        "'use strict'; function set(o) { o.x = 2 } var o = {x: 1}; set(o); var p = {}; Object_freeze_missing = 0",
        "!ReferenceError: Object_freeze_missing is not defined",
    );
}

#[test]
fn inline_caches_follow_prototype_changes() {
    let _report = Report;
    // An inherited method is cached, then replaced, shadowed and deleted.
    check(
        "function P() {} P.prototype.m = function () { return 1 }; function call(o) { return o.m() } \
         var o = new P(), r = ''; r += call(o); r += call(o); \
         P.prototype.m = function () { return 2 }; r += call(o); \
         o.m = function () { return 3 }; r += call(o); delete o.m; r += call(o); \
         delete P.prototype.m; try { call(o) } catch (e) { r += e.name } r",
        "11232TypeError",
    );
    // A property added to a link between the receiver and the cached holder.
    check(
        "var top = {x: 'top'}, mid = {__proto__: top}, o = {__proto__: mid}; function get(v) { return v.x } \
         var r = get(o) + get(o); mid.x = 'mid'; r += get(o); delete mid.x; r += get(o); r",
        "toptopmidtop",
    );
    // Objects that share a shape but not a prototype must not share a hit.
    check(
        "var a = {__proto__: {x: 1}}, b = {__proto__: {x: 2}}, c = {__proto__: null}; function get(v) { return v.x } \
         '' + get(a) + get(b) + get(a) + get(c) + get(b)",
        "121undefined2",
    );
    // An absent property is not remembered as absent.
    check(
        "var p = {}, o = {__proto__: p}; function get(v) { return v.x } var r = '' + get(o) + get(o); p.x = 5; r + get(o)",
        "undefinedundefined5",
    );
    // A cached add must notice a setter or a read-only property that appears on the chain.
    check(
        "var log = '', p = {}; function make() { var o = {__proto__: p}; o.x = 1; return o } \
         make(); make(); p = {set x(v) { log += 's' + v }}; var a = make(); \
         log + (a.x === undefined)",
        "s1true",
    );
    check(
        "var seen = '', p = {}; function put(o) { o.x = 1 } \
         put({__proto__: p}); put({__proto__: p}); \
         var holder = {set x(v) { seen += v }}; var late = {__proto__: holder}; put(late); put(late); \
         seen + ':' + late.x",
        "11:undefined",
    );
    // The same add site sees a prototype that gains a setter after the cache filled.
    check(
        "var seen = 0, p = {}; function put(o) { o.x = 7 } \
         var a = {__proto__: p}; put(a); var b = {__proto__: p}; put(b); \
         var acc = {set x(v) { seen += v }}; p.y = 1; \
         var c = {__proto__: p}; put(c); \
         '' + a.x + b.x + c.x + seen",
        "7770",
    );
    // Constructors: fields added through cached transitions land in the right slots.
    check(
        "function P(a, b) { this.a = a; this.b = b } P.prototype.sum = function () { return this.a + this.b }; \
         var t = 0; for (var i = 0; i < 2000; i++) { var o = new P(i, 1); t += o.sum() } \
         P.prototype = {sum: function () { return -1 }}; t += new P(1, 2).sum(); t",
        "2000999",
    );
    // Collections between hits keep the caches honest.
    check(
        "function P(v) { this.v = v } P.prototype.get = function () { return this.v }; \
         var t = 0; for (var i = 0; i < 60000; i++) { t += new P([i]).get()[0] } t",
        "1799970000",
    );
    // Global sites: a property that turns into an accessor, then is deleted.
    check(
        "globalThis.g = 1; function read() { return g } var r = '' + read() + read(); \
         delete globalThis.g; try { read() } catch (e) { r += e.name } r",
        "11ReferenceError",
    );
}

#[test]
fn function_prototypes_are_created_on_first_use() {
    let _report = Report;
    // Identity, the constructor link and attributes are as if it always existed.
    check(
        "function F() {} F.prototype === F.prototype && F.prototype.constructor === F",
        "true",
    );
    check(
        "function F() {} var o = new F(); o instanceof F && o.constructor === F",
        "true",
    );
    check(
        "function F() {} var keys = ''; for (var k in F) keys += k; keys + delete F.prototype",
        "false",
    );
    check(
        "'use strict'; function F() {} try { delete F.prototype } catch (e) { e.name }",
        "TypeError",
    );
    check(
        "function F() {} F.prototype = {m() { return 1 }}; new F().m()",
        "1",
    );
    check(
        "function F() {} F.x = 1; F.prototype.y = 2; new F().y + F.x",
        "3",
    );
    check(
        "function F() {} function get(f) { return f.prototype } get(F) === get(F) && get(F) === F.prototype",
        "true",
    );
    check(
        "function F() {} var p = F['proto' + 'type']; p === F.prototype && typeof p",
        "object",
    );
    check(
        "var a = () => 1, o = {m() {}}; typeof a.prototype + typeof o.m.prototype",
        "undefinedundefined",
    );
    check(
        "function F() {} var G = F; ({...F}).prototype === undefined && F.prototype !== undefined",
        "true",
    );
    // Forty extra properties push the function into dictionary mode first.
    check(
        "function F() {} for (var i = 0; i < 40; i++) F['p' + i] = i; var p = F.prototype; p === F.prototype && p.constructor === F && new F() instanceof F",
        "true",
    );
    // A function that never uses its prototype does not pay for one.
    let mut agent = Agent::new().expect("agent");
    agent
        .evaluate("var a = []; for (var i = 0; i < 1000; i++) a[i] = function () {}")
        .expect("runs");
    let lazy = agent.heap.object_count();
    agent
        .evaluate("for (var i = 0; i < 1000; i++) a[i].prototype")
        .expect("runs");
    assert_eq!(agent.heap.object_count(), lazy + 1000);
}

#[test]
fn closures_and_their_scopes_stay_small() {
    // The memory gate for functions: a closure with no captured variable,
    // and one that captures a variable in its own scope environment.
    // The bounds are the 64-bit sizes; 32-bit targets use less.
    let mut agent = Agent::new().expect("agent");
    agent
        .evaluate("var a = []; for (var i = 0; i < 2000; i++) a[i] = 0")
        .expect("runs");
    let before = agent.heap.bytes_used();
    agent
        .evaluate("for (var i = 0; i < 2000; i++) a[i] = function () {}")
        .expect("runs");
    let plain = (agent.heap.bytes_used() - before) / 2000;
    assert!(plain <= 120, "a closure costs {plain} bytes");

    let before = agent.heap.bytes_used();
    agent
        .evaluate("function mk(n) { return function () { return n } } for (var i = 0; i < 2000; i++) a[i] = mk(i)")
        .expect("runs");
    agent.collect_garbage(rustic_js::runtime::Roots::default());
    let capturing = agent.heap.bytes_used().saturating_sub(before) / 2000;
    assert!(
        capturing <= 80,
        "a capturing closure costs {capturing} bytes more than a plain one"
    );
}

#[test]
fn unsupported_syntax_is_reported_not_miscompiled() {
    for source in [
        "class A {}",
        "function* g() {}",
        "async function f() {}",
        "/x/",
        "tag`x`",
    ] {
        let mut agent = Agent::new().expect("agent");
        assert!(
            matches!(agent.evaluate(source), Err(EvalError::Compile(_))),
            "{source}"
        );
    }
}

/// An agent with a global `Symbol` holding the well-known symbols, which
/// the library (JS-05) will provide for real.
fn agent_with_symbols() -> Agent {
    use rustic_js::runtime::PropertyKey;
    let mut agent = Agent::new().expect("agent");
    let holder = agent.new_object().expect("object");
    let iterator = Value::symbol(agent.symbols.iterator);
    let has_instance = Value::symbol(agent.symbols.has_instance);
    let to_primitive = Value::symbol(agent.symbols.to_primitive);
    for (name, value) in [
        ("iterator", iterator),
        ("hasInstance", has_instance),
        ("toPrimitive", to_primitive),
    ] {
        let key = agent.heap.key(name).expect("key");
        agent
            .create_data_property_or_throw(holder, key, value)
            .expect("define");
    }
    let global = agent.intrinsics.global;
    let key: PropertyKey = agent.heap.key("Symbol").expect("key");
    agent
        .create_data_property_or_throw(global, key, Value::object(holder))
        .expect("define");
    agent
}

fn check_sym(source: &str, expected: &str) {
    let actual = std::panic::catch_unwind(|| run_in(&mut agent_with_symbols(), source))
        .unwrap_or_else(|_| "<panic>".into());
    if actual != expected {
        FAILURES.with_borrow_mut(|f| {
            f.push(format!(
                "{source}\n    expected: {expected}\n    actual:   {actual}"
            ));
        });
    }
}

const COUNTER: &str = "var log = ''; function counter(n) { return {[Symbol.iterator]() { var i = 0; return {next() { log += 'n'; return i < n ? {value: i++, done: false} : {done: true} }, return() { log += 'R'; return {} }} }} } ";

#[test]
fn iteration_protocol() {
    let _report = Report;
    let c = |body: &str| format!("{COUNTER}{body}");
    check_sym(
        &c("var s = 0; for (var v of counter(3)) s += v; s + log"),
        "3nnnn",
    );
    check_sym(
        &c("for (var v of counter(3)) { if (v == 1) break } log"),
        "nnR",
    );
    check_sym(
        &c("for (var v of counter(3)) { if (v == 1) continue } log"),
        "nnnn",
    );
    check_sym(
        &c("function f() { for (var v of counter(3)) return v } f() + log"),
        "0nR",
    );
    check_sym(
        &c("try { for (var v of counter(3)) throw 'x' } catch (e) { log += e } log"),
        "nRx",
    );
    check_sym(
        &c("outer: for (var a of counter(2)) for (var b of counter(2)) continue outer; log"),
        "nnRnnRn",
    );
    check_sym(
        &c("outer: for (var a of counter(2)) for (var b of counter(2)) break outer; log"),
        "nnRR",
    );
    check_sym(&c("var [a, b] = counter(5); a + b + log"), "1nnR");
    check_sym(&c("var [a, ...r] = counter(3); r.length + log"), "2nnnn");
    check_sym(&c("var [a, b, c] = counter(2); c + log"), "undefinednnn");
    check_sym(&c("var a = [...counter(2), 9]; a.length + log"), "3nnn");
    check_sym(
        &c(
            "function f() { try { for (var v of counter(3)) return 'r' } finally { log += 'F' } } f() + log",
        ),
        "rnRF",
    );
    check_sym("for (var x of 5) {}", "!TypeError: value is not iterable");
    check_sym(
        "var it = {[Symbol.iterator]() { return 1 }}; for (var x of it) {}",
        "!TypeError: iterator is not an object",
    );
    check_sym(
        "var a = [1, 2, 3]; var s = 0; for (var v of a) { if (v == 1) a[a.length] = 10; s += v } s",
        "16",
    );
    check_sym(
        "var a = [1, 2]; var it = a[Symbol.iterator](); it.next().value + it.next().value + '' + it.next().done + it.next().done",
        "3truetrue",
    );
    check_sym(
        "var it = [7].values(); it.next = function () { return {done: true} }; var n = 0; for (var v of it) n++; n",
        "!TypeError: value is not iterable",
    );
    check_sym(
        "var o = {[Symbol.hasInstance](v) { return v === 1 }}; (1 instanceof o) + ' ' + (2 instanceof o)",
        "true false",
    );
    check_sym(
        "var o = {[Symbol.toPrimitive](hint) { return hint }}; `${o}` + (o + '') + (+o !== +o)",
        "stringdefaulttrue",
    );
}

#[test]
fn evaluation_order_and_tricky_scoping() {
    let _report = Report;
    check("var a = 1; a + (a = 5)", "6");
    check("var a = 1; (a = 5) + a", "10");
    check("var a = 1; a += a++; a", "2");
    check("var a = 1; a = a++ + a; a", "3");
    check("function f(a) { return a + (a = 2) + a } f(1)", "5");
    check(
        "function f(a, b) { return [a, a = b, a].length + a } f(1, 2)",
        "5",
    );
    check("var o = {x: 1}; o.x += (o = {x: 10}, 5); o.x", "10");
    check(
        "var log = ''; function t(n) { log += n; return n } t(1) + t(2) * t(3); log",
        "123",
    );
    check(
        "var log = ''; var o = {get a() { log += 'a'; return {} }}; function k() { log += 'k'; return 'p' } o.a[k()] = (log += 'v'); log",
        "akv",
    );
    check(
        "var fs = []; for (let i = 0; i < 3; i++) { fs[i] = () => i; i += 0 } fs[0]() + '' + fs[1]() + fs[2]()",
        "012",
    );
    check(
        "var fs = []; for (let i = 0; i < 3; i++) { fs[i] = () => i++ } fs[0](); fs[0]() + '' + fs[2]()",
        "12",
    );
    check(
        "var fs = []; for (let k in {a: 1, b: 2}) fs[fs.length] = () => k; fs[0]() + fs[1]()",
        "ab",
    );
    check("var f; { let x = 1; f = () => x; x = 2 } f()", "2");
    check("var f = () => t; let t = 7; f()", "7");
    check(
        "var f = () => t; try { f() } catch (e) { e.name } let t = 7;",
        "ReferenceError",
    );
    check(
        "function f() { return typeof t; let t } try { f() } catch (e) { e.name }",
        "ReferenceError",
    );
    check("function f(a, b = () => a) { a = 2; return b() } f(1)", "2");
    check(
        "var f = function g() { g = 1; return typeof g }; f()",
        "function",
    );
    check(
        "var f = function g() { 'use strict'; g = 1 }; try { f() } catch (e) { e.name }",
        "TypeError",
    );
    check("var f = function g() { var g = 2; return g }; f()", "2");
    check("function f() { return (() => arguments[0])() } f(8)", "8");
    check("var t = () => this === globalThis; t()", "true");
    check("var x = 1; delete x", "false");
    check("y = 1; delete y; typeof y", "undefined");
    check("function f() { var v; return delete v } f()", "false");
    check(
        "switch (1) { case 1: let z = 5; var g = () => z; } g()",
        "5",
    );
    check(
        "var x = 'outer'; function f() { return x; var x = 'inner' } f()",
        "undefined",
    );
    check(
        "var s = 0; for (var i = 0, j = 10; i < j; i++, j--) s++; s",
        "5",
    );
    check(
        "var r = ''; a: for (;;) { try { break a } finally { r += 'f' } } r",
        "f",
    );
    check(
        "function f() { a: try { return 1 } finally { break a } return 2 } f()",
        "2",
    );
    check(
        "function f() { for (var i = 0; i < 2; i++) { try { continue } finally { if (i == 1) return 'done' } } } f()",
        "done",
    );
    check(
        "function f() { try { try { throw 1 } finally { throw 2 } } catch (e) { return e } } f()",
        "2",
    );
    check(
        "var o = {valueOf() { throw 'v' }}; function f() { try { return o + 1 } catch (e) { return e } } f() + f()",
        "vv",
    );
    check(
        "var o = {get g() { throw 'g' }}; var n = 0; for (var i = 0; i < 50; i++) { try { o.g } catch (e) { n++ } } n",
        "50",
    );
    check("var o = {toString() { return 's' }}; o + 1 + `${o}`", "s1s");
    check(
        "var k = {toString() { return 'key' }}; var o = {[k]: 1}; o.key",
        "1",
    );
    check("var a = []; a[0] = 1; a['1'] = 2; a.length", "2");
    check(
        "var s = ''; var o = {b: 1, a: 2, 1: 3, 0: 4}; for (var k in o) s += k; s",
        "01ba",
    );
    check("(function () { return this })() === globalThis", "true");
    check(
        "var f = function () { return this.v }; var o = {v: 3, f}; (o.f)() + (0, o.f)() ",
        "NaN",
    );
}

#[test]
fn deep_and_wide_programs_do_not_overflow() {
    let _report = Report;
    // A long left-nested chain is compiled iteratively.
    let sum = format!("0{}", " + 1".repeat(20_000));
    check(&sum, "20000");
    let chain = format!("var o = {{}}; o.p = o; o{}.p === o", ".p".repeat(20_000));
    check(&chain, "true");
    // Deep right-nesting is refused by the parser's depth limit, not a
    // crash.
    let nested = format!("{}1{}", "(".repeat(5_000), ")".repeat(5_000));
    assert!(run(&nested).starts_with("!!Parse(TooDeep"));
    let calls = format!(
        "function f(x) {{ return x }} {}1{}",
        "f(".repeat(200),
        ")".repeat(200)
    );
    check(&calls, "1");
}

#[test]
fn host_calls_and_reentrancy() {
    let mut agent = Agent::new().expect("agent");
    assert_eq!(
        run_in(
            &mut agent,
            "function twice(f, x) { return f(f(x)) } function inc(x) { return x + 1 }"
        ),
        "undefined"
    );
    let global = agent.intrinsics.global;
    let twice_key = agent.heap.key("twice").expect("key");
    let inc_key = agent.heap.key("inc").expect("key");
    let twice = agent.get(global, twice_key).expect("twice");
    let inc = agent.get(global, inc_key).expect("inc");
    let result = agent
        .call_from_host(twice, Value::UNDEFINED, &[inc, Value::number(5.0)])
        .expect("call");
    assert_eq!(result, Value::number(7.0));
    // A thrown error leaves the interpreter usable.
    let thrown = agent.call_from_host(twice, Value::UNDEFINED, &[Value::number(1.0)]);
    assert!(matches!(thrown, Err(Abrupt::Throw(_))));
    assert_eq!(run_in(&mut agent, "twice(inc, 1)"), "3");
    // Construct from the host.
    assert_eq!(
        run_in(&mut agent, "function P(v) { this.v = v }"),
        "undefined"
    );
    let p_key = agent.heap.key("P").expect("key");
    let p = agent.get(global, p_key).expect("P");
    let made = agent
        .construct(p, &[Value::number(4.0)])
        .expect("construct");
    let v_key = agent.heap.key("v").expect("key");
    let v = agent
        .get(made.as_object().expect("object"), v_key)
        .expect("v");
    assert_eq!(v, Value::number(4.0));
}
