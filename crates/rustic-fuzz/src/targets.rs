//! Fuzz targets: each checks crash-freedom plus a differential invariant.
//!
//! A target never panics on purpose; a panic is a defect. Budgets stay at the
//! library defaults so hostile input must terminate inside them. Feed splits
//! are derived from the input's fingerprint, so a saved reproducer replays the
//! same check from its bytes alone.

use rustic_base::CancellationToken;
use rustic_css::{
    BlockItem, ColorScheme, ContainerFacts, CssParser, CustomValue, Declaration, MediaEnvironment,
    MediaType, NamespaceMap, Parsed, PointerAccuracy, Rule, SelectorList, parse_block_contents,
    parse_container_query_list, parse_declaration,
};
use rustic_html::{
    Diagnostic, FragmentContext, HtmlDocumentParser, HtmlError, HtmlFragmentParser, HtmlTokenizer,
    ParseError, Token,
};
use rustic_style::{
    Cascade, ComputeContext, ContainerSize, LayerSegment, MatchContext, Mutation, Origin,
    StyleSheet, StyledDocument, dump_computed_styles, query_selector_all, style_tree,
};
use rustic_url::{Url, UrlParser};

use crate::fingerprint;

/// A deterministic check over arbitrary bytes.
pub trait Target: Send + Sync {
    /// Stable name used on the command line and for corpus directories.
    fn name(&self) -> &'static str;
    /// Mutated inputs are truncated to this many bytes.
    fn max_len(&self) -> usize;
    /// Tokens inserted by the dictionary mutation.
    fn dictionary(&self) -> &'static [&'static [u8]];
    /// Return `Err` with an explanation when an invariant is violated.
    fn check(&self, input: &[u8]) -> Result<(), String>;
}

/// Every built-in target, in documentation order.
pub fn all() -> Vec<Box<dyn Target>> {
    vec![
        Box::new(HtmlTokenizerTarget),
        Box::new(HtmlDocumentTarget),
        Box::new(HtmlFragmentTarget),
        Box::new(CssTarget),
        Box::new(CssSelectorTarget),
        Box::new(CssCascadeTarget),
        Box::new(CssMediaTarget),
        Box::new(CssInvalidationTarget),
        Box::new(crate::formatting::FormattingTarget),
        Box::new(crate::paint::PaintTarget),
        Box::new(crate::png::PngTarget),
        Box::new(crate::images::JPEG),
        Box::new(crate::images::GIF),
        Box::new(crate::images::WEBP),
        Box::new(crate::sfnt::SfntTarget),
        Box::new(crate::shaping::ShapingTarget),
        Box::new(crate::js::JsParseTarget),
        Box::new(crate::js_heap::JsHeapTarget),
        Box::new(UrlTarget),
    ]
}

/// Look up a built-in target by name.
pub fn by_name(name: &str) -> Option<Box<dyn Target>> {
    all().into_iter().find(|target| target.name() == name)
}

const HTML_DICTIONARY: &[&[u8]] = &[
    b"<!DOCTYPE html>",
    b"<!doctype html public \"-//W3C//DTD HTML 4.01//EN\">",
    b"<html>",
    b"<head>",
    b"<body>",
    b"</body>",
    b"<p>",
    b"</p>",
    b"<a href=x>",
    b"</a>",
    b"<b>",
    b"</b>",
    b"<i>",
    b"<nobr>",
    b"<table>",
    b"<tr>",
    b"<td>",
    b"</table>",
    b"<caption>",
    b"<colgroup>",
    b"<select>",
    b"<option>",
    b"<optgroup>",
    b"<template>",
    b"</template>",
    b"<frameset>",
    b"<frame>",
    b"<svg>",
    b"<math>",
    b"<foreignObject>",
    b"<annotation-xml encoding=\"text/html\">",
    b"<mi>",
    b"<desc>",
    b"<![CDATA[",
    b"]]>",
    b"<script>",
    b"</script>",
    b"<!--",
    b"-->",
    b"<style>",
    b"<textarea>",
    b"<title>",
    b"<xmp>",
    b"<plaintext>",
    b"<noscript>",
    b"<iframe>",
    b"<button>",
    b"<form>",
    b"<li>",
    b"<dd>",
    b"<h1>",
    b"<pre>\n",
    b"<listing>",
    b"<image>",
    b"<isindex>",
    b"<menuitem>",
    b"&amp;",
    b"&notin",
    b"&#x1F600;",
    b"&#0;",
    b"&#x110000;",
    b"&",
    b"</",
    b"<?",
    b"<!",
    b"\0",
    b"\r\n",
    b" x=\"y\"",
    b" xlink:href=z",
    b"/>",
];

/// The pinned feed plan for one input: one split point and one chunk size.
fn plan(input: &[u8]) -> (usize, usize) {
    let hash = fingerprint(input);
    let split = (hash % (input.len() as u64 + 1)) as usize;
    let chunk = 1 + ((hash >> 32) % 17) as usize;
    (split, chunk)
}

/// Feed plans compared against the unbroken input.
fn feeds(input: &[u8]) -> Vec<Vec<&[u8]>> {
    let (split, chunk) = plan(input);
    let mut plans = vec![
        vec![&input[..split], &input[split..]],
        input.chunks(chunk).collect(),
    ];
    if input.len() <= 512 {
        plans.push(input.chunks(1).collect());
    }
    plans
}

fn describe(plan: &[&[u8]]) -> String {
    let sizes: Vec<usize> = plan.iter().map(|chunk| chunk.len()).collect();
    if sizes.len() > 8 {
        format!("{} chunks of up to {} bytes", sizes.len(), sizes[0])
    } else {
        format!("chunks {sizes:?}")
    }
}

