use rustic_base::CancellationToken;
use rustic_dom::{Document, DomError, HTML_NAMESPACE, NodeId, NodeKind};
use rustic_html::{
    DocumentMode, HtmlDocumentParser, HtmlError, ParseError, ParsedDocument, ParserLimits,
    TreeDiagnosticKind as D, UnsupportedFeature as U,
};

fn parse(bytes: &[u8], limits: ParserLimits) -> Result<ParsedDocument, ParseError> {
    let mut parser = HtmlDocumentParser::new(limits, CancellationToken::default())?;
    parser.feed(bytes)?;
    parser.finish()
}

fn shape(result: &ParsedDocument) -> String {
    let doc = result.document();
    let mut lines = Vec::new();
    for (id, depth) in doc.walk(doc.root()).unwrap().skip(1) {
        let label = match doc.node(id).unwrap().kind() {
            NodeKind::Element(element) => {
                assert_eq!(element.name().namespace(), Some(HTML_NAMESPACE));
                element.name().local().to_owned()
            }
            NodeKind::Text(value) => format!("{:?}", String::from_utf16(value.as_units()).unwrap()),
            NodeKind::Comment(value) => format!(
                "comment {:?}",
                String::from_utf16(value.as_units()).unwrap()
            ),
            NodeKind::DocumentType(value) => format!(
                "doctype {:?}",
                String::from_utf16(value.name.as_units()).unwrap()
            ),
            NodeKind::Document | NodeKind::DocumentFragment => unreachable!(),
        };
        lines.push(format!("{depth}:{label}"));
    }
    lines.join("\n")
}

fn elements(doc: &Document, local: &str) -> Vec<NodeId> {
    doc.walk(doc.root()).unwrap().filter_map(|(id, _)| {
        matches!(doc.node(id).unwrap().kind(), NodeKind::Element(e) if e.name().local() == local).then_some(id)
    }).collect()
}

#[test]
fn eof_reprocesses_all_shell_states_without_duplicate_nodes() {
    for source in [
        "",
        " ",
        "<!doctype html>",
        "<html>",
        "<head>",
        "<head><noscript>",
        "<head><style>",
        "<head><noscript><style>",
        "<head></head>",
        "</body>",
        "</html>",
    ] {
        let result = parse(source.as_bytes(), ParserLimits::default()).unwrap();
        for name in ["html", "head", "body"] {
            assert_eq!(elements(result.document(), name).len(), 1, "{source}");
        }
        let doc = result.document();
        let count = doc.walk(doc.root()).unwrap().count();
        assert_eq!(count, doc.node_count(), "no orphan parser nodes: {source}");
    }
    let result = parse(b"", ParserLimits::default()).unwrap();
    assert_eq!(shape(&result), "1:html\n2:head\n2:body");
    assert_eq!(result.mode(), DocumentMode::Quirks);
    assert_eq!(
        result
            .tree_diagnostics()
            .iter()
            .map(|d| d.kind)
            .collect::<Vec<_>>(),
        [D::MissingDoctype]
    );
}

#[test]
fn comments_whitespace_and_nul_follow_their_insertion_modes() {
    let source = " \t<!--pre--><!doctype html><!--d--><html><!--h--><head> \n<title>A&amp;<b>🚀</title><!--in--></head> \t<!--after--><body>a\0b<!--split-->c</body><!--tail--></html><!--end-->";
    let result = parse(source.as_bytes(), ParserLimits::default()).unwrap();
    assert_eq!(
        shape(&result),
        concat!(
            "1:comment \"pre\"\n1:doctype \"html\"\n1:comment \"d\"\n1:html\n2:comment \"h\"\n2:head\n",
            "3:\" \\n\"\n3:title\n4:\"A&<b>🚀\"\n3:comment \"in\"\n2:\" \\t\"\n2:comment \"after\"\n",
            "2:body\n3:\"ab\"\n3:comment \"split\"\n3:\"c\"\n2:comment \"tail\"\n1:comment \"end\""
        )
    );
    assert_eq!(result.mode(), DocumentMode::NoQuirks);
    assert_eq!(
        result
            .tree_diagnostics()
            .iter()
            .map(|d| d.kind)
            .collect::<Vec<_>>(),
        [D::UnexpectedNull]
    );
    assert_eq!(result.tokenizer_diagnostics().len(), 1);
}

