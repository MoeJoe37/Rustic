use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use rustic_bindings::dom_exception;
use rustic_bindings::page::{self, PageTree};
use rustic_dom::{Document, DocumentLimits, QualifiedName};
use rustic_js::runtime::{Agent, Value};

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

#[test]
fn text_encoding_uses_web_idl_brands_and_exact_byte_ranges() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    agent.evaluate(r#"
        function assert(condition, message) { if (!condition) throw message; }
        function throws(type, f) {
            try { f(); } catch (error) { if (error instanceof type) return; throw error; }
            throw 'expected ' + type.name;
        }
        class Encoder extends TextEncoder {}
        const encoder = new Encoder();
        assert(encoder instanceof Encoder && encoder.encoding === 'utf-8', 'encoder brand');
        assert(Object.prototype.toString.call(encoder) === '[object TextEncoder]', 'tag');
        assert(encoder.encode().length === 0 && encoder.encode(undefined).length === 0, 'defaults');
        assert(encoder.encode(null).join() === '110,117,108,108', 'null conversion');
        assert(encoder.encode('aé𝄞\uD800b').join() === '97,195,169,240,157,132,158,239,191,189,98', 'USVString');
        const bytes = new Uint8Array(10).fill(255);
        const result = encoder.encodeInto('a𝄞é', bytes.subarray(2, 8));
        assert(result.read === 3 && result.written === 5, 'full scalars and UTF-16 counts');
        assert(bytes.join() === '255,255,97,240,157,132,158,255,255,255', 'view range');
        const short = encoder.encodeInto('\uDC00', bytes.subarray(0, 2));
        assert(short.read === 0 && short.written === 0, 'replacement cannot be split');
        const shared = new Uint8Array(new SharedArrayBuffer(3));
        assert(encoder.encodeInto('é', shared).written === 2, 'shared output');
        const decoder = new TextDecoder(' \tlatin1\n');
        assert(decoder.encoding === 'windows-1252' && !decoder.fatal && !decoder.ignoreBOM, 'label');
        assert(decoder.decode(Uint8Array.of(0x80)) === '€', 'legacy decoder');
        assert(new TextDecoder().decode(new DataView(shared.buffer, 0, 2)) === 'é', 'DataView range');
        assert(new TextDecoder().decode(Uint8Array.of(65).buffer) === 'A', 'buffer source');
        assert(new TextDecoder().decode(new Uint16Array([0x4241])) === 'AB', 'typed array bytes');
        throws(TypeError, () => TextEncoder());
        throws(TypeError, () => TextDecoder());
        throws(TypeError, () => TextEncoder.prototype.encode.call({}, 'x'));
        throws(TypeError, () => Object.create(TextDecoder.prototype).decode());
        throws(TypeError, () => encoder.encodeInto('x', new Uint8ClampedArray(1)));
        throws(TypeError, () => encoder.encodeInto('x', new Proxy(bytes, {})));
        throws(TypeError, () => encoder.encodeInto('x', new Uint8Array(new ArrayBuffer(1, {maxByteLength: 3}))));
        throws(TypeError, () => decoder.decode(new ArrayBuffer(1, {maxByteLength: 3})));
        throws(TypeError, () => decoder.decode(null));
        throws(TypeError, () => decoder.decode(bytes, true));
        for (const label of ['replacement', 'iso-2022-kr', 'utf-7', null]) {
            throws(RangeError, () => new TextDecoder(label));
        }
        let trace = '';
        throws(RangeError, () => new TextDecoder({toString() { trace += 'l'; return '?'; }}, {
            get fatal() { trace += 'f'; }, get ignoreBOM() { trace += 'b'; },
        }));
        assert(trace === 'lfb', 'all dictionary getters before constructor steps');
        const detached = new Uint8Array([65]);
        assert(decoder.decode(detached, {get stream() { detached.buffer.transfer(); }}) === '', 'detach by options');
        assert(encoder.encodeInto('x', detached).written === 0, 'detached output');
    "#).unwrap();
}

#[test]
fn text_decoder_retains_stream_state_through_bom_splits_and_reentry() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    agent.evaluate(r#"
        function assert(condition, message) { if (!condition) throw message; }
        const decoder = new TextDecoder();
        const stream = {stream: true};
        const bom = Uint8Array.of(0xEF,0xBB,0xBF);
        assert(decoder.decode(bom.subarray(0, 1), stream) === '', 'split BOM lead');
        assert(decoder.decode(bom.subarray(1), stream) === '', 'split BOM continuation');
        assert(decoder.decode(bom, stream) === '\uFEFF', 'same stream keeps next BOM');
        assert(decoder.decode(Uint8Array.of(0xF0,0x9D), stream) === '', 'split scalar lead');
        assert(decoder.decode(Uint8Array.of(0x84,0x9E)) === '𝄞', 'split scalar continuation');
        assert(decoder.decode(bom) === '', 'BOM resets');
        assert(new TextDecoder(undefined, {ignoreBOM: true}).decode(bom) === '\uFEFF', 'ignoreBOM');
        assert(new TextDecoder('utf-16be').decode(Uint8Array.of(0xFE,0xFF,0,65)) === 'A', 'UTF-16 BOM');
        assert(new TextDecoder('shift_jis').decode(Uint8Array.of(0x82,0xA0)) === 'あ', 'multibyte legacy');
        assert(decoder.decode(Uint8Array.of(0xC2), stream) === '', 'incomplete sequence');
        assert(decoder.decode() === '\uFFFD', 'empty final flush');
        const fatal = new TextDecoder('UTF-8', {fatal: 1});
        assert(fatal.fatal, 'IDL boolean');
        assert(fatal.decode(Uint8Array.of(0xC2), stream) === '', 'fatal pending input');
        try { fatal.decode(); throw 'expected fatal error'; }
        catch (error) { if (!(error instanceof TypeError)) throw error; }
        assert(fatal.decode(Uint8Array.of(65)) === 'A', 'reuse after fatal error');
        const input = Uint8Array.of(0xA2);
        assert(decoder.decode(input, {get stream() {
            decoder.decode(Uint8Array.of(0xC2), stream);
            return false;
        }}) === '¢', 'dictionary getter reentry precedes decode');
        const large = new Uint8Array(4098).fill(65);
        large.set([0xF0,0x9D,0x84,0x9E], 4094);
        assert(decoder.decode(large) === 'A'.repeat(4094) + '𝄞', 'native chunk boundary');
    "#).unwrap();
}

#[test]
fn fetch_binary_and_search_params_bodies_snapshot_the_input_and_consume_once() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    agent.evaluate(r#"
        let checked;
        function eq(a, b) { if (a !== b) throw `${a} != ${b}`; }
        async function exercise() {
            const data = Uint8Array.of(9, 0xEF, 0xBB, 0xBF, 65, 8);
            const response = new Response(new DataView(data.buffer, 1, 4));
            const clone = response.clone();
            data.fill(0);
            eq(response.headers.get('Content-Type'), null);
            eq(response.bodyUsed, false);
            const pending = response.bytes();
            eq(response.bodyUsed, true);
            const bytes = await pending;
            eq(bytes instanceof Uint8Array, true);
            eq(bytes.join(), '239,187,191,65');
            bytes[0] = 0;
            eq(await clone.text(), 'A');
            try { await response.arrayBuffer(); throw 'expected used body error'; }
            catch (e) { if (!(e instanceof TypeError)) throw e; }

            const source = Uint8Array.of(1, 2, 3);
            const request = new Request('https://example.test/', {method:'POST', body:source.subarray(1)});
            source[1] = 9;
            eq(request.headers.get('Content-Type'), null);
            const buffer = await request.arrayBuffer();
            eq(buffer instanceof ArrayBuffer, true);
            eq(new Uint8Array(buffer).join(), '2,3');

            const params = new URLSearchParams('q=a b');
            params.toString = () => 'unexpected';
            const form = new Response(params, { get status() { params.append('x', '&'); return 200; }});
            eq(form.headers.get('Content-Type'), 'application/x-www-form-urlencoded;charset=UTF-8');
            eq(await form.text(), 'q=a+b&x=%26');
            const detached = Uint8Array.of(65);
            const empty = new Response(detached, { get status() { detached.buffer.transfer(); return 200; }});
            eq((await empty.arrayBuffer()).byteLength, 0);
            eq(empty.bodyUsed, true);
            const noBody = new Response();
            eq((await noBody.bytes()).length, 0);
            eq((await noBody.arrayBuffer()).byteLength, 0);
            eq(noBody.bodyUsed, false);
            for (const body of [new SharedArrayBuffer(1), new Uint8Array(new SharedArrayBuffer(1)),
                                new ArrayBuffer(1, {maxByteLength: 2})]) {
                try { new Response(body); throw 'expected buffer type error'; }
                catch (e) { if (!(e instanceof TypeError)) throw e; }
            }
        }
        exercise().then(() => { checked = 'pass'; }, e => { checked = String(e); });
    "#).unwrap();
    agent.run_jobs().unwrap();
    let value = agent.evaluate("checked").unwrap().as_string().unwrap();
    assert_eq!(agent.heap.string(value).to_text(), "pass");
}

#[test]
fn blob_construction_slicing_and_fetch_preserve_immutable_bytes_and_mime() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    agent.evaluate(r#"
        let checked;
        function eq(a, b) { if (a !== b) throw `${a} != ${b}`; }
        async function exercise() {
            const backing = Uint8Array.of(7,65,66,8);
            const suffix = new Blob(['é\uD800']);
            const blob = new Blob([new DataView(backing.buffer, 1, 2), suffix, 42], {type: 'TEXT/PLAIN'});
            backing.fill(0);
            eq(blob.size, 9);
            eq(blob.type, 'text/plain');
            eq(await blob.text(), 'ABé\uFFFD42');
            const bytes = await blob.bytes(); bytes[0] = 0;
            const buffer = await blob.arrayBuffer();
            eq(new Uint8Array(buffer)[0], 65);
            eq(await blob.slice(-2, undefined, 'X/CUSTOM').text(), '42');
            eq(blob.slice(-2, undefined, 'X/CUSTOM').type, 'x/custom');
            eq(blob.slice(5, 1, 'a\n').type, '');
            eq(blob.slice(5, 1).size, 0);
            eq(await new Blob(['abcd']).slice(0.5, 2.5).text(), 'ab');
            eq(blob.slice(Infinity).size, 0);
            eq(blob.slice(-Infinity).size, blob.size);

            const detached = Uint8Array.of(65);
            const noBytes = new Blob([detached, 'Z'], { get type() { detached.buffer.transfer(); return ''; }});
            eq(await noBytes.text(), 'Z');
            const bom = new Blob([Uint8Array.of(0xEF,0xBB,0xBF,65)]);
            eq(await bom.text(), 'A');
            eq((await bom.bytes()).length, 4);

            const response = new Response(blob);
            eq(response.headers.get('content-type'), 'text/plain');
            response.headers.set('content-type', 'Text/HTML; Charset=UTF-8');
            const changed = await response.blob();
            eq(changed instanceof Blob, true);
            eq(changed.type, 'text/html;charset=UTF-8');
            eq(await changed.text(), 'ABé\uFFFD42');
            eq(response.bodyUsed, true);
            const request = new Request('https://example.test/', {method: 'POST', body:suffix});
            eq(request.headers.get('content-type'), null);
            eq(await (await request.blob()).text(), 'é\uFFFD');
            eq((await new Response().blob()).size, 0);
        }
        exercise().then(() => { checked = 'pass'; }, e => { checked = String(e); });
    "#).unwrap();
    agent.run_jobs().unwrap();
    let value = agent.evaluate("checked").unwrap().as_string().unwrap();
    assert_eq!(agent.heap.string(value).to_text(), "pass");
}

#[test]
fn response_json_uses_intrinsic_serialization_after_dictionary_conversion() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    agent.evaluate(r#"
        let checked;
        function eq(a, b) { if (a !== b) throw `${a} != ${b}`; }
        async function exercise() {
            let calls = '';
            JSON.stringify = () => { throw 'must use intrinsic JSON'; };
            const response = Response.json({toJSON() { calls += 'j'; return {ok: true}; }}, {
                get headers() { calls += 'h'; return {'X-Test': '1'}; },
                get status() { calls += 's'; return 65536 + 201.9; },
                get statusText() { calls += 't'; return 'Created'; },
            });
            eq(calls, 'hstj');
            eq(response.status, 201);
            eq(response.statusText, 'Created');
            eq(response.headers.get('Content-Type'), 'application/json');
            eq(response.headers.get('X-Test'), '1');
            eq((await response.json()).ok, true);
            const reason = {};
            try { Response.json({toJSON() { throw reason; }}, {status: 10}); throw 'expected error'; }
            catch (e) { eq(e, reason); }
            for (const value of [undefined, Symbol('x'), 1n]) {
                try { Response.json(value); throw 'expected TypeError'; }
                catch (e) { if (!(e instanceof TypeError)) throw e; }
            }
        }
        exercise().then(() => { checked = 'pass'; }, e => { checked = String(e); });
    "#).unwrap();
    agent.run_jobs().unwrap();
    let value = agent.evaluate("checked").unwrap().as_string().unwrap();
    assert_eq!(agent.heap.string(value).to_text(), "pass");
}

#[test]
fn dom_rects_are_branded_and_keep_their_values_through_reentry() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let tree = Rc::new(Tree(RefCell::new(document)));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree).unwrap();
    agent.heap.set_gc_stress(true);
    agent
        .evaluate(
            r#"
        let trace = '';
        const readOnly = DOMRectReadOnly.fromRect({
            get height() { trace += 'h'; return -3; },
            get width() { trace += 'w'; return -5; },
            get x() { trace += 'x'; return 20; },
            get y() { trace += 'y'; return 10; },
        });
        if (trace !== 'hwxy' || readOnly instanceof DOMRect ||
            readOnly.top !== 7 || readOnly.right !== 20 ||
            readOnly.bottom !== 10 || readOnly.left !== 15) throw 1;
        const write = Object.getOwnPropertyDescriptor(DOMRect.prototype, 'x').set;
        try { write.call(readOnly, 0); throw 2; }
        catch (error) { if (!(error instanceof TypeError)) throw error; }
        const mutable = DOMRect.fromRect(readOnly);
        mutable.x = { valueOf() {
            const temporary = new DOMRect(4, 3, 2, 1);
            if (temporary.right !== 6) throw 3;
            return 7;
        } };
        if (mutable.x !== 7 || mutable.left !== 2 || readOnly.x !== 20 ||
            mutable.toJSON().height !== -3 || Object.keys(mutable).length !== 0) throw 4;
        if (!Number.isNaN(new DOMRect(0, 0, NaN).right)) throw 5;
    "#,
        )
        .unwrap();
}

#[test]
fn text_content_preserves_code_units_and_reentrant_conversion() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let tree = Rc::new(Tree(RefCell::new(document)));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree).unwrap();
    agent.heap.set_gc_stress(true);
    agent
        .evaluate(
            r#"
        const parent = document.createElement('div');
        const child = document.createElement('span');
        const units = 'a' + String.fromCharCode(0xd800);
        child.appendChild(document.createTextNode(units));
        parent.appendChild(child);
        parent.appendChild(document.createComment('ignored'));
        if (parent.textContent !== units || document.textContent !== null) throw 1;
        parent.textContent = { toString() {
            child.textContent = 'changed during conversion';
            return units;
        } };
        if (parent.childNodes.length !== 1 || parent.firstChild.data !== units ||
            child.parentNode !== null || child.textContent !== 'changed during conversion') throw 2;
        parent.textContent = null;
        if (parent.textContent !== '' || parent.firstChild !== null) throw 3;
    "#,
        )
        .unwrap();
}

#[test]
fn body_replacement_adopts_wrappers_and_preserves_old_children_on_error() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let tree = Rc::new(Tree(RefCell::new(document)));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree).unwrap();
    agent.heap.set_gc_stress(true);
    agent.evaluate(r#"
        if (document.body !== null || document.hidden || document.visibilityState !== 'visible') throw 1;
        document.appendChild(document.createElement('html'));
        const body = document.createElement('body');
        document.body = body;
        if (document.body !== body || body.parentNode !== document.documentElement) throw 2;
        try { document.body = document.createElement('div'); throw 3; }
        catch (error) { if (error.name !== 'HierarchyRequestError') throw error; }
        if (document.body !== body) throw 4;
        const other = document.implementation.createHTMLDocument('other');
        if (!other.hidden || other.visibilityState !== 'hidden') throw 'detached document visibility';
        const foreign = other.body;
        document.body = foreign;
        if (document.body !== foreign || foreign.ownerDocument !== document ||
            other.body !== null || body.parentNode !== null) throw 5;
        document.body = foreign;
        if (document.documentElement.childNodes.length !== 1) throw 6;
    "#).unwrap();
}

#[test]
fn automatic_collection_preserves_attached_wrappers_and_sweeps_detached_cycles() {
    let mut document = Document::new(DocumentLimits::default()).unwrap();
    let [root, retained, unreachable] = ["main", "p", "em"].map(|name| {
        document
            .create_element(QualifiedName::html(name).unwrap())
            .unwrap()
    });
    document.append_child(document.root(), root).unwrap();
    document.append_child(root, retained).unwrap();
    document.append_child(root, unreachable).unwrap();
    let tree = Rc::new(Tree(RefCell::new(document)));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree.clone()).unwrap();
    agent.heap.set_gc_stress(true);
    let before = agent.heap.collections();

    agent
        .evaluate(
            r#"
        document.documentElement.value = 42;
        (() => {
            const root = document.documentElement;
            const retained = root.firstChild;
            (() => {
                const unreachable = retained.nextSibling;
                unreachable.cycle = () => unreachable;
                root.removeChild(unreachable);
            })();
            if (root.removeChild(retained) !== retained || retained.parentNode !== null)
                throw "detach changed the wrapper";
            Promise.resolve().then(() => {
                if (retained.localName !== "p" || retained.ownerDocument !== document ||
                    document.documentElement.value !== 42)
                    throw "lost a reachable wrapper";
                root.appendChild(retained);
                globalThis.finished = true;
            });
        })();
    "#,
        )
        .unwrap();
    let before_checkpoint = agent.heap.collections();
    assert!(agent.run_jobs_limited(8).unwrap());
    assert!(before_checkpoint > before);
    assert!(agent.heap.collections() > before_checkpoint);
    assert_eq!(
        agent
            .evaluate("(() => finished && document.documentElement.firstChild.localName === 'p')()")
            .unwrap(),
        Value::TRUE
    );
    let document = tree.0.borrow();
    assert_eq!(document.node(retained).unwrap().parent(), Some(root));
    assert!(document.node(unreachable).is_err());
    document.check_invariants().unwrap();
}

