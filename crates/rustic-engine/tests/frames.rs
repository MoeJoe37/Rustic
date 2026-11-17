use std::time::Duration;

use rustic_base::ManualClock;
use rustic_engine::{
    DocumentTurn, Engine, EngineDocument, RenderFrame, ScriptStop, ScriptTurn, ScriptedDocument,
    TaskLimits,
};
use rustic_html::ParserLimits;
use rustic_js::Source;

fn document(engine: &mut Engine, limits: TaskLimits) -> ScriptedDocument<ManualClock> {
    engine
        .begin_document(ParserLimits::default())
        .unwrap()
        .finish()
        .unwrap()
        .into_scripted_with_clock(limits, ManualClock::default())
        .unwrap()
}

fn source(text: &str) -> Source {
    Source::from_str_limited(text, 4096).unwrap()
}

fn unexpected_render(_: &EngineDocument, _: RenderFrame) {
    panic!("unexpected rendering turn");
}

fn render_turn(scripts: &mut ScriptedDocument<ManualClock>) -> RenderFrame {
    match scripts
        .run_next_with_render(|_, frame| frame.timestamp())
        .unwrap()
    {
        Some(DocumentTurn::Rendered { frame, output }) => {
            assert_eq!(output, frame.timestamp());
            frame
        }
        other => panic!("expected a rendering turn, got {other:?}"),
    }
}

#[test]
fn rendering_follows_script_checkpoints_and_alternates_with_ready_work() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    scripts
        .queue_script(source(
            "var trace = ''; Promise.resolve().then(() => trace += 'a');",
        ))
        .unwrap();
    scripts
        .queue_script(source("if (trace !== 'ab') throw 'timer order';"))
        .unwrap();
    scripts
        .schedule_script(
            Duration::ZERO,
            source(
                "if (trace !== 'a') throw 'checkpoint order';
                 Promise.resolve().then(() => trace += 'b');",
            ),
        )
        .unwrap();

    scripts.clock().advance(Duration::from_millis(3)).unwrap();
    scripts.offer_rendering_opportunity().unwrap();
    scripts.clock().advance(Duration::from_millis(4)).unwrap();
    scripts.offer_rendering_opportunity().unwrap();
    scripts.clock().advance(Duration::from_millis(1)).unwrap();

    assert_eq!(
        scripts.run_next_with_render(unexpected_render),
        Ok(Some(DocumentTurn::Script(ScriptTurn::Completed)))
    );
    let first = render_turn(&mut scripts);
    assert_eq!(first.sequence(), 1);
    assert_eq!(first.timestamp(), Duration::from_millis(7));

    scripts.offer_rendering_opportunity().unwrap();
    assert_eq!(
        scripts.run_next_with_render(unexpected_render),
        Ok(Some(DocumentTurn::Script(ScriptTurn::Completed)))
    );
    let second = render_turn(&mut scripts);
    assert_eq!(second.sequence(), 2);
    assert_eq!(second.timestamp(), Duration::from_millis(8));
    assert_eq!(scripts.accept_rendered_frame(first), Ok(false));
    assert_eq!(scripts.accept_rendered_frame(second), Ok(true));
    assert_eq!(scripts.accept_rendered_frame(second), Ok(false));
    assert_eq!(
        scripts.run_next_with_render(unexpected_render),
        Ok(Some(DocumentTurn::Script(ScriptTurn::Completed)))
    );
    assert_eq!(scripts.run_next_with_render(unexpected_render), Ok(None));
}

#[test]
fn frames_are_document_scoped_and_script_only_pumping_keeps_offers_pending() {
    let mut engine = Engine::new(2);
    let mut first = document(&mut engine, TaskLimits::default());
    let mut second = document(&mut engine, TaskLimits::default());
    first.offer_rendering_opportunity().unwrap();
    second.offer_rendering_opportunity().unwrap();

    assert_eq!(first.run_next(), Ok(None));
    let first_frame = render_turn(&mut first);
    let second_frame = render_turn(&mut second);
    assert_eq!(first_frame.sequence(), second_frame.sequence());
    assert_eq!(first_frame.timestamp(), second_frame.timestamp());
    assert_eq!(second.accept_rendered_frame(first_frame), Ok(false));
    assert_eq!(second.accept_rendered_frame(second_frame), Ok(true));
    assert_eq!(first.accept_rendered_frame(first_frame), Ok(true));
}