#[test]
fn parser_feedback_text_and_plaintext_match_at_all_byte_splits_and_token_sizes() {
    let source = "<!doctype html><title>A&amp;<b>🚀</title><style>a<b>&amp;</style><script><!--<script>lit</script>--></script><body><plaintext>x<b>&amp;";
    let expected = "1:doctype \"html\"\n1:html\n2:head\n3:title\n4:\"A&<b>🚀\"\n3:style\n4:\"a<b>&amp;\"\n3:script\n4:\"<!--<script>lit</script>-->\"\n2:body\n3:plaintext\n4:\"x<b>&amp;\"";
    for token_bytes in [4, 7, 16384] {
        let mut limits = ParserLimits::default();
        limits.tokenizer.max_text_chunk_bytes = token_bytes;
        for split in 0..=source.len() {
            let mut parser = HtmlDocumentParser::new(limits, CancellationToken::default()).unwrap();
            parser.feed(&source.as_bytes()[..split]).unwrap();
            parser.feed(&source.as_bytes()[split..]).unwrap();
            let result = parser.finish().unwrap();
            assert_eq!(
                shape(&result),
                expected,
                "split {split}, chunk {token_bytes}"
            );
            assert_eq!(result.document().node_count(), 13);
        }
    }
}

#[test]
fn paragraph_recovery_closes_spans_and_blocks_without_moving_existing_nodes() {
    let result = parse(
        b"<p>one<span>x<div>two<p>three</div>four</p></br><hr/>",
        ParserLimits::default(),
    )
    .unwrap();
    assert_eq!(
        shape(&result),
        "1:html\n2:head\n2:body\n3:p\n4:\"one\"\n4:span\n5:\"x\"\n3:div\n4:\"two\"\n4:p\n5:\"three\"\n3:\"four\"\n3:p\n3:br\n3:hr"
    );
    assert_eq!(
        result
            .tree_diagnostics()
            .iter()
            .filter(|d| d.kind == D::UnexpectedToken)
            .count(),
        3
    );
}

