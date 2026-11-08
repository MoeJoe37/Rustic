//! Mutated HTML/style input exercises bounded anonymous flow normalization.

use crate::Target;
use rustic_base::CancellationToken;
use rustic_css::CssParser;
use rustic_html::HtmlDocumentParser;
use rustic_layout::{
    BoxKind, BoxTree, FormattingKind as K, FormattingLimits, FormattingTree, LayoutLimits,
};
use rustic_style::{ComputeContext, MatchContext, document_cascade, style_tree};

pub(crate) struct FormattingTarget;
impl Target for FormattingTarget {
    fn name(&self) -> &'static str {
        "layout-formatting"
    }
    fn max_len(&self) -> usize {
        4096
    }
    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"<div>",
            b"</div>",
            b"<span>",
            b"</span>",
            b"<em>",
            b"</em>",
            b"<style>",
            b"</style>",
            b"display:contents",
            b"display:none",
            b"display:inline-block",
            b"white-space:pre",
            b"<span style='display:block'>",
            b"<span style='display:inline-block'>",
            b"<section>",
            b"</section>",
            b"<template>",
            b"</template>",
            b"\n\t ",
            b"&#x1f600;",
            b"&nbsp;",
            b"x",
        ]
    }
    fn check(&self, input: &[u8]) -> Result<(), String> {
        let mut parser = HtmlDocumentParser::default();
        if parser.feed(input).is_err() {
            return Ok(());
        }
        let Ok(parsed) = parser.finish() else {
            return Ok(());
        };
        let document = parsed.document();
        let parser = CssParser::default();
        let (cascade, _) = document_cascade(document, &parser).map_err(|e| e.to_string())?;
        let styles = style_tree(
            document,
            document.root(),
            &cascade,
            &parser,
            &MatchContext::html(),
            &ComputeContext::default(),
        )
        .map_err(|e| e.to_string())?;
        let Ok(principal) = BoxTree::build(
            document,
            document.root(),
            &styles,
            LayoutLimits::default(),
            &CancellationToken::default(),
        ) else {
            return Ok(());
        };
        let Ok(tree) = FormattingTree::build(
            &principal,
            FormattingLimits::default(),
            &CancellationToken::default(),
        ) else {
            return Ok(());
        };
        let mut seen = vec![false; principal.boxes().len()];
        let mut children = vec![(0usize, 0usize); tree.boxes().len()];
        let mut previous_text = None;
        for (i, b) in tree.boxes().iter().enumerate() {
            if tree.style(i).is_none() {
                return Err("formatting member lacks a style".into());
            }
            if let Some(p) = b.parent() {
                if p >= i {
                    return Err("formatting parent is not earlier in preorder".into());
                }
                children[p].0 += 1;
                if matches!(b.kind(), K::Block(_) | K::AnonymousBlock) {
                    children[p].1 += 1;
                }
            }
            if b.kind() == K::Text {
                let source = b.source().ok_or("text has no principal source")?;
                if seen[source] || previous_text.is_some_and(|p| p >= source) {
                    return Err("text reordered or duplicated".into());
                }
                seen[source] = true;
                previous_text = Some(source);
                if tree.text(i) != principal.boxes()[source].text() {
                    return Err("UTF-16 text changed".into());
                }
            }
        }
        for (i, &(total, blocks)) in children.iter().enumerate() {
            if blocks != 0 && (total != blocks || tree.boxes()[i].kind() == K::Inline) {
                return Err("block and inline children mixed after normalization".into());
            }
        }
        for &source in tree.suppressed_whitespace() {
            if seen[source] || principal.boxes()[source].kind() != BoxKind::Text {
                return Err("invalid suppressed whitespace source".into());
            }
            if !principal.boxes()[source]
                .text()
                .iter()
                .all(|u| matches!(u, 9 | 10 | 12 | 13 | 32))
            {
                return Err("non-whitespace text suppressed".into());
            }
            seen[source] = true;
        }
        for (i, b) in principal.boxes().iter().enumerate() {
            if b.kind() == BoxKind::Text && !seen[i] {
                return Err("source text silently lost".into());
            }
        }
        Ok(())
    }
}
