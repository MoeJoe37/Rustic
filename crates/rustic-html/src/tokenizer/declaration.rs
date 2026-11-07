use super::{CommentState, DiagnosticKind, HtmlError, HtmlTokenizer};

impl HtmlTokenizer {
    pub(super) fn declaration_step(&mut self, value: char) -> Result<bool, HtmlError> {
        // Delay decisions until a keyword matches or cannot match. Only a
        // possible ASCII prefix persists across feeds (at most six bytes).
        let mut candidate = self.prefix.clone();
        candidate.push(value);
        if candidate == "--" {
            self.start_comment(CommentState::Start);
        } else if candidate.eq_ignore_ascii_case("DOCTYPE") {
            self.start_doctype();
        } else if candidate == "[CDATA[" {
            self.diagnostic(DiagnosticKind::CdataInHtmlContent)?;
            // The current tokenizer always has HTML context. Foreign-content
            // CDATA needs the future tree builder's adjusted-current-node input.
            self.start_comment(CommentState::Bogus);
            self.append_comment_text("[CDATA[")?;
        } else if "--".starts_with(&candidate)
            || "[CDATA[".starts_with(&candidate)
            || ascii_keyword_prefix(&candidate, "DOCTYPE")
        {
            self.prefix = candidate;
        } else {
            self.diagnostic_at(
                DiagnosticKind::IncorrectlyOpenedComment,
                self.normalized_offset - self.prefix.len(),
            )?;
            let prefix = std::mem::take(&mut self.prefix);
            self.start_comment(CommentState::Bogus);
            self.append_comment_text(&prefix)?;
            // The mismatch may be a closing '>' or null; let bogus-comment
            // processing handle it rather than appending it as ordinary data.
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) fn finish_declaration(
        &mut self,
        output: &mut Vec<super::Token>,
    ) -> Result<(), HtmlError> {
        self.diagnostic_at(
            DiagnosticKind::IncorrectlyOpenedComment,
            self.normalized_offset - self.prefix.len(),
        )?;
        let prefix = std::mem::take(&mut self.prefix);
        self.start_comment(CommentState::Bogus);
        self.append_comment_text(&prefix)?;
        self.emit_comment(output)
    }
}

pub(super) fn ascii_keyword_prefix(candidate: &str, keyword: &str) -> bool {
    keyword
        .get(..candidate.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(candidate))
}
