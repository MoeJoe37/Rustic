//! a11y.editing (ADR 0070): the caret and selection in text controls, the
//! HTML selection API, IME composition events, assistive-technology
//! actions, and the live state the accessibility tree reads.

use rustic_a11y::Checked;
use rustic_dom::{NodeId, NodeKind};
use rustic_engine::{
    AssistiveAction, Composition, Engine, InputEvent, KeyInput, Modifiers, ScriptTurn,
    ScriptedDocument, TaskLimits,
};
use rustic_html::ParserLimits;
use rustic_js::Source;

fn page(engine: &mut Engine, markup: &str) -> ScriptedDocument {
    let mut load = engine.begin_document(ParserLimits::default()).unwrap();
    load.feed(markup.as_bytes()).unwrap();
    load.finish()
        .unwrap()
        .into_scripted(TaskLimits::default())
        .unwrap()
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
            element.attributes().iter().any(|a| {
                a.name().local() == "id" && String::from_utf16_lossy(a.value().as_units()) == id
            })
        })
        .unwrap()
}

fn run(page: &mut ScriptedDocument, text: &str) {
    page.queue_script(Source::from_str_limited(text, 16384).unwrap())
        .unwrap();
    let turn = page.run_next();
    let console = page.take_console_messages();
    assert_eq!(turn, Ok(Some(ScriptTurn::Completed)), "{text}\n{console:?}");
}

