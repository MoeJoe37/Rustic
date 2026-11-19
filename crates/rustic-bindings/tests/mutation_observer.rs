//! MutationObserver records, options, delivery order and lifetime.

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use rustic_bindings::dom_exception;
use rustic_bindings::page::{self, PageTree};
use rustic_dom::{Document, DocumentLimits, QualifiedName};
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

fn page() -> (Agent, Rc<Tree>) {
    let mut document = Document::new(DocumentLimits::default()).unwrap();
    let html = document
        .create_element(QualifiedName::html("html").unwrap())
        .unwrap();
    let body = document
        .create_element(QualifiedName::html("body").unwrap())
        .unwrap();
    document.append_child(document.root(), html).unwrap();
    document.append_child(html, body).unwrap();
    let tree = Rc::new(Tree(RefCell::new(document)));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree.clone()).unwrap();
    agent.heap.set_gc_stress(true);
    (agent, tree)
}

/// Run `script`, then a checkpoint, then `check` (which throws on failure).
fn run(agent: &mut Agent, script: &str, check: &str) {
    agent.evaluate(script).unwrap();
    page::flush_mutation_records(agent).unwrap();
    agent.run_jobs().unwrap();
    if let Err(error) = agent.evaluate(check) {
        let message = agent
            .evaluate("String(globalThis.log)")
            .ok()
            .and_then(|value| value.as_string())
            .map(|id| String::from_utf16_lossy(&agent.heap.string(id).units().collect::<Vec<_>>()))
            .unwrap_or_default();
        panic!("check failed: {error:?}; log = {message}");
    }
}

const DESCRIBE: &str = r#"
    globalThis.log = [];
    globalThis.describe = (record) => {
        const name = (node) => node === null ? 'null'
            : node.nodeType === 1 ? node.localName : '#' + node.nodeType;
        let out = record.type + ':' + name(record.target);
        if (record.type === 'childList') {
            out += '+' + Array.from(record.addedNodes, name).join(',')
                + '-' + Array.from(record.removedNodes, name).join(',')
                + '<' + name(record.previousSibling) + '>' + name(record.nextSibling);
        } else if (record.type === 'attributes') {
            out += '@' + record.attributeName + ',' + record.attributeNamespace;
        }
        return out + '=' + record.oldValue;
    };
"#;

#[test]
fn records_arrive_in_one_microtask_before_later_promise_jobs() {
    let (mut agent, _tree) = page();
    run(
        &mut agent,
        &format!(
            "{DESCRIBE}
            const body = document.body;
            const observer = new MutationObserver(function (records, self) {{
                if (this !== observer || self !== observer) throw 'receiver';
                log.push(records.map(describe).join(' | '));
            }});
            observer.observe(body, {{ childList: true, attributes: true, attributeOldValue: true }});
            Promise.resolve().then(() => log.push('early promise'));
            const a = document.createElement('a');
            const b = document.createElement('b');
            body.appendChild(a);
            body.appendChild(b);
            body.setAttribute('id', 'one');
            body.setAttribute('id', 'two');
            body.removeChild(a);
            Promise.resolve().then(() => log.push('late promise'));
            log.push('sync');"
        ),
        r#"
            const expected = [
                'sync',
                'early promise',
                'childList:body+a-<null>null=null | childList:body+b-<a>null=null'
                    + ' | attributes:body@id,null=null | attributes:body@id,null=one'
                    + ' | childList:body+-a<null>b=null',
                'late promise',
            ];
            if (JSON.stringify(log) !== JSON.stringify(expected)) throw JSON.stringify(log);
        "#,
    );
}

