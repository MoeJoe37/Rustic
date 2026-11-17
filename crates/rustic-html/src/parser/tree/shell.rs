use super::{
    Chars, DocumentMode, ErrorKind, InsertionMode, ParseError, Scripting, Step, Tag, Tok,
    TokenizerMode, TreeBuilder,
};

impl TreeBuilder {
    pub(super) fn initial(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Text(Chars::Space, _) => {}
            Tok::Annotation(annotation) => {
                self.insert_annotation(Some(self.document.root()), annotation)?
            }
            Tok::Doctype(doctype) => {
                self.doctype(doctype)?;
                self.insertion_mode = InsertionMode::BeforeHtml;
            }
            _ => {
                self.diagnostic(ErrorKind::MissingDoctype)?;
                self.document_mode = DocumentMode::Quirks;
                self.insertion_mode = InsertionMode::BeforeHtml;
                return Ok(Step::Reprocess);
            }
        }
        Ok(Step::Consumed)
    }

    pub(super) fn before_html(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Doctype(_) => return self.ignore(),
            Tok::Annotation(annotation) => {
                self.insert_annotation(Some(self.document.root()), annotation)?
            }
            Tok::Text(Chars::Space, _) => {}
            Tok::Start(tag) if tag.name() == "html" => {
                self.insert_html(tag)?;
                self.insertion_mode = InsertionMode::BeforeHead;
            }
            Tok::End(name) if !matches!(name, "head" | "body" | "html" | "br") => {
                return self.ignore();
            }
            _ => {
                self.insert_implied("html")?;
                self.insertion_mode = InsertionMode::BeforeHead;
                return Ok(Step::Reprocess);
            }
        }
        Ok(Step::Consumed)
    }

    pub(super) fn before_head(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Text(Chars::Space, _) => {}
            Tok::Annotation(annotation) => self.insert_annotation(None, annotation)?,
            Tok::Doctype(_) => return self.ignore(),
            Tok::Start(tag) if tag.name() == "html" => return self.in_body(token),
            Tok::Start(tag) if tag.name() == "head" => {
                self.head = Some(self.insert_html(tag)?);
                self.insertion_mode = InsertionMode::InHead;
            }
            Tok::End(name) if !matches!(name, "head" | "body" | "html" | "br") => {
                return self.ignore();
            }
            _ => {
                self.head = Some(self.insert_implied("head")?);
                self.insertion_mode = InsertionMode::InHead;
                return Ok(Step::Reprocess);
            }
        }
        Ok(Step::Consumed)
    }

    /// Generic raw text and RCDATA element parsing.
    pub(super) fn insert_text_element(
        &mut self,
        tag: &Tag,
        mode: TokenizerMode,
    ) -> Result<(), ParseError> {
        self.insert_html(tag)?;
        self.text_mode = Some(mode);
        self.original_mode = self.insertion_mode;
        self.insertion_mode = InsertionMode::Text;
        Ok(())
    }

    pub(super) fn in_head(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Text(Chars::Space, text) => self.insert_text(text)?,
            Tok::Annotation(annotation) => self.insert_annotation(None, annotation)?,
            Tok::Doctype(_) => return self.ignore(),
            Tok::Start(tag) => match tag.name() {
                "html" => return self.in_body(token),
                "base" | "basefont" | "bgsound" | "link" | "meta" => self.insert_void(tag)?,
                "title" => self.insert_text_element(tag, TokenizerMode::Rcdata)?,
                "noscript" if self.scripting == Scripting::Enabled => {
                    self.insert_text_element(tag, TokenizerMode::Rawtext)?;
                }
                "noscript" => {
                    self.insert_html(tag)?;
                    self.insertion_mode = InsertionMode::InHeadNoscript;
                }
                "noframes" | "style" => self.insert_text_element(tag, TokenizerMode::Rawtext)?,
                "script" => self.insert_text_element(tag, TokenizerMode::ScriptData)?,
                "template" => {
                    self.insert_html(tag)?;
                    self.push_marker()?;
                    self.frameset_ok = false;
                    self.insertion_mode = InsertionMode::InTemplate;
                    self.template_modes
                        .try_reserve(1)
                        .map_err(|_| rustic_dom::DomError::AllocationFailed)?;
                    self.template_modes.push(InsertionMode::InTemplate);
                }
                "head" => return self.ignore(),
                _ => return Ok(self.leave_head()),
            },
            Tok::End("head") => {
                self.open.pop();
                self.insertion_mode = InsertionMode::AfterHead;
            }
            Tok::End("body" | "html" | "br") => return Ok(self.leave_head()),
            Tok::End("template") => {
                if !self.has_open("template") {
                    return self.ignore();
                }
                self.generate_all_implied_end_tags();
                if !self.current_is("template") {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                }
                self.pop_until("template");
                self.clear_formatting_to_marker();
                self.template_modes.pop();
                self.reset_insertion_mode();
            }
            Tok::End(_) => return self.ignore(),
            _ => return Ok(self.leave_head()),
        }
        Ok(Step::Consumed)
    }

    fn leave_head(&mut self) -> Step {
        self.open.pop();
        self.insertion_mode = InsertionMode::AfterHead;
        Step::Reprocess
    }

    pub(super) fn in_head_noscript(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Doctype(_) => return self.ignore(),
            Tok::Start(tag) if tag.name() == "html" => return self.in_body(token),
            Tok::End("noscript") => {
                self.open.pop();
                self.insertion_mode = InsertionMode::InHead;
            }
            Tok::Text(Chars::Space, _) | Tok::Annotation(_) => return self.in_head(token),
            Tok::Start(tag)
                if matches!(
                    tag.name(),
                    "basefont" | "bgsound" | "link" | "meta" | "noframes" | "style"
                ) =>
            {
                return self.in_head(token);
            }
            Tok::Start(tag) if matches!(tag.name(), "head" | "noscript") => return self.ignore(),
            Tok::End(name) if name != "br" => return self.ignore(),
            _ => {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
                self.open.pop();
                self.insertion_mode = InsertionMode::InHead;
                return Ok(Step::Reprocess);
            }
        }
        Ok(Step::Consumed)
    }

    pub(super) fn after_head(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Text(Chars::Space, text) => self.insert_text(text)?,
            Tok::Annotation(annotation) => self.insert_annotation(None, annotation)?,
            Tok::Doctype(_) => return self.ignore(),
            Tok::Start(tag) => match tag.name() {
                "html" => return self.in_body(token),
                "body" => {
                    self.insert_html(tag)?;
                    self.frameset_ok = false;
                    self.insertion_mode = InsertionMode::InBody;
                }
                "frameset" => {
                    self.insert_html(tag)?;
                    self.insertion_mode = InsertionMode::InFrameset;
                }
                "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script"
                | "style" | "template" | "title" => {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                    let head = self.head.expect("after-head pointer");
                    self.open
                        .try_reserve(1)
                        .map_err(|_| rustic_dom::DomError::AllocationFailed)?;
                    self.open.push(head);
                    let step = self.in_head(token)?;
                    if let Some(index) = self.open.iter().rposition(|id| *id == head) {
                        self.open.remove(index);
                    }
                    return Ok(step);
                }
                "head" => return self.ignore(),
                _ => return self.imply_body(),
            },
            Tok::End("template") => return self.in_head(token),
            Tok::End("body" | "html" | "br") => return self.imply_body(),
            Tok::End(_) => return self.ignore(),
            _ => return self.imply_body(),
        }
        Ok(Step::Consumed)
    }

    fn imply_body(&mut self) -> Result<Step, ParseError> {
        self.insert_implied("body")?;
        self.insertion_mode = InsertionMode::InBody;
        Ok(Step::Reprocess)
    }

    pub(super) fn text(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Text(_, text) => {
                self.insert_text(text)?;
                Ok(Step::Consumed)
            }
            Tok::Eof => {
                self.diagnostic(ErrorKind::EofInText)?;
                self.open.pop();
                self.insertion_mode = self.original_mode;
                Ok(Step::Reprocess)
            }
            _ => {
                let closed = self.open.pop();
                self.insertion_mode = self.original_mode;
                if matches!(token, Tok::End("script"))
                    && self.scripting == Scripting::Enabled
                    && !self.fragment()
                    && self.template_modes.is_empty()
                {
                    self.script_boundary = closed;
                }
                Ok(Step::Consumed)
            }
        }
    }

    pub(super) fn after_body(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Text(Chars::Space, _) => return self.in_body(token),
            Tok::Annotation(annotation) => {
                self.insert_annotation(Some(self.open[0]), annotation)?
            }
            Tok::Doctype(_) => return self.ignore(),
            Tok::Start(tag) if tag.name() == "html" => return self.in_body(token),
            Tok::End("html") if self.fragment() => return self.ignore(),
            Tok::End("html") => self.insertion_mode = InsertionMode::AfterAfterBody,
            Tok::Eof => self.done = true,
            _ => {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
                self.insertion_mode = InsertionMode::InBody;
                return Ok(Step::Reprocess);
            }
        }
        Ok(Step::Consumed)
    }

    pub(super) fn in_frameset(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Text(Chars::Space, text) => self.insert_text(text)?,
            Tok::Annotation(annotation) => self.insert_annotation(None, annotation)?,
            Tok::Start(tag) => match tag.name() {
                "html" => return self.in_body(token),
                "frameset" => {
                    self.insert_html(tag)?;
                }
                "frame" => self.insert_void(tag)?,
                "noframes" => return self.in_head(token),
                _ => return self.ignore(),
            },
            Tok::End("frameset") => {
                if self.open.len() == 1 {
                    return self.ignore();
                }
                self.open.pop();
                if !self.fragment() && !self.current_is("frameset") {
                    self.insertion_mode = InsertionMode::AfterFrameset;
                }
            }
            Tok::Eof => {
                if self.open.len() != 1 {
                    self.diagnostic(ErrorKind::UnclosedElements)?;
                }
                self.done = true;
            }
            _ => return self.ignore(),
        }
        Ok(Step::Consumed)
    }

    pub(super) fn after_frameset(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Text(Chars::Space, text) => self.insert_text(text)?,
            Tok::Annotation(annotation) => self.insert_annotation(None, annotation)?,
            Tok::Start(tag) if tag.name() == "html" => return self.in_body(token),
            Tok::Start(tag) if tag.name() == "noframes" => return self.in_head(token),
            Tok::End("html") => self.insertion_mode = InsertionMode::AfterAfterFrameset,
            Tok::Eof => self.done = true,
            _ => return self.ignore(),
        }
        Ok(Step::Consumed)
    }

    pub(super) fn after_after_body(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Annotation(annotation) => {
                self.insert_annotation(Some(self.document.root()), annotation)?
            }
            Tok::Doctype(_) | Tok::Text(Chars::Space, _) => return self.in_body(token),
            Tok::Start(tag) if tag.name() == "html" => return self.in_body(token),
            Tok::Eof => self.done = true,
            _ => {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
                self.insertion_mode = InsertionMode::InBody;
                return Ok(Step::Reprocess);
            }
        }
        Ok(Step::Consumed)
    }

    pub(super) fn after_after_frameset(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Annotation(annotation) => {
                self.insert_annotation(Some(self.document.root()), annotation)?
            }
            Tok::Doctype(_) | Tok::Text(Chars::Space, _) => return self.in_body(token),
            Tok::Start(tag) if tag.name() == "html" => return self.in_body(token),
            Tok::Start(tag) if tag.name() == "noframes" => return self.in_head(token),
            Tok::Eof => self.done = true,
            _ => return self.ignore(),
        }
        Ok(Step::Consumed)
    }
}
