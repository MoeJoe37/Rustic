//! Stylesheets for HTML documents: the user-agent stylesheet subset (HTML
//! §15 Rendering) and the author sheets from `<style>` elements, plus a
//! deterministic computed-style dump.

use std::collections::HashMap;

use rustic_css::{BorderWidth, CssParser, PropertyId, PropertyValue};
use rustic_dom::{
    Document, DomError, HTML_NAMESPACE, MATHML_NAMESPACE, NodeId, NodeKind, SVG_NAMESPACE,
};

use crate::{
    Cascade, ComputedStyle, DomElement, ElementStyle, ElementView, Origin, StyleDiagnostic,
    StyleSheet,
};

/// The supported subset of the HTML user-agent stylesheet (HTML §15.3).
///
/// Rules use only the properties Rustic parses today. Logical properties
/// such as `margin-inline-start` are written as their left-to-right physical
/// equivalents until writing modes exist.
pub const HTML_USER_AGENT_STYLESHEET: &str = r#"@namespace "http://www.w3.org/1999/xhtml";

[hidden], area, base, basefont, datalist, head, link, meta, noembed,
noframes, param, rp, script, style, template, title { display: none }

html, body { display: block }
body { margin: 8px }

address, blockquote, center, dialog, div, figure, figcaption, footer, form,
header, hr, legend, listing, main, p, plaintext, pre, search, xmp { display: block }
blockquote, figure, listing, p, plaintext, pre, xmp { margin-top: 1em; margin-bottom: 1em }
blockquote, figure { margin-left: 40px; margin-right: 40px }
address { font-style: italic }
listing, plaintext, pre, xmp { font-family: monospace }
dialog:not([open]) { display: none }

article, aside, h1, h2, h3, h4, h5, h6, hgroup, nav, section { display: block }
h1 { margin-top: 0.67em; margin-bottom: 0.67em; font-size: 2em; font-weight: bold }
h2 { margin-top: 0.83em; margin-bottom: 0.83em; font-size: 1.5em; font-weight: bold }
h3 { margin-top: 1em; margin-bottom: 1em; font-size: 1.17em; font-weight: bold }
h4 { margin-top: 1.33em; margin-bottom: 1.33em; font-size: 1em; font-weight: bold }
h5 { margin-top: 1.67em; margin-bottom: 1.67em; font-size: 0.83em; font-weight: bold }
h6 { margin-top: 2.33em; margin-bottom: 2.33em; font-size: 0.67em; font-weight: bold }

dir, dd, dl, dt, menu, ol, ul { display: block }
li { display: list-item }
dir, dl, menu, ol, ul { margin-top: 1em; margin-bottom: 1em }
:is(dir, dl, menu, ol, ul) :is(dir, dl, menu, ol, ul) { margin-top: 0; margin-bottom: 0 }
dd { margin-left: 40px }
dir, menu, ol, ul { padding-left: 40px }

table { display: table; box-sizing: border-box }
caption { display: table-caption }
colgroup { display: table-column-group }
col { display: table-column }
thead { display: table-header-group }
tbody { display: table-row-group }
tfoot { display: table-footer-group }
tr { display: table-row }
td, th { display: table-cell; padding: 1px }
th { font-weight: bold }

b, strong { font-weight: bolder }
i, cite, dfn, em, var { font-style: italic }
code, kbd, samp, tt { font-family: monospace }
big { font-size: larger }
small, sub, sup { font-size: smaller }

hr { color: gray; border-style: inset; border-width: 1px; margin: 0.5em auto }
fieldset { display: block; margin-left: 2px; margin-right: 2px; border: 2px groove gray }
"#;

/// The parsed [`HTML_USER_AGENT_STYLESHEET`].
pub fn html_user_agent_sheet(parser: &CssParser) -> StyleSheet {
    StyleSheet::parse(parser, HTML_USER_AGENT_STYLESHEET, Origin::UserAgent)
        .map(|(sheet, _)| sheet)
        .unwrap_or(StyleSheet {
            origin: Origin::UserAgent,
            rules: Vec::new(),
        })
}

/// What happened to one `<style>` element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StyleElementStatus {
    /// Its sheet was added to the cascade.
    Applied,
    /// Its `type` attribute is not empty or `text/css`, so it is not CSS.
    NotCss,
    /// Its `media` attribute needs media queries, which are not implemented
    /// yet; only an empty list, `all` and `screen` are applied.
    UnsupportedMedia,
    /// Its contents exceeded the CSS parser's limits.
    ResourceLimit,
}

/// One `<style>` element found by [`document_cascade`].
#[derive(Clone, Debug, PartialEq)]
pub struct StyleElementSheet {
    /// The `<style>` element.
    pub node: NodeId,
    /// Whether its sheet was applied.
    pub status: StyleElementStatus,
    /// Constructs dropped from its sheet (spans are within its text).
    pub diagnostics: Vec<StyleDiagnostic>,
}

fn attribute(element: DomElement<'_>, name: &str) -> Option<String> {
    element
        .attributes()
        .find(|a| a.namespace.is_none() && a.local == name)
        .map(|a| String::from_utf16_lossy(a.value))
}

fn trim_ascii_whitespace(s: &str) -> &str {
    s.trim_matches(|c: char| matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' '))
}

fn media_applies(media: &str) -> bool {
    let media = trim_ascii_whitespace(media);
    media.is_empty()
        || media.split(',').any(|query| {
            let query = trim_ascii_whitespace(query);
            query.eq_ignore_ascii_case("all") || query.eq_ignore_ascii_case("screen")
        })
}

