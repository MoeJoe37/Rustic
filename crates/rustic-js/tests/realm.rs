//! JS-06: realms. An agent can hold several realms, each with its own
//! global object and intrinsics; code runs in the realm of its function,
//! and the agent's symbols are shared.

use rustic_js::EvalError;
use rustic_js::runtime::{Agent, RealmId, Roots, Value};

fn text(agent: &mut Agent, source: &str) -> String {
    match agent.evaluate(source) {
        Ok(value) => agent.display(value),
        Err(EvalError::Throw(value)) => {
            let s = agent.to_string(value).expect("a printable error");
            format!("uncaught {}", agent.heap.string(s).to_text())
        }
        Err(other) => format!("error: {other}"),
    }
}

/// An agent whose global `other` is the global object of a second realm.
fn two_realms() -> (Agent, RealmId) {
    let mut agent = Agent::new().expect("agent");
    let realm = agent.create_realm().expect("a second realm");
    let other = agent.realm_global(realm);
    let global = agent.intrinsics.global;
    let key = agent.heap.key("other").expect("key");
    agent
        .create_data_property_or_throw(global, key, Value::object(other))
        .expect("global.other");
    (agent, realm)
}

#[test]
fn each_realm_has_its_own_global_and_intrinsics() {
    let (mut agent, realm) = two_realms();
    assert_eq!(agent.realm_count(), 2);
    assert_eq!(agent.current_realm(), RealmId::MAIN);
    assert_ne!(agent.realm_global(realm), agent.intrinsics.global);
    let source = "[other !== globalThis, other.globalThis === other, other.Array !== Array,
        other.Object.prototype !== Object.prototype, typeof other.other,
        Object.getPrototypeOf(other) === other.Object.prototype].join()";
    assert_eq!(
        text(&mut agent, source),
        "true,true,true,true,undefined,true"
    );
}

#[test]
fn symbols_and_the_symbol_registry_belong_to_the_agent() {
    let (mut agent, _) = two_realms();
    let source = "[other.Symbol.iterator === Symbol.iterator,
        other.Symbol.for('k') === Symbol.for('k'),
        other.Symbol.keyFor(Symbol.for('k')),
        other.Symbol !== Symbol].join()";
    assert_eq!(text(&mut agent, source), "true,true,k,true");
}

#[test]
fn a_function_runs_in_the_realm_it_was_created_in() {
    let (mut agent, _) = two_realms();
    // Literals, wrappers, the global `this` and errors come from the
    // callee's realm, and the caller's realm is back afterwards.
    let source = "var f = other.eval('(function () { return [[], this, function () {}] })');
        var made = f();
        var caught; try { other.eval('null.x') } catch (e) { caught = e }
        [Object.getPrototypeOf(made[0]) === other.Array.prototype,
         made[1] === other,
         Object.getPrototypeOf(made[2]) === other.Function.prototype,
         Object.getPrototypeOf(made[2].prototype) === other.Object.prototype,
         caught instanceof other.TypeError, caught instanceof TypeError,
         Object.getPrototypeOf([]) === Array.prototype].join()";
    assert_eq!(
        text(&mut agent, source),
        "true,true,true,true,true,false,true"
    );
    // Built-in functions too: `other.Array` makes arrays of its realm.
    let source = "[Object.getPrototypeOf(other.Array(1)) === other.Array.prototype,
        Object.getPrototypeOf(other.Object.create(null, {})) === null,
        other.Function('return this')() === other,
        other.eval('var leaked = 1; typeof leaked') + typeof leaked].join()";
    assert_eq!(text(&mut agent, source), "true,true,true,numberundefined");
}

#[test]
fn bound_functions_and_proxies_do_not_change_realm() {
    let (mut agent, _) = two_realms();
    // A revoked proxy made in the other realm throws the caller's error.
    let source = "var r = other.Proxy.revocable(function () {}, {}); r.revoke();
        var caught; try { r.proxy() } catch (e) { caught = e }
        var bound = other.Function.prototype.bind.call(function () { return [] });
        [caught instanceof TypeError, caught instanceof other.TypeError,
         Object.getPrototypeOf(bound()) === Array.prototype].join()";
    assert_eq!(text(&mut agent, source), "true,false,true");
}

#[test]
fn a_missing_prototype_falls_back_to_the_realm_of_the_new_target() {
    let (mut agent, _) = two_realms();
    // GetPrototypeFromConstructor (§10.1.14), through bound functions
    // and proxies (GetFunctionRealm, §7.3.24).
    let source = "var C = new other.Function(); C.prototype = null;
        var B = Function.prototype.bind.call(C);
        var P = new Proxy(C, {});
        var E = function () {}; E.prototype = 1;
        function proto(F, T) { return Object.getPrototypeOf(Reflect.construct(F, [], T)) }
        [proto(Map, C) === other.Map.prototype, proto(Array, B) === other.Array.prototype,
         proto(Error, P) === other.Error.prototype, proto(Date, C) === other.Date.prototype,
         proto(Promise.bind(null, function () {}), C) === other.Promise.prototype,
         proto(function () {}, C) === other.Object.prototype,
         proto(Uint8Array, C) === other.Uint8Array.prototype,
         proto(Map, E) === Map.prototype].join()";
    assert_eq!(
        text(&mut agent, source),
        "true,true,true,true,true,true,true,true"
    );
    let source = "var r = Proxy.revocable(C, {}); r.revoke();
        try { Reflect.construct(Map, [], r.proxy) } catch (e) { e instanceof TypeError }";
    assert_eq!(text(&mut agent, source), "true");
}

