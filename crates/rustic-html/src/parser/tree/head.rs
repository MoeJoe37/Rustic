use super::{
    ErrorKind, InsertionMode, NodeId, ParseError, Step, Tag, Token, TokenizerMode, TreeBuilder,
    UnsupportedFeature,
};

fn head_start(name: &str) -> bool {
    matches!(
        name,
        "base"
            | "basefont"
            | "bgsound"
            | "link"
            | "meta"
            | "noframes"
            | "script"
            | "style"
            | "template"
            | "title"
    )
}

impl TreeBuilder {
    pub(super) fn html_tag(&mut self, tag: &Tag) -> Result<(), ParseError> {
        self.diagnostic(ErrorKind::UnexpectedToken)?;
        self.add_missing_attributes(self.open[0], tag)
    }

    pub(super) fn in_head(
        &mut self,
        token: &Token,
        parent: NodeId,
        return_mode: InsertionMode,
    ) -> Result<Step, ParseError> {
        match token {
            Token::Comment(text) => self.comment(parent, text)?,
            Token::ProcessingInstruction(_) => {
                return Err(ParseError::Unsupported(
                    UnsupportedFeature::ProcessingInstructions,
                ));
            }
            Token::Doctype(_) => self.diagnostic(ErrorKind::UnexpectedToken)?,
            Token::StartTag(tag) => match tag.name() {
                "html" => self.html_tag(tag)?,
                "base" | "basefont" | "bgsound" | "link" | "meta" => {
                    self.insert(parent, tag, false)?;
                }
                "title" | "style" | "noframes" | "script" => {
                    self.insert(parent, tag, true)?;
                    let mode = match tag.name() {
                        "title" => TokenizerMode::Rcdata,
                        "script" => TokenizerMode::ScriptData,
                        _ => TokenizerMode::Rawtext,
                    };
                    self.original_mode = return_mode;
                    self.insertion_mode = InsertionMode::Text;
                    return Ok(Step::TextMode(mode));
                }
                "noscript" => {
                    // This inert parser always uses the scripting-disabled rules.
                    self.insert(parent, tag, true)?;
                    self.insertion_mode = InsertionMode::InHeadNoscript;
                }
                "template" => return Err(ParseError::Unsupported(UnsupportedFeature::Templates)),
                "head" => self.diagnostic(ErrorKind::UnexpectedToken)?,
                _ => return Ok(self.leave_head()),
            },
            Token::EndTag(name) if name == "head" => {
                self.open.pop();
                self.insertion_mode = InsertionMode::AfterHead;
            }
            Token::EndTag(name) if !matches!(name.as_str(), "body" | "html" | "br") => {
                self.diagnostic(ErrorKind::UnexpectedToken)?
            }
            _ => return Ok(self.leave_head()),
        }
        Ok(Step::Consumed)
    }

    pub(super) fn leave_head(&mut self) -> Step {
        self.open.pop();
        self.insertion_mode = InsertionMode::AfterHead;
        Step::Reprocess
    }

    pub(super) fn in_head_noscript(&mut self, token: &Token) -> Result<Step, ParseError> {
        match token {
            Token::EndTag(name) if name == "noscript" => {
                self.open.pop();
                self.insertion_mode = InsertionMode::InHead;
            }
            Token::Doctype(_) | Token::Comment(_) | Token::ProcessingInstruction(_) => {
                return self.in_head(token, self.current(), InsertionMode::InHeadNoscript);
            }
            Token::StartTag(tag)
                if matches!(
                    tag.name(),
                    "html" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "style"
                ) =>
            {
                return self.in_head(token, self.current(), InsertionMode::InHeadNoscript);
            }
            Token::StartTag(tag) if matches!(tag.name(), "head" | "noscript") => {
                self.diagnostic(ErrorKind::UnexpectedToken)?
            }
            Token::EndTag(name) if name != "br" => self.diagnostic(ErrorKind::UnexpectedToken)?,
            _ => {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
                self.open.pop();
                self.insertion_mode = InsertionMode::InHead;
                return Ok(Step::Reprocess);
            }
        }
        Ok(Step::Consumed)
    }

    pub(super) fn after_head(&mut self, token: &Token) -> Result<Step, ParseError> {
        match token {
            Token::Comment(text) => self.comment(self.current(), text)?,
            Token::ProcessingInstruction(_) => {
                return Err(ParseError::Unsupported(
                    UnsupportedFeature::ProcessingInstructions,
                ));
            }
            Token::Doctype(_) => self.diagnostic(ErrorKind::UnexpectedToken)?,
            Token::StartTag(tag) if tag.name() == "html" => self.html_tag(tag)?,
            Token::StartTag(tag) if tag.name() == "body" => {
                self.insert(self.current(), tag, true)?;
                self.insertion_mode = InsertionMode::InBody;
            }
            Token::StartTag(tag) if tag.name() == "frameset" => {
                return Err(ParseError::Unsupported(UnsupportedFeature::Framesets));
            }
            Token::StartTag(tag) if head_start(tag.name()) => {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
                // The standard's temporary head stack entry is removed before
                // the next token. Passing its checked parent gives the same
                // retained stack, including a newly opened text element.
                return self.in_head(
                    token,
                    self.head.expect("after-head pointer"),
                    InsertionMode::AfterHead,
                );
            }
            Token::StartTag(tag) if tag.name() == "head" => {
                self.diagnostic(ErrorKind::UnexpectedToken)?
            }
            Token::EndTag(name) if !matches!(name.as_str(), "body" | "html" | "br") => {
                self.diagnostic(ErrorKind::UnexpectedToken)?
            }
            _ => {
                self.imply_body()?;
                return Ok(Step::Reprocess);
            }
        }
        Ok(Step::Consumed)
    }

    pub(super) fn text_end(&mut self, token: &Token) -> Result<Step, ParseError> {
        let eof = matches!(token, Token::Eof);
        if eof {
            self.diagnostic(ErrorKind::EofInText)?;
        } else {
            assert!(
                matches!(token, Token::EndTag(_)),
                "text tokenizer emits characters/end/EOF"
            );
        }
        self.open.pop();
        self.insertion_mode = self.original_mode;
        Ok(if eof { Step::Reprocess } else { Step::Consumed })
    }
}
