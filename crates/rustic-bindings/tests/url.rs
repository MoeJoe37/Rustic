use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use rustic_bindings::page::{self, PageTree};
use rustic_dom::{Document, DocumentLimits};
use rustic_js::EvalError;
use rustic_js::runtime::Agent;

#[derive(Debug)]
struct Tree(RefCell<Document>);

impl PageTree for Tree {
    fn read(&self) -> Option<Ref<'_, Document>> {
        self.0.try_borrow().ok()
    }

    fn write(&self) -> Option<RefMut<'_, Document>> {
        self.0.try_borrow_mut().ok()
    }
}

fn run(script: &str) {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    match agent.evaluate(script) {
        Ok(_) => {}
        Err(EvalError::Throw(value)) if value.as_string().is_some() => {
            panic!(
                "{}",
                agent.heap.string(value.as_string().unwrap()).to_text()
            )
        }
        Err(error) => panic!("{error:?}"),
    }
}

#[test]
fn pinned_url_setters_pass_through_the_window_bindings() {
    // WPT cb97c571c3b1947e08b8615eeb4028a2794b23a4, url/resources/setters_tests.json.
    let cases = include_str!("fixtures/url/setters_tests.json");
    run(&format!(
        r#"
        const tests = {cases};
        let total = 0;
        for (const [property, cases] of Object.entries(tests)) {{
            if (property === 'comment') continue;
            for (const test of cases) {{
                const url = new URL(test.href);
                url[property] = test.new_value;
                for (const [key, value] of Object.entries(test.expected)) {{
                    if (url[key] !== value) throw JSON.stringify([property, key, test, url[key]]);
                }}
                total += 1;
            }}
        }}
        if (total !== 278) throw `Unexpected WPT URL setter count ${{total}}`;
    "#
    ));
}

#[test]
fn url_setters_keep_the_linked_params_object_and_follow_parser_overrides() {
    run(r#"
        function eq(a, b) { if (a !== b) throw `${a} != ${b}`; }
        function fails(f) {
            try { f(); } catch (e) { if (e instanceof TypeError) return; throw e; }
            throw 'missing TypeError';
        }
        class SubURL extends URL {}
        const url = new SubURL('../resource?q=a%20b&x=1&x=2', 'http://example.test:443/dir/a');
        eq(url instanceof SubURL, true);
        eq(url.href, 'http://example.test:443/resource?q=a%20b&x=1&x=2');
        const params = url.searchParams;
        eq(params.get('q'), 'a b');
        eq(params.getAll('x').join(), '1,2');
        eq(params.size, 3);
        url.protocol = 'HTTPS:ignored';
        eq(url.port, '');
        url.username = 'a:b';
        url.password = '密';
        url.host = '[::1]:8443x/ignored';
        eq(url.host, '[::1]:8443');
        eq(url.origin, 'https://[::1]:8443');
        url.hostname = 'ignored.test:123';
        eq(url.hostname, '[::1]');
        url.host = 'new.test:65536';
        eq(url.host, 'new.test:8443');
        url.pathname = 'a\\..\\b?literal#path';
        eq(url.pathname, '/b%3Fliteral%23path');
        url.search = '?x=one two';
        url.hash = '# trailing ';
        eq(url.search, '?x=one%20two');
        eq(url.hash, '#%20trailing%20');
        eq(params.get('x'), 'one two');
        params.append('y', '~!');
        eq(url.search, '?x=one+two&y=%7E%21');
        eq(url.searchParams, params);
        eq(URL.canParse('https://x.test', 'invalid base'), false);
        eq(URL.canParse('/r', 'https://x.test'), true);
        eq(URL.parse('/r', 'https://x.test').href, 'https://x.test/r');
        eq(URL.parse('/r'), null);
        fails(() => { url.href = '/relative'; });
        fails(() => URL());
        fails(() => URL.prototype.toString.call(Object.create(URL.prototype)));
        fails(() => new URL('https://x.test', {toString() { throw new TypeError('base'); }}));
        url.href = 'foo:opaque?y=3';
        eq(url.toJSON(), 'foo:opaque?y=3');
        eq(url.origin, 'null');
        eq(url.searchParams, params);
        eq(params.get('y'), '3');
        url.pathname = '/unchanged';
        eq(url.pathname, 'opaque');
        url.search = '?';
        eq(url.search, '');
        eq(url.href, 'foo:opaque?');
        params.sort();
        eq(url.href, 'foo:opaque');
        eq(params.size, 0);
    "#);
}

#[test]
fn search_params_conversion_sort_and_iteration_keep_pair_order_and_live_state() {
    run(r#"
        function eq(a, b) { if (a !== b) throw `${a} != ${b}`; }
        const fromRecord = new URLSearchParams({b: 2, a: null, 1: 'space &', [Symbol()]: 5});
        eq(String(fromRecord), '1=space+%26&b=2&a=null');
        const fromString = new URLSearchParams('?empty&&key&bad=%ZZ&plus=%2B+');
        eq(String(fromString), 'empty=&key=&bad=%25ZZ&plus=%2B+');
        let trace = '';
        const input = {
            get [Symbol.iterator]() {
                trace += 'i';
                return function* () { yield ['x', {toString() { trace += 'v'; return 1; }}]; };
            }
        };
        const fromSequence = new URLSearchParams(input);
        eq(trace, 'iv');
        eq(fromSequence.get('x'), '1');
        const params = new URLSearchParams([['\uE000', 0], ['🐈', 1], ['🐈', 2]]);
        params.sort();
        eq([...params.keys()].join(), '🐈,🐈,\uE000');
        eq(params.has('🐈', 2), true);
        params.delete('🐈', 1);
        eq(params.get('🐈'), '2');
        params.set('🐈', 'cat');
        eq([...params.values()].join(), 'cat,0');
        eq(params[Symbol.iterator], params.entries);
        const iter = params.entries();
        eq(iter.next().value.join(), '🐈,cat');
        params.set('\uE000', '+');
        eq(iter.next().value.join(), '\uE000,+');
        eq(iter.next().done, true);
        params.append('later', 7);
        eq(iter.next().value.join(), 'later,7');
        let called = '';
        params.forEach(function(value, name, receiver) {
            eq(this, input); eq(receiver, params);
            called += value;
            if (name === '🐈') params.delete('\uE000');
        }, input);
        eq(called, 'cat7');
        let url = new URL('https://x.test/?a=1');
        const stable = url.searchParams;
        stable.append({toString() { url.search = '?reentrant=2'; return 'outer'; }}, 3);
        eq(url.search, '?reentrant=2&outer=3');
        url = null;
        eq(stable.get('outer'), '3');
        eq(new URLSearchParams(stable).toString(), stable.toString());
        try { new URLSearchParams([['bad']]); throw 'missing TypeError'; }
        catch (e) { if (!(e instanceof TypeError)) throw e; }
    "#);
}
