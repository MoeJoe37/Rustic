//! Script, timer and syntax errors and unhandled rejections reach the page
//! and the host console in document turns.

use std::time::Duration;

use rustic_base::ManualClock;
use rustic_engine::{
    ConsoleLevel, ConsoleMessage, Engine, ScriptTurn, ScriptedDocument, TaskLimits,
};
use rustic_html::ParserLimits;
use rustic_js::Source;

fn source(text: &str) -> Source {
    Source::from_str_limited(text, 4096).unwrap()
}

fn document(engine: &mut Engine) -> ScriptedDocument<ManualClock> {
    engine
        .begin_document(ParserLimits::default())
        .unwrap()
        .finish()
        .unwrap()
        .into_scripted_with_clock(TaskLimits::default(), ManualClock::default())
        .unwrap()
}

fn run(document: &mut ScriptedDocument<ManualClock>, script: &str) -> ScriptTurn {
    document.queue_script(source(script)).unwrap();
    document.run_next().unwrap().unwrap()
}

fn texts(document: &ScriptedDocument<ManualClock>) -> Vec<(ConsoleLevel, String)> {
    let log = document.take_console_messages();
    assert_eq!(log.dropped, 0);
    log.messages
        .into_iter()
        .map(|ConsoleMessage { level, text, .. }| (level, text))
        .collect()
}

#[test]
fn uncaught_errors_and_rejections_are_reported_in_their_turns() {
    let mut engine = Engine::new(1);
    let mut document = document(&mut engine);
    assert_eq!(
        run(
            &mut document,
            r#"
            var seen = [];
            addEventListener('error', (event) => seen.push('error ' + event.message));
            onunhandledrejection = (event) => seen.push('rejection ' + event.reason);
            console.log('ready');
            "#,
        ),
        ScriptTurn::Completed
    );
    // A thrown script error is reported before the turn's checkpoint.
    assert!(matches!(
        run(
            &mut document,
            "Promise.resolve().then(() => seen.push('job')); throw new Error('script');"
        ),
        ScriptTurn::Failed(_)
    ));
    // A parse failure is reported as a SyntaxError.
    assert!(matches!(
        run(&mut document, "let = ;"),
        ScriptTurn::Failed(_)
    ));
    // A timer callback's exception and a rejection from a job.
    assert_eq!(
        run(
            &mut document,
            "setTimeout(() => { throw new RangeError('timer'); }, 5);
             Promise.resolve().then(() => Promise.reject('from job'));"
        ),
        ScriptTurn::Completed
    );
    // The checkpoint queued the rejection notification as a task; the due
    // timer runs first because the task pump alternates sources.
    document.clock().advance(Duration::from_millis(5)).unwrap();
    assert!(matches!(
        document.run_next(),
        Ok(Some(ScriptTurn::Failed(_)))
    ));
    assert_eq!(document.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(
        run(
            &mut document,
            r#"
            const got = seen.join('|');
            if (!got.startsWith('error Uncaught Error: script|job|error Uncaught SyntaxError')
                || !got.endsWith('error Uncaught RangeError: timer|rejection from job')) throw got;
            "#,
        ),
        ScriptTurn::Completed
    );
    let logged = texts(&document);
    assert_eq!(logged[0], (ConsoleLevel::Log, "ready".to_owned()));
    assert_eq!(
        logged[1],
        (ConsoleLevel::Error, "Uncaught Error: script".to_owned())
    );
    assert!(logged[2].1.starts_with("Uncaught SyntaxError"));
    assert_eq!(
        &logged[3..],
        [
            (ConsoleLevel::Error, "Uncaught RangeError: timer".to_owned()),
            (
                ConsoleLevel::Error,
                "Uncaught (in promise) from job".to_owned()
            ),
        ]
    );
}
