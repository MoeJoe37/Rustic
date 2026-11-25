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
            assert(rejects(TypeError, () => table.get(at(2))(null)), 'unsupported host signature has a typed error');
            clear(at(0));
            assert(table.get(at(0)) === null, 'guest clear visible to JS');
            assert(rejects(WebAssembly.RuntimeError, () => call(at(0))), 'guest observes null');
            table.set(at(0), hidden);
            assert(call(at(0)) === 43, 'write visible to indirect call');
            assert(linked === foreign, 'import re-export identity');
            table.set(at(1), foreign);
            assert(table.get(at(1)) === foreign && call(at(1)) === 17, 'existing import used by table');
            assert(rejects(TypeError, () => table.set(at(1), unrelated)), 'a foreign function needs a binding');
            assert(rejects(TypeError, () => table.set(at(1), new WebAssembly.Memory({initial:0}))), 'memory is not a function');
            assert(table.get(at(1)) === foreign, 'failed write preserves entry');
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
