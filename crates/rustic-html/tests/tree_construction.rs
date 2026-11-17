//! Complete html5lib tree-construction documents, run against the pinned corpus.
//!
//! Document and fragment records run in their selected scripting mode.
//! The pinned corpus predates HTML processing instructions. Exact updates for
//! affected records are listed in `CURRENT_HTML_TREES`. Expected failures
//! must be listed explicitly in `KNOWN_FAILURES` with a reason.

use std::fmt::Write;

use rustic_dom::{Document, DomString, NodeId, NodeKind};
use rustic_html::{FragmentContext, HtmlDocumentParser, HtmlFragmentParser, Scripting};

macro_rules! corpus {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("fixtures/tree/", $name)))),*]
    };
}

/// html5lib/html5lib-tests 9329e64694e7835d0dcff9811e22856ef6ad16f9,
/// `tree-construction/*.dat`; MIT notice: fixtures/HTML5LIB-LICENSE.md.
const FILES: &[(&str, &str)] = corpus![
    "adoption01.dat",
    "adoption02.dat",
    "blocks.dat",
    "comments01.dat",
    "doctype01.dat",
    "domjs-unsafe.dat",
    "entities01.dat",
    "entities02.dat",
    "foreign-fragment.dat",
    "html5test-com.dat",
    "inbody01.dat",
    "isindex.dat",
    "main-element.dat",
    "math.dat",
    "menuitem-element.dat",
    "namespace-sensitivity.dat",
    "noscript01.dat",
    "pending-spec-changes-plain-text-unsafe.dat",
    "pending-spec-changes.dat",
    "plain-text-unsafe.dat",
    "quirks01.dat",
    "ruby.dat",
    "scriptdata01.dat",
    "search-element.dat",
    "svg.dat",
    "tables01.dat",
    "template.dat",
    "tests1.dat",
    "tests10.dat",
    "tests11.dat",
    "tests12.dat",
    "tests14.dat",
    "tests15.dat",
    "tests16.dat",
    "tests17.dat",
    "tests18.dat",
    "tests19.dat",
    "tests2.dat",
    "tests20.dat",
    "tests21.dat",
    "tests22.dat",
    "tests23.dat",
    "tests24.dat",
    "tests25.dat",
    "tests26.dat",
    "tests3.dat",
    "tests4.dat",
    "tests5.dat",
    "tests6.dat",
    "tests7.dat",
    "tests8.dat",
    "tests9.dat",
    "tests_innerHTML_1.dat",
    "tricky01.dat",
    "void-in-phrasing.dat",
    "webkit01.dat",
    "webkit02.dat",
];

/// `(file, zero-based record index, reason)` for records expected to differ.
const KNOWN_FAILURES: &[(&str, usize, &str)] = &[];

/// Trees changed by the current HTML processing-instruction tokenizer rules.
/// The remaining four formerly excluded records still recover as comments.
const CURRENT_HTML_TREES: &[(&str, usize, &str)] = &[
    (
        "html5test-com.dat",
        11,
        "| <?import namespace=\"foo\" implementation=\"#bar\"?>\n| <html>\n|   <head>\n|   <body>\n",
    ),
    ("tests1.dat", 39, "| <html>\n|   <head>\n|   <body>\n"),
    (
        "tests1.dat",
        43,
        "| <?COMMENT?>\n| <html>\n|   <head>\n|   <body>\n",
    ),
    (
        "tests1.dat",
        46,
        "| <?COM--MENT?>\n| <html>\n|   <head>\n|   <body>\n",
    ),
];

struct Record {
    data: String,
    document: String,
    /// `#document-fragment` context, such as `td` or `svg path`.
    context: Option<String>,
    script_on: bool,
}

fn records(source: &str) -> Vec<Record> {
    let headers = [
        "#errors",
        "#new-errors",
        "#document",
        "#document-fragment",
        "#script-on",
        "#script-off",
    ];
    let mut result = Vec::new();
    for block in source.split("#data\n").skip(1) {
        let mut current = "#data";
        let mut data = String::new();
        let mut document = String::new();
        let mut context = None;
        let mut script_on = false;
        for line in block.split_inclusive('\n') {
            let header = line.trim_end_matches('\n');
            if headers.contains(&header) {
                current = header;
                script_on |= header == "#script-on";
                continue;
            }
            match current {
                "#data" => data.push_str(line),
                "#document" => document.push_str(line),
                "#document-fragment" => context = Some(header.to_owned()),
                _ => {}
            }
        }
        data.truncate(data.strip_suffix('\n').unwrap_or(&data).len());
        let document = format!("{}\n", document.trim_end_matches('\n'));
        result.push(Record {
            data,
            document,
            context,
            script_on,
        });
    }
    result
}

