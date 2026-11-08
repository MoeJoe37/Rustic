//! Fuzz targets: each checks crash-freedom plus a differential invariant.
//!
//! A target never panics on purpose; a panic is a defect. Budgets stay at the
//! library defaults so hostile input must terminate inside them. Feed splits
//! are derived from the input's fingerprint, so a saved reproducer replays the
//! same check from its bytes alone.

use rustic_base::CancellationToken;
use rustic_css::{BlockItem, CssParser, Rule, parse_block_contents};
use rustic_html::{
    Diagnostic, FragmentContext, HtmlDocumentParser, HtmlError, HtmlFragmentParser, HtmlTokenizer,
    ParseError, Token,
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
