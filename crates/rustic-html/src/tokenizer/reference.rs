use crate::entities::{MAX_NAME_BYTES, NAMED_ENTITIES};

use super::{DiagnosticKind as D, HtmlError, HtmlTokenizer, State, Token};

#[derive(Clone, Copy, Debug)]
pub(super) enum ReferenceContext {
    Data,
    QuotedAttribute(char),
    UnquotedAttribute,
}

impl ReferenceContext {
    fn state(self) -> State {
        match self {
            Self::Data => State::Data,
            Self::QuotedAttribute(quote) => State::QuotedValue(quote),
            Self::UnquotedAttribute => State::UnquotedValue,
        }
    }

    fn in_attribute(self) -> bool {
        !matches!(self, Self::Data)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ReferenceState {
    Start,
    Named,
    Ambiguous,
    NumericStart,
    HexStart,
    Decimal,
    Hex,
}

#[derive(Clone, Copy, Debug)]
struct NamedMatch {
    value: &'static str,
    length: usize,
    terminated: bool,
    following_offset: usize,
}

#[derive(Debug)]
pub(super) struct CharacterReference {
    context: ReferenceContext,
    buffer: String,
    matched: Option<NamedMatch>,
    number: u32,
    overflow: bool,
}

impl HtmlTokenizer {
    pub(super) fn start_reference(&mut self, context: ReferenceContext) {
        self.reference = Some(CharacterReference {
            context,
            buffer: String::with_capacity(MAX_NAME_BYTES),
            matched: None,
            number: 0,
            overflow: false,
        });
        self.state = State::Reference(ReferenceState::Start);
    }

    pub(super) fn reference_step(
        &mut self,
        state: ReferenceState,
        value: char,
        output: &mut Vec<Token>,
    ) -> Result<bool, HtmlError> {
        use ReferenceState::*;
        let next = match state {
            Start => {
                if value.is_ascii_alphanumeric() {
                    self.state = State::Reference(Named);
                    return Ok(true);
                } else if value == '#' {
                    self.reference_record().buffer.push('#');
                    NumericStart
                } else {
                    self.literal_reference(output)?;
                    return Ok(true);
                }
            }
            Named => return self.named_reference_step(value, output),
            Ambiguous => {
                if value.is_ascii_alphanumeric() {
                    let context = self.reference_record().context;
                    self.reference_char(context, value, output)?;
                    Ambiguous
                } else {
                    if value == ';' {
                        self.diagnostic(D::UnknownNamedCharacterReference)?;
                    }
                    self.take_reference();
                    return Ok(true);
                }
            }
            NumericStart => {
                if matches!(value, 'x' | 'X') {
                    self.reference_record().buffer.push(value);
                    HexStart
                } else if value.is_ascii_digit() {
                    self.numeric_digit(value.to_digit(10).expect("ASCII decimal digit"), 10);
                    Decimal
                } else {
                    self.diagnostic(D::AbsenceOfDigitsInNumericReference)?;
                    self.literal_reference(output)?;
                    return Ok(true);
                }
            }
            HexStart => {
                if value.is_ascii_hexdigit() {
                    self.numeric_digit(value.to_digit(16).expect("ASCII hex digit"), 16);
                    Hex
                } else {
                    self.diagnostic(D::AbsenceOfDigitsInNumericReference)?;
                    self.literal_reference(output)?;
                    return Ok(true);
                }
            }
            Decimal | Hex => {
                let radix = if matches!(state, Hex) { 16 } else { 10 };
                let digit = value.to_digit(radix).filter(|_| value.is_ascii());
                if let Some(digit) = digit {
                    self.numeric_digit(digit, radix);
                    state
                } else {
                    if value != ';' {
                        self.diagnostic(D::MissingSemicolonAfterCharacterReference)?;
                    }
                    self.finish_numeric_reference(output)?;
                    return Ok(value != ';');
                }
            }
        };
        self.state = State::Reference(next);
        Ok(false)
    }

    fn reference_record(&mut self) -> &mut CharacterReference {
        self.reference
            .as_mut()
            .expect("reference state requires context")
    }

    fn take_reference(&mut self) -> CharacterReference {
        let reference = self.reference.take().expect("active character reference");
        self.state = reference.context.state();
        reference
    }

    fn reference_char(
        &mut self,
        context: ReferenceContext,
        value: char,
        output: &mut Vec<Token>,
    ) -> Result<(), HtmlError> {
        match context {
            ReferenceContext::Data => self.text_char(value, output),
            _ => self.push_attribute_value(value),
        }
    }

    fn reference_text(
        &mut self,
        context: ReferenceContext,
        text: &str,
        output: &mut Vec<Token>,
    ) -> Result<(), HtmlError> {
        // Named output and buffered suffixes have a fixed, table-derived bound.
        for value in text.chars() {
            self.reference_char(context, value, output)?;
        }
        Ok(())
    }

    fn literal_reference(&mut self, output: &mut Vec<Token>) -> Result<(), HtmlError> {
        let reference = self.take_reference();
        self.reference_char(reference.context, '&', output)?;
        self.reference_text(reference.context, &reference.buffer, output)
    }

    fn named_reference_step(
        &mut self,
        value: char,
        output: &mut Vec<Token>,
    ) -> Result<bool, HtmlError> {
        let mut candidate = self.reference_record().buffer.clone();
        candidate.push(value);
        if let Some((name, decoded)) = named_prefix(&candidate) {
            let exact = name == candidate;
            let terminated = exact && candidate.ends_with(';');
            let following_offset = self
                .normalized_offset
                .checked_add(1)
                .ok_or(HtmlError::InputLimit)?;
            let reference = self.reference_record();
            reference.buffer = candidate;
            if exact {
                reference.matched = Some(NamedMatch {
                    value: decoded,
                    length: name.len(),
                    terminated,
                    following_offset,
                });
            }
            if terminated {
                self.finish_named_reference(None, output)?;
            }
            Ok(false)
        } else if self.reference_record().matched.is_some() {
            self.finish_named_reference(Some(value), output)?;
            Ok(true)
        } else {
            let reference = self.reference_record();
            let context = reference.context;
            let buffer = std::mem::take(&mut reference.buffer);
            self.reference_char(context, '&', output)?;
            self.reference_text(context, &buffer, output)?;
            self.state = State::Reference(ReferenceState::Ambiguous);
            Ok(true)
        }
    }

    fn finish_named_reference(
        &mut self,
        next: Option<char>,
        output: &mut Vec<Token>,
    ) -> Result<(), HtmlError> {
        let reference = self.take_reference();
        let Some(matched) = reference.matched else {
            self.reference_char(reference.context, '&', output)?;
            return self.reference_text(reference.context, &reference.buffer, output);
        };
        let suffix = &reference.buffer[matched.length..];
        let following = suffix.chars().next().or(next);
        if !matched.terminated
            && reference.context.in_attribute()
            && following.is_some_and(|value| value.is_ascii_alphanumeric() || value == '=')
        {
            // Historical attribute ambiguity: preserve the entire consumed text.
            self.reference_char(reference.context, '&', output)?;
            self.reference_text(reference.context, &reference.buffer, output)
        } else {
            if !matched.terminated {
                self.diagnostic_at(
                    D::MissingSemicolonAfterCharacterReference,
                    matched.following_offset,
                )?;
            }
            self.reference_text(reference.context, matched.value, output)?;
            self.reference_text(reference.context, suffix, output)
        }
    }

    fn numeric_digit(&mut self, digit: u32, radix: u32) {
        let reference = self.reference_record();
        if !reference.overflow {
            if let Some(value) = reference
                .number
                .checked_mul(radix)
                .and_then(|value| value.checked_add(digit))
            {
                reference.number = value;
            } else {
                reference.overflow = true;
            }
        }
        // No digit string is retained, even for an arbitrarily long reference.
    }

    fn finish_numeric_reference(&mut self, output: &mut Vec<Token>) -> Result<(), HtmlError> {
        let reference = self.take_reference();
        let mut value = reference.number;
        if reference.overflow || value > 0x10ffff {
            self.diagnostic(D::CharacterReferenceOutsideUnicodeRange)?;
            value = 0xfffd;
        } else if value == 0 {
            self.diagnostic(D::NullCharacterReference)?;
            value = 0xfffd;
        } else if (0xd800..=0xdfff).contains(&value) {
            self.diagnostic(D::SurrogateCharacterReference)?;
            value = 0xfffd;
        } else if (0xfdd0..=0xfdef).contains(&value) || value & 0xffff >= 0xfffe {
            self.diagnostic(D::NoncharacterCharacterReference)?;
        } else if value == 13 || matches!(value, 1..=8 | 11 | 14..=31 | 0x7f..=0x9f) {
            self.diagnostic(D::ControlCharacterReference)?;
            if (0x80..=0x9f).contains(&value) {
                value = C1_REPLACEMENTS[(value - 0x80) as usize];
            }
        }
        let value = char::from_u32(value).expect("validated/replaced Unicode scalar");
        self.reference_char(reference.context, value, output)
    }

    pub(super) fn finish_reference(
        &mut self,
        state: ReferenceState,
        output: &mut Vec<Token>,
    ) -> Result<(), HtmlError> {
        match state {
            ReferenceState::Start => self.literal_reference(output),
            ReferenceState::Named => self.finish_named_reference(None, output),
            ReferenceState::Ambiguous => {
                self.take_reference();
                Ok(())
            }
            ReferenceState::NumericStart | ReferenceState::HexStart => {
                self.diagnostic(D::AbsenceOfDigitsInNumericReference)?;
                self.literal_reference(output)
            }
            ReferenceState::Decimal | ReferenceState::Hex => {
                self.diagnostic(D::MissingSemicolonAfterCharacterReference)?;
                self.finish_numeric_reference(output)
            }
        }
    }
}

fn named_prefix(candidate: &str) -> Option<(&'static str, &'static str)> {
    if candidate.len() > MAX_NAME_BYTES {
        return None;
    }
    let index = NAMED_ENTITIES.partition_point(|(name, _)| *name < candidate);
    NAMED_ENTITIES
        .get(index)
        .copied()
        .filter(|(name, _)| name.starts_with(candidate))
}

const C1_REPLACEMENTS: [u32; 32] = [
    0x20ac, 0x81, 0x201a, 0x192, 0x201e, 0x2026, 0x2020, 0x2021, 0x2c6, 0x2030, 0x160, 0x2039,
    0x152, 0x8d, 0x17d, 0x8f, 0x90, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014, 0x2dc,
    0x2122, 0x161, 0x203a, 0x153, 0x9d, 0x17e, 0x178,
];