/// Tokenization in the data state must not depend on feed boundaries.
pub struct HtmlTokenizerTarget;

type TokenRun = Result<(Vec<Token>, Vec<Diagnostic>), HtmlError>;

fn tokenize(chunks: &[&[u8]]) -> TokenRun {
    let mut tokenizer = HtmlTokenizer::default();
    let mut tokens = Vec::new();
    for chunk in chunks {
        tokens.extend(tokenizer.feed(chunk)?);
    }
    tokens.extend(tokenizer.finish()?);
    Ok((tokens, tokenizer.diagnostics().to_vec()))
}

impl Target for HtmlTokenizerTarget {
    fn name(&self) -> &'static str {
        "html-tokenizer"
    }

    fn max_len(&self) -> usize {
        4096
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        HTML_DICTIONARY
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let whole = tokenize(&[input]);
        for plan in feeds(input) {
            if tokenize(&plan) != whole {
                return Err(format!(
                    "token stream differs from unbroken input with {}",
                    describe(&plan)
                ));
            }
        }
        Ok(())
    }
}

/// Document construction must terminate in budget and ignore feed boundaries.
pub struct HtmlDocumentTarget;

/// Dump cap well above any tree that default parser budgets can produce.
const MAX_DUMP_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Eq, PartialEq)]
enum DocumentRun {
    Parsed(String),
    Rejected(ParseError),
}

fn build(chunks: &[&[u8]]) -> Result<DocumentRun, String> {
    let mut parser = HtmlDocumentParser::default();
    for chunk in chunks {
        if let Err(error) = parser.feed(chunk) {
            return Ok(DocumentRun::Rejected(error));
        }
    }
    let parsed = match parser.finish() {
        Ok(parsed) => parsed,
        Err(error) => return Ok(DocumentRun::Rejected(error)),
    };
    let document = parsed.document();
    document
        .check_invariants()
        .map_err(|error| format!("completed document breaks a DOM invariant: {error}"))?;
    document
        .dump_tree(document.root(), MAX_DUMP_BYTES)
        .map(DocumentRun::Parsed)
        .map_err(|error| format!("completed document could not be dumped: {error}"))
}

impl Target for HtmlDocumentTarget {
    fn name(&self) -> &'static str {
        "html-document"
    }

    fn max_len(&self) -> usize {
        4096
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        HTML_DICTIONARY
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let whole = build(&[input])?;
        if whole == DocumentRun::Rejected(ParseError::Poisoned)
            || whole == DocumentRun::Rejected(ParseError::AlreadyFinished)
        {
            return Err(format!(
                "fresh parser reported a lifecycle error: {whole:?}"
            ));
        }
        for plan in feeds(input) {
            let split = build(&plan)?;
            if split != whole {
                return Err(format!(
                    "document differs from unbroken input with {}: {} vs {}",
                    describe(&plan),
                    summary(&split),
                    summary(&whole)
                ));
            }
        }
        // A cancelled parser must stop cleanly rather than return a tree.
        let cancellation = CancellationToken::default();
        let mut parser = HtmlDocumentParser::new(Default::default(), cancellation.clone())
            .map_err(|error| format!("default limits rejected: {error}"))?;
        cancellation.cancel();
        if parser.feed(input).is_ok() && parser.finish().is_ok() {
            return Err("cancelled parser completed a document".into());
        }
        Ok(())
    }
}

/// Fragment parsing must also terminate in budget and ignore feed boundaries.
///
/// The context element comes from the input's fingerprint, covering tokenizer
/// states, table/template/select modes and foreign integration points.
pub struct HtmlFragmentTarget;

fn fragment_context(input: &[u8]) -> FragmentContext {
    const HTML: &[&str] = &[
        "body",
        "div",
        "html",
        "head",
        "table",
        "tbody",
        "tr",
        "td",
        "caption",
        "colgroup",
        "select",
        "template",
        "form",
        "frameset",
        "title",
        "textarea",
        "style",
        "script",
        "plaintext",
        "noscript",
        "iframe",
        "xmp",
    ];
    let pick = (fingerprint(input) >> 8) as usize % (HTML.len() + 6);
    match pick.checked_sub(HTML.len()) {
        None => FragmentContext::html(HTML[pick]),
        Some(0) => FragmentContext::svg("svg"),
        Some(1) => FragmentContext::svg("foreignObject"),
        Some(2) => FragmentContext::svg("desc"),
        Some(3) => FragmentContext::mathml("mi"),
        Some(4) => {
            FragmentContext::mathml("annotation-xml").with_attribute("encoding", "text/html")
        }
        Some(_) => FragmentContext::mathml("math"),
    }
}

fn build_fragment(context: &FragmentContext, chunks: &[&[u8]]) -> Result<DocumentRun, String> {
    let mut parser = HtmlFragmentParser::new(context, Default::default(), Default::default())
        .map_err(|error| format!("default limits rejected: {error}"))?;
    for chunk in chunks {
        if let Err(error) = parser.feed(chunk) {
            return Ok(DocumentRun::Rejected(error));
        }
    }
    let parsed = match parser.finish() {
        Ok(parsed) => parsed,
        Err(error) => return Ok(DocumentRun::Rejected(error)),
    };
    parsed
        .document()
        .check_invariants()
        .map_err(|error| format!("completed fragment breaks a DOM invariant: {error}"))?;
    parsed
        .document()
        .dump_tree(parsed.root(), MAX_DUMP_BYTES)
        .map(DocumentRun::Parsed)
        .map_err(|error| format!("completed fragment could not be dumped: {error}"))
}

