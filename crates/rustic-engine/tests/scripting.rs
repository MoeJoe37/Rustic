use rustic_engine::{Engine, ScriptFailure, ScriptQueueError, ScriptStop, ScriptTurn, TaskLimits};
use rustic_html::ParserLimits;
use rustic_js::Source;

fn source(text: &str) -> Source {
    Source::from_str_limited(text, 4096).unwrap()
}

#[test]
fn jobs_queued_by_scripts_and_jobs_run_before_the_next_script() {
    let mut engine = Engine::new(1);
    let document = engine.begin_document(ParserLimits::default()).unwrap();
    let mut scripts = document
        .finish()
        .unwrap()
        .into_scripted(TaskLimits {
            jobs_per_checkpoint: 2,
            ..TaskLimits::default()
        })
        .unwrap();
    for text in [
        r#"var trace = "a"; Promise.resolve().then(() => {
            trace += "b"; Promise.resolve().then(() => trace += "c");
        });"#,
        r#"if (trace !== "abc") throw "job order"; trace += "d";"#,
        r#"Promise.resolve().then(() => trace += "e"); throw "script error";"#,
        r#"if (trace !== "abcde") throw "checkpoint after exception";"#,
    ] {
        scripts.queue_script(source(text)).unwrap();
    }

    for expected in [
        ScriptTurn::Completed,
        ScriptTurn::Completed,
        ScriptTurn::Failed(ScriptFailure::UncaughtException),
        ScriptTurn::Completed,
    ] {
        assert_eq!(scripts.run_next(), Ok(Some(expected)));
    }
    assert_eq!(scripts.run_next(), Ok(None));
}

#[test]
fn rejected_admission_leaves_the_pending_queue_and_budget_intact() {
    let mut engine = Engine::new(1);
    let mut scripts = engine
        .begin_document(ParserLimits::default())
        .unwrap()
        .finish()
        .unwrap()
        .into_scripted(TaskLimits {
            pending_scripts: 2,
            pending_source_units: 2,
            ..TaskLimits::default()
        })
        .unwrap();

    scripts.queue_script(source(";;")).unwrap();
    assert_eq!(
        scripts.queue_script(source(";")),
        Err(ScriptQueueError::Limit)
    );
    scripts.queue_script(source("")).unwrap();
    assert_eq!(
        scripts.queue_script(source("")),
        Err(ScriptQueueError::Limit)
    );
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    scripts.queue_script(source(";;")).unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(scripts.run_next(), Ok(None));
}

#[test]
fn a_microtask_chain_cannot_run_past_the_checkpoint_budget() {
    let mut engine = Engine::new(1);
    let mut scripts = engine
        .begin_document(ParserLimits::default())
        .unwrap()
        .finish()
        .unwrap()
        .into_scripted(TaskLimits {
            jobs_per_checkpoint: 2,
            ..TaskLimits::default()
        })
        .unwrap();

    scripts
        .queue_script(source(
            "function again() { Promise.resolve().then(again); } again();",
        ))
        .unwrap();
    scripts.queue_script(source("throw 'too late';")).unwrap();
    assert_eq!(scripts.run_next(), Err(ScriptStop::JobLimit));
    assert_eq!(scripts.run_next(), Err(ScriptStop::JobLimit));
    assert_eq!(
        scripts.queue_script(source(";")),
        Err(ScriptQueueError::Stopped(ScriptStop::JobLimit))
    );
}

#[test]
fn a_turn_shares_its_step_budget_with_its_microtasks() {
    let mut engine = Engine::new(1);
    let mut scripts = engine
        .begin_document(ParserLimits::default())
        .unwrap()
        .finish()
        .unwrap()
        .into_scripted(TaskLimits {
            steps_per_turn: 100,
            ..TaskLimits::default()
        })
        .unwrap();

    scripts
        .queue_script(source(
            "for (var i = 0; i < 60; i++) {}
             Promise.resolve().then(() => { for (var j = 0; j < 60; j++) {} });",
        ))
        .unwrap();
    assert_eq!(scripts.run_next(), Err(ScriptStop::StepLimit));
    assert_eq!(scripts.run_next(), Err(ScriptStop::StepLimit));
}

