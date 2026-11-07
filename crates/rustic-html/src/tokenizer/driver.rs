use super::{
    DiagnosticKind, HtmlError, HtmlTokenizer, State, Token, TokenizerLimits, TokenizerMode,
};
use rustic_base::CancellationToken;

/// UTF-8 storage with amortized compaction; boundaries are always scalar-aligned.
#[derive(Debug, Default)]
pub(super) struct DecodedInput {
    text: String,
    offset: usize,
}

impl DecodedInput {
    fn append(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        if self.text.is_empty() {
            self.text = text;
            return;
        }
        if self.offset > 0 && self.offset >= self.text.len() / 2 {
            self.text.drain(..self.offset);
            self.offset = 0;
        }
        self.text.push_str(&text);
    }

    fn next(&mut self) -> Option<char> {
        let value = self.text[self.offset..].chars().next()?;
        self.offset += value.len_utf8(); // Bounded by the existing string length.
        if self.offset == self.text.len() {
            self.text.clear();
            self.offset = 0;
        }
        Some(value)
    }
}

impl HtmlTokenizer {
    /// Start in an explicit tokenizer context, for fragments and conformance tests.
    ///
    /// `last_start_tag` is ASCII-lowercased and bounded by `max_token_bytes`.
    /// Missing context never matches an end tag. This does not build a fragment
    /// or infer namespaces, scripting flags or insertion modes from tag names.
    pub fn with_context(
        limits: TokenizerLimits,
        cancellation: CancellationToken,
        mode: TokenizerMode,
        last_start_tag: Option<&str>,
    ) -> Result<Self, HtmlError> {
        let mut tokenizer = Self::new(limits, cancellation)?;
        if let Some(name) = last_start_tag {
            if name.len() > limits.max_token_bytes {
                return Err(HtmlError::TokenBytesLimit);
            }
            tokenizer.last_start_tag = Some(name.to_ascii_lowercase());
        }
        tokenizer.state = mode.state();
        Ok(tokenizer)
    }

    /// Queue original bytes without tokenizing ahead of the next token boundary.
    ///
    /// The existing cumulative raw-input budget also bounds queued decoded input.
    /// Empty writes still check cancellation. A fatal error poisons the stream.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), HtmlError> {
        self.ensure_writable()?;
        let result = self
            .input
            .feed(bytes)
            .map(|decoded| self.decoded.append(decoded));
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Close input and queue any incomplete UTF-8 replacement, without parsing ahead.
    ///
    /// Read tokens through [`Self::next_token`] until the single EOF token arrives.
    /// Another write/end/finish is rejected; unread input remains available.
    pub fn end(&mut self) -> Result<(), HtmlError> {
        self.ensure_writable()?;
        let result = self
            .input
            .finish()
            .map(|decoded| self.decoded.append(decoded));
        if result.is_ok() {
            self.input_closed = true;
        } else {
            self.poisoned = true;
        }
        result
    }

