use rustic_base::ManualClock;
use rustic_engine::{
    Engine, InlineLoadError, ScriptStop, ScriptTurn, ScriptedDocument, TaskLimits,
};
use rustic_html::{ParseError, ParserLimits};
use rustic_js::Source;

fn check_after_load<C: rustic_base::Clock>(document: &mut ScriptedDocument<C>, js: &str) {
    document
        .queue_script(Source::from_str_limited(js, 4096).unwrap())
        .unwrap();
    assert_eq!(document.run_next(), Ok(Some(ScriptTurn::Completed)), "{js}");
}

#[test]
fn inline_classics_observe_and_edit_the_partial_document_at_every_split() {
    let html = r#"<!doctype html><p id="early"></p><script id="first">
var early = document.querySelector('#early');
var first = document.currentScript;
if (!first || document.querySelector('#later')) throw 'parser ran ahead';
early.setAttribute('data-run', {toString() { return document.currentScript.getAttribute('id'); }});
Promise.resolve().then(() => early.setAttribute('data-job', document.currentScript === null));
</script><p id="later"></p><script>
if (!document.querySelector('#later') || document.currentScript === first) throw 'second boundary';
</script>"#
        .as_bytes();
    for split in 0..=html.len() {
        let mut engine = Engine::new(1);
        let mut load = engine
            .begin_inline_scripted_document(ParserLimits::default(), TaskLimits::default())
            .unwrap();
        load.feed(&html[..split]).unwrap();
        load.feed(&html[split..]).unwrap();
        let mut document = load.finish().unwrap();
        assert_eq!(
            document.take_inline_script_error_count(),
            0,
            "split {split}"
        );
        check_after_load(
            &mut document,
            r#"
if (document.currentScript !== null || first !== document.querySelector('#first') ||
    early !== document.querySelector('#early') || early.getAttribute('data-run') !== 'first' ||
    early.getAttribute('data-job') !== 'true') throw 'realm or DOM changed';
"#,
        );
    }
}

#[test]
fn inline_preparation_and_timer_order_follow_classic_rules() {
    let mut engine = Engine::new(1);
    let mut load = engine
        .begin_inline_scripted_document_with_clock(
            ParserLimits::default(),
            TaskLimits::default(),
            ManualClock::default(),
        )
        .unwrap();
    load.feed(
        br#"<!doctype html><body>
<script type="" language="vbscript">var trace = 'a';</script>
<script type=" application/Javascript " async defer>trace += 'b';</script>
<script language="javascript1.5" for=" WINDOW " event="OnLoad()">trace += 'c';</script>
<script type="application/json">throw 'data block';</script>
<script type="module">throw 'module';</script>
<script type="text/javascript; charset=UTF-8">throw 'MIME parameters';</script>
<script type=" ">throw 'whitespace type';</script>
<script language=" javascript ">throw 'whitespace language';</script>
<script for="document" event="onload">throw 'legacy event';</script>
<script nomodule>throw 'nomodule';</script>
<script src="">throw 'src fallback';</script>
<template><script>throw 'template';</script></template>
<script>
    var detached = document.createElement('script');
    detached.appendChild(document.createTextNode("throw 'dynamic'"));
    document.documentElement.appendChild(detached);
    setTimeout(() => {
        if (document.currentScript !== null || trace !== 'abcd') throw 'timer order';
        trace += 'e';
    }, 0);
</script><script>trace += 'd';</script>"#,
    )
    .unwrap();
    let mut document = load.finish().unwrap();
    assert_eq!(document.take_inline_script_error_count(), 0);
    assert_eq!(document.run_next(), Ok(Some(ScriptTurn::Completed)));
    check_after_load(
        &mut document,
        "if (trace !== 'abcde') throw 'classic selection or order';",
    );
}