impl Target for HtmlFragmentTarget {
    fn name(&self) -> &'static str {
        "html-fragment"
    }

    fn max_len(&self) -> usize {
        4096
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        HTML_DICTIONARY
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let context = fragment_context(input);
        let whole = build_fragment(&context, &[input])?;
        for plan in feeds(input) {
            let split = build_fragment(&context, &plan)?;
            if split != whole {
                return Err(format!(
                    "fragment for {} {:?} differs from unbroken input with {}: {} vs {}",
                    context.local_name(),
                    context.namespace(),
                    describe(&plan),
                    summary(&split),
                    summary(&whole)
                ));
            }
        }
        Ok(())
    }
}

fn summary(run: &DocumentRun) -> String {
    match run {
        DocumentRun::Parsed(dump) => format!("tree of {} dump bytes", dump.len()),
        DocumentRun::Rejected(error) => format!("error `{error}`"),
    }
}

/// CSS parsing must be total, with ordered spans that cover all non-comment input.
pub struct CssTarget;

impl Target for CssTarget {
    fn name(&self) -> &'static str {
        "css"
    }

    fn max_len(&self) -> usize {
        4096
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"/*",
            b"*/",
            b"{",
            b"}",
            b"(",
            b")",
            b"[",
            b"]",
            b";",
            b":",
            b"!important",
            b"@media ",
            b"@import ",
            b"url(",
            b"url( 'x' )",
            b"\\",
            b"\\41 ",
            b"\"",
            b"'",
            b"#",
            b"-->",
            b"<!--",
            b"--x:",
            b"1e3",
            b"+.5",
            b"-",
            b"%",
            b"px",
            b"calc(",
            b"min(",
            b"clamp(",
            b" + ",
            b" - ",
            b" * ",
            b" / ",
            b"infinity",
            b"\r\n",
            b"\x0c",
            b"\0",
            b"&",
            b"a:hover",
            b"rgb(",
            b"U+26",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let text = String::from_utf8_lossy(input);
        let parser = CssParser::default();
        let tokens = parser
            .tokenize(&text)
            .map_err(|e| format!("default limits: {e}"))?;
        let mut pos = 0;
        for token in &tokens.value {
            let range = token.span.range();
            if range.start < pos || range.is_empty() || token.span.slice_str(&text).is_none() {
                return Err(format!("bad token span {range:?} after {pos}"));
            }
            let gap = &text[pos..range.start];
            if !gap.is_empty() && !gap.starts_with("/*") {
                return Err(format!("uncovered input {gap:?} before {range:?}"));
            }
            pos = range.end;
        }
        let tail = &text[pos..];
        if !tail.is_empty() && !tail.starts_with("/*") {
            return Err(format!("uncovered trailing input {tail:?}"));
        }
        let sheet = match parser.stylesheet(&text) {
            Ok(sheet) => sheet,
            // Depth and token limits are legitimate outcomes for hostile input.
            Err(_) => return Ok(()),
        };
        let mut blocks: Vec<Vec<rustic_css::Value>> = sheet
            .value
            .rules
            .iter()
            .filter_map(|rule| match rule {
                Rule::Qualified(q) => Some(q.block.clone()),
                Rule::At(a) => a.block.clone(),
            })
            .collect();
        // Read nested blocks iteratively, as a style engine would.
        while let Some(block) = blocks.pop() {
            for item in parse_block_contents(&block, 64).value {
                if let BlockItem::Declaration(declaration) = &item {
                    check_typed_declaration(&parser, declaration)?;
                }
                if let BlockItem::Rule(Rule::Qualified(q)) = item {
                    blocks.push(q.block);
                } else if let BlockItem::Rule(Rule::At(a)) = item {
                    blocks.extend(a.block);
                }
            }
        }
        if parser.stylesheet(&text).ok() != Some(sheet) {
            return Err("stylesheet parsing is not deterministic".into());
        }
        Ok(())
    }
}

/// Typed values must serialize to a fixed point: reparsing a longhand's
/// serialization serializes identically.
fn check_typed_declaration(parser: &CssParser, declaration: &Declaration) -> Result<(), String> {
    let Ok(longhands) = parse_declaration(declaration) else {
        return Ok(());
    };
    for longhand in longhands {
        let once = longhand.to_string();
        let again = parser
            .declaration(&once)
            .ok()
            .and_then(|parsed| parsed.value)
            .and_then(|d| parse_declaration(&d).ok());
        match again.as_deref() {
            Some([reparsed]) if reparsed.to_string() == once => {}
            other => return Err(format!("typed value {once:?} reparsed as {other:?}")),
        }
    }
    Ok(())
}

/// URL parsing must be total and serialization must be a fixed point.
pub struct UrlTarget;

const URL_BASES: &[&str] = &[
    "https://user:pass@example.com:8080/a/b/c?q=1#frag",
    "http://[::1]/dir/",
    "non-spec:/a/b",
    "sc://host/path",
    "data:text/plain,opaque",
    "file:///C:/dir/file?q",
    "file://server/share/doc",
];

