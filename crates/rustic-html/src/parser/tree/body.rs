use super::{
    ErrorKind, InsertionMode, ParseError, Step, Tag, Token, TokenizerMode, TreeBuilder,
    UnsupportedFeature,
};

impl TreeBuilder {
    pub(super) fn in_body(&mut self, token: &Token) -> Result<Step, ParseError> {
        match token {
            Token::Comment(text) => self.comment(self.current(), text)?,
            Token::ProcessingInstruction(_) => {
                return Err(ParseError::Unsupported(
                    UnsupportedFeature::ProcessingInstructions,
                ));
            }
            Token::Doctype(_) => self.diagnostic(ErrorKind::UnexpectedToken)?,
            Token::StartTag(tag) => match tag.name() {
                "html" => self.html_tag(tag)?,
                "body" => {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                    self.add_missing_attributes(self.open[1], tag)?;
                }
                "head" => self.diagnostic(ErrorKind::UnexpectedToken)?,
                "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script"
                | "style" | "title" | "template" => {
                    return self.in_head(token, self.current(), InsertionMode::InBody);
                }
                "frameset" => return Err(ParseError::Unsupported(UnsupportedFeature::Framesets)),
                "p" | "div" => {
                    self.close_existing_p()?;
                    self.insert(self.current(), tag, true)?;
                }
                "span" => {
                    self.insert(self.current(), tag, true)?;
                }
                "br" => {
                    self.insert(self.current(), tag, false)?;
                }
                "hr" => {
                    self.close_existing_p()?;
                    self.insert(self.current(), tag, false)?;
                }
                "plaintext" => {
                    self.close_existing_p()?;
                    self.insert(self.current(), tag, true)?;
                    return Ok(Step::TextMode(TokenizerMode::Plaintext));
                }
                _ => return Err(ParseError::Unsupported(UnsupportedFeature::BodyElement)),
            },
            Token::EndTag(name) => match name.as_str() {
                "body" | "html" => {
                    self.check_unclosed()?;
                    self.insertion_mode = InsertionMode::AfterBody;
                    if name == "html" {
                        return Ok(Step::Reprocess);
                    }
                }
                "p" => {
                    if !self.open.iter().any(|id| self.name(*id) == "p") {
                        self.diagnostic(ErrorKind::UnexpectedToken)?;
                        self.insert_implied(self.current(), "p")?;
                    }
                    self.close_existing_p()?;
                }
                "div" => self.close_div()?,
                "br" => {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                    self.insert(
                        self.current(),
                        &Tag {
                            name: "br".into(),
                            attributes: Vec::new(),
                            self_closing: false,
                        },
                        false,
                    )?;
                }
                "template" => self.diagnostic(ErrorKind::UnexpectedToken)?,
                _ => self.close_generic(name)?,
            },
            Token::Eof => {
                self.check_unclosed()?;
                self.done = true;
            }
            Token::Characters(_) => unreachable!("characters use the batched path"),
        }
        Ok(Step::Consumed)
    }

    // Within this subset, no button/scope boundary can appear above a paragraph.
    // Expand the scope predicate before adding body elements with new boundaries.
    fn close_existing_p(&mut self) -> Result<(), ParseError> {
        if let Some(index) = self.open.iter().rposition(|id| self.name(*id) == "p") {
            if index + 1 != self.open.len() {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
            }
            self.open.truncate(index);
        }
        Ok(())
    }

    fn close_div(&mut self) -> Result<(), ParseError> {
        if let Some(index) = self.open.iter().rposition(|id| self.name(*id) == "div") {
            // The only supported implied-end-tag element is p.
            if self.name(self.current()) == "p" {
                self.open.pop();
            }
            if index + 1 != self.open.len() {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
            }
            self.open.truncate(index);
        } else {
            self.diagnostic(ErrorKind::UnexpectedToken)?;
        }
        Ok(())
    }

    fn close_generic(&mut self, name: &str) -> Result<(), ParseError> {
        for index in (0..self.open.len()).rev() {
            let current = self.name(self.open[index]);
            if current == name {
                if index + 1 != self.open.len() {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                }
                self.open.truncate(index);
                return Ok(());
            }
            if current != "span" {
                break;
            } // Every other open kind here is special.
        }
        self.diagnostic(ErrorKind::UnexpectedToken)
    }

    fn check_unclosed(&mut self) -> Result<(), ParseError> {
        if self
            .open
            .iter()
            .any(|id| !matches!(self.name(*id), "html" | "body" | "p"))
        {
            self.diagnostic(ErrorKind::UnclosedElements)?;
        }
        Ok(())
    }

    pub(super) fn after_body(&mut self, token: &Token) -> Result<Step, ParseError> {
        match token {
            Token::Comment(text) => self.comment(self.open[0], text)?,
            Token::ProcessingInstruction(_) => {
                return Err(ParseError::Unsupported(
                    UnsupportedFeature::ProcessingInstructions,
                ));
            }
            Token::Doctype(_) => self.diagnostic(ErrorKind::UnexpectedToken)?,
            Token::StartTag(tag) if tag.name() == "html" => self.html_tag(tag)?,
            Token::EndTag(name) if name == "html" => {
                self.insertion_mode = InsertionMode::AfterAfterBody
            }
            Token::Eof => self.done = true,
            _ => {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
                self.insertion_mode = InsertionMode::InBody;
                return Ok(Step::Reprocess);
            }
        }
        Ok(Step::Consumed)
    }

    pub(super) fn after_after_body(&mut self, token: &Token) -> Result<Step, ParseError> {
        match token {
            Token::Comment(text) => self.comment(self.document.root(), text)?,
            Token::ProcessingInstruction(_) => {
                return Err(ParseError::Unsupported(
                    UnsupportedFeature::ProcessingInstructions,
                ));
            }
            Token::Doctype(_) => self.diagnostic(ErrorKind::UnexpectedToken)?,
            Token::StartTag(tag) if tag.name() == "html" => self.html_tag(tag)?,
            Token::Eof => self.done = true,
            _ => {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
                self.insertion_mode = InsertionMode::InBody;
                return Ok(Step::Reprocess);
            }
        }
        Ok(Step::Consumed)
    }
}
