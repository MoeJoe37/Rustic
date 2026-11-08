//! Focused HTML fragment parsing behavior beyond the html5lib fragment records.

use rustic_base::CancellationToken;
use rustic_dom::{DomError, NodeKind, SVG_NAMESPACE};
use rustic_html::{
    ContextNamespace, FragmentContext, HtmlError, HtmlFragmentParser, ParseError, ParsedFragment,
    ParserLimits,
};

fn parse_with(context: &FragmentContext, chunks: &[&str]) -> Result<ParsedFragment, ParseError> {
    let mut parser = HtmlFragmentParser::new(context, ParserLimits::default(), Default::default())?;
    for chunk in chunks {
        parser.feed(chunk.as_bytes())?;
    }
    parser.finish()
}

/// Compact preorder outline of the fragment: `name` / `svg:name` / `"text"`.
fn outline(result: &ParsedFragment) -> Vec<String> {
    let doc = result.document();
    doc.walk(result.root())
        .unwrap()
        .skip(1)
        .map(|(id, depth)| {
            let indent = "  ".repeat(depth - 1);
            match doc.node(id).unwrap().kind() {
                NodeKind::Element(e) if e.name().namespace() == Some(SVG_NAMESPACE) => {
                    format!("{indent}svg:{}", e.name().local())
                }
                NodeKind::Element(e) => format!("{indent}{}", e.name().local()),
                NodeKind::Text(text) => {
                    format!(
                        "{indent}\"{}\"",
                        String::from_utf16(text.as_units()).unwrap()
                    )
                }
                NodeKind::Comment(_) => format!("{indent}<!---->"),
                other => format!("{indent}{other:?}"),
            }
        })
        .collect()
}

fn fragment(context: &FragmentContext, source: &str) -> Vec<String> {
    outline(&parse_with(context, &[source]).unwrap())
}

#[test]
fn context_selects_tokenizer_state() {
    let markup = "<b>&amp;</title>";
    assert_eq!(
        fragment(&FragmentContext::html("div"), markup),
        ["b", "  \"&\""]
    );
    // RCDATA keeps markup as text, still decodes references, and the
    // context's own end tag is not appropriate (no last start tag).
    assert_eq!(
        fragment(&FragmentContext::html("TITLE"), markup),
        ["\"<b>&</title>\""]
    );
    assert_eq!(
        fragment(&FragmentContext::html("style"), markup),
        ["\"<b>&amp;</title>\""]
    );
    assert_eq!(
        fragment(&FragmentContext::html("plaintext"), "</plaintext><p>"),
        ["\"</plaintext><p>\""]
    );
    // Scripting is disabled, so noscript content is parsed as markup.
    assert_eq!(
        fragment(&FragmentContext::html("noscript"), "<p>x"),
        ["p", "  \"x\""]
    );
}

#[test]
fn context_element_and_shell_are_not_in_the_result() {
    let result = parse_with(&FragmentContext::html("body"), &["a<p>b"]).unwrap();
    assert_eq!(outline(&result), ["\"a\"", "p", "  \"b\""]);
    let doc = result.document();
    // Document root, scratch html root, text, p and text: no context node.
    assert_eq!(doc.walk(doc.root()).unwrap().count(), 5);
    assert_eq!(doc.node(result.root()).unwrap().parent(), Some(doc.root()));
}

#[test]
fn table_contexts_reset_the_insertion_mode() {
    assert_eq!(
        fragment(&FragmentContext::html("tr"), "<td>a<td>b"),
        ["td", "  \"a\"", "td", "  \"b\""]
    );
    // A td context is a cell only when it is not the last stack entry.
    assert_eq!(fragment(&FragmentContext::html("td"), "<td>x"), ["\"x\""]);
    assert_eq!(
        fragment(&FragmentContext::html("table"), "x<tr>"),
        ["\"x\"", "tbody", "  tr"]
    );
}

