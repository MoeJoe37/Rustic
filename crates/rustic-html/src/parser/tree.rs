use rustic_dom::{
    Document, DocumentLimits, DocumentType, DomError, NodeId, NodeKind, QualifiedName,
};

use crate::{Diagnostic, Doctype, Tag, Token, TokenizerMode};

use super::{
    DocumentMode, ParseError, ParsedDocument, TreeDiagnostic, TreeDiagnosticKind as ErrorKind,
    UnsupportedFeature,
};

mod body;
mod characters;
mod head;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InsertionMode {
    Initial,
    BeforeHtml,
    BeforeHead,
    InHead,
    InHeadNoscript,
    AfterHead,
    InBody,
    Text,
    AfterBody,
    AfterAfterBody,
}

enum Step {
    Consumed,
    Reprocess,
    TextMode(TokenizerMode),
}

/// Owns all construction state; the tokenizer never edits a node directly.
#[derive(Debug)]
pub(super) struct TreeBuilder {
    document: Document,
    document_mode: DocumentMode,
    insertion_mode: InsertionMode,
    original_mode: InsertionMode,
    open: Vec<NodeId>,
    head: Option<NodeId>,
    diagnostics: Vec<TreeDiagnostic>,
    max_diagnostics: usize,
    token_index: usize,
    done: bool,
    self_closing_acknowledged: bool,
}

impl TreeBuilder {
    pub(super) fn new(limits: DocumentLimits, max_diagnostics: usize) -> Result<Self, ParseError> {
        Ok(Self {
            document: Document::new(limits)?,
            document_mode: DocumentMode::NoQuirks,
            insertion_mode: InsertionMode::Initial,
            original_mode: InsertionMode::Initial,
            open: Vec::new(),
            head: None,
            diagnostics: Vec::new(),
            max_diagnostics,
            token_index: 0,
            done: false,
            self_closing_acknowledged: false,
        })
    }

    pub(super) fn process(&mut self, token: &Token) -> Result<Option<TokenizerMode>, ParseError> {
        self.self_closing_acknowledged = false;
        let mut selected_mode = None;
        if let Token::Characters(text) = token {
            self.characters(text)?;
        } else {
            // Reprocessing advances a finite insertion-mode chain; no recursion
            // or token cloning, including EOF through all implied shell elements.
            loop {
                let step = match self.insertion_mode {
                    InsertionMode::Initial => self.initial(token)?,
                    InsertionMode::BeforeHtml => self.before_html(token)?,
                    InsertionMode::BeforeHead => self.before_head(token)?,
                    InsertionMode::InHead => {
                        self.in_head(token, self.current(), InsertionMode::InHead)?
                    }
                    InsertionMode::InHeadNoscript => self.in_head_noscript(token)?,
                    InsertionMode::AfterHead => self.after_head(token)?,
                    InsertionMode::InBody => self.in_body(token)?,
                    InsertionMode::Text => self.text_end(token)?,
                    InsertionMode::AfterBody => self.after_body(token)?,
                    InsertionMode::AfterAfterBody => self.after_after_body(token)?,
                };
                match step {
                    Step::Consumed => break,
                    Step::Reprocess => continue,
                    Step::TextMode(mode) => {
                        selected_mode = Some(mode);
                        break;
                    }
                }
            }
        }
        if matches!(token, Token::StartTag(tag) if tag.self_closing() && !self.self_closing_acknowledged)
        {
            self.diagnostic(ErrorKind::UnacknowledgedSelfClosing)?;
        }
        // Delivery count is bounded by the tokenizer's cumulative token budget.
        self.token_index = self
            .token_index
            .checked_add(1)
            .ok_or(ParseError::Tokenizer(crate::HtmlError::TokenLimit))?;
        Ok(selected_mode)
    }