#[test]
fn duplicate_root_body_and_attribute_values_preserve_first_value_and_order() {
    let result = parse(b"<!doctype html><html z=first><head/><body id=a><html z=second a=new><body id=b class=c><p x=1 X=2>", ParserLimits::default()).unwrap();
    let doc = result.document();
    for (name, expected) in [
        ("html", vec![("z", "first"), ("a", "new")]),
        ("body", vec![("id", "a"), ("class", "c")]),
        ("p", vec![("x", "1")]),
    ] {
        let ids = elements(doc, name);
        assert_eq!(ids.len(), 1);
        let NodeKind::Element(element) = doc.node(ids[0]).unwrap().kind() else {
            unreachable!()
        };
        let actual = element
            .attributes()
            .iter()
            .map(|a| {
                assert_eq!(a.name().namespace(), None);
                (
                    a.name().local().to_owned(),
                    String::from_utf16(a.value().as_units()).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            expected
                .into_iter()
                .map(|(a, b)| (a.to_owned(), b.to_owned()))
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(
        result
            .tree_diagnostics()
            .iter()
            .filter(|d| d.kind == D::UnacknowledgedSelfClosing)
            .count(),
        1
    );
}

#[test]
fn self_closing_acknowledgement_includes_ignored_tags_and_keeps_nonvoid_open() {
    let result = parse(
        b"<html/><head/><head/><body/><head/><html/><body/><p/>x<br/>y<hr/>",
        ParserLimits::default(),
    )
    .unwrap();
    assert_eq!(
        result
            .tree_diagnostics()
            .iter()
            .filter(|d| d.kind == D::UnacknowledgedSelfClosing)
            .count(),
        8
    );
    assert!(shape(&result).contains("3:p\n4:\"x\"\n4:br\n4:\"y\"\n3:hr"));
}

#[test]
fn scripting_disabled_noscript_and_late_head_elements_use_actual_dom_parents() {
    let result = parse(b"<!doctype html><head><noscript> <link href=x><style>x<b></style></noscript></head> <title>T</title><meta charset=windows-1252><body>tail", ParserLimits::default()).unwrap();
    assert_eq!(
        shape(&result),
        "1:doctype \"html\"\n1:html\n2:head\n3:noscript\n4:\" \"\n4:link\n4:style\n5:\"x<b>\"\n3:title\n4:\"T\"\n3:meta\n2:\" \"\n2:body\n3:\"tail\""
    );
    let recovered = parse(b"<head><noscript><p>body", ParserLimits::default()).unwrap();
    assert_eq!(
        shape(&recovered),
        "1:html\n2:head\n3:noscript\n2:body\n3:p\n4:\"body\""
    );
}

#[test]
fn content_after_body_and_html_uses_recovery_and_preserves_comment_placement() {
    let result = parse(
        b"<body>x</body> <!--html-->y</html><!--doc-->z",
        ParserLimits::default(),
    )
    .unwrap();
    assert_eq!(
        shape(&result),
        "1:html\n2:head\n2:body\n3:\"x yz\"\n2:comment \"html\"\n1:comment \"doc\""
    );
}

#[test]
fn compatibility_modes_cover_legacy_ids_case_and_missing_vs_empty_system_id() {
    for (source, mode) in [
        ("<!doctype html>", DocumentMode::NoQuirks),
        (
            "<!doctype html system 'about:legacy-compat'>",
            DocumentMode::NoQuirks,
        ),
        ("<!doctype html system ''>", DocumentMode::NoQuirks),
        ("<!doctype html public 'HTML'>", DocumentMode::Quirks),
        (
            "<!doctype HTML public '-//ietf//dtd html 2.0//EN'>",
            DocumentMode::Quirks,
        ),
        (
            "<!doctype html public '-//W3C//DTD HTML 4.01 Transitional//EN'>",
            DocumentMode::Quirks,
        ),
        (
            "<!doctype html public '-//W3C//DTD HTML 4.01 Transitional//EN' ''>",
            DocumentMode::Quirks,
        ),
        (
            "<!doctype html public '-//W3C//DTD HTML 4.01 Transitional//EN' 'x'>",
            DocumentMode::LimitedQuirks,
        ),
        (
            "<!doctype html public '-//W3C//DTD XHTML 1.0 Frameset//EN'>",
            DocumentMode::LimitedQuirks,
        ),
        (
            "<!doctype html system 'HTTP://WWW.IBM.COM/data/dtd/v11/ibmxhtml1-transitional.dtd'>",
            DocumentMode::Quirks,
        ),
        ("<!doctype other>", DocumentMode::Quirks),
        ("<!doctype html wat>", DocumentMode::Quirks),
        ("<!doctype html public '未知'>", DocumentMode::NoQuirks),
    ] {
        assert_eq!(
            parse(source.as_bytes(), ParserLimits::default())
                .unwrap()
                .mode(),
            mode,
            "{source}"
        );
    }
}

#[test]
fn unsupported_constructs_drop_partial_documents_and_poison_the_stream() {
    for (source, unsupported) in [
        ("<?target value?>", U::ProcessingInstructions),
        ("<body><table><b><?target?>", U::ProcessingInstructions),
        ("<svg><?target?>", U::ProcessingInstructions),
    ] {
        let mut parser = HtmlDocumentParser::default();
        assert_eq!(
            parser.feed(source.as_bytes()),
            Err(ParseError::Unsupported(unsupported))
        );
        assert_eq!(parser.feed(b""), Err(ParseError::Poisoned));
        assert_eq!(parser.finish().unwrap_err(), ParseError::Poisoned);
    }
}

#[test]
fn exact_node_depth_and_utf16_payload_limits_include_implied_nodes_and_text_merges() {
    let source = "<!doctype html><p>🚀&amp;x";
    let expected = parse(source.as_bytes(), ParserLimits::default()).unwrap();
    let mut exact = ParserLimits::default();
    exact.document.max_nodes = expected.document().node_count();
    exact.document.max_depth = 4;
    exact.document.max_data_bytes = expected.document().data_bytes();
    exact.tokenizer.max_text_chunk_bytes = 4;
    assert_eq!(
        shape(&parse(source.as_bytes(), exact).unwrap()),
        shape(&expected)
    );
    for (field, error) in [
        (0, DomError::NodeLimit),
        (1, DomError::DepthLimit),
        (2, DomError::DataLimit),
    ] {
        let mut limits = exact;
        match field {
            0 => limits.document.max_nodes -= 1,
            1 => limits.document.max_depth -= 1,
            _ => limits.document.max_data_bytes -= 1,
        }
        assert_eq!(
            parse(source.as_bytes(), limits).unwrap_err(),
            ParseError::Document(error)
        );
    }
    // The late-head stack is shorter than the DOM path: enforce actual DOM depth.
    exact.document.max_nodes = 32;
    exact.document.max_data_bytes = 1024;
    exact.document.max_depth = 3;
    assert_eq!(
        parse(b"<head></head><title>x", exact).unwrap_err(),
        ParseError::Document(DomError::DepthLimit)
    );
}

#[test]
fn diagnostics_have_separate_exact_budgets_and_stable_delivery_indexes() {
    let mut limits = ParserLimits {
        max_tree_diagnostics: 1,
        ..ParserLimits::default()
    };
    let result = parse(b"", limits).unwrap();
    assert_eq!(result.tree_diagnostics()[0].token_index, 0);
    assert_eq!(result.dropped_diagnostics(), 0);
    // Exceeding a retention limit keeps parsing and counts the rest (TD-01).
    limits.max_tree_diagnostics = 0;
    let result = parse(b"", limits).unwrap();
    assert!(result.tree_diagnostics().is_empty());
    assert_eq!(result.dropped_diagnostics(), 1);
    assert_eq!(
        parse(b"<!doctype html>", limits)
            .unwrap()
            .dropped_diagnostics(),
        0
    );
    limits.max_tree_diagnostics = 8;
    limits.tokenizer.max_diagnostics = 0;
    let result = parse(b"<p>\0", limits).unwrap();
    assert!(result.tokenizer_diagnostics().is_empty());
    assert!(result.dropped_diagnostics() >= 1);
    let source = b"<!doctype html><body>\0\0";
    limits.tokenizer.max_diagnostics = 2;
    limits.max_tree_diagnostics = 2;
    let result = parse(source, limits).unwrap();
    assert_eq!(result.tree_diagnostics().len(), 2);
    assert_eq!(
        result.tree_diagnostics()[0].token_index,
        result.tree_diagnostics()[1].token_index
    );
    limits.max_tree_diagnostics = 1;
    let truncated = parse(source, limits).unwrap();
    assert_eq!(
        truncated.tree_diagnostics(),
        &result.tree_diagnostics()[..1]
    );
    assert_eq!(
        truncated.dropped_diagnostics(),
        result.dropped_diagnostics() + 1
    );
    // The tree never depends on how many diagnostics were retained.
    assert_eq!(
        truncated
            .document()
            .dump_tree(truncated.document().root(), 1 << 16),
        result
            .document()
            .dump_tree(result.document().root(), 1 << 16)
    );
}

#[test]
fn error_heavy_documents_load_with_default_limits() {
    // Far more recoverable errors than the default retention limits.
    let source = "<p>\0</b>".repeat(3000);
    let result = parse(source.as_bytes(), ParserLimits::default()).unwrap();
    assert_eq!(result.tokenizer_diagnostics().len(), 1024);
    assert_eq!(result.tree_diagnostics().len(), 1024);
    assert!(result.dropped_diagnostics() > 3000);
}

#[test]
fn input_token_limits_cancellation_and_successful_lifecycle_are_distinct() {
    let mut limits = ParserLimits::default();
    limits.tokenizer.max_input_bytes = 1;
    let mut parser = HtmlDocumentParser::new(limits, CancellationToken::default()).unwrap();
    assert_eq!(
        parser.feed(b"xx"),
        Err(ParseError::Tokenizer(HtmlError::InputLimit))
    );
    assert_eq!(parser.finish().unwrap_err(), ParseError::Poisoned);
    limits.tokenizer.max_input_bytes = 64;
    limits.tokenizer.max_tokens = 1;
    assert_eq!(
        parse(b"x", limits).unwrap_err(),
        ParseError::Tokenizer(HtmlError::TokenLimit)
    );
    let cancel = CancellationToken::default();
    let mut parser = HtmlDocumentParser::new(ParserLimits::default(), cancel.clone()).unwrap();
    parser.feed(b"<head><title>").unwrap();
    cancel.cancel();
    assert_eq!(
        parser.feed(b""),
        Err(ParseError::Tokenizer(HtmlError::Cancelled))
    );
    assert_eq!(parser.finish().unwrap_err(), ParseError::Poisoned);
    let mut parser = HtmlDocumentParser::default();
    let result = parser.finish().unwrap();
    assert_eq!(parser.feed(b""), Err(ParseError::AlreadyFinished));
    assert_eq!(parser.finish().unwrap_err(), ParseError::AlreadyFinished);
    let mut doc = result.into_document();
    assert!(doc.create_comment("caller owns completed document").is_ok());
}

#[test]
fn bom_crlf_invalid_utf8_and_unclosed_text_have_explicit_recovery() {
    let result = parse(b"\xef\xbb\xbf<title>\r\n\xff\0", ParserLimits::default()).unwrap();
    assert_eq!(
        shape(&result),
        "1:html\n2:head\n3:title\n4:\"\\n��\"\n2:body"
    );
    assert!(
        result
            .tree_diagnostics()
            .iter()
            .any(|d| d.kind == D::EofInText)
    );
    assert_eq!(elements(result.document(), "body").len(), 1);
}

#[test]
fn deep_construction_and_completion_remain_iterative_and_obey_actual_edge_depth() {
    let source = format!(
        "<!doctype html>{}x{}",
        "<span>".repeat(700),
        "</span>".repeat(700)
    );
    let mut limits = ParserLimits::default();
    limits.document.max_depth = 703;
    limits.document.max_nodes = 706;
    let mut parser = HtmlDocumentParser::new(limits, CancellationToken::default()).unwrap();
    for chunk in source.as_bytes().chunks(13) {
        parser.feed(chunk).unwrap();
    }
    let result = parser.finish().unwrap();
    assert!(result.tree_diagnostics().is_empty());
    let doc = result.document();
    assert_eq!(doc.node_count(), 706);
    assert_eq!(doc.walk(doc.root()).unwrap().count(), 706);
    assert_eq!(
        doc.walk(doc.root()).unwrap().map(|(_, depth)| depth).max(),
        Some(703)
    );
    limits.document.max_depth -= 1;
    assert_eq!(
        parse(source.as_bytes(), limits).unwrap_err(),
        ParseError::Document(DomError::DepthLimit)
    );
}

#[test]
fn later_doctypes_are_ignored_without_changing_the_initial_document_mode() {
    for (source, mode, doctypes) in [
        ("<body>x<!doctype html>", DocumentMode::Quirks, 0),
        (
            "<!doctype html><head><!doctype other><body>",
            DocumentMode::NoQuirks,
            1,
        ),
        (
            "<!doctype html public '-//W3C//DTD XHTML 1.0 Transitional//EN'><body></html><!doctype html>",
            DocumentMode::LimitedQuirks,
            1,
        ),
    ] {
        let result = parse(source.as_bytes(), ParserLimits::default()).unwrap();
        assert_eq!(result.mode(), mode);
        let doc = result.document();
        assert_eq!(
            doc.walk(doc.root())
                .unwrap()
                .filter(|(id, _)| matches!(
                    doc.node(*id).unwrap().kind(),
                    NodeKind::DocumentType(_)
                ))
                .count(),
            doctypes
        );
    }
}
