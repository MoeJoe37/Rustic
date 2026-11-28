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

#[test]
fn observing_many_short_lived_nodes_does_not_retain_their_registrations() {
    // TD-150: an observer that observes nodes which then die keeps weak
    // references only, compacted as the list grows, so its footprint does
    // not grow with every node it ever observed.
    let (mut agent, _tree) = page();
    agent
        .evaluate(
            "globalThis.observer = new MutationObserver(() => {});
             globalThis.churn = (n) => {
                 for (let i = 0; i < n; i++) {
                     observer.observe(document.createElement('p'), { attributes: true });
                 }
             };
             churn(100);",
        )
        .unwrap();
    let after_warmup = agent.heap.bytes_used();
    agent.evaluate("churn(3000)").unwrap();
    let after_churn = agent.heap.bytes_used();
    let growth = after_churn.saturating_sub(after_warmup);
    assert!(
        growth < 64 * 1024,
        "observer state grew by {growth} bytes for 3,000 dead nodes"
    );
    // Disconnecting still stops delivery for a node that is alive.
    run(
        &mut agent,
        "globalThis.log = [];
         const kept = document.createElement('div');
         document.body.appendChild(kept);
         const seen = new MutationObserver(() => log.push('seen'));
         seen.observe(kept, { attributes: true });
         seen.disconnect();
         kept.setAttribute('a', '1');",
        "if (log.length !== 0) throw 'delivered after disconnect';",
    );
}

#[test]
fn shadow_slots_update_and_signal_after_observers_in_the_same_checkpoint() {
    let (mut agent, _) = page();
    run(
        &mut agent,
        r#"
        let host = document.createElement('div');
        document.body.append(host);
        let root = host.attachShadow({mode: 'open', clonable: true});
        let slot = document.createElement('slot');
        let second = document.createElement('slot');
        slot.name = second.name = 'named';
        root.append(slot, second);
        let fallback = document.createElement('b');
        slot.append(fallback);
        if (slot.assignedNodes().length || slot.assignedElements({flatten:true})[0] !== fallback)
            throw 'fallback';
        let light = document.createElement('i');
        light.slot = 'named';
        let signals = [];
        root.addEventListener('slotchange', event => {
            if (!event.isTrusted || event.composed || !event.bubbles || event.target !== slot)
                throw 'slot event';
            signals.push('slot');
        });
        new MutationObserver(() => signals.push('observer')).observe(host, {childList:true});
        host.append(light);
        if (slot.assignedElements()[0] !== light || second.assignedElements().length || light.assignedSlot !== slot)
            throw 'named assignment';
        if (light.parentNode !== host || !fallback.isConnected ||
            fallback.getRootNode() !== root || fallback.getRootNode({composed:true}) !== document)
            throw 'shadow ancestry';
        const clone = host.cloneNode();
        if (clone.shadowRoot === root || clone.shadowRoot.firstChild.name !== 'named' ||
            clone.childNodes.length || root.host !== host || !root.clonable ||
            !(root instanceof DocumentFragment) || !(slot instanceof HTMLSlotElement))
            throw 'shadow clone or interfaces';
        "#,
        "if (signals.join() !== 'observer,slot') throw signals.join();",
    );
}

#[test]
fn manual_slot_assignment_is_ordered_unique_and_closed_roots_hide_the_slot() {
    let (mut agent, _) = page();
    agent
        .evaluate(
            r#"
        const host = document.createElement('span');
        const shadow = host.attachShadow({mode: 'closed', slotAssignment: 'manual'});
        const slot = document.createElement('slot');
        const other = document.createElement('slot');
        shadow.append(slot, other);
        const a = document.createElement('a');
        const text = document.createTextNode('text');
        slot.assign(text, a, text);
        host.append(a, text);
        if (host.shadowRoot !== null || a.assignedSlot !== null || text.assignedSlot !== null ||
            slot.assignedNodes().length !== 2 || slot.assignedNodes()[0] !== text ||
            slot.assignedElements()[0] !== a) throw 'manual assignment';
        other.assign(text);
        if (slot.assignedNodes()[0] !== a || other.assignedNodes()[0] !== text) throw 'reassign';
        text.remove();
        if (other.assignedNodes().length) throw 'detached';
        host.append(text);
        if (other.assignedNodes()[0] !== text) throw 'reinserted';
    "#,
        )
        .unwrap();
}