#[test]
fn factories_create_live_nodes_with_specialized_prototypes() {
    let tree = Rc::new(Tree(RefCell::new(
        Document::new(DocumentLimits::default()).unwrap(),
    )));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree).unwrap();
    agent.heap.set_gc_stress(true);
    assert_eq!(
        agent
            .evaluate(
                r#"
        const element = document.createElement("MÉTa");
        const fragment = document.createDocumentFragment();
        const comment = document.createComment("comment");
        const text = document.createTextNode("\uD800");
        const instruction = document.createProcessingInstruction("xml-stylesheet", "a");
        class CustomText extends Text {}
        const derived = new CustomText(null);
        if (!(element instanceof Element) || element.localName !== "mÉta" ||
            !(fragment instanceof DocumentFragment) || !(new DocumentFragment() instanceof Node) ||
            !(comment instanceof Comment) || !(comment instanceof CharacterData) ||
            !(text instanceof Text) || text.data.charCodeAt(0) !== 0xD800 ||
            !(derived instanceof CustomText) || derived.data !== "null" ||
            (new Text(undefined)).data !== "" || (new Comment()).data !== "" ||
            !(instruction instanceof ProcessingInstruction) ||
            (new ProcessingInstruction("target")).data !== "" ||
            instruction.target !== "xml-stylesheet")
            throw "factory defaults or prototypes";
        fragment.appendChild(element);
        element.appendChild(text);
        element.appendChild(comment);
        document.appendChild(fragment);
        if (document.documentElement !== element || fragment.firstChild !== null ||
            text.ownerDocument !== document || text.parentNode !== element)
            throw "wrong document or wrapper identity";
        const badNames = ["", "1x", "a b", "a/b", "\uD800"];
        for (const name of badNames) {
            try { document.createElement(name); throw "invalid name accepted"; }
            catch (error) {
                if (!(error instanceof DOMException) || error.name !== "InvalidCharacterError")
                    throw error;
            }
        }
        try { document.createCDATASection("x"); throw "HTML CDATA accepted"; }
        catch (error) { if (error.name !== "NotSupportedError") throw error; }
        document.createElement("a+", {is: undefined}).localName === "a+";
    "#
            )
            .unwrap(),
        Value::TRUE
    );
}

#[test]
fn character_data_uses_utf16_and_reenters_before_tree_mutation() {
    let tree = Rc::new(Tree(RefCell::new(
        Document::new(DocumentLimits::default()).unwrap(),
    )));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree).unwrap();
    agent.heap.set_gc_stress(true);
    assert_eq!(agent.evaluate(r#"
        const root = document.createElement("div");
        document.appendChild(root);
        const text = new Text("A\uD83D\uDE80Z");
        root.appendChild(text);
        if (text.length !== 4 || text.substringData(1, 1).charCodeAt(0) !== 0xD83D)
            throw "byte offsets instead of UTF-16";
        const tail = text.splitText(2);
        if (!(tail instanceof Text) || tail === text || tail.parentNode !== root ||
            text.nextSibling !== tail || tail.data.charCodeAt(0) !== 0xDE80 ||
            text.wholeText !== "A\uD83D\uDE80Z" || tail.wholeText !== text.wholeText)
            throw "split or wholeText";
        tail.data = null;
        if (tail.length !== 0) throw "legacy null";
        let order = "";
        tail.replaceData(
            { valueOf() { text.appendData("n"); order += "1"; return 0; } },
            { valueOf() { tail.appendData("x"); order += "2"; return 4294967295; } },
            { toString() { order += "3"; return "done"; } }
        );
        if (order !== "123" || tail.data !== "done" || text.data !== "A\uD83Dn")
            throw "argument conversion order";
        tail.insertData(0, "_");
        tail.deleteData(0, 1);
        tail.appendData("!");
        try { tail.splitText(99); throw "out-of-range split accepted"; }
        catch (error) { if (error.name !== "IndexSizeError") throw error; }
        const sentinel = {};
        try { tail.data = { toString() { throw sentinel; } }; }
        catch (error) { if (error !== sentinel) throw error; }
        try {
            CharacterData.prototype.replaceData.call(root, {valueOf(){throw "not branded";}}, 0, "");
            throw "unbranded receiver accepted";
        } catch (error) { if (!(error instanceof TypeError)) throw error; }
        tail.data === "done!" && text.nextSibling === tail && tail.nextSibling === null;
    "#).unwrap(), Value::TRUE);
}

#[test]
fn attribute_calls_preserve_namespaces_and_finish_coercion_before_editing() {
    let mut document = Document::new(DocumentLimits::default()).unwrap();
    let svg = document
        .create_element(
            QualifiedName::from_parsed(Some(rustic_dom::SVG_NAMESPACE), None, "svg").unwrap(),
        )
        .unwrap();
    let link =
        QualifiedName::from_parsed(Some("http://www.w3.org/1999/xlink"), Some("xlink"), "href")
            .unwrap();
    document.set_attribute(svg, link, "initial").unwrap();
    document.append_child(document.root(), svg).unwrap();
    let tree = Rc::new(Tree(RefCell::new(document)));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree.clone()).unwrap();

    assert_eq!(agent.evaluate(r#"
        const svg = document.documentElement;
        svg.setAttribute("xlink:href", {toString() { svg.setAttribute("viewBox", "0 0 10 10"); return "new"; }});
        const p = document.createElement("P");
        p.setAttribute("DATA-TEST", "\uD800");
        if (!p.hasAttribute("data-test") || p.getAttribute("DATA-TEST").charCodeAt(0) !== 0xD800 ||
            svg.getAttribute("xlink:href") !== "new" || svg.hasAttribute("VIEWBOX"))
            throw "qualified name or UTF-16 mismatch";
        p.removeAttribute("DATA-test");
        try { p.setAttribute("bad/name", "value"); throw "invalid attribute accepted"; }
        catch (error) { if (error.name !== "InvalidCharacterError") throw error; }
        p.getAttribute("data-test") === null && svg.getAttribute("viewBox") === "0 0 10 10";
    "#).unwrap(), Value::TRUE);
    let document = tree.0.borrow();
    let rustic_dom::NodeKind::Element(element) = document.node(svg).unwrap().kind() else {
        panic!("SVG root was replaced")
    };
    assert_eq!(element.attributes().len(), 2);
    let link = element.attributes()[0].name();
    assert_eq!(link.namespace(), Some("http://www.w3.org/1999/xlink"));
    assert_eq!(link.prefix(), Some("xlink"));
    assert_eq!(link.local(), "href");
}

#[test]
fn page_selectors_keep_scope_order_and_use_live_tree_after_coercion() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let tree = Rc::new(Tree(RefCell::new(document)));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree).unwrap();
    assert_eq!(agent.evaluate(r#"
        const root = document.createElement("main");
        const section = document.createElement("section");
        const first = document.createElement("p");
        const second = document.createElement("p");
        const fragment = document.createDocumentFragment();
        document.appendChild(root);
        root.appendChild(section);
        section.appendChild(first);
        section.appendChild(second);
        first.setAttribute("id", "Heading");
        first.setAttribute("class", "active");
        second.setAttribute("id", "Heading");
        if (document.querySelector(":scope") !== root || section.querySelector(":scope") !== null ||
            section.querySelector(":scope > p") !== first ||
            section.querySelector("main p") !== first ||
            section.querySelectorAll(".active, p").length !== 2 ||
            document.getElementById("Heading") !== first ||
            document.getElementById("heading") !== null ||
            !first.matches("section > :scope") ||
            first.closest("main") !== root || first.closest(":scope") !== first)
            throw "selector scope, identity, or order";
        if (first.matches("p::before") || root.querySelector("p::before") !== null)
            throw "pseudo-element returned as a DOM node";
        const result = section.querySelector({ toString() {
            fragment.appendChild(first);
            return "p";
        } });
        if (result !== second || fragment.querySelector(".active") !== first ||
            fragment.querySelector(":scope > .active") !== first ||
            fragment.querySelector(":scope") !== null ||
            fragment.querySelector("* > .active") !== null ||
            fragment.getElementById("Heading") !== first)
            throw "query used the tree before coercion";
        for (const invalid of ["", "div >", "p, > x", "x|p"]) {
            try { root.querySelectorAll(invalid); throw "accepted invalid selector"; }
            catch (error) { if (!(error instanceof DOMException) || error.name !== "SyntaxError") throw error; }
        }
        try { Element.prototype.matches.call(document, {toString(){throw "converted unbranded receiver";}}); }
        catch (error) { if (!(error instanceof TypeError)) throw error; }
        true;
    "#).unwrap(), Value::TRUE);
}

#[test]
fn node_lists_distinguish_static_membership_and_live_children_through_gc() {
    let tree = Rc::new(Tree(RefCell::new(
        Document::new(DocumentLimits::default()).unwrap(),
    )));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree.clone()).unwrap();
    agent.heap.set_gc_stress(true);
    assert_eq!(agent.evaluate(r#"
        var snapshot, live;
        (() => {
            const detached = document.createElement("section");
            const p = document.createElement("p");
            p.marker = "snapshot";
            detached.appendChild(p);
            detached.appendChild(document.createTextNode("tail"));
            snapshot = detached.querySelectorAll("p");
            live = detached.childNodes;
            if (!(live instanceof NodeList) || Array.isArray(live) || detached.childNodes !== live ||
                !(snapshot instanceof NodeList) || snapshot[0] !== p || live[0] !== p ||
                live.item(1).data !== "tail" || live.item(2) !== null || live[2] !== undefined)
                throw "NodeList brand or indices";
            const desc = Object.getOwnPropertyDescriptor(live, "0");
            if (!desc.enumerable || !desc.configurable || desc.writable ||
                Reflect.set(live, "0", p) || Reflect.deleteProperty(live, "0") ||
                Reflect.defineProperty(live, "8", {value:p}) || Reflect.preventExtensions(live))
                throw "invalid indexed internal methods";
            detached.removeChild(p);
            if (snapshot.item(0) !== p || snapshot.length !== 1 ||
                live.length !== 1 || live[0].data !== "tail" ||
                Reflect.ownKeys(live).join() !== "0" || 1 in live)
                throw "snapshot or live membership";
            if (Array.from(snapshot)[0] !== p || [...live.keys()][0] !== 0 ||
                [...live.entries()][0][1] !== live[0])
                throw "iterable";
            let visited = 0;
            live.forEach((value, index, list) => {
                if (value !== live[0] || index !== 0 || list !== live) throw "wrong visitor arguments";
                visited++;
            });
            if (visited !== 1) throw "missing visitor";
        })();
        (() => snapshot[0].marker === "snapshot" && live[0].parentNode.childNodes === live)();
    "#).unwrap(), Value::TRUE);
    assert_eq!(tree.0.borrow().node_count(), 4);
    agent
        .evaluate("snapshot = null; live = null; (() => 0)();")
        .unwrap();
    assert_eq!(tree.0.borrow().node_count(), 1);
}

#[test]
fn selector_limits_stop_native_walks_instead_of_returning_partial_lists() {
    let mut document = Document::new(DocumentLimits::default()).unwrap();
    let root = document
        .create_element(QualifiedName::html("main").unwrap())
        .unwrap();
    document.append_child(document.root(), root).unwrap();
    for _ in 0..20 {
        let child = document
            .create_element(QualifiedName::html("p").unwrap())
            .unwrap();
        document.append_child(root, child).unwrap();
    }
    let tree = Rc::new(Tree(RefCell::new(document)));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree).unwrap();
    agent.set_step_budget(Some(10));
    assert!(matches!(
        agent.evaluate("globalThis.result = document.querySelectorAll('p');"),
        Err(rustic_js::EvalError::Cancelled)
    ));
    agent.set_step_budget(None);
    assert_eq!(
        agent
            .evaluate(
                "typeof result === 'undefined' && document.querySelectorAll('p').length === 20"
            )
            .unwrap(),
        Value::TRUE
    );
    agent
        .evaluate("globalThis.live = document.getElementsByTagName('p');")
        .unwrap();
    agent.set_step_budget(Some(10));
    assert!(matches!(
        agent.evaluate("globalThis.collectionKeys = Reflect.ownKeys(live);"),
        Err(rustic_js::EvalError::Cancelled)
    ));
    agent.set_step_budget(None);
    assert_eq!(
        agent
            .evaluate(
                "typeof collectionKeys === 'undefined' && Reflect.ownKeys(live).length === 20"
            )
            .unwrap(),
        Value::TRUE
    );
}

#[test]
fn html_collection_children_live_names_and_descriptor_visibility() {
    let tree = Rc::new(Tree(RefCell::new(
        Document::new(DocumentLimits::default()).unwrap(),
    )));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree.clone()).unwrap();
    agent.heap.set_gc_stress(true);
    assert_eq!(agent.evaluate(r#"
        var saved;
        (() => {
            const root = document.createDocumentFragment();
            const a = document.createElement("p");
            const b = document.createElement("p");
            const c = document.createElement("p");
            const text = document.createTextNode("ab");
            const nodes = root.childNodes;
            saved = root.children;
            const list = saved;
            list.future = 42;
            a.setAttribute("name", "shared");
            a.setAttribute("id", "first");
            b.setAttribute("id", "shared");
            b.setAttribute("name", "item");
            c.setAttribute("id", "8");
            c.setAttribute("name", "future");
            root.appendChild(a);
            root.appendChild(text);
            root.appendChild(b);
            root.appendChild(c);
            if (!(list instanceof HTMLCollection) || list !== root.children ||
                list.length !== 3 || list[1] !== b || list.item(2) !== c ||
                list.item(3) !== null || list[3] !== undefined || [...list][1] !== b ||
                nodes.length !== 4 || nodes[1] !== text || list.namedItem("") !== null)
                throw "child collections";
            if (list.shared !== a || list.namedItem("shared") !== a ||
                list.namedItem("item") !== b || list.item !== HTMLCollection.prototype.item ||
                list.namedItem("8") !== c || list[8] !== undefined || list.future !== 42)
                throw "names or shadowing";
            const symbol = Symbol();
            list[symbol] = 19;
            const keys = Reflect.ownKeys(list);
            if (keys.slice(0, -1).join() !== "0,1,2,first,shared,future" ||
                keys[keys.length - 1] !== symbol || Object.keys(list).join() !== "0,1,2,future")
                throw "own key order or enumerability";
            const desc = Object.getOwnPropertyDescriptor(list, "shared");
            if (!desc.configurable || desc.enumerable || desc.writable || desc.value !== a ||
                Reflect.set(list, "shared", 1) || Reflect.deleteProperty(list, "shared") ||
                Reflect.defineProperty(list, "item", {value: 1}) || Reflect.preventExtensions(list))
                throw "named property internal methods";
            if (!Reflect.deleteProperty(list, "future") || list.future !== c)
                throw "named property after deleting expando";
            text.splitText(1);
            root.removeChild(a);
            if (nodes.length !== 4 || list.length !== 2 || list.shared !== b || "first" in list)
                throw "child index after split and remove";
            b.removeAttribute("id");
            if ("shared" in list || list.namedItem("shared") !== null)
                throw "name must follow attributes";
            b.setAttribute("id", "\ud800");
            if (list["\ud800"] !== b || list.namedItem("\ud800") !== b)
                throw "names are UTF-16";
            const getter = Object.getOwnPropertyDescriptor(DocumentFragment.prototype, "children").get;
            let illegal = 0;
            try { getter.call(text); } catch (error) { if (error instanceof TypeError) illegal++; }
            try { getter.call(a); } catch (error) { if (error instanceof TypeError) illegal++; }
            try { Document.prototype.getElementsByTagName.call(a, "p"); }
            catch (error) { if (error instanceof TypeError) illegal++; }
            try { list.namedItem(); } catch (error) { if (error instanceof TypeError) illegal++; }
            try { new HTMLCollection(); } catch (error) { if (error instanceof TypeError) illegal++; }
            if (illegal !== 5) throw "brand or required argument";
        })();
        (() => saved.length === 2 && saved[0].parentNode.children === saved)();
    "#).unwrap(), Value::TRUE);
    agent.evaluate("(() => 0)();").unwrap();
    assert_eq!(tree.0.borrow().node_count(), 6);
    agent.evaluate("saved = null; (() => 0)();").unwrap();
    assert_eq!(tree.0.borrow().node_count(), 1);
}

#[test]
fn html_collection_descendant_filters_preserve_scope_namespaces_and_liveness() {
    use rustic_dom::{DomString, SVG_NAMESPACE};

    let mut document = Document::new(DocumentLimits::default()).unwrap();
    let root = document
        .create_element(QualifiedName::html("main").unwrap())
        .unwrap();
    document.append_child(document.root(), root).unwrap();
    for (name, id) in [
        (
            QualifiedName::from_parsed(Some(SVG_NAMESPACE), Some("svg"), "linearGradient").unwrap(),
            "gradient",
        ),
        (
            QualifiedName::from_parsed(None, None, "P").unwrap(),
            "foreign",
        ),
    ] {
        let node = document.create_element(name).unwrap();
        document
            .set_attribute(
                node,
                QualifiedName::from_parsed(None, None, "id").unwrap(),
                DomString::from(id),
            )
            .unwrap();
        document
            .set_attribute(
                node,
                QualifiedName::from_parsed(None, None, "name").unwrap(),
                DomString::from("foreignName"),
            )
            .unwrap();
        document.append_child(root, node).unwrap();
    }
    let tree = Rc::new(Tree(RefCell::new(document)));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree.clone()).unwrap();
    agent.heap.set_gc_stress(true);
    assert_eq!(
        agent
            .evaluate(
                r#"
        const root = document.documentElement;
        const branch = document.createElement("div");
        root.appendChild(branch);
        const p = document.createElement("p");
        p.setAttribute("class", "alpha\tbeta");
        p.setAttribute("id", "selected");
        branch.appendChild(p);
        const tag = branch.getElementsByTagName("P");
        const cls = root.getElementsByClassName(" beta  alpha alpha\r");
        const explicitNamespace = root.getElementsByTagNameNS(
            {toString() { return "http://www.w3.org/2000/" + "svg"; }},
            {toString() { (() => 0)(); return "linearGradient"; }});
        const gradient = explicitNamespace[0];
        if (!(cls instanceof HTMLCollection) || root.getElementsByTagName("main").length !== 0 ||
            document.getElementsByTagName("main")[0] !== root || tag[0] !== p ||
            cls.length !== 1 || cls.namedItem("selected") !== p ||
            root.getElementsByClassName("Alpha").length !== 0 ||
            root.getElementsByClassName("\r\n \t").length !== 0)
            throw "descendant scope and class tokens";
        if (root.getElementsByTagName("svg:linearGradient")[0] !== gradient ||
            root.getElementsByTagName("SVG:LINEARGRADIENT").length !== 0 ||
            explicitNamespace.length !== 1 || gradient.getAttribute("id") !== "gradient" ||
            root.getElementsByTagNameNS("*", "linearGradient")[0] !== gradient ||
            root.getElementsByTagNameNS("", "P")[0].getAttribute("id") !== "foreign" ||
            root.getElementsByTagNameNS(null, "p").length !== 0 ||
            root.getElementsByTagName("P").length !== 2 ||
            root.getElementsByTagName("*").namedItem("foreignName") !== null)
            throw "qualified names and namespaces";
        p.setAttribute("class", "beta");
        if (cls.length !== 0) throw "class mutation";
        const newClasses = root.getElementsByClassName({toString() {
            p.setAttribute("class", "alpha beta");
            return "alpha";
        }});
        if (newClasses[0] !== p || cls[0] !== p) throw "class conversion reentry";
        root.appendChild(p);
        tag.length === 0 && cls[0] === p && root.children.namedItem("selected") === p;
    "#
            )
            .unwrap(),
        Value::TRUE
    );
    tree.0.borrow().check_invariants().unwrap();
}

#[test]
fn html_collection_named_property_allows_proxy_visibility_traps_to_mutate_the_dom() {
    let tree = Rc::new(Tree(RefCell::new(
        Document::new(DocumentLimits::default()).unwrap(),
    )));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree).unwrap();
    agent.heap.set_gc_stress(true);
    assert_eq!(
        agent
            .evaluate(
                r#"
        (() => {
            const root = document.createElement("div");
            const element = document.createElement("p");
            element.setAttribute("id", "one");
            root.appendChild(element);
            const list = root.children;
            Object.setPrototypeOf(list, new Proxy(HTMLCollection.prototype, {
                getOwnPropertyDescriptor(target, key) {
                    if (key === "one") element.setAttribute("id", "two");
                    return Reflect.getOwnPropertyDescriptor(target, key);
                }
            }));
            const descriptor = Object.getOwnPropertyDescriptor(list, "one");
            return descriptor.value === null && !("one" in list) && list.two === element &&
                Reflect.ownKeys(list).join() === "0,two";
        })();
    "#
            )
            .unwrap(),
        Value::TRUE
    );
}

#[test]
fn page_window_keeps_a_distinct_global_this_and_the_live_document() {
    let tree = Rc::new(Tree(RefCell::new(
        Document::new(DocumentLimits::default()).unwrap(),
    )));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree).unwrap();
    assert_ne!(agent.intrinsics.global, agent.global_this());
    agent.heap.set_gc_stress(true);
    assert_eq!(
        agent
            .evaluate(
                r#"
        var declared = 1;
        let lexicalOnly = 2;
        const originalView = window;
        if (this !== window || globalThis !== window || window.window !== window ||
            self !== window || frames !== window || parent !== window || top !== window ||
            length !== 0 || closed || frameElement !== null ||
            document.defaultView !== window || window.document !== document ||
            !(window instanceof Window) || Object.getPrototypeOf(window) !== Window.prototype ||
            Object.prototype.toString.call(window) !== "[object Window]" ||
            window.declared !== 1 || "lexicalOnly" in window)
            throw "Window identity or bindings";
        window.declared = 3;
        window.fromWindow = 4;
        if (declared !== 3 || fromWindow !== 4 ||
            (function () { return this; })() !== window || eval("this") !== window ||
            (0, eval)("this") !== window || Function("return this")() !== window)
            throw "Window global environment";
        Object.defineProperty(window, "accessor", {
            get() { return this; },
            set(value) { this.receiverWasView = this === originalView && value === 8; }
        });
        accessor = 8;
        if (accessor !== originalView || !receiverWasView || eval("accessor") !== originalView)
            throw "an identifier accessor exposed the underlying Window";
        globalThis = 5;
        const sameView = () => this === originalView;
        if (!sameView() || globalThis !== 5 || document.defaultView !== originalView ||
            (function () { return this; })() !== originalView)
            throw "mutable globalThis property changed the realm's this";
        Promise.resolve().then(function () {
            window.jobSawWindow = this === originalView && document.defaultView === originalView;
        });
        true;
    "#
            )
            .unwrap(),
        Value::TRUE
    );
    assert!(agent.run_jobs_limited(8).unwrap());
    assert_eq!(
        agent
            .evaluate("this === originalView && jobSawWindow && window.document === document")
            .unwrap(),
        Value::TRUE
    );

    let page = agent.current_realm();
    let standalone = agent.create_realm().unwrap();
    agent.enter_realm(standalone);
    assert_eq!(agent.global_this(), agent.intrinsics.global);
    assert_eq!(agent.evaluate("this === globalThis && typeof Window === 'undefined' && typeof document === 'undefined'").unwrap(), Value::TRUE);
    agent.enter_realm(page);
    assert_eq!(
        agent
            .evaluate("this === originalView && globalThis === 5")
            .unwrap(),
        Value::TRUE
    );
}

