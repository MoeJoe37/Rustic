use rustic_base::{Budget, CancellationToken};

use crate::{
    Attribute, Diagnostic, DiagnosticKind, Doctype, HtmlError, ProcessingInstruction, Tag, Token,
    Utf8Input,
};

mod comment;
mod declaration;
mod doctype;
mod instruction;

use comment::CommentState;
use doctype::DoctypeState;
use instruction::InstructionState;

/// Original-input, token, attribute and diagnostic budgets for one stream.
#[derive(Clone, Copy, Debug)]
pub struct TokenizerLimits {
    /// Cumulative raw input bytes, before decoding/BOM/newline preprocessing.
    pub max_input_bytes: usize,
    /// Total emitted tokens, including the final EOF token.
    pub max_tokens: usize,
    /// UTF-8 payload per markup token, or bytes retained in a character token.
    pub max_token_bytes: usize,
    /// Deterministic maximum character-token UTF-8 bytes, independent of feeds.
    pub max_text_chunk_bytes: usize,
    /// Attribute occurrences per tag; duplicates also consume this limit.
    pub max_attributes: usize,
    /// Maximum retained recoverable diagnostics.
    pub max_diagnostics: usize,
}

impl Default for TokenizerLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 8 * 1024 * 1024,
            max_tokens: 100_000,
            max_token_bytes: 64 * 1024,
            max_text_chunk_bytes: 16 * 1024,
            max_attributes: 128,
            max_diagnostics: 1024,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum State {
    Data,
    TagOpen,
    EndTagOpen,
    TagName,
    BeforeAttribute,
    AttributeName,
    AfterAttributeName,
    BeforeValue,
    QuotedValue(char),
    UnquotedValue,
    AfterQuotedValue,
    SelfClosing,
    MarkupDeclaration,
    Comment(CommentState),
    Doctype(DoctypeState),
    Instruction(InstructionState),
}

#[derive(Debug)]
struct BuildingTag {
    tag: Tag,
    end: bool,
    current: Option<Attribute>,
    current_duplicate: bool,
    attributes_seen: usize,
}

/// Incremental tokenizer for data, tags and declarations in HTML context.
///
/// Feed boundaries do not change token grouping. Character references fail
/// explicitly. RCDATA/RAWTEXT/script/foreign-content modes and DOM tree
/// construction remain unimplemented. Recoverable syntax errors are diagnosed;
/// fatal/unsupported conditions poison the stream instead of guessing behavior.
#[derive(Debug)]
pub struct HtmlTokenizer {
    input: Utf8Input,
    cancellation: CancellationToken,
    limits: TokenizerLimits,
    state: State,
    tag: Option<BuildingTag>,
    token_bytes: Budget,
    prefix: String,
    comment: String,
    doctype: Option<Doctype>,
    instruction: Option<ProcessingInstruction>,
    tokens: Budget,
    text: String,
    diagnostics: Vec<Diagnostic>,
    normalized_offset: usize,
    finished: bool,
    poisoned: bool,
}

impl Default for HtmlTokenizer {
    fn default() -> Self {
        Self::new(TokenizerLimits::default(), CancellationToken::default())
            .expect("valid default tokenizer limits")
    }
}

impl HtmlTokenizer {
    /// Create a tokenizer with explicit budgets and cooperative cancellation.
    pub fn new(
        limits: TokenizerLimits,
        cancellation: CancellationToken,
    ) -> Result<Self, HtmlError> {
        if limits.max_text_chunk_bytes == 0 || limits.max_text_chunk_bytes > limits.max_token_bytes
        {
            return Err(HtmlError::InvalidLimits);
        }
        Ok(Self {
            input: Utf8Input::new(limits.max_input_bytes, cancellation.clone()),
            cancellation,
            limits,
            state: State::Data,
            tag: None,
            token_bytes: Budget::new(limits.max_token_bytes),
            prefix: String::new(),
            comment: String::new(),
            doctype: None,
            instruction: None,
            tokens: Budget::new(limits.max_tokens),
            text: String::new(),
            diagnostics: Vec::new(),
            normalized_offset: 0,
            finished: false,
            poisoned: false,
        })
    }

