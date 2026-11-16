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

#[test]
fn document_script_factories_change_the_rendered_tree() {
    let mut engine = Engine::new(1);
    let document = engine
        .begin_document(ParserLimits::default())
        .unwrap()
        .finish()
        .unwrap();
    let mut scripts = document.into_scripted(TaskLimits::default()).unwrap();
    scripts
        .queue_script(source(
            r#"
        const body = document.documentElement.firstChild.nextSibling;
        const element = document.createElement("P");
        element.setAttribute("ID", "created");
        body.appendChild(element);
        element.appendChild(document.createTextNode("from "));
        element.appendChild(new Text("script"));
    "#,
        ))
        .unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    scripts.offer_rendering_opportunity().unwrap();
    let render = scripts
        .run_next_with_render(|document, _| {
            let tree = document.parsed().document();
            let element = tree
                .walk(tree.root())
                .unwrap()
                .find_map(|(id, _)| match tree.node(id).unwrap().kind() {
                    rustic_dom::NodeKind::Element(element) if element.name().local() == "p" => {
                        Some(element)
                    }
                    _ => None,
                })
                .expect("script-created paragraph");
            let attribute = &element.attributes()[0];
            assert_eq!(attribute.name().local(), "id");
            assert_eq!(attribute.value(), &rustic_dom::DomString::from("created"));
            tree.dump_tree(tree.root(), 4096).unwrap()
        })
        .unwrap()
        .unwrap();
    let rustic_engine::DocumentTurn::Rendered { output, .. } = render else {
        panic!("expected rendering after the script");
    };
    assert!(output.contains("from "));
    assert!(output.contains("script"));
}

#[test]
fn document_selectors_follow_parser_quirks_mode() {
    let mut engine = Engine::new(1);
    for (markup, expected) in [
        ("<p id='Heading' class='Active'>", true),
        ("<!doctype html><p id='Heading' class='Active'>", false),
    ] {
        let mut load = engine.begin_document(ParserLimits::default()).unwrap();
        load.feed(markup.as_bytes()).unwrap();
        let mut scripts = load
            .finish()
            .unwrap()
            .into_scripted(TaskLimits::default())
            .unwrap();
        scripts
            .queue_script(source(&format!(
                r#"
            const p = document.getElementById("Heading");
            if (!p || document.getElementById("heading") !== null ||
                p.matches('#heading.active') !== {expected} ||
                (document.querySelector('#heading.active') === p) !== {expected} ||
                (document.getElementsByClassName("active")[0] === p) !== {expected})
                throw "incorrect case sensitivity";
        "#
            )))
            .unwrap();
        assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    }
}

#[test]
fn window_identity_persists_across_document_turns() {
    let mut engine = Engine::new(1);
    let mut load = engine.begin_document(ParserLimits::default()).unwrap();
    load.feed(b"<!doctype html><main>original</main>").unwrap();
    let mut scripts = load
        .finish()
        .unwrap()
        .into_scripted(TaskLimits::default())
        .unwrap();
    scripts
        .queue_script(source(
            r#"
        const firstView = document.defaultView;
        if (firstView !== this || firstView !== window) throw "initial Window";
        Promise.resolve().then(function () {
            if (this !== firstView) throw "promise receiver";
            this.readyForNextTurn = true;
        });
    "#,
        ))
        .unwrap();
    scripts
        .queue_script(source(
            r#"
        if (this !== firstView || window !== firstView || !readyForNextTurn)
            throw "Window identity across turns";
        window.document.documentElement.setAttribute("data-window", "ready");
    "#,
        ))
        .unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    let document = scripts.document();
    let tree = document.parsed().document();
    let root = tree
        .walk(tree.root())
        .unwrap()
        .find_map(|(id, _)| {
            let rustic_dom::NodeKind::Element(element) = tree.node(id).unwrap().kind() else {
                return None;
            };
            (element.name().local() == "html").then_some(element)
        })
        .unwrap();
    assert_eq!(root.attributes()[0].name().local(), "data-window");
}

#[test]
fn detached_document_moves_reach_the_rendered_page_and_change_live_quirks_matching() {
    let mut engine = Engine::new(1);
    let mut load = engine.begin_document(ParserLimits::default()).unwrap();
    load.feed(b"<main><section><span class=Active></span></section></main>")
        .unwrap();
    let mut scripts = load
        .finish()
        .unwrap()
        .into_scripted(TaskLimits::default())
        .unwrap();
    scripts
        .queue_script(source(
            r#"
        const main = document.querySelector("main");
        const section = main.firstChild;
        const collection = section.getElementsByClassName("active");
        if (collection.length !== 1) throw "expected quirks matching";
        const auxiliary = new Document();
        auxiliary.appendChild(section);
        if (collection.length !== 0 || main.firstChild !== null || auxiliary.defaultView !== null)
            throw "moved subtree is in a standards-mode document";
        const newChild = auxiliary.createElement("Added");
        section.appendChild(newChild);
        main.appendChild(section);
        if (collection.length !== 1 || section.firstChild !== collection[0] ||
            newChild.ownerDocument !== document || newChild.localName !== "Added")
            throw "did not preserve the live tree across adoption";
        newChild.appendChild(document.createTextNode("adopted text"));
    "#,
        ))
        .unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    let document = scripts.document();
    let tree = document.parsed().document();
    let dump = tree.dump_tree(tree.root(), 65536).unwrap();
    assert!(dump.contains("Added"), "{dump}");
    assert!(dump.contains("adopted text"), "{dump}");
}

#[test]
fn event_listeners_survive_turns_and_mutate_the_renderer_owned_tree() {
    let mut engine = Engine::new(1);
    let mut load = engine.begin_document(ParserLimits::default()).unwrap();
    load.feed(b"<!doctype html><main><button></button></main>")
        .unwrap();
    let mut page = load
        .finish()
        .unwrap()
        .into_scripted(TaskLimits::default())
        .unwrap();
    page.queue_script(source(
        r#"
        const main = document.querySelector("main");
        const button = main.firstChild;
        main.addEventListener("count", () => main.setAttribute("data-capture", "hit"), true);
        button.addEventListener("count", () => { throw new Error("listener"); }, {once: true});
        button.addEventListener("count", function (event) {
            this.setAttribute("data-count", event.detail);
            event.preventDefault();
            Promise.resolve().then(() => {
                if (event.currentTarget === null && event.eventPhase === Event.NONE)
                    button.setAttribute("data-checkpoint", "ready");
            });
        });
    "#,
    ))
    .unwrap();
    page.queue_script(source(
        r#"
        const result = button.dispatchEvent(new CustomEvent("count", {
            cancelable: true, bubbles: true, detail: 12,
        }));
        if (result || main.getAttribute("data-capture") !== "hit" ||
            button.getAttribute("data-checkpoint") !== null)
            throw "dispatch should be synchronous and schedule its microtask";
    "#,
    ))
    .unwrap();
    assert_eq!(page.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(page.take_listener_error_count(), 0);
    assert_eq!(page.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(page.take_listener_error_count(), 1);
    assert_eq!(page.take_listener_error_count(), 0);
    let document = page.document();
    let tree = document.parsed().document();
    let serialized = tree.dump_tree(tree.root(), 65536).unwrap();
    assert!(
        serialized.contains(r#"local="data-count" value="12""#),
        "{serialized}"
    );
    assert!(
        serialized.contains(r#"local="data-checkpoint" value="ready""#),
        "{serialized}"
    );
}