    pub(super) fn into_result(self, tokenizer_diagnostics: Vec<Diagnostic>) -> ParsedDocument {
        assert!(self.done, "result requires successfully processed EOF");
        ParsedDocument {
            document: self.document,
            mode: self.document_mode,
            tokenizer_diagnostics,
            tree_diagnostics: self.diagnostics,
        }
    }

    fn initial(&mut self, token: &Token) -> Result<Step, ParseError> {
        match token {
            Token::Comment(text) => self.comment(self.document.root(), text)?,
            Token::ProcessingInstruction(_) => {
                return Err(ParseError::Unsupported(
                    UnsupportedFeature::ProcessingInstructions,
                ));
            }
            Token::Doctype(doctype) => {
                self.doctype(doctype)?;
                self.insertion_mode = InsertionMode::BeforeHtml;
            }
            _ => {
                self.missing_doctype()?;
                return Ok(Step::Reprocess);
            }
        }
        Ok(Step::Consumed)
    }

    fn before_html(&mut self, token: &Token) -> Result<Step, ParseError> {
        match token {
            Token::Comment(text) => self.comment(self.document.root(), text)?,
            Token::Doctype(_) => self.diagnostic(ErrorKind::UnexpectedToken)?,
            Token::ProcessingInstruction(_) => {
                return Err(ParseError::Unsupported(
                    UnsupportedFeature::ProcessingInstructions,
                ));
            }
            Token::StartTag(tag) if tag.name() == "html" => {
                self.insert(self.document.root(), tag, true)?;
                self.insertion_mode = InsertionMode::BeforeHead;
            }
            Token::EndTag(name) if !matches!(name.as_str(), "head" | "body" | "html" | "br") => {
                self.diagnostic(ErrorKind::UnexpectedToken)?
            }
            _ => {
                self.imply_html()?;
                return Ok(Step::Reprocess);
            }
        }
        Ok(Step::Consumed)
    }

    fn before_head(&mut self, token: &Token) -> Result<Step, ParseError> {
        match token {
            Token::Comment(text) => self.comment(self.current(), text)?,
            Token::Doctype(_) => self.diagnostic(ErrorKind::UnexpectedToken)?,
            Token::ProcessingInstruction(_) => {
                return Err(ParseError::Unsupported(
                    UnsupportedFeature::ProcessingInstructions,
                ));
            }
            Token::StartTag(tag) if tag.name() == "html" => self.html_tag(tag)?,
            Token::StartTag(tag) if tag.name() == "head" => {
                self.head = Some(self.insert(self.current(), tag, true)?);
                self.insertion_mode = InsertionMode::InHead;
            }
            Token::EndTag(name) if !matches!(name.as_str(), "head" | "body" | "html" | "br") => {
                self.diagnostic(ErrorKind::UnexpectedToken)?
            }
            _ => {
                self.imply_head()?;
                return Ok(Step::Reprocess);
            }
        }
        Ok(Step::Consumed)
    }

    fn doctype(&mut self, token: &Doctype) -> Result<(), ParseError> {
        if token.name() != Some("html")
            || token.public_id().is_some()
            || token
                .system_id()
                .is_some_and(|value| value != "about:legacy-compat")
        {
            self.diagnostic(ErrorKind::NonconformingDoctype)?;
        }
        let id = self.document.create_doctype(DocumentType {
            name: token.name().unwrap_or("").into(),
            public_id: token.public_id().unwrap_or("").into(),
            system_id: token.system_id().unwrap_or("").into(),
        })?;
        self.attach_or_destroy(self.document.root(), id)?;
        self.document_mode = super::mode::classify(token);
        Ok(())
    }

    fn missing_doctype(&mut self) -> Result<(), ParseError> {
        self.diagnostic(ErrorKind::MissingDoctype)?;
        self.document_mode = DocumentMode::Quirks;
        self.insertion_mode = InsertionMode::BeforeHtml;
        Ok(())
    }

    fn imply_html(&mut self) -> Result<(), ParseError> {
        self.insert_implied(self.document.root(), "html")?;
        self.insertion_mode = InsertionMode::BeforeHead;
        Ok(())
    }