impl Target for UrlTarget {
    fn name(&self) -> &'static str {
        "url"
    }

    fn max_len(&self) -> usize {
        1024
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"http://",
            b"https:",
            b"ws:",
            b"file:",
            b"blob:",
            b"sc:",
            b"//",
            b"\\",
            b"@",
            b":",
            b"[",
            b"]",
            b"::",
            b"%2e",
            b"%2E%2e",
            b"%00",
            b"%",
            b"..",
            b"./",
            b"?",
            b"#",
            b"0x",
            b"1.2.3.4",
            b"255.255.255.255.",
            b"xn--",
            b"\t",
            b"\n",
            b" ",
            b"\xc3\x9f",
            b"\xef\xbc\x8e",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let text = String::from_utf8_lossy(input);
        let bases: Vec<Url> = URL_BASES
            .iter()
            .map(|base| Url::parse(base, None).expect("pinned base parses"))
            .collect();
        let parser = UrlParser::default();
        let mut results = Vec::new();
        for base in std::iter::once(None).chain(bases.iter().map(Some)) {
            let report = parser.parse_with_diagnostics(&text, base);
            let plain = Url::parse(&text, base);
            if report.result() != plain.as_ref().map_err(|error| *error) {
                return Err("diagnostic parsing changed the result".into());
            }
            for diagnostic in report.diagnostics() {
                if !text.is_char_boundary(diagnostic.offset()) {
                    return Err(format!(
                        "{} offset {} is not a char boundary of the input",
                        diagnostic.kind(),
                        diagnostic.offset()
                    ));
                }
            }
            results.push(plain);
        }
        for url in results.into_iter().flatten() {
            let serialized = url.serialize();
            let reparsed = Url::parse(&serialized, None).map_err(|error| {
                format!("serialization `{serialized}` failed to reparse: {error}")
            })?;
            if reparsed.serialize() != serialized {
                return Err(format!(
                    "serialization is not a fixed point: `{serialized}` became `{}`",
                    reparsed.serialize()
                ));
            }
            if reparsed != url {
                return Err(format!("reparsed record differs for `{serialized}`"));
            }
            let _ = url.origin().serialize();
            let _ = format!("{url:?}");
        }
        Ok(())
    }
}

/// Selector parsing, serialization and matching (`rustic-css` selectors and
/// `rustic-style`).
///
/// Invariants: a valid selector list serializes to a fixed point with the
/// same specificities, and the reparsed list selects exactly the same
/// elements of a fixed document, in HTML, quirks and XML matching modes.
pub struct CssSelectorTarget;

const SELECTOR_DOCUMENT: &[u8] = b"<!doctype html><ul id=list class='a B'>\
<li id=one class=x lang=en-US>1<li class='x y'><!--c--><li title='Hello World'>\
<li><a href=/x id=link>l</a><li></ul><p id=Odd data-v=Abc><b></b><i></i><b></b></p>\
<svg viewBox='0 0 1 1'><foreignObject><p></p></foreignObject><rect/></svg>\
<table><tr><td>cell</td></tr></table><input type=TEXT>\
<form id=f><input type=checkbox checked><input type=radio name=g checked>\
<input type=radio name=g form=f><button></button><select><option disabled>a<option>b</select>\
<textarea placeholder=t></textarea><fieldset disabled><legend><input></legend><input readonly>\
</fieldset></form><div dir=auto lang=he>\xd7\xa9<bdi>x</bdi></div><p contenteditable>e</p>\
<progress></progress>";

fn select_all(
    document: &rustic_dom::Document,
    list: &SelectorList,
) -> Vec<Vec<rustic_base::NodeId>> {
    let mut quirks = MatchContext::html();
    quirks.quirks = true;
    [MatchContext::html(), quirks, MatchContext::xml()]
        .iter()
        .map(|cx| query_selector_all(document, document.root(), list, cx).unwrap_or_default())
        .collect()
}

impl Target for CssSelectorTarget {
    fn name(&self) -> &'static str {
        "css-selector"
    }

    fn max_len(&self) -> usize {
        1024
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"*",
            b"|",
            b"*|",
            b"svg|",
            b"#",
            b".",
            b"[",
            b"]",
            b"=",
            b"~=",
            b"|=",
            b"^=",
            b"$=",
            b"*=",
            b" i]",
            b" s]",
            b"'",
            b"\\",
            b"\\31 ",
            b" ",
            b">",
            b"+",
            b"~",
            b",",
            b":",
            b"::",
            b"(",
            b")",
            b":is(",
            b":where(",
            b":not(",
            b":nth-child(",
            b":nth-last-of-type(",
            b" of ",
            b"2n+1",
            b"-n+3",
            b"odd",
            b"even",
            b":root",
            b":empty",
            b":scope",
            b":first-child",
            b":only-of-type",
            b":any-link",
            b":hover",
            b"::before",
            b":first-line",
            b"li",
            b"p",
            b"rect",
            b"foreignObject",
            b"&",
            b"||",
            b"/**/",
            b":has(",
            b":has(> ",
            b":has(+ ",
            b":has(~ ",
            b":lang(",
            b"\"*-US\"",
            b"en",
            b":dir(",
            b"ltr",
            b"rtl",
            b":checked",
            b":disabled",
            b":enabled",
            b":indeterminate",
            b":default",
            b":required",
            b":optional",
            b":read-only",
            b":read-write",
            b":placeholder-shown",
            b"::placeholder",
            b"::selection",
            b"::backdrop",
            b"::after:hover",
            b"input",
            b"option",
            b"fieldset",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let text = String::from_utf8_lossy(input);
        let namespaces = NamespaceMap::new().with_prefix("svg", "http://www.w3.org/2000/svg");
        let parser = CssParser::default();
        let Ok(parsed) = parser.selector_list(&text, &namespaces) else {
            return Ok(()); // A resource limit is a legitimate outcome.
        };
        let Ok(list) = parsed.value else {
            return Ok(());
        };
        let serialized = list.to_string();
        let reparsed = match parser.selector_list(&serialized, &namespaces) {
            Ok(Parsed {
                value: Ok(list), ..
            }) => list,
            other => {
                return Err(format!(
                    "serialization {serialized:?} does not reparse: {other:?}"
                ));
            }
        };
        if reparsed.to_string() != serialized {
            return Err(format!(
                "serialization {serialized:?} is not a fixed point: {reparsed}"
            ));
        }
        let specificities = |l: &SelectorList| {
            l.selectors()
                .iter()
                .map(|s| s.specificity())
                .collect::<Vec<_>>()
        };
        if specificities(&list) != specificities(&reparsed) {
            return Err(format!("specificity changed through {serialized:?}"));
        }
        let mut document = HtmlDocumentParser::default();
        document
            .feed(SELECTOR_DOCUMENT)
            .map_err(|e| format!("fixture: {e}"))?;
        let document = document.finish().map_err(|e| format!("fixture: {e}"))?;
        let document = document.document();
        if select_all(document, &list) != select_all(document, &reparsed) {
            return Err(format!("matching changed through {serialized:?}"));
        }
        Ok(())
    }
}

