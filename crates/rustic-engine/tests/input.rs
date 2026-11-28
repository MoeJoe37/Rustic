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
    let turn = page.run_next();
    if turn != Ok(Some(ScriptTurn::Completed)) {
        panic!("{turn:?}: {:?}", page.take_console_messages());
    }
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

#[test]
fn trusted_input_gives_transient_and_sticky_user_activation() {
    let mut engine = Engine::new(1);
    let mut load = engine.begin_document(ParserLimits::default()).unwrap();
    load.feed(b"<button id=b>x</button>").unwrap();
    let clock = rustic_base::ManualClock::default();
    let mut page = load
        .finish()
        .unwrap()
        .into_scripted_with_clock(TaskLimits::default(), clock)
        .unwrap();
    let button = {
        let owner = page.document();
        let tree = owner.parsed().document();
        tree.walk(tree.root())
            .unwrap()
            .map(|(node, _)| node)
            .find(|&node| matches!(tree.node(node).unwrap().kind(), NodeKind::Element(e) if e.name().local() == "button"))
            .unwrap()
    };
    let check = |page: &mut ScriptedDocument<rustic_base::ManualClock>, text: &str| {
        page.queue_script(source(text)).unwrap();
        assert_eq!(page.run_next(), Ok(Some(ScriptTurn::Completed)), "{text}");
    };
    check(
        &mut page,
        "var seen = []; var a = navigator.userActivation;\
         if (a !== navigator.userActivation || !(a instanceof UserActivation)) throw 'identity';\
         if (a.isActive || a.hasBeenActive) throw 'initially inactive';\
         document.getElementById('b').addEventListener('click', () => seen.push(a.isActive));\
         document.getElementById('b').dispatchEvent(new MouseEvent('click'));\
         if (a.isActive || a.hasBeenActive || seen.join() !== 'false') throw 'synthetic events never activate';",
    );
    assert!(!page.has_transient_activation());
    page.queue_input(button, click()).unwrap();
    assert_eq!(page.run_next(), Ok(Some(ScriptTurn::Completed)));
    check(
        &mut page,
        "if (seen.join() !== 'false,true' || !navigator.userActivation.isActive) throw seen.join();",
    );
    page.clock()
        .advance(std::time::Duration::from_millis(4999))
        .unwrap();
    check(
        &mut page,
        "if (!navigator.userActivation.isActive) throw 'still active';",
    );
    page.clock()
        .advance(std::time::Duration::from_millis(1))
        .unwrap();
    check(
        &mut page,
        "if (navigator.userActivation.isActive || !navigator.userActivation.hasBeenActive) throw 'expired';",
    );
    // Escape never activates; another key does, and a consumer spends it.
    let key = |key: &str| {
        InputEvent::KeyDown(KeyInput {
            key: key.into(),
            code: key.into(),
            repeat: false,
            location: 0,
            modifiers: Modifiers::default(),
        })
    };
    page.queue_input(button, key("Escape")).unwrap();
    page.run_next().unwrap();
    assert!(!page.has_transient_activation());
    page.queue_input(button, key("a")).unwrap();
    page.run_next().unwrap();
    assert!(page.has_transient_activation());
    assert!(page.consume_user_activation());
    assert!(!page.has_transient_activation());
    check(
        &mut page,
        "if (!navigator.userActivation.hasBeenActive) throw 'sticky';",
    );
}

#[test]
fn window_open_needs_and_spends_transient_activation() {
    let mut engine = Engine::new(1);
    let mut page = page(&mut engine, "<button id=b></button>", TaskLimits::default());
    run(
        &mut page,
        "document.getElementById('b').onclick = () => { window.result = \
           [open('https://b.test/one'), open('https://b.test/two')] };",
    );
    // No gesture: the popup blocker answers null and records nothing.
    run(
        &mut page,
        "window.blocked = open('https://b.test/blocked'); \
         window.self_target = open('https://b.test/', '_self');",
    );
    assert!(page.take_popups().is_empty());
    // One click opens one popup; the second call has no activation left.
    let button = id(&page, "b");
    deliver(&mut page, button, click());
    let popups = page.take_popups();
    assert_eq!(popups.len(), 1);
    assert_eq!(popups[0].url.serialize(), "https://b.test/one");
    assert!(!popups[0].noopener && !popups[0].noreferrer);
    assert!(!page.has_transient_activation());
    run(
        &mut page,
        "document.getElementById('b').onclick = () => \
           open('https://b.test/x', 'name', 'noreferrer');",
    );
    deliver(&mut page, button, click());
    let popups = page.take_popups();
    assert!(popups[0].noopener && popups[0].noreferrer);
    // An invalid URL throws a SyntaxError without spending activation.
    run(
        &mut page,
        "document.getElementById('b').onclick = () => { \
           try { open('http://[') } catch (e) { window.error = e.name } \
           open('about:blank', '', 'noopener') };",
    );
    deliver(&mut page, button, click());
    let popups = page.take_popups();
    assert_eq!(popups.len(), 1);
    assert_eq!(popups[0].url.serialize(), "about:blank");
    assert!(popups[0].noopener);
    run(
        &mut page,
        "if (window.error !== 'SyntaxError' || window.blocked !== null) throw 1",
    );
}