    fn imply_head(&mut self) -> Result<(), ParseError> {
        self.head = Some(self.insert_implied(self.current(), "head")?);
        self.insertion_mode = InsertionMode::InHead;
        Ok(())
    }

    fn imply_body(&mut self) -> Result<(), ParseError> {
        self.insert_implied(self.current(), "body")?;
        self.insertion_mode = InsertionMode::InBody;
        Ok(())
    }

    fn current(&self) -> NodeId {
        *self
            .open
            .last()
            .expect("insertion mode requires a current element")
    }

    fn name(&self, id: NodeId) -> &str {
        match self.document.node(id).expect("live parser node").kind() {
            NodeKind::Element(element) => element.name().local(),
            _ => unreachable!("open stack only contains elements"),
        }
    }

    fn diagnostic(&mut self, kind: ErrorKind) -> Result<(), ParseError> {
        if self.diagnostics.len() == self.max_diagnostics {
            return Err(ParseError::TreeDiagnosticLimit);
        }
        self.diagnostics
            .try_reserve(1)
            .map_err(|_| DomError::AllocationFailed)?;
        self.diagnostics.push(TreeDiagnostic {
            kind,
            token_index: self.token_index,
        });
        Ok(())
    }

    fn insert_implied(&mut self, parent: NodeId, name: &str) -> Result<NodeId, ParseError> {
        self.insert(
            parent,
            &Tag {
                name: name.into(),
                attributes: Vec::new(),
                self_closing: false,
            },
            true,
        )
    }

    fn insert(&mut self, parent: NodeId, tag: &Tag, push: bool) -> Result<NodeId, ParseError> {
        if push {
            self.open
                .try_reserve(1)
                .map_err(|_| DomError::AllocationFailed)?;
        }
        let id = self
            .document
            .create_element(QualifiedName::html(tag.name())?)?;
        if let Err(error) = self.add_missing_attributes(id, tag) {
            self.document
                .destroy_subtree(id)
                .expect("unattached new element");
            return Err(error);
        }
        self.attach_or_destroy(parent, id)?;
        if push {
            self.open.push(id);
        } else if tag.self_closing() {
            self.self_closing_acknowledged = true;
        }
        Ok(id)
    }

    fn add_missing_attributes(&mut self, id: NodeId, tag: &Tag) -> Result<(), ParseError> {
        for attribute in tag.attributes() {
            let exists = match self.document.node(id)?.kind() {
                NodeKind::Element(element) => element.attributes().iter().any(|a| {
                    a.name().namespace().is_none() && a.name().local() == attribute.name()
                }),
                _ => unreachable!("attribute target is an element"),
            };
            if !exists {
                self.document.set_attribute(
                    id,
                    QualifiedName::from_parsed(None, None, attribute.name())?,
                    attribute.value(),
                )?;
            }
        }
        Ok(())
    }

    fn attach_or_destroy(&mut self, parent: NodeId, id: NodeId) -> Result<(), ParseError> {
        if let Err(error) = self.document.append_child(parent, id) {
            self.document
                .destroy_subtree(id)
                .expect("new unattached parser node");
            return Err(error.into());
        }
        Ok(())
    }

    fn comment(&mut self, parent: NodeId, text: &str) -> Result<(), ParseError> {
        let id = self.document.create_comment(text)?;
        self.attach_or_destroy(parent, id)
    }

    fn insert_text(&mut self, parent: NodeId, text: &str) -> Result<(), ParseError> {
        if text.is_empty() {
            return Ok(());
        }
        let last = self.document.node(parent)?.last_child();
        if let Some(id) = last {
            if matches!(self.document.node(id)?.kind(), NodeKind::Text(_)) {
                self.document.append_utf8_character_data(id, text)?;
                return Ok(());
            }
        }
        let id = self.document.create_text(text)?;
        self.attach_or_destroy(parent, id)
    }
}