/// The cascade over an arbitrary author stylesheet (`rustic-style`).
///
/// Invariants: styling a fixed document is total; splitting the sheet's
/// rules across two author sheets, or appending the sheet to itself, gives
/// the same specified styles; and so does adding a user-agent copy of the
/// sheet, because author declarations win unless a UA one is `!important`
/// (checked by comparing against a cascade where the copy is a no-op).
pub struct CssCascadeTarget;

const CASCADE_DOCUMENT: &[u8] =
    b"<!doctype html><div id=a class='x y' style='color: red; width: 1px'>\
<p id=b class=x>one<span style='font-size: 2em !important'>two</span></p><p lang=en>\
<b></b><i style='margin: inherit; color: revert'></i></p></div><ul><li><li class=y></ul>";

fn cascade_styles(cascade: &Cascade) -> Vec<String> {
    let mut document = HtmlDocumentParser::default();
    if document.feed(CASCADE_DOCUMENT).is_err() {
        return Vec::new();
    }
    let Ok(document) = document.finish() else {
        return Vec::new();
    };
    let document = document.document();
    style_tree(
        document,
        document.root(),
        cascade,
        &CssParser::default(),
        &MatchContext::html(),
        &ComputeContext::default(),
    )
    .map(|styles| {
        styles
            .into_iter()
            .map(|s| s.computed.dump_non_initial())
            .collect()
    })
    .unwrap_or_default()
}

/// Media query lists: parsing is total, serialization reaches a fixed point
/// after one round, and the reparsed list matches exactly the same
/// environments (a desktop, a phone, print and a zero-sized viewport).
pub struct CssMediaTarget;

fn media_environments() -> Vec<MediaEnvironment> {
    let desktop = MediaEnvironment::default();
    let mut phone = MediaEnvironment::screen(390.0, 844.0);
    phone.resolution = 3.0;
    phone.pointer = PointerAccuracy::Coarse;
    phone.any_pointer = PointerAccuracy::Coarse;
    phone.hover = false;
    phone.any_hover = false;
    phone.color_scheme = ColorScheme::Dark;
    phone.reduced_motion = true;
    let mut print = MediaEnvironment::screen(816.0, 1056.0);
    print.media_type = MediaType::Print;
    print.color = 0;
    print.monochrome = 1;
    let empty = MediaEnvironment::screen(0.0, 0.0);
    vec![desktop, phone, print, empty]
}

impl Target for CssMediaTarget {
    fn name(&self) -> &'static str {
        "css-media"
    }

    fn max_len(&self) -> usize {
        1024
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"(",
            b")",
            b",",
            b":",
            b" and ",
            b" or ",
            b"not ",
            b"only ",
            b"screen",
            b"print",
            b"all",
            b"width",
            b"min-width",
            b"max-height",
            b"aspect-ratio",
            b"resolution",
            b"hover",
            b"orientation",
            b"prefers-color-scheme",
            b"grid",
            b"color",
            b"<",
            b"<=",
            b">=",
            b"=",
            b"600px",
            b"40em",
            b"16/9",
            b"2dppx",
            b"0",
            b"dark",
            b"foo(",
            b"style(",
            b"--x",
            b"inline-size",
            b"block-size",
            b"\\",
            b"/*",
            b"*/",
            b"\"",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let text = String::from_utf8_lossy(input);
        let parser = CssParser::default();
        let Ok(parsed) = parser.media_query_list(&text) else {
            return Ok(()); // A resource limit is a legitimate outcome.
        };
        let list = parsed.value;
        let first = list.to_string();
        let Ok(reparsed) = parser.media_query_list(&first) else {
            return Err(format!("serialization {first:?} exceeded parser limits"));
        };
        let reparsed = reparsed.value;
        let second = reparsed.to_string();
        if second != first {
            return Err(format!(
                "serialization not a fixed point: {first:?} -> {second:?}"
            ));
        }
        for environment in media_environments() {
            if list.matches(&environment) != reparsed.matches(&environment) {
                return Err(format!(
                    "{first:?} matches {environment:?} differently after reparsing"
                ));
            }
        }
        check_container_prelude(&parser, &text)
    }
}

