//! The HTML fragment parsing algorithm (scripting disabled).
//!
//! Specification: WHATWG HTML, "Parsing HTML fragments", reviewed 2026-11-08.
//! <https://html.spec.whatwg.org/multipage/parsing.html#parsing-html-fragments>

use rustic_base::CancellationToken;
use rustic_dom::{Document, NodeId};

use super::{Driver, Lifecycle, ParseError, ParserLimits, TreeBuilder, TreeDiagnostic};
use crate::{Attribute, Diagnostic, HtmlTokenizer, Tag};

/// Namespace of a fragment-parsing context element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextNamespace {
    /// The HTML namespace.
    Html,
    /// The SVG namespace.
    Svg,
    /// The MathML namespace.
    MathMl,
}

/// The context element a fragment is parsed for, as with `innerHTML`.
///
/// Only its namespace, local name and attributes matter. It has no document,
/// ancestors or form owner: a standalone context, so no ancestor `form` is
/// found and the fragment is parsed in no-quirks mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FragmentContext {
    namespace: ContextNamespace,
    local_name: String,
    attributes: Vec<Attribute>,
}

impl FragmentContext {
    /// An HTML context element; the name is ASCII-lowercased.
    pub fn html(local_name: &str) -> Self {
        Self::new(ContextNamespace::Html, local_name.to_ascii_lowercase())
    }

    /// An SVG context element, such as `svg`, `foreignObject` or `desc`.
    pub fn svg(local_name: &str) -> Self {
        Self::new(ContextNamespace::Svg, local_name.into())
    }

    /// A MathML context element, such as `math`, `mi` or `annotation-xml`.
    pub fn mathml(local_name: &str) -> Self {
        Self::new(ContextNamespace::MathMl, local_name.into())
    }

    fn new(namespace: ContextNamespace, local_name: String) -> Self {
        Self {
            namespace,
            local_name,
            attributes: Vec::new(),
        }
    }

    /// Add an attribute, for example `encoding="text/html"` on `annotation-xml`.
    /// A repeated name keeps its first value, as tokenized tags do.
    pub fn with_attribute(mut self, name: &str, value: &str) -> Self {
        let name = name.to_ascii_lowercase();
        if !self.attributes.iter().any(|a| a.name == name) {
            self.attributes.push(Attribute {
                name,
                value: value.into(),
            });
        }
        self
    }

    /// The context element's namespace.
    pub fn namespace(&self) -> ContextNamespace {
        self.namespace
    }

    /// The context element's local name.
    pub fn local_name(&self) -> &str {
        &self.local_name
    }

    pub(super) fn tag(&self) -> Tag {
        let mut tag = Tag::implied(&self.local_name);
        tag.attributes.clone_from(&self.attributes);
        tag
    }
}

/// A completed fragment: the children of [`ParsedFragment::root`].
#[derive(Debug)]
pub struct ParsedFragment {
    pub(super) document: Document,
    pub(super) root: NodeId,
    pub(super) tokenizer_diagnostics: Vec<Diagnostic>,
    pub(super) tree_diagnostics: Vec<TreeDiagnostic>,
    pub(super) dropped_diagnostics: usize,
}

impl ParsedFragment {
    /// The owning scratch document; the context element is not part of it.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// The scratch root `html` element whose children are the fragment nodes.
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// Transfer ownership for subsequent checked document mutations.
    pub fn into_document(self) -> Document {
        self.document
    }

    /// Bounded tokenizer recoveries with normalized code-point offsets.
    pub fn tokenizer_diagnostics(&self) -> &[Diagnostic] {
        &self.tokenizer_diagnostics
    }

    /// Bounded tree recoveries with zero-based delivered-token indexes.
    pub fn tree_diagnostics(&self) -> &[TreeDiagnostic] {
        &self.tree_diagnostics
    }

    /// Tokenizer and tree recoveries that exceeded the retention limits
    /// (saturating); they were recovered from but not recorded.
    pub fn dropped_diagnostics(&self) -> usize {
        self.dropped_diagnostics
    }
}

/// Incremental UTF-8 parser for the HTML fragment parsing algorithm.
///
/// Shares the document parser's tree construction, budgets, lifecycle and
/// failure atomicity. The context selects the initial tokenizer state and
/// insertion mode; feed boundaries never change the result.
#[derive(Debug)]
pub struct HtmlFragmentParser {
    driver: Driver,
}

impl HtmlFragmentParser {
    /// Create a fragment parser for `context` with explicit budgets.
    ///
    /// The detached context element and the scratch `html` root count toward
    /// the document node limit while parsing.
    pub fn new(
        context: &FragmentContext,
        limits: ParserLimits,
        cancellation: CancellationToken,
    ) -> Result<Self, ParseError> {
        let (tree, mode) =
            TreeBuilder::new_fragment(limits.document, limits.max_tree_diagnostics, context)?;
        // No last start tag: the context's own end tag is not "appropriate".
        let mut tokenizer =
            HtmlTokenizer::with_context(limits.tokenizer, cancellation, mode, None)?;
        // A foreign context allows CDATA before the first token.
        tokenizer.set_cdata_allowed(tree.cdata_allowed());
        Ok(Self {
            driver: Driver {
                tokenizer,
                tree: Some(tree),
                lifecycle: Lifecycle::Active,
            },
        })
    }

    /// Consume bytes through token/tree boundaries; feed splits do not change trees.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), ParseError> {
        self.driver.feed(bytes)
    }

    /// Close input and return the completed fragment.
    pub fn finish(&mut self) -> Result<ParsedFragment, ParseError> {
        let (tree, diagnostics) = self.driver.finish()?;
        tree.into_fragment(diagnostics)
    }
}
