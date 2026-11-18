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
