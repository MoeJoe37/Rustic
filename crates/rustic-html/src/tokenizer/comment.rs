use rustic_base::Budget;

use super::{DiagnosticKind, HtmlError, HtmlTokenizer, State, Token};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CommentState {
    Start,
    StartDash,
    Data,
    LessThan,
    LessThanBang,
    LessThanBangDash,
    LessThanBangDashDash,
    EndDash,
    End,
    EndBang,
    Bogus,
}

impl HtmlTokenizer {
    pub(super) fn start_comment(&mut self, state: CommentState) {
        self.comment.clear();
        self.prefix.clear();
        self.token_bytes = Budget::new(self.limits.max_token_bytes);
        self.state = State::Comment(state);
    }

    pub(super) fn comment_step(
        &mut self,
        state: CommentState,
        value: char,
        output: &mut Vec<Token>,
    ) -> Result<bool, HtmlError> {
        use CommentState::*;
        let next = match (state, value) {
            (Start, '-') => StartDash,
            (Start | StartDash, '>') => {
                self.diagnostic(DiagnosticKind::AbruptClosingOfEmptyComment)?;
                self.emit_comment(output)?;
                return Ok(false);
            }
            (Start, _) => return self.reconsume_comment(Data),
            (StartDash, '-') => End,
            (StartDash, _) => {
                self.append_comment('-')?;
                return self.reconsume_comment(Data);
            }
            (Data, '<') => {
                self.append_comment('<')?;
                LessThan
            }
            (Data, '-') => EndDash,
            (Data, _) => {
                self.append_comment(value)?;
                Data
            }
            (LessThan, '!') => {
                self.append_comment('!')?;
                LessThanBang
            }
            (LessThan, '<') => {
                self.append_comment('<')?;
                LessThan
            }
            (LessThanBang, '-') => LessThanBangDash,
            (LessThan | LessThanBang, _) => return self.reconsume_comment(Data),
            (LessThanBangDash, '-') => LessThanBangDashDash,
            (LessThanBangDash, _) => return self.reconsume_comment(EndDash),
            (LessThanBangDashDash, _) => {
                if value != '>' {
                    self.diagnostic(DiagnosticKind::NestedComment)?;
                }
                return self.reconsume_comment(End);
            }
            (EndDash, '-') => End,
            (EndDash, _) => {
                self.append_comment('-')?;
                return self.reconsume_comment(Data);
            }
            (End | Bogus, '>') => {
                self.emit_comment(output)?;
                return Ok(false);
            }
            (End, '!') => EndBang,
            (End, '-') => {
                self.append_comment('-')?;
                End
            }
            (End, _) => {
                self.append_comment_text("--")?;
                return self.reconsume_comment(Data);
            }
            (EndBang, '-') => {
                self.append_comment_text("--!")?;
                EndDash
            }
            (EndBang, '>') => {
                self.diagnostic(DiagnosticKind::IncorrectlyClosedComment)?;
                self.emit_comment(output)?;
                return Ok(false);
            }
            (EndBang, _) => {
                self.append_comment_text("--!")?;
                return self.reconsume_comment(Data);
            }
            (Bogus, _) => {
                self.append_comment(value)?;
                Bogus
            }
        };
        self.state = State::Comment(next);
        Ok(false)
    }

    fn reconsume_comment(&mut self, state: CommentState) -> Result<bool, HtmlError> {
        self.state = State::Comment(state);
        Ok(true)
    }

    pub(super) fn append_comment(&mut self, value: char) -> Result<(), HtmlError> {
        let value = self.replace_null(value)?;
        self.charge_token(value)?;
        self.comment.push(value);
        Ok(())
    }

    pub(super) fn append_comment_text(&mut self, text: &str) -> Result<(), HtmlError> {
        for value in text.chars() {
            self.append_comment(value)?;
        }
        Ok(())
    }

    pub(super) fn emit_comment(&mut self, output: &mut Vec<Token>) -> Result<(), HtmlError> {
        let data = std::mem::take(&mut self.comment);
        self.state = State::Data;
        self.emit(Token::Comment(data), output)
    }

    pub(super) fn finish_comment(
        &mut self,
        state: CommentState,
        output: &mut Vec<Token>,
    ) -> Result<(), HtmlError> {
        // Pending delimiter hyphens/bang are not data on EOF in these states.
        // Less-than/bang characters that are data were appended when consumed.
        if state != CommentState::Bogus {
            self.diagnostic(DiagnosticKind::EofInComment)?;
        }
        self.emit_comment(output)
    }
}
