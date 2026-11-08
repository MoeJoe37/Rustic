//! Computed values, the HTML user-agent stylesheet, `<style>` attachment and
//! the computed-style dump (CSS-05).

use rustic_css::{CssParser, PropertyId};
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_html::{HtmlDocumentParser, ParsedDocument};
use rustic_style::{
    Cascade, ComputeContext, ElementStyle, HTML_USER_AGENT_STYLESHEET, MatchContext, Origin,
    StyleElementStatus, StyleSheet, document_cascade, dump_computed_styles, style_tree,
};

fn document(html: &str) -> ParsedDocument {
    let mut parser = HtmlDocumentParser::default();
    parser.feed(html.as_bytes()).unwrap();
    parser.finish().unwrap()
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

fn styled(
    html: &str,
    author: &str,
    compute: ComputeContext,
) -> (ParsedDocument, Vec<ElementStyle>) {
    let parser = CssParser::default();
    let page = document(html);
    let mut cascade = Cascade::new();
    let (sheet, diagnostics) = StyleSheet::parse(&parser, author, Origin::Author).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    cascade.push(sheet);
    let doc = page.document();
    let styles = style_tree(
        doc,
        doc.root(),
        &cascade,
        &parser,
        &MatchContext::html(),
        &compute,
    )
    .unwrap();
    (page, styles)
}

/// The computed value of `property` on `#id`, serialized.
fn computed(html: &str, author: &str, id: &str, property: &str) -> String {
    computed_in(html, author, id, property, ComputeContext::default())
}

fn computed_in(
    html: &str,
    author: &str,
    id: &str,
    property: &str,
    compute: ComputeContext,
) -> String {
    let (page, styles) = styled(html, author, compute);
    let doc = page.document();
    let style = styles
        .iter()
        .find(|s| id_of(doc, s.node).as_deref() == Some(id))
        .unwrap_or_else(|| panic!("no element #{id}"));
    let property = PropertyId::from_name(property).unwrap();
    style.computed.get(property).to_string()
}

#[test]
fn font_sizes_resolve_against_the_parent_and_root() {
    let html = "<div id=a><div id=b><div id=c><div id=d><div id=e></div></div></div></div></div>";
    let sheet = "html { font-size: 20px } #a { font-size: 150% } #b { font-size: 0.5em } \
                 #c { font-size: 2rem } #d { font-size: larger } #e { font-size: x-small }";
    assert_eq!(computed(html, sheet, "a", "font-size"), "30px");
    assert_eq!(computed(html, sheet, "b", "font-size"), "15px");
    assert_eq!(computed(html, sheet, "c", "font-size"), "40px");
    assert_eq!(computed(html, sheet, "d", "font-size"), "48px");
    assert_eq!(computed(html, sheet, "e", "font-size"), "10px");
    // Inherited font sizes are the parent's computed pixels, not its `em`.
    let html = "<div id=a><p id=b></p></div>";
    assert_eq!(
        computed(html, "#a { font-size: 2em }", "b", "font-size"),
        "32px"
    );
    // `rem` on the root itself refers to the initial font size.
    assert_eq!(
        computed("<html id=r>", "html { font-size: 2rem }", "r", "font-size"),
        "32px"
    );
}

#[test]
fn lengths_become_pixels_and_percentages_stay() {
    let html = "<div id=a></div>";
    let sheet = "#a { font-size: 10px; width: 2em; height: 1in; margin-left: 50%; \
                 padding-top: 3ex; min-width: 2ch; max-width: 10vw; min-height: 5vh; \
                 margin-top: 1lh; line-height: 2 }";
    assert_eq!(computed(html, sheet, "a", "width"), "20px");
    assert_eq!(computed(html, sheet, "a", "height"), "96px");
    assert_eq!(computed(html, sheet, "a", "margin-left"), "50%");
    assert_eq!(computed(html, sheet, "a", "padding-top"), "15px");
    assert_eq!(computed(html, sheet, "a", "min-width"), "10px");
    assert_eq!(computed(html, sheet, "a", "max-width"), "128px");
    assert_eq!(computed(html, sheet, "a", "min-height"), "36px");
    assert_eq!(computed(html, sheet, "a", "margin-top"), "20px");
    let small = ComputeContext {
        viewport_width: 400.0,
        viewport_height: 800.0,
    };
    let sheet = "#a { width: 50vmin; height: 25vmax }";
    assert_eq!(computed_in(html, sheet, "a", "width", small), "200px");
    assert_eq!(computed_in(html, sheet, "a", "height", small), "200px");
}

#[test]
fn line_height_percentages_compute_and_numbers_inherit() {
    let html = "<div id=a><p id=b></p></div><div id=c><p id=d></p></div>";
    let sheet = "#a { font-size: 10px; line-height: 150% } #b { font-size: 20px } \
                 #c { font-size: 10px; line-height: 1.5 } #d { font-size: 20px }";
    assert_eq!(computed(html, sheet, "a", "line-height"), "15px");
    assert_eq!(computed(html, sheet, "b", "line-height"), "15px");
    assert_eq!(computed(html, sheet, "d", "line-height"), "1.5");
}

#[test]
fn font_weights_resolve_relative_keywords() {
    let html = "<div id=a><b id=b><b id=c><i id=d></i></b></b></div>";
    let sheet =
        "#a { font-weight: 300 } #b, #c { font-weight: bolder } #d { font-weight: lighter }";
    assert_eq!(computed(html, sheet, "a", "font-weight"), "300");
    assert_eq!(computed(html, sheet, "b", "font-weight"), "400");
    assert_eq!(computed(html, sheet, "c", "font-weight"), "700");
    assert_eq!(computed(html, sheet, "d", "font-weight"), "400");
    assert_eq!(
        computed(html, "#a { font-weight: bold }", "a", "font-weight"),
        "700"
    );
}

#[test]
fn currentcolor_and_border_widths() {
    let html = "<div id=a><p id=b></p></div>";
    let sheet = "#a { color: red; border-top: 2px solid; background-color: currentcolor; \
                 border-right-width: thick; border-bottom: thin solid } \
                 #b { color: currentcolor; border-left-color: currentcolor }";
    assert_eq!(
        computed(html, sheet, "a", "border-top-color"),
        "rgb(255, 0, 0)"
    );
    assert_eq!(
        computed(html, sheet, "a", "background-color"),
        "rgb(255, 0, 0)"
    );
    assert_eq!(computed(html, sheet, "a", "border-top-width"), "2px");
    // A side whose style is `none` has a zero width, whatever was specified.
    assert_eq!(computed(html, sheet, "a", "border-right-width"), "0px");
    assert_eq!(computed(html, sheet, "a", "border-bottom-width"), "1px");
    assert_eq!(computed(html, sheet, "b", "color"), "rgb(255, 0, 0)");
    assert_eq!(
        computed(html, sheet, "b", "border-left-color"),
        "rgb(255, 0, 0)"
    );
    assert_eq!(
        computed(html, "#a { border-style: solid }", "a", "border-left-width"),
        "3px"
    );
}

#[test]
fn the_root_is_blockified() {
    assert_eq!(
        computed(
            "<html id=r>",
            "html { display: inline-flex }",
            "r",
            "display"
        ),
        "flex"
    );
    assert_eq!(
        computed(
            "<html id=r><body id=b>",
            "body { display: inline }",
            "b",
            "display"
        ),
        "inline"
    );
}

#[test]
fn the_user_agent_sheet_parses_cleanly() {
    let (sheet, diagnostics) = StyleSheet::parse(
        &CssParser::default(),
        HTML_USER_AGENT_STYLESHEET,
        Origin::UserAgent,
    )
    .unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert!(sheet.rules.len() > 30);
}

#[test]
fn style_elements_are_attached_in_tree_order() {
    let html = "<!doctype html><style>p { color: red; colour: blue }</style>\
                <style type=text/plain>p { color: lime }</style>\
                <style media=print>p { color: lime }</style>\
                <style media='screen, print'>p { width: 5px }</style>\
                <template><style>p { color: lime }</style></template>\
                <svg><style>p { color: lime }</style></svg>\
                <p id=p>text</p><style TYPE=' TEXT/CSS '>p { height: 2px }</style>";
    let page = document(html);
    let doc = page.document();
    let parser = CssParser::default();
    let (cascade, reports) = document_cascade(doc, &parser).unwrap();
    let statuses: Vec<_> = reports.iter().map(|r| r.status).collect();
    assert_eq!(
        statuses,
        [
            StyleElementStatus::Applied,
            StyleElementStatus::NotCss,
            StyleElementStatus::Applied,
            StyleElementStatus::Applied,
            StyleElementStatus::Applied,
        ]
    );
    assert_eq!(reports[0].diagnostics.len(), 1);
    // The user-agent sheet plus four author sheets; the print one is
    // attached but its media query does not match a screen.
    assert_eq!(cascade.sheets().len(), 5);
    let styles = style_tree(
        doc,
        doc.root(),
        &cascade,
        &parser,
        &MatchContext::html(),
        &ComputeContext::default(),
    )
    .unwrap();
    let p = styles
        .iter()
        .find(|s| id_of(doc, s.node).as_deref() == Some("p"))
        .unwrap();
    assert_eq!(
        p.computed.dump_non_initial(),
        "display: block\ncolor: rgb(255, 0, 0)\nmargin-top: 16px\nmargin-bottom: 16px\n\
         border-top-color: rgb(255, 0, 0)\nborder-right-color: rgb(255, 0, 0)\n\
         border-bottom-color: rgb(255, 0, 0)\nborder-left-color: rgb(255, 0, 0)\n\
         width: 5px\nheight: 2px\n"
    );
}

#[test]
fn the_dump_lists_computed_values_by_element() {
    let html = "<!doctype html><title>t</title><style>.n { color: blue }</style>\
                <h1 id=top class='n big'>Hi <b>there</b></h1><hr>";
    let page = document(html);
    let doc = page.document();
    let parser = CssParser::default();
    let (cascade, _) = document_cascade(doc, &parser).unwrap();
    let styles = style_tree(
        doc,
        doc.root(),
        &cascade,
        &parser,
        &MatchContext::html(),
        &ComputeContext::default(),
    )
    .unwrap();
    let dump = dump_computed_styles(doc, &styles, usize::MAX).unwrap();
    let expected = "\
html
  display: block
  head
    display: none
    title
      display: none
    style
      display: none
  body
    display: block
    margin-top: 8px
    margin-right: 8px
    margin-bottom: 8px
    margin-left: 8px
    h1#top.n.big
      display: block
      color: rgb(0, 0, 255)
      margin-top: 21.44px
      margin-bottom: 21.44px
      font-size: 32px
      font-weight: 700
      b
        font-weight: 900
    hr
      display: block
      color: rgb(128, 128, 128)
      margin-top: 8px
      margin-right: auto
      margin-bottom: 8px
      margin-left: auto
      border-top-width: 1px
      border-right-width: 1px
      border-bottom-width: 1px
      border-left-width: 1px
      border-top-style: inset
      border-right-style: inset
      border-bottom-style: inset
      border-left-style: inset
      border-top-color: rgb(128, 128, 128)
      border-right-color: rgb(128, 128, 128)
      border-bottom-color: rgb(128, 128, 128)
      border-left-color: rgb(128, 128, 128)
";
    assert_eq!(dump, expected);
    assert_eq!(dump_computed_styles(doc, &styles, 10), None);
}
