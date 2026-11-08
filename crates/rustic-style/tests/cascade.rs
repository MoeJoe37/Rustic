//! The cascade: origins, importance, style attributes, specificity, order,
//! CSS-wide keywords and inheritance (CSS-04).

use rustic_css::CssParser;
use rustic_css::{MediaEnvironment, MediaType};
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_html::{HtmlDocumentParser, ParsedDocument};
use rustic_style::{
    Cascade, ComputeContext, MatchContext, Origin, StyleDiagnosticKind, StyleSheet, style_tree,
};

fn document(html: &str) -> ParsedDocument {
    let mut parser = HtmlDocumentParser::default();
    parser.feed(html.as_bytes()).unwrap();
    parser.finish().unwrap()
}

fn cascade(sheets: &[(Origin, &str)]) -> Cascade {
    let parser = CssParser::default();
    let mut cascade = Cascade::new();
    for &(origin, source) in sheets {
        let (sheet, diagnostics) = StyleSheet::parse(&parser, source, origin).unwrap();
        assert!(diagnostics.is_empty(), "{source:?}: {diagnostics:?}");
        cascade.push(sheet);
    }
    cascade
}

fn id_of(document: &Document, id: NodeId) -> Option<String> {
    match document.node(id).ok()?.kind() {
        NodeKind::Element(element) => element
            .attributes()
            .iter()
            .find(|a| a.name().local() == "id")
            .map(|a| String::from_utf16_lossy(a.value().as_units())),
        _ => None,
    }
}

/// `id -> non-initial specified values`, for elements with an `id`.
fn styles(html: &str, sheets: &[(Origin, &str)]) -> Vec<(String, String)> {
    let page = document(html);
    let doc = page.document();
    style_tree(
        doc,
        doc.root(),
        &cascade(sheets),
        &CssParser::default(),
        &MatchContext::html(),
        &ComputeContext::default(),
    )
    .unwrap()
    .into_iter()
    .filter_map(|s| Some((id_of(doc, s.node)?, s.specified.dump_non_initial())))
    .collect()
}

fn style_of(html: &str, sheets: &[(Origin, &str)], id: &str) -> String {
    styles(html, sheets)
        .into_iter()
        .find(|(i, _)| i == id)
        .map(|(_, s)| s)
        .unwrap_or_else(|| panic!("no element #{id}"))
}

#[test]
fn specificity_then_order_decides_within_an_origin() {
    let sheet = "p { color: red } #x { color: blue } p.c { color: green } p { color: yellow; display: block }";
    assert_eq!(
        style_of("<p id=x class=c>", &[(Origin::Author, sheet)], "x"),
        "display: block\ncolor: rgb(0, 0, 255)\n"
    );
    assert_eq!(
        style_of("<p id=y class=c>", &[(Origin::Author, sheet)], "y"),
        "display: block\ncolor: rgb(0, 128, 0)\n"
    );
    // A later declaration in the same block wins; so does a later sheet.
    let html = "<p id=z>";
    assert_eq!(
        style_of(
            html,
            &[(Origin::Author, "p { width: 1px; width: 2px }")],
            "z"
        ),
        "width: 2px\n"
    );
    assert_eq!(
        style_of(
            html,
            &[
                (Origin::Author, "p { width: 1px }"),
                (Origin::Author, "p { width: 3px }")
            ],
            "z"
        ),
        "width: 3px\n"
    );
    // The most specific matching selector of a list counts.
    assert_eq!(
        style_of(
            "<p id=w>",
            &[(Origin::Author, "#w, p { width: 1px } p { width: 2px }")],
            "w"
        ),
        "width: 1px\n"
    );
}

#[test]
fn origins_and_importance_follow_the_cascade_order() {
    let ua = (
        Origin::UserAgent,
        "p { width: 1px !important; height: 1px; color: red; display: block }",
    );
    let user = (
        Origin::User,
        "p { width: 2px; height: 2px !important; color: lime !important }",
    );
    let author = (
        Origin::Author,
        "#p { width: 3px !important; height: 3px !important; color: blue }",
    );
    // Important UA > important user > important author > normal author.
    assert_eq!(
        style_of("<p id=p>", &[author, user, ua], "p"),
        "display: block\ncolor: rgb(0, 255, 0)\nwidth: 1px\nheight: 2px\n"
    );
    // Normal author beats normal user beats normal UA, regardless of specificity.
    assert_eq!(
        style_of(
            "<p id=p>",
            &[
                (Origin::Author, "p { color: blue }"),
                (Origin::User, "#p#p { color: lime }"),
                (Origin::UserAgent, "#p#p#p { color: red; display: block }")
            ],
            "p"
        ),
        "display: block\ncolor: rgb(0, 0, 255)\n"
    );
}