    /// Read one token, pausing before any later input scalar is processed.
    ///
    /// `None` means more input is needed, or EOF was already delivered. Consuming
    /// callers distinguish completion by the EOF token. A partially retained text
    /// chunk may wait for markup/chunk limits/EOF. Errors terminate the stream.
    pub fn next_token(&mut self) -> Result<Option<Token>, HtmlError> {
        if self.poisoned {
            return Err(HtmlError::Poisoned);
        }
        if self.finished && self.pending.is_empty() {
            return Ok(None);
        }
        let result = (|| {
            self.cancellation
                .check()
                .map_err(|_| HtmlError::Cancelled)?;
            self.read_token()
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Select a mode immediately after receiving a start tag from the pull driver.
    ///
    /// The opportunity ends when another input scalar/token is processed. An
    /// invalid control call preserves parser state and does not poison the stream.
    /// The future tree builder owns this decision; tag names alone are insufficient.
    pub fn set_text_mode(&mut self, mode: TokenizerMode) -> Result<(), HtmlError> {
        self.ensure_active()?;
        if !self.mode_opportunity || !matches!(self.state, State::Data) || !self.pending.is_empty()
        {
            return Err(HtmlError::InvalidModeChange);
        }
        self.state = mode.state();
        self.mode_opportunity = false;
        Ok(())
    }

    /// Queue bytes and collect currently available tokens in the current context.
    ///
    /// For tree-builder mode feedback use write/next_token/set_text_mode instead.
    /// Earlier successful results are final; a failing call returns no partial vector.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Token>, HtmlError> {
        self.write(bytes)?;
        self.collect_tokens()
    }

    /// Close input and collect all remaining tokens, including exactly one EOF.
    pub fn finish(&mut self) -> Result<Vec<Token>, HtmlError> {
        self.end()?;
        self.collect_tokens()
    }

    fn ensure_writable(&self) -> Result<(), HtmlError> {
        self.ensure_active()?;
        if self.input_closed {
            return Err(HtmlError::AlreadyFinished);
        }
        Ok(())
    }

    fn collect_tokens(&mut self) -> Result<Vec<Token>, HtmlError> {
        let mut tokens = Vec::new();
        while let Some(token) = self.next_token()? {
            tokens.push(token);
        }
        Ok(tokens)
    }

    fn deliver_token(&mut self) -> Option<Token> {
        let token = self.pending.pop_front()?;
        self.mode_opportunity = matches!(token, Token::StartTag(_));
        if let Token::StartTag(tag) = &token {
            self.last_start_tag = Some(tag.name.clone());
        }
        Some(token)
    }

    fn read_token(&mut self) -> Result<Option<Token>, HtmlError> {
        if !self.pending.is_empty() {
            return Ok(self.deliver_token());
        }
        while let Some(value) = self.decoded.next() {
            self.mode_opportunity = false;
            self.cancellation
                .check()
                .map_err(|_| HtmlError::Cancelled)?;
            let mut output = Vec::new();
            while self.step(value, &mut output)? {} // Reconsume after state transitions.
            self.normalized_offset = self
                .normalized_offset
                .checked_add(1)
                .ok_or(HtmlError::InputLimit)?;
            self.pending.extend(output);
            if !self.pending.is_empty() {
                return Ok(self.deliver_token());
            }
        }
        if self.input_closed && !self.finished {
            self.mode_opportunity = false;
            let mut output = Vec::new();
            self.finish_states(&mut output)?;
            self.flush_text(&mut output)?;
            self.emit(Token::Eof, &mut output)?;
            self.pending.extend(output);
            self.finished = true;
        }
        Ok(self.deliver_token())
    }

    fn finish_states(&mut self, output: &mut Vec<Token>) -> Result<(), HtmlError> {
        if let State::Reference(state) = self.state {
            self.finish_reference(state, output)?;
        }
        if matches!(self.state, State::AttributeName) {
            self.finish_attribute_name()?;
        }
        match self.state {
            State::Data => {}
            State::TagOpen => {
                self.diagnostic(DiagnosticKind::EofBeforeTagName)?;
                self.text_char('<', output)?;
            }
            State::EndTagOpen => {
                self.diagnostic(DiagnosticKind::EofBeforeTagName)?;
                self.text_char('<', output)?;
                self.text_char('/', output)?;
            }
            State::MarkupDeclaration => self.finish_declaration(output)?,
            State::Comment(state) => self.finish_comment(state, output)?,
            State::Doctype(state) => self.finish_doctype(state, output)?,
            State::Instruction(_) => {
                self.diagnostic(DiagnosticKind::EofInProcessingInstruction)?;
                self.prefix.clear();
                self.instruction = None;
            }
            State::Text(state) => self.finish_text(state, output)?,
            _ => {
                self.diagnostic(DiagnosticKind::EofInTag)?;
                self.tag = None;
            }
        }
        Ok(())
    }
}
