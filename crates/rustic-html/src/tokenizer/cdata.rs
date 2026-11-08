use super::{DiagnosticKind, HtmlError, HtmlTokenizer, State, Token};

/// CDATA section states, entered only when the tree builder allows CDATA.
#[derive(Clone, Copy, Debug)]
pub(super) enum CdataState {
    Section,
    Bracket,
    End,
}

impl HtmlTokenizer {
    /// Allow `<![CDATA[` sections, as required when the tree builder's adjusted
    /// current node is a non-HTML element. Otherwise they recover as comments.
    ///
    /// The setting applies to markup declarations processed after the call.
    pub fn set_cdata_allowed(&mut self, allowed: bool) {
        self.cdata_allowed = allowed;
    }

    pub(super) fn cdata_step(
        &mut self,
        state: CdataState,
        value: char,
        output: &mut Vec<Token>,
    ) -> Result<bool, HtmlError> {
        match (state, value) {
            (CdataState::Section, ']') => self.state = State::Cdata(CdataState::Bracket),
            (CdataState::Section, _) => self.text_char(value, output)?,
            (CdataState::Bracket, ']') => self.state = State::Cdata(CdataState::End),
            (CdataState::Bracket, _) => {
                self.text_char(']', output)?;
                self.state = State::Cdata(CdataState::Section);
                return Ok(true);
            }
            (CdataState::End, ']') => self.text_char(']', output)?,
            (CdataState::End, '>') => self.state = State::Data,
            (CdataState::End, _) => {
                self.text_char(']', output)?;
                self.text_char(']', output)?;
                self.state = State::Cdata(CdataState::Section);
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn finish_cdata(
        &mut self,
        state: CdataState,
        output: &mut Vec<Token>,
    ) -> Result<(), HtmlError> {
        self.diagnostic(DiagnosticKind::EofInCdata)?;
        match state {
            CdataState::Section => {}
            CdataState::Bracket => self.text_char(']', output)?,
            CdataState::End => {
                self.text_char(']', output)?;
                self.text_char(']', output)?;
            }
        }
        Ok(())
    }
}