fn deliver(page: &mut ScriptedDocument, target: NodeId, input: InputEvent) {
    page.queue_input(target, input).unwrap();
    assert_eq!(page.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(page.take_listener_error_count(), 0);
}

fn key(name: &str, shift: bool, control: bool) -> KeyInput {
    KeyInput {
        key: name.into(),
        code: name.into(),
        repeat: false,
        location: 0,
        modifiers: Modifiers {
            shift,
            control,
            ..Modifiers::default()
        },
    }
}

fn press(page: &mut ScriptedDocument, target: NodeId, name: &str, shift: bool) {
    deliver(page, target, InputEvent::KeyDown(key(name, shift, false)));
    deliver(page, target, InputEvent::KeyUp(key(name, shift, false)));
}

#[test]
fn caret_moves_by_code_point_and_edits_apply_at_the_selection() {
    let mut engine = Engine::new(1);
    let mut page = page(
        &mut engine,
        "<!doctype html><input id=t value='ab\u{1F600}cd'>",
    );
    let t = id(&page, "t");
    run(
        &mut page,
        "var t = document.getElementById('t'); var log = [];
         t.addEventListener('beforeinput', e => log.push('b:' + e.inputType + ':' + e.data));
         t.addEventListener('input', e => log.push('i:' + e.inputType + ':' + t.value));
         if (t.selectionStart !== 6 || t.selectionEnd !== 6) throw 'caret starts at the end';",
    );
    // Left twice crosses "d", "c"; once more crosses the whole emoji.
    for _ in 0..3 {
        press(&mut page, t, "ArrowLeft", false);
    }
    run(
        &mut page,
        "if (t.selectionStart !== 2) throw 'caret ' + t.selectionStart;",
    );
    press(&mut page, t, "X", false);
    run(
        &mut page,
        "if (t.value !== 'abX\u{1F600}cd' || t.selectionStart !== 3) throw t.value;",
    );
    // Shift+Right selects the emoji (two units); typing replaces it.
    press(&mut page, t, "ArrowRight", true);
    run(
        &mut page,
        "if (t.selectionStart !== 3 || t.selectionEnd !== 5 || t.selectionDirection !== 'forward') throw 'sel';",
    );
    press(&mut page, t, "Y", false);
    // Backspace at the caret, Delete after it, Home and End.
    press(&mut page, t, "Backspace", false);
    press(&mut page, t, "Delete", false);
    press(&mut page, t, "Home", false);
    press(&mut page, t, "Delete", false);
    press(&mut page, t, "End", false);
    press(&mut page, t, "Z", false);
    // Shift+Home selects backward; Control+A selects all.
    press(&mut page, t, "Home", true);
    run(
        &mut page,
        "if (t.selectionStart !== 0 || t.selectionEnd !== 4 || t.selectionDirection !== 'backward') throw [t.selectionStart, t.selectionEnd, t.selectionDirection];",
    );
    press(&mut page, t, "ArrowRight", false);
    deliver(&mut page, t, InputEvent::KeyDown(key("a", false, true)));
    run(
        &mut page,
        "if (t.value !== 'bXdZ' || t.selectionStart !== 0 || t.selectionEnd !== 4) throw t.value;
         const want = ['b:insertText:X', 'i:insertText:abX\u{1F600}cd',
            'b:insertText:Y', 'i:insertText:abXYcd',
            'b:deleteContentBackward:null', 'i:deleteContentBackward:abXcd',
            'b:deleteContentForward:null', 'i:deleteContentForward:abXd',
            'b:deleteContentForward:null', 'i:deleteContentForward:bXd',
            'b:insertText:Z', 'i:insertText:bXdZ'];
         if (JSON.stringify(log) !== JSON.stringify(want)) throw JSON.stringify(log);",
    );
}

#[test]
fn textarea_lines_readonly_and_canceled_input() {
    let mut engine = Engine::new(1);
    let mut page = page(
        &mut engine,
        "<!doctype html><textarea id=a>one\ntwo</textarea><input id=r readonly value=fixed>",
    );
    let (a, r) = (id(&page, "a"), id(&page, "r"));
    press(&mut page, a, "Home", false);
    press(&mut page, a, "Enter", false);
    run(
        &mut page,
        "var a = document.getElementById('a');
         if (a.value !== 'one\\n\\ntwo' || a.selectionStart !== 5) throw JSON.stringify(a.value) + a.selectionStart;
         a.addEventListener('beforeinput', e => e.preventDefault());",
    );
    press(&mut page, a, "q", false);
    // Read-only fields move the caret but do not change.
    press(&mut page, r, "ArrowLeft", false);
    press(&mut page, r, "x", false);
    run(
        &mut page,
        "if (a.value !== 'one\\n\\ntwo') throw 'canceled';
         var r = document.getElementById('r');
         if (r.value !== 'fixed' || r.selectionStart !== 4) throw r.value + r.selectionStart;",
    );
}

#[test]
fn the_html_selection_api() {
    let mut engine = Engine::new(1);
    let mut page = page(
        &mut engine,
        "<!doctype html><input id=t value=hello><input id=n type=number><input id=c type=checkbox>",
    );
    run(
        &mut page,
        r#"
        const t = document.getElementById('t');
        t.setSelectionRange(1, 3, 'backward');
        if (t.selectionStart !== 1 || t.selectionEnd !== 3 || t.selectionDirection !== 'backward') throw 'range';
        t.setSelectionRange(4, 2);
        if (t.selectionStart !== 2 || t.selectionEnd !== 2) throw 'collapsed to end ' + t.selectionStart;
        t.setSelectionRange(0, 99);
        if (t.selectionEnd !== 5) throw 'clamped';
        t.selectionStart = 4;
        t.selectionEnd = 1;
        if (t.selectionStart !== 1 || t.selectionEnd !== 1) throw 'start follows end';
        t.select();
        if (t.selectionStart !== 0 || t.selectionEnd !== 5) throw 'select';
        t.setSelectionRange(1, 4);
        t.setRangeText('EL', 1, 3);
        if (t.value !== 'hELlo' || t.selectionStart !== 1 || t.selectionEnd !== 4) throw t.value + ' ' + t.selectionStart + ' ' + t.selectionEnd;
        t.setRangeText('__', 0, 0, 'select');
        if (t.value !== '__hELlo' || t.selectionStart !== 0 || t.selectionEnd !== 2) throw 'select mode';
        t.setRangeText('!', 7, 7, 'end');
        if (t.value !== '__hELlo!' || t.selectionStart !== 8) throw 'end mode';
        t.setSelectionRange(2, 3);
        t.setRangeText('h');
        if (t.value !== '__hELlo!') throw 'replaces the selection';
        let threw = false;
        try { t.setRangeText('x', 3, 1); } catch (e) { threw = e.name === 'IndexSizeError'; }
        if (!threw) throw 'IndexSizeError';
        t.value = 'reset';
        if (t.selectionStart !== 5) throw 'value setter moves the caret to the end';
        const n = document.getElementById('n'), c = document.getElementById('c');
        if (n.selectionStart !== null || c.selectionDirection !== null) throw 'null for other types';
        for (const f of [() => n.setSelectionRange(0, 0), () => { c.selectionStart = 0; }, () => n.setRangeText('x')]) {
            let ok = false;
            try { f(); } catch (e) { ok = e.name === 'InvalidStateError'; }
            if (!ok) throw 'InvalidStateError';
        }
        n.select();
        "#,
    );
}

#[test]
fn ime_composition_events_and_text() {
    let mut engine = Engine::new(1);
    let mut page = page(&mut engine, "<!doctype html><input id=t value=ab>");
    let t = id(&page, "t");
    run(
        &mut page,
        "var t = document.getElementById('t'); var log = [];
         for (const type of ['compositionstart', 'compositionupdate', 'compositionend'])
             t.addEventListener(type, e => log.push(type + ':' + e.data + ':' + (e instanceof CompositionEvent)));
         t.addEventListener('beforeinput', e => log.push('b:' + e.inputType + ':' + e.data + ':' + e.isComposing + ':' + e.cancelable));
         t.addEventListener('input', e => log.push('i:' + t.value + ':' + e.isComposing));
         t.setSelectionRange(1, 1);",
    );
    deliver(&mut page, t, InputEvent::Composition(Composition::Start));
    deliver(
        &mut page,
        t,
        InputEvent::Composition(Composition::Update("k".into())),
    );
    deliver(
        &mut page,
        t,
        InputEvent::Composition(Composition::Update("か".into())),
    );
    deliver(
        &mut page,
        t,
        InputEvent::Composition(Composition::End("漢".into())),
    );
    run(
        &mut page,
        "const want = ['compositionstart::true',
            'compositionupdate:k:true', 'b:insertCompositionText:k:true:false', 'i:akb:true',
            'compositionupdate:か:true', 'b:insertCompositionText:か:true:false', 'i:aかb:true',
            'compositionupdate:漢:true', 'b:insertCompositionText:漢:true:false', 'i:a漢b:true',
            'compositionend:漢:true'];
         if (JSON.stringify(log) !== JSON.stringify(want)) throw JSON.stringify(log);
         if (t.selectionStart !== 2) throw 'caret after the commit';
         const e = new CompositionEvent('compositionupdate', {data: 'x', bubbles: true});
         if (e.data !== 'x' || !e.bubbles || new CompositionEvent('x').data !== '') throw 'constructor';",
    );
    // A commit without an update still starts and ends a composition.
    run(&mut page, "log = [];");
    deliver(
        &mut page,
        t,
        InputEvent::Composition(Composition::End("!".into())),
    );
    run(
        &mut page,
        "if (t.value !== 'a漢!b' || log[0] !== 'compositionstart::true' || log[log.length - 1] !== 'compositionend:!:true') throw JSON.stringify(log);",
    );
}