#[test]
fn foreign_contexts_use_the_adjusted_current_node() {
    let svg = FragmentContext::svg("svg");
    assert_eq!(svg.namespace(), ContextNamespace::Svg);
    assert_eq!(
        fragment(&svg, "<path/><foreignobject>"),
        ["svg:path", "svg:foreignObject"]
    );
    // CDATA is only recognized when the adjusted current node is foreign.
    assert_eq!(fragment(&svg, "<![CDATA[a<b]]>"), ["\"a<b\""]);
    assert_eq!(
        fragment(&FragmentContext::html("div"), "<![CDATA[a]]>"),
        ["<!---->"]
    );
    // Breakout elements leave foreign content and use HTML rules.
    assert_eq!(fragment(&svg, "<p>x"), ["p", "  \"x\""]);
    // An HTML integration point context parses HTML.
    assert_eq!(fragment(&FragmentContext::svg("desc"), "<b>"), ["b"]);
    let plain = FragmentContext::mathml("annotation-xml");
    assert_eq!(outline(&parse_with(&plain, &["<div>"]).unwrap()), ["div"]);
    let html = FragmentContext::mathml("annotation-xml").with_attribute("ENCODING", "text/html");
    assert_eq!(fragment(&html, "<div>"), ["div"]);
    assert_eq!(
        fragment(&FragmentContext::mathml("mi"), "<b>x"),
        ["b", "  \"x\""]
    );
}

#[test]
fn template_select_and_form_contexts_follow_fragment_rules() {
    assert_eq!(
        fragment(&FragmentContext::html("template"), "<td>a"),
        ["td", "  \"a\""]
    );
    assert_eq!(
        fragment(&FragmentContext::html("select"), "<input><select><option>a"),
        ["option", "  \"a\""]
    );
    // A form context is the form element pointer, so nested forms are ignored.
    assert_eq!(fragment(&FragmentContext::html("form"), "<form><p>"), ["p"]);
    assert_eq!(
        fragment(&FragmentContext::html("div"), "<form><p>"),
        ["form", "  p"]
    );
    // `</html>` and frameset end tags never leave the fragment's root.
    assert_eq!(
        fragment(&FragmentContext::html("body"), "a</body></html>b"),
        ["\"ab\""]
    );
    assert_eq!(
        fragment(
            &FragmentContext::html("html"),
            "<frameset><frame></frameset><p>"
        ),
        ["head", "frameset", "  frame"]
    );
}

#[test]
fn feed_boundaries_do_not_change_fragments() {
    let source = "<table><tr><td>a<svg><desc><b>x</b></desc></svg>&amp;<!-- c -->";
    let context = FragmentContext::html("div");
    let whole = fragment(&context, source);
    for split in 0..=source.len() {
        if !source.is_char_boundary(split) {
            continue;
        }
        let (a, b) = source.split_at(split);
        assert_eq!(outline(&parse_with(&context, &[a, b]).unwrap()), whole);
    }
}

#[test]
fn budgets_count_the_context_and_failures_poison() {
    let mut limits = ParserLimits::default();
    // Document root, scratch html and detached context element.
    limits.document.max_nodes = 3;
    let context = FragmentContext::html("div");
    let mut parser = HtmlFragmentParser::new(&context, limits, Default::default()).unwrap();
    assert!(parser.finish().is_ok());
    let mut parser = HtmlFragmentParser::new(&context, limits, Default::default()).unwrap();
    assert_eq!(
        parser.feed(b"x").and_then(|()| parser.finish().map(drop)),
        Err(ParseError::Document(DomError::NodeLimit))
    );
    assert_eq!(parser.feed(b"y"), Err(ParseError::Poisoned));
    limits.document.max_nodes = 2;
    assert_eq!(
        HtmlFragmentParser::new(&context, limits, Default::default()).map(drop),
        Err(ParseError::Document(DomError::NodeLimit))
    );

    let mut parser =
        HtmlFragmentParser::new(&context, ParserLimits::default(), Default::default()).unwrap();
    parser.finish().unwrap();
    assert_eq!(parser.feed(b"z"), Err(ParseError::AlreadyFinished));
    assert!(matches!(parser.finish(), Err(ParseError::AlreadyFinished)));

    let cancellation = CancellationToken::default();
    let mut parser =
        HtmlFragmentParser::new(&context, ParserLimits::default(), cancellation.clone()).unwrap();
    cancellation.cancel();
    assert_eq!(
        parser.feed(b"<p>"),
        Err(ParseError::Tokenizer(HtmlError::Cancelled))
    );
    assert!(
        FragmentContext::html("div")
            .with_attribute("a", "1")
            .with_attribute("A", "2")
            == FragmentContext::html("DIV").with_attribute("a", "1")
    );
    assert!(
        HtmlFragmentParser::new(
            &FragmentContext::html(""),
            ParserLimits::default(),
            Default::default()
        )
        .is_err()
    );
}
