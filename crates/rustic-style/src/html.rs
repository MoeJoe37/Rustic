//! Stylesheets for HTML documents: the user-agent stylesheet subset (HTML
//! §15 Rendering) and the author sheets from `<style>` elements, plus a
//! deterministic computed-style dump.

use std::collections::HashMap;

use rustic_css::{BorderWidth, CssParser, PropertyId, PropertyValue};
use rustic_dom::{
    Document, DomError, HTML_NAMESPACE, MATHML_NAMESPACE, NodeId, NodeKind, SVG_NAMESPACE,
};

use rustic_url::Url;

use crate::{
    Cascade, ComputedStyle, DomElement, ElementStyle, ElementView, ImportReport, NoFetch, Origin,
    SheetStatus, StyleDiagnostic, StyleLoadLimits, StyleLoader, StyleSheet, TreeContext,
    shadow_tree_context,
};

/// The supported subset of the HTML user-agent stylesheet (HTML §15.3).
///
/// Rules use only the properties Rustic parses today, with the HTML
/// specification's flow-relative properties (`margin-block`,
/// `padding-inline-start`).
pub const HTML_USER_AGENT_STYLESHEET: &str = r#"@namespace "http://www.w3.org/1999/xhtml";
@namespace m "http://www.w3.org/1998/Math/MathML";

m|math { display: inline; font-family: math, serif; font-style: normal;
  font-weight: normal; line-height: normal; text-indent: 0; direction: ltr }
m|math[display=block i] { display: block; text-align: center; margin-block: 1em }
m|annotation, m|annotation-xml, m|none, m|mprescripts { display: none }
m|mi { font-style: italic }
m|mo { padding-inline: 0.2em }
m|mfrac { display: inline-flex; flex-direction: column; align-items: center;
  vertical-align: 0.55em; padding-inline: 0.1em }
m|mfrac > * { display: block; font-size: 90%; text-align: center }
m|munder, m|mover, m|munderover { display: inline-flex; align-items: center }
m|munder, m|munderover { flex-direction: column }
m|mover { flex-direction: column-reverse }
m|msqrt, m|mroot { display: inline-block; border-top: 0.06em solid;
  margin-inline-start: 0.7em; padding-inline: 0.1em; padding-top: 0.1em }
m|mtable { display: inline-table; vertical-align: middle }
m|mtr { display: table-row }
m|mtd { display: table-cell; padding: 0.2em 0.4em; text-align: center }
m|mspace { display: inline-block }
m|menclose { display: inline-block; border: 0.06em solid; padding: 0.1em }
m|merror { border: 1px solid red; background-color: lightyellow }
m|mphantom { visibility: hidden }

[hidden], area, base, basefont, datalist, head, link, meta, noembed,
noframes, param, rp, script, style, template, title { display: none }

html, body { display: block }
body { margin: 8px }

address, blockquote, center, dialog, div, figure, figcaption, footer, form,
header, hr, legend, listing, main, p, plaintext, pre, search, xmp { display: block }
blockquote, figure, listing, p, plaintext, pre, xmp { margin-block: 1em }
blockquote, figure { margin-inline: 40px }
address { font-style: italic }
listing, plaintext, pre, xmp { font-family: monospace; white-space: pre }
dialog:not([open]) { display: none }

details { display: block }
summary { display: list-item; list-style: disclosure-closed inside }
details[open] > summary { list-style-type: disclosure-open }
details:not([open]) > :not(summary) { display: none }
dialog { position: absolute; inset-inline-start: 0; inset-inline-end: 0;
  width: fit-content; height: fit-content; margin: auto; border: solid;
  padding: 1em; background-color: white; color: black }
dialog:modal { position: fixed; overflow: auto; inset-block: 0;
  max-width: calc(100% - 6px - 2em); max-height: calc(100% - 6px - 2em) }
[popover] { position: fixed; inset: 0; width: fit-content; height: fit-content;
  margin: auto; border: solid; padding: 0.25em; overflow: auto;
  color: black; background-color: white }
[popover]:not(:popover-open):not(dialog[open]) { display: none }
dialog:popover-open { display: block }
:modal, :popover-open { z-index: 2147483647 }