#[test]
fn window_proxy_forwards_properties_and_enforces_its_own_descriptor_rules() {
    let tree = Rc::new(Tree(RefCell::new(
        Document::new(DocumentLimits::default()).unwrap(),
    )));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree).unwrap();
    assert_eq!(agent.evaluate(r#"
        (() => {
            for (const key of ["window", "top", "document"]) {
                const descriptor = Object.getOwnPropertyDescriptor(window, key);
                if (typeof descriptor.get !== "function" || descriptor.set !== undefined ||
                    !descriptor.enumerable || descriptor.configurable ||
                    Reflect.set(window, key, 5) || Reflect.deleteProperty(window, key) ||
                    Reflect.defineProperty(window, key, { value: 5 }))
                    throw "unforgeable attribute";
            }
            const getDocument = Object.getOwnPropertyDescriptor(window, "document").get;
            const getView = Object.getOwnPropertyDescriptor(Document.prototype, "defaultView").get;
            if (getDocument() !== document || getDocument.call(null) !== document)
                throw "default Window receiver";
            let rejected = 0;
            for (const call of [
                () => getDocument.call(Window.prototype),
                () => getView.call(window),
                () => new Window(),
                () => Window(),
                () => Reflect.get(window, "document", {}),
            ]) {
                try { call(); } catch (error) { if (error instanceof TypeError) rejected++; }
            }
            if (rejected !== 5 || !Object.isExtensible(window) ||
                Reflect.preventExtensions(window) || Reflect.setPrototypeOf(window, {}) ||
                !Reflect.setPrototypeOf(window, Window.prototype) ||
                Reflect.setPrototypeOf(Window.prototype, window))
                throw "Window brand or prototype";
            for (const key of ["length", "self", "frames", "parent"]) {
                const descriptor = Object.getOwnPropertyDescriptor(window, key);
                if (!descriptor.enumerable || !descriptor.configurable || typeof descriptor.set !== "function")
                    throw "replaceable attribute descriptor";
                window[key] = 41;
                const data = Object.getOwnPropertyDescriptor(window, key);
                if (data.value !== 41 || !data.writable || !data.enumerable || !data.configurable)
                    throw "replaceable attribute setter";
            }
            if ("0" in window || window[0] !== undefined ||
                Reflect.defineProperty(window, "0", {value: 9}) || Reflect.set(window, "0", 9) ||
                !Reflect.deleteProperty(window, "0"))
                throw "empty child window indices";
            const marker = Symbol();
            const receiver = {};
            Object.defineProperty(window, marker, {
                get() { return this; },
                set(value) { this.captured = value; },
                configurable: true
            });
            if (window[marker] !== window || Reflect.get(window, marker, receiver) !== receiver ||
                !Reflect.set(window, marker, 6, receiver) || receiver.captured !== 6 ||
                !Reflect.ownKeys(window).includes(marker) || !Reflect.deleteProperty(window, marker) ||
                marker in window)
                throw "ordinary property forwarding";
            return document.defaultView === window && top === window;
        })();
    "#).unwrap(), Value::TRUE);
}

#[test]
fn detached_document_factories_keep_format_and_namespace_rules() {
    let tree = Rc::new(Tree(RefCell::new(
        Document::new(DocumentLimits::default()).unwrap(),
    )));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree).unwrap();
    agent.heap.set_gc_stress(true);
    assert_eq!(agent.evaluate(r#"
        (() => {
            const empty = new Document();
            const api = document.implementation;
            if (!(empty instanceof Document) || empty instanceof XMLDocument ||
                empty.contentType !== "application/xml" || empty.documentElement !== null ||
                empty.defaultView !== null || document.defaultView !== window ||
                api !== document.implementation || !(api instanceof DOMImplementation) ||
                empty.implementation === api || !api.hasFeature())
                throw "factory brands or document identity";
            const doctype = api.createDocumentType("kite", "pub", "sys");
            if (doctype.ownerDocument !== document) throw "initial doctype owner";
            const xml = api.createDocument("urn:kite", "k:Kite", doctype);
            if (!(xml instanceof XMLDocument) || xml.contentType !== "application/xml" ||
                xml.documentElement.localName !== "Kite" || xml.documentElement.prefix !== "k" ||
                xml.documentElement.tagName !== "k:Kite" ||
                xml.documentElement.namespaceURI !== "urn:kite" || xml.doctype !== doctype ||
                xml.firstChild !== doctype || doctype.ownerDocument !== xml ||
                doctype.publicId !== "pub" || doctype.systemId !== "sys" ||
                xml.createElement("Mixed").localName !== "Mixed" ||
                xml.createElement("Mixed").namespaceURI !== null ||
                xml.createCDATASection("\ud800").data !== "\ud800")
                throw "XML document or namespace";
            const htmlNS = "http://www.w3.org/1999/xhtml";
            const xhtml = api.createDocument(htmlNS, "html");
            const htmlElement = xhtml.createElement("Mixed");
            xhtml.documentElement.appendChild(htmlElement);
            htmlElement.setAttribute("Case", "yes");
            if (xhtml.contentType !== "application/xhtml+xml" ||
                htmlElement.namespaceURI !== htmlNS || htmlElement.tagName !== "Mixed" ||
                htmlElement.getAttribute("case") !== null ||
                xhtml.getElementsByTagName("Mixed")[0] !== htmlElement ||
                xhtml.getElementsByTagName("MIXED").length !== 0)
                throw "XHTML is case-sensitive";
            const svgNS = "http://www.w3.org/2000/svg";
            const svg = api.createDocument(svgNS, "svg");
            const svgNode = document.createElementNS(svgNS, "svg:linearGradient");
            if (svg.contentType !== "image/svg+xml" ||
                svgNode.localName !== "linearGradient" || svgNode.tagName !== "svg:linearGradient")
                throw "SVG factory";
            svgNode.setAttributeNS("urn:paint", "old:Paint", "\ud800");
            svgNode.setAttributeNS("urn:paint", "new:Paint", "blue");
            if (!svgNode.hasAttributeNS("urn:paint", "Paint") ||
                svgNode.getAttributeNS("urn:paint", "Paint") !== "blue" ||
                svgNode.getAttribute("old:Paint") !== "blue" ||
                svgNode.getAttribute("new:Paint") !== null)
                throw "expanded name update";
            svgNode.removeAttributeNS("urn:paint", "Paint");
            if (svgNode.hasAttributeNS("urn:paint", "Paint")) throw "expanded name removal";
            const html = api.createHTMLDocument("ship \ud800");
            if (html.contentType !== "text/html" || html.defaultView !== null ||
                html.doctype.name !== "html" || html.documentElement.tagName !== "HTML" ||
                html.querySelector("head > title").firstChild.data !== "ship \ud800" ||
                html.querySelector("body").tagName !== "BODY")
                throw "HTML skeleton";
            for (const [call, expected] of [
                [() => document.createElementNS(null, "p:name"), "NamespaceError"],
                [() => document.createElementNS("urn:x", "xml:name"), "NamespaceError"],
                [() => document.createElementNS("http://www.w3.org/2000/xmlns/", "name"), "NamespaceError"],
                [() => xml.createElementNS("urn:x", ":bad"), "InvalidCharacterError"],
                [() => xml.createCDATASection("]]>"), "InvalidCharacterError"],
                [() => api.createDocumentType("bad name", "", ""), "InvalidCharacterError"],
                [() => api.createDocument("urn:x", "root", {}), "TypeError"]
            ]) {
                let found;
                try { call(); } catch (error) { found = error.name; }
                if (found !== expected) throw "unexpected factory exception";
            }
            return true;
        })();
    "#).unwrap(), Value::TRUE);
}

#[test]
fn cross_document_moves_preserve_wrappers_and_revalidate_after_coercion() {
    let tree = Rc::new(Tree(RefCell::new(
        Document::new(DocumentLimits::default()).unwrap(),
    )));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree).unwrap();
    agent.heap.set_gc_stress(true);
    assert_eq!(agent.evaluate(r#"
        (() => {
            const a = document.implementation.createDocument(null, "a");
            const b = document.implementation.createDocument(null, "b");
            const root = a.documentElement;
            const child = a.createElement("Child");
            const text = a.createTextNode("ab");
            root.appendChild(child);
            child.appendChild(text);
            const children = child.children;
            const nodes = child.childNodes;
            child.saved = {marker: 7};
            const shallow = b.importNode(child);
            const deep = b.importNode(child, true);
            const dictionary = b.importNode(child, {selfOnly: false});
            const clone = child.cloneNode(true);
            if (shallow.firstChild !== null || deep.firstChild.data !== "ab" ||
                dictionary.firstChild.data !== "ab" || clone.firstChild.data !== "ab" ||
                deep.firstChild === text || deep.ownerDocument !== b || clone.ownerDocument !== a ||
                deep.saved !== undefined || child.parentNode !== root)
                throw "cross document import and clone";
            let rejected;
            try { b.appendChild(child); } catch (error) { rejected = error.name; }
            if (rejected !== "HierarchyRequestError" || child.parentNode !== root ||
                text.ownerDocument !== a || root.firstChild !== child)
                throw "failed append changed source tree";
            const split = text.splitText({valueOf() {
                if (b.adoptNode(child) !== child) throw "adopt returns the same wrapper";
                return 1;
            }});
            if (root.firstChild !== null || child.parentNode !== null ||
                child.ownerDocument !== b || text.ownerDocument !== b || split.ownerDocument !== b ||
                child.children !== children || child.childNodes !== nodes ||
                nodes.length !== 2 || nodes[1] !== split || child.saved.marker !== 7 ||
                text.data !== "a" || split.data !== "b")
                throw "adoption and reentrant mutation";
            b.documentElement.appendChild(child);
            const sentinel = b.createElement("sentinel");
            child.appendChild(sentinel);
            const fragment = a.createDocumentFragment();
            const first = a.createElement("first");
            const second = a.createElement("second");
            fragment.appendChild(first);
            fragment.appendChild(second);
            const fragmentChildren = fragment.children;
            if (child.insertBefore(fragment, sentinel) !== fragment || fragment.ownerDocument !== a ||
                fragment.children !== fragmentChildren || fragmentChildren.length !== 0 ||
                children.length !== 3 || children[0] !== first || children[1] !== second ||
                children[2] !== sentinel ||
                first.ownerDocument !== b || second.parentNode !== child)
                throw "foreign fragment contents";
            const clonedDocument = b.cloneNode(true);
            if (clonedDocument === b || clonedDocument.contentType !== b.contentType ||
                clonedDocument.documentElement.firstChild.localName !== "Child" ||
                clonedDocument.documentElement.ownerDocument !== clonedDocument ||
                b.cloneNode().documentElement !== null)
                throw "document cloning";
            try { b.removeChild(a.documentElement); throw "removed foreign node"; }
            catch (error) { if (error.name !== "NotFoundError") throw error; }
            try { b.importNode(a); throw "imported a Document"; }
            catch (error) { if (error.name !== "NotSupportedError") throw error; }
            try { b.adoptNode(a); throw "adopted a Document"; }
            catch (error) { if (error.name !== "NotSupportedError") throw error; }
            try { b.appendChild(a.implementation); throw "implementation accepted as a Node"; }
            catch (error) { if (error.name !== "TypeError") throw error; }
            return true;
        })();
    "#).unwrap(), Value::TRUE);
}

#[test]
fn auxiliary_documents_are_collected_when_their_last_wrapper_dies() {
    let tree = Rc::new(Tree(RefCell::new(
        Document::new(DocumentLimits::default()).unwrap(),
    )));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree).unwrap();
    agent.heap.set_gc_stress(true);
    assert_eq!(
        agent
            .evaluate(
                r#"
        var kept = [];
        var aNode, anImplementation;
        (() => {
            const doc = document.implementation.createDocument(null, "tree");
            doc.marker = 4;
            aNode = doc.createElement("detached");
            anImplementation = doc.implementation;
        })();
        (() => {
            if (aNode.ownerDocument.marker !== 4 ||
                aNode.ownerDocument.implementation !== anImplementation)
                throw "lost the owning document of a detached node";
            aNode = null;
            if (anImplementation.createDocumentType("root", "", "").ownerDocument.marker !== 4)
                throw "lost the owning document of DOMImplementation";
            anImplementation = null;
        })();
        for (let i = 0; i < 16; i++) kept.push(new Document());
        let errorName;
        try { kept.push(new Document()); } catch (error) { errorName = error.name; }
        errorName === "QuotaExceededError" && kept.length === 16;
    "#
            )
            .unwrap(),
        Value::TRUE
    );
    assert_eq!(
        agent
            .evaluate(
                r#"
        kept = null;
        for (let i = 0; i < 64; i++) {
            (() => {
                const fresh = new Document();
                fresh.cycle = fresh.implementation;
                fresh.implementation.cycle = fresh;
            })();
        }
        new Document().defaultView === null && document.defaultView === window;
    "#
            )
            .unwrap(),
        Value::TRUE
    );
}

#[test]
fn wasm_modules_snapshot_views_and_expose_fresh_ordered_metadata() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    agent.evaluate(r#"
        function assert(value, label) { if (!value) throw label; }
        function throws(type, run) {
            try { run(); } catch (error) { if (error instanceof type) return; throw error; }
            throw 'expected ' + type.name;
        }
        const raw = [
            0,97,115,109,1,0,0,0,
            0,5,1,120,1,2,3,
            1,4,1,96,0,0,
            2,7,1,1,109,1,102,0,0,
            7,5,1,1,102,0,0,
            0,4,1,120,8,9,
            0,1,0
        ];
        const backing = new Uint8Array(raw.length + 6);
        backing.set(raw, 3);
        const view = new DataView(backing.buffer, 3, raw.length);
        assert(WebAssembly.validate(view) && !WebAssembly.validate(backing), 'view range');
        const Module = WebAssembly.Module;
        class Derived extends Module {}
        const m = new Derived(view);
        assert(m instanceof Derived && Object.prototype.toString.call(m) === '[object WebAssembly.Module]', 'prototype');
        assert(Object.prototype.toString.call(WebAssembly) === '[object WebAssembly]', 'namespace');
        backing.fill(0);
        const getImports = Module.imports;
        const first = getImports(m);
        assert(first instanceof Array && first.length === 1, 'array of imports');
        assert(first[0].module === 'm' && first[0].name === 'f' && first[0].kind === 'function', 'import shape');
        first[0].name = 'wrong';
        assert(getImports(m)[0].name === 'f', 'fresh descriptor');
        assert(Module.exports(m)[0].name === 'f', 'export name');
        const x = Module.customSections(m, 'x');
        assert(x.length === 2 && new Uint8Array(x[0]).join() === '1,2,3' && new Uint8Array(x[1]).join() === '8,9', 'custom order');
        new Uint8Array(x[0])[0] = 99;
        assert(new Uint8Array(Module.customSections(m, 'x')[0])[0] === 1, 'fresh data copy');
        assert(Module.customSections(m, '')[0].byteLength === 0, 'empty custom payload');
        assert(Module.customSections(m, '\uD800').length === 0, 'DOMString exact comparison');
        assert(!WebAssembly.validate(new Uint8Array(raw), { builtins: ['same','same'] }), 'duplicate builtins');
        assert(WebAssembly.validate(new Uint8Array(raw), { builtins: ['unknown'] }), 'unrecognized builtin');
        assert(!WebAssembly.validate(new Uint8Array(raw), { importedStringConstants: 'm' }), 'matching unsupported string imports');
        const resizable = new ArrayBuffer(raw.length, {maxByteLength: raw.length + 2});
        new Uint8Array(resizable).set(raw);
        assert(WebAssembly.validate(resizable), 'resizable');
        const detached = resizable.transfer();
        assert(!WebAssembly.validate(resizable) && WebAssembly.validate(detached), 'detached buffer');
        throws(TypeError, () => WebAssembly.validate({byteLength: 8}));
        throws(TypeError, () => Module(new Uint8Array(raw)));
        throws(TypeError, () => Module.imports(Object.create(Module.prototype)));
        throws(TypeError, () => Module.customSections(m));
        throws(WebAssembly.CompileError, () => new Module(view));
        const e = WebAssembly.CompileError('message', {cause: 2});
        assert(e instanceof Error && Error.isError(e) && e.cause === 2 && e.message === 'message', 'actual error');
    "#).unwrap();
}

#[test]
fn wasm_compilation_promises_snapshot_input_and_use_branded_response_metadata() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    agent.evaluate(r#"
        const bytes = Uint8Array.of(0,97,115,109,1,0,0,0);
        const source = bytes.slice();
        let results = [];
        const compile = WebAssembly.compile(source);
        source.fill(0);
        compile.then(m => results.push(m instanceof WebAssembly.Module));
        WebAssembly.compile(source).catch(e => results.push(e instanceof WebAssembly.CompileError));
        WebAssembly.compile({}).catch(e => results.push(e instanceof TypeError));
        const make = (type, status = 200) => new Response(bytes, {status, headers: {'Content-Type': type}});
        const good = make(' Application/Wasm\t ');
        Object.defineProperty(good, 'arrayBuffer', {get() { throw 'must use response internal state' }});
        Object.defineProperty(good, 'headers', {get() { throw 'must use response internal state' }});
        const wrapped = {then(resolve) { resolve(good); }};
        WebAssembly.compileStreaming(wrapped).then(m => {
            results.push(m instanceof WebAssembly.Module && good.bodyUsed);
            WebAssembly.compileStreaming(good).catch(e => results.push(e instanceof TypeError));
        });
        const bad = make('application/wasm;');
        WebAssembly.compileStreaming(bad).catch(e => results.push(e instanceof TypeError && !bad.bodyUsed));
        const missing = new Response(bytes);
        WebAssembly.compileStreaming(missing).catch(e => results.push(e instanceof TypeError && !missing.bodyUsed));
        const badStatus = make('application/wasm', 404);
        WebAssembly.compileStreaming(badStatus).catch(e => results.push(e instanceof TypeError && !badStatus.bodyUsed));
        const forged = {headers: { 'Content-Type': 'application/wasm' }, status: 200, arrayBuffer() { return bytes.buffer; }};
        WebAssembly.compileStreaming(forged).catch(e => results.push(e instanceof TypeError));
        const reason = new Error('reject source');
        WebAssembly.compileStreaming(Promise.reject(reason)).catch(e => results.push(e === reason));
        const broken = {get then() { throw reason; }};
        WebAssembly.compileStreaming(broken).catch(e => results.push(e === reason));
    "#).unwrap();
    agent.run_jobs().unwrap();
    let outcome = agent
        .evaluate("results.length === 11 && results.every(x => x)")
        .unwrap();
    assert_eq!(outcome, Value::TRUE);
}

#[test]
fn wasm_compilation_policy_applies_to_constructor_and_promises() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    page::set_wasm_compilation_policy(
        &mut agent,
        rustic_net::csp::Verdict {
            blocked: true,
            ..Default::default()
        },
    )
    .unwrap();
    agent.evaluate(r#"
        const b = Uint8Array.of(0,97,115,109,1,0,0,0);
        let allowed = WebAssembly.validate(b) && eval('40 + 2') === 42;
        let blocked = 0;
        function failure(e) { if (e instanceof WebAssembly.CompileError) blocked++; }
        try { new WebAssembly.Module(b); } catch (e) { failure(e); }
        WebAssembly.compile(b).catch(failure);
        WebAssembly.compileStreaming(new Response(b, {headers:{'Content-Type': 'application/wasm'}})).catch(failure);
    "#).unwrap();
    agent.run_jobs().unwrap();
    assert_eq!(
        agent.evaluate("allowed && blocked === 3").unwrap(),
        Value::TRUE
    );
}

#[test]
fn wasm_validation_charges_native_work_and_propagates_cancellation() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    let validate = agent.evaluate("WebAssembly.validate").unwrap();
    let bytes = agent.evaluate("new ArrayBuffer(4096)").unwrap();
    let args = &[bytes];
    agent.set_step_budget(Some(1));
    assert!(matches!(
        agent.call_from_host(validate, Value::UNDEFINED, args),
        Err(rustic_js::runtime::Abrupt::Cancelled)
    ));
    agent.set_step_budget(None);
    assert_eq!(
        agent
            .call_from_host(validate, Value::UNDEFINED, args)
            .unwrap(),
        Value::FALSE
    );
    let token = rustic_base::CancellationToken::default();
    token.cancel();
    agent.set_cancellation(Some(token));
    assert!(matches!(
        agent.call_from_host(validate, Value::UNDEFINED, args),
        Err(rustic_js::runtime::Abrupt::Cancelled)
    ));
}

#[test]
fn wasm_instances_keep_export_identity_owned_state_and_numeric_conversions() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    agent.evaluate(r#"
        function assert(result, reason) { if (!result) throw reason; }
        function throws(type, run) {
            try { run(); } catch (error) { if (error instanceof type) return; throw error; }
            throw 'expected ' + type.name;
        }
        const source = Uint8Array.of(
            0,97,115,109,1,0,0,0,
            1,15,3,96,1,127,1,127,96,1,126,2,126,124,96,0,0,
            3,5,4,0,1,2,2,
            6,6,1,127,1,65,7,11,
            7,29,4,3,97,100,100,0,0,5,97,108,105,97,115,0,0,
            4,112,97,105,114,0,1,4,116,114,97,112,0,2,
            8,1,3,
            10,38,4,
               11,0,35,0,32,0,106,36,0,35,0,11,
               13,0,32,0,68,0,0,0,0,0,0,0,128,11,
               3,0,0,11,
               6,0,65,9,36,0,11
        );
        const mod = new WebAssembly.Module(source);
        source.fill(0);
        class Derived extends WebAssembly.Instance {}
        const first = new Derived(mod);
        const second = new WebAssembly.Instance(mod);
        const exp = first.exports;
        assert(first instanceof Derived && first.exports === exp, 'instance identity');
        assert(Object.getPrototypeOf(exp) === null && Object.isFrozen(exp), 'exports shape');
        assert(exp.add === exp.alias && exp.add.name === '0' && exp.add.length === 1, 'alias identity');
        const saved = (new WebAssembly.Instance(mod)).exports.add;
        assert(saved(1) === 10, 'export roots its instance');
        assert(exp.add(2**32 + 3) === 12, 'i32 coercion');
        assert(exp.add({valueOf(){ return exp.add(1); }}) === 26, 'reentrant conversion');
        assert(second.exports.add(0) === 9, 'independent state and start');
        const pair = exp.pair(-123n);
        assert(Array.isArray(pair) && pair[0] === -123n && Object.is(pair[1], -0), 'multi-value');
        throws(TypeError, () => exp.pair(12));
        throws(TypeError, () => new exp.add(1));
        throws(WebAssembly.RuntimeError, exp.trap);
        throws(TypeError, () => WebAssembly.Instance.prototype.exports);
        throws(TypeError, () => new WebAssembly.Instance(source));

        const imported = new WebAssembly.Module(Uint8Array.of(
            0,97,115,109,1,0,0,0, 1,4,1,96,0,0, 2,7,1,1,109,1,102,0,0
        ));
        const badStart = new WebAssembly.Module(Uint8Array.of(
            0,97,115,109,1,0,0,0, 1,4,1,96,0,0, 3,2,1,0, 8,1,0, 10,5,1,3,0,0,11
        ));
        throws(TypeError, () => new WebAssembly.Instance(imported));
        throws(TypeError, () => new WebAssembly.Instance(imported, {}));
        throws(WebAssembly.LinkError, () => new WebAssembly.Instance(imported, {m:{}}));
        throws(WebAssembly.RuntimeError, () => new WebAssembly.Instance(badStart));

        const empty = Uint8Array.of(0,97,115,109,1,0,0,0);
        let results = [];
        WebAssembly.instantiate(mod).then(i => results.push(i.exports.add(1) === 10));
        WebAssembly.instantiate(empty).then(x => results.push(x.module instanceof WebAssembly.Module && x.instance instanceof WebAssembly.Instance));
        const response = new Response(empty, {headers: {'content-type':'application/wasm'}});
        WebAssembly.instantiateStreaming(response).then(x => results.push(x.instance instanceof WebAssembly.Instance && response.bodyUsed));
        WebAssembly.instantiate(empty, 12).catch(e => results.push(e instanceof TypeError));
        WebAssembly.instantiate(imported, {m:{}}).catch(e => results.push(e instanceof WebAssembly.LinkError));
        WebAssembly.instantiate(badStart).catch(e => results.push(e instanceof WebAssembly.RuntimeError));
    "#).unwrap();
    agent.run_jobs().unwrap();
    assert_eq!(
        agent
            .evaluate("results.length === 6 && results.every(x => x)")
            .unwrap(),
        Value::TRUE,
    );
}

#[test]
fn wasm_numeric_globals_share_live_state_with_exported_functions() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    assert_eq!(
        agent
            .evaluate(
                r#"
        function assert(ok, message) { if (!ok) throw message; }
        const source = Uint8Array.of(
            0,97,115,109,1,0,0,0,
            1,6,1,96,1,127,1,127,
            3,2,1,0,
            6,11,2,127,1,65,7,11,127,0,65,9,11,
            7,35,4,3,97,100,100,0,0,6,115,104,97,114,101,100,3,0,
            5,97,108,105,97,115,3,0,8,99,111,110,115,116,97,110,116,3,1,
            10,13,1,11,0,35,0,32,0,106,36,0,35,0,11
        );
        const mod = new WebAssembly.Module(source);
        const independent = new WebAssembly.Instance(mod).exports;
        let exports = new WebAssembly.Instance(mod).exports;
        const shared = exports.shared;
        assert(shared instanceof WebAssembly.Global && shared === exports.alias, 'identity');
        assert(shared.valueOf() === 7 && exports.constant.value === 9, 'initial values');
        let coerced = false;
        let rejected = false;
        try { exports.constant.value = {valueOf() { coerced = true; return 1; }}; }
        catch (error) { rejected = error instanceof TypeError; }
        assert(rejected && !coerced && exports.constant.value === 9, 'immutable without coercion');
        shared.value = 100;
        assert(exports.add(4) === 104 && shared.value === 104, 'bidirectional mutation');
        shared.value = {valueOf() { return exports.add(1) + 1; }};
        assert(shared.value === 106 && independent.shared.value === 7, 'reentrancy and isolation');
        exports = null;
        shared.value = 15;
        assert(shared.value === 15, 'global keeps the instance alive');
        class Scalar extends WebAssembly.Global {}
        const scalar = new Scalar({value:'i64', mutable:true});
        assert(scalar instanceof Scalar && scalar.value === 0n, 'standalone subclass');
        scalar.value = 1n << 63n;
        assert(scalar.value === -(1n << 63n), 'signed i64');
        true
    "#,
            )
            .unwrap(),
        Value::TRUE,
    );
}

