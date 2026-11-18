use rustic_base::CancellationToken;
use rustic_dom::{Document, HTML_NAMESPACE, NodeId, NodeKind};
use rustic_html::{
    DocumentMode, HtmlDocumentParser, HtmlError, ParseError, ParsedDocument, ParserLimits,
    Scripting, TreeDiagnosticKind as D,
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
            NodeKind::Document
            | NodeKind::DocumentFragment
            | NodeKind::ProcessingInstruction(_)
            | NodeKind::CdataSection(_) => unreachable!("not produced by the HTML parser"),
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
fn script_boundaries_precede_later_markup_at_every_split() {
    let source = "<script>const text = 'é';</script><body><template><script>inert</script></template><svg><script/></svg><script>second</script><p>later<script>unfinished";
    let bytes = source.as_bytes();

    for split in 0..=bytes.len() {
        let mut parser = HtmlDocumentParser::new_with_scripting(
            Default::default(),
            Default::default(),
            Scripting::Enabled,
        )
        .unwrap();
        let mut scripts = Vec::new();
        for chunk in [&bytes[..split], &bytes[split..]] {
            parser.write(chunk).unwrap();
            while let Some(boundary) = parser.advance_to_script().unwrap() {
                let doc = boundary.document;
                let child = doc.node(boundary.element).unwrap().first_child().unwrap();
                let NodeKind::Text(text) = doc.node(child).unwrap().kind() else {
                    panic!("script child must be text");
                };
                assert!(elements(doc, "p").is_empty(), "split {split}");
                assert_eq!(elements(doc, "body").len(), scripts.len(), "split {split}");
                scripts.push(String::from_utf16(text.as_units()).unwrap());
            }
        }
        parser.close_input().unwrap();
        assert!(parser.advance_to_script().unwrap().is_none());
        assert_eq!(scripts, ["const text = 'é';", "second"], "split {split}");
        let parsed = parser.finish().unwrap();
        assert_eq!(elements(parsed.document(), "p").len(), 1);
    }
}

#[test]
fn closed_input_can_resume_and_cancellation_discards_the_partial_tree() {
    let token = CancellationToken::default();
    let mut parser = HtmlDocumentParser::new_with_scripting(
        Default::default(),
        token.clone(),
        Scripting::Enabled,
    )
    .unwrap();
    parser.write(b"<script></script><script></script>").unwrap();
    parser.close_input().unwrap();
    assert_eq!(parser.write(b"x"), Err(ParseError::InputClosed));
    assert_eq!(parser.close_input(), Err(ParseError::InputClosed));
    assert!(parser.advance_to_script().unwrap().is_some());
    token.cancel();
    assert!(matches!(
        parser.advance_to_script(),
        Err(ParseError::Tokenizer(HtmlError::Cancelled))
    ));
    assert!(matches!(parser.finish(), Err(ParseError::Poisoned)));

    let mut inert = HtmlDocumentParser::default();
    inert.write(b"<script></script>").unwrap();
    assert!(inert.advance_to_script().unwrap().is_none());
    let parsed = inert.finish().unwrap();
    assert_eq!(elements(parsed.document(), "script").len(), 1);
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
fn processing_instructions_preserve_data_and_use_tree_insertion_rules() {
    let source = concat!(
        "<?before?><!doctype html><html><?html?><head><noscript><?noscript?></noscript></head>",
        "<body><table><?table data?></table><svg><?foreign α??></svg>",
        "<template><?template nul\0?></template></body><?afterBody?></html><?afterHtml?>"
    );
    let parsed = parse(source.as_bytes(), ParserLimits::default()).unwrap();
    let doc = parsed.document();
    let template = elements(doc, "template")[0];
    let contents = match doc.node(template).unwrap().kind() {
        NodeKind::Element(element) => element.template_contents().unwrap(),
        _ => unreachable!(),
    };
    let actual = doc
        .walk(doc.root())
        .unwrap()
        .chain(doc.walk(contents).unwrap())
        .filter_map(|(node, _)| {
            let node = doc.node(node).unwrap();
            let NodeKind::ProcessingInstruction(value) = node.kind() else {
                return None;
            };
            let parent = match doc.node(node.parent().unwrap()).unwrap().kind() {
                NodeKind::Document => "#document",
                NodeKind::DocumentFragment => "#fragment",
                NodeKind::Element(element) => element.name().local(),
                _ => unreachable!(),
            };
            Some((
                String::from_utf16(value.target().as_units()).unwrap(),
                parent,
                String::from_utf16(value.data().as_units()).unwrap(),
            ))
        })
        .collect::<Vec<_>>();
    let actual = actual
        .iter()
        .map(|(target, parent, data)| (target.as_str(), *parent, data.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        [
            ("before", "#document", ""),
            ("html", "html", ""),
            ("noscript", "noscript", ""),
            ("table", "table", "data"),
            ("foreign", "svg", "α?"),
            ("afterBody", "html", ""),
            ("afterHtml", "#document", ""),
            ("template", "#fragment", "nul\0"),
        ]
    );
}

#[test]
fn node_limit_truncates_the_document_and_ignores_later_input() {
    let mut limits = ParserLimits::default();
    limits.document.max_nodes = 1;
    let mut parser = HtmlDocumentParser::new(limits, CancellationToken::default()).unwrap();
    parser.feed(b"<?target value?>").unwrap();
    parser.feed(b"<p>ignored").unwrap();
    let result = parser.finish().unwrap();
    assert!(result.truncated());
    assert_eq!(result.document().node_count(), 1);
    assert_eq!(parser.finish().unwrap_err(), ParseError::AlreadyFinished);
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
    // One node or byte less: the document keeps what was built before the
    // budget ran out, and says so (ADR 0046).
    for field in 0..2 {
        let mut limits = exact;
        match field {
            0 => limits.document.max_nodes -= 1,
            _ => limits.document.max_data_bytes -= 1,
        }
        let truncated = parse(source.as_bytes(), limits).unwrap();
        assert!(truncated.truncated());
        // The payload budget still fits the first text chunk.
        let text = if field == 0 { "" } else { "\n4:\"🚀\"" };
        assert_eq!(
            shape(&truncated),
            format!("1:doctype \"html\"\n1:html\n2:head\n2:body\n3:p{text}")
        );
        assert_eq!(
            truncated.tree_diagnostics().last().map(|d| d.kind),
            Some(rustic_html::TreeDiagnosticKind::Truncated)
        );
    }
    assert!(!parse(source.as_bytes(), exact).unwrap().truncated());
    // One edge less: the paragraph, which keeps an edge free for its text,
    // becomes the body's sibling instead of failing the parse (ADR 0046).
    let mut limits = exact;
    limits.document.max_depth -= 1;
    let capped = parse(source.as_bytes(), limits).unwrap();
    assert_eq!(
        shape(&capped),
        "1:doctype \"html\"\n1:html\n2:head\n2:body\n2:p\n3:\"🚀&x\""
    );
    assert!(
        capped
            .tree_diagnostics()
            .iter()
            .any(|d| d.kind == rustic_html::TreeDiagnosticKind::NestingTooDeep)
    );
    // The late-head stack is shorter than the DOM path: the cap follows the
    // actual DOM depth.
    exact.document.max_nodes = 32;
    exact.document.max_data_bytes = 1024;
    exact.document.max_depth = 3;
    let late = parse(b"<head></head><title>x", exact).unwrap();
    assert_eq!(shape(&late), "1:html\n2:head\n2:title\n3:\"x\"\n2:body");
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
    // One edge less: the innermost span becomes its parent's sibling and
    // its text stays inside it, as in Blink (ADR 0046).
    limits.document.max_depth -= 1;
    let capped = parse(source.as_bytes(), limits).unwrap();
    let doc = capped.document();
    assert_eq!(doc.node_count(), 706);
    assert_eq!(
        doc.walk(doc.root()).unwrap().map(|(_, depth)| depth).max(),
        Some(702)
    );
    assert!(
        capped
            .tree_diagnostics()
            .iter()
            .any(|d| d.kind == rustic_html::TreeDiagnosticKind::NestingTooDeep)
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

/// `depth:label` lines in the format of `tools/html_depth_reference.js`.
fn reference_shape(result: &ParsedDocument) -> String {
    let doc = result.document();
    let mut lines = Vec::new();
    for (id, depth) in doc.walk(doc.root()).unwrap().skip(1) {
        let label = match doc.node(id).unwrap().kind() {
            NodeKind::Element(element) => element.name().local().to_owned(),
            NodeKind::Text(value) => format!("{:?}", String::from_utf16(value.as_units()).unwrap()),
            NodeKind::Comment(value) => format!(
                "comment {:?}",
                String::from_utf16(value.as_units()).unwrap()
            ),
            NodeKind::DocumentType(_) => "doctype".to_owned(),
            other => panic!("unexpected node {other:?}"),
        };
        lines.push(format!("{depth}:{label}"));
    }
    lines.join("\n")
}

#[test]
fn depth_cap_places_nodes_like_chromium() {
    // Chromium caps elements at 513 edges and lets text go one deeper;
    // a 514-edge document limit reproduces its trees exactly (ADR 0046).
    let fixture = include_str!("fixtures/depth-cap-chromium.txt");
    let mut limits = ParserLimits::default();
    limits.document.max_depth = 514;
    limits.document.max_nodes = 10_000;
    let mut cases = 0;
    for case in fixture.split("#case\n").skip(1) {
        let (html, expected) = case.split_once("\n#chromium\n").unwrap();
        let result = parse(html.as_bytes(), limits).unwrap();
        assert!(!result.truncated());
        let actual = reference_shape(&result);
        let first_difference = actual
            .lines()
            .zip(expected.lines())
            .position(|(a, b)| a != b);
        assert_eq!(first_difference, None, "case {cases}");
        assert_eq!(actual, expected.trim_end(), "case {cases}");
        cases += 1;
    }
    assert_eq!(cases, 5);
}
