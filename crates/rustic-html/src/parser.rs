use rustic_base::CancellationToken;
use rustic_dom::{Document, DocumentLimits, NodeId};

use crate::{Diagnostic, HtmlTokenizer, Token, TokenizerLimits};

mod error;
mod fragment;
mod mode;
mod tree;

pub use error::{ParseError, TreeDiagnostic, TreeDiagnosticKind};
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
    /// Maximum retained tree-construction recovery diagnostics. Later
    /// recoveries are counted in `dropped_diagnostics`, never fatal.
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

/// A completed document and its HTML parse diagnostics.
#[derive(Debug)]
pub struct ParsedDocument {
    document: Document,
    mode: DocumentMode,
    tokenizer_diagnostics: Vec<Diagnostic>,
    tree_diagnostics: Vec<TreeDiagnostic>,
    dropped_diagnostics: usize,
}

/// Tokenizer diagnostics handed from the driver to a finished tree builder.
#[derive(Debug)]
pub(crate) struct TokenizerDiagnostics {
    pub(crate) retained: Vec<Diagnostic>,
    pub(crate) dropped: usize,
}

impl ParsedDocument {
    /// Read the completed document; partial failed trees are never returned.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// Access the completed tree for checked document mutation.
    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.document
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

    /// Tokenizer and tree recoveries that exceeded the retention limits
    /// (saturating); they were recovered from but not recorded.
    pub fn dropped_diagnostics(&self) -> usize {
        self.dropped_diagnostics
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Lifecycle {
    Active,
    Finished,
    Failed,
}

/// Select how tree construction parses `noscript` and handles closed scripts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Scripting {
    /// Parse without a script host.
    #[default]
    Disabled,
    /// Parse `noscript` as raw text and surface HTML document script boundaries.
    /// Fragment parsers use the raw-text rules but do not surface boundaries.
    Enabled,
}

/// A closed HTML script element at its parser insertion point.
#[derive(Debug)]
pub struct ScriptBoundary<'a> {
    /// The partially built document, before the next token is processed.
    pub document: &'a Document,
    /// The script element whose end tag was just processed.
    pub element: NodeId,
}

/// Incremental UTF-8 parser implementing WHATWG document tree construction.
///
/// The default parser is inert. A host can opt into scripting-enabled tree
/// construction and inspect the document at script boundaries. Processing
/// instruction tokens fail explicitly. The parser owns its document until
/// successful EOF; any fatal error discards the partial tree and poisons it.
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
        Self::new_with_scripting(limits, cancellation, Scripting::Disabled)
    }

    /// Create a parser with an explicit scripting mode.
    pub fn new_with_scripting(
        limits: ParserLimits,
        cancellation: CancellationToken,
        scripting: Scripting,
    ) -> Result<Self, ParseError> {
        Ok(Self {
            driver: Driver::new(
                HtmlTokenizer::new(limits.tokenizer, cancellation)?,
                TreeBuilder::new(limits.document, limits.max_tree_diagnostics, scripting)?,
            ),
        })
    }

    /// Consume bytes through token/tree boundaries; feed splits do not change trees.
    /// This convenience method skips script boundaries. Use [`Self::write`] and
    /// [`Self::advance_to_script`] to observe them.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), ParseError> {
        self.driver.feed(bytes)
    }

    /// Queue bytes without running tree construction.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), ParseError> {
        self.driver.write(bytes)
    }

    /// Close the input stream without advancing tree construction.
    pub fn close_input(&mut self) -> Result<(), ParseError> {
        self.driver.close_input()
    }

    /// Process available input up to the next script that can be prepared.
    ///
    /// The host decides whether and how to prepare the element. Each boundary
    /// is returned once; calling this again resumes immediately after it.
    /// `None` means this parser needs more input, or has reached EOF if input
    /// is closed. Call [`Self::finish`] to obtain the completed document.
    pub fn advance_to_script(&mut self) -> Result<Option<ScriptBoundary<'_>>, ParseError> {
        let Some(element) = self.driver.advance()? else {
            return Ok(None);
        };
        let tree = self.driver.tree.as_ref().expect("active parser owns tree");
        Ok(Some(ScriptBoundary {
            document: tree.document(),
            element,
        }))
    }

    /// Read the active document between parser calls.
    pub fn live_document(&self) -> Option<&Document> {
        self.driver.tree.as_ref().map(TreeBuilder::document)
    }

    /// Lend the active document to the script host between parser calls.
    ///
    /// Checked same-document DOM mutations may detach or move nodes. The host
    /// must retain parser roots during collection and preserve their IDs
    /// until construction ends.
    pub fn live_document_mut(&mut self) -> Option<&mut Document> {
        self.driver.tree.as_mut().map(TreeBuilder::document_mut)
    }

    /// Current compatibility mode, including before construction is complete.
    pub fn live_document_mode(&self) -> Option<DocumentMode> {
        self.driver.tree.as_ref().map(TreeBuilder::document_mode)
    }

    /// Node identities retained by live construction state for joint collection.
    pub fn retained_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.driver
            .tree
            .iter()
            .flat_map(TreeBuilder::retained_nodes)
    }

    /// Close input, construct implied nodes and return the completed owned tree.
    /// Any remaining script boundaries are skipped.
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
    input_closed: bool,
}

impl Driver {
    fn new(tokenizer: HtmlTokenizer, tree: TreeBuilder) -> Self {
        Self {
            tokenizer,
            tree: Some(tree),
            lifecycle: Lifecycle::Active,
            input_closed: false,
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<(), ParseError> {
        self.write(bytes)?;
        self.drain()
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), ParseError> {
        self.ensure_writable()?;
        let result = self.tokenizer.write(bytes).map_err(ParseError::from);
        self.record_failure(result)
    }

    fn close_input(&mut self) -> Result<(), ParseError> {
        self.ensure_writable()?;
        let result = self.tokenizer.end().map_err(ParseError::from);
        self.record_failure(result)?;
        self.input_closed = true;
        Ok(())
    }

    fn finish(&mut self) -> Result<(TreeBuilder, TokenizerDiagnostics), ParseError> {
        self.ensure_active()?;
        if !self.input_closed {
            self.close_input()?;
        }
        self.drain()?;
        self.lifecycle = Lifecycle::Finished;
        let tree = self.tree.take().expect("active tree after successful EOF");
        let diagnostics = TokenizerDiagnostics {
            retained: self.tokenizer.diagnostics().to_vec(),
            dropped: self.tokenizer.dropped_diagnostics(),
        };
        Ok((tree, diagnostics))
    }

    fn drain(&mut self) -> Result<(), ParseError> {
        while self.advance()?.is_some() {}
        Ok(())
    }

    fn advance(&mut self) -> Result<Option<NodeId>, ParseError> {
        self.ensure_active()?;
        let result = self.pump();
        self.record_failure(result)
    }

    fn pump(&mut self) -> Result<Option<NodeId>, ParseError> {
        while let Some(token) = self.tokenizer.next_token()? {
            let tree = self.tree.as_mut().expect("active parser owns tree");
            let feedback = tree.process(&token)?;
            if let Some(mode) = feedback.text_mode {
                self.tokenizer.set_text_mode(mode)?;
            }
            self.tokenizer.set_cdata_allowed(tree.cdata_allowed());
            if let Some(element) = feedback.script {
                return Ok(Some(element));
            }
            if matches!(token, Token::Eof) {
                break;
            }
        }
        Ok(None)
    }

    fn ensure_writable(&self) -> Result<(), ParseError> {
        self.ensure_active()?;
        if self.input_closed {
            return Err(ParseError::InputClosed);
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
