//! css-tables layout WPT comparison (`layout.tables`).
//!
//! `tools/import_wpt_tables_tests.py` copies the pinned web-platform-tests
//! pages whose only script is `checkLayout(selector)` into
//! `fixtures/wpt-tables/cases.txt`. Each page is laid out at the WPT
//! viewport (800×600) with the bundled test fonts, and every element's
//! `data-expected-width`/`-height`, `data-offset-x`/`-y` and
//! `data-expected-client-width`/`-height` are compared with its used
//! geometry, as `check-layout-th.js` would through `offsetWidth`,
//! `offsetLeft` and `clientWidth` (with `offsetParent` stopping at table
//! cells and tables for static elements). Any other expectation attribute counts as
//! a failure. `expected_failures.txt` lists the reviewed failures with a
//! reason; the test fails when a listed page passes or an unlisted page
//! fails, so the list cannot go stale.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use rustic_base::CancellationToken;
use rustic_css::{CssParser, Position, PropertyId as P, PropertyValue as V};
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_html::HtmlDocumentParser;
use rustic_layout::{BlockLayout, BoxTree, CssSize, LayoutLimits};
use rustic_style::{ComputeContext, MatchContext, document_cascade, style_tree};
use rustic_text::bundled::test_database;

const CASES: &str = include_str!("fixtures/wpt-tables/cases.txt");
const EXPECTED_FAILURES: &str = include_str!("fixtures/wpt-tables/expected_failures.txt");

struct Case<'a> {
    path: &'a str,
    html: String,
}

fn cases() -> Vec<Case<'static>> {
    let mut out = Vec::new();
    let mut current: Option<Case<'static>> = None;
    for line in CASES.lines() {
        if let Some(path) = line.strip_prefix("#test ") {
            current = Some(Case {
                path,
                html: String::new(),
            });
        } else if line.starts_with("#selector ") {
            // Every element with expectations is checked, which covers the
            // selected subtrees.
        } else if line == "#end" {
            out.extend(current.take());
        } else if let Some(case) = current.as_mut() {
            case.html.push_str(line);
            case.html.push('\n');
        }
    }
    out
}

fn text(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

fn attributes(doc: &Document, id: NodeId) -> Vec<(String, String)> {
    match doc.node(id).map(|n| n.kind()) {
        Ok(NodeKind::Element(e)) => e
            .attributes()
            .iter()
            .map(|a| (a.name().local().to_owned(), text(a.value().as_units())))
            .collect(),
        _ => Vec::new(),
    }
}

fn local_name(doc: &Document, id: NodeId) -> Option<String> {
    match doc.node(id).map(|n| n.kind()) {
        Ok(NodeKind::Element(e)) => Some(e.name().local().to_owned()),
        _ => None,
    }
}

/// Elements in document order.
fn elements(doc: &Document) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![doc.root()];
    while let Some(id) = stack.pop() {
        let Ok(node) = doc.node(id) else { continue };
        if matches!(node.kind(), NodeKind::Element(_)) {
            out.push(id);
        }
        let mut children = Vec::new();
        let mut c = node.first_child();
        while let Some(child) = c {
            children.push(child);
            c = doc.node(child).ok().and_then(|n| n.next_sibling());
        }
        stack.extend(children.into_iter().rev());
    }
    out
}