/// The child text content of `id` (its direct text children, concatenated).
fn child_text(document: &Document, id: NodeId) -> String {
    let mut units = Vec::new();
    if let Ok(children) = document.children(id) {
        for child in children {
            if let Ok(NodeKind::Text(data)) = document.node(child).map(|n| n.kind()) {
                units.extend_from_slice(data.as_units());
            }
        }
    }
    String::from_utf16_lossy(&units)
}

/// A cascade for an HTML document: the user-agent stylesheet, then the sheet
/// of every HTML `<style>` element in tree order (template contents are not
/// searched), each reported in the returned list.
///
/// External stylesheets (`<link rel=stylesheet>`) need fetching and are not
/// loaded.
pub fn document_cascade(
    document: &Document,
    parser: &CssParser,
) -> Result<(Cascade, Vec<StyleElementSheet>), DomError> {
    let mut cascade = Cascade::new();
    cascade.push(html_user_agent_sheet(parser));
    let mut reports = Vec::new();
    for (id, _) in document.walk(document.root())? {
        let Some(element) = DomElement::new(document, id) else {
            continue;
        };
        if element.local_name() != "style" || element.namespace() != Some(HTML_NAMESPACE) {
            continue;
        }
        let css_type = attribute(element, "type").is_none_or(|t| {
            let t = trim_ascii_whitespace(&t);
            t.is_empty() || t.eq_ignore_ascii_case("text/css")
        });
        let (status, diagnostics) = if !css_type {
            (StyleElementStatus::NotCss, Vec::new())
        } else if !attribute(element, "media").is_none_or(|m| media_applies(&m)) {
            (StyleElementStatus::UnsupportedMedia, Vec::new())
        } else {
            match StyleSheet::parse(parser, &child_text(document, id), Origin::Author) {
                Ok((sheet, diagnostics)) => {
                    cascade.push(sheet);
                    (StyleElementStatus::Applied, diagnostics)
                }
                Err(_) => (StyleElementStatus::ResourceLimit, Vec::new()),
            }
        };
        reports.push(StyleElementSheet {
            node: id,
            status,
            diagnostics,
        });
    }
    Ok((cascade, reports))
}

fn label(element: DomElement<'_>) -> String {
    let mut out = String::new();
    match element.namespace() {
        Some(HTML_NAMESPACE) => {}
        Some(SVG_NAMESPACE) => out.push_str("svg "),
        Some(MATHML_NAMESPACE) => out.push_str("math "),
        _ => out.push_str("? "),
    }
    out.push_str(element.local_name());
    if let Some(id) = attribute(element, "id").filter(|id| !id.is_empty()) {
        out.push('#');
        out.push_str(&id);
    }
    if let Some(classes) = attribute(element, "class") {
        for class in classes.split(|c: char| c.is_ascii_whitespace()) {
            if !class.is_empty() {
                out.push('.');
                out.push_str(class);
            }
        }
    }
    out
}

fn is_zero_width(value: &PropertyValue) -> bool {
    matches!(value, PropertyValue::BorderWidth(BorderWidth::Length(l)) if l.value == 0.0)
}

/// A deterministic, indented dump of `styles` (as returned by
/// [`crate::style_tree`] for `document`): one line per element with its tag,
/// `#id` and `.class`es, then one `property: value` line per computed value
/// that the element does not simply get by default. That is, an inherited
/// property is listed where it differs from the parent element's computed
/// value (the root's is compared with the initial value), and any other
/// property where it differs from its initial value. A border color is listed
/// only when that side's border width is not zero, since it has no effect
/// otherwise.
///
/// Returns `None` once the dump would exceed `max_bytes`.
pub fn dump_computed_styles(
    document: &Document,
    styles: &[ElementStyle],
    max_bytes: usize,
) -> Option<String> {
    use PropertyId as P;
    const BORDERS: [(PropertyId, PropertyId); 4] = [
        (P::BorderTopColor, P::BorderTopWidth),
        (P::BorderRightColor, P::BorderRightWidth),
        (P::BorderBottomColor, P::BorderBottomWidth),
        (P::BorderLeftColor, P::BorderLeftWidth),
    ];
    let initial = ComputedStyle::initial();
    let mut out = String::new();
    // Styles come in tree order, so a parent's entry exists before its
    // children's: (depth, index into `styles`).
    let mut seen: HashMap<NodeId, (usize, usize)> = HashMap::new();
    for (index, style) in styles.iter().enumerate() {
        let Some(element) = DomElement::new(document, style.node) else {
            continue;
        };
        let parent = element.parent_element().and_then(|p| seen.get(&p.id()));
        let depth = parent.map_or(0, |&(d, _)| d + 1);
        let inherited_from = parent.map_or(&initial, |&(_, i)| &styles[i].computed);
        seen.insert(style.node, (depth, index));
        let indent = "  ".repeat(depth);
        out.push_str(&indent);
        out.push_str(&label(element));
        out.push('\n');
        for &property in PropertyId::ALL {
            let value = style.computed.get(property);
            let default = if property.inherited() {
                inherited_from.get(property)
            } else {
                initial.get(property)
            };
            let invisible = BORDERS.iter().any(|&(color, width)| {
                property == color && is_zero_width(style.computed.get(width))
            });
            if value != default && !invisible {
                out.push_str(&indent);
                out.push_str("  ");
                out.push_str(property.name());
                out.push_str(": ");
                out.push_str(&value.to_string());
                out.push('\n');
            }
        }
        if out.len() > max_bytes {
            return None;
        }
    }
    Some(out)
}
