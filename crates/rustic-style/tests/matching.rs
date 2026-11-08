//! Selector matching against parsed HTML documents (CSS-02).

use rustic_css::{CssParser, NamespaceMap, SelectorList};
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_html::{DocumentMode, HtmlDocumentParser, ParsedDocument};
use rustic_style::{DomElement, MatchContext, matches, matches_complex, query_selector_all};

fn document(html: &str) -> ParsedDocument {
    let mut parser = HtmlDocumentParser::default();
    parser.feed(html.as_bytes()).unwrap();
    parser.finish().unwrap()
}

fn selectors(source: &str, namespaces: &NamespaceMap) -> SelectorList {
    CssParser::default()
        .selector_list(source, namespaces)
        .unwrap()
        .value
        .unwrap_or_else(|e| panic!("{source:?}: {e:?}"))
}

fn id_of(document: &Document, id: NodeId) -> String {
    match document.node(id).unwrap().kind() {
        NodeKind::Element(element) => element
            .attributes()
            .iter()
            .find(|a| a.name().local() == "id")
            .map(|a| String::from_utf16_lossy(a.value().as_units()))
            .unwrap_or_else(|| element.name().local().to_owned()),
        _ => unreachable!("only elements match"),
    }
}

fn select_in(parsed: &ParsedDocument, source: &str, cx: &MatchContext<DomElement<'_>>) -> String {
    let doc = parsed.document();
    let list = selectors(source, &NamespaceMap::new());
    query_selector_all(doc, doc.root(), &list, cx)
        .unwrap()
        .into_iter()
        .map(|id| id_of(doc, id))
        .collect::<Vec<_>>()
        .join(" ")
}

fn select(parsed: &ParsedDocument, source: &str) -> String {
    select_in(parsed, source, &MatchContext::html())
}

const PAGE: &str = r#"<!doctype html>
<div id=a class="x Y">
  <p id=p1 class=x>one</p>
  <p id=p2 lang=en-GB title="Hello World">two <span id=s1></span></p>
  <!-- comment -->
  <section id=sec><p id=p3></p><em id=e1></em><p id=p4> </p></section>
  <a id=l1 href=/x>link</a><a id=l2>anchor</a>
  <input id=i1 type=TEXT data-v=Abc>
</div>
<svg id=svg><foreignObject id=fo></foreignObject><rect id=r1 viewBox="0 0 1 1"/></svg>"#;

