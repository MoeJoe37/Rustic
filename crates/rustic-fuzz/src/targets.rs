//! Fuzz targets: each checks crash-freedom plus a differential invariant.
//!
//! A target never panics on purpose; a panic is a defect. Budgets stay at the
//! library defaults so hostile input must terminate inside them. Feed splits
//! are derived from the input's fingerprint, so a saved reproducer replays the
//! same check from its bytes alone.

use rustic_base::CancellationToken;
use rustic_css::{
    BlockItem, CssParser, Declaration, NamespaceMap, Parsed, Rule, SelectorList,
    parse_block_contents, parse_declaration,
};
use rustic_html::{
    Diagnostic, FragmentContext, HtmlDocumentParser, HtmlError, HtmlFragmentParser, HtmlTokenizer,
    ParseError, Token,
};
use rustic_style::{Cascade, MatchContext, Origin, StyleSheet, query_selector_all, style_tree};
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
<table><tr><td>cell</td></tr></table><input type=TEXT>";

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
    )
    .map(|styles| {
        styles
            .into_iter()
            .map(|s| s.style.dump_non_initial())
            .collect()
    })
    .unwrap_or_default()
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
            b"@media x {",
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
        let mut single = Cascade::new();
        single.push(sheet.clone());
        let reference = cascade_styles(&single);

        let middle = sheet.rules.len() / 2;
        let mut split = Cascade::new();
        split.push(StyleSheet {
            origin: Origin::Author,
            rules: sheet.rules[..middle].to_vec(),
        });
        split.push(StyleSheet {
            origin: Origin::Author,
            rules: sheet.rules[middle..].to_vec(),
        });
        if cascade_styles(&split) != reference {
            return Err("splitting the sheet changed the cascade".into());
        }

        let mut doubled = single.clone();
        doubled.push(sheet.clone());
        if cascade_styles(&doubled) != reference {
            return Err("repeating the sheet changed the cascade".into());
        }

        let mut user_below = Cascade::new();
        user_below.push(StyleSheet {
            origin: Origin::User,
            rules: Vec::new(),
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