fn utf8(value: &DomString) -> String {
    String::from_utf16_lossy(value.as_units())
}

fn namespace_prefix(namespace: Option<&str>) -> Option<&'static str> {
    match namespace {
        Some("http://www.w3.org/2000/svg") => Some("svg"),
        Some("http://www.w3.org/1998/Math/MathML") => Some("math"),
        Some("http://www.w3.org/1999/xlink") => Some("xlink"),
        Some("http://www.w3.org/XML/1998/namespace") => Some("xml"),
        Some("http://www.w3.org/2000/xmlns/") => Some("xmlns"),
        _ => None,
    }
}

/// Serialize in the html5lib tree format, including template contents,
/// with an explicit stack instead of recursion.
fn serialize(document: &Document, parent: NodeId) -> String {
    let mut output = String::new();
    let mut stack: Vec<(NodeId, usize)> = document
        .children(parent)
        .unwrap()
        .map(|id| (id, 0))
        .collect();
    stack.reverse();
    while let Some((id, depth)) = stack.pop() {
        let prefix = format!("| {}", "  ".repeat(depth));
        let node = document.node(id).unwrap();
        match node.kind() {
            NodeKind::Element(element) => {
                let name = element.name();
                match namespace_prefix(name.namespace()) {
                    Some(ns) => writeln!(output, "{prefix}<{ns} {}>", name.local()).unwrap(),
                    None => writeln!(output, "{prefix}<{}>", name.local()).unwrap(),
                }
                let mut attributes = element
                    .attributes()
                    .iter()
                    .map(|a| {
                        let name = match namespace_prefix(a.name().namespace()) {
                            Some(ns) => format!("{ns} {}", a.name().local()),
                            None => a.name().local().to_owned(),
                        };
                        (name, utf8(a.value()))
                    })
                    .collect::<Vec<_>>();
                attributes.sort();
                for (name, value) in attributes {
                    writeln!(output, "{prefix}  {name}=\"{value}\"").unwrap();
                }
                let mut children = document
                    .children(id)
                    .unwrap()
                    .map(|child| (child, depth + 1))
                    .collect::<Vec<_>>();
                if let Some(contents) = element.template_contents() {
                    writeln!(output, "{prefix}  content").unwrap();
                    children = document
                        .children(contents)
                        .unwrap()
                        .map(|child| (child, depth + 2))
                        .chain(children)
                        .collect();
                }
                stack.extend(children.into_iter().rev());
            }
            NodeKind::Text(value) => writeln!(output, "{prefix}\"{}\"", utf8(value)).unwrap(),
            NodeKind::Comment(value) => {
                writeln!(output, "{prefix}<!-- {} -->", utf8(value)).unwrap()
            }
            NodeKind::ProcessingInstruction(value) => {
                write!(output, "{prefix}<?{}", utf8(value.target())).unwrap();
                if !value.data().as_units().is_empty() {
                    write!(output, " {}", utf8(value.data())).unwrap();
                }
                writeln!(output, "?>").unwrap();
            }
            NodeKind::DocumentType(value) => {
                let (name, public, system) = (
                    utf8(&value.name),
                    utf8(&value.public_id),
                    utf8(&value.system_id),
                );
                if public.is_empty() && system.is_empty() {
                    writeln!(output, "{prefix}<!DOCTYPE {name}>").unwrap();
                } else {
                    writeln!(
                        output,
                        "{prefix}<!DOCTYPE {name} \"{public}\" \"{system}\">"
                    )
                    .unwrap();
                }
            }
            NodeKind::Document | NodeKind::DocumentFragment | NodeKind::CdataSection(_) => {
                unreachable!("not produced by the HTML parser")
            }
        }
    }
    output
}

