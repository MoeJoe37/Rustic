//! M5 gate: the WEB-07 surfaces reenter from callbacks and conversions
//! without borrow failures, and shutdown discards their pending work.

use rustic_engine::{Engine, ScriptStop, ScriptTurn, ScriptedDocument, TaskLimits};
use rustic_html::ParserLimits;
use rustic_js::Source;

fn page(engine: &mut Engine) -> ScriptedDocument {
    let mut load = engine.begin_document(ParserLimits::default()).unwrap();
    load.feed(br#"<!doctype html><form id="f"><input id="a" required><input id="b"></form>"#)
        .unwrap();
    load.finish()
        .unwrap()
        .into_scripted(TaskLimits::default())
        .unwrap()
}

fn run(page: &mut ScriptedDocument, text: &str) {
    page.queue_script(Source::from_str_limited(text, 16384).unwrap())
        .unwrap();
    let turn = page.run_next();
    let console = page.take_console_messages();
    assert_eq!(turn, Ok(Some(ScriptTurn::Completed)), "{text}\n{console:?}");
}

#[test]
fn callbacks_and_conversions_reenter_the_new_apis() {
    let mut engine = Engine::new(1);
    let mut page = page(&mut engine);
    run(
        &mut page,
        r#"
        var log = [];
        const $ = (id) => document.getElementById(id);
        // A dictionary getter edits the tree while observe() converts it.
        const observer = new MutationObserver((records) => {
            log.push('records:' + records.length);
            // Reentrant edits from the callback queue another delivery.
            if (log.length === 1) document.body.appendChild(document.createElement('i'));
        });
        observer.observe(document.body, {
            get childList() { document.body.appendChild(document.createElement('b')); return true; },
            subtree: true,
        });
        // The conversion's own edit precedes the registration; this one is seen.
        document.body.appendChild(document.createElement('u'));
        // Focus handlers move focus again while focus is changing.
        $('a').addEventListener('focus', () => $('b').focus());
        $('b').addEventListener('blur', () => log.push('blur b'));
        $('a').focus();
        // An invalid handler rejects again; a submit handler submits again.
        onunhandledrejection = (event) => {
            log.push('rejection:' + event.reason);
            if (event.reason === 1) Promise.reject(2);
        };
        Promise.reject(1);
        $('f').addEventListener('invalid', () => $('f').checkValidity(), { once: true });
        $('f').checkValidity();
        // A ResizeObserver callback that disconnects itself.
        const resize = new ResizeObserver(() => resize.disconnect());
        resize.observe($('a'));
        "#,
    );
    // The rejection notification is a task of its own; run it and its successor.
    assert_eq!(page.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(page.run_next(), Ok(Some(ScriptTurn::Completed)));
    run(
        &mut page,
        r#"
        if (document.activeElement.id !== 'b') throw 'focus ' + document.activeElement.id;
        const expected = ['records:1', 'records:1', 'rejection:1', 'rejection:2'];
        if (log.join() !== expected.join()) throw log.join();
        "#,
    );
    assert_eq!(page.take_listener_error_count(), 0);
}

#[test]
fn shutdown_discards_pending_observer_rejection_and_form_work() {
    let mut engine = Engine::new(1);
    let mut page = page(&mut engine);
    run(
        &mut page,
        r#"
        new MutationObserver(() => { throw 'must not run'; })
            .observe(document.body, { childList: true });
        Promise.reject('pending notification');
        "#,
    );
    // A rejection task is queued; a mutation is queued behind it.
    page.queue_script(
        Source::from_str_limited(
            "document.body.appendChild(document.createElement('p'));",
            256,
        )
        .unwrap(),
    )
    .unwrap();
    engine.shutdown();
    assert_eq!(page.run_next(), Err(ScriptStop::DocumentCancelled));
    assert!(page.take_form_submissions().submissions.is_empty());
    assert!(page.take_console_messages().messages.is_empty());
    assert_eq!(page.take_listener_error_count(), 0);
}