#[test]
fn wasm_numeric_function_imports_preserve_link_order_reentry_and_exceptions() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    agent.evaluate(r#"
        function assert(ok, message) { if (!ok) throw message; }
        function caught(run) {
            try { run(); } catch (error) { return error; }
            throw 'expected exception';
        }
        const bytes = Uint8Array.of(
            0,97,115,109,1,0,0,0,
            1,9,2,96,1,127,1,127,96,0,0,
            2,17,2,1,109,2,102,110,0,0,1,109,4,105,110,105,116,0,1,
            3,4,3,0,0,1,
            6,6,1,127,1,65,0,11,
            7,46,5,7,102,111,114,119,97,114,100,0,2,4,98,117,109,112,0,3,
            6,109,97,114,107,101,114,3,0,8,99,97,108,108,98,97,99,107,0,0,
            5,97,108,105,97,115,0,0,
            8,1,4,
            10,25,3,9,0,32,0,16,0,35,0,106,11,
            8,0,32,0,36,0,35,0,11,4,0,16,1,11
        );
        const mod = new WebAssembly.Module(bytes);
        let order = [];
        const sentinel = {origin:'callback'};
        let mode = '';
        let exports;
        function callback(v) {
            'use strict';
            assert(this === undefined && typeof v === 'number', 'JS call ABI');
            if (mode === 'throw') throw sentinel;
            if (mode === 'recursive') return exports.forward(v);
            exports.bump(v * 2);
            return v + 1;
        }
        const imports = {
            get m() {
                order.push('m');
                return {
                    get fn() { order.push('fn'); return callback; },
                    get init() { order.push('init'); return () => order.push('start'); },
                };
            },
        };
        exports = new WebAssembly.Instance(mod, imports).exports;
        assert(order.join() === 'm,fn,m,init,start', 'import read and start order');
        assert(exports.callback === exports.alias && exports.callback !== callback, 'host wrapper identity');
        assert(exports.forward(5) === 16 && exports.marker.value === 10, 'same-instance reentry');
        mode = 'throw';
        assert(caught(() => exports.forward(1)) === sentinel, 'exact JS exception');
        mode = 'recursive';
        const depthError = caught(() => exports.forward(1));
        assert(depthError instanceof WebAssembly.RuntimeError || depthError instanceof RangeError, 'bounded reentry');
        mode = '';
        assert(exports.forward(3) === 10, 'reentry depth recovered');
        const linked = new WebAssembly.Instance(mod, {m:{fn:exports.bump, init(){}}}).exports;
        assert(linked.callback === exports.bump && linked.alias === exports.bump, 'direct Wasm identity');
        assert(linked.forward(29) === 29 && exports.marker.value === 29, 'direct Wasm invocation');
        const raw = new WebAssembly.Instance(new WebAssembly.Module(Uint8Array.of(
            0,97,115,109,1,0,0,0,
            1,5,1,96,0,1,125, 3,2,1,0, 7,7,1,3,114,97,119,0,0,
            10,9,1,7,0,67,52,18,192,127,11
        ))).exports.raw;
        const bits = new WebAssembly.Instance(new WebAssembly.Module(Uint8Array.of(
            0,97,115,109,1,0,0,0, 1,9,2,96,0,1,125,96,0,1,127,
            2,9,1,1,109,3,114,97,119,0,0, 3,2,1,1, 7,8,1,4,98,105,116,115,0,1,
            10,7,1,5,0,16,0,188,11
        )), {m:{raw}}).exports.bits;
        assert(bits() === 0x7fc01234, 'Wasm-to-Wasm calls preserve raw NaN bits');
        const different = new WebAssembly.Instance(new WebAssembly.Module(Uint8Array.of(
            0,97,115,109,1,0,0,0, 1,4,1,96,0,0, 3,2,1,0,
            7,5,1,1,102,0,0, 10,4,1,2,0,11
        ))).exports.f;
        assert(caught(() => new WebAssembly.Instance(mod, {m:{fn:different, init(){}}})) instanceof WebAssembly.LinkError, 'exact link signature');
        assert(caught(() => new WebAssembly.Instance(mod, {get m(){ throw sentinel; }})) === sentinel, 'getter exception');
        let outcome = [];
        WebAssembly.instantiate(mod, {m:{fn:callback, init(){throw sentinel;}}}).catch(error => outcome.push(error === sentinel));
        WebAssembly.instantiateStreaming(
            new Response(bytes, {headers:{'content-type':'application/wasm'}}),
            {m:{get fn(){ const amount = 8; return v => v + amount; }, init(){}}}
        ).then(({instance}) => outcome.push(instance.exports.forward(5) === 13));
    "#).unwrap();
    agent.run_jobs().unwrap();
    assert_eq!(
        agent
            .evaluate("outcome.length === 2 && outcome.every(Boolean)")
            .unwrap(),
        Value::TRUE
    );
}

