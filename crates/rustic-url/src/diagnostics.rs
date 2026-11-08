use std::fmt;

use crate::{ParseError, Url};

/// Most diagnostics retained for one parse; later ones only set `truncated`.
pub const MAX_DIAGNOSTICS: usize = 64;

/// A WHATWG URL validation error that Rustic reports with a source location.
///
/// Variants use the standard's validation-error names (see [`Self::name`]). Some
/// accompany a failed parse; others report input that parsed with specified
/// recovery. The enum is non-exhaustive because IDNA and other pending
/// subsystems will add the remaining standard names when they emit them.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ValidationError {
    /// `invalid-URL-unit`: a code point that is not a URL code point, an
    /// unescaped `%`, or leading/trailing C0 controls, spaces, tabs or newlines.
    InvalidUrlUnit,
    /// `special-scheme-missing-following-solidus`: a special scheme is not
    /// followed by exactly `//`.
    SpecialSchemeMissingFollowingSolidus,
    /// `missing-scheme-non-relative-URL`: no scheme and no usable base.
    MissingSchemeNonRelativeUrl,
    /// `invalid-reverse-solidus`: a special URL uses `\` as a separator.
    InvalidReverseSolidus,
    /// `invalid-credentials`: the authority contains `@` userinfo.
    InvalidCredentials,
    /// `host-missing`: a required host is empty.
    HostMissing,
    /// `port-out-of-range`: a port exceeds 65535.
    PortOutOfRange,
    /// `port-invalid`: a port contains a non-digit.
    PortInvalid,
    /// `file-invalid-Windows-drive-letter`: a relative file reference starting
    /// with a drive letter discards the base path.
    FileInvalidWindowsDriveLetter,
    /// `file-invalid-Windows-drive-letter-host`: a file URL host is a drive letter.
    FileInvalidWindowsDriveLetterHost,
    /// `host-invalid-code-point`: an opaque host contains a forbidden host code point.
    HostInvalidCodePoint,
    /// `domain-invalid-code-point`: a domain contains a forbidden domain code point.
    DomainInvalidCodePoint,
    /// `domain-to-ASCII`: a domain's percent-decoded bytes cannot become a valid domain.
    DomainToAscii,
    /// `IPv4-empty-part`: an IPv4 address ends with `.`.
    Ipv4EmptyPart,
    /// `IPv4-too-many-parts`: an IPv4 address has more than four parts.
    Ipv4TooManyParts,
    /// `IPv4-non-numeric-part`: an IPv4 part is not a number.
    Ipv4NonNumericPart,
    /// `IPv4-non-decimal-part`: an IPv4 part uses hexadecimal or octal.
    Ipv4NonDecimalPart,
    /// `IPv4-out-of-range-part`: an IPv4 part exceeds its range.
    Ipv4OutOfRangePart,
    /// `IPv6-unclosed`: an IPv6 address lacks its closing `]`.
    Ipv6Unclosed,
    /// `IPv6-invalid-compression`: an IPv6 address starts with a single `:`.
    Ipv6InvalidCompression,
    /// `IPv6-too-many-pieces`: an IPv6 address has more than eight pieces.
    Ipv6TooManyPieces,
    /// `IPv6-multiple-compression`: an IPv6 address uses `::` more than once.
    Ipv6MultipleCompression,
    /// `IPv6-invalid-code-point`: an IPv6 address has an unexpected code point.
    Ipv6InvalidCodePoint,
    /// `IPv6-too-few-pieces`: an uncompressed IPv6 address has fewer than eight pieces.
    Ipv6TooFewPieces,
    /// `IPv4-in-IPv6-too-many-pieces`: an embedded IPv4 address leaves no room.
    Ipv4InIpv6TooManyPieces,
    /// `IPv4-in-IPv6-invalid-code-point`: an embedded IPv4 address is malformed.
    Ipv4InIpv6InvalidCodePoint,
    /// `IPv4-in-IPv6-out-of-range-part`: an embedded IPv4 part exceeds 255.
    Ipv4InIpv6OutOfRangePart,
    /// `IPv4-in-IPv6-too-few-parts`: an embedded IPv4 address has fewer than four parts.
    Ipv4InIpv6TooFewParts,
}

