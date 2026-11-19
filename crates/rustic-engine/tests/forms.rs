//! Focus, text editing, constraint validation and submission without
//! navigation, through script and trusted host input.

use rustic_dom::{NodeId, NodeKind};
use rustic_engine::{
    Engine, InputEvent, KeyInput, Modifiers, ScriptTurn, ScriptedDocument, TaskLimits,
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

fn key(name: &str, shift: bool) -> KeyInput {
    KeyInput {
        key: name.into(),
        code: name.into(),
        repeat: false,
        location: 0,
        modifiers: Modifiers {
            shift,
            ..Modifiers::default()
        },
    }
}

fn press(page: &mut ScriptedDocument, target: NodeId, name: &str) {
    deliver(page, target, InputEvent::KeyDown(key(name, false)));
    deliver(page, target, InputEvent::KeyUp(key(name, false)));
}

fn click() -> InputEvent {
    InputEvent::Click {
        client_x: 1,
        client_y: 1,
        modifiers: Modifiers::default(),
    }
}

const FORM: &str = r##"<!doctype html><body>
<form id="form" action="/send" method="post">
  <input id="name" name="name" required minlength="3">
  <input id="mail" name="mail" type="email">
  <input id="agree" name="agree" type="checkbox" required>
  <textarea id="note" name="note"></textarea>
  <button id="send">Send</button>
</form>
<p id="plain">text</p>
<a id="link" href="#x">link</a>
<div id="last" tabindex="0"></div>
"##;

#[test]
fn tab_moves_focus_in_tree_order_with_focus_events() {
    let mut engine = Engine::new(1);
    let mut page = page(&mut engine, FORM);
    run(
        &mut page,
        r#"
        var log = [];
        for (const type of ['focus', 'blur', 'focusin', 'focusout'])
            document.addEventListener(type, (event) => log.push(type + ':' + event.target.id
                + '<' + (event.relatedTarget ? event.relatedTarget.id : '') + '>'), true);
        if (document.activeElement !== document.body || !document.hasFocus()) throw 'initial';
        "#,
    );
    let body = id(&page, "plain");
    for _ in 0..3 {
        deliver(&mut page, body, InputEvent::KeyDown(key("Tab", false)));
    }
    run(
        &mut page,
        r#"
        if (document.activeElement.id !== 'agree') throw document.activeElement.id;
        const expected = ['focus:name<>', 'focusin:name<>',
            'blur:name<mail>', 'focusout:name<mail>', 'focus:mail<name>', 'focusin:mail<name>',
            'blur:mail<agree>', 'focusout:mail<agree>', 'focus:agree<mail>', 'focusin:agree<mail>'];
        if (log.join() !== expected.join()) throw log.join();
        log.length = 0;
        "#,
    );
    // Shift+Tab goes back; Tab from the last area wraps to the first.
    deliver(&mut page, body, InputEvent::KeyDown(key("Tab", true)));
    run(
        &mut page,
        "if (document.activeElement.id !== 'mail') throw document.activeElement.id;",
    );
    run(&mut page, "document.getElementById('last').focus();");
    deliver(&mut page, body, InputEvent::KeyDown(key("Tab", false)));
    run(
        &mut page,
        r#"
        if (document.activeElement.id !== 'name') throw document.activeElement.id;
        // A canceled Tab keeps focus.
        document.addEventListener('keydown', (event) => event.preventDefault(), { once: true });
        "#,
    );
    deliver(&mut page, body, InputEvent::KeyDown(key("Tab", false)));
    run(
        &mut page,
        "if (document.activeElement.id !== 'name') throw 'canceled Tab moved focus';",
    );
}

#[test]
fn clicks_focus_the_nearest_focusable_area_or_the_viewport() {
    let mut engine = Engine::new(1);
    let mut page = page(&mut engine, FORM);
    let link = id(&page, "link");
    let plain = id(&page, "plain");
    deliver(&mut page, link, click());
    run(
        &mut page,
        "if (document.activeElement.id !== 'link') throw document.activeElement.id;",
    );
    deliver(&mut page, plain, click());
    run(
        &mut page,
        r#"
        if (document.activeElement !== document.body) throw 'viewport';
        const name = document.getElementById('name');
        name.disabled = true;
        name.focus();
        if (document.activeElement === name) throw 'disabled control focused';
        name.disabled = false;
        name.focus();
        if (document.activeElement !== name) throw 'focus()';
        name.blur();
        if (document.activeElement !== document.body) throw 'blur()';
        if (document.getElementById('plain').tabIndex !== -1 || name.tabIndex !== 0) throw 'tabIndex';
        "#,
    );
}

#[test]
fn typing_edits_values_with_input_events_and_change_on_blur() {
    let mut engine = Engine::new(1);
    let mut page = page(&mut engine, FORM);
    run(
        &mut page,
        r#"
        var log = [];
        const name = document.getElementById('name');
        for (const type of ['beforeinput', 'input', 'change'])
            name.addEventListener(type, (event) => log.push(type + ':' + (event.inputType ?? '')
                + ':' + (event.data ?? '') + ':' + name.value));
        name.addEventListener('beforeinput', (event) => {
            if (event.data === 'x') event.preventDefault();
        });
        name.focus();
        "#,
    );
    let name = id(&page, "name");
    for letter in ["a", "x", "b", "c", "Backspace", "d"] {
        press(&mut page, name, letter);
    }
    run(
        &mut page,
        r#"
        const typed = document.getElementById('name');
        if (typed.value !== 'abd' || typed.defaultValue !== '') throw typed.value;
        if (!log.includes('input:insertText:a:a') || log.includes('input:insertText:x:ax')
            || !log.includes('input:deleteContentBackward::ab')) throw log.join();
        typed.blur();
        if (log[log.length - 1] !== 'change::undefined:abd'.replace('undefined', '')) throw log.join();
        "#,
    );
    // Enter in a textarea inserts a line break; a readonly field ignores keys.
    run(&mut page, "document.getElementById('note').focus();");
    let note = id(&page, "note");
    for letter in ["h", "Enter", "i"] {
        press(&mut page, note, letter);
    }
    run(
        &mut page,
        r#"
        const note = document.getElementById('note');
        if (note.value !== 'h\ni' || note.textLength !== 3) throw JSON.stringify(note.value);
        note.readOnly = true;
        "#,
    );
    press(&mut page, note, "z");
    run(
        &mut page,
        "if (document.getElementById('note').value !== 'h\\ni') throw 'readonly edited';",
    );
}

#[test]
fn enter_submits_after_validation_and_the_host_receives_the_entries() {
    let mut engine = Engine::new(1);
    let mut page = page(&mut engine, FORM);
    run(
        &mut page,
        r#"
        var log = [];
        const form = document.getElementById('form');
        form.addEventListener('submit', (event) => log.push('submit:' + event.submitter.id));
        form.addEventListener('invalid', (event) => log.push('invalid:' + event.target.id), true);
        document.getElementById('name').focus();
        "#,
    );
    let name = id(&page, "name");
    for letter in ["a", "b"] {
        press(&mut page, name, letter);
    }
    // Too short and the required checkbox is unchecked: no submission.
    press(&mut page, name, "Enter");
    assert!(page.take_form_submissions().submissions.is_empty());
    run(
        &mut page,
        r#"
        if (log.join() !== 'invalid:name,invalid:agree') throw log.join();
        const name = document.getElementById('name');
        if (!name.validity.tooShort || name.validity.valid || name.checkValidity()) throw 'tooShort';
        log.length = 0;
        document.getElementById('agree').checked = true;
        document.getElementById('mail').value = 'person@example.org';
        document.getElementById('note').value = 'hello';
        "#,
    );
    press(&mut page, name, "c");
    press(&mut page, name, "Enter");
    run(
        &mut page,
        "if (log.join() !== 'submit:send') throw log.join();",
    );
    let submissions = page.take_form_submissions();
    assert_eq!(submissions.dropped, 0);
    let [submission] = &submissions.submissions[..] else {
        panic!("one submission: {submissions:?}");
    };
    assert_eq!(submission.form, id(&page, "form"));
    assert_eq!(submission.submitter, Some(id(&page, "send")));
    assert_eq!(submission.method, "post");
    assert_eq!(submission.action, "/send");
    let entries: Vec<_> = submission
        .entries
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        entries,
        [
            ("name", "abc"),
            ("mail", "person@example.org"),
            ("agree", "on"),
            ("note", "hello"),
        ]
    );
}