#[test]
fn wasm_numeric_global_imports_share_cells_and_snapshot_primitive_values() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    agent.evaluate(r#"
        function assert(ok, message) { if (!ok) throw message; }
        const provider = new WebAssembly.Instance(new WebAssembly.Module(Uint8Array.of(
            0,97,115,109,1,0,0,0, 6,6,1,127,1,65,7,11, 7,5,1,1,103,3,0
        )));
        const shared = provider.exports.g;
        const source = Uint8Array.of(
            0,97,115,109,1,0,0,0,
            1,12,3,96,0,1,127,96,1,127,0,96,0,0,
            2,28,4,1,109,1,103,3,127,1, 1,109,1,102,0,0,
            1,109,1,107,3,127,0, 1,109,1,98,3,126,0,
            3,4,3,0,1,2,
            6,6,1,127,0,35,1,11,
            7,36,6,1,103,3,0, 4,115,97,109,101,3,0, 3,98,105,103,3,2,
            4,115,110,97,112,3,3, 2,103,111,0,1, 3,115,101,116,0,2,
            8,1,3,
            10,29,3,10,0,16,0,35,0,106,35,1,106,11,
            6,0,32,0,36,0,11, 9,0,35,0,65,1,106,36,0,11
        );
        const module = new WebAssembly.Module(source);
        let coerced = false;
        function rejects(override) {
            try {
                new WebAssembly.Instance(module, {m:{g:shared, f:() => 5, k:9, b:0n, ...override}});
            } catch(e) {
                return e instanceof WebAssembly.LinkError;
            }
            return false;
        }
        assert(rejects({g:7}), 'mutable imports require a mutable Global');
        assert(rejects({g:new WebAssembly.Global({value:'i32'}, 7)}), 'exact mutability');
        assert(rejects({g:new WebAssembly.Global({value:'f64',mutable:true}, 7)}), 'exact type');
        assert(rejects({k:{valueOf() { coerced = true; return 9; }}}), 'no object coercion');
        assert(rejects({b:2}) && !coerced && shared.value === 7, 'no start on failed link');

        let order = [];
        let k = 2**32 + 9;
        const namespace = {
            get g() { order.push('g'); return shared; },
            get f() { order.push('f'); return () => 5; },
            get k() { order.push('k'); return k; },
            get b() { order.push('b'); return 2n**63n + 4n; },
        };
        let result = false;
        const pending = WebAssembly.instantiate(module, {
            get m() { order.push('m'); return namespace; },
        });
        assert(order.join() === 'm,g,m,f,m,k,m,b', 'ordered synchronous lookup');
        assert(shared.value === 7, 'start remains deferred');
        k = 99;
        shared.value = 8;
        pending.then(instance => {
            const exports = instance.exports;
            assert(exports.g === shared && exports.same === shared, 're-export identity');
            assert(exports.go() === 23 && exports.snap.value === 9, 'primitive snapshot and initializer');
            assert(exports.big.value === -(2n**63n) + 4n, 'BigInt import');
            exports.set(30);
            assert(provider.exports.g.value === 30, 'guest writes reach provider');
            const second = new WebAssembly.Instance(module, {m:{g:shared, f:() => 0, k:2, b:11n}}).exports;
            assert(exports.g.value === 31, 'second start updates the shared slot');
            second.set(4);
            assert(exports.go() === 18 && second.go() === 6, 'same slot across independent instances');
            result = true;
        });
    "#).unwrap();
    agent.run_jobs().unwrap();
    assert_eq!(agent.evaluate("result").unwrap(), Value::TRUE);
}

#[test]
fn wasm_module_import_lookup_charges_for_long_names_before_reading_properties() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    let module = agent
        .evaluate(
            r#"
        const name = new Uint8Array(1024).fill(109);
        new WebAssembly.Module(Uint8Array.of(
            0,97,115,109,1,0,0,0, 1,4,1,96,0,0,
            2,135,8,1,128,8,...name,1,102,0,0
        ));
    "#,
        )
        .unwrap();
    let instantiate = agent.evaluate("WebAssembly.instantiate").unwrap();
    let imports = agent.evaluate("({})").unwrap();
    agent.set_step_budget(Some(8));
    assert!(matches!(
        agent.call_from_host(instantiate, Value::UNDEFINED, &[module, imports]),
        Err(rustic_js::runtime::Abrupt::Cancelled)
    ));
}

#[test]
fn wasm_fixed_memories_share_live_bytes_and_detach_views_on_successful_growth() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    let outcome = agent.evaluate(r#"
        function assert(ok, message) { if (!ok) throw message; }
        function rejects(type, call) {
            try { call(); } catch (error) { return error instanceof type; }
            return false;
        }
        const code = Uint8Array.of(
            0,97,115,109,1,0,0,0,
            1,11,2,96,1,127,1,127,96,2,127,127,0, 3,4,3,0,1,0,
            5,4,1,1,1,3,
            7,37,5,3,109,101,109,2,0, 5,97,108,105,97,115,2,0,
            4,108,111,97,100,0,0, 5,115,116,111,114,101,0,1, 4,103,114,111,119,0,2,
            10,26,3,7,0,32,0,45,0,0,11, 9,0,32,0,32,1,58,0,0,11, 6,0,32,0,64,0,11,
            11,7,1,0,65,0,11,1,5
        );
        const consumer = new WebAssembly.Module(Uint8Array.of(
            0,97,115,109,1,0,0,0, 2,11,1,1,109,3,109,101,109,2,1,2,3,
            7,7,1,3,109,101,109,2,0
        ));
        const {mem, alias, load, store, grow} = new WebAssembly.Instance(new WebAssembly.Module(code)).exports;
        assert(mem instanceof WebAssembly.Memory && mem === alias, 'local memory alias');
        assert(rejects(WebAssembly.LinkError, () => new WebAssembly.Instance(consumer, {m:{mem}})), 'minimum');
        const old = mem.buffer;
        const bytes = new Uint8Array(old);
        assert(old === alias.buffer && old === mem.toFixedLengthBuffer() && old.byteLength === 65536, 'fixed buffer');
        assert(bytes[0] === 5, 'active data');
        new DataView(old).setUint8(1, 44);
        assert(load(1) === 44, 'JS write visible to guest');
        store(0, 39);
        assert(bytes[0] === 39, 'guest write visible to JS');
        assert(rejects(TypeError, () => old.transfer()) && old === mem.buffer, 'protected detachment');
        assert(grow(1) === 1 && old.detached && bytes.length === 0, 'guest growth detaches views');
        const larger = mem.buffer;
        assert(larger !== old && larger.byteLength === 131072, 'replacement buffer');
        assert(new Uint8Array(larger)[0] === 39 && load(65536) === 0, 'prefix and zeroed tail');
        const linked = new WebAssembly.Instance(consumer, {m:{mem}});
        assert(linked.exports.mem === mem, 'memory import and re-export identity');
        const tooWide = new WebAssembly.Memory({initial:2,maximum:4});
        assert(rejects(WebAssembly.LinkError, () => new WebAssembly.Instance(consumer, {m:{mem:tooWide}})), 'maximum');
        assert(rejects(RangeError, () => linked.exports.mem.grow(2)) && mem.buffer === larger && !larger.detached, 'failed host growth');
        assert(linked.exports.mem.grow(0) === 2 && larger.detached, 'zero host growth detaches');
        assert(load(0) === 39, 'retained value after refresh');
        const onlyBuffer = (new WebAssembly.Memory({initial:1,maximum:1})).buffer;
        new Uint8Array(onlyBuffer)[0] = 17;
        assert(new DataView(onlyBuffer).getUint8(0) === 17, 'attached buffer keeps its owner');
        true
    "#);
    if let Err(rustic_js::EvalError::Throw(value)) = outcome {
        panic!("{}", agent.describe_value(value));
    }
    assert_eq!(outcome.unwrap(), Value::TRUE);
}

#[test]
fn wasm_memory64_uses_bigint_counts_and_links_only_the_same_address_width() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    let outcome = agent.evaluate(r#"
        function assert(ok, message) { if (!ok) throw message; }
        function rejects(type, call) {
            try { call(); } catch (error) { return error instanceof type; }
            return false;
        }
        const producer = Uint8Array.of(
            0,97,115,109,1,0,0,0,
            1,16,3,96,1,126,1,127,96,2,126,127,0,96,1,126,1,126,
            3,4,3,0,1,2, 5,4,1,5,1,3,
            7,37,5,3,109,101,109,2,0, 5,97,108,105,97,115,2,0,
            4,108,111,97,100,0,0, 5,115,116,111,114,101,0,1, 4,103,114,111,119,0,2,
            10,26,3,7,0,32,0,45,0,0,11, 9,0,32,0,32,1,58,0,0,11, 6,0,32,0,64,0,11,
            11,7,1,0,66,0,11,1,5
        );
        const importer = new WebAssembly.Module(Uint8Array.of(
            0,97,115,109,1,0,0,0, 2,11,1,1,109,3,109,101,109,2,5,2,3,
            7,7,1,3,109,101,109,2,0
        ));
        assert(WebAssembly.validate(producer), 'valid i64 address module');
        const {mem, alias, load, store, grow} = new WebAssembly.Instance(new WebAssembly.Module(producer)).exports;
        assert(mem === alias && mem instanceof WebAssembly.Memory && load(0n) === 5, 'i64 active data');
        const old = mem.buffer;
        new DataView(old).setUint8(1, 42);
        assert(load(1n) === 42, 'live host write');
        store(0n, 27);
        assert(new Uint8Array(old)[0] === 27, 'live guest write');
        assert(rejects(WebAssembly.RuntimeError, () => load(4294967296n)), 'no address truncation');
        assert(rejects(TypeError, () => mem.grow(1)), 'i64 growth requires BigInt conversion');
        assert(rejects(TypeError, () => mem.grow(-1n)), 'negative BigInt');
        assert(rejects(TypeError, () => mem.grow(18446744073709551616n)), 'BigInt beyond u64');
        assert(rejects(RangeError, () => mem.grow(18446744073709551615n)), 'u64 beyond page limit');
        assert(grow(-1n) === -1n && old === mem.buffer && !old.detached, 'rejected guest growth');
        assert(grow(1n) === 1n && old.detached, 'guest growth detaches');
        const larger = mem.buffer;
        assert(larger.byteLength === 131072 && load(0n) === 27 && load(65536n) === 0, 'grown memory');
        assert(new WebAssembly.Instance(importer, {m:{mem}}).exports.mem === mem, 'live i64 import');
        assert(mem.grow(0n) === 2n && larger.detached, 'zero BigInt growth detaches');
        const wrong = new WebAssembly.Memory({initial:2, maximum:3});
        assert(rejects(WebAssembly.LinkError, () => new WebAssembly.Instance(importer, {m:{mem:wrong}})), 'width mismatch');
        const standalone = new WebAssembly.Memory({address:'i64', initial:'2', maximum:3n});
        assert(new WebAssembly.Instance(importer, {m:{mem:standalone}}).exports.mem === standalone, 'standalone i64 import');
        assert(rejects(TypeError, () => new WebAssembly.Memory({address:'i64', initial:1})), 'Number has the wrong type');
        assert(rejects(RangeError, () => new WebAssembly.Memory({address:'i64', initial:129n})), 'allocation cap');
        assert(rejects(RangeError, () => new WebAssembly.Memory({address:'i64', initial:0n, maximum:281474976710657n})), 'page maximum');
        true
    "#);
    if let Err(rustic_js::EvalError::Throw(value)) = outcome {
        panic!("{}", agent.describe_value(value));
    }
    assert_eq!(outcome.unwrap(), Value::TRUE);
}

#[test]
fn wasm_resizable_buffers_follow_guest_and_host_growth_and_detach_on_mode_switch() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    let outcome = agent.evaluate(r#"
        function assert(ok, message) { if (!ok) throw message; }
        function rejects(type, call) {
            try { call(); } catch (error) { return error instanceof type; }
            return false;
        }
        const pageSize = 65536;
        const mem = new WebAssembly.Memory({initial:1, maximum:4});
        const fixed = mem.buffer;
        new Uint8Array(fixed)[0] = 19;
        const expanded = mem.toResizableBuffer();
        assert(fixed.detached && expanded.resizable && expanded.maxByteLength === 4 * pageSize, 'mode change');
        assert(mem.buffer === expanded && mem.toResizableBuffer() === expanded, 'stable identity');
        const tracking = new Uint8Array(expanded);
        const data = new DataView(expanded);
        const short = new Uint8Array(expanded, 0, 1);
        const module = new WebAssembly.Module(Uint8Array.of(
            0,97,115,109,1,0,0,0,
            1,10,2,96,1,127,1,127,96,0,1,127,
            2,11,1,1,109,3,109,101,109,2,1,1,4,
            3,3,2,0,1,
            7,21,3,3,109,101,109,2,0,4,103,114,111,119,0,0,4,115,105,122,101,0,1,
            10,13,2,6,0,32,0,64,0,11,4,0,63,0,11
        ));
        const guest = new WebAssembly.Instance(module, {m:{mem}}).exports;
        assert(guest.mem === mem && guest.grow(1) === 1 && mem.buffer === expanded, 'guest growth');
        assert(tracking.length === 2 * pageSize && data.byteLength === 2 * pageSize && short.length === 1, 'tracking bounds');
        data.setUint8(pageSize, 41);
        expanded.resize(3 * pageSize);
        assert(guest.size() === 3 && tracking.length === 3 * pageSize, 'resize reaches guest');
        assert(tracking[0] === 19 && tracking[pageSize] === 41 && data.getUint8(2 * pageSize) === 0, 'preserved bytes');
        assert(rejects(RangeError, () => expanded.resize(2 * pageSize)), 'shrink rejected');
        assert(rejects(RangeError, () => expanded.resize(3 * pageSize + 1)), 'alignment checked');
        assert(rejects(RangeError, () => expanded.resize(5 * pageSize)), 'declared max checked');
        assert(rejects(TypeError, () => expanded.transfer()) && guest.size() === 3, 'no transfer');
        assert(mem.grow(0) === 3 && mem.grow(1) === 3 && mem.buffer === expanded, 'host growth');
        assert(guest.grow(1) === -1 && expanded.byteLength === 4 * pageSize, 'rejected growth preserves view');
        let newFixed;
        assert(rejects(TypeError, () => expanded.resize({valueOf() {
            newFixed = mem.toFixedLengthBuffer();
            return 4 * pageSize;
        }})), 'resize rechecks attachment after coercion');
        assert(expanded.detached && expanded.maxByteLength === 0 && tracking.length === 0, 'old views detached');
        assert(!newFixed.resizable && mem.buffer === newFixed && mem.toFixedLengthBuffer() === newFixed, 'fixed identity');
        assert(new Uint8Array(newFixed)[0] === 19 && guest.grow(0) === 4 && newFixed.detached, 'fixed growth');

        const unlimited = new WebAssembly.Memory({initial:0});
        const original = unlimited.buffer;
        assert(rejects(TypeError, () => unlimited.toResizableBuffer()) && unlimited.buffer === original, 'maximum required');
        const wide = new WebAssembly.Memory({address:'i64', initial:0n, maximum:281474976710656n});
        const wideView = wide.toResizableBuffer();
        assert(wideView.maxByteLength === 2 ** 64, 'maximum is declared even above host index range');
        wideView.resize(pageSize);
        assert(wide.grow(1n) === 1n && wideView.byteLength === 2 * pageSize, 'memory64 growth paths');
        assert(rejects(RangeError, () => wideView.resize(129 * pageSize)) && wideView.byteLength === 2 * pageSize, 'owner budget');
        const onlyBuffer = new WebAssembly.Memory({initial:0, maximum:1}).toResizableBuffer();
        onlyBuffer.resize(pageSize);
        assert(onlyBuffer.byteLength === pageSize, 'attached view keeps owner alive');
        true
    "#);
    if let Err(rustic_js::EvalError::Throw(value)) = outcome {
        panic!("{}", agent.describe_value(value));
    }
    assert_eq!(outcome.unwrap(), Value::TRUE);
}

#[test]
fn wasm_window_executes_indirect_calls_with_local_table64_indices() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    let outcome = agent.evaluate(
        r#"
        const bytes = Uint8Array.of(
            0,97,115,109,1,0,0,0,
            1,10,2,96,0,1,127,96,1,126,1,127,
            3,3,2,0,1, 4,4,1,112,4,1,
            7,8,1,4,99,97,108,108,0,1,
            9,7,1,0,66,0,11,1,0,
            10,14,2,4,0,65,42,11,7,0,32,0,17,0,0,11
        );
        function rejects(type, call) {
            try { call(); } catch (error) { return error instanceof type; }
            return false;
        }
        const valid = WebAssembly.validate(bytes);
        const {call} = new WebAssembly.Instance(new WebAssembly.Module(bytes)).exports;
        valid && call(0n) === 42 &&
            rejects(WebAssembly.RuntimeError, () => call(4294967296n)) &&
            rejects(TypeError, () => call(0))
    "#,
    );
    if let Err(rustic_js::EvalError::Throw(value)) = outcome {
        panic!("{}", agent.describe_value(value));
    }
    assert_eq!(outcome.unwrap(), Value::TRUE);
}