article, aside, h1, h2, h3, h4, h5, h6, hgroup, nav, section { display: block }
h1 { margin-block: 0.67em; font-size: 2em; font-weight: bold }
h2 { margin-block: 0.83em; font-size: 1.5em; font-weight: bold }
h3 { margin-block: 1em; font-size: 1.17em; font-weight: bold }
h4 { margin-block: 1.33em; font-size: 1em; font-weight: bold }
h5 { margin-block: 1.67em; font-size: 0.83em; font-weight: bold }
h6 { margin-block: 2.33em; font-size: 0.67em; font-weight: bold }

dir, dd, dl, dt, menu, ol, ul { display: block }
li { display: list-item }
dir, dl, menu, ol, ul { margin-block: 1em }
:is(dir, dl, menu, ol, ul) :is(dir, dl, menu, ol, ul) { margin-block: 0 }
dd { margin-inline-start: 40px }
dir, menu, ol, ul { padding-inline-start: 40px }
ol { list-style-type: decimal }
:is(dir, menu, ol, ul) :is(dir, menu, ul) { list-style-type: circle }
:is(dir, menu, ol, ul) :is(dir, menu, ol, ul) :is(dir, menu, ul) { list-style-type: square }

table { display: table; box-sizing: border-box; border-spacing: 2px; border-collapse: separate; text-indent: initial }
caption { display: table-caption; text-align: center }
colgroup { display: table-column-group }
col { display: table-column }
thead { display: table-header-group }
tbody { display: table-row-group }
tfoot { display: table-footer-group }
tr { display: table-row }
td, th { display: table-cell; padding: 1px }
thead, tbody, tfoot, table > tr { vertical-align: middle }
tr, td, th { vertical-align: inherit }
th { font-weight: bold; text-align: center }

