use super::{DiagnosticKind, HtmlError, HtmlTokenizer, ReferenceContext, State, Token, whitespace};

/// Initial or tree-builder-selected HTML tokenizer context.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TokenizerMode {
    /// Ordinary HTML data; the default for new tokenizers.
    #[default]
    Data,
    /// Character references and appropriate end tags, as used for title/textarea.
    Rcdata,
    /// Literal text except appropriate end tags, as used for style.
    Rawtext,
    /// HTML script-data escaping/end-tag rules; this does not execute JavaScript.
    ScriptData,
    /// Literal text through EOF, with null replacement and no end-tag recognition.
    Plaintext,
}

impl TokenizerMode {
    pub(super) fn state(self) -> State {
        match self {
            Self::Data => State::Data,
            Self::Rcdata => State::Text(TextState::Rcdata),
            Self::Rawtext => State::Text(TextState::Rawtext),
            Self::ScriptData => State::Text(TextState::Script),
            Self::Plaintext => State::Text(TextState::Plaintext),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TextKind {
    Rcdata,
    Rawtext,
    Script,
    EscapedScript,
}

impl TextKind {
    fn state(self) -> TextState {
        match self {
            Self::Rcdata => TextState::Rcdata,
            Self::Rawtext => TextState::Rawtext,
            Self::Script => TextState::Script,
            Self::EscapedScript => TextState::Escaped,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum TextState {
    Rcdata,
    Rawtext,
    Plaintext,
    Script,
    LessThan(TextKind),
    EndTagOpen(TextKind),
    EndTagName(TextKind),
    EscapeStart,
    EscapeStartDash,
    Escaped,
    EscapedDash,
    EscapedDashDash,
    DoubleEscapeStart,
    DoubleEscaped,
    DoubleEscapedDash,
    DoubleEscapedDashDash,
    DoubleEscapedLessThan,
    DoubleEscapeEnd,
}

impl HtmlTokenizer {
    pub(super) fn text_step(
        &mut self,
        state: TextState,
        value: char,
        output: &mut Vec<Token>,
    ) -> Result<bool, HtmlError> {
        use TextState::*;
        let next = match state {
            Rcdata | Rawtext | Script => {
                if value == '<' {
                    let kind = match state {
                        Rcdata => TextKind::Rcdata,
                        Rawtext => TextKind::Rawtext,
                        _ => TextKind::Script,
                    };
                    LessThan(kind)
                } else if value == '&' && matches!(state, Rcdata) {
                    self.start_reference(ReferenceContext::Rcdata);
                    return Ok(false);
                } else {
                    self.text_value(value, output)?;
                    state
                }
            }
            Plaintext => {
                self.text_value(value, output)?;
                Plaintext
            }
            LessThan(kind) => {
                if value == '/' {
                    self.text_probe.clear();
                    EndTagOpen(kind)
                } else if kind == TextKind::Script && value == '!' {
                    self.text_literal("<!", output)?;
                    EscapeStart
                } else if kind == TextKind::EscapedScript && value.is_ascii_alphabetic() {
                    self.reset_script_word();
                    self.text_char('<', output)?;
                    return self.reconsume_text(DoubleEscapeStart);
                } else {
                    self.text_char('<', output)?;
                    return self.reconsume_text(kind.state());
                }
            }
            EndTagOpen(kind) | EndTagName(kind) => {
                if value.is_ascii_alphabetic() && self.extend_end_tag_probe(value) {
                    EndTagName(kind)
                } else if matches!(state, EndTagName(_))
                    && (whitespace(value) || matches!(value, '/' | '>'))
                    && self
                        .last_start_tag
                        .as_ref()
                        .is_some_and(|name| name.len() == self.text_probe.len())
                {
                    self.accept_text_end_tag(value, output)?;
                    return Ok(false);
                } else {
                    self.fallback_end_tag(output)?;
                    return self.reconsume_text(kind.state());
                }
            }
            EscapeStart => {
                if value == '-' {
                    self.text_char('-', output)?;
                    EscapeStartDash
                } else {
                    return self.reconsume_text(Script);
                }
            }
            EscapeStartDash => {
                if value == '-' {
                    self.text_char('-', output)?;
                    EscapedDashDash
                } else {
                    return self.reconsume_text(Script);
                }
            }
            Escaped | EscapedDash | EscapedDashDash => match value {
                '<' => LessThan(TextKind::EscapedScript),
                '-' => {
                    self.text_char('-', output)?;
                    if matches!(state, Escaped) {
                        EscapedDash
                    } else {
                        EscapedDashDash
                    }
                }
                '>' if matches!(state, EscapedDashDash) => {
                    self.text_char('>', output)?;
                    Script
                }
                _ => {
                    self.text_value(value, output)?;
                    Escaped
                }
            },
            DoubleEscapeStart | DoubleEscapeEnd => {
                if value.is_ascii_alphabetic() {
                    self.script_word_char(value);
                    self.text_char(value, output)?;
                    state
                } else if whitespace(value) || matches!(value, '/' | '>') {
                    let matched = self.text_probe_matches && self.text_probe == "script";
                    self.text_probe.clear();
                    self.text_char(value, output)?;
                    match (matches!(state, DoubleEscapeStart), matched) {
                        (true, true) | (false, false) => DoubleEscaped,
                        _ => Escaped,
                    }
                } else {
                    self.text_probe.clear();
                    return self.reconsume_text(if matches!(state, DoubleEscapeStart) {
                        Escaped
                    } else {
                        DoubleEscaped
                    });
                }
            }
            DoubleEscaped | DoubleEscapedDash | DoubleEscapedDashDash => match value {
                '<' => {
                    self.text_char('<', output)?;
                    DoubleEscapedLessThan
                }
                '-' => {
                    self.text_char('-', output)?;
                    if matches!(state, DoubleEscaped) {
                        DoubleEscapedDash
                    } else {
                        DoubleEscapedDashDash
                    }
                }
                '>' if matches!(state, DoubleEscapedDashDash) => {
                    self.text_char('>', output)?;
                    Script
                }
                _ => {
                    self.text_value(value, output)?;
                    DoubleEscaped
                }
            },
            DoubleEscapedLessThan => {
                if value == '/' {
                    self.reset_script_word();
                    self.text_char('/', output)?;
                    DoubleEscapeEnd
                } else {
                    return self.reconsume_text(DoubleEscaped);
                }
            }
        };
        self.state = State::Text(next);
        Ok(false)
    }

    fn reconsume_text(&mut self, state: TextState) -> Result<bool, HtmlError> {
        self.state = State::Text(state);
        Ok(true)
    }

    fn text_value(&mut self, value: char, output: &mut Vec<Token>) -> Result<(), HtmlError> {
        let value = self.replace_null(value)?;
        self.text_char(value, output)
    }

    fn text_literal(&mut self, text: &str, output: &mut Vec<Token>) -> Result<(), HtmlError> {
        for value in text.chars() {
            self.cancellation
                .check()
                .map_err(|_| HtmlError::Cancelled)?;
            self.text_char(value, output)?;
        }
        Ok(())
    }

    fn extend_end_tag_probe(&mut self, value: char) -> bool {
        // Once a candidate cannot match, emitting it as literal text early has
        // identical semantics and prevents an unbounded alphabetic name buffer.
        // Every retained byte matches the last start tag after ASCII folding,
        // so the delimiter test only needs to compare their lengths.
        let expected = self
            .last_start_tag
            .as_ref()
            .and_then(|name| name.as_bytes().get(self.text_probe.len()));
        if expected.copied() == Some(value.to_ascii_lowercase() as u8) {
            self.text_probe.push(value);
            true
        } else {
            false
        }
    }

    fn fallback_end_tag(&mut self, output: &mut Vec<Token>) -> Result<(), HtmlError> {
        self.text_literal("</", output)?;
        let probe = std::mem::take(&mut self.text_probe);
        self.text_literal(&probe, output)
    }

    fn accept_text_end_tag(
        &mut self,
        delimiter: char,
        output: &mut Vec<Token>,
    ) -> Result<(), HtmlError> {
        self.flush_text(output)?;
        let probe = std::mem::take(&mut self.text_probe);
        let mut chars = probe.chars();
        self.start_tag(true, chars.next().expect("matched nonempty end tag"))?;
        for value in chars {
            self.cancellation
                .check()
                .map_err(|_| HtmlError::Cancelled)?;
            self.charge_token(value)?;
            self.building().tag.name.push(value.to_ascii_lowercase());
        }
        if delimiter == '>' {
            self.emit_tag(output)?;
        } else if delimiter == '/' {
            self.state = State::SelfClosing;
        } else {
            self.state = State::BeforeAttribute;
        }
        Ok(())
    }

    fn reset_script_word(&mut self) {
        self.text_probe.clear();
        self.text_probe_matches = true;
    }

    fn script_word_char(&mut self, value: char) {
        if self.text_probe_matches
            && b"script".get(self.text_probe.len()).copied()
                == Some(value.to_ascii_lowercase() as u8)
        {
            self.text_probe.push(value.to_ascii_lowercase());
        } else {
            self.text_probe_matches = false;
        }
    }

    pub(super) fn finish_text(
        &mut self,
        state: TextState,
        output: &mut Vec<Token>,
    ) -> Result<(), HtmlError> {
        use TextState::*;
        let escaped = match state {
            LessThan(kind) => {
                self.text_char('<', output)?;
                kind == TextKind::EscapedScript
            }
            EndTagOpen(kind) | EndTagName(kind) => {
                self.fallback_end_tag(output)?;
                kind == TextKind::EscapedScript
            }
            Rcdata | Rawtext | Script | Plaintext | EscapeStart | EscapeStartDash => false,
            _ => true,
        };
        if escaped {
            self.diagnostic(DiagnosticKind::EofInScriptHtmlCommentLikeText)?;
        }
        self.text_probe.clear();
        Ok(())
    }
}