    /// Bounded recoverable diagnostics in processing order.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Consume bytes without closing an unfinished UTF-8 sequence, text or tag.
    ///
    /// Tokens returned by earlier successful calls are final. A failing call
    /// returns no partial vector and must terminate the consuming document.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Token>, HtmlError> {
        self.ensure_active()?;
        let result = (|| {
            let decoded = self.input.feed(bytes)?;
            let mut output = Vec::new();
            self.consume(&decoded, &mut output)?;
            Ok(output)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Finish input, recover from supported EOF states, and emit exactly one EOF.
    pub fn finish(&mut self) -> Result<Vec<Token>, HtmlError> {
        self.ensure_active()?;
        let result = (|| {
            let decoded = self.input.finish()?;
            let mut output = Vec::new();
            self.consume(&decoded, &mut output)?;
            if matches!(self.state, State::AttributeName) {
                self.finish_attribute_name()?;
            }
            match self.state {
                State::Data => {}
                State::TagOpen => {
                    self.diagnostic(DiagnosticKind::EofBeforeTagName)?;
                    self.text_char('<', &mut output)?;
                }
                State::EndTagOpen => {
                    self.diagnostic(DiagnosticKind::EofBeforeTagName)?;
                    self.text_char('<', &mut output)?;
                    self.text_char('/', &mut output)?;
                }
                State::MarkupDeclaration => self.finish_declaration(&mut output)?,
                State::Comment(state) => self.finish_comment(state, &mut output)?,
                State::Doctype(state) => self.finish_doctype(state, &mut output)?,
                State::Instruction(_) => {
                    self.diagnostic(DiagnosticKind::EofInProcessingInstruction)?;
                    self.prefix.clear();
                    self.instruction = None;
                }
                _ => {
                    self.diagnostic(DiagnosticKind::EofInTag)?;
                    self.tag = None;
                }
            }
            self.flush_text(&mut output)?;
            self.emit(Token::Eof, &mut output)?;
            Ok(output)
        })();
        if result.is_ok() {
            self.finished = true;
        } else {
            self.poisoned = true;
        }
        result
    }

    fn ensure_active(&self) -> Result<(), HtmlError> {
        if self.poisoned {
            Err(HtmlError::Poisoned)
        } else if self.finished {
            Err(HtmlError::AlreadyFinished)
        } else {
            Ok(())
        }
    }

    fn consume(&mut self, decoded: &str, output: &mut Vec<Token>) -> Result<(), HtmlError> {
        for value in decoded.chars() {
            self.cancellation
                .check()
                .map_err(|_| HtmlError::Cancelled)?;
            while self.step(value, output)? {} // Reconsume only after a state transition.
            self.normalized_offset = self
                .normalized_offset
                .checked_add(1)
                .ok_or(HtmlError::InputLimit)?;
        }
        Ok(())
    }

    fn step(&mut self, value: char, output: &mut Vec<Token>) -> Result<bool, HtmlError> {
        match self.state {
            State::Data => match value {
                '<' => {
                    self.flush_text(output)?;
                    self.state = State::TagOpen;
                }
                '&' => return Err(HtmlError::UnsupportedCharacterReference),
                _ => {
                    if value == '\0' {
                        self.diagnostic(DiagnosticKind::UnexpectedNull)?;
                    }
                    self.text_char(value, output)?;
                }
            },
            State::TagOpen => match value {
                '/' => self.state = State::EndTagOpen,
                '!' => {
                    self.prefix.clear();
                    self.state = State::MarkupDeclaration;
                }
                '?' => self.start_instruction(),
                value if value.is_ascii_alphabetic() => self.start_tag(false, value)?,
                _ => {
                    self.diagnostic(DiagnosticKind::InvalidTagStart)?;
                    self.text_char('<', output)?;
                    self.state = State::Data;
                    return Ok(true);
                }
            },
            State::EndTagOpen => match value {
                value if value.is_ascii_alphabetic() => self.start_tag(true, value)?,
                '>' => {
                    self.diagnostic(DiagnosticKind::MissingEndTagName)?;
                    self.state = State::Data;
                }
                _ => {
                    self.diagnostic(DiagnosticKind::InvalidTagStart)?;
                    self.start_comment(CommentState::Bogus);
                    return Ok(true);
                }
            },
            State::TagName => match value {
                value if whitespace(value) => self.state = State::BeforeAttribute,
                '/' => self.state = State::SelfClosing,
                '>' => self.emit_tag(output)?,
                _ => {
                    let value = self.name_char(value)?;
                    self.charge_token(value)?;
                    self.building().tag.name.push(value);
                }
            },
            State::BeforeAttribute => match value {
                value if whitespace(value) => {}
                '/' => self.state = State::SelfClosing,
                '>' => self.emit_tag(output)?,
                '=' => {
                    self.diagnostic(DiagnosticKind::UnexpectedEquals)?;
                    self.start_attribute()?;
                    self.push_attribute_name('=')?;
                    self.state = State::AttributeName;
                }
                _ => {
                    self.start_attribute()?;
                    self.state = State::AttributeName;
                    return Ok(true);
                }
            },
            State::AttributeName => match value {
                value if whitespace(value) => {
                    self.finish_attribute_name()?;
                    self.state = State::AfterAttributeName;
                }
                '/' | '>' => {
                    self.finish_attribute_name()?;
                    self.state = State::AfterAttributeName;
                    return Ok(true);
                }
                '=' => {
                    self.finish_attribute_name()?;
                    self.state = State::BeforeValue;
                }
                _ => {
                    if matches!(value, '"' | '\'' | '<') {
                        self.diagnostic(DiagnosticKind::UnexpectedAttributeCharacter)?;
                    }
                    let value = self.name_char(value)?;
                    self.push_attribute_name(value)?;
                }
            },
            State::AfterAttributeName => match value {
                value if whitespace(value) => {}
                '=' => self.state = State::BeforeValue,
                '/' => {
                    self.finish_attribute()?;
                    self.state = State::SelfClosing;
                }
                '>' => {
                    self.finish_attribute()?;
                    self.emit_tag(output)?;
                }
                _ => {
                    self.finish_attribute()?;
                    self.start_attribute()?;
                    self.state = State::AttributeName;
                    return Ok(true);
                }
            },
            State::BeforeValue => match value {
                value if whitespace(value) => {}
                '"' | '\'' => self.state = State::QuotedValue(value),
                '>' => {
                    self.diagnostic(DiagnosticKind::MissingAttributeValue)?;
                    self.finish_attribute()?;
                    self.emit_tag(output)?;
                }
                _ => {
                    self.state = State::UnquotedValue;
                    return Ok(true);
                }
            },
            State::QuotedValue(quote) => {
                if value == quote {
                    self.finish_attribute()?;
                    self.state = State::AfterQuotedValue;
                } else {
                    self.push_attribute_value(value)?;
                }
            }
            State::UnquotedValue => match value {
                value if whitespace(value) => {
                    self.finish_attribute()?;
                    self.state = State::BeforeAttribute;
                }
                '>' => {
                    self.finish_attribute()?;
                    self.emit_tag(output)?;
                }
                _ => {
                    if matches!(value, '"' | '\'' | '<' | '=' | '`') {
                        self.diagnostic(DiagnosticKind::UnexpectedAttributeCharacter)?;
                    }
                    self.push_attribute_value(value)?;
                }
            },
            State::AfterQuotedValue => match value {
                value if whitespace(value) => self.state = State::BeforeAttribute,
                '/' => self.state = State::SelfClosing,
                '>' => self.emit_tag(output)?,
                _ => {
                    self.diagnostic(DiagnosticKind::MissingAttributeWhitespace)?;
                    self.state = State::BeforeAttribute;
                    return Ok(true);
                }
            },
            State::SelfClosing => {
                if value == '>' {
                    self.building().tag.self_closing = true;
                    self.emit_tag(output)?;
                } else {
                    self.diagnostic(DiagnosticKind::UnexpectedSolidus)?;
                    self.state = State::BeforeAttribute;
                    return Ok(true);
                }
            }
            State::MarkupDeclaration => return self.declaration_step(value),
            State::Comment(state) => return self.comment_step(state, value, output),
            State::Doctype(state) => return self.doctype_step(state, value, output),
            State::Instruction(state) => return self.instruction_step(state, value, output),
        }
        Ok(false)
    }

    fn start_tag(&mut self, end: bool, first: char) -> Result<(), HtmlError> {
        self.token_bytes = Budget::new(self.limits.max_token_bytes);
        self.tag = Some(BuildingTag {
            tag: Tag {
                name: String::new(),
                attributes: Vec::new(),
                self_closing: false,
            },
            end,
            current: None,
            current_duplicate: false,
            attributes_seen: 0,
        });
        let first = first.to_ascii_lowercase();
        self.charge_token(first)?;
        self.building().tag.name.push(first);
        self.state = State::TagName;
        Ok(())
    }

    fn start_attribute(&mut self) -> Result<(), HtmlError> {
        if self.building().attributes_seen >= self.limits.max_attributes {
            return Err(HtmlError::AttributeLimit);
        }
        self.building().attributes_seen += 1;
        self.building().current = Some(Attribute {
            name: String::new(),
            value: String::new(),
        });
        self.building().current_duplicate = false;
        Ok(())
    }

    fn finish_attribute_name(&mut self) -> Result<(), HtmlError> {
        let tag = self.building();
        let current = tag.current.as_ref().expect("active attribute name");
        tag.current_duplicate = tag
            .tag
            .attributes
            .iter()
            .any(|old| old.name == current.name);
        if tag.current_duplicate {
            self.diagnostic(DiagnosticKind::DuplicateAttribute)?;
        }
        Ok(())
    }

    fn finish_attribute(&mut self) -> Result<(), HtmlError> {
        if let Some(attribute) = self.building().current.take() {
            if !self.building().current_duplicate {
                self.building().tag.attributes.push(attribute);
            }
        }
        Ok(())
    }

    fn push_attribute_name(&mut self, value: char) -> Result<(), HtmlError> {
        self.charge_token(value)?;
        self.building()
            .current
            .as_mut()
            .expect("active attribute")
            .name
            .push(value);
        Ok(())
    }

    fn push_attribute_value(&mut self, value: char) -> Result<(), HtmlError> {
        if value == '&' {
            return Err(HtmlError::UnsupportedCharacterReference);
        }
        let value = self.replace_null(value)?;
        self.charge_token(value)?;
        self.building()
            .current
            .as_mut()
            .expect("active attribute")
            .value
            .push(value);
        Ok(())
    }

    fn name_char(&mut self, value: char) -> Result<char, HtmlError> {
        Ok(self.replace_null(value)?.to_ascii_lowercase())
    }

    fn replace_null(&mut self, value: char) -> Result<char, HtmlError> {
        if value == '\0' {
            self.diagnostic(DiagnosticKind::UnexpectedNull)?;
            Ok('\u{fffd}')
        } else {
            Ok(value)
        }
    }

    fn building(&mut self) -> &mut BuildingTag {
        self.tag.as_mut().expect("tag state requires a builder")
    }
    fn charge_token(&mut self, value: char) -> Result<(), HtmlError> {
        self.token_bytes
            .charge(value.len_utf8())
            .map_err(|_| HtmlError::TokenBytesLimit)
    }

    fn emit_tag(&mut self, output: &mut Vec<Token>) -> Result<(), HtmlError> {
        let tag = self.tag.take().expect("complete tag builder");
        let token = if tag.end {
            if !tag.tag.attributes.is_empty() {
                self.diagnostic(DiagnosticKind::EndTagAttributes)?;
            }
            if tag.tag.self_closing {
                self.diagnostic(DiagnosticKind::EndTagSolidus)?;
            }
            Token::EndTag(tag.tag.name)
        } else {
            Token::StartTag(tag.tag)
        };
        self.state = State::Data;
        self.emit(token, output)
    }

    fn text_char(&mut self, value: char, output: &mut Vec<Token>) -> Result<(), HtmlError> {
        if value.len_utf8() > self.limits.max_text_chunk_bytes {
            return Err(HtmlError::TokenBytesLimit);
        }
        if self.text.len() > self.limits.max_text_chunk_bytes - value.len_utf8() {
            self.flush_text(output)?;
        }
        self.text.push(value);
        Ok(())
    }

    fn flush_text(&mut self, output: &mut Vec<Token>) -> Result<(), HtmlError> {
        if !self.text.is_empty() {
            let text = std::mem::take(&mut self.text);
            self.emit(Token::Characters(text), output)?;
        }
        Ok(())
    }

    fn emit(&mut self, token: Token, output: &mut Vec<Token>) -> Result<(), HtmlError> {
        self.tokens.charge(1).map_err(|_| HtmlError::TokenLimit)?;
        output.push(token);
        Ok(())
    }

    fn diagnostic(&mut self, kind: DiagnosticKind) -> Result<(), HtmlError> {
        self.diagnostic_at(kind, self.normalized_offset)
    }

    fn diagnostic_at(
        &mut self,
        kind: DiagnosticKind,
        normalized_offset: usize,
    ) -> Result<(), HtmlError> {
        if self.diagnostics.len() >= self.limits.max_diagnostics {
            return Err(HtmlError::DiagnosticLimit);
        }
        self.diagnostics.push(Diagnostic {
            kind,
            normalized_offset,
        });
        Ok(())
    }
}

fn whitespace(value: char) -> bool {
    matches!(value, '\t' | '\n' | '\u{c}' | '\r' | ' ')
}
