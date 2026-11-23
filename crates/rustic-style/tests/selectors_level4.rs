//! Matching for the CSS-07a selectors: `:has()`, form states, `:lang()`,
//! `:dir()`, the nesting selector `&` (also through nested style rules) and
//! pseudo-element states.

use std::time::Instant;

use rustic_base::CancellationToken;
use rustic_css::{CssParser, NamespaceMap, SelectorList};
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_html::{HtmlDocumentParser, ParsedDocument, ParserLimits};
use rustic_style::{
    Cascade, ComputeContext, DomElement, MatchContext, Origin, StyleSheet, matches_complex,
    query_selector_all, style_tree,
};

fn document(html: &str) -> ParsedDocument {
    let mut parser = HtmlDocumentParser::default();
    parser.feed(html.as_bytes()).unwrap();
    parser.finish().unwrap()
}

fn selectors(source: &str) -> SelectorList {
    CssParser::default()
        .selector_list(source, &NamespaceMap::new())
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

fn select(parsed: &ParsedDocument, source: &str) -> String {
    let doc = parsed.document();
    query_selector_all(doc, doc.root(), &selectors(source), &MatchContext::html())
        .unwrap()
        .into_iter()
        .map(|id| id_of(doc, id))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Selected IDs, skipping elements without one.
fn select_ids(parsed: &ParsedDocument, source: &str) -> String {
    select(parsed, source)
        .split(' ')
        .filter(|s| {
            s.chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit() || c == '_')
        })
        .collect::<Vec<_>>()
        .join(" ")
}

const HAS_PAGE: &str = r#"<!doctype html>
<div id=a>
  <p id=p1><b id=b1>bold</b></p>
  <p id=p2 class=x></p>
  <section id=s1><div id=d1><em id=e1 class=y></em></div></section>
  <h2 id=h1></h2><p id=p3></p><ul id=u1></ul>
</div>"#;

#[test]
fn has_matches_relative_selectors() {
    let page = document(HAS_PAGE);
    let cases: &[(&str, &str)] = &[
        ("p:has(b)", "p1"),
        ("p:has(> b)", "p1"),
        ("div:has(> p)", "a"),
        ("section:has(> em)", ""),
        ("section:has(em)", "s1"),
        (":has(> div > .y)", "s1"),
        (":has(.y):not(html, body)", "a s1 d1"),
        ("h2:has(+ p)", "h1"),
        ("h2:has(+ ul)", ""),
        ("h2:has(~ ul)", "h1"),
        ("p:has(~ section em)", "p1 p2"),
        ("p:has(+ section > div .y)", "p2"),
        ("p:has(~ p, ~ ul)", "p1 p2 p3"),
        ("p:not(:has(*))", "p2 p3"),
        (":is(p:has(.nope), section:has(em))", "s1"),
        ("body > div:has(p.x):has(em)", "a"),
        // The anchor is the :has() subject, never a sibling or descendant.
        ("p:has(> p)", ""),
    ];
    let mut failures = Vec::new();
    for &(selector, want) in cases {
        let got = select(&page, selector);
        if got != want {
            failures.push(format!("{selector:?}: got {got:?}, want {want:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A deterministic tree of `n` elements with ids `_0.._n` and every third
/// carrying class `x`.
fn generated(n: usize) -> String {
    let mut html = String::from("<!doctype html><div id=_0>");
    let mut seed = 7u32;
    let mut open = 1;
    for i in 1..n {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let class = if i % 3 == 0 { " class=x" } else { "" };
        html.push_str(&format!("<span id=_{i}{class}>"));
        open += 1;
        // Close a few elements at random to make siblings.
        for _ in 0..(seed >> 16) % 3 {
            if open > 1 {
                html.push_str("</span>");
                open -= 1;
            }
        }
    }
    html
}

#[test]
fn has_fast_paths_agree_with_the_general_walk() {
    let page = document(&generated(300));
    // Each pair is equivalent; the first uses the single-compound fast path
    // and the second the general candidate walk.
    let pairs: &[(&str, &str)] = &[
        (":has(.x)", ":has(> .x, > * .x)"),
        (":has(> .x)", ":has(> .x:is(*), > .x.x)"),
        (":has(~ .x)", ":has(+ .x, + * ~ .x)"),
        (":has(+ .x)", ":has(+ .x:not(.nope), + .x.x)"),
        (
            ":has(.x):has(+ span)",
            ":has(> * .x, > .x):has(+ span:is(*), + span span)",
        ),
    ];
    for &(fast, general) in pairs {
        let a = select_ids(&page, fast);
        assert!(!a.is_empty(), "{fast:?} selects nothing");
        assert_eq!(a, select_ids(&page, general), "{fast:?} vs {general:?}");
    }
}

#[test]
fn has_stays_linear_on_deep_trees() {
    let depth = 3000;
    let html = format!("<!doctype html>{}<i class=leaf></i>", "<div>".repeat(depth));
    let mut limits = ParserLimits::default();
    limits.document.max_depth = depth + 16;
    let mut parser = HtmlDocumentParser::new(limits, CancellationToken::default()).unwrap();
    parser.feed(html.as_bytes()).unwrap();
    let page = parser.finish().unwrap();
    let doc = page.document();
    let start = Instant::now();
    let found = query_selector_all(
        doc,
        doc.root(),
        &selectors("div:has(.leaf)"),
        &MatchContext::html(),
    )
    .unwrap();
    assert_eq!(found.len(), depth);
    // Without the subtree memo this is ~4.5M compound tests; with it, 3000.
    assert!(start.elapsed().as_secs() < 5, "took {:?}", start.elapsed());
}

/// Elements `a` with a descendant chain `x0 c0 x1 c1 ... xn` below them (or
/// starting at a child for a leading `>`), checked by brute force over parent
/// links, for comparing with the memoized `:has()` chain matcher (TD-30).
fn brute_chain(parsed: &ParsedDocument, leading_child: bool, steps: &[(&str, bool)]) -> String {
    let doc = parsed.document();
    let all = |source: &str| {
        query_selector_all(doc, doc.root(), &selectors(source), &MatchContext::html()).unwrap()
    };
    let sets: Vec<Vec<NodeId>> = steps.iter().map(|(s, _)| all(s)).collect();
    let parent = |id: NodeId| doc.node(id).unwrap().parent();
    // Can `y` end a chain whose compound `k` is matched at or above `y`,
    // with compound 0 hanging off `anchor` through the leading combinator?
    fn ends(
        k: usize,
        e: NodeId,
        anchor: NodeId,
        leading_child: bool,
        steps: &[(&str, bool)],
        sets: &[Vec<NodeId>],
        parent: &dyn Fn(NodeId) -> Option<NodeId>,
    ) -> bool {
        if !sets[k].contains(&e) {
            return false;
        }
        let Some(up) = parent(e) else { return false };
        if k == 0 {
            if leading_child {
                return up == anchor;
            }
            let mut cursor = Some(up);
            while let Some(c) = cursor {
                if c == anchor {
                    return true;
                }
                cursor = parent(c);
            }
            return false;
        }
        // steps[k - 1].1: is the combinator before compound k a child one?
        if steps[k - 1].1 {
            return up != anchor && ends(k - 1, up, anchor, leading_child, steps, sets, parent);
        }
        let mut cursor = Some(up);
        while let Some(c) = cursor {
            if c == anchor {
                return false;
            }
            if ends(k - 1, c, anchor, leading_child, steps, sets, parent) {
                return true;
            }
            cursor = parent(c);
        }
        false
    }
    let last = steps.len() - 1;
    all("*")
        .into_iter()
        .filter(|&a| {
            sets[last]
                .iter()
                .any(|&y| ends(last, y, a, leading_child, steps, &sets, &parent))
        })
        .map(|id| id_of(doc, id))
        .filter(|s| s.starts_with('_'))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn has_chains_agree_with_brute_force() {
    let page = document(&generated(400));
    // (selector, leading child, [(compound, child combinator after it)])
    type Case<'a> = (&'a str, bool, &'a [(&'a str, bool)]);
    let cases: &[Case] = &[
        (":has(.x span)", false, &[(".x", false), ("span", false)]),
        (":has(> .x span)", true, &[(".x", false), ("span", false)]),
        (":has(.x > span)", false, &[(".x", true), ("span", false)]),
        (":has(> span > .x)", true, &[("span", true), (".x", false)]),
        (
            ":has(.x span > .x)",
            false,
            &[(".x", false), ("span", true), (".x", false)],
        ),
    ];
    for &(selector, leading_child, steps) in cases {
        let fast = select_ids(&page, selector);
        assert!(!fast.is_empty(), "{selector:?} selects nothing");
        assert_eq!(
            fast,
            brute_chain(&page, leading_child, steps),
            "{selector:?}"
        );
    }
}

#[test]
fn multi_compound_has_stays_linear_on_deep_trees() {
    // TD-30: `:has()` with a combinator inside its argument used to match
    // every candidate in each anchor's subtree, O(depth²) for a chain.
    let depth = 3000;
    let html = format!(
        "<!doctype html>{}<b><i class=leaf></i></b>",
        "<div>".repeat(depth)
    );
    let mut limits = ParserLimits::default();
    limits.document.max_depth = depth + 16;
    let mut parser = HtmlDocumentParser::new(limits, CancellationToken::default()).unwrap();
    parser.feed(html.as_bytes()).unwrap();
    let page = parser.finish().unwrap();
    let doc = page.document();
    let start = Instant::now();
    for (selector, count) in [
        ("div:has(div b > .leaf)", depth - 1),
        ("div:has(> div .leaf)", depth - 1),
        ("div:has(b .leaf)", depth),
    ] {
        let found =
            query_selector_all(doc, doc.root(), &selectors(selector), &MatchContext::html())
                .unwrap();
        assert_eq!(found.len(), count, "{selector}");
    }
    assert!(start.elapsed().as_secs() < 5, "took {:?}", start.elapsed());
}

const FORM_PAGE: &str = r#"<!doctype html>
<form id=f1>
  <input id=t1 required placeholder=Name>
  <input id=t2 value=x placeholder=Name readonly>
  <input id=t3 type=number value=abc placeholder=0>
  <input id=t4 type=email value="  " placeholder=e>
  <input id=h1 type=hidden required>
  <input id=c1 type=checkbox checked>
  <input id=c2 type=CHECKBOX>
  <input id=r1 type=radio name=g checked>
  <input id=r2 type=radio name=g checked>
  <input id=r3 type=radio name=h>
  <input id=r4 type=radio>
  <button id=b1 type=button></button>
  <button id=b2></button>
  <input id=b3 type=submit>
  <fieldset id=fs1 disabled>
    <legend id=lg1><input id=t5></legend>
    <legend id=lg2><input id=t6></legend>
    <input id=t7>
    <fieldset id=fs2><input id=t8></fieldset>
  </fieldset>
  <select id=s1><option id=o1 disabled>a<option id=o2>b<optgroup id=og1 disabled><option id=o3>c</optgroup></select>
  <select id=s2 required><option id=o4 selected>a<option id=o5 selected>b</select>
  <select id=s3 multiple><option id=o6>a<option id=o7 selected>b</select>
  <textarea id=ta1 placeholder=p></textarea>
  <textarea id=ta2 placeholder=p readonly>text</textarea>
  <progress id=pr1></progress><progress id=pr2 value=1></progress>
</form>
<input id=r5 type=radio name=g form=f2>
<form id=f2><input id=r6 type=radio name=g checked></form>
<input id=b4 type=image form=f2>
<div id=ce contenteditable><p id=cp>edit</p><span id=cs contenteditable=false>x</span></div>"#;

#[test]
fn form_states_follow_html() {
    let page = document(FORM_PAGE);
    let cases: &[(&str, &str)] = &[
        (":disabled", "fs1 t6 t7 fs2 t8 o1 og1 o3"),
        (":enabled:not(input, option)", "b1 b2 s1 s2 s3 ta1 ta2"),
        // Only the last checked radio of a group stays checked; the first
        // non-disabled option of a single-line select without `selected` is
        // selected, and the last `selected` wins.
        (":checked", "c1 r2 o2 o5 o7 r6"),
        (":indeterminate", "r3 r4 pr1"),
        (":default", "c1 r1 r2 b2 o4 o5 o7 r6 b4"),
        (":required", "t1 s2"),
        (":optional:not(input)", "s1 s3 ta1 ta2"),
        (":read-write", "t1 t3 t4 t5 ta1 ce cp"),
        (
            "input:read-only",
            "t2 h1 c1 c2 r1 r2 r3 r4 b3 t6 t7 t8 r5 r6 b4",
        ),
        (":placeholder-shown", "t1 t3 t4 ta1"),
    ];
    let mut failures = Vec::new();
    for &(selector, want) in cases {
        let got = select(&page, selector);
        if got != want {
            failures.push(format!("{selector:?}: got {got:?}, want {want:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const LANG_PAGE: &str = r#"<!doctype html>
<html lang=en-US>
<meta http-equiv=content-language content="fr">
<p id=p1>x</p>
<div lang=de-Latn-DE><p id=p2>x</p><p id=p3 lang="">x</p></div>
<div lang=zh-Hant-TW id=d1><span id=s1 lang=ZH-hant>x</span></div>
<svg id=svg xml:lang=ja lang=ko><rect id=r1 /></svg>
</html>"#;

#[test]
fn lang_uses_extended_filtering_and_inheritance() {
    let page = document(LANG_PAGE);
    let cases: &[(&str, &str)] = &[
        (":lang(en)", "html head meta body p1"),
        ("p:lang(de-DE)", "p2"),
        ("p:lang(\"*-DE\")", "p2"),
        ("p:lang(de, en)", "p1 p2"),
        (":lang(\"\")", "p3"),
        (":lang(zh-TW)", "d1"),
        (":lang(zh-hant)", "d1 s1"),
        // xml:lang wins over lang on the same element.
        (":lang(ja)", "svg r1"),
        (":lang(ko)", ""),
    ];
    let mut failures = Vec::new();
    for &(selector, want) in cases {
        let got = select(&page, selector);
        if got != want {
            failures.push(format!("{selector:?}: got {got:?}, want {want:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // Without any lang attribute, the pragma-set default language applies.
    let page = document(
        "<!doctype html><meta http-equiv=Content-Language content=' pt-BR '><p id=p>x</p>",
    );
    assert_eq!(select(&page, "p:lang(pt)"), "p");
    let page =
        document("<!doctype html><meta http-equiv=content-language content='pt, en'><p id=p>x</p>");
    assert_eq!(select(&page, "p:lang(pt), p:lang(en)"), "");
}

const DIR_PAGE: &str = "<!doctype html>
<div id=d1><p id=p1>abc</p></div>
<div id=d2 dir=rtl><p id=p2>abc</p><p id=p3 dir=LTR>x</p><p id=p4 dir=bogus>x</p></div>
<div id=d3 dir=auto><span><b dir=ltr>abc</b><script>abc</script>\u{5d0}</span></div>
<div id=d4 dir=auto>123 abc</div>
<div dir=rtl><span id=s1 dir=auto>123</span><bdi id=bdi1>abc</bdi><input id=i1 type=tel></div>
<input id=i2 dir=auto value=\"\u{627}\"><textarea id=ta1 dir=auto>\u{5d0}</textarea>";

#[test]
fn dir_follows_html_directionality() {
    let page = document(DIR_PAGE);
    // Unnamed elements print as their local name: d3's span and script
    // inherit rtl, the bdi's parent div is rtl, and the b with dir=ltr is not.
    assert_eq!(
        select(&page, ":dir(rtl)"),
        "d2 p2 p4 d3 span script div s1 i2 ta1"
    );
    let rtl = select(&page, ":dir(rtl)");
    for id in ["d2", "p2", "p4", "d3", "s1", "i2", "ta1"] {
        assert!(rtl.split(' ').any(|s| s == id), "{id} not rtl: {rtl}");
    }
    let ltr = select(&page, ":dir(ltr)");
    for id in ["d1", "p1", "p3", "d4", "bdi1", "i1", "html", "b"] {
        assert!(ltr.split(' ').any(|s| s == id), "{id} not ltr: {ltr}");
    }
    assert_eq!(select(&page, ":dir(sideways)"), "");
}

#[test]
fn nesting_selector_resolves_in_nested_rules() {
    let html = r#"<!doctype html>
<ul class=menu><li id=l1>a<li id=l2 class=open>b</ul>
<ol><li id=l3>c</ol>"#;
    let page = document(html);
    let doc = page.document();
    let parser = CssParser::default();
    let source = ".menu {
        color: red;
        > li { font-weight: 700; &.open { font-style: italic } }
        :is(&) li { width: 10px }
        width: 1px;
    }
    & li { height: 2px }";
    let (sheet, diagnostics) = StyleSheet::parse(&parser, source, Origin::Author).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let mut cascade = Cascade::new();
    cascade.push(sheet);
    let styles = style_tree(
        doc,
        doc.root(),
        &cascade,
        &parser,
        &MatchContext::html(),
        &ComputeContext::default(),
    )
    .unwrap();
    let style = |id: &str| {
        styles
            .iter()
            .find(|s| id_of(doc, s.node) == id)
            .map(|s| s.specified.dump_non_initial())
            .unwrap()
    };
    assert!(style("l1").contains("font-weight: 700"), "{}", style("l1"));
    assert!(style("l1").contains("width: 10px"), "{}", style("l1"));
    assert!(!style("l1").contains("italic"));
    assert!(style("l2").contains("font-style: italic"));
    assert!(!style("l3").contains("700"), "{}", style("l3"));
    // A top-level `&` is :scope, the root element, so `& li` matches every li.
    assert!(style("l3").contains("height: 2px"), "{}", style("l3"));
    // The nested declarations rule after the nested rules applies too.
    assert!(style("ul").contains("width: 1px"), "{}", style("ul"));
}

#[test]
fn pseudo_element_states_need_an_interaction_model() {
    let page = document("<!doctype html><p id=p>x</p>");
    let doc = page.document();
    let p = query_selector_all(doc, doc.root(), &selectors("p"), &MatchContext::html()).unwrap()[0];
    let element = DomElement::new(doc, p).unwrap();
    let cx = MatchContext::html();
    let originates =
        |source: &str| matches_complex(&selectors(source).selectors()[0], element, &cx);
    assert!(originates("p::before"));
    assert!(!originates("p::before:hover"));
    assert!(originates("p::before:not(:hover)"));
    assert!(!originates("p::before:is(:focus, :active)"));
    assert!(originates("p::selection"));
}

#[test]
fn nested_media_keeps_the_parent_selectors() {
    let parser = CssParser::default();
    let source = "p { color: red; @media print { width: 1px; > b { height: 2px } } margin: 0 }";
    let (sheet, diagnostics) = StyleSheet::parse(&parser, source, Origin::Author).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let rules: Vec<(String, usize, Vec<usize>)> = sheet
        .rules
        .iter()
        .map(|r| {
            (
                r.selectors.to_string(),
                r.declarations.len(),
                r.media.clone(),
            )
        })
        .collect();
    assert_eq!(
        rules,
        [
            ("p".to_owned(), 1, vec![]),
            ("p".to_owned(), 1, vec![0]),
            ("& > b".to_owned(), 1, vec![0]),
            ("p".to_owned(), 4, vec![]),
        ]
    );
}