#[test]
fn a_failed_checkpoint_discards_the_rendering_offer() {
    let mut engine = Engine::new(1);
    let mut scripts = document(
        &mut engine,
        TaskLimits {
            jobs_per_checkpoint: 0,
            ..TaskLimits::default()
        },
    );
    scripts
        .queue_script(source("Promise.resolve().then(() => 0)"))
        .unwrap();
    scripts.offer_rendering_opportunity().unwrap();

    assert_eq!(
        scripts.run_next_with_render(unexpected_render),
        Err(ScriptStop::JobLimit)
    );
    assert_eq!(
        scripts.run_next_with_render(unexpected_render),
        Err(ScriptStop::JobLimit)
    );
}

#[test]
fn shutdown_during_rendering_discards_the_output_and_ticket() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    scripts.offer_rendering_opportunity().unwrap();
    let mut issued = None;
    let result = scripts.run_next_with_render(|_, frame| {
        issued = Some(frame);
        engine.shutdown();
        "rendered"
    });

    assert_eq!(result, Err(ScriptStop::DocumentCancelled));
    assert_eq!(
        scripts.accept_rendered_frame(issued.unwrap()),
        Err(ScriptStop::DocumentCancelled)
    );
    assert_eq!(
        scripts.offer_rendering_opportunity(),
        Err(ScriptStop::DocumentCancelled)
    );
}

#[test]
fn animation_callbacks_run_before_render_with_live_cancellation_and_per_callback_jobs() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    scripts
        .queue_script(source(
            r#"
        const body = document.querySelector("body");
        let trace = "";
        let cancelled;
        requestAnimationFrame(function (timestamp) {
            "use strict";
            if (timestamp !== 7.5 || this !== undefined) throw "frame arguments";
            trace += "a";
            Promise.resolve().then(() => {
                trace += "b";
                cancelAnimationFrame(cancelled);
                requestAnimationFrame(next => {
                    if (next !== 12) throw "next timestamp";
                    body.firstChild.data = trace + "e";
                });
            });
        });
        cancelled = requestAnimationFrame(() => trace += "x");
        requestAnimationFrame(timestamp => {
            if (timestamp !== 7.5 || trace !== "ab") throw "callback order";
            trace += "c";
            throw "uncaught callback";
        });
        requestAnimationFrame(() => {
            trace += "d";
            body.appendChild(document.createTextNode(trace));
        });
    "#,
        ))
        .unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(scripts.run_next(), Ok(None));
    scripts
        .clock()
        .advance(Duration::from_micros(7500))
        .unwrap();
    scripts.offer_rendering_opportunity().unwrap();
    for (expected, clock_step) in [
        ("abcd", Duration::ZERO),
        ("abcde", Duration::from_micros(4500)),
    ] {
        if clock_step != Duration::ZERO {
            scripts.clock().advance(clock_step).unwrap();
            scripts.offer_rendering_opportunity().unwrap();
        }
        let Some(DocumentTurn::Rendered { frame, output }) = scripts
            .run_next_with_render(|document, _| {
                let tree = document.parsed().document();
                tree.dump_tree(tree.root(), 4096).unwrap()
            })
            .unwrap()
        else {
            panic!("expected rendering");
        };
        assert!(output.contains(&format!("\"{expected}\"")), "{output}");
        assert_eq!(scripts.accept_rendered_frame(frame), Ok(true));
    }
    assert_eq!(scripts.take_animation_error_count(), 1);
    assert_eq!(scripts.take_animation_error_count(), 0);
}

#[test]
fn a_failed_animation_checkpoint_prevents_rendering_and_discards_callbacks() {
    let mut engine = Engine::new(1);
    let mut scripts = document(
        &mut engine,
        TaskLimits {
            jobs_per_checkpoint: 0,
            ..TaskLimits::default()
        },
    );
    scripts
        .queue_script(source(
            r#"
        requestAnimationFrame(() => { Promise.resolve().then(() => 0); });
        requestAnimationFrame(() => { throw "after the failed callback"; });
    "#,
        ))
        .unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    scripts.offer_rendering_opportunity().unwrap();
    assert_eq!(
        scripts.run_next_with_render(unexpected_render),
        Err(ScriptStop::JobLimit)
    );
    assert_eq!(scripts.take_animation_error_count(), 0);
    assert_eq!(
        scripts.offer_rendering_opportunity(),
        Err(ScriptStop::JobLimit)
    );
}