#[test]
fn style_attributes_beat_rules_of_the_same_importance() {
    let sheet = (
        Origin::Author,
        "#s { color: red; width: 1px !important; height: 1px }",
    );
    assert_eq!(
        style_of(
            "<p id=s style='color: blue; width: 2px; height: 2px !important'>",
            &[sheet],
            "s"
        ),
        "color: rgb(0, 0, 255)\nwidth: 1px\nheight: 2px\n"
    );
    // Important user styles still beat important style attributes.
    assert_eq!(
        style_of(
            "<p id=s style='width: 2px !important'>",
            &[(Origin::User, "p { width: 9px !important }")],
            "s"
        ),
        "width: 9px\n"
    );
    // Invalid declarations in the attribute are dropped with diagnostics.
    let page = document("<p style='color: nonsense; width: 4px; transform: none'>");
    let doc = page.document();
    let styled = style_tree(
        doc,
        doc.root(),
        &Cascade::new(),
        &CssParser::default(),
        &MatchContext::html(),
        &ComputeContext::default(),
    )
    .unwrap();
    let p = styled
        .iter()
        .find(|s| !s.inline_diagnostics.is_empty())
        .unwrap();
    assert_eq!(p.specified.dump_non_initial(), "width: 4px\n");
    let kinds: Vec<_> = p.inline_diagnostics.iter().map(|d| d.kind).collect();
    assert_eq!(
        kinds,
        [
            StyleDiagnosticKind::InvalidValue,
            StyleDiagnosticKind::UnknownProperty
        ]
    );
}

#[test]
fn inheritance_and_css_wide_keywords() {
    let sheet = (
        Origin::Author,
        "#outer { color: red; width: 10px; font-size: 20px; border-top-color: blue } \
         #a { width: inherit; color: initial } \
         #b { color: unset; width: unset } \
         #c { font-size: initial; border-top-color: inherit }",
    );
    let html =
        "<div id=outer><p id=a></p><p id=b></p><p id=c><span id=d></span></p><p id=e></p></div>";
    let got = styles(html, &[sheet]);
    let get = |id: &str| got.iter().find(|(i, _)| i == id).unwrap().1.clone();
    assert_eq!(get("a"), "width: 10px\nfont-size: 20px\n");
    assert_eq!(get("b"), "color: rgb(255, 0, 0)\nfont-size: 20px\n");
    assert_eq!(
        get("c"),
        "color: rgb(255, 0, 0)\nborder-top-color: rgb(0, 0, 255)\n"
    );
    // Inherited properties flow through several generations; others do not.
    assert_eq!(get("d"), "color: rgb(255, 0, 0)\n");
    assert_eq!(get("e"), "color: rgb(255, 0, 0)\nfont-size: 20px\n");
}

#[test]
fn revert_rolls_back_to_the_previous_origin() {
    let ua = (
        Origin::UserAgent,
        "p { display: block; width: 1px; height: 1px }",
    );
    let user = (Origin::User, "p { width: 2px }");
    let author = (
        Origin::Author,
        "p { display: inline; width: 3px; height: 3px; color: red } \
         #r { display: revert; width: revert; height: revert-layer; color: revert }",
    );
    assert_eq!(
        style_of("<p id=r>", &[ua, user, author], "r"),
        "display: block\nwidth: 2px\nheight: 1px\n"
    );
    // `revert` in the user-agent origin acts like `unset`.
    assert_eq!(
        style_of(
            "<div id=o style='color: lime'><p id=r>",
            &[(Origin::UserAgent, "p { display: block; color: revert }")],
            "r"
        ),
        "display: block\ncolor: rgb(0, 255, 0)\n"
    );
    // An important author `revert` also skips normal author declarations.
    assert_eq!(
        style_of(
            "<p id=r>",
            &[
                (Origin::UserAgent, "p { width: 1px }"),
                (
                    Origin::Author,
                    "p { width: 3px } p { width: revert !important }"
                )
            ],
            "r"
        ),
        "width: 1px\n"
    );
}

