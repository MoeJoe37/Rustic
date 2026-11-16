use rustic_dom::{NodeId, NodeKind};
use rustic_engine::{
    Engine, InputEvent, KeyInput, Modifiers, ScriptQueueError, ScriptStop, ScriptTurn,
    ScriptedDocument, TaskLimits,
};
use rustic_html::ParserLimits;
use rustic_js::Source;

fn page(engine: &mut Engine, markup: &str, limits: TaskLimits) -> ScriptedDocument {
    let mut load = engine.begin_document(ParserLimits::default()).unwrap();
    load.feed(markup.as_bytes()).unwrap();
    load.finish().unwrap().into_scripted(limits).unwrap()
}

fn id(page: &ScriptedDocument, id: &str) -> NodeId {
    let owner = page.document();
    let tree = owner.parsed().document();
    tree.walk(tree.root())
        .unwrap()
        .map(|(node, _)| node)
        .find(|&node| {
            let NodeKind::Element(element) = tree.node(node).unwrap().kind() else {
                return false;
            };
            element.attributes().iter().any(|attribute| {
                attribute.name().local() == "id"
                    && attribute
                        .value()
                        .as_units()
                        .iter()
                        .copied()
                        .eq(id.encode_utf16())
            })
        })
        .unwrap()
}

fn source(text: &str) -> Source {
    Source::from_str_limited(text, 8192).unwrap()
}

fn run(page: &mut ScriptedDocument, text: &str) {
    page.queue_script(source(text)).unwrap();
    assert_eq!(page.run_next(), Ok(Some(ScriptTurn::Completed)));
}

fn deliver(page: &mut ScriptedDocument, target: NodeId, input: InputEvent) {
    page.queue_input(target, input).unwrap();
    assert_eq!(page.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(page.take_listener_error_count(), 0);
}

fn click() -> InputEvent {
    InputEvent::Click {
        client_x: 12,
        client_y: -4,
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::default()
        },
    }
}

fn key(name: &str) -> KeyInput {
    KeyInput {
        key: name.into(),
        code: if name == " " { "Space" } else { name }.into(),
        repeat: false,
        location: 0,
        modifiers: Modifiers::default(),
    }
}

#[test]
fn the_host_click_carries_pointer_fields_and_finishes_its_checkpoint() {
    let mut engine = Engine::new(1);
    let mut page = page(
        &mut engine,
        "<!doctype html><button type=button><span id=hit>click</span></button>",
        TaskLimits::default(),
    );
    let hit = id(&page, "hit");
    run(
        &mut page,
        r#"
        const hit = document.getElementById("hit");
        const button = hit.parentNode;
        let saved, trace = "";
        window.addEventListener("click", e => {
            if (!e.isTrusted || e.target !== hit || e.eventPhase !== Event.CAPTURING_PHASE)
                throw "capture";
            trace += "capture ";
        }, {capture: true, once: true});
        button.addEventListener("click", e => {
            if (!(e instanceof PointerEvent) || !(e instanceof MouseEvent) ||
                !(e instanceof UIEvent) || e.clientX !== 12 || e.clientY !== -4 ||
                e.button !== 0 || e.buttons !== 0 || e.pointerId !== 1 || e.pointerType !== "mouse" ||
                e.detail !== 1 || !e.isPrimary || !e.shiftKey || e.ctrlKey ||
                !e.bubbles || !e.composed || !e.cancelable || e.view !== window || !e.isTrusted)
                throw "host pointer payload";
            saved = e;
            trace += "bubble ";
            Promise.resolve().then(() => {
                if (e.isTrusted && e.currentTarget === null) trace += "checkpoint";
            });
        }, {once: true});
    "#,
    );
    deliver(&mut page, hit, click());
    run(
        &mut page,
        r#"
        if (trace !== "capture bubble checkpoint" || !saved.isTrusted ||
            new PointerEvent("click", {isTrusted:true}).isTrusted)
            throw "click completion and trust";
        let redispatch = false;
        hit.addEventListener("click", e => { redispatch = e === saved && !e.isTrusted; });
        hit.dispatchEvent(saved);
        if (!redispatch || saved.isTrusted) throw "script redispatch trust";
    "#,
    );
}