#[test]
fn autonomous_custom_element_definition_upgrades_and_runs_lifecycle_callbacks() {
    let (mut agent, _) = page();
    run(
        &mut agent,
        r#"
        let events = [];
        const node = document.createElement('rustic-widget');
        node.setAttribute('watched', 'initial');
        document.body.appendChild(node);
        const pending = customElements.whenDefined('rustic-widget');
        let resolved;
        pending.then(value => resolved = value);
        class Widget extends HTMLElement {
            static get observedAttributes() { return ['watched']; }
            constructor() { super(); events.push('construct'); }
            connectedCallback() { events.push('connected'); }
            disconnectedCallback() { events.push('disconnected'); }
            attributeChangedCallback(name, oldValue, value, namespace) {
                events.push([name, oldValue, value, namespace].join(':'));
            }
        }
        customElements.define('rustic-widget', Widget);
        if (!(node instanceof Widget) || customElements.get('rustic-widget') !== Widget ||
            customElements.getName(Widget) !== 'rustic-widget') throw 'upgrade';
        node.setAttribute('unwatched', 'skip');
        node.setAttribute('watched', 'change');
        node.remove();
        const fresh = document.createElement('rustic-widget');
        if (!(fresh instanceof Widget) || !(new Widget() instanceof Widget)) throw 'construction';
        document.body.append(fresh);
        "#,
        r#"
        if (resolved !== Widget || events.join() !==
            'construct,watched::initial:,connected,watched:initial:change:,disconnected,construct,construct,connected')
            throw events.join();
        "#,
    );
}

#[test]
fn upgraded_elements_keep_their_wrapper_and_receive_old_and_new_documents_on_adoption() {
    let (mut agent, _) = page();
    agent
        .evaluate(
            r#"
        const other = document.implementation.createHTMLDocument();
        const events = [];
        class AdoptedWidget extends HTMLElement {
            adoptedCallback(before, after) {
                if (this.ownerDocument !== after) throw 'wrong owner';
                events.push([before, after]);
            }
        }
        customElements.define('adopted-widget', AdoptedWidget);
        const widget = document.createElement('adopted-widget');
        if (other.adoptNode(widget) !== widget) throw 'wrapper replaced';
        document.body.append(widget);
        if (!(widget instanceof AdoptedWidget) || events.length !== 2 ||
            events[0][0] !== document || events[0][1] !== other ||
            events[1][0] !== other || events[1][1] !== document)
            throw 'adopting steps';
    "#,
        )
        .unwrap();
}

#[test]
fn customized_built_ins_preserve_their_name_prototype_and_creation_identity() {
    let (mut agent, _) = page();
    agent
        .evaluate(
            r#"
        const early = document.createElement('button', {is:'x-button'});
        const ordinary = document.createElement('button');
        ordinary.setAttribute('is', 'x-button');
        early.setAttribute('is', 'something-else');
        document.body.append(early, ordinary);
        let connections = 0;
        class CustomButton extends HTMLButtonElement {
            connectedCallback() { connections++; }
        }
        customElements.define('x-button', CustomButton, {extends:'button'});
        if (!(early instanceof CustomButton) || ordinary instanceof CustomButton ||
            early !== document.body.firstChild || connections !== 1)
            throw 'late upgrade';
        const direct = new CustomButton();
        const made = document.createElement('button', {is:'x-button'});
        const namespaced = document.createElementNS(
            'http://www.w3.org/1999/xhtml', 'button', {is:'x-button'});
        for (const node of [direct, made, namespaced]) {
            if (node.localName !== 'button' || !(node instanceof HTMLButtonElement) ||
                !(node instanceof CustomButton)) throw 'constructed base';
        }
        if (document.createElement('span', {is:'x-button'}) instanceof CustomButton)
            throw 'wrong local name';
        class WrongBase extends HTMLElement {}
        customElements.define('x-wrong', WrongBase, {extends:'button'});
        try {
            new WrongBase();
            throw 'wrong native base accepted';
        } catch (error) {
            if (!(error instanceof TypeError)) throw error;
        }
    "#,
        )
        .unwrap();
}

