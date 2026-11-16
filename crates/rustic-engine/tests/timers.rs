use std::time::Duration;

use rustic_base::ManualClock;
use rustic_engine::{
    Engine, ScriptQueueError, ScriptStop, ScriptTurn, ScriptedDocument, TaskLimits,
};
use rustic_html::ParserLimits;
use rustic_js::Source;

fn source(text: &str) -> Source {
    Source::from_str_limited(text, 4096).unwrap()
}

fn document(engine: &mut Engine, limits: TaskLimits) -> ScriptedDocument<ManualClock> {
    engine
        .begin_document(ParserLimits::default())
        .unwrap()
        .finish()
        .unwrap()
        .into_scripted_with_clock(limits, ManualClock::default())
        .unwrap()
}

#[test]
fn timer_turns_follow_deadlines_and_alternate_with_immediate_tasks() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    scripts.queue_script(source("var trace = ''")).unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));

    let last = Duration::from_millis(20);
    let first = Duration::from_millis(10);
    scripts
        .schedule_script(
            last,
            source("if (trace !== 'abcd') throw 'last timer ran out of order'"),
        )
        .unwrap();
    scripts
        .schedule_script(
            first,
            source("trace += 'a'; Promise.resolve().then(() => trace += 'b')"),
        )
        .unwrap();
    scripts
        .schedule_script(
            first,
            source("if (trace !== 'abc') throw 'timer order'; trace += 'd'"),
        )
        .unwrap();

    scripts
        .clock()
        .advance(first - Duration::from_nanos(1))
        .unwrap();
    assert_eq!(scripts.run_next(), Ok(None));
    assert_eq!(
        scripts.time_until_next_timer(),
        Ok(Some(Duration::from_nanos(1)))
    );
    scripts.clock().advance(Duration::from_nanos(1)).unwrap();
    assert_eq!(scripts.time_until_next_timer(), Ok(Some(Duration::ZERO)));

    scripts
        .queue_script(source(
            "if (trace !== 'ab') throw 'checkpoint order'; trace += 'c'",
        ))
        .unwrap();
    scripts
        .queue_script(source("if (trace !== 'abcd') throw 'source selection'"))
        .unwrap();
    for _ in 0..4 {
        assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    }
    assert_eq!(scripts.run_next(), Ok(None));
    assert_eq!(scripts.time_until_next_timer(), Ok(Some(last - first)));
    scripts.clock().advance(last - first).unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(scripts.time_until_next_timer(), Ok(None));
}

#[test]
fn timer_ids_are_document_scoped_and_cancellation_refunds_both_limits() {
    let mut engine = Engine::new(2);
    let limits = TaskLimits {
        pending_scripts: 2,
        pending_source_units: 2,
        ..TaskLimits::default()
    };
    let mut first = document(&mut engine, limits);
    let mut second = document(&mut engine, limits);
    let delay = Duration::from_secs(1);
    let cancelled = first.schedule_script(delay, source(";;")).unwrap();
    let foreign = second.schedule_script(delay, source(";;")).unwrap();

    assert_eq!(first.cancel_timer(foreign), Ok(false));
    assert_eq!(
        first.queue_script(source(";")),
        Err(ScriptQueueError::Limit)
    );
    first.queue_script(source("")).unwrap();
    assert_eq!(
        first.schedule_script(Duration::ZERO, source("")),
        Err(ScriptQueueError::Limit)
    );
    assert_eq!(first.cancel_timer(cancelled), Ok(true));
    assert_eq!(first.cancel_timer(cancelled), Ok(false));

    let replacement = first.schedule_script(Duration::ZERO, source(";;")).unwrap();
    assert_ne!(replacement, cancelled);
    assert_eq!(first.cancel_timer(cancelled), Ok(false));
    assert_eq!(first.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(first.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(first.cancel_timer(replacement), Ok(false));
    assert_eq!(first.time_until_next_timer(), Ok(None));
    assert_eq!(second.time_until_next_timer(), Ok(Some(delay)));
}

#[test]
fn timer_deadline_overflow_preserves_admission() {
    let mut engine = Engine::new(1);
    let mut scripts = document(
        &mut engine,
        TaskLimits {
            pending_scripts: 1,
            ..TaskLimits::default()
        },
    );
    scripts.clock().advance(Duration::MAX).unwrap();
    assert_eq!(
        scripts.schedule_script(Duration::from_nanos(1), source(";")),
        Err(ScriptQueueError::DeadlineOverflow)
    );
    assert_eq!(scripts.time_until_next_timer(), Ok(None));
    scripts
        .schedule_script(Duration::ZERO, source(";"))
        .unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
}

#[test]
fn shutdown_cancels_timers_even_after_they_become_eligible() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    let delay = Duration::from_millis(4);
    let timer = scripts
        .schedule_script(delay, source("throw 'cancelled timer'"))
        .unwrap();
    scripts.clock().advance(delay).unwrap();
    engine.shutdown();
    assert_eq!(scripts.run_next(), Err(ScriptStop::DocumentCancelled));
    assert_eq!(
        scripts.cancel_timer(timer),
        Err(ScriptStop::DocumentCancelled)
    );
    assert_eq!(
        scripts.time_until_next_timer(),
        Err(ScriptStop::DocumentCancelled)
    );
}

#[test]
fn zero_delay_timer_runs_through_the_default_host_pump() {
    let mut engine = Engine::new(1);
    let mut scripts = engine
        .begin_document(ParserLimits::default())
        .unwrap()
        .finish()
        .unwrap()
        .into_scripted(TaskLimits::default())
        .unwrap();
    let timer = scripts
        .schedule_script(Duration::ZERO, source(";"))
        .unwrap();
    assert_eq!(scripts.time_until_next_timer(), Ok(Some(Duration::ZERO)));
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(scripts.cancel_timer(timer), Ok(false));
    assert_eq!(scripts.run_next(), Ok(None));
}
