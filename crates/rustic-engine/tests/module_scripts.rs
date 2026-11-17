use std::time::Duration;

use rustic_base::ManualClock;
use rustic_engine::{
    ClassicScriptLimits, ClassicScriptLoad, Engine, ScriptResourceKind, ScriptTurn,
    ScriptedDocument, TaskLimits,
};
use rustic_html::ParserLimits;
use rustic_js::Source;
use rustic_url::Url;

fn load(engine: &mut Engine, limits: ClassicScriptLimits) -> ClassicScriptLoad<ManualClock> {
    engine
        .begin_local_scripted_document_with_clock(
            ParserLimits::default(),
            TaskLimits::default(),
            Url::parse("file:///fixtures/site/index.html", None).unwrap(),
            limits,
            ManualClock::default(),
        )
        .unwrap()
}

fn check(document: &mut ScriptedDocument<ManualClock>, code: &str) {
    document
        .queue_script(Source::from_str_limited(code, 4096).unwrap())
        .unwrap();
    assert_eq!(document.run_next(), Ok(Some(ScriptTurn::Completed)));
}

#[test]
fn mixed_async_and_deferred_modules_share_sources_and_evaluate_once() {
    for async_first in [false, true] {
        let mut engine = Engine::new(1);
        let mut load = load(&mut engine, ClassicScriptLimits::default());
        load.feed(
            br#"<script>var trace = '';</script>
            <script defer src="first.js"></script>
            <script type="module" src="sub/main.js"></script>
            <script type="module">
                import { value } from './shared.js'; trace += value;
                if (import.meta.url !== 'file:///fixtures/site/index.html') throw 'inline URL';
            </script>
            <script defer src="last.js"></script>
            <script type="module" async src="async.js"></script>"#,
        )
        .unwrap();
        let first = load.next_request().unwrap();
        let main = load.next_request().unwrap();
        let shared = load.next_request().unwrap();
        let last = load.next_request().unwrap();
        let fast = load.next_request().unwrap();
        assert_eq!(first.kind, ScriptResourceKind::Classic);
        assert_eq!(main.kind, ScriptResourceKind::JavaScriptModule);
        assert_eq!(shared.url.pathname(), "/fixtures/site/shared.js");
        let main_source = br#"import { value } from '../shared.js';
            if (document.currentScript !== null || import.meta.url !== 'file:///fixtures/site/sub/main.js')
                throw 'module environment';
            trace += 'M' + value;"#;
        let fast_source = b"import { value } from './shared.js'; trace += 'A' + value;";
        if async_first {
            load.complete_script(fast.id, Ok(fast_source)).unwrap();
            load.complete_script(main.id, Ok(main_source)).unwrap();
        } else {
            load.complete_script(main.id, Ok(main_source)).unwrap();
            load.complete_script(fast.id, Ok(fast_source)).unwrap();
        }
        assert!(
            load.next_request().is_none(),
            "the dependency was already requested"
        );
        load.complete_script(last.id, Ok(b"trace += 'L';")).unwrap();
        load.complete_script(first.id, Ok(b"trace += 'F';"))
            .unwrap();
        load.complete_script(shared.id, Ok(b"trace += 'S'; export const value = 'v';"))
            .unwrap();
        load.feed(b"<script>trace += 'E';</script>").unwrap();
        load.close_input().unwrap();
        let mut document = load.take_document().unwrap().unwrap();
        assert_eq!(document.take_module_script_error_count(), 0);
        assert_eq!(document.take_external_script_error_count(), 0);
        check(&mut document, "if (trace !== 'SAvEFMvvL') throw trace;");
    }
}