/// Map an html5lib context (`td`, `svg path`, `math mi`) to a context element.
fn context(name: &str) -> FragmentContext {
    match name.split_once(' ') {
        Some(("svg", local)) => FragmentContext::svg(local),
        Some(("math", local)) => FragmentContext::mathml(local),
        _ => FragmentContext::html(name),
    }
}

fn parse(
    record: &Record,
    input: &[u8],
    sizes: impl IntoIterator<Item = usize>,
) -> Result<String, String> {
    enum Parser {
        Document(HtmlDocumentParser),
        Fragment(HtmlFragmentParser),
    }
    let scripting = if record.script_on {
        Scripting::Enabled
    } else {
        Scripting::Disabled
    };
    let mut parser = match &record.context {
        None => Parser::Document(
            HtmlDocumentParser::new_with_scripting(
                Default::default(),
                Default::default(),
                scripting,
            )
            .map_err(|e| e.to_string())?,
        ),
        Some(name) => Parser::Fragment(
            HtmlFragmentParser::new_with_scripting(
                &context(name),
                Default::default(),
                Default::default(),
                scripting,
            )
            .map_err(|e| e.to_string())?,
        ),
    };
    let mut feed = |bytes: &[u8]| match &mut parser {
        Parser::Document(parser) => parser.feed(bytes),
        Parser::Fragment(parser) => parser.feed(bytes),
    };
    // Upstream input is Unicode, separate from the byte decoder's transport BOM.
    feed(b"\xef\xbb\xbf").map_err(|e| e.to_string())?;
    let mut offset = 0;
    for size in sizes {
        feed(&input[offset..offset + size]).map_err(|e| e.to_string())?;
        offset += size;
    }
    assert_eq!(offset, input.len());
    match parser {
        Parser::Document(mut parser) => {
            let result = parser.finish().map_err(|e| e.to_string())?;
            Ok(serialize(result.document(), result.document().root()))
        }
        Parser::Fragment(mut parser) => {
            let result = parser.finish().map_err(|e| e.to_string())?;
            Ok(serialize(result.document(), result.root()))
        }
    }
}

#[test]
fn html5lib_tree_construction_documents() {
    let mut passed = 0;
    let mut fragments = 0;
    let mut adapted = 0;
    let mut splits = 0;
    let mut failures = Vec::new();
    for (file, source) in FILES {
        for (index, record) in records(source).iter().enumerate() {
            let bytes = record.data.as_bytes();
            let actual = parse(record, bytes, [bytes.len()]);
            let expected = CURRENT_HTML_TREES
                .iter()
                .find(|(f, i, _)| f == file && *i == index)
                .map_or(record.document.as_str(), |&(_, _, expected)| {
                    assert_ne!(
                        expected, record.document,
                        "obsolete adapter for {file} #{index}"
                    );
                    adapted += 1;
                    expected
                });
            let known = KNOWN_FAILURES
                .iter()
                .any(|(f, i, _)| f == file && *i == index);
            match (actual.as_deref() == Ok(expected), known) {
                (true, false) => {
                    passed += 1;
                    fragments += usize::from(record.context.is_some());
                    // Feed boundaries must not change the tree: every
                    // two-chunk split and one-byte feeds.
                    for split in 0..=bytes.len() {
                        let split_result = parse(record, bytes, [split, bytes.len() - split]);
                        if split_result != actual {
                            failures.push(format!("{file} #{index}: differs at split {split}"));
                        }
                        splits += 1;
                    }
                    if parse(record, bytes, std::iter::repeat_n(1, bytes.len())) != actual {
                        failures.push(format!("{file} #{index}: differs with one-byte feeds"));
                    }
                }
                (false, true) => {}
                (true, true) => failures.push(format!("{file} #{index}: unexpectedly passes")),
                (false, false) => failures.push(format!(
                    "{file} #{index}\n--- input\n{}\n--- expected\n{}--- actual\n{}",
                    record.data,
                    expected,
                    actual.unwrap_or_else(|e| format!("error: {e}\n"))
                )),
            }
        }
    }
    assert_eq!(adapted, CURRENT_HTML_TREES.len());
    eprintln!(
        "html5lib tree records: {passed} passed ({fragments} fragments), {} known failures, \
         {adapted} adapted expectations, {splits} two-chunk splits",
        KNOWN_FAILURES.len()
    );
    if !failures.is_empty() {
        panic!("{} failures\n\n{}", failures.len(), failures.join("\n"));
    }
}
