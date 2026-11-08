use super::{ErrorKind, InsertionMode, ParseError, TreeBuilder};

fn whitespace(value: char) -> bool {
    matches!(value, '\t' | '\n' | '\u{c}' | '\r' | ' ')
}

impl TreeBuilder {
    pub(super) fn characters(&mut self, mut text: &str) -> Result<(), ParseError> {
        while !text.is_empty() {
            match self.insertion_mode {
                InsertionMode::Initial | InsertionMode::BeforeHtml | InsertionMode::BeforeHead => {
                    text = text.trim_start_matches(whitespace);
                    if text.is_empty() {
                        return Ok(());
                    }
                    match self.insertion_mode {
                        InsertionMode::Initial => self.missing_doctype()?,
                        InsertionMode::BeforeHtml => self.imply_html()?,
                        InsertionMode::BeforeHead => self.imply_head()?,
                        _ => unreachable!(),
                    }
                }
                InsertionMode::InHead
                | InsertionMode::InHeadNoscript
                | InsertionMode::AfterHead
                | InsertionMode::AfterBody
                | InsertionMode::AfterAfterBody => {
                    // Character tokens group scalars: only a leading whitespace
                    // run uses these rules. Reprocess the remainder in the next
                    // mode so a chunk never moves prefix whitespace into body.
                    let rest = text.trim_start_matches(whitespace);
                    let prefix = &text[..text.len() - rest.len()];
                    self.insert_text(self.current(), prefix)?;
                    text = rest;
                    if text.is_empty() {
                        return Ok(());
                    }
                    match self.insertion_mode {
                        InsertionMode::InHead => {
                            self.leave_head();
                        }
                        InsertionMode::InHeadNoscript => {
                            self.diagnostic(ErrorKind::UnexpectedToken)?;
                            self.open.pop();
                            self.insertion_mode = InsertionMode::InHead;
                        }
                        InsertionMode::AfterHead => self.imply_body()?,
                        _ => {
                            self.diagnostic(ErrorKind::UnexpectedToken)?;
                            self.insertion_mode = InsertionMode::InBody;
                        }
                    }
                }
                InsertionMode::InBody => {
                    for (index, run) in text.split('\0').enumerate() {
                        if index != 0 {
                            self.diagnostic(ErrorKind::UnexpectedNull)?;
                        }
                        self.insert_text(self.current(), run)?;
                    }
                    return Ok(());
                }
                InsertionMode::Text => {
                    self.insert_text(self.current(), text)?;
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}