/// Lay out one page and compare its expectations; `Err` names the first
/// mismatch.
fn run(case: &Case<'_>) -> Result<(), String> {
    let mut parser = HtmlDocumentParser::default();
    parser
        .feed(case.html.as_bytes())
        .map_err(|e| format!("{e:?}"))?;
    let doc = parser
        .finish()
        .map_err(|e| format!("{e:?}"))?
        .into_document();
    let css = CssParser::default();
    let (cascade, _) = document_cascade(&doc, &css).map_err(|e| format!("{e:?}"))?;
    let context = ComputeContext {
        viewport_width: 800.0,
        viewport_height: 600.0,
        ..ComputeContext::with_fonts(Arc::new(test_database()))
    };
    let styles = style_tree(
        &doc,
        doc.root(),
        &cascade,
        &css,
        &MatchContext::html(),
        &context,
    )
    .map_err(|e| format!("{e:?}"))?;
    let cancel = CancellationToken::default();
    let tree = BoxTree::build(&doc, doc.root(), &styles, LayoutLimits::default(), &cancel)
        .map_err(|e| format!("box tree: {e:?}"))?;
    let fonts = test_database();
    let layout = BlockLayout::compute_with_fonts(
        &tree,
        CssSize::new(800.0, 600.0).map_err(|e| e.to_string())?,
        LayoutLimits::default(),
        &fonts,
        &cancel,
    )
    .map_err(|e| match e {
        rustic_layout::FlowError::Unsupported { reason, .. } => format!("unsupported {reason:?}"),
        other => format!("layout: {other:?}"),
    })?;
    let mut fragment_of: HashMap<NodeId, usize> = HashMap::new();
    for (i, f) in layout.fragments().iter().enumerate() {
        fragment_of.entry(f.node()).or_insert(i);
    }
    let mut positioned: HashMap<NodeId, bool> = HashMap::new();
    for b in tree.boxes() {
        if let Some(style) = b.style() {
            let p = !matches!(style.get(P::Position), V::Position(Position::Static));
            positioned.entry(b.node()).or_insert(p);
        }
    }
    for id in elements(&doc) {
        let expectations: Vec<(String, f64)> = attributes(&doc, id)
            .into_iter()
            .filter(|(name, _)| {
                name.starts_with("data-expected-")
                    || name.starts_with("data-offset-")
                    || name.starts_with("data-total-")
            })
            .map(|(name, value)| {
                let number = value.trim().parse::<f64>().unwrap_or(f64::NAN);
                (name, number)
            })
            .collect();
        if expectations.is_empty() {
            continue;
        }
        let name = local_name(&doc, id).unwrap_or_default();
        let f = fragment_of
            .get(&id)
            .map(|&i| layout.fragments()[i])
            .ok_or_else(|| format!("<{name}> has no block fragment"))?;
        // offsetParent: the nearest positioned ancestor, or for a static
        // element the nearest `td`, `th` or `table` (CSSOM View §7), else
        // body (whose offsets are relative to the initial containing block).
        let mut parent_box = None;
        let own_static = !positioned.get(&id).copied().unwrap_or(false);
        let mut a = doc.node(id).ok().and_then(|n| n.parent());
        while let Some(p) = a {
            let tag = local_name(&doc, p);
            if tag.as_deref() == Some("body") {
                break;
            }
            let cell = own_static && matches!(tag.as_deref(), Some("td" | "th" | "table"));
            if cell || positioned.get(&p).copied().unwrap_or(false) {
                parent_box = fragment_of.get(&p).map(|&i| layout.fragments()[i]);
                break;
            }
            a = doc.node(p).ok().and_then(|n| n.parent());
        }
        let border = f.border();
        let (ox, oy) = match parent_box {
            Some(pf) => (
                border.origin().x.get() - pf.padding().origin().x.get(),
                border.origin().y.get() - pf.padding().origin().y.get(),
            ),
            None => (border.origin().x.get(), border.origin().y.get()),
        };
        for (attribute, expected) in expectations {
            let actual = match attribute.as_str() {
                "data-expected-width" => border.size().width().get(),
                "data-expected-height" => border.size().height().get(),
                "data-offset-x" => ox,
                "data-offset-y" => oy,
                "data-expected-client-width" => f.padding().size().width().get(),
                "data-expected-client-height" => f.padding().size().height().get(),
                other => return Err(format!("unsupported expectation {other}")),
            };
            if (actual.round() - expected).abs() > 0.5 {
                return Err(format!(
                    "<{name}> {attribute}: expected {expected}, got {actual}"
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn table_layout_matches_the_reviewed_wpt_expectations() {
    let all = cases();
    assert!(all.len() >= 35, "{} cases", all.len());
    let mut listed: BTreeMap<&str, &str> = BTreeMap::new();
    for line in EXPECTED_FAILURES.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let (path, reason) = line.split_once('\t').expect("path<TAB>reason");
        assert!(listed.insert(path, reason).is_none(), "duplicate {path}");
    }
    let mut unexpected = Vec::new();
    let mut passed = 0;
    for case in &all {
        let result = run(case);
        match (&result, listed.remove(case.path)) {
            (Ok(()), None) => passed += 1,
            (Err(_), Some(_)) => {}
            (Ok(()), Some(reason)) => {
                unexpected.push(format!("{} now passes (listed: {reason})", case.path));
            }
            (Err(e), None) => unexpected.push(format!("{}\t{e}", case.path)),
        }
    }
    for path in listed.keys() {
        unexpected.push(format!("{path} is listed but not imported"));
    }
    eprintln!("css-tables WPT: {passed} of {} pages pass", all.len());
    assert!(unexpected.is_empty(), "{}", unexpected.join("\n"));
}
