use rustic_html::{Diagnostic, DiagnosticKind, HtmlTokenizer, Token};

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
}

struct Case {
    file: &'static str,
    index: usize,
    description: &'static str,
    input: &'static str,
    output: &'static [Expected],
    errors: &'static [DiagnosticKind],
}

#[path = "fixtures/html5lib_cases.rs"]
mod fixtures;

fn tokenize(chunks: &[&[u8]], case: &Case) -> (Vec<Token>, Vec<Diagnostic>) {
    let mut tokenizer = HtmlTokenizer::default();
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
        _ => false,
    }
}

#[test]
fn pinned_declaration_corpus_and_all_input_splits_have_no_unexpected_failures() {
    assert_eq!(fixtures::CASES.len(), fixtures::SELECTED);
    let mut failures = Vec::new();
    let mut split_runs = 0;
    for case in fixtures::CASES {
        let bytes = case.input.as_bytes();
        let (tokens, errors) = tokenize(&[bytes], case);
        let kinds: Vec<_> = errors.iter().map(|error| error.kind).collect();
        if tokens.len() != case.output.len()
            || !tokens
                .iter()
                .zip(case.output)
                .all(|(a, b)| token_matches(a, b))
            || kinds != case.errors
        {
            failures.push(format!(
                "{}[{}] {}: expected {:?} / {:?}; got {:?} / {:?}",
                case.file, case.index, case.description, case.output, case.errors, tokens, kinds
            ));
            continue;
        }
        for split in 0..=bytes.len() {
            assert_eq!(
                tokenize(&[&bytes[..split], b"", &bytes[split..]], case),
                (tokens.clone(), errors.clone()),
                "{}[{}], split {split}",
                case.file,
                case.index
            );
            split_runs += 1;
        }
        assert_eq!(
            tokenize(&bytes.chunks(1).collect::<Vec<_>>(), case),
            (tokens, errors),
            "{}[{}], one-byte feeds",
            case.file,
            case.index
        );
    }
    assert!(
        failures.is_empty(),
        "{} corpus failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    println!(
        "html5lib declaration subset: {} selected / {} upstream; {split_runs} two-chunk splits and {} one-byte runs passed.",
        fixtures::SELECTED,
        fixtures::UPSTREAM,
        fixtures::SELECTED
    );
}
