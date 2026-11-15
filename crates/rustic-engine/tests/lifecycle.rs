use rustic_engine::{DocumentStartError, Engine};
use rustic_html::{HtmlError, ParseError, ParserLimits};

#[test]
fn pending_and_completed_documents_share_the_same_bounded_slots() {
    let mut engine = Engine::new(1);
    let limits = ParserLimits::default();
    let mut pending = engine.begin_document(limits).unwrap();
    let token = pending.cancellation().unwrap().clone();
    assert_eq!(
        engine.begin_document(limits).unwrap_err(),
        DocumentStartError::DocumentLimit
    );

    pending
        .feed(b"<!doctype html><title>Rustic</title>")
        .unwrap();
    let document = pending.finish().unwrap();
    assert!(document.parsed().tree_diagnostics().is_empty());
    assert!(!token.is_cancelled());
    assert_eq!(
        engine.begin_document(limits).unwrap_err(),
        DocumentStartError::DocumentLimit
    );

    drop(document);
    assert!(token.is_cancelled());
    let replacement = engine.begin_document(limits).unwrap();
    assert!(!replacement.cancellation().unwrap().is_cancelled());
    assert_eq!(
        Engine::new(0).begin_document(limits).unwrap_err(),
        DocumentStartError::DocumentLimit
    );
}

#[test]
fn shutdown_cancels_existing_work_and_closes_admission() {
    let limits = ParserLimits::default();
    let mut engine = Engine::new(2);
    let pending = engine.begin_document(limits).unwrap();
    let loading = pending.cancellation().unwrap().clone();
    let document = engine.begin_document(limits).unwrap().finish().unwrap();
    let live = document.cancellation().clone();
    let mut separate_engine = Engine::new(1);
    let unrelated = separate_engine.begin_document(limits).unwrap();

    engine.shutdown();
    engine.shutdown();
    std::thread::spawn(move || {
        assert!(loading.is_cancelled());
        assert!(live.is_cancelled());
    })
    .join()
    .unwrap();
    assert_eq!(
        pending.finish().unwrap_err(),
        ParseError::Tokenizer(HtmlError::Cancelled)
    );
    assert_eq!(
        engine.begin_document(limits).unwrap_err(),
        DocumentStartError::Shutdown
    );
    assert!(!unrelated.cancellation().unwrap().is_cancelled());
}

#[test]
fn failed_and_abandoned_parses_release_their_slots() {
    let mut engine = Engine::new(1);
    let limits = ParserLimits::default();
    let mut invalid = limits;
    invalid.tokenizer.max_text_chunk_bytes = 0;
    assert!(matches!(
        engine.begin_document(invalid),
        Err(DocumentStartError::Parser(_))
    ));

    let mut small = limits;
    small.tokenizer.max_input_bytes = 1;
    let mut failed = engine.begin_document(small).unwrap();
    let failed_token = failed.cancellation().unwrap().clone();
    assert!(failed.feed(b"<p>").is_err());
    assert_eq!(failed.feed(b""), Err(ParseError::Poisoned));
    assert!(failed_token.is_cancelled());

    let pending = engine.begin_document(limits).unwrap();
    let abandoned_token = pending.cancellation().unwrap().clone();
    drop(pending);
    assert!(abandoned_token.is_cancelled());

    let live = engine.begin_document(limits).unwrap().finish().unwrap();
    drop(engine);
    assert!(live.cancellation().is_cancelled());
}
