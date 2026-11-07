use rustic_base::Budget;

use super::declaration::ascii_keyword_prefix;
use super::{DiagnosticKind, Doctype, HtmlError, HtmlTokenizer, State, Token, whitespace};

#[derive(Clone, Copy, Debug)]
pub(super) enum Identifier {
    Public,
    System,
}

impl Identifier {
    fn missing(self) -> DiagnosticKind {
        match self {
            Self::Public => DiagnosticKind::MissingDoctypePublicIdentifier,
            Self::System => DiagnosticKind::MissingDoctypeSystemIdentifier,
        }
    }

    fn missing_quote(self) -> DiagnosticKind {
        match self {
            Self::Public => DiagnosticKind::MissingQuoteBeforeDoctypePublicIdentifier,
            Self::System => DiagnosticKind::MissingQuoteBeforeDoctypeSystemIdentifier,
        }
    }

    fn missing_whitespace(self) -> DiagnosticKind {
        match self {
            Self::Public => DiagnosticKind::MissingWhitespaceAfterDoctypePublicKeyword,
            Self::System => DiagnosticKind::MissingWhitespaceAfterDoctypeSystemKeyword,
        }
    }

    fn abrupt(self) -> DiagnosticKind {
        match self {
            Self::Public => DiagnosticKind::AbruptDoctypePublicIdentifier,
            Self::System => DiagnosticKind::AbruptDoctypeSystemIdentifier,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum DoctypeState {
    Initial,
    BeforeName,
    Name,
    AfterName,
    AfterKeyword(Identifier),
    BeforeIdentifier(Identifier),
    QuotedIdentifier(Identifier, char),
    AfterPublic,
    BetweenIdentifiers,
    AfterSystem,
    Bogus,
}

impl HtmlTokenizer {
    pub(super) fn start_doctype(&mut self) {
        self.prefix.clear();
        self.token_bytes = Budget::new(self.limits.max_token_bytes);
        self.doctype = Some(Doctype {
            name: None,
            public_id: None,
            system_id: None,
            force_quirks: false,
        });
        self.state = State::Doctype(DoctypeState::Initial);
    }

    pub(super) fn doctype_step(
        &mut self,
        state: DoctypeState,
        value: char,
        output: &mut Vec<Token>,
    ) -> Result<bool, HtmlError> {
        use DoctypeState::*;
        let next = match state {
            Initial => {
                if whitespace(value) {
                    BeforeName
                } else {
                    if value != '>' {
                        self.diagnostic(DiagnosticKind::MissingWhitespaceBeforeDoctypeName)?;
                    }
                    return self.reconsume_doctype(BeforeName);
                }
            }
            BeforeName => {
                if whitespace(value) {
                    BeforeName
                } else if value == '>' {
                    self.diagnostic(DiagnosticKind::MissingDoctypeName)?;
                    self.doctype_record().force_quirks = true;
                    self.emit_doctype(output)?;
                    return Ok(false);
                } else {
                    self.push_doctype_name(value)?;
                    Name
                }
            }
            Name => {
                if whitespace(value) {
                    AfterName
                } else if value == '>' {
                    self.emit_doctype(output)?;
                    return Ok(false);
                } else {
                    self.push_doctype_name(value)?;
                    Name
                }
            }
            AfterName => {
                if self.prefix.is_empty() && whitespace(value) {
                    AfterName
                } else if self.prefix.is_empty() && value == '>' {
                    self.emit_doctype(output)?;
                    return Ok(false);
                } else {
                    return self.doctype_keyword(value);
                }
            }
            AfterKeyword(kind) | BeforeIdentifier(kind) => {
                if whitespace(value) {
                    BeforeIdentifier(kind)
                } else if matches!(value, '\'' | '"') {
                    if matches!(state, AfterKeyword(_)) {
                        self.diagnostic(kind.missing_whitespace())?;
                    }
                    self.begin_identifier(kind);
                    QuotedIdentifier(kind, value)
                } else {
                    if value == '>' {
                        self.diagnostic(kind.missing())?;
                        self.doctype_record().force_quirks = true;
                        self.emit_doctype(output)?;
                        return Ok(false);
                    }
                    self.diagnostic(kind.missing_quote())?;
                    self.doctype_record().force_quirks = true;
                    return self.reconsume_doctype(Bogus);
                }
            }
            QuotedIdentifier(kind, quote) => {
                if value == quote {
                    match kind {
                        Identifier::Public => AfterPublic,
                        Identifier::System => AfterSystem,
                    }
                } else if value == '>' {
                    self.diagnostic(kind.abrupt())?;
                    self.doctype_record().force_quirks = true;
                    self.emit_doctype(output)?;
                    return Ok(false);
                } else {
                    self.push_identifier(kind, value)?;
                    QuotedIdentifier(kind, quote)
                }
            }
            AfterPublic | BetweenIdentifiers => {
                if whitespace(value) {
                    BetweenIdentifiers
                } else if value == '>' {
                    self.emit_doctype(output)?;
                    return Ok(false);
                } else if matches!(value, '\'' | '"') {
                    if matches!(state, AfterPublic) {
                        self.diagnostic(
                            DiagnosticKind::MissingWhitespaceBetweenDoctypeIdentifiers,
                        )?;
                    }
                    self.begin_identifier(Identifier::System);
                    QuotedIdentifier(Identifier::System, value)
                } else {
                    self.diagnostic(DiagnosticKind::MissingQuoteBeforeDoctypeSystemIdentifier)?;
                    self.doctype_record().force_quirks = true;
                    return self.reconsume_doctype(Bogus);
                }
            }
            AfterSystem => {
                if whitespace(value) {
                    AfterSystem
                } else if value == '>' {
                    self.emit_doctype(output)?;
                    return Ok(false);
                } else {
                    self.diagnostic(
                        DiagnosticKind::UnexpectedCharacterAfterDoctypeSystemIdentifier,
                    )?;
                    // This error explicitly does not force quirks mode.
                    return self.reconsume_doctype(Bogus);
                }
            }
            Bogus => {
                if value == '>' {
                    self.emit_doctype(output)?;
                    return Ok(false);
                }
                if value == '\0' {
                    self.diagnostic(DiagnosticKind::UnexpectedNull)?;
                }
                Bogus
            }
        };
        self.state = State::Doctype(next);
        Ok(false)
    }

    fn reconsume_doctype(&mut self, state: DoctypeState) -> Result<bool, HtmlError> {
        self.state = State::Doctype(state);
        Ok(true)
    }

    fn doctype_keyword(&mut self, value: char) -> Result<bool, HtmlError> {
        let mut candidate = self.prefix.clone();
        candidate.push(value);
        let kind = if candidate.eq_ignore_ascii_case("PUBLIC") {
            Some(Identifier::Public)
        } else if candidate.eq_ignore_ascii_case("SYSTEM") {
            Some(Identifier::System)
        } else {
            None
        };
        if let Some(kind) = kind {
            self.prefix.clear();
            self.state = State::Doctype(DoctypeState::AfterKeyword(kind));
        } else if ascii_keyword_prefix(&candidate, "PUBLIC")
            || ascii_keyword_prefix(&candidate, "SYSTEM")
        {
            self.prefix = candidate;
        } else {
            self.diagnostic_at(
                DiagnosticKind::InvalidSequenceAfterDoctypeName,
                self.normalized_offset - self.prefix.len(),
            )?;
            self.prefix.clear();
            self.doctype_record().force_quirks = true;
            // Buffered keyword-prefix bytes are ASCII with no null or '>'.
            // Bogus-doctype processing ignores them; reconsume the mismatch.
            return self.reconsume_doctype(DoctypeState::Bogus);
        }
        Ok(false)
    }

    fn doctype_record(&mut self) -> &mut Doctype {
        self.doctype
            .as_mut()
            .expect("doctype state requires a token")
    }

    fn push_doctype_name(&mut self, value: char) -> Result<(), HtmlError> {
        let value = self.name_char(value)?;
        self.charge_token(value)?;
        self.doctype_record()
            .name
            .get_or_insert_with(String::new)
            .push(value);
        Ok(())
    }

    fn begin_identifier(&mut self, kind: Identifier) {
        let record = self.doctype_record();
        match kind {
            Identifier::Public => record.public_id = Some(String::new()),
            Identifier::System => record.system_id = Some(String::new()),
        }
    }

    fn push_identifier(&mut self, kind: Identifier, value: char) -> Result<(), HtmlError> {
        let value = self.replace_null(value)?;
        self.charge_token(value)?;
        let record = self.doctype_record();
        match kind {
            Identifier::Public => record.public_id.as_mut(),
            Identifier::System => record.system_id.as_mut(),
        }
        .expect("active quoted identifier")
        .push(value);
        Ok(())
    }

    fn emit_doctype(&mut self, output: &mut Vec<Token>) -> Result<(), HtmlError> {
        let record = self.doctype.take().expect("complete doctype token");
        self.state = State::Data;
        self.emit(Token::Doctype(record), output)
    }

    pub(super) fn finish_doctype(
        &mut self,
        state: DoctypeState,
        output: &mut Vec<Token>,
    ) -> Result<(), HtmlError> {
        if matches!(state, DoctypeState::AfterName) && !self.prefix.is_empty() {
            // A partial lookahead at EOF is an invalid keyword, then bogus EOF.
            self.diagnostic_at(
                DiagnosticKind::InvalidSequenceAfterDoctypeName,
                self.normalized_offset - self.prefix.len(),
            )?;
            self.doctype_record().force_quirks = true;
        } else if !matches!(state, DoctypeState::Bogus) {
            self.diagnostic(DiagnosticKind::EofInDoctype)?;
            self.doctype_record().force_quirks = true;
        }
        self.prefix.clear();
        self.emit_doctype(output)
    }
}