#[test]
fn engine_shutdown_cancels_queued_script_turns() {
    let mut engine = Engine::new(1);
    let mut scripts = engine
        .begin_document(ParserLimits::default())
        .unwrap()
        .finish()
        .unwrap()
        .into_scripted(TaskLimits::default())
        .unwrap();

    scripts
        .queue_script(source("throw 'cancelled work';"))
        .unwrap();
    engine.shutdown();
    assert!(scripts.document().cancellation().is_cancelled());
    assert_eq!(scripts.run_next(), Err(ScriptStop::DocumentCancelled));
    assert_eq!(
        scripts.queue_script(source(";")),
        Err(ScriptQueueError::Stopped(ScriptStop::DocumentCancelled))
    );
}

#[test]
fn failed_agent_initialization_releases_document_admission() {
    let mut engine = Engine::new(1);
    let doc = engine
        .begin_document(ParserLimits::default())
        .unwrap()
        .finish()
        .unwrap();
    let token = doc.cancellation().clone();
    assert!(matches!(
        doc.into_scripted(TaskLimits {
            heap_bytes: 0,
            ..TaskLimits::default()
        }),
        Err(ScriptStop::OutOfMemory)
    ));
    assert!(token.is_cancelled());
    assert!(engine.begin_document(ParserLimits::default()).is_ok());
}

#[test]
fn document_script_realms_install_dom_exception() {
    let mut engine = Engine::new(1);
    let mut scripts = engine
        .begin_document(ParserLimits::default())
        .unwrap()
        .finish()
        .unwrap()
        .into_scripted(TaskLimits::default())
        .unwrap();
    scripts
        .queue_script(source(
            r#"
        var failure;
        Promise.resolve().then(() => {
            failure = new DOMException("operation aborted", "AbortError");
        });
    "#,
        ))
        .unwrap();
    scripts
        .queue_script(source(
            r#"
        if (!(failure instanceof DOMException) || !(failure instanceof Error) ||
            failure.message !== "operation aborted" || failure.code !== DOMException.ABORT_ERR)
            throw "DOMException was not installed";
    "#,
        ))
        .unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
}

#[test]
fn document_wrappers_use_the_live_dom_across_script_and_render_turns() {
    let mut engine = Engine::new(1);
    let mut load = engine.begin_document(ParserLimits::default()).unwrap();
    load.feed(b"<!doctype html><p>keep</p><em>drop</em>")
        .unwrap();
    let mut scripts = load
        .finish()
        .unwrap()
        .into_scripted(TaskLimits::default())
        .unwrap();

    scripts.queue_script(source(r#"
        const html = document.documentElement;
        const body = html.firstChild.nextSibling;
        const keep = body.firstChild;
        if (!(document instanceof Document) || !(document instanceof Node) ||
            !(keep instanceof Element) || keep.nodeType !== 1 || document.nodeType !== 9 ||
            keep.localName !== "p" || keep.parentNode !== body || keep.ownerDocument !== document ||
            document.ownerDocument !== null)
            throw "bad interface or traversal";
        let caught = 0;
        try { body.removeChild(html); } catch (error) {
            if (!(error instanceof DOMException) || error.name !== "NotFoundError") throw error;
            caught++;
        }
        try { keep.appendChild(body); } catch (error) {
            if (!(error instanceof DOMException) || error.name !== "HierarchyRequestError") throw error;
            caught++;
        }
        try { Node.prototype.appendChild.call(Object.create(Node.prototype), keep); }
        catch (error) {
            if (!(error instanceof TypeError)) throw error;
            caught++;
        }
        if (caught !== 3 || body.firstChild !== keep) throw "mutation was not atomic";
        (() => {
            const removed = keep.nextSibling;
            if (body.removeChild(removed) !== removed || keep.nextSibling !== null)
                throw "bad removeChild";
        })();
    "#)).unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    scripts.offer_rendering_opportunity().unwrap();
    let rendered = scripts
        .run_next_with_render(|host, _| {
            let tree = host.parsed().document();
            tree.dump_tree(tree.root(), 4096).unwrap()
        })
        .unwrap()
        .unwrap();
    let rustic_engine::DocumentTurn::Rendered { output, .. } = rendered else {
        panic!("expected a rendering turn");
    };
    assert!(output.contains("keep"));
    assert!(!output.contains("drop"));
}
