use rustic_dom::{
    Document, DocumentLimits, DocumentType, DomError, Element, MATHML_NAMESPACE, NodeId, NodeKind,
    ReadyState, SVG_NAMESPACE,
};

use crate::{Doctype, ProcessingInstruction, Tag, Token, TokenizerMode};

use super::{
    ContextNamespace, DocumentMode, FragmentContext, ParseError, ParsedDocument, ParsedFragment,
    Scripting, TokenizerDiagnostics, TreeDiagnostic, TreeDiagnosticKind as ErrorKind,
};

mod body;
mod foreign;
mod formatting;
mod insert;
mod select;
mod shell;
mod stack;
mod table;

use formatting::Entry;

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
    InTable,
    InTableText,
    InCaption,
    InColumnGroup,
    InTableBody,
    InRow,
    InCell,
    InTemplate,
    AfterBody,
    InFrameset,
    AfterFrameset,
    AfterAfterBody,
    AfterAfterFrameset,
}

/// Element namespaces that tree construction distinguishes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ns {
    Html,
    MathMl,
    Svg,
}

/// Character-token class. Grouped tokenizer text is split into homogeneous
/// runs so each insertion mode sees the standard per-character distinction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Chars {
    Space,
    Null,
    Other,
}

/// A borrowed token view; character data is one homogeneous run.
#[derive(Clone, Copy, Debug)]
enum Tok<'a> {
    Text(Chars, &'a str),
    Start(&'a Tag),
    End(&'a str),
    Annotation(Annotation<'a>),
    Doctype(&'a Doctype),
    Eof,
}

/// Tokens with the same placement rules and distinct DOM node kinds.
#[derive(Clone, Copy, Debug)]
enum Annotation<'a> {
    Comment(&'a str),
    Instruction(&'a ProcessingInstruction),
}

enum Step {
    Consumed,
    Reprocess,
}

pub(super) struct TreeFeedback {
    pub(super) text_mode: Option<TokenizerMode>,
    pub(super) script: Option<NodeId>,
}

fn space(value: char) -> bool {
    matches!(value, '\t' | '\n' | '\u{c}' | '\r' | ' ')
}

fn class(value: char) -> Chars {
    if value == '\0' {
        Chars::Null
    } else if space(value) {
        Chars::Space
    } else {
        Chars::Other
    }
}

/// Split text into maximal runs of one character class.
fn runs(text: &str) -> impl Iterator<Item = (Chars, &str)> {
    let mut rest = text;
    std::iter::from_fn(move || {
        let first = class(rest.chars().next()?);
        let end = rest
            .char_indices()
            .find(|(_, value)| class(*value) != first)
            .map_or(rest.len(), |(index, _)| index);
        let (run, tail) = rest.split_at(end);
        rest = tail;
        Some((first, run))
    })
}

/// Owns all construction state; the tokenizer never edits a node directly.
#[derive(Debug)]
pub(super) struct TreeBuilder {
    document: Document,
    document_mode: DocumentMode,
    scripting: Scripting,
    exposed_document: bool,
    insertion_mode: InsertionMode,
    original_mode: InsertionMode,
    template_modes: Vec<InsertionMode>,
    open: Vec<NodeId>,
    formatting: Vec<Entry>,
    head: Option<NodeId>,
    form: Option<NodeId>,
    frameset_ok: bool,
    foster_parenting: bool,
    pending_table_text: String,
    pending_table_text_other: bool,
    skip_newline: bool,
    text_mode: Option<TokenizerMode>,
    script_boundary: Option<NodeId>,
    diagnostics: Vec<TreeDiagnostic>,
    max_diagnostics: usize,
    dropped_diagnostics: usize,
    token_index: usize,
    done: bool,
    self_closing_acknowledged: bool,
    /// Detached fragment-parsing context element; `None` for documents.
    context: Option<NodeId>,
    /// `(select, option)` with selectedness, one entry per single-selection select.
    selected_options: Vec<(NodeId, NodeId)>,
    /// Inserted options in a select, not yet popped and mirrored.
    pending_options: Vec<(NodeId, NodeId)>,
}

impl TreeBuilder {
    pub(super) fn new(
        limits: DocumentLimits,
        max_diagnostics: usize,
        scripting: Scripting,
    ) -> Result<Self, ParseError> {
        Ok(Self {
            document: match scripting {
                Scripting::Disabled => Document::new(limits)?,
                Scripting::Enabled => Document::new_for_parsing(limits)?,
            },
            document_mode: DocumentMode::NoQuirks,
            scripting,
            exposed_document: false,
            insertion_mode: InsertionMode::Initial,
            original_mode: InsertionMode::Initial,
            template_modes: Vec::new(),
            open: Vec::new(),
            formatting: Vec::new(),
            head: None,
            form: None,
            frameset_ok: true,
            foster_parenting: false,
            pending_table_text: String::new(),
            pending_table_text_other: false,
            skip_newline: false,
            text_mode: None,
            script_boundary: None,
            diagnostics: Vec::new(),
            max_diagnostics,
            dropped_diagnostics: 0,
            token_index: 0,
            done: false,
            self_closing_acknowledged: false,
            context: None,
            selected_options: Vec::new(),
            pending_options: Vec::new(),
        })
    }

    /// Set up the HTML fragment parsing algorithm for `context`.
    ///
    /// The document gets a root `html` element whose children become the
    /// fragment. The context element is created detached (it is never part of
    /// the result) so namespace and integration-point checks can inspect it.
    /// Returns the tokenizer state the context and scripting mode select.
    pub(super) fn new_fragment(
        limits: DocumentLimits,
        max_diagnostics: usize,
        context: &FragmentContext,
        scripting: Scripting,
    ) -> Result<(Self, TokenizerMode), ParseError> {
        let mut builder = Self::new(limits, max_diagnostics, scripting)?;
        if scripting == Scripting::Enabled {
            builder.document.advance_ready_state(ReadyState::Complete)?;
        }
        let html = builder.create_element(&Tag::implied("html"), Ns::Html)?;
        let root = builder.document.root();
        builder.attach_or_destroy(root, None, html)?;
        builder.open.push(html);
        let ns = match context.namespace() {
            ContextNamespace::Html => Ns::Html,
            ContextNamespace::Svg => Ns::Svg,
            ContextNamespace::MathMl => Ns::MathMl,
        };
        let element = builder.create_element(&context.tag(), ns)?;
        builder.context = Some(element);
        let name = builder.html_name(element).map(str::to_owned);
        if name.as_deref() == Some("template") {
            builder.template_modes.push(InsertionMode::InTemplate);
        }
        if name.as_deref() == Some("form") {
            builder.form = Some(element);
        }
        builder.reset_insertion_mode();
        let mode = match name.as_deref() {
            Some("title" | "textarea") => TokenizerMode::Rcdata,
            Some("style" | "xmp" | "iframe" | "noembed" | "noframes") => TokenizerMode::Rawtext,
            Some("noscript") if scripting == Scripting::Enabled => TokenizerMode::Rawtext,
            Some("script") => TokenizerMode::ScriptData,
            Some("plaintext") => TokenizerMode::Plaintext,
            _ => TokenizerMode::Data,
        };
        Ok((builder, mode))
    }

    /// Whether this builder runs the fragment parsing algorithm.
    fn fragment(&self) -> bool {
        self.context.is_some()
    }

    /// Whether the fragment context is the HTML element `name`.
    fn context_is(&self, name: &str) -> bool {
        self.context.is_some_and(|id| self.is_html(id, name))
    }

    /// The standard's adjusted current node: the context element when only
    /// the fragment's root `html` element is open, otherwise the current node.
    fn adjusted_current(&self) -> Option<NodeId> {
        match self.context {
            Some(context) if self.open.len() == 1 => Some(context),
            _ => self.open.last().copied(),
        }
    }

    pub(super) fn document(&self) -> &Document {
        &self.document
    }

    pub(super) fn document_mut(&mut self) -> &mut Document {
        self.exposed_document = true;
        &mut self.document
    }

    pub(super) fn document_mode(&self) -> DocumentMode {
        self.document_mode
    }

    pub(super) fn retained_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.open
            .iter()
            .copied()
            .chain(self.head)
            .chain(self.form)
            .chain(self.context)
            .chain(self.formatting.iter().filter_map(|entry| match entry {
                Entry::Element(node, _) => Some(*node),
                Entry::Marker => None,
            }))
            .chain(
                self.selected_options
                    .iter()
                    .chain(&self.pending_options)
                    .flat_map(|&(select, option)| [select, option]),
            )
            .filter(|&node| self.document.node(node).is_ok())
    }

    pub(super) fn process(&mut self, token: &Token) -> Result<TreeFeedback, ParseError> {
        self.self_closing_acknowledged = false;
        self.text_mode = None;
        let skip_newline = std::mem::take(&mut self.skip_newline);
        match token {
            Token::Characters(text) => {
                let text = if skip_newline {
                    text.strip_prefix('\n').unwrap_or(text)
                } else {
                    text
                };
                for (class, run) in runs(text) {
                    self.dispatch(Tok::Text(class, run))?;
                }
            }
            Token::StartTag(tag) => self.dispatch(Tok::Start(tag))?,
            Token::EndTag(name) => self.dispatch(Tok::End(name))?,
            Token::Comment(text) => self.dispatch(Tok::Annotation(Annotation::Comment(text)))?,
            Token::Doctype(doctype) => self.dispatch(Tok::Doctype(doctype))?,
            Token::ProcessingInstruction(instruction) => {
                self.dispatch(Tok::Annotation(Annotation::Instruction(instruction)))?;
            }
            Token::Eof => self.dispatch(Tok::Eof)?,
        }
        self.mirror_popped_options(self.done)?;
        if matches!(token, Token::StartTag(tag) if tag.self_closing() && !self.self_closing_acknowledged)
        {
            self.diagnostic(ErrorKind::UnacknowledgedSelfClosing)?;
        }
        // Delivery count is bounded by the tokenizer's cumulative token budget.
        self.token_index = self
            .token_index
            .checked_add(1)
            .ok_or(ParseError::Tokenizer(crate::HtmlError::TokenLimit))?;
        Ok(TreeFeedback {
            text_mode: self.text_mode.take(),
            script: self.script_boundary.take(),
        })
    }

    /// Whether the tokenizer may recognize CDATA sections before the next token.
    pub(super) fn cdata_allowed(&self) -> bool {
        self.adjusted_current()
            .is_some_and(|id| self.ns(id) != Ns::Html)
    }

    pub(super) fn into_result(mut self, tokenizer: TokenizerDiagnostics) -> ParsedDocument {
        if self.document.ready_state() != ReadyState::Complete {
            self.document
                .advance_ready_state(ReadyState::Complete)
                .expect("readiness advances");
        }
        assert!(self.done, "result requires successfully processed EOF");
        ParsedDocument {
            document: self.document,
            mode: self.document_mode,
            tokenizer_diagnostics: tokenizer.retained,
            tree_diagnostics: self.diagnostics,
            dropped_diagnostics: tokenizer.dropped.saturating_add(self.dropped_diagnostics),
        }
    }

    pub(super) fn into_fragment(
        mut self,
        tokenizer: TokenizerDiagnostics,
    ) -> Result<ParsedFragment, ParseError> {
        assert!(self.done, "result requires successfully processed EOF");
        let context = self.context.take().expect("fragment builder has a context");
        self.document.destroy_subtree(context)?;
        let root = self
            .document
            .node(self.document.root())?
            .first_child()
            .expect("fragment root html element");
        Ok(ParsedFragment {
            document: self.document,
            root,
            tokenizer_diagnostics: tokenizer.retained,
            tree_diagnostics: self.diagnostics,
            dropped_diagnostics: tokenizer.dropped.saturating_add(self.dropped_diagnostics),
        })
    }

    /// The tree construction dispatcher. Reprocessing advances a finite chain
    /// of insertion modes or pops open elements; no token is cloned.
    fn dispatch(&mut self, token: Tok) -> Result<(), ParseError> {
        loop {
            let step = if self.html_rules(token) {
                self.step(self.insertion_mode, token)?
            } else {
                self.foreign(token)?
            };
            if let Step::Consumed = step {
                return Ok(());
            }
        }
    }

    fn html_rules(&self, token: Tok) -> bool {
        let Some(current) = self.adjusted_current() else {
            return true;
        };
        match self.ns(current) {
            Ns::Html => true,
            _ if matches!(token, Tok::Eof) => true,
            ns => {
                let start = |name: &str| matches!(token, Tok::Start(tag) if tag.name() == name);
                let text_point = ns == Ns::MathMl
                    && matches!(self.local(current), "mi" | "mo" | "mn" | "ms" | "mtext");
                (text_point
                    && (matches!(token, Tok::Text(..))
                        || matches!(token, Tok::Start(tag) if !matches!(tag.name(), "mglyph" | "malignmark"))))
                    || (ns == Ns::MathMl && self.local(current) == "annotation-xml" && start("svg"))
                    || (self.html_integration_point(current)
                        && matches!(token, Tok::Start(_) | Tok::Text(..)))
            }
        }
    }

    fn step(&mut self, mode: InsertionMode, token: Tok) -> Result<Step, ParseError> {
        match mode {
            InsertionMode::Initial => self.initial(token),
            InsertionMode::BeforeHtml => self.before_html(token),
            InsertionMode::BeforeHead => self.before_head(token),
            InsertionMode::InHead => self.in_head(token),
            InsertionMode::InHeadNoscript => self.in_head_noscript(token),
            InsertionMode::AfterHead => self.after_head(token),
            InsertionMode::InBody => self.in_body(token),
            InsertionMode::Text => self.text(token),
            InsertionMode::InTable => self.in_table(token),
            InsertionMode::InTableText => self.in_table_text(token),
            InsertionMode::InCaption => self.in_caption(token),
            InsertionMode::InColumnGroup => self.in_column_group(token),
            InsertionMode::InTableBody => self.in_table_body(token),
            InsertionMode::InRow => self.in_row(token),
            InsertionMode::InCell => self.in_cell(token),
            InsertionMode::InTemplate => self.in_template(token),
            InsertionMode::AfterBody => self.after_body(token),
            InsertionMode::InFrameset => self.in_frameset(token),
            InsertionMode::AfterFrameset => self.after_frameset(token),
            InsertionMode::AfterAfterBody => self.after_after_body(token),
            InsertionMode::AfterAfterFrameset => self.after_after_frameset(token),
        }
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
        self.attach_or_destroy(self.document.root(), None, id)?;
        self.document_mode = super::mode::classify(token);
        Ok(())
    }

    fn current(&self) -> NodeId {
        *self
            .open
            .last()
            .expect("insertion mode requires a current element")
    }

    fn element(&self, id: NodeId) -> &Element {
        match self.document.node(id).expect("live parser node").kind() {
            NodeKind::Element(element) => element,
            _ => unreachable!("open stack and formatting list only contain elements"),
        }
    }

    fn ns(&self, id: NodeId) -> Ns {
        match self.element(id).name().namespace() {
            Some(SVG_NAMESPACE) => Ns::Svg,
            Some(MATHML_NAMESPACE) => Ns::MathMl,
            _ => Ns::Html,
        }
    }

    fn local(&self, id: NodeId) -> &str {
        self.element(id).name().local()
    }

    /// The local name of an HTML element, or `None` for foreign elements.
    fn html_name(&self, id: NodeId) -> Option<&str> {
        (self.ns(id) == Ns::Html).then(|| self.local(id))
    }

    fn is_html(&self, id: NodeId, name: &str) -> bool {
        self.html_name(id) == Some(name)
    }

    fn html_integration_point(&self, id: NodeId) -> bool {
        match self.ns(id) {
            Ns::Svg => matches!(self.local(id), "foreignObject" | "desc" | "title"),
            Ns::MathMl if self.local(id) == "annotation-xml" => {
                self.element(id).attributes().iter().any(|attribute| {
                    attribute.name().namespace().is_none()
                        && attribute.name().local() == "encoding"
                        && String::from_utf16(attribute.value().as_units()).is_ok_and(|value| {
                            value.eq_ignore_ascii_case("text/html")
                                || value.eq_ignore_ascii_case("application/xhtml+xml")
                        })
                })
            }
            _ => false,
        }
    }

    fn diagnostic(&mut self, kind: ErrorKind) -> Result<(), ParseError> {
        if self.diagnostics.len() >= self.max_diagnostics {
            // Recovery continues; only the retained list is bounded (TD-01).
            self.dropped_diagnostics = self.dropped_diagnostics.saturating_add(1);
            return Ok(());
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

    /// Diagnose and ignore each NUL character of a run.
    fn ignore_nulls(&mut self, run: &str) -> Result<Step, ParseError> {
        for _ in run.chars() {
            self.diagnostic(ErrorKind::UnexpectedNull)?;
        }
        Ok(Step::Consumed)
    }

    /// Record a misplaced/ignored token recovery and consume the token.
    fn ignore(&mut self) -> Result<Step, ParseError> {
        self.diagnostic(ErrorKind::UnexpectedToken)?;
        Ok(Step::Consumed)
    }
}