#[test]
fn wasm_standalone_tables_trace_function_and_external_references() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    let outcome = agent.evaluate(r#"
        function assert(condition, message) { if (!condition) throw message; }
        function rejects(type, call) {
            try { call(); } catch (error) { return error instanceof type; }
            return false;
        }
        function makeFunction() {
            const bytes = Uint8Array.of(
                0,97,115,109,1,0,0,0,
                1,5,1,96,0,1,127, 3,2,1,0,
                7,7,1,3,114,117,110,0,0, 10,6,1,4,0,65,42,11
            );
            return new WebAssembly.Instance(new WebAssembly.Module(bytes)).exports.run;
        }
        const {Table} = WebAssembly;
        class Child extends Table {}
        const table = new Child({element: 'anyfunc', initial: 1, maximum: 4}, makeFunction());
        const external = new Table({element: 'externref', initial: 2}, {tag: 'retained'});
        assert(table instanceof Child && table.length === 1, 'subclass and length');
        const fn = table.get(0);
        assert(fn() === 42, 'table retains the Wasm function and its instance');
        assert(table.grow(2, fn) === 1 && table.get(2) === fn, 'growth and function identity');
        assert(table.set(0) === undefined && table.get(0) === null, 'default function entry');
        assert(rejects(TypeError, () => table.set(1, undefined)), 'explicit undefined is invalid');
        assert(rejects(TypeError, () => table.set(1, () => 42)), 'plain JS function is invalid');
        assert(rejects(TypeError, () => table.set(1, new Proxy(fn, {}))), 'function proxies are invalid');
        assert(rejects(TypeError, () => Table.prototype.get.call({}, 0)), 'receiver brand');
        assert(rejects(RangeError, () => table.grow(2)) && table.length === 3, 'failed growth');
        assert(table.get(1) === fn, 'failed mutations leave entries intact');
        assert(external.get(0).tag === 'retained' && external.get(0) === external.get(1), 'externref traces identity');
        const symbol = Symbol('entry');
        external.set(1, symbol);
        assert(external.get(1) === symbol, 'symbol round trip');
        external.set(0, null);
        assert(external.get(0) === null, 'null round trip');
        external.set(0);
        assert(external.get(0) === undefined, 'default externref entry');
        true
    "#);
    if let Err(rustic_js::EvalError::Throw(value)) = outcome {
        panic!("{}", agent.describe_value(value));
    }
    assert_eq!(outcome.unwrap(), Value::TRUE);
}

#[test]
fn wasm_standalone_table64_checks_full_indices_and_growth_after_reentry() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    let outcome = agent.evaluate(r#"
        function assert(condition, message) { if (!condition) throw message; }
        function rejects(type, call) {
            try { call(); } catch (error) { return error instanceof type; }
            return false;
        }
        const {Table} = WebAssembly;
        const table = new Table({element: 'externref', address: 'i64', initial: 1n, maximum: 3n}, 10);
        assert(table.length === 1n && table.get(0n) === 10, 'wide length and index');
        assert(rejects(TypeError, () => table.get(0)), 'wide counts reject Numbers');
        assert(rejects(TypeError, () => table.get(-1n)), 'negative index');
        assert(rejects(TypeError, () => table.get(2n ** 64n)), 'index outside address width');
        assert(rejects(RangeError, () => table.get(2n ** 32n)), 'index is never truncated');
        assert(table.grow({valueOf() {
            assert(table.grow(1n, 20) === 1n, 'reentrant growth');
            return 1n;
        }}, 30) === 2n, 'growth returns the current previous length');
        assert(table.length === 3n && table.get(1n) === 20 && table.get(2n) === 30, 'both growths are visible');
        assert(rejects(RangeError, () => table.grow(1n)) && table.length === 3n, 'maximum is enforced');
        const capped = new Table({element: 'anyfunc', address: 'i64', initial: 65535n, maximum: 2n ** 63n});
        assert(capped.grow(1n) === 65535n && capped.get(65535n) === null, 'exact host cap');
        assert(rejects(RangeError, () => capped.grow(1n)), 'declared maximum does not override host cap');
        assert(capped.length === 65536n, 'failed growth preserves length');
        true
    "#);
    if let Err(rustic_js::EvalError::Throw(value)) = outcome {
        panic!("{}", agent.describe_value(value));
    }
    assert_eq!(outcome.unwrap(), Value::TRUE);
}

#[test]
fn wasm_standalone_table_growth_respects_the_heap_budget() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::with_heap_limit(512 * 1024).unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    let outcome = agent.evaluate(
        r#"
        const table = new WebAssembly.Table({element: 'externref', initial: 1}, 42);
        let rejected = false;
        try { table.grow(65535, 'too large for this heap'); }
        catch (error) { rejected = error instanceof RangeError; }
        rejected && table.length === 1 && table.get(0) === 42 &&
            table.grow(1) === 1 && table.get(1) === undefined
    "#,
    );
    if let Err(rustic_js::EvalError::Throw(value)) = outcome {
        panic!("{}", agent.describe_value(value));
    }
    assert_eq!(outcome.unwrap(), Value::TRUE);
}

#[test]
fn wasm_exported_function_tables_share_guest_state_and_preserve_identity() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    let outcome = agent.evaluate(r#"
        function assert(condition, message) { if (!condition) throw message; }
        function rejects(type, call) {
            try { call(); } catch (error) { return error instanceof type; }
            return false;
        }
        function section(kind, bytes) { return [kind, bytes.length, ...bytes]; }
        function body(...instructions) { return [instructions.length + 2, 0, ...instructions, 11]; }
        function exportOf(name, kind, index) {
            return [name.length, ...Array.from(name, char => char.charCodeAt(0)), kind, index];
        }
        function provider() {
            return Uint8Array.of(
                0,97,115,109,1,0,0,0,
                1,5,1,96,0,1,127, 3,2,1,0,
                7,7,1,3,114,117,110,0,0, 10,6,1,4,0,65,17,11
            );
        }
        function module(wide) {
            const addr = wide ? 126 : 127;
            const constant = wide ? 66 : 65;
            return new WebAssembly.Module(Uint8Array.of(
                0,97,115,109,1,0,0,0,
                ...section(1, [5,96,0,1,127, 96,1,addr,1,127, 96,1,addr,0,
                    96,1,addr,1,addr, 96,1,111,0]),
                ...section(2, [1,1,109,1,102,0,0]), // import m.f at function index 0
                ...section(3, [6,0,0,1,2,3,4]),
                ...section(4, [1,112,wide ? 5 : 1,3,5]),
                ...section(7, [7, ...exportOf('table', 1, 0), ...exportOf('alias',1,0),
                    ...exportOf('answer',0,1), ...exportOf('call',0,3),
                    ...exportOf('clear',0,4), ...exportOf('grow',0,5), ...exportOf('linked',0,0)]),
                ...section(9, [1,0,constant,0,11,3,1,2,6]),
                ...section(10, [6, ...body(65,42), ...body(65,43),
                    ...body(32,0,17,0,0), ...body(32,0,208,112,38,0),
                    ...body(208,112,32,0,252,15,0), ...body()])
            ));
        }
        const foreign = new WebAssembly.Instance(new WebAssembly.Module(provider())).exports.run;
        const unrelated = new WebAssembly.Instance(new WebAssembly.Module(provider())).exports.run;
        function exercise(wide) {
            const {table, alias, answer, call, clear, grow, linked} =
                new WebAssembly.Instance(module(wide), {m:{f:foreign}}).exports;
            const at = i => wide ? BigInt(i) : i;
            assert(table === alias && table instanceof WebAssembly.Table, 'table aliases and brand');
            assert(table.length === at(3) && table.get(at(0)) === answer, 'initial elements and exported identity');
            const hidden = table.get(at(1));
            assert(hidden === table.get(at(1)) && hidden() === 43, 'lazy function wrapper');
            assert(table.get(at(2))(null) === undefined, 'call a reference signature through the table wrapper');
            clear(at(0));
            assert(table.get(at(0)) === null, 'guest clear visible to JS');
            assert(rejects(WebAssembly.RuntimeError, () => call(at(0))), 'guest observes null');
            table.set(at(0), hidden);
            assert(call(at(0)) === 43, 'write visible to indirect call');
            assert(linked === foreign, 'import re-export identity');
            table.set(at(1), foreign);
            assert(table.get(at(1)) === foreign && call(at(1)) === 17, 'existing import used by table');
            table.set(at(1), unrelated);
            assert(table.get(at(1)) === unrelated && call(at(1)) === 17, 'independently defined function identity and call');
            assert(rejects(TypeError, () => table.set(at(1), new WebAssembly.Memory({initial:0}))), 'memory is not a function');
            assert(table.get(at(1)) === unrelated, 'failed write preserves entry');
            assert(grow(at(1)) === at(3) && table.length === at(4), 'guest growth visible to JS');
            assert(table.grow(at(1), answer) === at(4) && call(at(4)) === 42, 'JS growth visible to guest');
            assert(rejects(RangeError, () => table.grow(at(1))) && table.length === at(5), 'growth limit');
            assert(rejects(RangeError, () => table.get(wide ? 4294967296n : 0xffffffff)), 'full-width bounds');
            return table;
        }
        const table = exercise(false);
        exercise(true);
        assert(table.get(0)() === 43 && table.get(4)() === 42, 'table alone retains instance and functions');
        true
    "#);
    if let Err(rustic_js::EvalError::Throw(value)) = outcome {
        panic!("{}", agent.describe_value(value));
    }
    assert_eq!(outcome.unwrap(), Value::TRUE);
}

#[test]
fn wasm_exported_external_tables_trace_guest_state_and_suspended_frames() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    install_wasm_collector(&mut agent);
    let outcome = agent.evaluate(r#"
        function assert(condition, message) { if (!condition) throw message; }
        function rangeError(call) {
            try { call(); } catch (error) { return error instanceof RangeError; }
            return false;
        }
        function section(kind, bytes) { return [kind, bytes.length, ...bytes]; }
        function body(locals, instructions) {
            return [locals.length + instructions.length + 1, ...locals, ...instructions, 11];
        }
        function exportOf(name, kind, index) {
            return [name.length, ...Array.from(name, char => char.charCodeAt(0)), kind, index];
        }
        function module(wide) {
            const c = wide ? 66 : 65;
            const addr = wide ? 126 : 127;
            const nullRef = [208,111];
            const get = i => [c,i,37,0];
            const clear = i => [c,i,...nullRef,38,0];
            // During the imported call, one object is held only in the caller's
            // local, another only on its operand stack.
            const move = [ ...get(0), 33,0, ...clear(0),
                c,0, ...get(1), ...clear(1), 16,5, 38,0, 32,0, 36,0 ];
            const restore = [c,1,35,0,38,0,...nullRef,36,0];
            return new WebAssembly.Module(Uint8Array.of(
                0,97,115,109,1,0,0,0,
                ...section(1, [3,96,0,0, 96,1,addr,1,127, 96,1,addr,1,addr]),
                ...section(2, [1,1,109,1,102,0,0]),
                ...section(3, [5,0,0,1,2,0]),
                ...section(4, [1,111,wide ? 5 : 1,3,6]),
                ...section(6, [1,111,1,...nullRef,11]),
                ...section(7, [6, ...exportOf('table',1,0), ...exportOf('alias',1,0),
                    ...exportOf('move',0,1), ...exportOf('restore',0,2),
                    ...exportOf('isNull',0,3), ...exportOf('grow',0,4)]),
                ...section(10, [5, ...body([1,1,111], move), ...body([0], restore),
                    ...body([0], [32,0,37,0,209]),
                    ...body([0], [...nullRef,32,0,252,15,0]),
                    ...body([0], [16,0])])
            ));
        }
        function exercise(wide) {
            const at = i => wide ? BigInt(i) : i;
            let live;
            let calls = 0;
            live = new WebAssembly.Instance(module(wide), {m:{f() {
                calls++;
                assert(live.isNull(at(0)) && live.isNull(at(1)), 'reentrant guest sees cleared entries');
                collect();
            }}}).exports;
            const {table, alias, move, restore, grow, isNull} = live;
            assert(table === alias && table instanceof WebAssembly.Table, 'external table aliases and brand');
            assert(table.length === at(3) && isNull(at(0)) && table.get(at(0)) === null, 'guest initializes with null');
            table.set(at(0), {tag:'local'});
            table.set(at(1), {tag:'operand'});
            move();
            assert(calls === 1 && table.get(at(0)).tag === 'operand', 'suspended operand stays live');
            assert(table.get(at(1)) === null, 'local is now held only by the Wasm global');
            restore();
            assert(table.get(at(1)).tag === 'local', 'external global and suspended local stay live');

            const marker = Symbol('external');
            table.set(at(0), marker);
            assert(table.get(at(0)) === marker && !isNull(at(0)), 'symbol identity');
            assert(table.grow(at(1), 123456789012345678901n) === at(3), 'host grow');
            assert(table.get(at(3)) === 123456789012345678901n, 'full BigInt value');
            assert(grow(at(1)) === at(4) && table.get(at(4)) === null, 'guest grow');
            assert(table.grow(at(1)) === at(5) && table.get(at(5)) === undefined, 'host grow default');
            table.set(at(2));
            assert(table.get(at(2)) === undefined && !isNull(at(2)), 'undefined is not null');
            assert(rangeError(() => table.grow(at(1), marker)) && table.length === at(6), 'failed grow preserves length');
            const out = wide ? 4294967296n : 0xffffffff;
            assert(rangeError(() => table.set(out, marker)) && rangeError(() => table.get(out)), 'full-width bounds');
            return table;
        }
        const table = exercise(false);
        exercise(true);
        assert(table.get(1).tag === 'local', 'table wrapper retains its instance and references');
        function forgetEntry() {
            const weak = new WeakRef(table.get(1));
            table.set(1, null);
            return weak;
        }
        const weak = forgetEntry();
        collect();
        assert(weak.deref() === undefined, 'overwritten references are reclaimable');
        true
    "#);
    if let Err(rustic_js::EvalError::Throw(value)) = outcome {
        panic!("{}", agent.describe_value(value));
    }
    assert_eq!(outcome.unwrap(), Value::TRUE);
}

fn install_wasm_collector(agent: &mut Agent) {
    let collect = agent
        .new_native_function(
            |agent, _, _| {
                let host = agent.execution_host().expect("page collector");
                host.collect(agent)?;
                Ok(Value::UNDEFINED)
            },
            "collect",
            0,
        )
        .unwrap();
    let key = agent.heap.key("collect").unwrap();
    agent
        .set(agent.intrinsics.global, key, Value::object(collect), true)
        .unwrap();
}

#[test]
fn wasm_external_table_imports_share_identity_growth_and_start_roots() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    install_wasm_collector(&mut agent);
    let outcome = agent.evaluate(r#"
        function assert(condition, message) { if (!condition) throw message; }
        function rejects(type, call) {
            try { call(); } catch (error) { return error instanceof type; }
            return false;
        }
        const header = [0,97,115,109,1,0,0,0];
        function section(kind, bytes) { return [kind, bytes.length, ...bytes]; }
        function body(locals, instructions) {
            return [locals.length + instructions.length + 1, ...locals, ...instructions, 11];
        }
        function exportOf(name, kind, index) {
            return [name.length, ...Array.from(name, char => char.charCodeAt(0)), kind, index];
        }
        function module(wide, trapping, exposeTables = true) {
            const c = i => [wide ? 66 : 65,i];
            const addr = wide ? 126 : 127;
            const limits = [111,wide ? 5 : 1,4,6];
            const nullRef = [208,111];
            const get = i => [...c(i),37,0];
            const clear = i => [...c(i),...nullRef,38,0];
            const segment = i => [6,1,...c(i),11,111,1,...nullRef,11];
            const start = [...get(0),36,0, ...get(1),33,0, ...clear(0),...clear(1),
                16,0, ...c(1),32,0,38,1];
            const restore = [...c(0),35,0,38,0,...nullRef,36,0];
            return new WebAssembly.Module(Uint8Array.of(
                ...header,
                ...section(1, [2,96,0,0, 96,1,addr,1,addr]),
                ...section(2, [3,1,109,1,97,1,...limits, 1,109,1,98,1,...limits, 1,109,1,102,0,0]),
                ...section(3, [4,0,0,0,1]),
                ...section(4, [1,111,wide ? 5 : 1,1,2]),
                ...section(6, [1,111,1,...nullRef,11]),
                ...section(7, [exposeTables ? 6 : 3,
                    ...(exposeTables ? [...exportOf('first',1,0), ...exportOf('second',1,1),
                        ...exportOf('local',1,2)] : []),
                    ...exportOf('restore',0,2), ...exportOf('overlap',0,3), ...exportOf('grow',0,4)]),
                ...section(8, [1]),
                ...section(9, trapping ? [2,...segment(3),...segment(6)] : [1,...segment(3)]),
                ...section(10, [4, ...body([1,1,111], start), ...body([0], restore),
                    ...body([0], [...c(1),...c(0),...c(2),252,14,1,0]),
                    ...body([0], [...nullRef,32,0,252,15,0])])
            ));
        }
        function provider(wide) {
            return new WebAssembly.Instance(new WebAssembly.Module(Uint8Array.of(
                ...header, ...section(4,[1,111,wide ? 5 : 1,4,6]),
                ...section(7,[1,...exportOf('table',1,0)])
            ))).exports.table;
        }
        function exercise(wide, supplied) {
            const at = i => wide ? BigInt(i) : i;
            const table = supplied ?? new WebAssembly.Table({
                element:'externref', address:wide ? 'i64' : 'i32', initial:at(3), maximum:at(6)
            });
            if (!supplied) table.grow(at(1));
            table.set(at(0), {tag:'global'});
            table.set(at(1), {tag:'local'});
            table.set(at(3), {tag:'active'});
            const m = module(wide,false);
            let called = false;
            const exports = new WebAssembly.Instance(m, {m:{a:table,b:table,f() {
                called = true;
                assert(table.get(at(0)) === null && table.get(at(1)) === null, 'start writes visible to import');
                assert(table.get(at(3)) === null, 'active writes happen before start');
                collect();
            }}}).exports;
            assert(called && exports.first === table && exports.second === table, 'both aliases preserve identity');
            assert(exports.local !== table && exports.local.get(at(0)) === null, 'local table follows imported indices');
            collect();
            exports.restore();
            assert(table.get(at(0)).tag === 'global' && table.get(at(1)).tag === 'local', 'start roots survived');
            const first = table.get(at(0));
            const second = table.get(at(1));
            exports.overlap();
            assert(table.get(at(1)) === first && table.get(at(2)) === second, 'overlap resolves backing identity');
            assert(exports.grow(at(1)) === at(4), 'guest growth uses allocating table');
            assert(table.grow(at(1)) === at(5) && table.get(at(5)) === undefined, 'host grow uses same length');
            assert(exports.grow(at(1)) === at(-1) && table.length === at(6), 'shared maximum enforced');
            table.set(at(3), second);
            assert(rejects(WebAssembly.RuntimeError, () => new WebAssembly.Instance(module(wide,true), {
                m:{a:table,b:table,f() { throw 'unreachable start'; }}
            })), 'trapping active segment prevents start');
            assert(table.get(at(3)) === null && table.get(at(2)) === second, 'completed initializer prefix remains');
            const failure = {};
            try {
                new WebAssembly.Instance(m, {m:{a:table,b:table,f() { throw failure; }}});
                throw 'start was expected to throw';
            } catch (error) { assert(error === failure, 'start propagates callback exception'); }
            assert(table.get(at(0)) === null && table.get(at(1)) === null, 'start writes survive traps');
        }
        exercise(false);
        exercise(true);
        exercise(false, provider(false));
        exercise(true, provider(true));

        let weakTable;
        function exportedGrowth() {
            const table = new WebAssembly.Table({element:'externref',initial:4,maximum:6});
            weakTable = new WeakRef(table);
            return new WebAssembly.Instance(module(false,false,false), {
                m:{a:table,b:table,f(){}}
            }).exports.grow;
        }
        const grow = exportedGrowth();
        collect();
        assert(grow(1) === 4 && weakTable.deref().length === 5, 'instance retains unexported imports');

        const m = module(false,false);
        const value = new WebAssembly.Table({element:'externref',initial:4,maximum:6});
        const wrongCases = [
            new WebAssembly.Table({element:'anyfunc',initial:4,maximum:6}),
            new WebAssembly.Table({element:'externref',initial:3,maximum:6}),
            new WebAssembly.Table({element:'externref',initial:4,maximum:7}),
            new WebAssembly.Table({element:'externref',initial:4}),
            new WebAssembly.Table({element:'externref',address:'i64',initial:4n,maximum:6n})
        ];
        for (const wrong of wrongCases) {
            assert(rejects(WebAssembly.LinkError, () => new WebAssembly.Instance(m, {
                m:{a:wrong,b:value,f() { throw 'unreachable start'; }}
            })), 'check kind, current size, declared maximum and width');
        }
        // Index conversion is allowed to link and promote the receiver.
        value.set({valueOf() {
            new WebAssembly.Instance(m, {m:{a:value,b:value,f(){}}});
            return 0;
        }}, 42);
        assert(value.get(0) === 42, 'set uses the promoted backing after coercion');
        true
    "#);
    if let Err(rustic_js::EvalError::Throw(value)) = outcome {
        panic!("{}", agent.describe_value(value));
    }
    assert_eq!(outcome.unwrap(), Value::TRUE);
}