#[test]
fn stylesheet_diagnostics_drop_only_the_broken_parts() {
    let parser = CssParser::default();
    let source = "@namespace svg url(http://www.w3.org/2000/svg);\n\
                  p:has(a) { color: red }\n\
                  svg|rect, p { color: blue; colour: red; width: -1px }\n\
                  @supports (color: red) { p { color: green } }\n\
                  div { & p { color: red } height: 1px }\n\
                  @namespace late url(x);";
    let (sheet, diagnostics) = StyleSheet::parse(&parser, source, Origin::Author).unwrap();
    let kinds: Vec<_> = diagnostics.iter().map(|d| d.kind).collect();
    assert_eq!(
        kinds,
        [
            StyleDiagnosticKind::InvalidSelector(
                rustic_css::SelectorErrorKind::UnsupportedPseudoClass
            ),
            StyleDiagnosticKind::UnknownProperty,
            StyleDiagnosticKind::InvalidValue,
            StyleDiagnosticKind::UnsupportedAtRule,
            StyleDiagnosticKind::UnsupportedNestedRule,
            StyleDiagnosticKind::InvalidNamespaceRule,
        ]
    );
    assert_eq!(sheet.rules.len(), 2);
    assert!(sheet.media.is_empty());
    assert_eq!(sheet.rules[0].selectors.to_string(), "svg|rect, p");
    assert_eq!(sheet.rules[0].declarations.len(), 1);
    assert_eq!(sheet.rules[1].declarations.len(), 1);
    assert_eq!(&source[diagnostics[1].span.range()], "colour: red");
}

#[test]
fn media_rules_apply_only_when_their_queries_match() {
    let source = "p { width: 1px } \
                  @media (min-width: 1000px) { p { width: 2px } \
                      @media (hover) { #p { height: 5px } } } \
                  @media print { p { width: 3px; color: red } } \
                  @media (max-width: 999px) { p { color: blue } } \
                  p.late { width: 4px }";
    let parser = CssParser::default();
    let (sheet, diagnostics) = StyleSheet::parse(&parser, source, Origin::Author).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(sheet.media.len(), 4);
    let media: Vec<_> = sheet.rules.iter().map(|r| r.media.clone()).collect();
    assert_eq!(
        media,
        [vec![], vec![0], vec![0, 1], vec![2], vec![3], vec![]]
    );

    let page = document("<p id=p>");
    let doc = page.document();
    let dump = |cascade: &Cascade| {
        let styles = style_tree(
            doc,
            doc.root(),
            cascade,
            &parser,
            &MatchContext::html(),
            &ComputeContext::default(),
        )
        .unwrap();
        styles
            .iter()
            .find(|s| id_of(doc, s.node).as_deref() == Some("p"))
            .unwrap()
            .specified
            .dump_non_initial()
    };
    let mut cascade = Cascade::new();
    cascade.push(sheet);
    // The default environment is a 1280x720 screen with a hovering pointer.
    assert_eq!(dump(&cascade), "width: 2px\nheight: 5px\n");

    let mut narrow = MediaEnvironment::screen(600.0, 800.0);
    assert!(cascade.set_environment(narrow.clone()));
    assert_eq!(dump(&cascade), "color: rgb(0, 0, 255)\nwidth: 1px\n");
    // Nothing that a query tests changed: no restyle needed.
    narrow.reduced_motion = true;
    assert!(!cascade.set_environment(narrow));

    let mut print = MediaEnvironment::screen(800.0, 1000.0);
    print.media_type = MediaType::Print;
    assert!(cascade.set_environment(print));
    assert_eq!(dump(&cascade), "color: rgb(0, 0, 255)\nwidth: 3px\n");
}

#[test]
fn media_rule_errors_are_reported() {
    let parser = CssParser::default();
    let source = "@media screen;\n\
                  @media screen { color: red; p { width: 1px } }\n\
                  @media screen { @namespace x url(y); }\n\
                  @namespace late url(z);\n\
                  @media (bogus) , screen { p { height: 1px } }";
    let (sheet, diagnostics) = StyleSheet::parse(&parser, source, Origin::Author).unwrap();
    let kinds: Vec<_> = diagnostics.iter().map(|d| d.kind).collect();
    assert_eq!(
        kinds,
        [
            StyleDiagnosticKind::InvalidMediaRule,
            StyleDiagnosticKind::MisplacedDeclaration,
            StyleDiagnosticKind::InvalidNamespaceRule,
            StyleDiagnosticKind::InvalidNamespaceRule,
        ]
    );
    assert_eq!(&source[diagnostics[1].span.range()], "color: red");
    assert_eq!(sheet.rules.len(), 2);
    assert_eq!(sheet.media[2].to_string(), "(bogus), screen");
}
