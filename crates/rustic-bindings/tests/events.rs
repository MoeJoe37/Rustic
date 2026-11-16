use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use rustic_bindings::dom_exception;
use rustic_bindings::page::{self, PageTree};
use rustic_dom::{Document, DocumentLimits};
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

fn scripted_page() -> (Agent, Rc<Tree>) {
    let tree = Rc::new(Tree(RefCell::new(
        Document::new(DocumentLimits::default()).unwrap(),
    )));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree.clone()).unwrap();
    agent.heap.set_gc_stress(true);
    (agent, tree)
}

#[test]
fn event_phases_follow_the_dom_and_top_level_window() {
    let (mut agent, _) = scripted_page();
    assert_eq!(agent.evaluate(r#"
        (() => {
            const root = document.createElement("main");
            const leaf = document.createElement("button");
            document.appendChild(root);
            root.appendChild(leaf);
            const targets = [leaf, root, document, window];
            const names = ["l", "r", "d", "w"];
            const seen = [];
            const detail = {message: "\ud800"};
            const event = new CustomEvent("\ud800", {bubbles: true, cancelable: true, composed: true, detail});
            if (!(event instanceof Event) || !(window instanceof EventTarget) ||
                !(leaf instanceof EventTarget) || !(document instanceof EventTarget) ||
                event.type !== "\ud800" || event.detail !== detail ||
                event.target !== null || event.timeStamp < 0 || event.isTrusted ||
                Reflect.deleteProperty(event, "isTrusted") ||
                Object.getOwnPropertyDescriptor(event, "isTrusted").configurable)
                throw "initial event attributes";

            let broken = false;
            targets.forEach((target, index) => {
                for (const capture of [true, false]) {
                    target.addEventListener("\ud800", function (e) {
                        if (this !== target || e.currentTarget !== target || e.target !== leaf ||
                            e.srcElement !== leaf || e !== event ||
                            e.composedPath().some((item, i) => item !== targets[i]) ||
                            e.composedPath().length !== 4) broken = true;
                        seen.push(names[index] + e.eventPhase);
                    }, capture);
                }
            });
            if (!leaf.dispatchEvent(event) || broken ||
                seen.join() !== "w1,d1,r1,l2,l2,r3,d3,w3" ||
                event.eventPhase !== Event.NONE || event.currentTarget !== null ||
                event.target !== leaf || event.composedPath().length !== 0 ||
                !event.bubbles || !event.cancelable || !event.composed)
                throw "dispatch order or final state";
            let captured = 0;
            let bubbled = 0;
            root.addEventListener("solo", () => captured++, true);
            root.addEventListener("solo", () => bubbled++);
            leaf.dispatchEvent(new Event("solo"));
            if (captured !== 1 || bubbled !== 0) throw "non-bubbling capture";
            window.addEventListener("load", () => bubbled++, true);
            document.dispatchEvent(new Event("load"));
            if (bubbled !== 0) throw "document load reached the window";
            let wrongBrand = 0;
            for (const call of [
                () => leaf.dispatchEvent(Object.create(Event.prototype)),
                () => EventTarget.prototype.addEventListener.call({}, "a", () => {}),
                () => Object.getOwnPropertyDescriptor(CustomEvent.prototype, "detail").get.call(new Event("a")),
            ]) {
                try { call(); } catch (error) { if (error instanceof TypeError) wrongBrand++; }
            }
            return wrongBrand === 3 && Event.AT_TARGET === 2 && event.BUBBLING_PHASE === 3;
        })();
    "#).unwrap(), Value::TRUE);
    assert_eq!(page::take_listener_error_count(&agent), 0);
}

#[test]
fn listeners_handle_reentry_once_removal_and_ordered_option_reads() {
    let (mut agent, _) = scripted_page();
    assert_eq!(agent.evaluate(r#"
        (() => {
            const bus = new EventTarget();
            const seen = [];
            const skipped = () => seen.push("skipped");
            const newListener = () => seen.push("new");
            const object = {
                handleEvent(e) { seen.push(this === object && e.currentTarget === bus ? "object" : "bad-rx"); }
            };
            const event = new Event("x");
            bus.addEventListener("x", e => {
                seen.push("first");
                bus.removeEventListener("x", skipped);
                bus.addEventListener("x", newListener);
                try { bus.dispatchEvent(e); } catch (error) { seen.push(error.name); }
                bus.dispatchEvent(new Event("x"));
            }, {once: true});
            bus.addEventListener("x", skipped);
            bus.addEventListener("x", object);
            bus.addEventListener("x", object, {once: true});
            bus.dispatchEvent(event);
            if (seen.join() !== "first,InvalidStateError,object,new,object") throw "reentrant order";
            const options = [];
            const cb = () => options.push("called");
            bus.addEventListener({toString() { options.push("type"); return 7; }}, cb, {
                get capture() { options.push("capture"); return true; },
                get once() { options.push("once"); return false; },
                get passive() { options.push("passive"); return false; },
                get signal() { options.push("signal"); return undefined; }
            });
            bus.addEventListener("07", () => options.push("different key"));
            bus.dispatchEvent(new Event("7"));
            bus.removeEventListener("7", cb, {
                get capture() { options.push("remove"); return true; },
                get once() { throw "remove must not read once"; }
            });
            bus.dispatchEvent(new Event(7));
            return options.join() === "type,capture,once,passive,signal,called,remove";
        })();
    "#).unwrap(), Value::TRUE);
    assert_eq!(page::take_listener_error_count(&agent), 0);
}

#[test]
fn cancellation_propagation_and_passive_listeners_keep_distinct_flags() {
    let (mut agent, _) = scripted_page();
    assert_eq!(agent.evaluate(r#"
        (() => {
            const root = document.createElement("main");
            const leaf = document.createElement("p");
            document.appendChild(root);
            root.appendChild(leaf);
            const trace = [];
            root.addEventListener("stop", e => { trace.push("stop"); e.stopPropagation(); }, true);
            root.addEventListener("stop", e => { trace.push(e.cancelBubble); e.cancelBubble = false; }, true);
            leaf.addEventListener("stop", () => trace.push("leaf"));
            leaf.dispatchEvent(new Event("stop", {bubbles: true}));
            leaf.addEventListener("immediate", e => { e.stopImmediatePropagation(); trace.push("first"); });
            leaf.addEventListener("immediate", () => trace.push("second"));
            root.addEventListener("immediate", () => trace.push("bubble"));
            leaf.dispatchEvent(new Event("immediate", {bubbles: true}));
            if (trace.join() !== "stop,true,first") throw "propagation flags";
            window.addEventListener("wheel", e => e.preventDefault(), {once: true});
            const wheel = new Event("wheel", {cancelable: true});
            if (!window.dispatchEvent(wheel) || !wheel.returnValue) throw "default passive";
            window.addEventListener("wheel", e => e.returnValue = false, {passive: false});
            if (window.dispatchEvent(wheel) || wheel.returnValue) throw "passive override";
            wheel.initEvent("fresh", true, false);
            wheel.preventDefault();
            if (wheel.type !== "fresh" || wheel.defaultPrevented || wheel.target !== null || !wheel.bubbles)
                throw "reinitialize event";
            leaf.addEventListener("report", () => { throw new Error("reported"); });
            leaf.addEventListener("report", e => e.preventDefault());
            const report = new Event("report", {cancelable: true});
            return !leaf.dispatchEvent(report) && report.defaultPrevented;
        })();
    "#).unwrap(), Value::TRUE);
    assert_eq!(page::take_listener_error_count(&agent), 1);
    assert_eq!(page::take_listener_error_count(&agent), 0);
}

#[test]
fn frozen_dispatch_path_and_listener_edges_survive_gc_and_adoption() {
    let (mut agent, tree) = scripted_page();
    assert_eq!(
        agent
            .evaluate(
                r#"
        var retained;
        function retainNode(node) { return () => node.parentNode; }
        (() => {
            let root = document.createElement("main");
            const leaf = document.createElement("button");
            root.appendChild(leaf);
            document.appendChild(root);
            const out = [];
            const other = new Document();
            const listener = {handleEvent(e) {
                if (e.currentTarget !== leaf || leaf.ownerDocument !== other)
                    out.push("lost leaf");
                out.push("leaf");
            }};
            leaf.addEventListener("move", listener);
            leaf.addEventListener("later", () => out.push("adopted"));
            root.addEventListener("move", () => other.adoptNode(leaf), {capture: true});
            root.addEventListener("move", () => out.push("old root"));
            window.addEventListener("move", () => out.push("window"), {once: true});
            root.addEventListener("unused", retainNode(root));
            retained = new Event("move", {bubbles: true});
            leaf.dispatchEvent(retained);
            leaf.dispatchEvent(new Event("later"));
            document.removeChild(root);
            root = null;
            return out.join() === "leaf,old root,window,adopted" &&
                retained.target === leaf && retained.target.ownerDocument === other;
        })();
    "#
            )
            .unwrap(),
        Value::TRUE
    );
    assert_eq!(page::take_listener_error_count(&agent), 0);
    assert_eq!(
        agent
            .evaluate("(() => retained.target.ownerDocument.defaultView === null)();")
            .unwrap(),
        Value::TRUE
    );
    assert_eq!(tree.0.borrow().node_count(), 1);
    agent.evaluate("retained = null; (() => 0)();").unwrap();
}

#[test]
fn canceled_native_dispatch_can_reuse_the_same_event() {
    let (mut agent, _) = scripted_page();
    agent
        .evaluate(
            r#"
        var bus = new EventTarget();
        var event = new Event("busy");
        var calls = 0;
        for (let i = 0; i < 64; i++) bus.addEventListener("busy", () => calls += i !== -1);
    "#,
        )
        .unwrap();
    agent.set_step_budget(Some(24));
    assert!(matches!(
        agent.evaluate("bus.dispatchEvent(event)"),
        Err(rustic_js::EvalError::Cancelled)
    ));
    agent.set_step_budget(None);
    assert_eq!(agent.evaluate("event.eventPhase === 0 && event.currentTarget === null && event.composedPath().length === 0 && calls === 0 && bus.dispatchEvent(event) && calls === 64").unwrap(), Value::TRUE);
}

#[test]
fn synthetic_mouse_click_preactivation_rolls_back_and_traces_control_state() {
    let (mut agent, _) = scripted_page();
    assert_eq!(
        agent
            .evaluate(
                r#"
        (() => {
            const main = document.createElement("main");
            document.appendChild(main);
            const box = document.createElement("input");
            box.setAttribute("type", "checkbox");
            main.appendChild(box);
            if (!(box instanceof HTMLInputElement) || !(box instanceof Element) ||
                box.checked || box.defaultChecked || box.indeterminate)
                throw "initial control state";
            box.defaultChecked = true;
            if (!box.checked || box.getAttribute("checked") !== "") throw "content reflection";
            box.indeterminate = true;
            let trace = "";
            const cancel = (event) => {
                if (box.checked || box.indeterminate || event.isTrusted ||
                    event.button !== -1 || event.buttons !== 1) throw "preactivation or payload";
                trace += "cancel ";
                event.preventDefault();
            };
            box.addEventListener("click", cancel, {once:true});
            main.addEventListener("input", e => {
                if (e.target !== box || !e.isTrusted) throw "input";
                trace += "input ";
            });
            main.addEventListener("change", e => {
                if (e.target !== box || !e.isTrusted) throw "change";
                trace += "change ";
            });
            const mouse = new MouseEvent("click", {bubbles:true, cancelable:true, button:65535,
                buttons:65537, view:window});
            if (box.dispatchEvent(mouse) || !box.checked || !box.indeterminate ||
                trace !== "cancel " || mouse.view !== window)
                throw "rollback";
            box.dispatchEvent(new Event("click"));
            if (!box.checked || !box.indeterminate) throw "generic event activation";
            const successful = new MouseEvent("before", {clientX:5});
            successful.initEvent("click", true, true);
            if (!box.dispatchEvent(successful) || successful.clientX !== 5 || box.checked ||
                box.indeterminate || trace !== "cancel input change ")
                throw "successful mouse dispatch";
            box.defaultChecked = false;
            box.setAttribute("checked", "kept");
            box.defaultChecked = true;
            if (box.checked || !box.defaultChecked || box.getAttribute("checked") !== "kept")
                throw "dirty checkedness";
            const getter = Object.getOwnPropertyDescriptor(PointerEvent.prototype, "pointerId").get;
            try { getter.call(mouse); } catch (e) { return e instanceof TypeError; }
            return false;
        })();
    "#
            )
            .unwrap(),
        Value::TRUE
    );
    assert_eq!(page::take_listener_error_count(&agent), 0);
}