#[test]
fn array_species_ignores_the_array_constructor_of_another_realm() {
    let (mut agent, _) = two_realms();
    let source = "var a = new other.Array(1, 2, 3);
        var mapped = Array.prototype.map.call(a, function (x) { return x * 2 });
        var own = a.map(function (x) { return x });
        [Object.getPrototypeOf(mapped) === Array.prototype,
         Object.getPrototypeOf(own) === other.Array.prototype, mapped.join('+')].join()";
    assert_eq!(text(&mut agent, source), "true,true,2+4+6");
}

#[test]
fn generators_and_async_functions_resume_in_their_own_realm() {
    let (mut agent, _) = two_realms();
    let source = "var g = other.eval('(function* () { yield []; yield [] })')();
        var first = g.next().value, second = g.next().value;
        var out = [];
        other.eval('(async function () { await null; return [] })')()
            .then(function (v) { out.push(Object.getPrototypeOf(v) === other.Array.prototype) });
        [Object.getPrototypeOf(first) === other.Array.prototype,
         Object.getPrototypeOf(second) === other.Array.prototype,
         Object.getPrototypeOf(Object.getPrototypeOf(g)) ===
            other.eval('Object.getPrototypeOf(function* () {}.prototype)')].join()";
    assert_eq!(text(&mut agent, source), "true,true,true");
    agent.run_jobs().expect("jobs");
    assert_eq!(text(&mut agent, "out.join()"), "true");
}

#[test]
fn a_derived_constructor_reports_its_result_in_the_callers_realm() {
    let (mut agent, _) = two_realms();
    // §10.2.2 steps 11-14 run after the callee's context is gone.
    let source = "var C = other.eval('0, class extends Object { constructor() { return null } }');
        var D = other.eval('0, class extends Object { constructor() {} }');
        var a, b; try { new C() } catch (e) { a = e } try { new D() } catch (e) { b = e }
        [a instanceof TypeError, b instanceof ReferenceError,
         Object.getPrototypeOf([]) === Array.prototype].join()";
    assert_eq!(text(&mut agent, source), "true,true,true");
}

#[test]
fn the_host_can_run_script_in_any_realm() {
    let (mut agent, realm) = two_realms();
    let main = agent.enter_realm(realm);
    assert_eq!(main, RealmId::MAIN);
    assert_eq!(agent.current_realm(), realm);
    assert_eq!(
        text(&mut agent, "var where = 'second'; typeof other"),
        "undefined"
    );
    assert_eq!(agent.enter_realm(main), realm);
    assert_eq!(
        text(&mut agent, "typeof where + ' ' + other.where"),
        "undefined second"
    );
}

#[test]
fn realms_survive_collection_and_collection_at_every_safe_point() {
    let (mut agent, realm) = two_realms();
    let source = "var keep = other.eval('(function (n) { var a = []; for (var i = 0; i < n; i++) a.push({ i: i }); return a })');
        var total = 0;
        for (var round = 0; round < 20; round++) {
            var made = keep(50);
            total += made.length + (Object.getPrototypeOf(made) === other.Array.prototype ? 1 : 0);
        }
        total";
    let plain = text(&mut agent, source);
    assert_eq!(plain, "1020");
    agent.heap.set_gc_stress(true);
    assert_eq!(text(&mut agent, source), plain);
    agent.heap.set_gc_stress(false);
    // A realm nothing refers to from script is still there for the host.
    assert_eq!(text(&mut agent, "delete globalThis.other"), "true");
    agent.collect_garbage(Roots::default());
    let main = agent.enter_realm(realm);
    assert_eq!(
        text(
            &mut agent,
            "[1, 2, 3].map(function (x) { return x + 1 }).join()"
        ),
        "2,3,4"
    );
    agent.enter_realm(main);
    assert_eq!(text(&mut agent, "typeof other"), "undefined");
}

#[test]
fn a_realm_that_does_not_fit_leaves_the_agent_as_it_was() {
    let mut agent = Agent::new().expect("agent");
    let used = agent.heap.bytes_used();
    let mut small = Agent::with_heap_limit(used + used / 2).expect("agent");
    assert!(small.create_realm().is_err());
    assert_eq!(small.realm_count(), 1);
    assert_eq!(small.current_realm(), RealmId::MAIN);
    small.collect_garbage(Roots::default());
    assert_eq!(text(&mut small, "[1, 2].concat([3]).join()"), "1,2,3");
    // With room, the second realm costs about what the first did.
    let before = agent.heap.bytes_used();
    agent.create_realm().expect("a second realm");
    agent.collect_garbage(Roots::default());
    let cost = agent.heap.bytes_used() - before;
    assert!(
        cost <= used + used / 10,
        "{cost} bytes for a realm, {used} for the first"
    );
}