#[test]
fn module_cycles_and_reexports_reuse_the_root_request() {
    let mut engine = Engine::new(1);
    let mut load = load(&mut engine, ClassicScriptLimits::default());
    load.feed(br#"<script>var result = 0;</script><script type="module" src="a.js"></script>"#)
        .unwrap();
    let a = load.next_request().unwrap();
    load.complete_script(
        a.id,
        Ok(b"import { get } from './b.js'; export function value() { return 4; } result = get();"),
    )
    .unwrap();
    let b = load.next_request().unwrap();
    assert_eq!(b.url.pathname(), "/fixtures/site/b.js");
    load.complete_script(
        b.id,
        Ok(b"import { value } from './a.js'; export function get() { return value() + 1; }"),
    )
    .unwrap();
    assert!(load.next_request().is_none());
    load.close_input().unwrap();
    let mut document = load.take_document().unwrap().unwrap();
    assert_eq!(document.take_module_script_error_count(), 0);
    check(&mut document, "if (result !== 5) throw result;");
}

#[test]
fn top_level_await_can_settle_through_the_document_task_pump() {
    let mut engine = Engine::new(1);
    let mut load = load(&mut engine, ClassicScriptLimits::default());
    load.feed(
        br#"<script>var trace = '';</script><script type="module">
        trace += 'a';
        await new Promise(resolve => setTimeout(resolve, 5));
        trace += 'c';
        </script><script type="module">trace += 'b';</script>"#,
    )
    .unwrap();
    load.close_input().unwrap();
    assert!(load.take_document().unwrap().is_none());
    load.clock()
        .unwrap()
        .advance(Duration::from_millis(4))
        .unwrap();
    assert_eq!(load.run_next(), Ok(None));
    load.clock()
        .unwrap()
        .advance(Duration::from_millis(1))
        .unwrap();
    assert_eq!(load.run_next(), Ok(Some(ScriptTurn::Completed)));
    let mut document = load.take_document().unwrap().unwrap();
    assert_eq!(document.take_module_script_error_count(), 0);
    check(&mut document, "if (trace !== 'abc') throw trace;");
}

#[test]
fn failed_modules_release_order_and_keep_evaluated_dependencies_cached() {
    let mut engine = Engine::new(1);
    let mut load = load(&mut engine, ClassicScriptLimits::default());
    load.feed(
        br#"<script>var trace = '';</script>
        <script type="module" src="bad.js"></script>
        <script type="module" src="bad.js"></script>
        <script type="module">import '../outside.js'; throw 'unexpected';</script>
        <script type="module">import './shared.js'; trace += 'end';</script>"#,
    )
    .unwrap();
    let bad = load.next_request().unwrap();
    let shared = load.next_request().unwrap();
    assert!(load.next_request().is_none());
    load.complete_script(
        bad.id,
        Ok(b"import './shared.js'; trace += 'X'; throw 'failed';"),
    )
    .unwrap();
    load.complete_script(shared.id, Ok(b"trace += 'S';"))
        .unwrap();
    load.close_input().unwrap();
    let mut document = load.take_document().unwrap().unwrap();
    assert_eq!(document.take_module_script_error_count(), 3);
    check(&mut document, "if (trace !== 'SXend') throw trace;");
}

#[test]
fn module_dependencies_share_the_request_budget_with_classics() {
    let mut engine = Engine::new(1);
    let mut load = load(
        &mut engine,
        ClassicScriptLimits {
            max_requests: 2,
            ..ClassicScriptLimits::default()
        },
    );
    load.feed(
        br#"<script>var trace = '';</script><script src="classic.js"></script>
        <script type="module" src="entry.js"></script><script>trace += 'i';</script>"#,
    )
    .unwrap();
    let classic = load.next_request().unwrap();
    load.complete_script(classic.id, Ok(b"trace += 'c';"))
        .unwrap();
    let entry = load.next_request().unwrap();
    load.complete_script(entry.id, Ok(b"import './dependency.js';"))
        .unwrap();
    assert!(load.next_request().is_none());
    load.close_input().unwrap();
    let mut document = load.take_document().unwrap().unwrap();
    assert_eq!(document.take_module_script_error_count(), 1);
    check(&mut document, "if (trace !== 'ci') throw trace;");
}
