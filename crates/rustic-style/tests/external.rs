//! External stylesheets: `<link rel=stylesheet>` and `@import` over a
//! caller-controlled fetcher, with budgets, cycles and conditions.

use std::collections::HashMap;

use rustic_css::{CssParser, PropertyId};
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_html::{HtmlDocumentParser, ParsedDocument};
use rustic_style::{
    Cascade, ComputeContext, FetchError, MatchContext, NoFetch, Origin, SheetStatus,
    StyleDiagnosticKind, StyleElementSheet, StyleLoadLimits, StyleLoader, StyleSheet,
    StylesheetFetcher, document_cascade, document_cascade_with, style_tree,
};
use rustic_url::Url;

/// Serves sheets from a map keyed by serialized URL and logs every fetch.
#[derive(Default)]
struct MapFetcher {
    sheets: HashMap<String, String>,
    log: Vec<String>,
}

impl MapFetcher {
    fn new(sheets: &[(&str, &str)]) -> Self {
        Self {
            sheets: sheets
                .iter()
                .map(|(url, text)| (format!("https://site.test/{url}"), (*text).to_owned()))
                .collect(),
            log: Vec::new(),
        }
    }
}

impl StylesheetFetcher for MapFetcher {
    fn fetch(&mut self, url: &Url, max_bytes: usize) -> Result<String, FetchError> {
        let key = url.serialize_without_fragment();
        self.log.push(key.clone());
        let text = self.sheets.get(&key).ok_or(FetchError::NotFound)?;
        if text.len() > max_bytes {
            return Err(FetchError::TooLarge);
        }
        Ok(text.clone())
    }
}

fn document(html: &str) -> ParsedDocument {
    let mut parser = HtmlDocumentParser::default();
    parser.feed(html.as_bytes()).unwrap();
    parser.finish().unwrap()
}

fn page_url() -> Url {
    Url::parse("https://site.test/pages/index.html", None).unwrap()
}

struct Loaded {
    page: ParsedDocument,
    cascade: Cascade,
    reports: Vec<StyleElementSheet>,
    log: Vec<String>,
}