#[test]
fn wasm_function_table_imports_share_addresses_and_keep_trapping_owners_alive() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    install_wasm_collector(&mut agent);
    let outcome = agent.evaluate(r#"
        const header = [0,97,115,109,1,0,0,0];
        function assert(condition, message) { if (!condition) throw message; }
        function rejects(type, call) {
            try { call(); } catch (error) { return error instanceof type; }
            return false;
        }
        function section(id, bytes) { return [id, bytes.length, ...bytes]; }
        function body(locals, instructions) {
            return [locals.length + instructions.length + 1, ...locals, ...instructions, 11];
        }
        function named(name, kind, index) {
            return [name.length, ...Array.from(name, c => c.charCodeAt(0)), kind, index];
        }
        function compile(sections) {
            return new WebAssembly.Module(Uint8Array.of(...header, ...sections.flat()));
        }
        function source(parameter) {
            const type = parameter ? [1,96,1,127,1,127] : [1,96,0,1,127];
            return new WebAssembly.Instance(compile([
                section(1,type), section(3,[1,0]),
                section(7,[1,...named('run',0,0)]),
                section(10,[1,...body([0], [65,23])])
            ])).exports.run;
        }
        function callbackSource(f) {
            return new WebAssembly.Instance(compile([
                section(1,[2,96,0,0,96,0,1,127]),
                section(2,[1,1,109,1,102,0,0]), section(3,[1,1]),
                section(7,[1,...named('run',0,1)]),
                section(10,[1,...body([0], [16,0,65,31])])
            ]), {m:{f}}).exports.run;
        }
        function provider(wide) {
            return new WebAssembly.Instance(compile([
                section(4,[1,112,wide ? 5 : 1,3,5]),
                section(7,[1,...named('table',1,0)])
            ])).exports.table;
        }
        function consumer(wide, trap) {
            const address = wide ? 126 : 127;
            const c = n => [wide ? 66 : 65, n];
            const limits = [112,wide ? 5 : 1,3,5];
            const segment = (table, offset) => [2,table,...c(offset),11,0,1,1];
            const elements = trap ? [2,...segment(1,1),...segment(0,8)] : [1,...segment(1,1)];
            const hold = [...c(0),37,0,33,0, ...c(0),208,112,38,0,
                16,0, ...c(0),32,0,38,1];
            return compile([
                section(1,[4,96,0,0, 96,0,1,127, 96,1,address,1,127, 96,1,address,1,address]),
                section(2,[3,1,109,1,97,1,...limits, 1,109,1,98,1,...limits, 1,109,1,102,0,0]),
                section(3,[6,1,2,2,0,3,0]),
                section(7,[7,...named('a',1,0),...named('b',1,1),
                    ...named('call',0,2),...named('tail',0,3),...named('hold',0,4),
                    ...named('grow',0,5),...named('copy',0,6)]),
                section(9,elements),
                section(10,[6,
                    ...body([0], [65,42]),
                    ...body([0], [32,0,17,1,0]),
                    ...body([0], [32,0,19,1,1]),
                    ...body([1,1,112], hold),
                    ...body([0], [208,112,32,0,252,15,1]),
                    ...body([0], [...c(1),...c(0),...c(2),252,14,1,0])
                ])
            ]);
        }
        function exercise(wide, supplied) {
            const at = n => wide ? BigInt(n) : n;
            const table = supplied ?? new WebAssembly.Table({
                element:'anyfunc', address:wide ? 'i64' : 'i32', initial:at(3), maximum:at(5)
            });
            function seed() {
                const foreign = source(false);
                table.set(at(0), foreign);
                return new WeakRef(foreign);
            }
            const weak = seed();
            table.set(at(2), source(true));
            const imports = {m:{a:table,b:table,f:collect}};
            const exports = new WebAssembly.Instance(consumer(wide,false), imports).exports;
            assert(exports.a === table && exports.b === table, 're-exported table aliases');
            assert(table.get(at(0)) === weak.deref() && exports.call(at(0)) === 23, 'prelinked function identity');
            const hidden = table.get(at(1));
            assert(hidden === table.get(at(1)) && exports.tail(at(1)) === 42, 'active function and direct tail call');
            assert(rejects(WebAssembly.RuntimeError, () => exports.call(at(2))), 'cross-instance signature mismatch');
            exports.hold();
            assert(table.get(at(0)) === weak.deref() && exports.tail(at(0)) === 23, 'suspended foreign function and cross-instance tail call');
            exports.copy();
            assert(table.get(at(1)) === weak.deref() && table.get(at(2)) === hidden, 'overlapping copy preserves both addresses');
            assert(exports.grow(at(1)) === at(3) && table.get(at(3)) === null, 'guest growth in original table');
            assert(table.grow(at(1), hidden) === at(4) && exports.call(at(4)) === 42, 'host growth preserves guest function');
            assert(rejects(RangeError, () => table.grow(at(1))) && table.length === at(5), 'allocating maximum');
            assert(rejects(WebAssembly.RuntimeError,
                () => new WebAssembly.Instance(consumer(wide,true), imports)), 'trapping segment fails instantiation');
            collect();
            const retained = table.get(at(1));
            assert(retained !== hidden && retained() === 42 && exports.call(at(1)) === 42,
                'published initializer retains the trapping instance');
            let target;
            function seedCallback() {
                const run = callbackSource(() => {
                    table.set(at(3), null);
                    collect();
                    assert(target.deref() !== undefined, 'indirect call retains selected owner');
                });
                table.set(at(3), run);
                target = new WeakRef(run);
            }
            seedCallback();
            assert(exports.call(at(3)) === 31 && table.get(at(3)) === null, 'callback cleared selected entry');
            table.set(at(0), null);
            assert(rejects(WebAssembly.RuntimeError, () => exports.call(at(0))), 'null traps');
            collect();
            assert(weak.deref() === undefined && target.deref() === undefined, 'overwritten functions are reclaimable');
        }
        exercise(false);
        exercise(true);
        exercise(false, provider(false));
        exercise(true, provider(true));

        const m = consumer(false,false);
        const table = new WebAssembly.Table({element:'anyfunc',initial:3,maximum:5});
        const wrong = [
            new WebAssembly.Table({element:'externref',initial:3,maximum:5}),
            new WebAssembly.Table({element:'anyfunc',initial:2,maximum:5}),
            new WebAssembly.Table({element:'anyfunc',initial:3,maximum:6}),
            new WebAssembly.Table({element:'anyfunc',address:'i64',initial:3n,maximum:5n})
        ];
        for (const incompatible of wrong) {
            assert(rejects(WebAssembly.LinkError, () => new WebAssembly.Instance(m, {
                m:{a:table,b:incompatible,f:collect}
            })), 'check type, current minimum, maximum and width');
            assert(table.get(1) === null, 'link mismatch applies no active writes');
        }
        true
    "#);
    if let Err(rustic_js::EvalError::Throw(value)) = outcome {
        panic!("{}", agent.describe_value(value));
    }
    assert_eq!(outcome.unwrap(), Value::TRUE);
}

#[test]
fn wasm_reference_calls_preserve_values_and_roots_across_instances() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    install_wasm_collector(&mut agent);
    let outcome = agent.evaluate(r#"
        function assert(ok, message) { if (!ok) throw message; }
        function rejects(type, call) {
            try { call(); } catch (error) { return error instanceof type; }
            return false;
        }
        function section(kind, bytes) { return [kind, bytes.length, ...bytes]; }
        function module(...sections) {
            return new WebAssembly.Module(Uint8Array.of(0,97,115,109,1,0,0,0,...sections.flat()));
        }
        function body(locals, opcodes) {
            return [locals.length + opcodes.length + 1, ...locals, ...opcodes, 11];
        }
        function named(name, kind, index) {
            return [name.length, ...Array.from(name, c => c.charCodeAt(0)), kind, index];
        }
        // (externref, funcref, i32) -> (externref, i32, funcref).
        const signature = [96,3,111,112,127,3,111,127,112];
        const provider = module(
            section(1,[3,...signature,96,0,0,96,0,1,127]),
            section(2,[2,1,109,1,102,0,0,1,109,1,103,0,1]),
            section(3,[2,0,2]),
            section(7,[2,...named('run',0,2),...named('seven',0,3)]),
            section(10,[2,
                ...body([0], [32,0,32,1,32,2,16,0,16,1]),
                ...body([0], [65,7])
            ])
        );
        const id = Symbol('identity');
        let pending;
        const imports = {m:{f(input, fun, count) {
            if (count === 0) return [input, 0, fun];
            const item = {input, id};
            pending = new WeakRef(item);
            return [item, {valueOf() { collect(); return count + 1; }}, fun];
        }, g() {
            collect();
            if (pending) assert(pending.deref() !== undefined, 'callback result is live in Wasm');
        }}};
        const first = new WebAssembly.Instance(provider, imports).exports;
        const original = {key:42};
        const direct = first.run(original, first.seven, 1);
        assert(direct[0].input === original && direct[0].id === id &&
            direct[1] === 2 && direct[2] === first.seven, 'mixed export and callback results');
        const omitted = first.run(undefined, null, 0);
        assert(omitted.length === 3 && 0 in omitted && omitted[0] === undefined && omitted[2] === null,
            'undefined remains an occupied result slot');
        pending = undefined;
        assert(first.run(null, null, 0)[0] === null, 'external null round trip');
        assert(rejects(TypeError, () => first.run(null, () => 7, 0)), 'plain JS function is not a funcref');
        assert(rejects(TypeError, () => first.run(null, undefined, 0)), 'explicit undefined is not a funcref');

        const imported = module(
            section(1,[1,...signature]), section(2,[1,1,109,1,102,0,0]),
            section(3,[1,0]), section(7,[2,...named('call',0,1),...named('alias',0,0)]),
            section(10,[1,...body([0], [32,0,32,1,32,2,16,0])])
        );
        const linked = new WebAssembly.Instance(imported, {m:{f:first.run}}).exports;
        assert(linked.alias === first.run, 'import re-export preserves wrapper identity');
        const fromImport = linked.call(id, first.seven, 2);
        assert(fromImport[0].input === id && fromImport[1] === 3 && fromImport[2]() === 7,
            'direct cross-instance reference arguments and results');

        const indirect = module(
            section(1,[1,...signature]),
            section(2,[1,1,109,1,116,1,112,1,1,2]),
            section(3,[2,0,0]),
            section(7,[2,...named('call',0,0),...named('tail',0,1)]),
            section(10,[2,
                ...body([0],[32,0,32,1,32,2,65,0,17,0,0]),
                ...body([0],[32,0,32,1,32,2,65,0,19,0,0])
            ])
        );
        const table = new WebAssembly.Table({element:'anyfunc',initial:1,maximum:2}, first.run);
        const calls = new WebAssembly.Instance(indirect, {m:{t:table}}).exports;
        for (const call of [calls.call,calls.tail]) {
            const result = call(original, first.seven, {valueOf() { collect(); return 4; }});
            assert(result[0].input === original && result[1] === 5 && result[2] === first.seven,
                'indirect reference arguments and results across instances');
        }
        table.set(0, first.seven);
        assert(rejects(WebAssembly.RuntimeError, () => calls.call(null,null,0)), 'type mismatch before invocation');
        true
    "#);
    if let Err(rustic_js::EvalError::Throw(value)) = outcome {
        panic!("{}", agent.describe_value(value));
    }
    assert_eq!(outcome.unwrap(), Value::TRUE);
}

#[test]
fn wasm_reference_globals_share_cells_and_keep_current_values_alive() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    install_wasm_collector(&mut agent);
    let outcome = agent.evaluate(r#"
        function assert(ok, message) { if (!ok) throw message; }
        function rejects(type, call) {
            try { call(); } catch (error) { return error instanceof type; }
            return false;
        }
        function section(kind, bytes) { return [kind, bytes.length, ...bytes]; }
        function named(name, kind, index) {
            return [name.length, ...Array.from(name, c => c.charCodeAt(0)), kind, index];
        }
        function module(...sections) {
            return new WebAssembly.Module(Uint8Array.of(0,97,115,109,1,0,0,0,...sections.flat()));
        }
        const extern = new WebAssembly.Global({value:'externref',mutable:true});
        const fun = new WebAssembly.Global({value:'anyfunc',mutable:true});
        assert(extern.value === undefined && fun.valueOf() === null, 'reference defaults');
        assert(rejects(TypeError, () => new WebAssembly.Global({value:'anyfunc'}, undefined)), 'explicit undefined funcref');
        assert(rejects(TypeError, () => { fun.value = () => 1; }), 'plain JS function rejected');
        const read = module(
            section(1,[3,96,1,111,0,96,0,1,111,96,0,1,127]),
            section(2,[4,1,109,1,101,3,111,1,1,109,1,110,3,127,0,
                1,109,1,102,3,112,1,1,109,1,115,3,111,0]),
            section(3,[3,0,1,2]),
            section(6,[2,111,1,208,111,11,111,0,35,3,11]),
            section(7,[9,...named('ext',3,0),...named('number',3,1),...named('fn',3,2),
                ...named('local',3,4),...named('alias',3,4),...named('snapshot',3,5),
                ...named('write',0,0),...named('get',0,1),...named('seven',0,2)]),
            section(10,[3,6,0,32,0,36,0,11,4,0,35,0,11,4,0,65,7,11])
        );
        const marker = Symbol('immutable externref');
        const imports = {m:{e:extern,n:42,f:fun,s:marker}};
        const a = new WebAssembly.Instance(read, imports).exports;
        const b = new WebAssembly.Instance(read, imports).exports;
        assert(a.ext === b.ext && a.ext === extern && b.fn === fun, 'import identities');
        assert(a.number.value === 42 && a.snapshot.value === marker, 'mixed global indices and const initializer');
        assert(a.local === a.alias && a.local.value === null, 'local alias and Wasm null initialization');
        assert(rejects(TypeError, () => { a.snapshot.value = {}; }), 'immutable global');
        function writeGuest() {
            const value = {tag:'live'};
            a.write(value);
            return new WeakRef(value);
        }
        const weak = writeGuest();
        collect();
        assert(b.get() === weak.deref() && b.get().tag === 'live', 'shared guest write keeps value alive');
        extern.value = undefined;
        collect();
        assert(a.get() === undefined && weak.deref() === undefined, 'external overwrite releases old value');
        fun.value = a.seven;
        collect();
        assert(b.fn.value === a.seven && fun.valueOf()() === 7, 'function identity and owner retention');
        assert(rejects(WebAssembly.LinkError, () => new WebAssembly.Instance(read, {m:{
            e:{}, n:42, f:fun, s:marker
        }})), 'mutable reference import needs a Global');
        assert(rejects(WebAssembly.LinkError, () => new WebAssembly.Instance(read, {m:{
            e:fun, n:42, f:fun, s:marker
        }})), 'mutable global type mismatch');
        true
    "#);
    if let Err(rustic_js::EvalError::Throw(value)) = outcome {
        panic!("{}", agent.describe_value(value));
    }
    assert_eq!(outcome.unwrap(), Value::TRUE);
}

