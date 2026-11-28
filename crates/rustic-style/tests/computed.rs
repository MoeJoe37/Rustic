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
        ..ComputeContext::default()
    };
    let sheet = "#a { width: 50vmin; height: 25vmax }";
    assert_eq!(
        computed_in(html, sheet, "a", "width", small.clone()),
        "200px"
    );
    assert_eq!(computed_in(html, sheet, "a", "height", small), "200px");
}

#[test]
fn math_functions_compute_with_pixel_lengths() {
    let html = "<div id=p><div id=a></div></div>";
    let sheet = "#p { font-size: 20px } \
                 #a { font-size: calc(50% + 2px); width: calc(50% + 2em); \
                 height: calc(2em + 3px); min-width: calc(10px - 20px); \
                 margin-left: calc(10px - 20px); max-width: min(10px, 5em); \
                 padding-left: calc(-50%); line-height: calc(150% + 1px); \
                 border-top: calc(1em + 0.5px) solid; margin-top: calc(1lh); \
                 margin-bottom: clamp(1px, 50vw, 20px); margin-right: calc(1px * infinity) }";
    let c = |property| computed(html, sheet, "a", property);
    assert_eq!(c("font-size"), "12px");
    assert_eq!(c("width"), "calc(50% + 24px)");
    assert_eq!(c("height"), "27px");
    // Negative results clamp to the property's range; margins may be negative.
    assert_eq!(c("min-width"), "0px");
    assert_eq!(c("margin-left"), "-10px");
    assert_eq!(c("max-width"), "10px");
    assert_eq!(c("padding-left"), "0%");
    assert_eq!(c("line-height"), "19px");
    assert_eq!(c("border-top-width"), "12.5px");
    assert_eq!(c("margin-top"), "19px");
    assert_eq!(c("margin-bottom"), "20px");
    assert_eq!(
        c("margin-right"),
        rustic_css::Length::px(f64::MAX).to_string()
    );
    // `font-weight` and `line-height` numbers are evaluated when parsed.
    let sheet = "#a { font-weight: calc(250 * 2); line-height: calc(3 / 2) }";
    assert_eq!(computed(html, sheet, "a", "font-weight"), "500");
    assert_eq!(computed(html, sheet, "a", "line-height"), "1.5");
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
    // Outlines: the width is 0 while the style is `none`, the color follows
    // `color`, and the offset may be negative.
    let outline = "#a { color: red; outline-width: thick; outline-offset: -1em } \
                   #b { outline: thin auto }";
    assert_eq!(computed(html, outline, "a", "outline-width"), "0px");
    assert_eq!(
        computed(html, outline, "a", "outline-color"),
        "rgb(255, 0, 0)"
    );
    assert_eq!(computed(html, outline, "a", "outline-offset"), "-16px");
    assert_eq!(computed(html, outline, "b", "outline-width"), "1px");
    // Radii compute lengths to pixels and keep percentages.
    let radius = "#a { border-radius: 1em 10% / calc(2em - 4px) }";
    assert_eq!(
        computed(html, radius, "a", "border-top-left-radius"),
        "16px 28px"
    );
    assert_eq!(
        computed(html, radius, "a", "border-top-right-radius"),
        "10% 28px"
    );
    // Clip paths compute lengths and positions to offsets.
    let clip = "#a { clip-path: circle(1em at right 2em bottom) }";
    assert_eq!(
        computed(html, clip, "a", "clip-path"),
        "circle(16px at calc(100% - 32px) 100%)"
    );
    // Transforms compute lengths to pixels and origins to offsets.
    let transform = "#a { transform: translateX(1em) perspective(2em); transform-origin: right 1em; \
                     translate: 1em; perspective: 3em; perspective-origin: left }";
    assert_eq!(
        computed(html, transform, "a", "transform"),
        "translate(16px, 0px) perspective(32px)"
    );
    assert_eq!(
        computed(html, transform, "a", "transform-origin"),
        "100% 16px"
    );
    assert_eq!(computed(html, transform, "a", "translate"), "16px");
    assert_eq!(computed(html, transform, "a", "perspective"), "48px");
    assert_eq!(
        computed(html, transform, "a", "perspective-origin"),
        "0% 50%"
    );
    // Border spacing computes to absolute lengths, clamped at zero.
    let spacing = "#a { border-spacing: calc(10px + 0.5em) calc(10px - 0.5em) }";
    assert_eq!(computed(html, spacing, "a", "border-spacing"), "18px 2px");
    let negative = "#a { border-spacing: calc(10px - 1em) 1em }";
    assert_eq!(computed(html, negative, "a", "border-spacing"), "0px 16px");
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
         width: 5px\nheight: 2px\ntext-decoration-color: rgb(255, 0, 0)\n\
         outline-color: rgb(255, 0, 0)\n"
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

#[test]
fn catalog_properties_compute() {
    let html = "<div id=p><span id=a></span><span id=b></span><p id=c></p></div>";
    let sheet = "#p { font-size: 10px; line-height: 20px; direction: rtl; color: green } \
                 #a { float: left; overflow-x: visible; overflow-y: hidden; opacity: 2; \
                 top: -1em; text-indent: 2em; letter-spacing: 0.1em; vertical-align: 50%; \
                 text-decoration: underline; font-stretch: condensed; \
                 background: url(x) right 10px bottom 20% / 2em auto, center / 50% 1em } \
                 #b { position: absolute; float: right; overflow: clip scroll; opacity: -1; \
                 background-position: left 1em top, 3em } \
                 #c { text-align: match-parent; word-spacing: calc(1em + 10%) }";
    let c = |id, property| computed(html, sheet, id, property);
    assert_eq!(c("a", "display"), "block");
    assert_eq!(c("a", "float"), "left");
    assert_eq!(c("a", "overflow-x"), "auto");
    assert_eq!(c("a", "overflow-y"), "hidden");
    assert_eq!(c("a", "opacity"), "1");
    assert_eq!(c("a", "top"), "-10px");
    assert_eq!(c("a", "text-indent"), "20px");
    assert_eq!(c("a", "letter-spacing"), "1px");
    assert_eq!(c("a", "vertical-align"), "10px");
    assert_eq!(c("a", "text-decoration-color"), "rgb(0, 128, 0)");
    assert_eq!(c("a", "font-stretch"), "75%");
    assert_eq!(
        c("a", "background-position"),
        "calc(100% - 10px) 80%, 50% 50%"
    );
    assert_eq!(c("a", "background-size"), "20px auto, 50% 10px");
    assert_eq!(c("b", "display"), "block");
    assert_eq!(c("b", "float"), "none");
    assert_eq!(c("b", "overflow-x"), "hidden");
    assert_eq!(c("b", "overflow-y"), "scroll");
    assert_eq!(c("b", "opacity"), "0");
    assert_eq!(c("b", "background-position"), "10px 0%, 30px 50%");
    // Inherited text properties carry computed values.
    assert_eq!(c("c", "text-align"), "right");
    assert_eq!(c("c", "direction"), "rtl");
    assert_eq!(c("c", "word-spacing"), "calc(10% + 10px)");
    assert_eq!(c("c", "letter-spacing"), "normal");
    assert_eq!(c("p", "text-indent"), "0px");
    let sheet = "#p { text-indent: 2em; font-size: 10px } #c { font-size: 20px }";
    assert_eq!(computed(html, sheet, "c", "text-indent"), "20px");
}

#[test]
fn table_presentational_hints_map_to_css_below_author_rules() {
    let html = "<table id=t cellspacing=5 cellpadding=3 border=2 width=50% bgcolor=fc0 \
                align=center><tr id=r valign=top bgcolor='red; display: none'>\
                <td id=c width=0 height=20 nowrap align=right>x</td></tr></table>\
                <img id=i width=40 height=30.5>";
    let none = "#none { color: red }";
    let value = |id, property| computed(html, none, id, property);
    assert_eq!(value("t", "border-spacing"), "5px");
    assert_eq!(value("t", "width"), "50%");
    // HTML's legacy color parser: three digits without `#` are three
    // one-digit components (0f, 0c, 00), as in every browser.
    assert_eq!(value("t", "background-color"), "rgb(15, 12, 0)");
    assert_eq!(value("t", "margin-left"), "auto");
    assert_eq!(value("t", "border-top-width"), "2px");
    assert_eq!(value("t", "border-top-style"), "outset");
    // Table attributes reach the cells; zero cell widths are ignored.
    assert_eq!(value("c", "padding-top"), "3px");
    assert_eq!(value("c", "border-left-style"), "inset");
    assert_eq!(value("c", "width"), "auto");
    assert_eq!(value("c", "height"), "20px");
    assert_eq!(value("c", "white-space"), "nowrap");
    assert_eq!(value("c", "text-align"), "right");
    assert_eq!(value("r", "vertical-align"), "top");
    // Attribute text never reaches the CSS parser: the legacy color
    // parser turns it into hex digits (`0ed00d0000a0000000e` → #ed0000).
    assert_eq!(value("r", "background-color"), "rgb(237, 0, 0)");
    // (No user-agent sheet here, so `display` keeps its initial value.)
    assert_eq!(value("r", "display"), "inline");
    assert_eq!(value("i", "width"), "40px");
    assert_eq!(value("i", "height"), "30.5px");
    // Any author rule beats a hint, even with zero specificity.
    assert_eq!(
        computed(html, "* { padding: 9px }", "c", "padding-top"),
        "9px"
    );
}

#[test]
fn legacy_presentational_hints_cover_body_font_hr_lists_and_embedded_content() {
    let html = "<body id=b text=navy bgcolor=#ffffff link=green marginwidth=4 topmargin=6>\
                <a id=a href=x>l</a><a id=n>n</a>\
                <font id=f color=red size=+1 face='Georgia, x;y'>f</font>\
                <hr id=h align=left noshade size=5 width=50%>\
                <ol id=o type=A><li id=l type=i>x</li></ol><ul id=u type=SQUARE></ul>\
                <img id=i hspace=3 vspace=2 align=right><div id=d align=center></div>\
                <pre id=p wrap></pre><table id=t border=1 bordercolor=blue></table></body>";
    let value = |id, property| computed(html, "", id, property);
    assert_eq!(value("b", "color"), "rgb(0, 0, 128)");
    assert_eq!(value("b", "background-color"), "rgb(255, 255, 255)");
    assert_eq!(value("b", "margin-left"), "4px");
    assert_eq!(value("b", "margin-top"), "6px");
    assert_eq!(value("a", "color"), "rgb(0, 128, 0)");
    assert_eq!(value("n", "color"), "rgb(0, 0, 128)", "only links");
    assert_eq!(value("f", "color"), "rgb(255, 0, 0)");
    assert_eq!(value("f", "font-size"), "18px");
    assert_eq!(value("f", "font-family"), "\"Georgia\"");
    assert_eq!(value("h", "margin-right"), "auto");
    assert_eq!(value("h", "border-top-style"), "solid");
    assert_eq!(value("h", "height"), "3px");
    assert_eq!(value("h", "width"), "50%");
    assert_eq!(value("o", "list-style-type"), "upper-alpha");
    assert_eq!(value("l", "list-style-type"), "lower-roman");
    assert_eq!(value("u", "list-style-type"), "square");
    assert_eq!(value("i", "margin-left"), "3px");
    assert_eq!(value("i", "margin-bottom"), "2px");
    assert_eq!(value("i", "float"), "right");
    assert_eq!(value("d", "text-align"), "center");
    assert_eq!(value("p", "white-space"), "pre-wrap");
    assert_eq!(value("t", "border-top-color"), "rgb(0, 0, 255)");
}

#[test]
fn logical_properties_map_through_the_writing_mode() {
    let html = "<div id=h></div><div id=r></div><div id=v></div><div id=s></div>";
    let sheet = "div { margin-inline-start: 7px; padding-block-end: 3px; inline-size: 40px; \
                 border-start-end-radius: 2px } #v { writing-mode: vertical-rl } \
                 #s { writing-mode: sideways-lr } #r { direction: rtl }";
    let at = |id, p| computed(html, sheet, id, p);
    assert_eq!(at("h", "margin-left"), "7px");
    assert_eq!(at("h", "padding-bottom"), "3px");
    assert_eq!(at("h", "width"), "40px");
    assert_eq!(at("h", "border-top-right-radius"), "2px");
    assert_eq!(at("r", "margin-right"), "7px");
    assert_eq!(at("r", "margin-left"), "0px");
    assert_eq!(at("v", "margin-top"), "7px");
    assert_eq!(at("v", "padding-left"), "3px");
    assert_eq!(at("v", "height"), "40px");
    assert_eq!(at("v", "width"), "auto");
    assert_eq!(at("v", "border-bottom-right-radius"), "2px");
    assert_eq!(at("s", "margin-bottom"), "7px");
    assert_eq!(at("s", "padding-right"), "3px");
    // A logical longhand computes to its physical counterpart's value.
    assert_eq!(at("v", "margin-inline-start"), "7px");
    assert_eq!(at("v", "inline-size"), "40px");
    assert_eq!(at("h", "margin-block-start"), "0px");
}

#[test]
fn the_later_of_a_logical_and_a_physical_declaration_wins() {
    let html = "<div id=a></div><div id=b></div><div id=c></div><div id=d></div>";
    let sheet = "#a { margin-left: 1px; margin-inline-start: 2px } \
                 #b { margin-inline-start: 2px; margin-left: 1px } \
                 #c { margin-inline: 4px 5px; margin: 0 9px } \
                 div#d { width: 10px } #d { inline-size: 20px }";
    assert_eq!(computed(html, sheet, "a", "margin-left"), "2px");
    assert_eq!(computed(html, sheet, "b", "margin-left"), "1px");
    assert_eq!(computed(html, sheet, "b", "margin-inline-start"), "1px");
    assert_eq!(computed(html, sheet, "c", "margin-right"), "9px");
    // Specificity still decides before order.
    assert_eq!(computed(html, sheet, "d", "width"), "10px");
}

#[test]
fn logical_values_inherit_substitute_and_reset() {
    let html = "<div id=p><div id=c></div></div><div id=w></div><div id=x></div>";
    let sheet = "#p { margin-inline-start: 6px; margin-top: 2px } \
                 #c { writing-mode: vertical-lr; margin-inline-start: inherit; \
                 --b: 8px; border-block: var(--b) solid } \
                 #w { writing-mode: tb-rl } #x { writing-mode: lr }";
    // `inherit` on a logical longhand takes the parent's logical value.
    assert_eq!(computed(html, sheet, "c", "margin-top"), "6px");
    assert_eq!(computed(html, sheet, "c", "border-left-width"), "8px");
    assert_eq!(computed(html, sheet, "c", "border-right-style"), "solid");
    assert_eq!(computed(html, sheet, "c", "border-top-style"), "none");
    assert_eq!(computed(html, sheet, "w", "writing-mode"), "vertical-rl");
    assert_eq!(computed(html, sheet, "x", "writing-mode"), "horizontal-tb");
    assert_eq!(computed(html, sheet, "c", "writing-mode"), "vertical-lr");
}

#[test]
fn details_dialogs_and_popovers_style_from_their_state() {
    let parser = CssParser::default();
    let mut page = document(
        "<details id=closed><summary id=s>S</summary><p id=hidden>x</p></details>\
         <details open><p id=shown>x</p></details>\
         <dialog id=dialog open></dialog><dialog id=modal open></dialog>\
         <div popover id=pop></div><div popover id=shown-pop></div>",
    );
    let find = |doc: &Document, id: &str| {
        doc.walk(doc.root())
            .unwrap()
            .map(|(n, _)| n)
            .find(|&n| id_of(doc, n).as_deref() == Some(id))
            .unwrap()
    };
    let modal = find(page.document(), "modal");
    let shown = find(page.document(), "shown-pop");
    page.document_mut()
        .top_layer_add(modal, rustic_dom::TopLayer::Modal)
        .unwrap();
    page.document_mut()
        .top_layer_add(shown, rustic_dom::TopLayer::Popover)
        .unwrap();
    let mut cascade = Cascade::new();
    let (sheet, diagnostics) =
        StyleSheet::parse(&parser, HTML_USER_AGENT_STYLESHEET, Origin::UserAgent).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    cascade.push(sheet);
    let doc = page.document();
    let styles = style_tree(
        doc,
        doc.root(),
        &cascade,
        &parser,
        &MatchContext::html(),
        &ComputeContext::default(),
    )
    .unwrap();
    let value = |id: &str, property: &str| {
        let style = styles
            .iter()
            .find(|s| id_of(doc, s.node).as_deref() == Some(id))
            .unwrap_or_else(|| panic!("no element #{id}"));
        style
            .computed
            .get(PropertyId::from_name(property).unwrap())
            .to_string()
    };
    assert_eq!(value("hidden", "display"), "none");
    assert_eq!(value("s", "display"), "list-item");
    assert_eq!(value("shown", "display"), "block");
    assert_eq!(value("dialog", "position"), "absolute");
    assert_eq!(value("modal", "position"), "fixed");
    assert_eq!(value("modal", "z-index"), "2147483647");
    assert_eq!(value("pop", "display"), "none");
    assert_eq!(value("shown-pop", "display"), "block");
    assert_eq!(value("shown-pop", "position"), "fixed");
}
