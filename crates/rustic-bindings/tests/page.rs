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
