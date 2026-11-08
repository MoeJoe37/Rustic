use rustic_base::CancellationToken;
use rustic_dom::{Document, DocumentLimits};

use crate::{Diagnostic, HtmlTokenizer, Token, TokenizerLimits};

mod error;
mod fragment;
mod mode;
mod tree;

pub use error::{ParseError, TreeDiagnostic, TreeDiagnosticKind, UnsupportedFeature};
pub use fragment::{ContextNamespace, FragmentContext, HtmlFragmentParser, ParsedFragment};
pub use mode::DocumentMode;
use tree::TreeBuilder;

/// Independent input/token, document payload/tree and recovery-diagnostic limits.
#[derive(Clone, Copy, Debug)]
pub struct ParserLimits {
    /// Byte decoding and tokenizer limits, including tokenizer diagnostics.
    pub tokenizer: TokenizerLimits,
    /// Connected/detached node, edge-depth and payload limits.
    pub document: DocumentLimits,
    /// Maximum retained tree-construction recovery diagnostics.
    pub max_tree_diagnostics: usize,
}

impl Default for ParserLimits {
    fn default() -> Self {
        Self {
            tokenizer: TokenizerLimits::default(),
            document: DocumentLimits::default(),
            max_tree_diagnostics: 1024,
        }
    }
}

/// A completed inert document built with scripting disabled.
#[derive(Debug)]
pub struct ParsedDocument {
    document: Document,
    mode: DocumentMode,
    tokenizer_diagnostics: Vec<Diagnostic>,
    tree_diagnostics: Vec<TreeDiagnostic>,
}

impl ParsedDocument {
    /// Read the completed document; partial failed trees are never returned.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// Transfer ownership for subsequent checked document mutations.
    pub fn into_document(self) -> Document {
        self.document
    }

    /// Doctype-derived compatibility mode; no layout behavior is implied yet.
    pub fn mode(&self) -> DocumentMode {
        self.mode
    }

    /// Bounded tokenizer recoveries with normalized code-point offsets.
    pub fn tokenizer_diagnostics(&self) -> &[Diagnostic] {
        &self.tokenizer_diagnostics
    }

    /// Bounded tree recoveries with zero-based delivered-token indexes.
    pub fn tree_diagnostics(&self) -> &[TreeDiagnostic] {
        &self.tree_diagnostics
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Lifecycle {
    Active,
    Finished,
    Failed,
}

/// Incremental UTF-8 parser implementing WHATWG document tree construction.
///
/// Scripting is disabled; no script, network request or subresource is executed.
/// Processing-instruction tokens, which have no DOM node yet, fail explicitly. The parser owns its document
/// until successful EOF; any fatal error discards the partial tree and poisons
/// the parser. Rejected calls after successful completion preserve that state.
#[derive(Debug)]
pub struct HtmlDocumentParser {
    driver: Driver,
}

impl Default for HtmlDocumentParser {
    fn default() -> Self {
        Self::new(ParserLimits::default(), CancellationToken::default())
            .expect("valid default document parser limits")
    }
}

impl HtmlDocumentParser {
    /// Create a parser with explicit independent budgets and cancellation.
    pub fn new(limits: ParserLimits, cancellation: CancellationToken) -> Result<Self, ParseError> {
        Ok(Self {
            driver: Driver {
                tokenizer: HtmlTokenizer::new(limits.tokenizer, cancellation)?,
                tree: Some(TreeBuilder::new(
                    limits.document,
                    limits.max_tree_diagnostics,
                )?),
                lifecycle: Lifecycle::Active,
            },
        })
    }

    /// Consume bytes through token/tree boundaries; feed splits do not change trees.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), ParseError> {
        self.driver.feed(bytes)
    }

    /// Close input, construct implied nodes and return the completed owned tree.
    pub fn finish(&mut self) -> Result<ParsedDocument, ParseError> {
        let (tree, diagnostics) = self.driver.finish()?;
        Ok(tree.into_result(diagnostics))
    }
}

/// Tokenizer/tree-builder pump and lifecycle shared by document and fragment parsing.
#[derive(Debug)]
struct Driver {
    tokenizer: HtmlTokenizer,
    tree: Option<TreeBuilder>,
    lifecycle: Lifecycle,
}

impl Driver {
    fn feed(&mut self, bytes: &[u8]) -> Result<(), ParseError> {
        self.ensure_active()?;
        let result = self
            .tokenizer
            .write(bytes)
            .map_err(ParseError::from)
            .and_then(|()| self.pump());
        self.record_failure(result)
    }

    fn finish(&mut self) -> Result<(TreeBuilder, Vec<Diagnostic>), ParseError> {
        self.ensure_active()?;
        let result = self
            .tokenizer
            .end()
            .map_err(ParseError::from)
            .and_then(|()| self.pump());
        self.record_failure(result)?;
        self.lifecycle = Lifecycle::Finished;
        let tree = self.tree.take().expect("active tree after successful EOF");
        Ok((tree, self.tokenizer.diagnostics().to_vec()))
    }

    fn pump(&mut self) -> Result<(), ParseError> {
        while let Some(token) = self.tokenizer.next_token()? {
            let tree = self.tree.as_mut().expect("active parser owns tree");
            if let Some(mode) = tree.process(&token)? {
                self.tokenizer.set_text_mode(mode)?;
            }
            self.tokenizer.set_cdata_allowed(tree.cdata_allowed());
            if matches!(token, Token::Eof) {
                break;
            }
        }
        Ok(())
    }

    fn ensure_active(&self) -> Result<(), ParseError> {
        match self.lifecycle {
            Lifecycle::Active => Ok(()),
            Lifecycle::Finished => Err(ParseError::AlreadyFinished),
            Lifecycle::Failed => Err(ParseError::Poisoned),
        }
    }

    fn record_failure<T>(&mut self, result: Result<T, ParseError>) -> Result<T, ParseError> {
        if result.is_err() {
            self.lifecycle = Lifecycle::Failed;
            self.tree = None;
        }
        result
    }
}