#[test]
fn keyboard_defaults_follow_the_uncanceled_press_and_release() {
    let mut engine = Engine::new(1);
    let mut page = page(
        &mut engine,
        "<!doctype html><button id=b type=button></button><input id=c type=CHECKBOX checked>",
        TaskLimits::default(),
    );
    let button = id(&page, "b");
    let checkbox = id(&page, "c");
    run(
        &mut page,
        r#"
        const button = document.getElementById("b"), box = document.getElementById("c");
        let trace = "", cancel = "";
        document.addEventListener("keydown", e => {
            if (!(e instanceof KeyboardEvent) || !e.isTrusted || e.isComposing ||
                e.code !== (e.key === " " ? "Space" : "Enter") || e.location !== 0)
                throw "keyboard payload";
            trace += e.repeat ? "repeat " : "down ";
            if (cancel === "down") e.preventDefault();
        });
        document.addEventListener("keyup", e => {
            trace += "up ";
            if (cancel === "up") e.preventDefault();
        });
        button.addEventListener("click", e => {
            if (!(e instanceof PointerEvent) || !e.isTrusted || e.pointerId !== -1 ||
                e.pointerType !== "" || e.detail !== 0 || e.isPrimary)
                throw "keyboard click";
            trace += "button ";
        });
        box.indeterminate = true;
        box.addEventListener("click", e => {
            if (!e.isTrusted || box.checked || box.indeterminate) throw "preactivation";
            trace += "click ";
            if (cancel === "click") e.preventDefault();
            Promise.resolve().then(() => trace += "job ");
        });
        box.addEventListener("input", e => {
            if (!e.isTrusted || e.cancelable || !e.composed) throw "input flags";
            trace += "input ";
        });
        box.addEventListener("change", e => {
            if (!e.isTrusted || e.cancelable || e.composed) throw "change flags";
            trace += "change ";
        });
    "#,
    );
    deliver(&mut page, button, InputEvent::KeyDown(key("Enter")));
    run(
        &mut page,
        r#"if (trace !== "down button ") throw trace; trace=""; cancel="down";"#,
    );
    deliver(&mut page, button, InputEvent::KeyDown(key("Enter")));
    deliver(&mut page, checkbox, InputEvent::KeyDown(key(" ")));
    deliver(&mut page, checkbox, InputEvent::KeyUp(key(" ")));
    run(
        &mut page,
        r#"if (trace !== "down down up ") throw trace; trace=""; cancel="up";"#,
    );
    deliver(&mut page, checkbox, InputEvent::KeyDown(key(" ")));
    deliver(&mut page, checkbox, InputEvent::KeyUp(key(" ")));
    deliver(&mut page, checkbox, InputEvent::KeyUp(key(" ")));
    run(
        &mut page,
        r#"if (trace !== "down up up ") throw trace; trace=""; cancel="click";"#,
    );
    deliver(&mut page, checkbox, InputEvent::KeyDown(key(" ")));
    deliver(&mut page, checkbox, InputEvent::KeyUp(key(" ")));
    run(
        &mut page,
        r#"
        if (trace !== "down up click job " || !box.checked || !box.indeterminate) throw trace;
        trace=""; cancel="";
    "#,
    );
    deliver(&mut page, checkbox, InputEvent::KeyUp(key(" ")));
    deliver(&mut page, checkbox, InputEvent::KeyDown(key(" ")));
    deliver(
        &mut page,
        checkbox,
        InputEvent::KeyDown(KeyInput {
            repeat: true,
            ..key(" ")
        }),
    );
    deliver(&mut page, checkbox, InputEvent::KeyUp(key(" ")));
    run(
        &mut page,
        r#"
        if (trace !== "up down repeat up click input change job " || box.checked ||
            box.indeterminate || !box.defaultChecked) throw trace;
        trace="";
    "#,
    );
    deliver(
        &mut page,
        button,
        InputEvent::KeyDown(KeyInput {
            modifiers: Modifiers {
                control: true,
                ..Modifiers::default()
            },
            ..key("Enter")
        }),
    );
    deliver(&mut page, button, InputEvent::KeyDown(key(" ")));
    deliver(&mut page, checkbox, InputEvent::KeyUp(key(" ")));
    run(&mut page, r#"if (trace !== "down down up ") throw trace;"#);
}

#[test]
fn disabled_controls_and_targets_detached_during_a_key_handler_do_not_activate() {
    let mut engine = Engine::new(1);
    let mut page = page(
        &mut engine,
        "<!doctype html><fieldset disabled><legend><input id=first type=checkbox></legend>         <legend><input id=second type=checkbox></legend><input id=direct type=checkbox>         </fieldset><button id=disabled disabled><span id=child></span></button>         <button id=removed type=button></button>",
        TaskLimits::default(),
    );
    let first = id(&page, "first");
    let second = id(&page, "second");
    let direct = id(&page, "direct");
    let child = id(&page, "child");
    let removed = id(&page, "removed");
    run(
        &mut page,
        r#"
        let trace = "";
        document.addEventListener("click", event => trace += event.target.getAttribute("id"));
        const removed = document.getElementById("removed");
        removed.addEventListener("keydown", () => removed.parentNode.removeChild(removed));
        removed.addEventListener("click", () => trace += "unexpected");
    "#,
    );
    for target in [first, second, direct, child] {
        deliver(&mut page, target, click());
    }
    deliver(&mut page, removed, InputEvent::KeyDown(key("Enter")));
    run(
        &mut page,
        r#"
        if (trace !== "first" || !document.getElementById("first").checked ||
            document.getElementById("second").checked || document.getElementById("direct").checked)
            throw trace;
    "#,
    );
}

#[test]
fn queued_input_drops_adopted_targets_and_releases_shared_limits() {
    let mut engine = Engine::new(1);
    let mut page = page(
        &mut engine,
        "<!doctype html><button id=b type=button></button>",
        TaskLimits {
            pending_scripts: 2,
            pending_source_units: 1024,
            ..TaskLimits::default()
        },
    );
    let button = id(&page, "b");
    let foreign = {
        let mut engine = Engine::new(1);
        let other = self::page(
            &mut engine,
            "<button id=foreign></button>",
            TaskLimits::default(),
        );
        id(&other, "foreign")
    };
    assert_eq!(
        page.queue_input(foreign, click()),
        Err(ScriptQueueError::InvalidInputTarget)
    );
    let oversized = KeyInput {
        key: "😀".repeat(512).into(),
        ..key("Enter")
    };
    assert_eq!(
        page.queue_input(button, InputEvent::KeyDown(oversized)),
        Err(ScriptQueueError::Limit)
    );

    run(
        &mut page,
        r#"
        let hits = 0;
        const button = document.getElementById("b");
        button.addEventListener("keydown", () => hits++);
        const other = document.implementation.createHTMLDocument();
    "#,
    );
    page.queue_script(source("other.adoptNode(button);"))
        .unwrap();
    page.queue_input(button, InputEvent::KeyDown(key("Enter")))
        .unwrap();
    assert_eq!(
        page.queue_input(button, click()),
        Err(ScriptQueueError::Limit)
    );
    assert_eq!(page.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(
        page.queue_input(button, click()),
        Err(ScriptQueueError::InvalidInputTarget)
    );
    assert_eq!(page.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(page.run_next(), Ok(None));
    run(
        &mut page,
        r#"if (hits !== 0 || button.ownerDocument !== other) throw "stale delivery";"#,
    );
}

#[test]
fn document_cancellation_discards_input_before_delivery() {
    let mut engine = Engine::new(1);
    let mut page = page(&mut engine, "<button id=b></button>", TaskLimits::default());
    let button = id(&page, "b");
    page.queue_input(button, click()).unwrap();
    engine.shutdown();
    assert_eq!(page.run_next(), Err(ScriptStop::DocumentCancelled));
    assert_eq!(
        page.queue_input(button, click()),
        Err(ScriptQueueError::Stopped(ScriptStop::DocumentCancelled))
    );
}
