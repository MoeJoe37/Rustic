use rustic_base::CancellationToken;
use rustic_html::{
    Diagnostic, DiagnosticKind, HtmlTokenizer, Token, TokenizerLimits, TokenizerMode,
};

#[derive(Debug, Eq, PartialEq)]
enum Expected {
    Comment(&'static str),
    Character(&'static str),
    Doctype(
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
        bool,
    ),
    Instruction(&'static str, &'static str),
    StartTag(&'static str, &'static [(&'static str, &'static str)], bool),
    EndTag(&'static str),
}

struct Case {
    file: &'static str,
    index: usize,
    description: &'static str,
    input: &'static str,
    output: &'static [Expected],
    errors: &'static [DiagnosticKind],
}

struct TextCase {
    case: Case,
    mode: TokenizerMode,
    last_start_tag: Option<&'static str>,
}

#[path = "fixtures/html5lib_cases.rs"]
mod fixtures;

#[path = "fixtures/references_cases.rs"]
mod references;

#[path = "fixtures/text_cases.rs"]
mod text;

fn tokenize(
    chunks: &[&[u8]],
    case: &Case,
    mode: TokenizerMode,
    last: Option<&str>,
) -> (Vec<Token>, Vec<Diagnostic>) {
    let mut tokenizer = HtmlTokenizer::with_context(
        TokenizerLimits::default(),
        CancellationToken::default(),
        mode,
        last,
    )
    .unwrap();
    // Upstream supplies an already decoded Unicode stream. Add a transport BOM
    // so a leading U+FEFF in that stream remains content after UTF-8 decoding.
    assert!(tokenizer.feed(b"\xef\xbb\xbf").unwrap().is_empty());
    let mut output = Vec::new();
    for chunk in chunks {
        let tokens = tokenizer
            .feed(chunk)
            .unwrap_or_else(|error| panic!("{}[{}]: {error}", case.file, case.index));
        for token in tokens {
            assert_ne!(token, Token::Eof, "EOF must wait for finish");
            append_token(&mut output, token);
        }
    }
    let ending = tokenizer
        .finish()
        .unwrap_or_else(|error| panic!("{}[{}] at EOF: {error}", case.file, case.index));
    assert_eq!(ending.last(), Some(&Token::Eof));
    assert_eq!(
        ending.iter().filter(|token| **token == Token::Eof).count(),
        1
    );
    for token in ending {
        append_token(&mut output, token);
    }
    (output, tokenizer.diagnostics().to_vec())
}

fn append_token(output: &mut Vec<Token>, token: Token) {
    match (output.last_mut(), token) {
        (Some(Token::Characters(previous)), Token::Characters(next)) => previous.push_str(&next),
        (_, Token::Eof) => {}
        (_, token) => output.push(token),
    }
}

fn token_matches(actual: &Token, expected: &Expected) -> bool {
    match (actual, expected) {
        (Token::Comment(data), Expected::Comment(value))
        | (Token::Characters(data), Expected::Character(value)) => data == value,
        (Token::Doctype(data), Expected::Doctype(name, public, system, force)) => {
            (
                data.name(),
                data.public_id(),
                data.system_id(),
                data.force_quirks(),
            ) == (*name, *public, *system, *force)
        }
        (Token::ProcessingInstruction(data), Expected::Instruction(target, value)) => {
            (data.target(), data.data()) == (*target, *value)
        }
        (Token::StartTag(tag), Expected::StartTag(name, attributes, flag)) => {
            tag.name() == *name
                && tag.self_closing() == *flag
                && tag.attributes().len() == attributes.len()
                && attributes.iter().all(|(name, value)| {
                    tag.attributes()
                        .iter()
                        .any(|attribute| attribute.name() == *name && attribute.value() == *value)
                })
        }
        (Token::EndTag(name), Expected::EndTag(expected)) => name == expected,
        _ => false,
    }
}

#[test]
fn pinned_declaration_corpus_and_all_input_splits_have_no_unexpected_failures() {
    run_cases(
        "declarations",
        fixtures::CASES,
        fixtures::SELECTED,
        fixtures::UPSTREAM,
    );
}

#[test]
fn pinned_reference_corpus_and_all_input_splits_have_no_unexpected_failures() {
    run_cases(
        "references",
        references::CASES,
        references::SELECTED,
        references::UPSTREAM,
    );
}

fn run_cases(suite: &str, cases: &[Case], selected: usize, upstream: usize) {
    assert_eq!(cases.len(), selected);
    let mut failures = Vec::new();
    let mut split_runs = 0;
    for case in cases {
        check_case(
            case,
            TokenizerMode::Data,
            None,
            &mut failures,
            &mut split_runs,
        );
    }
    report(suite, failures, selected, upstream, split_runs, selected);
}

#[test]
fn pinned_text_modes_and_all_input_splits_have_no_unexpected_failures() {
    assert_eq!(text::CASES.len(), text::STATE_RUNS);
    let mut failures = Vec::new();
    let mut split_runs = 0;
    for context in text::CASES {
        check_case(
            &context.case,
            context.mode,
            context.last_start_tag,
            &mut failures,
            &mut split_runs,
        );
    }
    report(
        "text modes",
        failures,
        text::SELECTED,
        text::UPSTREAM,
        split_runs,
        text::STATE_RUNS,
    );
}

fn check_case(
    case: &Case,
    mode: TokenizerMode,
    last: Option<&str>,
    failures: &mut Vec<String>,
    split_runs: &mut usize,
) {
    let bytes = case.input.as_bytes();
    let (tokens, errors) = tokenize(&[bytes], case, mode, last);
    let kinds: Vec<_> = errors.iter().map(|error| error.kind).collect();
    if tokens.len() != case.output.len()
        || !tokens
            .iter()
            .zip(case.output)
            .all(|(a, b)| token_matches(a, b))
        || kinds != case.errors
    {
        failures.push(format!(
            "{}[{}] {:?} {}: expected {:?} / {:?}; got {:?} / {:?}",
            case.file, case.index, mode, case.description, case.output, case.errors, tokens, kinds
        ));
        return;
    }
    for split in 0..=bytes.len() {
        assert_eq!(
            tokenize(&[&bytes[..split], b"", &bytes[split..]], case, mode, last),
            (tokens.clone(), errors.clone()),
            "{}[{}], split {split}",
            case.file,
            case.index
        );
        *split_runs += 1;
    }
    assert_eq!(
        tokenize(&bytes.chunks(1).collect::<Vec<_>>(), case, mode, last),
        (tokens, errors),
        "{}[{}], one-byte feeds",
        case.file,
        case.index
    );
}

fn report(
    suite: &str,
    failures: Vec<String>,
    selected: usize,
    upstream: usize,
    split_runs: usize,
    state_runs: usize,
) {
    assert!(
        failures.is_empty(),
        "{} corpus failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    println!(
        "html5lib {suite}: {selected} selected / {upstream} upstream; {state_runs} contexts, {split_runs} two-chunk splits and {state_runs} one-byte runs passed."
    );
}
