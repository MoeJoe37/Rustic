//! Cascade layers (`@layer`, `revert-layer`) and the animation and
//! transition origins (CSS-07b).

use rustic_css::{CssParser, PropertyDeclaration};
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_html::{HtmlDocumentParser, ParsedDocument};
use rustic_style::{
    Cascade, ComputeContext, DomElement, LayerName, LayerSegment, MatchContext, Origin,
    StyleDiagnosticKind, StyleInputs, StyleSheet, parse_style_attribute, style_tree,
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

fn element_by_id(document: &Document, id: &str) -> NodeId {
    document
        .walk(document.root())
        .unwrap()
        .map(|(node, _)| node)
        .find(|&node| id_of(document, node).as_deref() == Some(id))
        .unwrap_or_else(|| panic!("no element #{id}"))
}

/// The non-initial specified values of the element `#x` in `html`.
fn style(html: &str, sheets: &[(Origin, &str)]) -> String {
    let page = document(html);
    let doc = page.document();
    let x = element_by_id(doc, "x");
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
    .find(|s| s.node == x)
    .unwrap()
    .specified
    .dump_non_initial()
}

fn author(source: &str) -> Vec<(Origin, &str)> {
    vec![(Origin::Author, source)]
}

const RED: &str = "color: rgb(255, 0, 0)\n";
const GREEN: &str = "color: rgb(0, 128, 0)\n";
const BLUE: &str = "color: rgb(0, 0, 255)\n";

#[test]
fn later_layers_win_and_unlayered_styles_win_over_layers() {
    let p = "<p id=x>";
    assert_eq!(
        style(
            p,
            &author("@layer a { p { color: red } } @layer b { p { color: green } }")
        ),
        GREEN
    );
    // Layer order beats specificity; unlayered styles form the last layer.
    assert_eq!(
        style(
            p,
            &author("p { color: blue } @layer a { #x#x { color: red } }")
        ),
        BLUE
    );
    // A statement fixes the order up front, by first declaration.
    assert_eq!(
        style(
            p,
            &author("@layer b, a; @layer a { p { color: red } } @layer b { p { color: green } }")
        ),
        RED
    );
    // Layer order continues across the author sheets, in sheet order.
    assert_eq!(
        style(
            p,
            &[
                (Origin::Author, "@layer b { p { color: green } }"),
                (Origin::Author, "@layer a { p { color: red } } @layer b { }"),
            ]
        ),
        RED
    );
}

#[test]
fn important_declarations_reverse_the_layer_order() {
    assert_eq!(
        style(
            "<p id=x>",
            &author(
                "@layer a { p { color: red !important } } \
                 @layer b { p { color: green !important } } \
                 p { color: blue !important }"
            )
        ),
        RED
    );
    // Normal style attributes still beat every normal rule, and important
    // style attributes beat important rules in any layer.
    assert_eq!(
        style(
            "<p id=x style='color: blue !important'>",
            &author("@layer a { p { color: red !important } }")
        ),
        BLUE
    );
}

#[test]
fn sublayers_sort_before_their_parent_and_dotted_names_reach_them() {
    let p = "<p id=x>";
    // `a`'s own rules beat its sublayer `a.inner`.
    assert_eq!(
        style(
            p,
            &author("@layer a { p { color: red } @layer inner { p { color: green } } }")
        ),
        RED
    );
    // `@layer a.inner` adds to the same sublayer; `b` comes after all of `a`.
    assert_eq!(
        style(
            p,
            &author(
                "@layer a { @layer inner { p { color: red } } } \
                 @layer b { p { color: blue } } \
                 @layer a.inner { p { color: green } }"
            )
        ),
        BLUE
    );
    let (sheet, _) = StyleSheet::parse(
        &CssParser::default(),
        "@layer a.b { } @layer c, a; @layer { @layer d { } }",
        Origin::Author,
    )
    .unwrap();
    let names: Vec<String> = sheet.layers.iter().map(LayerName::to_string).collect();
    assert_eq!(names, ["a.b", "c", "a", "<anonymous>", "<anonymous>.d"]);
    assert_eq!(sheet.layers[3].segments(), [LayerSegment::Anonymous(1)]);
}

#[test]
fn every_anonymous_layer_is_distinct() {
    let p = "<p id=x>";
    assert_eq!(
        style(
            p,
            &author("@layer { p { color: red } } @layer x { p { color: green } } @layer { }")
        ),
        GREEN
    );
    // Same-named children of different anonymous layers are different layers.
    assert_eq!(
        style(
            p,
            &author(
                "@layer { @layer x { p { color: red } } } \
                 @layer { @layer x { p { color: green } } } "
            )
        ),
        GREEN
    );
    assert_eq!(
        style(
            p,
            &[
                (Origin::Author, "@layer { p { color: red } }"),
                (Origin::Author, "@layer x { p { color: green } }"),
                (Origin::Author, "@layer { p { color: blue } }"),
            ]
        ),
        BLUE
    );
}

#[test]
fn layer_order_is_separate_per_origin() {
    // The user sheet's order (b, a) does not reorder the author layers.
    assert_eq!(
        style(
            "<p id=x>",
            &[
                (Origin::User, "@layer b, a;"),
                (
                    Origin::Author,
                    "@layer a { p { color: red } } @layer b { p { color: green } }"
                ),
            ]
        ),
        GREEN
    );
}

#[test]
fn revert_layer_rolls_back_to_the_previous_layer() {
    let p = "<p id=x>";
    assert_eq!(
        style(
            p,
            &author(
                "@layer one { p { color: red } } \
                 @layer two { p { color: green } p { color: revert-layer } }"
            )
        ),
        RED
    );
    // With no earlier layer it rolls back to the previous origin.
    assert_eq!(
        style(
            p,
            &[
                (Origin::UserAgent, "p { color: blue }"),
                (Origin::Author, "@layer one { p { color: revert-layer } }"),
            ]
        ),
        BLUE
    );
    // Important: everything between the layer's important and normal levels
    // is skipped, the unlayered normal rule included.
    assert_eq!(
        style(
            p,
            &author(
                "@layer one { p { color: red } } \
                 @layer two { p { color: green } p { color: revert-layer !important } } \
                 p { color: blue }"
            )
        ),
        RED
    );
    // A normal style attribute is its own layer after the unlayered one.
    assert_eq!(
        style(
            "<p id=x style='color: revert-layer'>",
            &author("@layer one { p { color: red } } p { color: blue }")
        ),
        BLUE
    );
    // An important one skips only style attributes (and animations), not
    // the important author rules in between.
    assert_eq!(
        style(
            "<p id=x style='color: green; color: revert-layer !important'>",
            &author("p { color: red !important } p { color: blue }")
        ),
        RED
    );
    assert_eq!(
        style(
            "<p id=x style='color: green; color: revert-layer !important'>",
            &author("p { color: blue }")
        ),
        BLUE
    );
}

#[test]
fn malformed_layer_rules_are_dropped_with_diagnostics() {
    let parser = CssParser::default();
    let kinds = |source: &str| -> Vec<StyleDiagnosticKind> {
        let (_, diagnostics) = StyleSheet::parse(&parser, source, Origin::Author).unwrap();
        diagnostics.iter().map(|d| d.kind).collect()
    };
    for source in [
        "@layer;",
        "@layer initial;",
        "@layer a.REVERT-LAYER { }",
        "@layer a, b { }",
        "@layer a b;",
        "@layer a. b;",
        "@layer a .b;",
        "@layer a.;",
        "@layer .a;",
        "@layer a,;",
        "@layer 'a';",
        "@layer a.1;",
    ] {
        assert_eq!(
            kinds(source),
            [StyleDiagnosticKind::InvalidLayerRule],
            "{source}"
        );
    }
    assert!(kinds("@layer  a.b , c ;").is_empty());
    assert_eq!(
        kinds("@layer a { color: red; p { color: blue } }"),
        [StyleDiagnosticKind::MisplacedDeclaration]
    );
    // A layer statement may precede `@namespace`; a block may not.
    assert!(kinds("@layer a; @namespace svg url(x); svg|a { }").is_empty());
    assert_eq!(
        kinds("@layer a { } @namespace svg url(x);"),
        [StyleDiagnosticKind::InvalidNamespaceRule]
    );
    // `@namespace` only counts at the top level.
    assert_eq!(
        kinds("@layer a { @namespace svg url(x); }"),
        [StyleDiagnosticKind::InvalidNamespaceRule]
    );
    // An invalid layer rule drops its rules too.
    let (sheet, _) =
        StyleSheet::parse(&parser, "@layer a, b { p { color: red } }", Origin::Author).unwrap();
    assert!(sheet.rules.is_empty() && sheet.layers.is_empty());
}

#[test]
fn deeply_nested_layers_stay_iterative() {
    // Just under the parser's default nesting limit of 256 blocks.
    let depth = 250;
    let source = format!(
        "{}p {{ color: red }}{}",
        "@layer a { ".repeat(depth),
        " }".repeat(depth)
    );
    let parser = CssParser::default();
    let (sheet, _) = StyleSheet::parse(&parser, &source, Origin::Author).unwrap();
    {
        assert_eq!(sheet.layers.len(), depth);
        let mut cascade = Cascade::new();
        cascade.push(sheet);
        let page = document("<p id=x>");
        let doc = page.document();
        style_tree(
            doc,
            doc.root(),
            &cascade,
            &parser,
            &MatchContext::html(),
            &ComputeContext::default(),
        )
        .unwrap();
    }
}

fn declarations(source: &str) -> Vec<PropertyDeclaration> {
    let (declarations, diagnostics) = parse_style_attribute(&CssParser::default(), source).unwrap();
    assert!(diagnostics.is_empty());
    declarations
}

/// `#x`'s specified color with the given animation and transition values.
fn animated(
    sheets: &[(Origin, &str)],
    style_attribute: &str,
    animations: &str,
    transitions: &str,
) -> String {
    let page = document("<p id=x>");
    let doc = page.document();
    let x = DomElement::new(doc, element_by_id(doc, "x")).unwrap();
    let style_attribute = declarations(style_attribute);
    let animations = declarations(animations);
    let transitions = declarations(transitions);
    let inputs = StyleInputs {
        style_attribute: &style_attribute,
        animations: &animations,
        transitions: &transitions,
        ..StyleInputs::document()
    };
    cascade(sheets)
        .specified_style_with(x, &MatchContext::html(), &inputs, None)
        .dump_non_initial()
}

#[test]
fn animations_and_transitions_have_their_own_origins() {
    let sheets = [
        (Origin::UserAgent, "p { width: 1px !important }"),
        (Origin::User, "p { color: blue }"),
        (
            Origin::Author,
            "#x#x { color: red; height: 3px !important }",
        ),
    ];
    // Animations beat every normal declaration, style attributes included,
    // and lose to important ones; later animation values win.
    assert_eq!(
        animated(
            &sheets,
            "color: yellow",
            "color: green; height: 9px; width: 9px; margin-top: 1px; margin-top: 2px",
            ""
        ),
        "color: rgb(0, 128, 0)\nmargin-top: 2px\nwidth: 1px\nheight: 3px\n"
    );
    // Transitions beat everything, important user-agent declarations too.
    assert_eq!(
        animated(&sheets, "", "width: 5px", "width: 7px; color: lime"),
        "color: rgb(0, 255, 0)\nwidth: 7px\nheight: 3px\n"
    );
    // `revert` in an animation counts it as author origin: back to the user.
    assert_eq!(
        animated(&sheets, "", "color: revert", ""),
        "color: rgb(0, 0, 255)\nwidth: 1px\nheight: 3px\n"
    );
    // `revert-layer` skips only the animation origin.
    assert_eq!(
        animated(&sheets, "", "color: green; color: revert-layer", ""),
        "color: rgb(255, 0, 0)\nwidth: 1px\nheight: 3px\n"
    );
    // An important style attribute's `revert-layer` skips animations too.
    assert_eq!(
        animated(
            &sheets,
            "color: revert-layer !important",
            "color: green",
            ""
        ),
        "color: rgb(255, 0, 0)\nwidth: 1px\nheight: 3px\n"
    );
}

#[test]
fn layers_and_media_rules_nest_in_each_other() {
    let p = "<p id=x>";
    // The default environment is a 1280×720 screen.
    assert_eq!(
        style(
            p,
            &author(
                "@media (min-width: 100px) { @layer a { p { color: red } } } \
                 @layer a { @media print { p { color: green } } } \
                 @layer b { @media screen { @layer c { p { color: blue } } } }"
            )
        ),
        BLUE
    );
    let (sheet, _) = StyleSheet::parse(
        &CssParser::default(),
        "@layer a { @media print { p { color: green } } }",
        Origin::Author,
    )
    .unwrap();
    assert_eq!(
        (sheet.rules[0].layer, sheet.rules[0].media.as_slice()),
        (Some(0), &[0][..])
    );
}
