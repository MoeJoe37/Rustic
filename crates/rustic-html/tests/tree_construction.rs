//! Complete html5lib tree-construction documents, run against the pinned corpus.
//!
//! Every document record is run unless it needs fragment parsing, scripting
//! or processing-instruction nodes; those are counted and skipped. Expected
//! failures must be listed explicitly in `KNOWN_FAILURES` with a reason.

use std::fmt::Write;

use rustic_dom::{Document, DomString, NodeId, NodeKind};
use rustic_html::HtmlDocumentParser;

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
const KNOWN_FAILURES: &[(&str, usize, &str)] = &[
    ("webkit02.dat", 44, SELECTEDCONTENT),
    ("webkit02.dat", 45, SELECTEDCONTENT),
    ("webkit02.dat", 46, SELECTEDCONTENT),
    ("webkit02.dat", 47, SELECTEDCONTENT),
];

const SELECTEDCONTENT: &str = "selectedcontent mirroring needs option selectedness and DOM node \
     cloning, which are DOM element behavior rather than tree construction";

struct Record {
    data: String,
    document: String,
    fragment: bool,
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
        let mut fragment = false;
        let mut script_on = false;
        for line in block.split_inclusive('\n') {
            let header = line.trim_end_matches('\n');
            if headers.contains(&header) {
                current = header;
                fragment |= header == "#document-fragment";
                script_on |= header == "#script-on";
                continue;
            }
            match current {
                "#data" => data.push_str(line),
                "#document" => document.push_str(line),
                _ => {}
            }
        }
        data.truncate(data.strip_suffix('\n').unwrap_or(&data).len());
        let document = format!("{}\n", document.trim_end_matches('\n'));
        result.push(Record {
            data,
            document,
            fragment,
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
fn serialize(document: &Document) -> String {
    let mut output = String::new();
    let mut stack: Vec<(NodeId, usize)> = document
        .children(document.root())
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
            NodeKind::Document | NodeKind::DocumentFragment => unreachable!(),
        }
    }
    output
}

fn parse(input: &[u8], sizes: impl IntoIterator<Item = usize>) -> Result<String, String> {
    let mut parser = HtmlDocumentParser::default();
    // Upstream input is Unicode, separate from the byte decoder's transport BOM.
    parser.feed(b"\xef\xbb\xbf").map_err(|e| e.to_string())?;
    let mut offset = 0;
    for size in sizes {
        parser
            .feed(&input[offset..offset + size])
            .map_err(|e| e.to_string())?;
        offset += size;
    }
    assert_eq!(offset, input.len());
    let result = parser.finish().map_err(|e| e.to_string())?;
    Ok(serialize(result.document()))
}

fn runnable(record: &Record) -> bool {
    !record.fragment && !record.script_on && !record.data.contains("<?")
}

#[test]
fn html5lib_tree_construction_documents() {
    let mut passed = 0;
    let mut skipped = 0;
    let mut splits = 0;
    let mut failures = Vec::new();
    for (file, source) in FILES {
        for (index, record) in records(source).iter().enumerate() {
            if !runnable(record) {
                skipped += 1;
                continue;
            }
            let bytes = record.data.as_bytes();
            let actual = parse(bytes, [bytes.len()]);
            let known = KNOWN_FAILURES
                .iter()
                .any(|(f, i, _)| f == file && *i == index);
            match (actual.as_deref() == Ok(record.document.as_str()), known) {
                (true, false) => {
                    passed += 1;
                    // Feed boundaries must not change the tree: every
                    // two-chunk split and one-byte feeds.
                    for split in 0..=bytes.len() {
                        let split_result = parse(bytes, [split, bytes.len() - split]);
                        if split_result != actual {
                            failures.push(format!("{file} #{index}: differs at split {split}"));
                        }
                        splits += 1;
                    }
                    if parse(bytes, std::iter::repeat_n(1, bytes.len())) != actual {
                        failures.push(format!("{file} #{index}: differs with one-byte feeds"));
                    }
                }
                (false, true) => {}
                (true, true) => failures.push(format!("{file} #{index}: unexpectedly passes")),
                (false, false) => failures.push(format!(
                    "{file} #{index}\n--- input\n{}\n--- expected\n{}--- actual\n{}",
                    record.data,
                    record.document,
                    actual.unwrap_or_else(|e| format!("error: {e}\n"))
                )),
            }
        }
    }
    eprintln!(
        "html5lib tree documents: {passed} passed, {} known failures, {skipped} skipped, \
         {splits} two-chunk splits",
        KNOWN_FAILURES.len()
    );
    if !failures.is_empty() {
        panic!("{} failures\n\n{}", failures.len(), failures.join("\n"));
    }
}