impl ValidationError {
    /// The URL Standard's name for this validation error.
    pub fn name(self) -> &'static str {
        match self {
            Self::InvalidUrlUnit => "invalid-URL-unit",
            Self::SpecialSchemeMissingFollowingSolidus => {
                "special-scheme-missing-following-solidus"
            }
            Self::MissingSchemeNonRelativeUrl => "missing-scheme-non-relative-URL",
            Self::InvalidReverseSolidus => "invalid-reverse-solidus",
            Self::InvalidCredentials => "invalid-credentials",
            Self::HostMissing => "host-missing",
            Self::PortOutOfRange => "port-out-of-range",
            Self::PortInvalid => "port-invalid",
            Self::FileInvalidWindowsDriveLetter => "file-invalid-Windows-drive-letter",
            Self::FileInvalidWindowsDriveLetterHost => "file-invalid-Windows-drive-letter-host",
            Self::HostInvalidCodePoint => "host-invalid-code-point",
            Self::DomainInvalidCodePoint => "domain-invalid-code-point",
            Self::DomainToAscii => "domain-to-ASCII",
            Self::Ipv4EmptyPart => "IPv4-empty-part",
            Self::Ipv4TooManyParts => "IPv4-too-many-parts",
            Self::Ipv4NonNumericPart => "IPv4-non-numeric-part",
            Self::Ipv4NonDecimalPart => "IPv4-non-decimal-part",
            Self::Ipv4OutOfRangePart => "IPv4-out-of-range-part",
            Self::Ipv6Unclosed => "IPv6-unclosed",
            Self::Ipv6InvalidCompression => "IPv6-invalid-compression",
            Self::Ipv6TooManyPieces => "IPv6-too-many-pieces",
            Self::Ipv6MultipleCompression => "IPv6-multiple-compression",
            Self::Ipv6InvalidCodePoint => "IPv6-invalid-code-point",
            Self::Ipv6TooFewPieces => "IPv6-too-few-pieces",
            Self::Ipv4InIpv6TooManyPieces => "IPv4-in-IPv6-too-many-pieces",
            Self::Ipv4InIpv6InvalidCodePoint => "IPv4-in-IPv6-invalid-code-point",
            Self::Ipv4InIpv6OutOfRangePart => "IPv4-in-IPv6-out-of-range-part",
            Self::Ipv4InIpv6TooFewParts => "IPv4-in-IPv6-too-few-parts",
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// One validation error and the UTF-8 byte offset where it was detected.
///
/// The offset indexes the original string passed to the parser, before C0/space
/// trimming and tab/newline removal. Host-level errors point at the start of the
/// host. A diagnostic never contains input text, so it is safe to log.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Diagnostic {
    kind: ValidationError,
    offset: usize,
}

impl Diagnostic {
    pub(crate) fn new(kind: ValidationError, offset: usize) -> Self {
        Self { kind, offset }
    }

    /// The standard validation error.
    pub fn kind(&self) -> ValidationError {
        self.kind
    }

    /// Byte offset into the original input; always on a `char` boundary.
    pub fn offset(&self) -> usize {
        self.offset
    }
}

/// The result of [`crate::UrlParser::parse_with_diagnostics`].
///
/// Diagnostics are ordered by offset, then by detection order. A failed parse
/// usually ends with the diagnostic that explains the failure; resource-limit,
/// cancellation and unsupported-feature failures have no standard name.
#[derive(Debug)]
pub struct ParseReport {
    result: Result<Url, ParseError>,
    diagnostics: Vec<Diagnostic>,
    truncated: bool,
}

impl ParseReport {
    pub(crate) fn new(
        result: Result<Url, ParseError>,
        mut diagnostics: Vec<Diagnostic>,
        truncated: bool,
    ) -> Self {
        diagnostics.sort_by_key(Diagnostic::offset);
        Self {
            result,
            diagnostics,
            truncated,
        }
    }

    /// The same result [`crate::UrlParser::parse`] returns for this input.
    pub fn result(&self) -> Result<&Url, ParseError> {
        self.result.as_ref().map_err(|error| *error)
    }

    /// Consume the report and keep only the parse result.
    pub fn into_result(self) -> Result<Url, ParseError> {
        self.result
    }

    /// Retained validation errors, at most [`MAX_DIAGNOSTICS`].
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Whether more than [`MAX_DIAGNOSTICS`] validation errors were detected.
    pub fn truncated(&self) -> bool {
        self.truncated
    }
}