#[test]
fn parser_retains_detached_open_nodes_and_current_script_through_collection() {
    let mut engine = Engine::new(1);
    let mut load = engine
        .begin_inline_scripted_document(
            ParserLimits::default(),
            TaskLimits {
                heap_bytes: 1024 * 1024,
                ..TaskLimits::default()
            },
        )
        .unwrap();
    load.feed(br#"<div id="branch"><script id="gc">
(function () {
    var branch = document.querySelector('#branch');
    branch.parentNode.removeChild(branch);
    branch.removeChild(document.currentScript);
})();
for (var i = 0; i < 12000; i++) {
    // Total allocation exceeds the heap limit. Only temporary objects are live.
    ({one: [], two: [], three: [], four: []});
}
if (document.currentScript.getAttribute('id') !== 'gc') throw 'collected script';
</script><b>detached again</b><script>throw 'disconnected';</script></div>
<p id="after"></p><script>document.querySelector('#after').setAttribute('data-seen', 'yes')</script>"#).unwrap();
    let mut document = load.finish().unwrap();
    assert_eq!(document.take_inline_script_error_count(), 0);
    check_after_load(
        &mut document,
        r#"
if (document.querySelector('#branch') !== null ||
    document.querySelector('#after').getAttribute('data-seen') !== 'yes') throw 'resume';
"#,
    );
}

#[test]
fn outward_adoption_during_parsing_protects_only_retained_subtrees() {
    let mut engine = Engine::new(1);
    let mut load = engine
        .begin_inline_scripted_document(ParserLimits::default(), TaskLimits::default())
        .unwrap();
    load.feed(
        br#"<!doctype html><div id="open"><span id="closed"></span><script>
var other = document.implementation.createHTMLDocument('');
var open = document.querySelector('#open');
var operations = [
    () => other.adoptNode(open),
    () => other.documentElement.appendChild(open),
    () => other.adoptNode(document.currentScript)
];
for (var operation of operations) {
    try { operation(); throw 'adopted'; }
    catch (error) { if (error.name !== 'NotSupportedError') throw error; }
}
if (open !== document.querySelector('#open') || open.ownerDocument !== document) throw 'lost owner';
var finishedChild = document.querySelector('#closed');
if (other.adoptNode(finishedChild) !== finishedChild || finishedChild.ownerDocument !== other ||
    document.querySelector('#closed') !== null) throw 'closed child stayed in the parser tree';
var doctype = document.doctype;
var xml = document.implementation.createDocument(null, '', doctype);
if (document.doctype !== null || xml.doctype !== doctype || doctype.ownerDocument !== xml)
    throw 'doctype relocation';
var incoming = other.createElement('em');
open.appendChild(incoming);
if (incoming.ownerDocument !== document) throw 'inward adoption';
var relocatedAtEof = false;
document.addEventListener('DOMContentLoaded', () => {
    var tail = document.querySelector('#tail');
    if (tail.parentNode !== open || incoming.nextSibling !== tail)
        throw 'failed to resume in the right element';
    relocatedAtEof = other.adoptNode(open) === open && open.ownerDocument === other;
});
</script><span id="tail"></span></div>"#,
    )
    .unwrap();
    let mut document = load.finish().unwrap();
    assert_eq!(document.take_inline_script_error_count(), 0);
    check_after_load(
        &mut document,
        "if (!relocatedAtEof || document.querySelector('#tail') !== null) throw 'EOF adoption';",
    );
}

#[test]
fn inline_errors_allow_later_scripts_and_dynamic_markup_reports_an_exception() {
    let mut engine = Engine::new(1);
    let mut load = engine
        .begin_inline_scripted_document(ParserLimits::default(), TaskLimits::default())
        .unwrap();
    load.feed(
        br#"<script>var result = ''; let = ;</script>
<script>var result = 'a'; Promise.resolve().then(() => result += 'b'); throw 0;</script>
<script>
if (result !== 'ab') throw 'missing checkpoint';
for (var method of ['write', 'writeln']) {
    try { document[method]('<p id=unexpected>'); throw 'no error'; }
    catch (error) { if (error.name !== 'NotSupportedError') throw error; }
}
result += 'c';
</script>"#,
    )
    .unwrap();
    let mut document = load.finish().unwrap();
    assert_eq!(document.take_inline_script_error_count(), 2);
    assert_eq!(document.take_inline_script_error_count(), 0);
    check_after_load(
        &mut document,
        r#"
if (result !== 'abc' || document.querySelector('#unexpected') !== null) throw 'error recovery';
"#,
    );
}

#[test]
fn source_and_turn_limits_discard_the_load_and_release_admission() {
    let mut engine = Engine::new(1);
    let parser = ParserLimits::default();
    let limits = TaskLimits {
        pending_source_units: 4,
        ..TaskLimits::default()
    };
    let mut load = engine
        .begin_inline_scripted_document(parser, limits)
        .unwrap();
    load.feed("<script>'😀'</script>".as_bytes()).unwrap();
    let token = load.cancellation().unwrap().clone();
    assert_eq!(
        load.feed("<script> '😀'</script>".as_bytes()),
        Err(InlineLoadError::SourceLimit { limit: 4 })
    );
    assert!(token.is_cancelled());
    assert_eq!(
        load.feed(b""),
        Err(InlineLoadError::Parse(ParseError::Poisoned))
    );

    let mut load = engine
        .begin_inline_scripted_document(
            parser,
            TaskLimits {
                steps_per_turn: 100,
                ..TaskLimits::default()
            },
        )
        .unwrap();
    assert_eq!(
        load.feed(b"<script>for (;;) {}</script>"),
        Err(InlineLoadError::Stopped(ScriptStop::StepLimit))
    );
    assert!(engine.begin_document(parser).is_ok());
}

#[test]
fn pending_inline_loads_share_engine_shutdown() {
    let mut engine = Engine::new(1);
    let mut load = engine
        .begin_inline_scripted_document(ParserLimits::default(), TaskLimits::default())
        .unwrap();
    load.feed(b"<script>var markup").unwrap();
    engine.shutdown();
    assert_eq!(
        load.feed(b"</script>"),
        Err(InlineLoadError::Stopped(ScriptStop::DocumentCancelled))
    );
    assert!(load.cancellation().is_none());
}