#[test]
fn simple_selectors_and_case_rules() {
    let page = document(PAGE);
    let cases = [
        ("#a", "a"),
        (".x", "a p1"),
        (".y", ""),
        ("P", "p1 p2 p3 p4"),
        ("div > p", "p1 p2"),
        ("div p", "p1 p2 p3 p4"),
        ("p + p", "p2"),
        ("p1 ~ p, #p1 ~ p", "p2"),
        ("#p3 ~ *", "e1 p4"),
        ("section > :first-child", "p3"),
        ("[lang|=en]", "p2"),
        ("[title~=World]", "p2"),
        ("[title~='']", ""),
        ("[title^=Hell][title$=rld][title*='o W']", "p2"),
        ("[title^='']", ""),
        ("[type=text]", "i1"),
        ("[type=text s]", ""),
        ("[data-v=abc]", ""),
        ("[data-v=abc i]", "i1"),
        ("[DATA-V]", "i1"),
        ("foreignObject", "fo"),
        ("foreignobject", ""),
        ("[viewBox]", "r1"),
        ("[viewbox]", ""),
        (":root", "html"),
        (":scope", "html"),
        ("p:empty", "p3"),
        ("span:empty, em:empty", "s1 e1"),
        (":any-link, :link", "l1"),
        ("a:visited, a:hover, a:focus", ""),
        ("p::before", ""),
    ];
    let failures: Vec<_> = cases
        .iter()
        .filter_map(|&(selector, want)| {
            let got = select(&page, selector);
            (got != want).then(|| format!("{selector:?}: got {got:?}, want {want:?}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn logical_and_structural_pseudo_classes() {
    let page = document(
        "<ol id=o><li id=1 class=x><li id=2><li id=3 class=x><li id=4><li id=5 class=x>\
         <li id=6><li id=7 class=x></ol><p id=only><b id=b1></b><i id=i1></i><b id=b2></b></p>",
    );
    let cases = [
        ("li:nth-child(2n+1)", "1 3 5 7"),
        ("li:nth-child(even)", "2 4 6"),
        ("li:nth-child(-n+3)", "1 2 3"),
        ("li:nth-last-child(2)", "6"),
        ("li:nth-child(2 of .x)", "3"),
        ("li:nth-last-child(odd of .x)", "3 7"),
        ("li:nth-child(n+5 of :not(.x))", ""),
        ("li:nth-child(0n+0)", ""),
        ("li:last-child", "7"),
        ("p > :first-of-type", "b1 i1"),
        ("p > :last-of-type", "i1 b2"),
        ("p > :only-of-type", "i1"),
        ("p > :nth-of-type(2)", "b2"),
        ("p > :nth-last-of-type(2)", "b1"),
        ("ol:only-child, li:only-child", ""),
        ("body > :only-of-type", "o only"),
        (":is(ol, p) > :is(.x, b):not(:nth-child(1))", "3 5 7 b2"),
        (":where(#o) li:not(.x, [id='2'])", "4 6"),
        ("li:not(:not(.x))", "1 3 5 7"),
        (":is(p) :is(b + i)", "i1"),
    ];
    let failures: Vec<_> = cases
        .iter()
        .filter_map(|&(selector, want)| {
            let got = select(&page, selector);
            (got != want).then(|| format!("{selector:?}: got {got:?}, want {want:?}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn nth_child_matches_a_brute_force_reference() {
    let mut html = String::from("<ul>");
    for i in 1..=12 {
        html.push_str(&format!("<li id={i}>"));
    }
    let page = document(&html);
    for a in -3..=3 {
        for b in -4..=8 {
            let got = select(&page, &format!("li:nth-child({a}n{b:+})"));
            let want: Vec<String> = (1..=12i32)
                .filter(|&p| {
                    if a == 0 {
                        p == b
                    } else {
                        (p - b) % a == 0 && (p - b) / a >= 0
                    }
                })
                .map(|p| p.to_string())
                .collect();
            assert_eq!(got, want.join(" "), "{a}n{b:+}");
        }
    }
}

#[test]
fn quirks_mode_matches_ids_and_classes_case_insensitively() {
    let quirks = document("<p id=Foo class=Bar>");
    assert_eq!(quirks.mode(), DocumentMode::Quirks);
    let cx = MatchContext {
        html_document: true,
        quirks: true,
        scope: None,
    };
    assert_eq!(select_in(&quirks, "#foo.bar", &cx), "Foo");
    assert_eq!(select(&quirks, "#foo, .bar"), "");
    // Attribute selectors are unaffected by quirks mode.
    assert_eq!(select_in(&quirks, "[id=foo]", &cx), "");
}

#[test]
fn xml_documents_and_namespaces() {
    let page = document(PAGE);
    let doc = page.document();
    let map = NamespaceMap::new()
        .with_prefix("svg", "http://www.w3.org/2000/svg")
        .with_prefix("h", "http://www.w3.org/1999/xhtml");
    let found = |source: &str, cx: &MatchContext<DomElement<'_>>| {
        query_selector_all(doc, doc.root(), &selectors(source, &map), cx)
            .unwrap()
            .into_iter()
            .map(|id| id_of(doc, id))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let html = MatchContext::html();
    assert_eq!(found("svg|*", &html), "svg fo r1");
    assert_eq!(found("h|svg, svg|p, |p", &html), "");
    assert_eq!(found("*|svg > svg|rect", &html), "r1");
    // Outside HTML documents names are case-sensitive even for HTML elements.
    assert_eq!(found("P", &MatchContext::xml()), "");
    assert_eq!(found("[type=text]", &MatchContext::xml()), "");
}

#[test]
fn scope_and_pseudo_element_subjects() {
    let page = document("<div id=d><p id=x></p></div><p id=y></p>");
    let doc = page.document();
    let all = query_selector_all(
        doc,
        doc.root(),
        &selectors("*", &NamespaceMap::new()),
        &MatchContext::html(),
    )
    .unwrap();
    let element = |name: &str| {
        all.iter()
            .copied()
            .find(|&id| id_of(doc, id) == name)
            .and_then(|id| DomElement::new(doc, id))
            .unwrap()
    };
    let cx = MatchContext {
        scope: Some(element("d")),
        ..MatchContext::html()
    };
    let scoped = selectors(":scope > p", &NamespaceMap::new());
    assert!(matches(&scoped, element("x"), &cx));
    assert!(!matches(&scoped, element("y"), &cx));
    let before = selectors("p::before", &NamespaceMap::new());
    assert!(!matches(&before, element("x"), &cx));
    assert!(matches_complex(&before.selectors()[0], element("x"), &cx));
    assert!(DomElement::new(doc, doc.root()).is_none());
}

#[test]
fn pathological_selectors_stay_linear_and_stack_safe() {
    // 100 nested divs; a descendant chain that almost matches everywhere
    // would take exponential time without restart pruning.
    let page = document(&format!("{}<i></i>", "<div>".repeat(100)));
    let chain = format!("{} span", "div ".repeat(40));
    let started = std::time::Instant::now();
    assert_eq!(select(&page, &chain), "");
    let chain = format!("{}i", "div ".repeat(40));
    assert_eq!(select(&page, &chain), "i");
    assert!(
        started.elapsed().as_secs() < 10,
        "matching took {:?}",
        started.elapsed()
    );

    // A 20,000-compound selector matches without recursion.
    let long = format!("{}i", "* ".repeat(20_000));
    assert_eq!(select(&page, &long), "");
}

#[test]
fn nested_logical_pseudo_classes_stay_polynomial() {
    // TD-21: an outer descendant combinator used to re-run a nested list for
    // every ancestor it tried, so each extra `:is(X *)` level multiplied the
    // work by the tree depth (k = 4 took about 0.8 s in release builds).
    let page = document(&format!("{}<i></i>", "<div>".repeat(100)));
    let mut selector = String::from("div");
    for _ in 0..8 {
        selector = format!(":is({selector} *)");
    }
    let not_selector = selector
        .replace(":is(", ":not(:not(")
        .replace(" *)", " *))");
    let started = std::time::Instant::now();
    let found = select(&page, &selector);
    assert!(found.ends_with("div div div i"), "{found}");
    let _ = select(&page, &not_selector);
    let nth = format!("i:nth-child(1 of {selector})");
    assert_eq!(select(&page, &nth), "i");
    assert!(
        started.elapsed().as_secs() < 5,
        "nested matching took {:?}",
        started.elapsed()
    );
}
