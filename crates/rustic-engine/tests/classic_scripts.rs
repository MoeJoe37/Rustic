use std::time::Duration;

use rustic_base::ManualClock;
use rustic_engine::{
    ClassicFetchError, ClassicScriptLimits, ClassicScriptLoad, ClassicScriptMode, Engine,
    ScriptLoadError, ScriptStop, ScriptTurn, ScriptedDocument, TaskLimits,
};
use rustic_html::{ParseError, ParserLimits};
use rustic_js::Source;
use rustic_url::Url;

fn load(engine: &mut Engine, clock: ManualClock) -> ClassicScriptLoad<ManualClock> {
    engine
        .begin_local_scripted_document_with_clock(
            ParserLimits::default(),
            TaskLimits::default(),
            Url::parse("file:///fixtures/site/index.html", None).unwrap(),
            ClassicScriptLimits::default(),
            clock,
        )
        .unwrap()
}

fn check<C: rustic_base::Clock>(document: &mut ScriptedDocument<C>, script: &str) {
    document
        .queue_script(Source::from_str_limited(script, 4096).unwrap())
        .unwrap();
    assert_eq!(
        document.run_next(),
        Ok(Some(ScriptTurn::Completed)),
        "{script}"
    );
}

#[test]
fn readiness_and_script_events_order_deferred_and_outstanding_async_work() {
    let mut engine = Engine::new(1);
    let mut load = load(&mut engine, ManualClock::default());
    load.feed(
        br#"<!doctype html>
      <script id="async" async src="async.js"></script>
      <script id="defer" defer src="defer.js"></script>
      <script id="module" type="module" src="module.js"></script>
      <script id="missing" defer src="missing.js"></script>
      <script>
        var trace = [document.readyState];
        document.addEventListener('readystatechange', e => {
          if (e.isTrusted && e.target === document) trace.push(document.readyState);
        });
        window.addEventListener('DOMContentLoaded', e => {
          if (e.isTrusted && e.target === document && e.bubbles)
            trace.push('dom:' + document.readyState);
          Promise.resolve().then(() => trace.push('dom-job'));
        });
        window.addEventListener('load', e => {
          if (e.isTrusted && e.currentTarget === window && e.target === document)
            trace.push('window:' + document.readyState);
        });
        for (const id of ['async', 'defer', 'module', 'missing']) {
          const element = document.getElementById(id);
          for (const kind of ['load', 'error']) {
            element.addEventListener(kind, e => {
              if (e.isTrusted && !e.bubbles && e.target === element)
                trace.push(id + ':' + kind);
            });
          }
        }
      </script>"#,
    )
    .unwrap();
    let asynchronous = load.next_request().unwrap();
    let deferred = load.next_request().unwrap();
    let module = load.next_request().unwrap();
    let missing = load.next_request().unwrap();
    load.complete_script(module.id, Ok(b"trace.push('module-run')"))
        .unwrap();
    load.complete_script(missing.id, Err(ClassicFetchError::NotFound))
        .unwrap();
    load.close_input().unwrap();
    assert!(load.take_document().unwrap().is_none());
    load.complete_script(deferred.id, Ok(b"trace.push('defer-run'); throw 1"))
        .unwrap();
    assert!(load.take_document().unwrap().is_none());
    load.complete_script(
        asynchronous.id,
        Ok(b"trace.push('async:' + document.readyState)"),
    )
    .unwrap();
    let mut document = load.take_document().unwrap().unwrap();
    assert!(load.take_document().unwrap().is_none());
    check(
        &mut document,
        r#"
      if (trace.join(',') !== 'loading,interactive,defer-run,defer:load,module-run,module:load,missing:error,dom:interactive,dom-job,async:interactive,async:load,complete,window:complete')
        throw trace.join(',');
    "#,
    );
}