fn load(html: &str, sheets: &[(&str, &str)], limits: StyleLoadLimits) -> Loaded {
    let page = document(html);
    let mut fetcher = MapFetcher::new(sheets);
    let mut loader = StyleLoader::new(&mut fetcher, limits);
    let url = page_url();
    let (cascade, reports) = document_cascade_with(
        page.document(),
        &CssParser::default(),
        Some(&url),
        &mut loader,
    )
    .unwrap();
    Loaded {
        page,
        cascade,
        reports,
        log: fetcher.log,
    }
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

/// The computed `property` of `#id`.
fn computed(loaded: &Loaded, id: &str, property: PropertyId) -> String {
    let doc = loaded.page.document();
    let styles = style_tree(
        doc,
        doc.root(),
        &loaded.cascade,
        &CssParser::default(),
        &MatchContext::html(),
        &ComputeContext::default(),
    )
    .unwrap();
    styles
        .iter()
        .find(|s| id_of(doc, s.node).as_deref() == Some(id))
        .unwrap()
        .computed
        .get(property)
        .to_string()
}

fn statuses(loaded: &Loaded) -> Vec<SheetStatus> {
    loaded.reports.iter().map(|r| r.status).collect()
}

fn import_statuses(report: &StyleElementSheet) -> Vec<(String, usize, SheetStatus)> {
    report
        .imports
        .iter()
        .map(|i| (i.url.clone(), i.depth, i.status))
        .collect()
}

#[test]
fn linked_sheets_join_the_cascade_in_tree_order() {
    let loaded = load(
        "<!doctype html><link rel=stylesheet href=a.css>\
         <style>#p { width: 2px }</style>\
         <link rel=' STYLESHEET ' href='/css/b.css#frag'>\
         <p id=p>text</p>",
        &[
            ("pages/a.css", "#p { width: 1px; height: 1px; color: red }"),
            ("css/b.css", "#p { height: 3px }"),
        ],
        StyleLoadLimits::default(),
    );
    use SheetStatus as S;
    assert_eq!(statuses(&loaded), [S::Applied, S::Applied, S::Applied]);
    assert_eq!(loaded.reports[0].href.as_deref(), Some("a.css"));
    assert_eq!(loaded.reports[1].href, None);
    assert_eq!(
        loaded.log,
        [
            "https://site.test/pages/a.css",
            "https://site.test/css/b.css"
        ]
    );
    // The UA sheet plus three author sheets; later sheets win ties.
    assert_eq!(loaded.cascade.sheets().len(), 4);
    assert_eq!(computed(&loaded, "p", PropertyId::Width), "2px");
    assert_eq!(computed(&loaded, "p", PropertyId::Height), "3px");
    assert_eq!(computed(&loaded, "p", PropertyId::Color), "rgb(255, 0, 0)");
}

#[test]
fn links_that_create_no_sheet_or_are_not_applied() {
    let loaded = load(
        "<!doctype html>\
         <link rel=stylesheet>\
         <link rel=stylesheet href=''>\
         <link rel=icon href=a.css>\
         <link rel='alternate stylesheet' title=Alt href=a.css>\
         <link rel=stylesheet disabled href=a.css>\
         <link rel=stylesheet type=text/plain href=a.css>\
         <link rel=stylesheet type=' Text/CSS ' href=a.css>\
         <link rel=stylesheet media=print href=a.css>\
         <link rel=stylesheet media='print, screen' href=missing.css>\
         <link rel=stylesheet href='https://[bad/'>\
         <template><link rel=stylesheet href=a.css></template>\
         <svg><link rel=stylesheet href=a.css /></svg>",
        &[("pages/a.css", "p { width: 1px }")],
        StyleLoadLimits::default(),
    );
    use SheetStatus as S;
    assert_eq!(
        statuses(&loaded),
        [
            S::Disabled,
            S::Disabled,
            S::NotCss,
            S::Applied,
            S::Applied,
            S::Fetch(FetchError::NotFound),
            S::InvalidUrl,
        ]
    );
    // a.css is fetched once (then cached), plus the missing sheet.
    assert_eq!(loaded.log.len(), 2);
}

#[test]
fn the_first_base_element_sets_the_base_url() {
    let loaded = load(
        "<!doctype html><base target=_top><base href='/themes/dark/'>\
         <base href=/ignored/><link rel=stylesheet href=main.css><p id=p>",
        &[("themes/dark/main.css", "#p { width: 7px }")],
        StyleLoadLimits::default(),
    );
    assert_eq!(statuses(&loaded), [SheetStatus::Applied]);
    assert_eq!(computed(&loaded, "p", PropertyId::Width), "7px");
}

#[test]
fn relative_links_without_a_base_url_are_invalid() {
    let page = document("<link rel=stylesheet href=a.css>");
    let mut fetcher = MapFetcher::default();
    let mut loader = StyleLoader::new(&mut fetcher, StyleLoadLimits::default());
    let (_, reports) =
        document_cascade_with(page.document(), &CssParser::default(), None, &mut loader).unwrap();
    assert_eq!(reports[0].status, SheetStatus::InvalidUrl);
    assert!(fetcher.log.is_empty());
}

#[test]
fn without_a_fetcher_external_sheets_are_blocked() {
    let page = document(
        "<link rel=stylesheet href=https://site.test/a.css>\
         <style>@import 'b.css'; p { width: 1px }</style>",
    );
    let (cascade, reports) = document_cascade(page.document(), &CssParser::default()).unwrap();
    assert_eq!(reports[0].status, SheetStatus::Fetch(FetchError::Blocked));
    assert_eq!(reports[1].status, SheetStatus::Applied);
    // The relative import cannot even be resolved without a base URL.
    assert_eq!(reports[1].imports[0].status, SheetStatus::InvalidUrl);
    assert_eq!(cascade.sheets().len(), 2);

    let mut fetcher = NoFetch;
    let mut loader = StyleLoader::new(&mut fetcher, StyleLoadLimits::default());
    let url = page_url();
    let (_, reports) = document_cascade_with(
        page.document(),
        &CssParser::default(),
        Some(&url),
        &mut loader,
    )
    .unwrap();
    assert_eq!(
        reports[1].imports[0].status,
        SheetStatus::Fetch(FetchError::Blocked)
    );
}

#[test]
fn imports_precede_their_sheet_and_resolve_against_it() {
    let loaded = load(
        "<!doctype html><link rel=stylesheet href=/css/main.css><p id=p>",
        &[
            (
                "css/main.css",
                "@charset \"utf-8\"; @layer base; @import url(parts/a.css);\n\
                 @import url('../shared/b.css') screen; #p { width: 9px }",
            ),
            (
                "css/parts/a.css",
                "@import 'c.css'; #p { width: 1px; height: 1px }",
            ),
            ("css/parts/c.css", "#p { height: 2px; color: red }"),
            ("shared/b.css", "#p { color: blue }"),
        ],
        StyleLoadLimits::default(),
    );
    assert_eq!(statuses(&loaded), [SheetStatus::Applied]);
    use SheetStatus as S;
    assert_eq!(
        import_statuses(&loaded.reports[0]),
        [
            ("parts/a.css".to_owned(), 1, S::Applied),
            ("c.css".to_owned(), 2, S::Applied),
            ("../shared/b.css".to_owned(), 1, S::Applied),
        ]
    );
    // Cascade order: c, a, b, main.
    assert_eq!(loaded.cascade.sheets().len(), 5);
    assert_eq!(computed(&loaded, "p", PropertyId::Width), "9px");
    assert_eq!(computed(&loaded, "p", PropertyId::Height), "1px");
    assert_eq!(computed(&loaded, "p", PropertyId::Color), "rgb(0, 0, 255)");
}

#[test]
fn import_conditions_and_placement() {
    let loaded = load(
        "<!doctype html><style>\
         @import 'a.css' print;\
         @import 'a.css' layer;\
         @import url(a.css) layer(theme) supports(display: grid);\
         @import 'a.css' all;\
         @import 'missing.css';\
         @import 'https://[bad/';\
         @import 42;\
         p { width: 1px }\
         @import 'late.css';\
         </style>",
        &[("pages/a.css", "p { height: 1px }")],
        StyleLoadLimits::default(),
    );
    use SheetStatus as S;
    assert_eq!(
        import_statuses(&loaded.reports[0])
            .into_iter()
            .map(|(_, _, s)| s)
            .collect::<Vec<_>>(),
        [
            S::Applied,
            S::UnsupportedCondition,
            S::UnsupportedCondition,
            S::Applied,
            S::Fetch(FetchError::NotFound),
            S::InvalidUrl,
        ]
    );
    let kinds: Vec<_> = loaded.reports[0]
        .diagnostics
        .iter()
        .map(|d| d.kind)
        .collect();
    assert_eq!(
        kinds,
        [
            StyleDiagnosticKind::InvalidImportRule,
            StyleDiagnosticKind::InvalidImportRule
        ]
    );
    assert_eq!(loaded.log.len(), 2);
}

#[test]
fn plain_parsing_still_reports_import_as_unsupported() {
    let parser = CssParser::default();
    let (_, diagnostics) =
        StyleSheet::parse(&parser, "@import 'a.css'; p { width: 1px }", Origin::Author).unwrap();
    assert_eq!(diagnostics[0].kind, StyleDiagnosticKind::UnsupportedAtRule);
    let (sheet, imports, diagnostics) =
        StyleSheet::parse_with_imports(&parser, "@import 'a.css' screen ;", Origin::Author)
            .unwrap();
    assert!(sheet.rules.is_empty() && diagnostics.is_empty());
    assert_eq!(imports[0].url, "a.css");
    assert_eq!(imports[0].media.as_ref().unwrap().to_string(), "screen");
    // `@import` must come before `@namespace`.
    let (_, imports, diagnostics) = StyleSheet::parse_with_imports(
        &parser,
        "@namespace svg 'x'; @import 'a.css';",
        Origin::Author,
    )
    .unwrap();
    assert!(imports.is_empty());
    assert_eq!(diagnostics[0].kind, StyleDiagnosticKind::InvalidImportRule);
}

#[test]
fn import_cycles_are_refused() {
    let loaded = load(
        "<!doctype html><link rel=stylesheet href=a.css>\
         <style>@import 'self.css';</style>",
        &[
            ("pages/a.css", "@import 'b.css'; p { width: 1px }"),
            ("pages/b.css", "@import 'a.css#again'; p { height: 1px }"),
            ("pages/self.css", "@import 'self.css';"),
        ],
        StyleLoadLimits::default(),
    );
    use SheetStatus as S;
    assert_eq!(
        import_statuses(&loaded.reports[0]),
        [
            ("b.css".to_owned(), 1, S::Applied),
            ("a.css#again".to_owned(), 2, S::Cycle),
        ]
    );
    assert_eq!(
        import_statuses(&loaded.reports[1]),
        [
            ("self.css".to_owned(), 1, S::Applied),
            ("self.css".to_owned(), 2, S::Cycle),
        ]
    );
}

#[test]
fn the_same_sheet_imported_twice_is_fetched_once() {
    let loaded = load(
        "<!doctype html><style>@import 'a.css'; @import 'a.css';</style>",
        &[("pages/a.css", "p { width: 1px }")],
        StyleLoadLimits::default(),
    );
    assert_eq!(loaded.log, ["https://site.test/pages/a.css"]);
    assert_eq!(loaded.cascade.sheets().len(), 4);
}

/// A chain `0.css` → `1.css` → ... → `n.css`.
fn chain(n: usize) -> Vec<(String, String)> {
    (0..=n)
        .map(|i| {
            let text = if i < n {
                format!("@import '{}.css'; p {{ width: {i}px }}", i + 1)
            } else {
                "p { height: 1px }".to_owned()
            };
            (format!("pages/{i}.css"), text)
        })
        .collect()
}

fn borrowed(sheets: &[(String, String)]) -> Vec<(&str, &str)> {
    sheets
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect()
}

#[test]
fn import_depth_is_bounded() {
    let sheets = chain(10);
    let limits = StyleLoadLimits {
        max_import_depth: 3,
        ..StyleLoadLimits::default()
    };
    let loaded = load(
        "<style>@import '0.css';</style>",
        &borrowed(&sheets),
        limits,
    );
    let depths: Vec<_> = import_statuses(&loaded.reports[0])
        .into_iter()
        .map(|(_, d, s)| (d, s))
        .collect();
    use SheetStatus as S;
    assert_eq!(
        depths,
        [
            (1, S::Applied),
            (2, S::Applied),
            (3, S::Applied),
            (4, S::DepthLimit)
        ]
    );
    assert_eq!(loaded.log.len(), 3);

    // A chain far deeper than any sensible limit loads without recursion.
    let sheets = chain(5000);
    let limits = StyleLoadLimits {
        max_import_depth: usize::MAX,
        max_sheets: usize::MAX,
        max_total_bytes: usize::MAX,
    };
    let loaded = load(
        "<style>@import '0.css';</style>",
        &borrowed(&sheets),
        limits,
    );
    assert_eq!(loaded.reports[0].imports.len(), 5001);
    assert_eq!(loaded.cascade.sheets().len(), 5003);
}

#[test]
fn the_sheet_budget_counts_links_imports_and_failures() {
    let limits = StyleLoadLimits {
        max_sheets: 3,
        ..StyleLoadLimits::default()
    };
    let loaded = load(
        "<link rel=stylesheet href=missing.css>\
         <style>@import 'a.css'; @import 'a.css'; @import 'a.css';</style>\
         <link rel=stylesheet href=a.css>",
        &[("pages/a.css", "p { width: 1px }")],
        limits,
    );
    use SheetStatus as S;
    assert_eq!(
        statuses(&loaded),
        [S::Fetch(FetchError::NotFound), S::Applied, S::LoadLimit]
    );
    assert_eq!(
        import_statuses(&loaded.reports[1])
            .into_iter()
            .map(|(_, _, s)| s)
            .collect::<Vec<_>>(),
        [S::Applied, S::Applied, S::LoadLimit]
    );
}

#[test]
fn the_byte_budget_is_exact() {
    let text = "p { width: 1px }"; // 16 bytes
    let run = |max_total_bytes| {
        let limits = StyleLoadLimits {
            max_total_bytes,
            ..StyleLoadLimits::default()
        };
        let loaded = load(
            "<link rel=stylesheet href=a.css><link rel=stylesheet href=a.css>",
            &[("pages/a.css", text)],
            limits,
        );
        statuses(&loaded)
    };
    use SheetStatus as S;
    assert_eq!(run(32), [S::Applied, S::Applied]);
    // The second inclusion comes from the cache but no longer fits.
    assert_eq!(run(31), [S::Applied, S::LoadLimit]);
    assert_eq!(run(16), [S::Applied, S::LoadLimit]);
    assert_eq!(
        run(15),
        [
            S::Fetch(FetchError::TooLarge),
            S::Fetch(FetchError::TooLarge)
        ]
    );
}

/// A fetcher that ignores the byte budget it is offered.
struct Oversized;

impl StylesheetFetcher for Oversized {
    fn fetch(&mut self, _url: &Url, max_bytes: usize) -> Result<String, FetchError> {
        Ok("x".repeat(max_bytes + 1))
    }
}

#[test]
fn a_fetcher_cannot_exceed_the_byte_budget() {
    let page = document("<link rel=stylesheet href=a.css>");
    let mut fetcher = Oversized;
    let limits = StyleLoadLimits {
        max_total_bytes: 100,
        ..StyleLoadLimits::default()
    };
    let mut loader = StyleLoader::new(&mut fetcher, limits);
    let url = page_url();
    let (_, reports) = document_cascade_with(
        page.document(),
        &CssParser::default(),
        Some(&url),
        &mut loader,
    )
    .unwrap();
    assert_eq!(reports[0].status, SheetStatus::Fetch(FetchError::TooLarge));
    assert_eq!(loader.bytes_used(), 0);
    assert_eq!(loader.sheets_used(), 1);
}

#[test]
fn imported_sheet_diagnostics_have_their_own_spans() {
    let loaded = load(
        "<style>@import 'a.css';</style>",
        &[("pages/a.css", "p { colour: red }")],
        StyleLoadLimits::default(),
    );
    let import = &loaded.reports[0].imports[0];
    assert_eq!(import.span.range(), 0..16);
    assert_eq!(import.diagnostics.len(), 1);
    assert_eq!(
        import.diagnostics[0].kind,
        StyleDiagnosticKind::UnknownProperty
    );
    assert_eq!(import.diagnostics[0].span.range(), 4..15);
}

#[test]
fn media_lists_restrict_the_whole_import_tree() {
    let loaded = load(
        "<!doctype html><style>@import 'print.css' print; @import 'screen.css' screen;</style>\
         <link rel=stylesheet media=print href=link.css><p id=p>",
        &[
            ("pages/print.css", "@import 'nested.css'; #p { width: 1px }"),
            ("pages/nested.css", "#p { height: 1px }"),
            ("pages/screen.css", "#p { color: blue }"),
            (
                "pages/link.css",
                "@import 'linked-nested.css'; #p { color: red }",
            ),
            ("pages/linked-nested.css", "#p { margin-top: 3px }"),
        ],
        StyleLoadLimits::default(),
    );
    assert_eq!(
        statuses(&loaded),
        [SheetStatus::Applied, SheetStatus::Applied]
    );
    assert_eq!(loaded.log.len(), 5);
    // The default environment is a screen: only screen.css applies.
    assert_eq!(computed(&loaded, "p", PropertyId::Width), "auto");
    assert_eq!(computed(&loaded, "p", PropertyId::Height), "auto");
    assert_eq!(computed(&loaded, "p", PropertyId::Color), "rgb(0, 0, 255)");
    assert_eq!(computed(&loaded, "p", PropertyId::MarginTop), "16px");
}

#[test]
fn link_and_base_urls_use_the_document_encoding_for_queries() {
    let page = document(
        "<base href='https://site.test/dir/?b=\u{E9}'>\
         <link rel=stylesheet href='a.css?q=\u{E9}\u{2603}'>\
         <style>@import 'i.css?q=\u{E9}';</style>",
    );
    let mut fetcher = MapFetcher::default();
    let mut loader = StyleLoader::new(&mut fetcher, StyleLoadLimits::default())
        .with_document_encoding(rustic_encoding::Encoding::Windows1252);
    let url = page_url();
    document_cascade_with(
        page.document(),
        &CssParser::default(),
        Some(&url),
        &mut loader,
    )
    .unwrap();
    assert_eq!(
        fetcher.log,
        [
            "https://site.test/dir/a.css?q=%E9%26%239731%3B",
            // @import resolves in UTF-8.
            "https://site.test/dir/i.css?q=%C3%A9",
        ]
    );
}

#[test]
fn url_values_resolve_against_the_declaring_sheets_base() {
    let loaded = load(
        "<!doctype html><base href='https://site.test/base/'>\
         <link rel=stylesheet href='/css/a.css'>\
         <style>#s { background-image: url(s.png) } #f { clip-path: url(#clip) }</style>\
         <p id=a>a</p><p id=i>i</p><p id=s>s</p><p id=f>f</p>\
         <p id=v>v</p><p id=w>w</p>\
         <p id=e style='background-image: url(e.png); filter: url(\"f.svg#x\") blur(1px)'>e</p>",
        &[
            (
                "css/a.css",
                "@import 'deep/i.css';\
                 #a { background-image: url(a.png), url('../img/b.png') }\
                 #v { --img: url(v.png) }\
                 #v, #w { background: var(--img, url(w.png)) }",
            ),
            ("css/deep/i.css", "#i { background-image: url(i.png) }"),
        ],
        StyleLoadLimits::default(),
    );
    let image = |id| computed(&loaded, id, PropertyId::BackgroundImage);
    // An external sheet resolves against its own URL, an imported sheet
    // against its URL, and a `<style>` element against the document base.
    assert_eq!(
        image("a"),
        "url(\"https://site.test/css/a.png\"), url(\"https://site.test/img/b.png\")"
    );
    assert_eq!(image("i"), "url(\"https://site.test/css/deep/i.png\")");
    assert_eq!(image("s"), "url(\"https://site.test/base/s.png\")");
    // After `var()` substitution, the declaring sheet's base still applies.
    assert_eq!(image("v"), "url(\"https://site.test/css/v.png\")");
    assert_eq!(image("w"), "url(\"https://site.test/css/w.png\")");
    // A `style` attribute uses the document base URL.
    assert_eq!(image("e"), "url(\"https://site.test/base/e.png\")");
    assert_eq!(
        computed(&loaded, "e", PropertyId::Filter),
        "url(\"https://site.test/base/f.svg#x\") blur(1px)"
    );
    // Fragment-only URLs refer to the document and stay as written.
    assert_eq!(
        computed(&loaded, "f", PropertyId::ClipPath),
        "url(\"#clip\")"
    );
}
