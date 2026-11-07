use rustic_base::Budget;

use super::{
    CommentState, DiagnosticKind, HtmlError, HtmlTokenizer, ProcessingInstruction, State, Token,
    whitespace,
};

#[derive(Clone, Copy, Debug)]
pub(super) enum InstructionState {
    Open,
    Target,
    AfterTarget,
    Data,
    Questionable,
}

impl HtmlTokenizer {
    pub(super) fn start_instruction(&mut self) {
        self.prefix.clear();
        self.token_bytes = Budget::new(self.limits.max_token_bytes);
        self.instruction = None;
        self.state = State::Instruction(InstructionState::Open);
    }

    pub(super) fn instruction_step(
        &mut self,
        state: InstructionState,
        value: char,
        output: &mut Vec<Token>,
    ) -> Result<bool, HtmlError> {
        use InstructionState::*;
        let next = match state {
            Open => {
                if value.is_ascii_alphabetic() || value == '_' {
                    self.state = State::Instruction(Target);
                } else {
                    self.diagnostic(DiagnosticKind::InvalidFirstInstructionTarget)?;
                    self.instruction_to_comment()?;
                }
                return Ok(true);
            }
            Target => {
                if whitespace(value) || matches!(value, '?' | '>') {
                    if self.prefix.eq_ignore_ascii_case("xml")
                        || self.prefix.eq_ignore_ascii_case("xml-stylesheet")
                    {
                        self.diagnostic(DiagnosticKind::DisallowedInstructionTarget)?;
                        self.instruction_to_comment()?;
                    } else {
                        self.instruction = Some(ProcessingInstruction {
                            target: std::mem::take(&mut self.prefix),
                            data: String::new(),
                        });
                        self.state = State::Instruction(AfterTarget);
                    }
                    return Ok(true);
                } else if value.is_ascii_alphanumeric() || matches!(value, '-' | '_') {
                    self.charge_token(value)?;
                    self.prefix.push(value);
                    Target
                } else {
                    self.diagnostic(DiagnosticKind::InvalidInstructionTarget)?;
                    self.instruction_to_comment()?;
                    return Ok(true);
                }
            }
            AfterTarget => {
                if whitespace(value) {
                    AfterTarget
                } else {
                    self.state = State::Instruction(Data);
                    return Ok(true);
                }
            }
            Data => match value {
                '?' => Questionable,
                '>' => {
                    self.emit_instruction(output)?;
                    return Ok(false);
                }
                _ => {
                    self.push_instruction_data(value)?;
                    Data
                }
            },
            Questionable => {
                if value == '>' {
                    self.emit_instruction(output)?;
                    return Ok(false);
                }
                self.push_instruction_data('?')?;
                self.state = State::Instruction(Data);
                return Ok(true);
            }
        };
        self.state = State::Instruction(next);
        Ok(false)
    }

    fn instruction_to_comment(&mut self) -> Result<(), HtmlError> {
        // The target has already been charged. Conversion adds a literal '?'
        // while preserving the same per-token budget and avoiding a second copy.
        self.charge_token('?')?;
        self.comment = std::mem::take(&mut self.prefix);
        self.comment.insert(0, '?');
        self.state = State::Comment(CommentState::Bogus);
        Ok(())
    }

    fn push_instruction_data(&mut self, value: char) -> Result<(), HtmlError> {
        self.charge_token(value)?;
        // The processing-instruction data rule preserves null code points.
        self.instruction
            .as_mut()
            .expect("active processing instruction")
            .data
            .push(value);
        Ok(())
    }

    fn emit_instruction(&mut self, output: &mut Vec<Token>) -> Result<(), HtmlError> {
        let record = self
            .instruction
            .take()
            .expect("complete processing instruction");
        self.state = State::Data;
        self.emit(Token::ProcessingInstruction(record), output)
    }
}