/// The same input as an `@container` prelude: serialization is a fixed
/// point and the reparsed list evaluates the same against sample sizes.
fn check_container_prelude(parser: &CssParser, text: &str) -> Result<(), String> {
    let Ok(values) = parser.component_values(text) else {
        return Ok(());
    };
    let Some(list) = parse_container_query_list(&values.value) else {
        return Ok(());
    };
    let first = list.to_string();
    let reparsed = parser
        .component_values(&first)
        .ok()
        .and_then(|v| parse_container_query_list(&v.value));
    let Some(reparsed) = reparsed else {
        return Err(format!("container prelude {first:?} did not reparse"));
    };
    let second = reparsed.to_string();
    if second != first {
        return Err(format!(
            "container serialization not a fixed point: {first:?} -> {second:?}"
        ));
    }
    let style = |name: &str, value: Option<&CustomValue>| match value {
        None => Some(name.len() % 2 == 0),
        Some(v) => Some(v.to_string().len() % 2 == 0),
    };
    for (width, height) in [
        (Some(0.0), Some(0.0)),
        (Some(400.0), Some(300.0)),
        (Some(50.0), None),
        (None, None),
    ] {
        let facts = ContainerFacts {
            width,
            height,
            font_size_px: 16.0,
            root_font_size_px: 16.0,
            font_ratios: rustic_css::FontUnitRatios::FALLBACK,
            root_font_ratios: rustic_css::FontUnitRatios::FALLBACK,
            viewport_width: 1280.0,
            viewport_height: 720.0,
        };
        for (a, b) in list.conditions.iter().zip(&reparsed.conditions) {
            let eval = |c: &rustic_css::ContainerCondition| {
                c.query
                    .as_ref()
                    .map(|q| q.evaluate_container(&facts, &style))
            };
            if a.name != b.name || eval(a) != eval(b) || a.axes() != b.axes() {
                return Err(format!(
                    "container prelude {first:?} changed meaning after reparsing"
                ));
            }
        }
    }
    Ok(())
}

/// Incremental restyling: a fixed document styled with an arbitrary author
/// sheet, then mutated by a script of attribute, child-list and text
/// changes, must match a full restyle at every checkpoint.
///
/// Input up to the first NUL byte is the sheet and the rest the script; an
/// input without NUL uses its own bytes as both.
pub struct CssInvalidationTarget;

const INVALIDATION_DOCUMENT: &[u8] = b"<!doctype html><html lang=en><body>\
<div id=a class='x y'><p id=b class=x>one<span>two</span></p><p dir=auto>\
<b></b><i style='margin: inherit'></i></p></div><ul><li><li class=y><li></ul>\
<form><input type=checkbox><input type=radio name=g checked><input type=radio name=g>\
<button>go</button><fieldset disabled><legend><input></legend><input></fieldset></form>\
<a href=x>link</a><p></p><dialog></dialog>";

const MUTATION_ATTRIBUTES: &[&str] = &[
    "class",
    "id",
    "style",
    "disabled",
    "checked",
    "lang",
    "dir",
    "hidden",
    "data-x",
    "type",
    "value",
    "href",
    "open",
    "name",
    "selected",
    "placeholder",
];
const MUTATION_VALUES: &[&str] = &[
    "",
    "x",
    "y",
    "x y",
    "a",
    "rtl",
    "auto",
    "de",
    "color: red",
    "font-size: 2em",
    "radio",
    "checkbox",
    "width: 1rem",
    "\u{5d0}",
    "--v: 3px",
    "--v: var(--w); --w: var(--v)",
    "width: var(--v, 2px); --w: calc(var(--v) * 2)",
    "container-type: size",
    "container: c / inline-size; --t: 1",
];
const MUTATION_TAGS: &[&str] = &["div", "p", "li", "span", "input", "b", "ul", "html"];

fn invalidation_elements(document: &rustic_dom::Document) -> Vec<rustic_dom::NodeId> {
    document
        .walk(document.root())
        .map(|walk| {
            walk.filter(|&(id, _)| {
                matches!(
                    document.node(id).map(|n| n.kind()),
                    Ok(rustic_dom::NodeKind::Element(_))
                )
            })
            .map(|(id, _)| id)
            .collect()
        })
        .unwrap_or_default()
}

fn invalidation_texts(document: &rustic_dom::Document) -> Vec<rustic_dom::NodeId> {
    document
        .walk(document.root())
        .map(|walk| {
            walk.filter(|&(id, _)| {
                matches!(
                    document.node(id).map(|n| n.kind()),
                    Ok(rustic_dom::NodeKind::Text(_))
                )
            })
            .map(|(id, _)| id)
            .collect()
        })
        .unwrap_or_default()
}