#[test]
fn blocking_classics_hold_the_parser_and_checkpoint_before_later_markup() {
    let mut engine = Engine::new(1);
    let mut load = load(&mut engine, ManualClock::default());
    load.feed(
        br#"<!doctype html><script>var trace = ''</script>
        <script id="async" async src="early.js"></script>
        <script id="block" src="block.js">throw 'src fallback';</script>"#,
    )
    .unwrap();
    let asynchronous = load.next_request().unwrap();
    let blocking = load.next_request().unwrap();
    assert_eq!(asynchronous.mode, ClassicScriptMode::Async);
    assert_eq!(blocking.mode, ClassicScriptMode::Blocking);
    load.feed(br#"<p id="later"></p><script>trace += 'c';</script>"#)
        .unwrap();
    load.feed(br#"<script id="late" async src="late.js"></script>"#)
        .unwrap();
    load.close_input().unwrap();
    assert!(matches!(
        load.feed(b"x"),
        Err(ScriptLoadError::Parse(ParseError::InputClosed))
    ));
    assert!(load.next_request().is_none());
    assert!(load.take_document().unwrap().is_none());

    load.complete_script(
        asynchronous.id,
        Ok(br#"
        if (document.currentScript.getAttribute('id') !== 'async' ||
            document.querySelector('#later') !== null) throw 'async ran late';
        trace += 'a';
    "#),
    )
    .unwrap();
    load.complete_script(
        blocking.id,
        Ok(br#"
        if (trace !== 'a' || document.querySelector('#later') !== null ||
            document.currentScript.getAttribute('id') !== 'block') throw 'parser ran ahead';
        trace += 'b';
        Promise.resolve().then(() => {
            if (document.currentScript === null && document.querySelector('#later') === null)
                trace += 'p';
        });
    "#),
    )
    .unwrap();
    let late = load.next_request().unwrap();
    assert_eq!(late.url.pathname(), "/fixtures/site/late.js");
    assert!(load.take_document().unwrap().is_none());
    load.complete_script(late.id, Ok(b"trace += 'd'")).unwrap();
    let mut document = load.take_document().unwrap().unwrap();
    assert_eq!(document.take_inline_script_error_count(), 0);
    assert_eq!(document.take_external_script_error_count(), 0);
    check(
        &mut document,
        "if (trace !== 'abpcd' || document.currentScript !== null) throw trace;",
    );
    assert!(load.take_document().unwrap().is_none());
}

#[test]
fn async_and_defer_follow_all_arrival_orders_on_the_manual_clock() {
    let sources: [&[u8]; 4] = [
        b"trace += 'a'; Promise.resolve().then(() => trace += 'A');",
        b"trace += 'b'; Promise.resolve().then(() => trace += 'B');",
        b"trace += '1'; Promise.resolve().then(() => trace += 'x');",
        b"trace += '2'; Promise.resolve().then(() => trace += 'y');",
    ];
    for first in 0..4 {
        for second in 0..4 {
            for third in 0..4 {
                for fourth in 0..4 {
                    let order = [first, second, third, fourth];
                    let mut sorted = order;
                    sorted.sort_unstable();
                    if sorted != [0, 1, 2, 3] {
                        continue;
                    }
                    for close_after in 0..=4 {
                        let mut engine = Engine::new(1);
                        let mut load = load(&mut engine, ManualClock::default());
                        load.feed(
                            br#"<script>var trace = '';</script>
                            <script async defer src="a.js"></script>
                            <script async src="b.js"></script>
                            <script defer src="d1.js"></script>
                            <script defer src="d2.js"></script>"#,
                        )
                        .unwrap();
                        let requests: Vec<_> = std::iter::from_fn(|| load.next_request()).collect();
                        assert_eq!(
                            requests
                                .iter()
                                .map(|request| request.mode)
                                .collect::<Vec<_>>(),
                            [
                                ClassicScriptMode::Async,
                                ClassicScriptMode::Async,
                                ClassicScriptMode::Defer,
                                ClassicScriptMode::Defer
                            ],
                        );
                        for (arrival, &index) in order.iter().enumerate() {
                            if arrival == close_after {
                                load.feed(b"<script>trace += 'e';</script>").unwrap();
                                load.close_input().unwrap();
                            }
                            assert!(load.take_document().unwrap().is_none());
                            load.clock()
                                .unwrap()
                                .advance(Duration::from_millis(1))
                                .unwrap();
                            load.complete_script(requests[index].id, Ok(sources[index]))
                                .unwrap();
                        }
                        if close_after == 4 {
                            load.feed(b"<script>trace += 'e';</script>").unwrap();
                            load.close_input().unwrap();
                        }
                        // Each defer can run only after both EOF and all earlier defers are ready.
                        let d1 = order.iter().position(|&index| index == 2).unwrap() + 1;
                        let d2 = order.iter().position(|&index| index == 3).unwrap() + 1;
                        let mut expected = String::new();
                        for completed in 0..=4 {
                            if completed > 0 {
                                match order[completed - 1] {
                                    0 => expected.push_str("aA"),
                                    1 => expected.push_str("bB"),
                                    _ => {}
                                }
                            }
                            if completed == close_after {
                                expected.push('e');
                            }
                            if completed == d1.max(close_after) {
                                expected.push_str("1x");
                            }
                            if completed == d1.max(d2).max(close_after) {
                                expected.push_str("2y");
                            }
                        }
                        let mut document = load.take_document().unwrap().unwrap();
                        assert_eq!(document.take_external_script_error_count(), 0);
                        check(
                            &mut document,
                            &format!("if (trace !== '{expected}') throw trace;"),
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn local_base_authority_and_response_budgets_preserve_later_scripts() {
    let mut engine = Engine::new(1);
    let invalid = Url::parse("https://example.test/index.html", None).unwrap();
    assert!(matches!(
        engine.begin_local_scripted_document(
            ParserLimits::default(),
            TaskLimits::default(),
            invalid,
            ClassicScriptLimits::default(),
        ),
        Err(ScriptLoadError::InvalidDocumentUrl)
    ));
    let mut load = engine
        .begin_local_scripted_document(
            ParserLimits::default(),
            TaskLimits::default(),
            Url::parse("file:///fixtures/site/index.html", None).unwrap(),
            ClassicScriptLimits {
                max_requests: 6,
                max_response_bytes: 6,
                max_total_bytes: 8,
            },
        )
        .unwrap();
    load.feed(
        br#"<base href="sub/"><base href="ignored/">
        <script>var mark = 0;</script>
        <script src="../../outside.js">throw 'outside';</script>
        <script src="https://example.test/remote.js"></script>
        <script src="big.js"></script><script src="good.js"></script>
        <script src="too-much.js"></script><script src="missing.js"></script>
        <script src="over-count.js"></script>
        <script>mark += 10;</script>"#,
    )
    .unwrap();
    load.close_input().unwrap();
    for (path, maximum, response) in [
        ("big.js", 6, Ok(b"mark=99".as_slice())),
        ("good.js", 6, Ok(b"mark=1".as_slice())),
        ("too-much.js", 2, Ok(b";;;".as_slice())),
        ("missing.js", 2, Err(ClassicFetchError::NotFound)),
    ] {
        let request = load.next_request().unwrap();
        assert_eq!(request.url.pathname(), format!("/fixtures/site/sub/{path}"));
        assert_eq!(request.max_bytes, maximum);
        load.complete_script(request.id, response).unwrap();
    }
    assert!(load.next_request().is_none());
    let mut document = load.take_document().unwrap().unwrap();
    assert_eq!(document.take_external_script_error_count(), 6);
    assert_eq!(document.take_inline_script_error_count(), 0);
    check(&mut document, "if (mark !== 11) throw mark;");
}

#[test]
fn pending_detached_scripts_keep_current_script_during_evaluation() {
    let mut engine = Engine::new(1);
    let mut load = engine
        .begin_local_scripted_document(
            ParserLimits::default(),
            TaskLimits {
                heap_bytes: 1024 * 1024,
                ..TaskLimits::default()
            },
            Url::parse("file:///index.html", None).unwrap(),
            ClassicScriptLimits::default(),
        )
        .unwrap();
    load.feed(
        br#"<script defer id="detached" src="later.js"></script>
        <script>
        (function () {
            var node = document.querySelector('#detached');
            node.parentNode.removeChild(node);
            var other = new Document();
            try {
                other.adoptNode(node);
                throw 'adopted a pending script';
            } catch (error) {
                if (error.name !== 'NotSupportedError') throw error;
            }
        })();
        for (var i = 0; i < 12000; i++) ({one: [], two: [], three: [], four: []});
        </script>"#,
    )
    .unwrap();
    let request = load.next_request().unwrap();
    load.close_input().unwrap();
    load.complete_script(
        request.id,
        Ok(br#"
        var detached = document.currentScript;
        if (detached.getAttribute('id') !== 'detached' || detached.parentNode !== null)
            throw 'lost pending script';
        throw 'recoverable JS fault';
    "#),
    )
    .unwrap();
    let mut document = load.take_document().unwrap().unwrap();
    assert_eq!(document.take_inline_script_error_count(), 0);
    assert_eq!(document.take_external_script_error_count(), 1);
    check(
        &mut document,
        r#"
        if (document.currentScript !== null || !detached ||
            detached.getAttribute('id') !== 'detached' || detached.parentNode !== null)
            throw 'missing script';
    "#,
    );
}

#[test]
fn external_source_limits_and_fatal_turns_use_the_document_budgets() {
    let mut engine = Engine::new(1);
    let url = Url::parse("file:///fixtures/site/index.html", None).unwrap();
    let parser = ParserLimits::default();
    let limits = ClassicScriptLimits::default();
    let mut load = engine
        .begin_local_scripted_document(
            parser,
            TaskLimits {
                pending_source_units: 6,
                ..TaskLimits::default()
            },
            url.clone(),
            limits,
        )
        .unwrap();
    load.feed(b"<script src=ok.js></script><script src=too-long.js></script>")
        .unwrap();
    load.close_input().unwrap();
    let first = load.next_request().unwrap();
    load.complete_script(first.id, Ok(b"\xef\xbb\xbfmark=1"))
        .unwrap();
    let second = load.next_request().unwrap();
    load.complete_script(second.id, Ok(b"mark=22")).unwrap();
    let mut document = load.take_document().unwrap().unwrap();
    assert_eq!(document.take_external_script_error_count(), 1);
    check(&mut document, "mark;");
}

#[test]
fn fatal_external_execution_releases_document_admission() {
    let mut engine = Engine::new(1);
    let mut load = engine
        .begin_local_scripted_document(
            ParserLimits::default(),
            TaskLimits {
                steps_per_turn: 100,
                ..TaskLimits::default()
            },
            Url::parse("file:///fixtures/site/index.html", None).unwrap(),
            ClassicScriptLimits::default(),
        )
        .unwrap();
    load.feed(b"<script src=loop.js></script>").unwrap();
    let token = load.cancellation().unwrap().clone();
    let request = load.next_request().unwrap();
    assert_eq!(
        load.complete_script(request.id, Ok(b"for (;;) {}")),
        Err(ScriptLoadError::Stopped(ScriptStop::StepLimit)),
    );
    assert!(token.is_cancelled());
    assert!(engine.begin_document(ParserLimits::default()).is_ok());
}

#[test]
fn request_ids_reject_stale_and_foreign_completions_and_shutdown_cancels() {
    let mut engine = Engine::new(2);
    let mut a = load(&mut engine, ManualClock::default());
    let mut b = load(&mut engine, ManualClock::default());
    a.feed(b"<script src=a.js></script>").unwrap();
    b.feed(b"<script src=b.js></script>").unwrap();
    let id = a.next_request().unwrap().id;
    assert_eq!(
        b.complete_script(id, Ok(b"")),
        Err(ScriptLoadError::UnknownScriptRequest)
    );
    a.complete_script(id, Ok(b"")).unwrap();
    assert_eq!(
        a.complete_script(id, Ok(b"")),
        Err(ScriptLoadError::UnknownScriptRequest)
    );
    a.close_input().unwrap();
    assert!(a.take_document().unwrap().is_some());
    assert_eq!(
        a.complete_script(id, Ok(b"")),
        Err(ScriptLoadError::UnknownScriptRequest)
    );
    let other = b.next_request().unwrap().id;
    engine.shutdown();
    assert_eq!(
        b.complete_script(other, Ok(b"")),
        Err(ScriptLoadError::Stopped(ScriptStop::DocumentCancelled))
    );
    assert!(b.next_request().is_none());
    assert!(b.cancellation().is_none());
}