#[test]
fn options_are_validated_in_dictionary_order() {
    let (mut agent, _tree) = page();
    run(
        &mut agent,
        r#"
            globalThis.log = [];
            const observer = new MutationObserver(() => {});
            const fails = (options) => {
                try { observer.observe(document.body, options); }
                catch (error) { return error instanceof TypeError; }
                return false;
            };
            if (!fails(undefined) || !fails({}) || !fails({ subtree: true })) throw 'empty';
            if (!fails({ attributes: false, attributeOldValue: true })) throw 'old value';
            if (!fails({ attributes: false, attributeFilter: [] })) throw 'filter';
            if (!fails({ characterData: false, characterDataOldValue: true })) throw 'data';
            if (!fails(3)) throw 'primitive';
            try { observer.observe({}, { childList: true }); throw 'node'; }
            catch (error) { if (!(error instanceof TypeError)) throw error; }
            const order = [];
            const options = {};
            for (const name of ['subtree', 'childList', 'characterDataOldValue',
                    'characterData', 'attributes', 'attributeOldValue', 'attributeFilter']) {
                Object.defineProperty(options, name, {
                    get() { order.push(name); return name === 'attributeFilter' ? ['x'] : undefined; },
                });
            }
            observer.observe(document.body, options);
            if (order.join() !== 'attributeFilter,attributeOldValue,attributes,characterData,'
                + 'characterDataOldValue,childList,subtree') throw order.join();
            try { new MutationObserver(1); throw 'callable'; }
            catch (error) { if (!(error instanceof TypeError)) throw error; }
            try { MutationObserver(() => {}); throw 'new'; }
            catch (error) { if (!(error instanceof TypeError)) throw error; }
            try { MutationRecord.prototype.type; throw 'brand'; }
            catch (error) { if (!(error instanceof TypeError)) throw error; }
        "#,
        "",
    );
}

#[test]
fn subtree_filters_old_values_and_character_data() {
    let (mut agent, _tree) = page();
    run(
        &mut agent,
        &format!(
            "{DESCRIBE}
            const body = document.body;
            const div = document.createElement('div');
            const text = document.createTextNode('abc');
            div.appendChild(text);
            body.appendChild(div);
            const observer = new MutationObserver((records) => {{
                log.push(...records.map(describe));
            }});
            observer.observe(body, {{ subtree: true, attributeFilter: ['keep'],
                characterDataOldValue: true }});
            div.setAttribute('keep', '1');
            div.setAttribute('drop', '1');
            div.setAttributeNS('urn:x', 'keep', '2');
            text.data = 'xyz';
            text.appendData('!');
            div.appendChild(document.createElement('ignored'));"
        ),
        r#"
            const expected = [
                'attributes:div@keep,null=null',
                'characterData:#3=abc',
                'characterData:#3=xyz',
            ];
            if (JSON.stringify(log) !== JSON.stringify(expected)) throw 'records';
        "#,
    );
}

#[test]
fn observing_again_replaces_options_and_take_records_empties_the_queue() {
    let (mut agent, _tree) = page();
    run(
        &mut agent,
        &format!(
            "{DESCRIBE}
            const body = document.body;
            const observer = new MutationObserver(() => log.push('callback'));
            observer.observe(body, {{ attributes: true }});
            observer.observe(body, {{ childList: true }});
            body.setAttribute('a', '1');
            body.appendChild(document.createElement('p'));
            const taken = observer.takeRecords();
            if (!Array.isArray(taken) || taken.length !== 1 ||
                describe(taken[0]) !== 'childList:body+p-<null>null=null') throw 'taken';
            if (observer.takeRecords().length !== 0) throw 'empty';
            body.appendChild(document.createElement('q'));
            observer.disconnect();
            body.appendChild(document.createElement('r'));
            if (observer.takeRecords().length !== 0) throw 'disconnected';
            observer.observe(body, {{ childList: true }});
            body.appendChild(document.createElement('s'));"
        ),
        r#"if (log.join() !== 'callback') throw log.join();"#,
    );
}

#[test]
fn removed_nodes_stay_observed_until_the_next_notification() {
    let (mut agent, _tree) = page();
    run(
        &mut agent,
        &format!(
            "{DESCRIBE}
            const body = document.body;
            const outer = document.createElement('div');
            const inner = document.createElement('span');
            outer.appendChild(inner);
            body.appendChild(outer);
            const observer = new MutationObserver((records) => {{
                log.push(records.map(describe).join(' | '));
            }});
            observer.observe(body, {{ subtree: true, attributes: true, childList: true }});
            body.removeChild(outer);
            inner.setAttribute('late', 'yes');
            Promise.resolve().then(() => inner.setAttribute('after', 'yes'));"
        ),
        r#"
            const expected = [
                'childList:body+-div<null>null=null | attributes:span@late,null=null',
            ];
            if (JSON.stringify(log) !== JSON.stringify(expected)) throw 'transient';
        "#,
    );
}

