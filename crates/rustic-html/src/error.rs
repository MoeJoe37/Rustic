use std::fmt;

/// Fatal resource, lifecycle or explicitly unsupported tokenizer condition.
///
/// Fatal errors poison the stream. Earlier successful feed results remain valid;
/// a failing call returns no partial result and cannot be retried on that stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtmlError {
    /// Original input byte budget was exceeded.
    InputLimit,
    /// Total emitted token count, including EOF, exceeded the budget.
    TokenLimit,
    /// Current token/tag data exceeds the configured byte budget.
    TokenBytesLimit,
    /// Too many attributes were encountered in a tag, including duplicates.
    AttributeLimit,
    /// The bounded diagnostic collection is full.
    DiagnosticLimit,
    /// Text chunk size is zero or exceeds the token byte limit.
    InvalidLimits,
    /// Cooperative cancellation was requested.
    Cancelled,
    /// Finish was already called successfully.
    AlreadyFinished,
    /// A prior fatal error made further input invalid.
    Poisoned,
    /// Character-reference tokenization is not implemented yet.
    UnsupportedCharacterReference,
}

impl fmt::Display for HtmlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InputLimit => "HTML input byte limit exceeded",
            Self::TokenLimit => "HTML token count limit exceeded",
            Self::TokenBytesLimit => "HTML token data limit exceeded",
            Self::AttributeLimit => "HTML attribute count limit exceeded",
            Self::DiagnosticLimit => "HTML diagnostic count limit exceeded",
            Self::InvalidLimits => "invalid HTML tokenizer limits",
            Self::Cancelled => "HTML processing cancelled",
            Self::AlreadyFinished => "HTML stream is already finished",
            Self::Poisoned => "HTML stream previously failed",
            Self::UnsupportedCharacterReference => "HTML character references are not implemented",
        })
    }
}

impl std::error::Error for HtmlError {}

/// Recoverable errors in the implemented tokenizer states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticKind {
    /// Null input in a tokenizer state with prescribed recovery.
    UnexpectedNull,
    /// A tag's attribute name was repeated; the first value is retained.
    DuplicateAttribute,
    /// End-tag attributes are ignored after parsing them.
    EndTagAttributes,
    /// Self-closing syntax on an end tag is ignored.
    EndTagSolidus,
    /// Invalid first character after a less-than sign.
    InvalidTagStart,
    /// Empty end-tag name before a greater-than sign.
    MissingEndTagName,
    /// Unexpected equals sign at the start of an attribute name.
    UnexpectedEquals,
    /// Character with prescribed recovery in an attribute name/value.
    UnexpectedAttributeCharacter,
    /// A closing attribute quote was followed by another name without whitespace.
    MissingAttributeWhitespace,
    /// A value was missing after an equals sign.
    MissingAttributeValue,
    /// A slash was not followed by the end of a tag.
    UnexpectedSolidus,
    /// EOF followed a tag-open sequence.
    EofBeforeTagName,
    /// EOF interrupted an unfinished tag, which was discarded.
    EofInTag,
    /// A markup declaration was neither a comment nor a recognized declaration.
    IncorrectlyOpenedComment,
    /// An empty comment closed before its normal closing delimiter.
    AbruptClosingOfEmptyComment,
    /// A nested comment opening occurred inside a comment.
    NestedComment,
    /// A comment closed with `--!>`.
    IncorrectlyClosedComment,
    /// Input ended inside an ordinary comment.
    EofInComment,
    /// A CDATA opening in HTML context recovered as a bogus comment.
    CdataInHtmlContent,
    /// The doctype keyword was immediately followed by its name.
    MissingWhitespaceBeforeDoctypeName,
    /// A doctype closed without a name.
    MissingDoctypeName,
    /// Input ended inside a doctype outside its bogus recovery state.
    EofInDoctype,
    /// Text after a doctype name was not a PUBLIC or SYSTEM keyword.
    InvalidSequenceAfterDoctypeName,
    /// A quoted identifier immediately followed PUBLIC.
    MissingWhitespaceAfterDoctypePublicKeyword,
    /// A doctype closed without the required public identifier.
    MissingDoctypePublicIdentifier,
    /// A public identifier did not begin with a quote.
    MissingQuoteBeforeDoctypePublicIdentifier,
    /// A greater-than sign interrupted a quoted public identifier.
    AbruptDoctypePublicIdentifier,
    /// A system identifier immediately followed the public identifier's quote.
    MissingWhitespaceBetweenDoctypeIdentifiers,
    /// A system identifier did not begin with a quote.
    MissingQuoteBeforeDoctypeSystemIdentifier,
    /// A quoted identifier immediately followed SYSTEM.
    MissingWhitespaceAfterDoctypeSystemKeyword,
    /// A doctype closed without the required system identifier.
    MissingDoctypeSystemIdentifier,
    /// A greater-than sign interrupted a quoted system identifier.
    AbruptDoctypeSystemIdentifier,
    /// Unexpected text followed a completed system identifier.
    UnexpectedCharacterAfterDoctypeSystemIdentifier,
    /// A processing-instruction target did not start with an ASCII letter or `_`.
    InvalidFirstInstructionTarget,
    /// A processing-instruction target contained an invalid character.
    InvalidInstructionTarget,
    /// An XML or XML-stylesheet target recovered as a bogus comment.
    DisallowedInstructionTarget,
    /// An unfinished processing instruction was discarded at EOF.
    EofInProcessingInstruction,
}

/// Diagnostic location in the normalized scalar-value stream.
///
/// `normalized_offset` is neither an original byte offset nor a UTF-16 offset.
/// Original-byte span mapping and line/column diagnostics remain separate work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    /// The recoverable condition.
    pub kind: DiagnosticKind,
    /// Zero-based normalized code-point offset, or stream length at EOF.
    pub normalized_offset: usize,
}
