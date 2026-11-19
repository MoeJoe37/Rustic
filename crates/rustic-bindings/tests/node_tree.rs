//! Node names, values, relatives, containment, position and normalization.

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

fn run(script: &str) {
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
    page::install(&mut agent, tree).unwrap();
    agent.heap.set_gc_stress(true);
    agent.evaluate(script).unwrap();
}

#[test]
fn names_values_and_relatives_follow_the_node_kind() {
    run(r#"
        const body = document.body;
        const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg:path');
        const text = document.createTextNode('t');
        const comment = document.createComment('c');
        const pi = document.createProcessingInstruction('target', 'data');
        const fragment = document.createDocumentFragment();
        const names = [document, body, svg, text, comment, pi, fragment].map((n) => n.nodeName);
        if (names.join() !== '#document,BODY,svg:path,#text,#comment,target,#document-fragment')
            throw names.join();
        if (body.nodeValue !== null || document.nodeValue !== null || text.nodeValue !== 't') throw 'value';
        text.nodeValue = null;
        comment.nodeValue = 42;
        body.nodeValue = 'ignored';
        if (text.data !== '' || comment.data !== '42' || body.firstChild !== null) throw 'set';
        body.appendChild(text);
        body.appendChild(comment);
        if (body.lastChild !== comment || comment.previousSibling !== text ||
            text.previousSibling !== null || text.parentElement !== body ||
            document.documentElement.parentElement !== null || !body.hasChildNodes() ||
            fragment.hasChildNodes()) throw 'relatives';
        if (!body.isConnected || svg.isConnected || body.getRootNode() !== document ||
            svg.getRootNode() !== svg) throw 'connected';
        if (!document.contains(text) || !text.contains(text) || text.contains(body) ||
            body.contains(null) || !body.isSameNode(body) || body.isSameNode(text)) throw 'contains';
        const other = document.implementation.createHTMLDocument('');
        if (other.body.isConnected !== true || document.contains(other.body)) throw 'other document';
    "#);
}

#[test]
fn document_position_bits_match_tree_order() {
    run(r#"
        const body = document.body;
        const a = document.createElement('a');
        const b = document.createElement('b');
        const inner = document.createElement('i');
        body.appendChild(a);
        body.appendChild(b);
        a.appendChild(inner);
        const F = Node.DOCUMENT_POSITION_FOLLOWING, C = Node.DOCUMENT_POSITION_CONTAINS,
            CB = Node.DOCUMENT_POSITION_CONTAINED_BY;
        if (F !== 4 || C !== 8 || CB !== 16 || Node.prototype.DOCUMENT_POSITION_DISCONNECTED !== 1) throw 'constants';
        if (a.compareDocumentPosition(a) !== 0) throw 'same';
        if (a.compareDocumentPosition(b) !== F || b.compareDocumentPosition(a) !== 2) throw 'siblings';
        if (a.compareDocumentPosition(inner) !== (CB | F)) throw 'descendant';
        if (inner.compareDocumentPosition(a) !== (C | 2)) throw 'ancestor';
        if (inner.compareDocumentPosition(b) !== F) throw 'cousin';
        const lone = document.createElement('lone');
        const there = lone.compareDocumentPosition(a), back = a.compareDocumentPosition(lone);
        if ((there & 0x21) !== 0x21 || (back & 0x21) !== 0x21 || (there & 6) === (back & 6) ||
            (there & 6) === 0) throw 'disconnected ' + there + ' ' + back;
        try { a.compareDocumentPosition(null); throw 'null'; }
        catch (error) { if (!(error instanceof TypeError)) throw error; }
    "#);
}

#[test]
fn replace_child_and_normalize_edit_the_tree_atomically() {
    run(r#"
        const body = document.body;
        const a = document.createElement('a');
        const b = document.createElement('b');
        body.appendChild(a);
        if (body.replaceChild(b, a) !== a || body.firstChild !== b || a.parentNode !== null) throw 'replace';
        try { body.replaceChild(a, a); throw 'not a child'; }
        catch (error) { if (error.name !== 'NotFoundError') throw error; }
        try { body.replaceChild(document.documentElement, b); throw 'cycle'; }
        catch (error) { if (error.name !== 'HierarchyRequestError') throw error; }
        if (body.firstChild !== b) throw 'unchanged';
        try { document.replaceChild(document.createTextNode('x'), document.documentElement); throw 'text'; }
        catch (error) { if (error.name !== 'HierarchyRequestError') throw error; }
        const html = document.documentElement;
        const replacement = document.createElement('html');
        document.replaceChild(replacement, html);
        if (document.documentElement !== replacement) throw 'document element';
        document.replaceChild(html, replacement);
        // Cross-document replacement adopts the new node.
        const other = document.implementation.createHTMLDocument('');
        const foreign = other.createElement('span');
        body.replaceChild(foreign, b);
        if (foreign.ownerDocument !== document || body.firstChild !== foreign) throw 'adopt';

        const p = document.createElement('p');
        for (const data of ['', 'a', '', 'b', 'c']) p.appendChild(document.createTextNode(data));
        p.appendChild(document.createElement('br'));
        p.appendChild(document.createTextNode('d'));
        p.appendChild(document.createTextNode(''));
        const first = p.childNodes[1];
        p.normalize();
        const kinds = Array.from(p.childNodes, (n) => n.nodeType === 3 ? n.data : n.localName);
        if (kinds.join() !== 'abc,br,d' || p.firstChild !== first) throw kinds.join();
        // A lone text node's own data is not normalized away.
        const empty = document.createTextNode('');
        empty.normalize();
        if (empty.data !== '') throw 'self';
    "#);
}