#[test]
fn scoped_registries_initialize_null_roots_and_isolate_definitions() {
    let (mut agent, _) = page();
    run(
        &mut agent,
        r#"
        const red = new CustomElementRegistry();
        const blue = new CustomElementRegistry();
        const host = document.createElement('div');
        document.body.append(host);
        const shadow = host.attachShadow({mode:'open', customElementRegistry:null});
        const first = document.createElement('x-scoped', {customElementRegistry:null});
        shadow.appendChild(first);
        const pending = red.whenDefined('x-scoped');
        let resolved;
        pending.then(ctor => resolved = ctor);
        class Red extends HTMLElement {}
        class Blue extends HTMLElement {}
        class Global extends HTMLElement {}
        red.define('x-scoped', Red);
        blue.define('x-scoped', Blue);
        customElements.define('x-scoped', Global);
        if (shadow.customElementRegistry !== null || first instanceof Global)
            throw 'null registry exposed the global definition';
        red.initialize(shadow);
        if (shadow.customElementRegistry !== red || first.customElementRegistry !== red ||
            !(first instanceof Red) || first instanceof Blue || first instanceof Global)
            throw 'initialization or definition isolation';
        const second = document.createElement('x-scoped', {customElementRegistry:blue});
        shadow.appendChild(second);
        if (!(second instanceof Blue) || second.customElementRegistry !== blue ||
            !(document.createElement('x-scoped') instanceof Global) ||
            document.customElementRegistry !== customElements)
            throw 'element creation registry';
        document.body.appendChild(first);
        if (first.customElementRegistry !== red || !(first instanceof Red))
            throw 'moving changed an assigned registry';
        try {
            red.define('x-special', class extends HTMLButtonElement {}, {extends:'button'});
            throw 'scoped extends was accepted';
        } catch (error) {
            if (error.name !== 'NotSupportedError') throw error;
        }
        try {
            new Red();
            throw 'scoped construction outside its registry was accepted';
        } catch (error) {
            if (!(error instanceof TypeError)) throw error;
        }
    "#,
        "if (resolved !== Red || blue.get('x-scoped') !== Blue) throw 'registry promise';",
    );
}

#[test]
fn scoped_registry_assignments_survive_clone_and_import() {
    let (mut agent, _) = page();
    agent
        .evaluate(
            r#"
        const scope = new CustomElementRegistry();
        class Widget extends HTMLElement {}
        scope.define('x-scoped-copy', Widget);
        const host = document.createElement('div');
        const root = host.attachShadow({mode:'open', clonable:true, customElementRegistry:scope});
        const scoped = document.createElement('x-scoped-copy', {customElementRegistry:scope});
        root.append(scoped);
        const shadowCopy = host.cloneNode().shadowRoot;
        if (shadowCopy.customElementRegistry !== scope ||
            shadowCopy.firstChild.customElementRegistry !== scope ||
            !(shadowCopy.firstChild instanceof Widget) || shadowCopy.firstChild === scoped)
            throw 'clonable shadow assignment';
        const other = document.implementation.createHTMLDocument();
        const wrapper = other.createElement('div');
        const unset = other.createElement('x-scoped-copy');
        const explicitNull = other.createElement('x-scoped-copy', {customElementRegistry:null});
        wrapper.append(unset, explicitNull);
        const imported = document.importNode(wrapper, {customElementRegistry:scope});
        if (!(imported.firstChild instanceof Widget) ||
            imported.firstChild.customElementRegistry !== scope ||
            imported.lastChild.customElementRegistry !== null ||
            imported.lastChild instanceof Widget)
            throw 'fallback registry changed an explicit null or missed an unset registry';
        if (document.importNode(wrapper, {selfOnly:true, customElementRegistry:scope}).firstChild)
            throw 'selfOnly';
    "#,
        )
        .unwrap();
}