#[test]
fn modal_dialogs_trap_focus_and_close_on_escape_and_backdrop_clicks() {
    let mut engine = Engine::new(1);
    let mut page = page(
        &mut engine,
        "<!doctype html><button id=opener type=button>open</button><p id=outside>text</p>\
         <dialog id=d closedby=any><input id=first><input id=auto autofocus></dialog>\
         <div id=p popover><button id=inner type=button>in</button></div>\
         <button id=invoker type=button popovertarget=p>toggle</button>",
        TaskLimits::default(),
    );
    let opener = id(&page, "opener");
    let outside = id(&page, "outside");
    let inner = id(&page, "inner");
    run(
        &mut page,
        r#"
        globalThis.$ = id => document.getElementById(id);
        globalThis.log = [];
        const d = $("d");
        d.addEventListener("cancel", () => log.push("cancel"));
        d.addEventListener("close", () => log.push("close"));
        d.addEventListener("click", e => log.push("click " + e.target.id));
        $("opener").focus();
        if (document.activeElement !== $("opener")) throw "opener focus";
        d.showModal();
        if (document.activeElement !== $("auto")) throw "autofocus delegate: " + document.activeElement;
        $("opener").focus();
        if (document.activeElement !== $("auto")) throw "content outside a modal dialog is inert";
        if (!$("opener").matches("button") || $("opener").inert) throw "inert IDL is the attribute";
    "#,
    );
    // Escape requests a close: cancel, then close, and focus goes back.
    deliver(&mut page, opener, InputEvent::KeyDown(key("Escape")));
    run(
        &mut page,
        r#"
        if ($("d").open || document.activeElement !== $("opener")) throw "escape closes and restores focus";
        $("d").showModal();
        if (document.activeElement !== $("auto")) throw "reopen";
    "#,
    );
    // A click on inert content lands on the dialog (its backdrop); with
    // closedby=any that light-dismisses it.
    deliver(&mut page, outside, click());
    run(
        &mut page,
        r#"
        if ($("d").open) throw "backdrop click closes a closedby=any dialog";
        if (log.join() !== "cancel,close,cancel,click d,close") throw "events: " + log.join();
        // Auto popovers: autofocus only, Escape hides, outside presses dismiss.
        const p = $("p");
        p.showPopover();
        if (document.activeElement !== $("opener")) throw "popovers without autofocus keep focus";
        $("inner").focus();
    "#,
    );
    deliver(&mut page, inner, InputEvent::KeyDown(key("Escape")));
    run(
        &mut page,
        r#"
        if ($("p").matches(":popover-open")) throw "escape hides the auto popover";
        if (document.activeElement !== $("opener")) throw "hiding restores focus from inside";
        $("p").showPopover();
    "#,
    );
    deliver(&mut page, inner, click());
    run(
        &mut page,
        r#"if (!$("p").matches(":popover-open")) throw "a press inside keeps the popover";"#,
    );
    deliver(&mut page, outside, click());
    run(
        &mut page,
        r#"
        if ($("p").matches(":popover-open")) throw "a press outside light-dismisses";
        // The `inert` attribute: not focusable, and clicks land on the parent.
        $("outside").inert = true;
        if (!$("outside").hasAttribute("inert")) throw "inert reflects";
        $("outside").tabIndex = 0;
        $("outside").focus();
        if (document.activeElement === $("outside")) throw "inert content is not focusable";
        document.body.addEventListener("click", e => log.push("body " + e.target.localName));
        log.length = 0;
    "#,
    );
    deliver(&mut page, outside, click());
    run(
        &mut page,
        r#"if (log.join() !== "body body") throw "inert click target: " + log.join();"#,
    );
}

#[test]
fn event_handler_content_attributes_compile_and_run_in_listener_order() {
    let mut engine = Engine::new(1);
    let mut page = page(
        &mut engine,
        "<!doctype html><body onload=\"log.push('load ' + (this === window))\">\
         <a id=a href=#x onclick=\"log.push('attr ' + event.type + ' ' + this.id); return false\">a</a>",
        TaskLimits::default(),
    );
    let a = id(&page, "a");
    run(
        &mut page,
        r#"
        globalThis.log = [];
        const a = document.getElementById("a");
        a.addEventListener("click", e => log.push("listener " + e.defaultPrevented));
        if (typeof a.onclick !== "function" || a.onclick.name !== "onclick") throw "compiled getter";
        if (document.body.onload !== null || typeof window.onload !== "function")
            throw "body onload sets the window handler";
        window.dispatchEvent(new Event("load"));
    "#,
    );
    deliver(&mut page, a, click());
    run(
        &mut page,
        r#"
        if (log.join() !== "load true,attr click a,listener true") throw "order: " + log.join();
        log.length = 0;
        a.setAttribute("onclick", "log.push('changed')");
        a.click();
        a.onclick = () => log.push("idl");
        a.click();
        a.setAttribute("onclick", "log.push('again')");
        a.click();
        a.removeAttribute("onclick");
        a.click();
        if (a.onclick !== null) throw "removing the attribute clears the handler";
        if (log.join() !== "changed,listener false,idl,listener false,again,listener false,listener false")
            throw "updates: " + log.join();
        a.setAttribute("onmouseover", "this is not javascript");
        if (a.onmouseover !== null) throw "an unparsable attribute is a null handler";
    "#,
    );
}