b, strong { font-weight: bolder }
i, cite, dfn, em, var { font-style: italic }
code, kbd, samp, tt { font-family: monospace }
big { font-size: larger }
small, sub, sup { font-size: smaller }
sub { vertical-align: sub }
sup { vertical-align: super }
nobr { white-space: nowrap }
:link { color: #0000ee; text-decoration: underline }
u, ins { text-decoration: underline }
s, strike, del { text-decoration: line-through }

[dir=ltr i] { direction: ltr; unicode-bidi:isolate }
[dir=rtl i] { direction: rtl; unicode-bidi:isolate }
[dir=auto i] { unicode-bidi:plaintext }
bdi { unicode-bidi:isolate }
bdi:not([dir]) { unicode-bidi:plaintext }
bdo, bdo[dir] { unicode-bidi:bidi-override }

hr { color: gray; border-style: inset; border-width: 1px; margin: 0.5em auto }
fieldset { display: block; margin-inline: 2px; border: 2px groove gray;
  padding-block: 0.35em 0.625em; padding-inline: 0.75em; min-inline-size: min-content }
legend { padding-inline: 2px }

rp { display: none }
rt { display: block; font-size: 50%; line-height: 1; text-align: center; text-indent: 0 }

input, select, textarea, button, meter, progress { display: inline-block }
input, select, textarea, button { font-size: 13.333px; font-family: sans-serif;
  font-weight: normal; font-style: normal; line-height: normal; color: black;
  letter-spacing: normal; word-spacing: normal; text-transform: none;
  text-indent: 0; text-align: start; margin: 0 }
input { padding: 1px 2px; border: 1px solid #767676; border-radius: 2px;
  background-color: white; white-space: pre; inline-size: 20ch }
input[type=hidden i] { display: none }
input:is([type=checkbox i], [type=radio i]) { box-sizing: border-box;
  inline-size: 13px; block-size: 13px; padding: 0; margin: 3px 3px 3px 4px }
input[type=radio i] { border-radius: 50%; margin-inline: 5px 3px }
input:is([type=submit i], [type=reset i], [type=button i], [type=file i]),
button { padding: 1px 6px; border: 1px solid #767676; border-radius: 2px;
  background-color: #efefef; text-align: center; inline-size: auto;
  white-space: pre }
input[type=file i] { border: none; background-color: transparent; padding: 0 }
input[type=range i] { inline-size: 129px; block-size: 16px; padding: 0;
  margin: 2px; border: none; background-color: transparent }
input[type=color i] { box-sizing: border-box; inline-size: 50px; block-size: 27px;
  padding: 4px 6px; background-color: #efefef }
select { padding: 1px 4px; border: 1px solid #767676; border-radius: 2px;
  background-color: white; white-space: pre }
textarea { font-family: monospace; padding: 2px; border: 1px solid #767676;
  background-color: white; white-space: pre-wrap; overflow-wrap: break-word;
  inline-size: 20ch; block-size: 2.4em; overflow: auto; vertical-align: text-bottom }
meter { inline-size: 5em; block-size: 1em; vertical-align: -0.2em;
  background-color: #efefef; border-radius: 2px }
progress { inline-size: 10em; block-size: 1em; vertical-align: -0.2em;
  background-color: #efefef; border-radius: 2px }
"#;

/// The parsed [`HTML_USER_AGENT_STYLESHEET`].
pub fn html_user_agent_sheet(parser: &CssParser) -> StyleSheet {
    StyleSheet::parse(parser, HTML_USER_AGENT_STYLESHEET, Origin::UserAgent)
        .map(|(sheet, _)| sheet)
        .unwrap_or(StyleSheet {
            origin: Origin::UserAgent,
            ..StyleSheet::default()
        })
}

/// What happened to one `<style>` or `<link rel=stylesheet>` element's
/// sheet (the name predates linked sheets).
pub type StyleElementStatus = SheetStatus;

/// One `<style>` or `<link rel=stylesheet>` element found by
/// [`document_cascade`] or [`document_cascade_with`].
#[derive(Clone, Debug, PartialEq)]
pub struct StyleElementSheet {
    /// The `<style>` or `<link>` element.
    pub node: NodeId,
    /// For a `<link>`, its `href` attribute as written.
    pub href: Option<String>,
    /// Whether its sheet was applied.
    pub status: StyleElementStatus,
    /// Constructs dropped from its sheet (spans are within its text).
    pub diagnostics: Vec<StyleDiagnostic>,
    /// Its `@import` rules and theirs, in preorder.
    pub imports: Vec<ImportReport>,
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

/// The HTML `<style>` and `<link>` elements of the document tree that
/// may own a stylesheet, in tree order, with each `<style>`'s text (empty
/// for a link): what `document.styleSheets` lists (CSSOM §6.2).
pub fn document_style_owners(document: &Document) -> Vec<(NodeId, bool, String)> {
    let mut out = Vec::new();
    let Ok(walk) = document.walk(document.root()) else {
        return out;
    };
    for (id, _) in walk {
        let Some(element) = DomElement::new(document, id) else {
            continue;
        };
        if is_html(element, "style") {
            let css = attribute(element, "type").is_none_or(|t| {
                t.is_empty() || trim_ascii_whitespace(&t).eq_ignore_ascii_case("text/css")
            });
            if css {
                out.push((id, true, child_text(document, id)));
            }
        } else if is_html(element, "link")
            && attribute(element, "rel")
                .and_then(|r| stylesheet_link(&r))
                .is_some()
            && attribute(element, "href").is_some_and(|h| !h.is_empty())
        {
            out.push((id, false, String::new()));
        }
    }
    out
}

/// A cascade for an HTML document without external stylesheets: the
/// user-agent stylesheet, then the sheet of every HTML `<style>` element in
/// tree order (template contents are not searched), each reported in the
/// returned list.
///
/// `<link rel=stylesheet>` elements and `@import` rules are reported as
/// [`FetchError::Blocked`](crate::FetchError::Blocked); use [`document_cascade_with`] to load them.
pub fn document_cascade(
    document: &Document,
    parser: &CssParser,
) -> Result<(Cascade, Vec<StyleElementSheet>), DomError> {
    let mut fetcher = NoFetch;
    let mut loader = StyleLoader::new(&mut fetcher, StyleLoadLimits::default());
    document_cascade_with(document, parser, None, &mut loader)
}

/// Is `rel` a link-type list containing `stylesheet`, and also `alternate`?
fn stylesheet_link(rel: &str) -> Option<bool> {
    let mut stylesheet = false;
    let mut alternate = false;
    for token in rel.split(['\t', '\n', '\x0c', '\r', ' ']) {
        stylesheet |= token.eq_ignore_ascii_case("stylesheet");
        alternate |= token.eq_ignore_ascii_case("alternate");
    }
    stylesheet.then_some(alternate)
}

fn is_html(element: DomElement<'_>, name: &str) -> bool {
    element.local_name() == name && element.namespace() == Some(HTML_NAMESPACE)
}

/// The document base URL (HTML §2.4.1): the first `<base href>` in tree
/// order resolved against `document_url`, or `document_url` itself.
fn base_url(
    document: &Document,
    document_url: Option<&Url>,
    loader: &StyleLoader<'_>,
) -> Result<Option<Url>, DomError> {
    for (id, _) in document.walk(document.root())? {
        let Some(element) = DomElement::new(document, id) else {
            continue;
        };
        if is_html(element, "base") {
            if let Some(href) = attribute(element, "href") {
                if let Some(url) = loader.parse_document_url(&href, document_url) {
                    return Ok(Some(url));
                }
                break;
            }
        }
    }
    Ok(document_url.cloned())
}

/// A cascade for an HTML document: the user-agent stylesheet, then the sheet
/// of every HTML `<style>` and `<link rel=stylesheet>` element in
/// shadow-including tree order (template contents are not searched; a sheet
/// in a shadow tree styles that tree only, through its
/// [`crate::shadow_tree_context`]), each preceded by the sheets it
/// `@import`s, depth first.
///
/// Relative URLs resolve against the document base URL: the first
/// `<base href>`, else `document_url`. Every external sheet is fetched
/// through `loader`, which applies its fetcher's policy and its budgets; a
/// sheet that cannot be loaded is reported and skipped, never fatal. Each
/// element is reported in the returned list with its imports.
pub fn document_cascade_with(
    document: &Document,
    parser: &CssParser,
    document_url: Option<&Url>,
    loader: &mut StyleLoader<'_>,
) -> Result<(Cascade, Vec<StyleElementSheet>), DomError> {
    let base = base_url(document, document_url, loader)?;
    let mut cascade = Cascade::new();
    cascade.set_document_base_url(base.clone());
    cascade.set_inline_policy(loader.inline_policy().cloned());
    cascade.push(html_user_agent_sheet(parser));
    let mut reports = Vec::new();
    // An explicit stack of (walk, tree) pairs for nested shadow trees.
    let mut walks = vec![(document.walk(document.root())?, TreeContext::DOCUMENT)];
    while let Some((walk, context)) = walks.last_mut() {
        let context = *context;
        let Some((id, _)) = walk.next() else {
            walks.pop();
            continue;
        };
        let Some(element) = DomElement::new(document, id) else {
            continue;
        };
        if let Ok(NodeKind::Element(data)) = document.node(id).map(|n| n.kind()) {
            if let Some(shadow_root) = data.shadow_root() {
                walks.push((
                    document.walk(shadow_root)?,
                    shadow_tree_context(shadow_root),
                ));
            }
        }
        let mut report = StyleElementSheet {
            node: id,
            href: None,
            status: SheetStatus::Applied,
            diagnostics: Vec::new(),
            imports: Vec::new(),
        };
        let css_type = attribute(element, "type").is_none_or(|t| {
            let t = trim_ascii_whitespace(&t);
            t.is_empty() || t.eq_ignore_ascii_case("text/css")
        });
        // A `media` attribute restricts the element's whole sheet tree.
        let media = match attribute(element, "media") {
            Some(media) => parser.media_query_list(&media).map(|m| Some(m.value)),
            None => Ok(None),
        };
        let loaded = if is_html(element, "style") {
            let text = child_text(document, id);
            let nonce = attribute(element, "nonce");
            if !css_type {
                Err(SheetStatus::NotCss)
            } else if loader
                .inline_policy()
                .is_some_and(|policy| !policy.allows_style_element(nonce.as_deref(), &text))
            {
                Err(SheetStatus::BlockedByPolicy)
            } else {
                media
                    .and_then(|media| loader.load_tree(parser, &text, base.as_ref(), None, media))
                    .map_err(|_| SheetStatus::ResourceLimit)
            }
        } else if is_html(element, "link") {
            let Some(alternate) = attribute(element, "rel").and_then(|r| stylesheet_link(&r))
            else {
                continue;
            };
            // A link with no or an empty href does not create a sheet.
            let Some(href) = attribute(element, "href").filter(|h| !h.is_empty()) else {
                continue;
            };
            report.href = Some(href.clone());
            if alternate || attribute(element, "disabled").is_some() {
                Err(SheetStatus::Disabled)
            } else if !css_type {
                Err(SheetStatus::NotCss)
            } else {
                match (loader.parse_document_url(&href, base.as_ref()), media) {
                    (None, _) => Err(SheetStatus::InvalidUrl),
                    (_, Err(_)) => Err(SheetStatus::ResourceLimit),
                    (Some(url), Ok(media)) => loader.fetch(&url).and_then(|text| {
                        loader
                            .load_tree(parser, &text, Some(&url), Some(&url), media)
                            .map_err(|_| SheetStatus::ResourceLimit)
                    }),
                }
            }
        } else {
            continue;
        };
        match loaded {
            Ok((tree, diagnostics)) => {
                report.diagnostics = diagnostics;
                report.imports = tree.imports;
                // Implicit `@scope` roots: the owner's parent element, or
                // the shadow host for a sheet in a shadow tree.
                let owner = element
                    .parent_element()
                    .or_else(|| element.parent_shadow_host());
                for mut sheet in tree.sheets {
                    sheet.set_scope_owner(owner.map(|o| o.id()));
                    cascade.push_in(sheet, context);
                }
            }
            Err(status) => report.status = status,
        }
        reports.push(report);
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
/// otherwise (`outline-color` likewise with `outline-width`), and
/// `text-decoration-color` only when a decoration line is
/// set. Custom properties (which all inherit) follow, in name order, where
/// they differ from the parent's.
///
/// Returns `None` once the dump would exceed `max_bytes`.
pub fn dump_computed_styles(
    document: &Document,
    styles: &[ElementStyle],
    max_bytes: usize,
) -> Option<String> {
    use PropertyId as P;
    const BORDERS: [(PropertyId, PropertyId); 5] = [
        (P::BorderTopColor, P::BorderTopWidth),
        (P::OutlineColor, P::OutlineWidth),
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
        let parent = element
            .parent_element()
            .or_else(|| element.parent_shadow_host())
            .and_then(|p| seen.get(&p.id()));
        let depth = parent.map_or(0, |&(d, _)| d + 1);
        let inherited_from = parent.map_or(&initial, |&(_, i)| &styles[i].computed);
        seen.insert(style.node, (depth, index));
        let indent = "  ".repeat(depth);
        out.push_str(&indent);
        out.push_str(&label(element));
        out.push('\n');
        // Logical longhands repeat their physical counterparts' values.
        for &property in PropertyId::ALL.iter().filter(|p| !p.is_logical()) {
            let value = style.computed.get(property);
            let default = if property.inherited() {
                inherited_from.get(property)
            } else {
                initial.get(property)
            };
            let invisible = BORDERS.iter().any(|&(color, width)| {
                property == color && is_zero_width(style.computed.get(width))
            }) || (property == P::TextDecorationColor
                && matches!(
                    style.computed.get(P::TextDecorationLine),
                    PropertyValue::TextDecorationLine(line) if line.is_none()
                ));
            if value != default && !invisible {
                out.push_str(&indent);
                out.push_str("  ");
                out.push_str(property.name());
                out.push_str(": ");
                out.push_str(&value.to_string());
                out.push('\n');
            }
        }
        out.push_str(
            &style
                .computed
                .custom
                .dump_differences(&inherited_from.custom, &format!("{indent}  ")),
        );
        if out.len() > max_bytes {
            return None;
        }
    }
    Some(out)
}