#[test]
fn observers_run_in_creation_order_and_errors_do_not_stop_delivery() {
    let (mut agent, _tree) = page();
    run(
        &mut agent,
        r#"
            globalThis.log = [];
            const body = document.body;
            const first = new MutationObserver(() => { log.push('first'); throw new Error('x'); });
            const second = new MutationObserver(() => {
                log.push('second');
                // A record queued during delivery is delivered in a new microtask.
                if (!globalThis.again) { globalThis.again = true; body.setAttribute('again', ''); }
            });
            // Still pending in the first round, it takes both records at once.
            const third = new MutationObserver((records) => log.push('third' + records.length));
            third.observe(body, { attributes: true });
            second.observe(body, { attributes: true });
            first.observe(body, { attributes: true });
            body.setAttribute('x', '');
        "#,
        r#"if (log.join() !== 'first,second,third2,first,second') throw log.join();"#,
    );
    assert_eq!(page::take_listener_error_count(&agent), 2);
}

#[test]
fn observers_survive_collection_through_their_nodes_and_follow_adoption() {
    let (mut agent, _tree) = page();
    run(
        &mut agent,
        &format!(
            "{DESCRIBE}
            const other = document.implementation.createHTMLDocument('');
            const moved = other.createElement('section');
            other.body.appendChild(moved);
            (() => {{
                new MutationObserver((records) => log.push('source ' + records.map(describe).join(' | ')))
                    .observe(other.body, {{ childList: true }});
                new MutationObserver((records) => log.push('moved ' + records.map(describe).join(' | ')))
                    .observe(moved, {{ attributes: true }});
            }})();
            // Only the observed nodes hold the observers now.
            for (let i = 0; i < 50; i++) document.createElement('garbage');
            document.body.appendChild(moved);
            moved.setAttribute('here', '1');"
        ),
        r#"
            const expected = [
                'source childList:body+-section<null>null=null',
                'moved attributes:section@here,null=null',
            ];
            if (JSON.stringify(log) !== JSON.stringify(expected)) throw 'adoption';
        "#,
    );
}

#[test]
fn text_content_and_host_edits_produce_records() {
    let (mut agent, tree) = page();
    agent
        .evaluate(&format!(
            "{DESCRIBE}
            const body = document.body;
            body.appendChild(document.createElement('old'));
            new MutationObserver((records) => log.push(...records.map(describe)))
                .observe(document.documentElement, {{ childList: true, subtree: true }});
            body.textContent = 'new';"
        ))
        .unwrap();
    // A host edit outside any binding call is flushed before the checkpoint.
    {
        let mut document = tree.0.borrow_mut();
        let html = document
            .node(document.root())
            .unwrap()
            .first_child()
            .unwrap();
        let footer = document
            .create_element(QualifiedName::html("footer").unwrap())
            .unwrap();
        document.append_child(html, footer).unwrap();
    }
    page::flush_mutation_records(&mut agent).unwrap();
    agent.run_jobs().unwrap();
    agent
        .evaluate(
            r#"
            const expected = [
                'childList:body+#3-old<null>null=null',
                'childList:html+footer-<body>null=null',
            ];
            if (JSON.stringify(log) !== JSON.stringify(expected)) throw JSON.stringify(log);
        "#,
        )
        .unwrap();
}

#[test]
fn queue_microtask_runs_in_order_and_reports_exceptions() {
    let (mut agent, _tree) = page();
    run(
        &mut agent,
        r#"
            globalThis.log = [];
            queueMicrotask(() => log.push(1));
            Promise.resolve().then(() => log.push(2));
            queueMicrotask(() => { log.push(3); throw new Error('reported'); });
            queueMicrotask(function () { 'use strict'; log.push(this === undefined ? 4 : 'this'); });
            try { queueMicrotask(1); throw 'callable'; }
            catch (error) { if (!(error instanceof TypeError)) throw error; }
            if (queueMicrotask.length !== 1 || queueMicrotask(() => {}) !== undefined) throw 'shape';
        "#,
        r#"if (log.join() !== '1,2,3,4') throw log.join();"#,
    );
    assert_eq!(page::take_listener_error_count(&agent), 1);
}
