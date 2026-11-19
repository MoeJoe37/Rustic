//! Mutation observers see parser insertions and run at microtask checkpoints.

use rustic_engine::{Engine, ScriptTurn, TaskLimits};
use rustic_html::ParserLimits;
use rustic_js::Source;

#[test]
fn parser_insertions_reach_observers_at_the_next_checkpoint() {
    let html = r#"<!doctype html><body><script>
var log = [];
new MutationObserver((records) => {
    for (const record of records)
        for (const node of record.addedNodes)
            log.push(node.nodeType === 1 ? node.localName : '#text');
}).observe(document.body, { childList: true, subtree: true });
</script><p id="a">text</p><div><span></span></div><script>
log.push('second script');
</script></body>"#
        .as_bytes();
    for split in [0, 40, html.len() / 2, html.len()] {
        let mut engine = Engine::new(1);
        let mut load = engine
            .begin_inline_scripted_document(ParserLimits::default(), TaskLimits::default())
            .unwrap();
        load.feed(&html[..split]).unwrap();
        load.feed(&html[split..]).unwrap();
        let mut document = load.finish().unwrap();
        assert_eq!(document.take_inline_script_error_count(), 0);
        document
            .queue_script(
                Source::from_str_limited(
                    r#"
                    // The parser inserted the first paragraph and its text
                    // before the second script ran; that script's checkpoint
                    // delivered them. Later insertions follow.
                    const seen = log.join();
                    if (!seen.startsWith('p,#text,div,span') || !seen.includes('second script'))
                        throw new Error(seen);
                    "#,
                    4096,
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            document.run_next(),
            Ok(Some(ScriptTurn::Completed)),
            "split {split}"
        );
    }
}