/// Apply one scripted mutation and return what to report (nothing when the
/// DOM refused it); `None` when the script ran out.
fn apply_mutation(
    document: &mut rustic_dom::Document,
    script: &mut impl Iterator<Item = u8>,
    sizes: &mut Vec<(rustic_dom::NodeId, Option<ContainerSize>)>,
) -> Option<Vec<Mutation>> {
    use rustic_dom::QualifiedName;
    let op = script.next()?;
    let pick = |script: &mut dyn Iterator<Item = u8>, len: usize| {
        script.next().map(|b| usize::from(b) % len.max(1))
    };
    let elements = invalidation_elements(document);
    let mutation = match op % 7 {
        6 => {
            let element = *elements.get(pick(script, elements.len())?)?;
            let b = script.next()?;
            let size = (b % 5 != 0).then(|| ContainerSize {
                width: f64::from(b) * 8.0,
                height: f64::from(b >> 4) * 50.0,
            });
            sizes.push((element, size));
            None
        }
        0 | 1 => {
            let element = *elements.get(pick(script, elements.len())?)?;
            let local = MUTATION_ATTRIBUTES[pick(script, MUTATION_ATTRIBUTES.len())?];
            let name = QualifiedName::from_parsed(None, None, local).ok()?;
            let done = if op % 7 == 0 {
                let value = MUTATION_VALUES[pick(script, MUTATION_VALUES.len())?];
                document.set_attribute(element, name, value).is_ok()
            } else {
                document.remove_attribute(element, None, local).is_ok()
            };
            done.then(|| Mutation::Attribute {
                element,
                local: local.into(),
            })
        }
        2 => {
            let parent = *elements.get(pick(script, elements.len())?)?;
            let tag = MUTATION_TAGS[pick(script, MUTATION_TAGS.len())?];
            let child = document
                .create_element(QualifiedName::html(tag).ok()?)
                .ok()?;
            document
                .append_child(parent, child)
                .is_ok()
                .then_some(Mutation::Children { parent })
        }
        3 => {
            // Skip the root element so the document keeps one.
            let element = *elements.get(1 + pick(script, elements.len().saturating_sub(1))?)?;
            let parent = document.node(element).ok()?.parent()?;
            document
                .detach(element)
                .is_ok()
                .then_some(Mutation::Children { parent })
        }
        4 => {
            let element = *elements.get(1 + pick(script, elements.len().saturating_sub(1))?)?;
            let target = *elements.get(pick(script, elements.len())?)?;
            let old_parent = document.node(element).ok()?.parent()?;
            if document.append_child(target, element).is_err() {
                return Some(Vec::new());
            }
            return Some(vec![
                Mutation::Children { parent: old_parent },
                Mutation::Children { parent: target },
            ]);
        }
        _ => {
            let texts = invalidation_texts(document);
            let text = *texts.get(pick(script, texts.len())?)?;
            let value = if script.next()? % 2 == 0 {
                ""
            } else {
                "x\u{5d0}"
            };
            document
                .set_character_data(text, value)
                .is_ok()
                .then_some(Mutation::Text { node: text })
        }
    };
    Some(mutation.into_iter().collect())
}

impl Target for CssInvalidationTarget {
    fn name(&self) -> &'static str {
        "css-invalidation"
    }

    fn max_len(&self) -> usize {
        1024
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"\0",
            b"{",
            b"}",
            b".x",
            b"#a",
            b"[hidden]",
            b"[data-x]",
            b" > ",
            b" + ",
            b" ~ ",
            b":nth-child(odd)",
            b":empty",
            b":has(.x)",
            b":has(> .x)",
            b":has(+ .x)",
            b":has(~ .x)",
            b":has(:checked)",
            b":checked",
            b":default",
            b":indeterminate",
            b":disabled",
            b":placeholder-shown",
            b":read-write",
            b":lang(de)",
            b":dir(rtl)",
            b":first-child",
            b"color: red",
            b"font-size: 2em",
            b"width: inherit",
            b"@container (width > 100px) {",
            b"@container c style(--t: 1) {",
            b"container-type: size",
            b"\x00\x00\x00\x00",
            b"\x02\x03\x04",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let (sheet_bytes, script_bytes) = match input.iter().position(|&b| b == 0) {
            Some(nul) => (&input[..nul], &input[nul + 1..]),
            None => (input, input),
        };
        let text = String::from_utf8_lossy(sheet_bytes);
        let parser = CssParser::default();
        let Ok((sheet, _)) = StyleSheet::parse(&parser, &text, Origin::Author) else {
            return Ok(());
        };
        let mut html = HtmlDocumentParser::default();
        if html.feed(INVALIDATION_DOCUMENT).is_err() {
            return Ok(());
        }
        let Ok(parsed) = html.finish() else {
            return Ok(());
        };
        let mut document = parsed.into_document();
        let Ok((mut cascade, _)) = rustic_style::document_cascade(&document, &parser) else {
            return Ok(());
        };
        cascade.push(sheet);
        let cx = MatchContext::html();
        let compute = ComputeContext::default();
        let Ok(mut styled) = StyledDocument::new(&document, &cascade, &parser, &cx, &compute)
        else {
            return Ok(());
        };
        let mut script = script_bytes.iter().copied().take(96);
        let mut step = 0usize;
        let check = |styled: &mut StyledDocument,
                     document: &rustic_dom::Document,
                     step: usize|
         -> Result<(), String> {
            let cx = MatchContext::html();
            styled
                .restyle(document, &cascade, &parser, &cx, &compute)
                .map_err(|e| format!("restyle failed: {e}"))?;
            let full = rustic_style::style_tree_with_sizes(
                document,
                document.root(),
                &cascade,
                &parser,
                &cx,
                &compute,
                styled.container_sizes(),
            )
            .map_err(|e| format!("full restyle failed: {e}"))?;
            if full.len() != styled.styles().len() {
                return Err(format!(
                    "step {step}: {} incremental styles, {} full",
                    styled.styles().len(),
                    full.len()
                ));
            }
            for (a, b) in styled.styles().iter().zip(&full) {
                if a != b {
                    return Err(format!(
                        "step {step}: element {:?} differs from a full restyle:\n{}---\n{}",
                        a.node,
                        a.computed.dump_non_initial(),
                        b.computed.dump_non_initial()
                    ));
                }
            }
            Ok(())
        };
        let mut sizes = Vec::new();
        while let Some(mutations) = apply_mutation(&mut document, &mut script, &mut sizes) {
            step += 1;
            for mutation in mutations {
                styled.note(&document, mutation);
            }
            for (element, size) in sizes.drain(..) {
                styled.set_container_size(element, size);
            }
            if step % 3 == 0 {
                check(&mut styled, &document, step)?;
            }
        }
        check(&mut styled, &document, step)
    }
}