#[test]
fn assistive_actions_focus_click_set_value_and_select() {
    let mut engine = Engine::new(1);
    let mut page = page(
        &mut engine,
        "<!doctype html><input id=t maxlength=5><input id=c type=checkbox>\
         <button id=b disabled>no</button><a id=l href='#x'>link</a>",
    );
    let (t, c, b, l) = (
        id(&page, "t"),
        id(&page, "c"),
        id(&page, "b"),
        id(&page, "l"),
    );
    run(
        &mut page,
        "var log = [];
         for (const id of ['t', 'c', 'b', 'l'])
             for (const type of ['focus', 'click', 'beforeinput', 'input'])
                 document.getElementById(id).addEventListener(type, e => log.push(id + ':' + type + ':' + e.isTrusted + (e.inputType ? ':' + e.inputType : '')));",
    );
    deliver(&mut page, t, InputEvent::Assistive(AssistiveAction::Focus));
    deliver(
        &mut page,
        t,
        InputEvent::Assistive(AssistiveAction::SetValue("hello world".into())),
    );
    deliver(
        &mut page,
        t,
        InputEvent::Assistive(AssistiveAction::SetSelection {
            anchor: 4,
            focus: 1,
        }),
    );
    deliver(&mut page, c, InputEvent::Assistive(AssistiveAction::Click));
    deliver(&mut page, b, InputEvent::Assistive(AssistiveAction::Click));
    deliver(&mut page, l, InputEvent::Assistive(AssistiveAction::Focus));
    run(
        &mut page,
        "const t = document.getElementById('t');
         if (t.value !== 'hello') throw 'maxlength: ' + t.value;
         if (t.selectionStart !== 1 || t.selectionEnd !== 4 || t.selectionDirection !== 'backward') throw 'selection';
         if (!document.getElementById('c').checked) throw 'click toggles';
         if (document.activeElement.id !== 'l') throw 'focus';
         const want = ['t:focus:true', 't:beforeinput:true:insertReplacementText',
            't:input:true:insertReplacementText', 'c:click:true', 'c:input:true', 'l:focus:true'];
         if (JSON.stringify(log) !== JSON.stringify(want)) throw JSON.stringify(log);",
    );
    // The accessibility tree reads the same state.
    let live = page.accessibility_state();
    assert_eq!(live.focused, Some(l));
    let field = &live.controls[&t];
    assert_eq!(field.value.as_deref(), Some("hello"));
    assert_eq!(field.selection, Some((4, 1)));
    assert_eq!(live.controls[&c].checked, Some(Checked::True));
    run(
        &mut page,
        "document.getElementById('c').indeterminate = true;",
    );
    assert_eq!(
        page.accessibility_state().controls[&c].checked,
        Some(Checked::Mixed)
    );
}
