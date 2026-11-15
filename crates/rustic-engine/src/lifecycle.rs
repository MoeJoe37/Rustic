use std::fmt;
use std::sync::{Arc, Weak};

use rustic_base::CancellationToken;
use rustic_html::{HtmlDocumentParser, ParseError, ParsedDocument, ParserLimits};

/// The host's coordinator for pending loads and live documents.
///
/// Each load owns its parser and each live document owns its parsed tree. The
/// coordinator holds only weak references to their cancellation state. A
/// background job can carry a cancellation token without keeping either the
/// parser or the document alive.
#[derive(Debug)]
pub struct Engine {
    documents: Vec<Weak<CancellationToken>>,
    max_documents: usize,
    shutting_down: bool,
}

impl Engine {
    /// Set the maximum number of concurrent pending and completed documents.
    pub const fn new(max_documents: usize) -> Self {
        Self {
            documents: Vec::new(),
            max_documents,
            shutting_down: false,
        }
    }

    /// Admit an inert HTML load, with the same independent limits as the parser.
    pub fn begin_document(
        &mut self,
        limits: ParserLimits,
    ) -> Result<DocumentLoad, DocumentStartError> {
        if self.shutting_down {
            return Err(DocumentStartError::Shutdown);
        }
        self.documents
            .retain(|document| document.strong_count() != 0);
        if self.documents.len() == self.max_documents {
            return Err(DocumentStartError::DocumentLimit);
        }
        self.documents
            .try_reserve(1)
            .map_err(|_| DocumentStartError::Capacity)?;
        let scope = DocumentScope(Arc::new(CancellationToken::default()));
        let parser = HtmlDocumentParser::new(limits, scope.0.as_ref().clone())
            .map_err(DocumentStartError::Parser)?;
        self.documents.push(Arc::downgrade(&scope.0));
        Ok(DocumentLoad {
            pending: Some(PendingDocument { scope, parser }),
        })
    }

    /// Close admission and signal every pending and live document to stop.
    ///
    /// This is idempotent. Cancellation is cooperative. Hosts release their
    /// document owners and join any work they started at their own safe points.
    pub fn shutdown(&mut self) {
        self.shutting_down = true;
        for document in self.documents.drain(..).filter_map(|weak| weak.upgrade()) {
            document.cancel();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Why the engine could not admit a new document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentStartError {
    /// The engine has shut down.
    Shutdown,
    /// All configured document slots are in use.
    DocumentLimit,
    /// The document registry could not grow.
    Capacity,
    /// Parser initialization failed.
    Parser(ParseError),
}

impl fmt::Display for DocumentStartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shutdown => f.write_str("engine has shut down"),
            Self::DocumentLimit => f.write_str("engine document limit reached"),
            Self::Capacity => f.write_str("cannot allocate an engine document slot"),
            Self::Parser(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for DocumentStartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parser(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct DocumentScope(Arc<CancellationToken>);

impl Drop for DocumentScope {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[derive(Debug)]
struct PendingDocument {
    scope: DocumentScope,
    parser: HtmlDocumentParser,
}

/// An admitted HTML load that privately owns the partial tree.
#[derive(Debug)]
pub struct DocumentLoad {
    pending: Option<PendingDocument>,
}

impl DocumentLoad {
    /// The shared work token while the load is active.
    pub fn cancellation(&self) -> Option<&CancellationToken> {
        self.pending
            .as_ref()
            .map(|pending| pending.scope.0.as_ref())
    }

    /// Feed another chunk. A fatal error discards the load and releases its slot.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), ParseError> {
        let pending = self.pending.as_mut().ok_or(ParseError::Poisoned)?;
        let result = pending.parser.feed(bytes);
        if result.is_err() {
            self.pending = None;
        }
        result
    }

    /// Finish input and transfer the completed tree and work token together.
    pub fn finish(self) -> Result<EngineDocument, ParseError> {
        let PendingDocument { scope, mut parser } = self.pending.ok_or(ParseError::Poisoned)?;
        let parsed = parser.finish()?;
        Ok(EngineDocument { scope, parsed })
    }
}

/// An inert document whose owner controls the lifetime of its tree and work.
#[derive(Debug)]
pub struct EngineDocument {
    scope: DocumentScope,
    parsed: ParsedDocument,
}

impl EngineDocument {
    /// Inspect the completed document and its parse diagnostics.
    pub fn parsed(&self) -> &ParsedDocument {
        &self.parsed
    }

    /// The token to pass through operations on this document.
    ///
    /// Dropping the document or shutting down its engine signals all clones.
    pub fn cancellation(&self) -> &CancellationToken {
        &self.scope.0
    }
}