/// Computed values stay finite and the computed-style dump always succeeds.
fn check_computed(cascade: &Cascade) -> Result<(), String> {
    let mut document = HtmlDocumentParser::default();
    if document.feed(CASCADE_DOCUMENT).is_err() {
        return Ok(());
    }
    let Ok(document) = document.finish() else {
        return Ok(());
    };
    let document = document.document();
    let Ok(styles) = style_tree(
        document,
        document.root(),
        cascade,
        &CssParser::default(),
        &MatchContext::html(),
        &ComputeContext::default(),
    ) else {
        return Ok(());
    };
    for style in &styles {
        let size = style.computed.font_size_px();
        if !size.is_finite() || size < 0.0 {
            return Err(format!("computed font size {size}"));
        }
        let dump = style.computed.dump_non_initial();
        if dump.contains("NaN") || dump.contains("inf") {
            return Err(format!("non-finite computed value in {dump:?}"));
        }
    }
    if dump_computed_styles(document, &styles, usize::MAX).is_none() {
        return Err("unbounded computed-style dump failed".into());
    }
    Ok(())
}

impl Target for CssCascadeTarget {
    fn name(&self) -> &'static str {
        "css-cascade"
    }

    fn max_len(&self) -> usize {
        2048
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"{",
            b"}",
            b";",
            b":",
            b"!important",
            b"p",
            b"#a",
            b".x",
            b"*",
            b" > ",
            b",",
            b"color:",
            b"width:",
            b"margin:",
            b"font-size:",
            b"display:",
            b"border:",
            b"inherit",
            b"initial",
            b"unset",
            b"revert",
            b"revert-layer",
            b"red",
            b"1px",
            b"2em",
            b"50%",
            b"auto",
            b"block",
            b"@namespace x url(y);",
            b"@import 'a.css' screen;",
            b"@import url(b) layer supports(x);",
            b"@charset \"utf-8\";",
            b"@layer a;",
            b"@media x {",
            b"@layer a, b;",
            b"@layer a {",
            b"@layer {",
            b"@layer b.c {",
            b"revert-layer !important",
            b"currentcolor",
            b"bolder",
            b"larger",
            b"1rem",
            b"3vw",
            b"1.5",
            b"line-height:",
            b"/*",
            b"*/",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let text = String::from_utf8_lossy(input);
        let parser = CssParser::default();
        let Ok((sheet, _)) = StyleSheet::parse(&parser, &text, Origin::Author) else {
            return Ok(()); // A resource limit is a legitimate outcome.
        };
        // Collecting @import rules for loading never changes the style rules.
        match StyleSheet::parse_with_imports(&parser, &text, Origin::Author) {
            Ok((with_imports, imports, _)) if with_imports == sheet => {
                if imports.iter().any(|i| text.get(i.span.range()).is_none()) {
                    return Err("an @import span is outside the sheet".into());
                }
            }
            _ => return Err("parse_with_imports disagreed with parse".into()),
        }
        let mut single = Cascade::new();
        single.push(sheet.clone());
        let reference = cascade_styles(&single);
        check_computed(&single)?;

        // Anonymous layers belong to their sheet, so only sheets without
        // them split or repeat cleanly. Both halves declare every layer, keeping the
        // layer order and the rules' layer indexes.
        let anonymous = sheet
            .layers
            .iter()
            .flat_map(|l| l.segments())
            .any(|s| matches!(s, LayerSegment::Anonymous(_)));
        if !anonymous {
            let middle = sheet.rules.len() / 2;
            let mut split = Cascade::new();
            split.push(StyleSheet {
                origin: Origin::Author,
                rules: sheet.rules[..middle].to_vec(),
                media: sheet.media.clone(),
                containers: sheet.containers.clone(),
                layers: sheet.layers.clone(),
                properties: sheet.properties.clone(),
            });
            split.push(StyleSheet {
                origin: Origin::Author,
                rules: sheet.rules[middle..].to_vec(),
                media: sheet.media.clone(),
                containers: sheet.containers.clone(),
                layers: sheet.layers.clone(),
                properties: sheet.properties.clone(),
            });
            if cascade_styles(&split) != reference {
                return Err("splitting the sheet changed the cascade".into());
            }
        }

        // A repeated anonymous layer is a new layer, so the copy may reorder.
        let mut doubled = single.clone();
        doubled.push(sheet.clone());
        if !anonymous && cascade_styles(&doubled) != reference {
            return Err("repeating the sheet changed the cascade".into());
        }

        let mut user_below = Cascade::new();
        user_below.push(StyleSheet {
            origin: Origin::User,
            ..StyleSheet::default()
        });
        user_below.push(sheet);
        if cascade_styles(&user_below) != reference {
            return Err("an empty user sheet changed the cascade".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_cover_the_whole_input() {
        let input = b"<p>hello<b>world";
        for plan in feeds(input) {
            assert_eq!(plan.concat(), input);
        }
        assert_eq!(feeds(&[b'x'; 600]).len(), 2);
        assert_eq!(plan(input), plan(input));
    }

    #[test]
    fn names_are_unique_and_resolvable() {
        let names: Vec<&str> = all().iter().map(|t| t.name()).collect();
        for name in &names {
            assert_eq!(by_name(name).map(|t| t.name()), Some(*name));
        }
        assert!(by_name("missing").is_none());
    }
}