#[test]
fn font_faces_settle_from_binary_data_and_the_document_set_tracks_them() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    agent.evaluate(r#"
        function assert(condition, message) { if (!condition) throw message; }
        var log = [];
        const font = new FontFace('Web Sans', Uint8Array.of(0, 1, 0, 0, 9), {weight: '300 700', style: 'italic'});
        assert(Object.prototype.toString.call(font) === '[object FontFace]', 'tag');
        assert(font.family === 'Web Sans' && font.weight === '300 700' && font.style === 'italic', 'descriptors');
        assert(font.stretch === 'normal' && font.unicodeRange === 'U+0-10FFFF' && font.display === 'auto', 'defaults');
        assert(font.status === 'loaded' && font.load() === font.loaded, 'binary data loads at once');
        font.loaded.then(f => log.push('loaded ' + (f === font)));
        const junk = new FontFace('Junk', new ArrayBuffer(8));
        assert(junk.status === 'error', 'not a font');
        junk.loaded.catch(e => log.push(e.name));
        const remote = new FontFace('Remote', 'url(remote.woff2)');
        assert(remote.status === 'unloaded', 'a src string waits for load()');
        remote.load().catch(e => log.push(e.name + ' ' + remote.status));
        const fonts = document.fonts;
        assert(fonts === document.fonts && fonts instanceof FontFaceSet, 'one set per document');
        assert(fonts.status === 'loaded' && fonts.size === 0 && fonts.check('12px Web Sans'), 'empty set');
        assert(fonts.add(font) === fonts && fonts.add(font).size === 1 && fonts.has(font), 'add once');
        fonts.add(junk);
        const seen = [];
        fonts.forEach((value, key, set) => seen.push(value === key && set === fonts));
        assert(seen.join() === 'true,true', 'forEach');
        assert(fonts.delete(junk) && !fonts.delete(junk) && fonts.size === 1, 'delete');
        fonts.load('bold 16px "Web Sans", serif').then(list => log.push('matched ' + list.length));
        fonts.ready.then(set => log.push('ready ' + (set === fonts)));
        fonts.clear();
        assert(fonts.size === 0, 'clear');
        let threw = false;
        try { fonts.add({}); } catch (e) { threw = e instanceof TypeError; }
        assert(threw, 'only FontFace objects join the set');
    "#).unwrap();
    agent.run_jobs().unwrap();
    let log = agent.evaluate("log.join('; ')").unwrap();
    let log = agent.to_string(log).unwrap();
    let log = String::from_utf16_lossy(&agent.heap.string(log).units().collect::<Vec<_>>());
    assert_eq!(
        log,
        "loaded true; SyntaxError; NetworkError error; matched 1; ready true"
    );
}

#[test]
fn range_content_operations_follow_the_dom_algorithms() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    expect_ok(
        &mut agent,
        r#"
        function assert(condition, message) { if (!condition) throw message; }
        const html = document.appendChild(document.createElement('html'));
        const p = html.appendChild(document.createElement('p'));
        p.append('Hello ', document.createElement('b'), ' world');
        p.childNodes[1].append('big');
        const serialize = n => n.nodeType === 3 ? n.data
            : '<' + n.localName + '>' + Array.from(n.childNodes, serialize).join('') + '</' + n.localName + '>';
        const r = document.createRange();
        r.setStart(p.firstChild, 3);
        r.setEnd(p.childNodes[1].firstChild, 2);
        assert(r.toString() === 'lo bi', 'stringifier: ' + r.toString());
        assert(r.commonAncestorContainer === p, 'common ancestor');
        const copy = r.cloneContents();
        assert(Array.from(copy.childNodes, serialize).join('') === 'lo <b>bi</b>', 'clone');
        assert(serialize(p) === '<p>Hello <b>big</b> world</p>', 'clone leaves the tree');
        const extracted = r.extractContents();
        assert(Array.from(extracted.childNodes, serialize).join('') === 'lo <b>bi</b>', 'extract');
        assert(serialize(p) === '<p>Hel<b>g</b> world</p>', 'extract cuts: ' + serialize(p));
        assert(r.collapsed && r.startContainer === p && r.startOffset === 1, 'extract collapses');
        const i = document.createElement('i');
        r.insertNode(i);
        assert(serialize(p) === '<p>Hel<i></i><b>g</b> world</p>', 'insertNode');
        assert(r.startOffset === 1 && r.endOffset === 2, 'collapsed range grows over the node');
        r.setStart(p.lastChild, 1);
        r.setEnd(p.lastChild, 6);
        const u = document.createElement('u');
        r.surroundContents(u);
        assert(serialize(p) === '<p>Hel<i></i><b>g</b> <u>world</u></p>', 'surround: ' + serialize(p));
        assert(r.startContainer === p && r.startOffset === 4 && r.endOffset === 5, 'selects newParent');
        r.selectNodeContents(p);
        r.setStart(p.firstChild, 1);
        r.deleteContents();
        assert(serialize(p) === '<p>H</p>', 'delete: ' + serialize(p));
        assert(r.collapsed && r.startContainer === p && r.startOffset === 1, 'delete collapses after the start node');
        r.selectNode(p);
        assert(r.compareBoundaryPoints(Range.START_TO_END, r) === 1, 'compareBoundaryPoints');
        assert(r.comparePoint(p, 0) === 0 && r.isPointInRange(html, 0), 'points');
        assert(r.intersectsNode(p.firstChild) && !r.intersectsNode(document.createElement('x')), 'intersects');
        // Live updates and the static variant.
        const live = document.createRange();
        live.setStart(p.firstChild, 1);
        p.firstChild.data = 'Hi there';
        assert(live.startOffset === 0, 'replace data clamps the offset');
        const fixed = new StaticRange({startContainer: p, startOffset: 0, endContainer: p, endOffset: 1});
        p.removeChild(p.firstChild);
        assert(fixed.endOffset === 1 && !fixed.collapsed && fixed instanceof AbstractRange, 'static ranges are not live');
        let threw = false;
        try { r.surroundContents(document); } catch (e) { threw = e.name === 'InvalidNodeTypeError'; }
        assert(threw, 'a document cannot surround');
    "#,
    );
}

#[test]
fn inline_style_reads_and_writes_the_style_attribute() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    expect_ok(
        &mut agent,
        r#"
        function assert(condition, message) { if (!condition) throw message; }
        const div = document.appendChild(document.createElement('div'));
        div.setAttribute('style', 'color: red; bogus: 1; margin-top: 2px !important');
        const style = div.style;
        assert(style === div.style && style instanceof CSSStyleDeclaration, 'one declaration per element');
        assert(style.length === 2 && style[0] === 'color' && style.item(1) === 'margin-top', 'indexed names');
        assert(style.color === 'red' && style.getPropertyPriority('margin-top') === 'important', 'values');
        style.display = 'none';
        assert(div.getAttribute('style') === 'color: red; margin-top: 2px !important; display: none;', div.getAttribute('style'));
        style.width = 'nonsense';
        assert(style.width === '', 'invalid values are ignored');
        style.setProperty('Background-Color', 'blue', 'important');
        assert(style.backgroundColor === 'blue' && style.getPropertyPriority('background-color') === 'important', 'setProperty');
        assert(style.removeProperty('color') === 'red' && style.color === '', 'removeProperty');
        style.cssText = 'padding-left: 3px; nope: 4';
        assert(style.cssText === 'padding-left: 3px;' && style.length === 1, 'cssText');
        div.style = 'opacity: 0.5';
        assert(div.getAttribute('style') === 'opacity: 0.5;', 'PutForwards=cssText');
        style.setProperty('opacity', '');
        assert(div.getAttribute('style') === '', 'an empty value removes');
        let threw = false;
        try { getComputedStyle(div).color = 'red'; } catch (e) { threw = e.name === 'NoModificationAllowedError'; }
        assert(threw, 'computed declarations stay read-only');
    "#,
    );
}

#[test]
fn parent_and_child_node_mixins_convert_strings_and_skip_their_arguments() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    expect_ok(
        &mut agent,
        r#"
        function assert(condition, message) { if (!condition) throw message; }
        const list = document.appendChild(document.createElement('ul'));
        const names = () => Array.from(list.childNodes, n => n.nodeType === 3 ? n.data : n.localName).join(',');
        const [a, b] = [document.createElement('a'), document.createElement('b')];
        list.append(a, 'text', b);
        assert(names() === 'a,text,b', names());
        list.prepend('first');
        b.before(b, 'x');
        a.after('y');
        // `b` moves into the converted fragment before the insertion.
        assert(names() === 'first,a,y,text,b,x', names());
        a.replaceWith('z', a);
        b.remove();
        assert(names() === 'first,z,a,y,text,x', names());
        list.replaceChildren(b);
        assert(names() === 'b', names());
        const other = document.createElement('ul');
        other.append(document.createElement('b'));
        assert(list.isEqualNode(other) && !list.isEqualNode(a) && !list.isEqualNode(null), 'isEqualNode');
        other.firstChild.setAttribute('id', 'x');
        assert(!list.isEqualNode(other), 'attributes count');
        let threw = false;
        try { a.append(a); } catch (e) { threw = e.name === 'HierarchyRequestError'; }
        assert(threw, 'hierarchy checks still apply');
    "#,
    );
}

/// Run `source` and fail with the message of anything it throws.
fn expect_ok(agent: &mut Agent, source: &str) {
    let wrapped = format!(
        "(() => {{ try {{ {source}; return 'ok'; }} catch (e) {{ return String(e); }} }})()"
    );
    let result = agent.evaluate(&wrapped).unwrap();
    let text = agent.to_string(result).unwrap();
    let text = String::from_utf16_lossy(&agent.heap.string(text).units().collect::<Vec<_>>());
    assert_eq!(text, "ok");
}

#[test]
fn token_lists_and_embedded_elements_reflect_their_attributes() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    expect_ok(
        &mut agent,
        r#"
        function assert(condition, message) { if (!condition) throw message; }
        function throws(name, f) { try { f(); } catch (e) { return e.name === name; } return false; }
        const div = document.createElement('div');
        const list = div.classList;
        assert(list instanceof DOMTokenList && list === div.classList, 'SameObject');
        assert(list.length === 0 && list.value === '' && !div.hasAttribute('class'), 'empty');
        list.remove('x');
        assert(!div.hasAttribute('class'), 'no attribute created for an empty update');
        div.className = '  b a b\tc ';
        assert(list.length === 3 && list[0] === 'b' && list.item(2) === 'c' && list.item(3) === null, 'live parse');
        assert(list.contains('a') && !list.contains('d'), 'contains');
        list.add('d', 'a');
        assert(div.className === 'b a c d', 'add serializes the set');
        assert(list.toggle('a') === false && list.toggle('e', false) === false, 'toggle off');
        assert(list.toggle('a', true) === true && list.contains('a'), 'toggle forced on');
        assert(list.replace('b', 'c') === true && div.className === 'c d a', 'replace merges duplicates');
        assert(list.replace('zz', 'y') === false, 'replace of a missing token');
        assert(throws('SyntaxError', () => list.add('')), 'empty token');
        assert(throws('InvalidCharacterError', () => list.toggle('a b')), 'whitespace token');
        assert(throws('TypeError', () => list.supports('a')), 'class has no supported tokens');
        assert([...list].join() === 'c,d,a' && String(list) === 'c d a', 'iterable and stringifier');
        div.classList = 'p q';
        assert(div.className === 'p q', 'PutForwards=value');
        // img and iframe reflect their attributes, loading among them.
        const img = document.createElement('img');
        assert(img instanceof HTMLImageElement && img.loading === 'eager', 'img interface');
        img.setAttribute('loading', 'LAZY');
        assert(img.loading === 'lazy', 'keywords are case-insensitive');
        img.loading = 'bogus';
        assert(img.getAttribute('loading') === 'bogus' && img.loading === 'eager', 'invalid default');
        assert(img.decoding === 'auto' && img.alt === '' && img.src === '', 'defaults');
        img.isMap = true;
        assert(img.getAttribute('ismap') === '', 'boolean reflection');
        const frame = document.createElement('iframe');
        assert(frame instanceof HTMLIFrameElement && frame.srcdoc === '', 'iframe interface');
        frame.srcdoc = '<p>hi';
        assert(frame.getAttribute('srcdoc') === '<p>hi', 'srcdoc reflects');
        const sandbox = frame.sandbox;
        assert(sandbox === frame.sandbox && sandbox.length === 0, 'sandbox list');
        assert(sandbox.supports('ALLOW-SCRIPTS') && !sandbox.supports('allow-everything'), 'supports');
        sandbox.add('allow-scripts', 'allow-forms');
        assert(frame.getAttribute('sandbox') === 'allow-scripts allow-forms', 'sandbox updates');
        // Hyperlinks: a, area and link interfaces with relList.
        const a = document.createElement('a');
        assert(a instanceof HTMLAnchorElement && a.relList === a.relList, 'anchor interface');
        a.relList.add('noopener', 'nofollow');
        assert(a.rel === 'noopener nofollow' && a.relList.supports('NOREFERRER'), 'a relList');
        assert(!a.relList.supports('stylesheet') && a.href === '' && a.target === '', 'a defaults');
        const link = document.createElement('link');
        assert(link instanceof HTMLLinkElement && link.relList.supports('stylesheet'), 'link relList');
        link.relList = 'stylesheet alternate';
        assert(link.getAttribute('rel') === 'stylesheet alternate' && link.relList.length === 2, 'link rel');
        assert(link.crossOrigin === null && img.crossOrigin === null, 'CORS settings default');
        link.crossOrigin = 'USE-CREDENTIALS';
        img.crossOrigin = 'bogus';
        assert(link.crossOrigin === 'use-credentials' && img.crossOrigin === 'anonymous', 'CORS keywords');
        img.crossOrigin = null;
        assert(!img.hasAttribute('crossorigin'), 'null removes the CORS attribute');
        assert(document.createElement('area') instanceof HTMLAreaElement, 'area interface');
        assert(img.complete && img.naturalWidth === 0 && img.currentSrc === '', 'no request');
        frame.sandbox = 'allow-popups';
        assert(sandbox.value === 'allow-popups', 'sandbox PutForwards');
        frame.allowFullscreen = true;
        assert(frame.hasAttribute('allowfullscreen') && frame.loading === 'eager', 'iframe booleans');
        assert(throws('TypeError', () => Object.getOwnPropertyDescriptor(
            HTMLImageElement.prototype, 'alt').get.call(frame)), 'brand checks');
        "#,
    );
}

#[test]
fn details_dialog_and_popover_follow_their_state_machines() {
    let document = Document::new(DocumentLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, Rc::new(Tree(RefCell::new(document)))).unwrap();
    agent.heap.set_gc_stress(true);
    expect_ok(
        &mut agent,
        r#"
        function assert(condition, message) { if (!condition) throw message; }
        globalThis.log = [];
        const html = document.appendChild(document.createElement('html'));
        const body = html.appendChild(document.createElement('body'));
        // details: summary clicks toggle, name groups are exclusive.
        const [one, two] = ['details', 'details'].map(n => body.appendChild(document.createElement(n)));
        assert(one instanceof HTMLDetailsElement && !one.open, 'details interface');
        one.name = two.name = 'group';
        const summary = one.appendChild(document.createElement('summary'));
        one.addEventListener('toggle', e => log.push('one ' + e.oldState + '>' + e.newState));
        two.addEventListener('toggle', e => log.push('two ' + e.newState));
        two.open = true;
        summary.click();
        assert(one.open && !two.open && one.hasAttribute('open'), 'summary opens one and closes two');
        // dialog: show/showModal/close with returnValue and events.
        const dialog = body.appendChild(document.createElement('dialog'));
        assert(dialog instanceof HTMLDialogElement && !dialog.open && dialog.returnValue === '', 'dialog');
        dialog.addEventListener('close', () => log.push('close ' + dialog.returnValue));
        dialog.addEventListener('cancel', e => { log.push('cancel'); e.preventDefault(); });
        dialog.showModal();
        assert(dialog.open, 'showModal opens');
        let threw = '';
        try { dialog.show(); } catch (e) { threw = e.name; }
        assert(threw === 'InvalidStateError', 'show() on a modal dialog throws');
        dialog.requestClose('nope');
        assert(dialog.open, 'a canceled requestClose keeps it open');
        dialog.close('done');
        assert(!dialog.open && dialog.returnValue === 'done', 'close sets returnValue');
        const loose = document.createElement('dialog');
        threw = '';
        try { loose.showModal(); } catch (e) { threw = e.name; }
        assert(threw === 'InvalidStateError', 'a disconnected dialog cannot be modal');
        // popover: attribute reflection, invokers, auto popovers hide each other.
        const [p1, p2] = ['div', 'div'].map(n => body.appendChild(document.createElement(n)));
        p1.popover = 'auto'; p2.popover = 'bogus';
        assert(p1.popover === 'auto' && p2.popover === 'manual' && document.body !== null, 'popover states');
        p1.id = 'p1';
        p1.addEventListener('beforetoggle', e => log.push('before ' + e.newState));
        const button = body.appendChild(document.createElement('button'));
        button.setAttribute('popovertarget', 'p1');
        button.click();
        assert(p1.matches(':popover-open'), 'the invoker shows its popover');
        assert(p2.togglePopover() === true && p1.matches(':popover-open'), 'manual popovers do not hide auto ones');
        p1.hidePopover();
        assert(!p1.matches(':popover-open') && p2.togglePopover({force: false}) === false, 'hide');
        threw = '';
        try { button.showPopover(); } catch (e) { threw = e.name; }
        assert(threw === 'NotSupportedError', 'only popovers show');
        assert(new ToggleEvent('toggle', {newState: 'open'}).newState === 'open', 'ToggleEvent');
        // Attribute APIs run the change steps: details toggles, dialog and popover state.
        const three = body.appendChild(document.createElement('details'));
        three.addEventListener('toggle', e => log.push('three ' + e.oldState + '>' + e.newState));
        three.setAttribute('open', '');
        three.removeAttribute('open');
        three.setAttribute('OPEN', 'x');
        const modal = body.appendChild(document.createElement('dialog'));
        modal.showModal();
        modal.removeAttribute('open');
        assert(!modal.matches(':modal'), 'removing open leaves the top layer');
        p1.showPopover();
        p1.setAttribute('popover', 'manual');
        assert(!p1.matches(':popover-open'), 'changing the popover state hides it');
        Promise.resolve().then(() => { globalThis.done = log.join(', '); });
    "#,
    );
    agent.run_jobs().unwrap();
    let done = agent.evaluate("done").unwrap();
    let done = agent.to_string(done).unwrap();
    let done = String::from_utf16_lossy(&agent.heap.string(done).units().collect::<Vec<_>>());
    assert_eq!(
        done,
        "cancel, before open, before closed, before open, two closed, one closed>open, close done, three closed>open"
    );
}