#[test]
fn script_apis_follow_value_owner_and_validity_rules() {
    let mut engine = Engine::new(1);
    let mut page = page(
        &mut engine,
        r##"<!doctype html><body>
<table><form id="table-form"><tr><td><input id="misnested" name="m" value="kept"></td></tr></form></table>
<form id="other"></form>
<input id="outside" form="other" name="o">
<form id="f" novalidate>
  <input id="num" type="NUMBER" min="2" max="5">
  <input id="url" type="url">
  <input id="r1" type="radio" name="g" required><input id="r2" type="radio" name="g">
  <fieldset disabled><input id="off" name="off" value="x"></fieldset>
  <button id="reset" type="reset">Reset</button>
  <button id="go">Go</button>
</form>
"##,
    );
    run(
        &mut page,
        r#"
        const $ = (id) => document.getElementById(id);
        // The parser's form element pointer owns a control outside the form.
        if ($('misnested').form !== $('table-form') || $('table-form').elements.length !== 1) throw 'TD-31';
        if ($('outside').form !== $('other') || $('other').elements[0] !== $('outside')) throw 'form attribute';
        const elements = $('f').elements;
        const count = elements.length;
        $('f').appendChild(document.createElement('textarea'));
        if (elements.length !== count + 1 || $('f').elements !== elements) throw 'live elements';

        const num = $('num');
        if (num.type !== 'number' || num.value !== '') throw 'type';
        num.value = 'abc';
        if (num.value !== '') throw 'number sanitization';
        num.value = '1';
        if (!num.validity.rangeUnderflow || num.validity.valid) throw 'underflow';
        num.value = '9';
        if (!num.validity.rangeOverflow || num.validationMessage === '') throw 'overflow';
        num.value = '3';
        if (!num.validity.valid || num.validationMessage !== '') throw 'in range';
        num.setCustomValidity('custom');
        if (!num.validity.customError || num.validationMessage !== 'custom' || num.checkValidity()) throw 'custom';
        num.setCustomValidity('');

        const url = $('url');
        url.value = '  not a url ';
        if (url.value !== 'not a url' || !url.validity.typeMismatch) throw 'url';
        url.value = 'https://example.org/';
        if (url.validity.typeMismatch) throw 'valid url';

        if (!$('r2').validity.valid || !$('r1').validity.valueMissing) throw 'radio group';
        $('r2').checked = true;
        if (!$('r1').validity.valid) throw 'radio group checked';
        if ($('off').willValidate || $('reset').willValidate || !$('go').willValidate) throw 'willValidate';

        // novalidate: submission skips validation; submit() fires no event.
        let events = [];
        $('f').addEventListener('submit', (event) => events.push('submit'));
        $('f').addEventListener('reset', () => events.push('reset'));
        num.value = '100';
        $('go').click();
        $('f').submit();
        try { $('f').requestSubmit($('reset')); throw 'requestSubmit type'; }
        catch (error) { if (!(error instanceof TypeError)) throw error; }
        try { $('f').requestSubmit(document.createElement('button')); throw 'requestSubmit owner'; }
        catch (error) { if (error.name !== 'NotFoundError') throw error; }

        // Reset restores defaults and the checkedness of radios.
        $('reset').click();
        if (num.value !== '' || $('r2').checked || events.join() !== 'submit,reset') throw events.join();

        const focus = new FocusEvent('focus', { relatedTarget: num });
        const input = new InputEvent('input', { data: 'a', inputType: 'insertText' });
        const submit = new SubmitEvent('submit', { submitter: $('go'), cancelable: true });
        if (focus.relatedTarget !== num || !(focus instanceof UIEvent) || input.data !== 'a' ||
            input.inputType !== 'insertText' || input.isComposing || submit.submitter !== $('go') ||
            !submit.cancelable) throw 'constructors';
        if (new InputEvent('x').data !== null || new FocusEvent('x').relatedTarget !== null) throw 'defaults';
        "#,
    );
    let submissions = page.take_form_submissions().submissions;
    assert_eq!(submissions.len(), 2);
    let entry = |index: usize| -> Vec<(String, String)> { submissions[index].entries.clone() };
    // The button click carries its submitter; `submit()` has none.
    assert_eq!(submissions[0].submitter, Some(id(&page, "go")));
    assert_eq!(submissions[1].submitter, None);
    assert_eq!(entry(0), [("g".to_owned(), "on".to_owned())]);
    assert_eq!(submissions[0].method, "get");
}
