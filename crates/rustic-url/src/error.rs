use std::fmt;

/// A failed parse or an explicitly unsupported portion of the URL standard.
///
/// Errors do not echo input, credentials or private paths. Validation errors
/// with specified recovery are not currently exposed as separate diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    /// The input lacks a scheme and no usable base was supplied.
    MissingScheme,
    /// Only fragment references can resolve against an opaque-path base.
    OpaqueBase,
    /// A required host is empty or contains a forbidden code point/encoding.
    InvalidHost,
    /// An IPv4 number, part count or range is invalid.
    InvalidIpv4,
    /// An IPv6 piece, compression, embedded IPv4 address or bracket is invalid.
    InvalidIpv6,
    /// A port is not decimal or is outside 0–65535.
    InvalidPort,
    /// Unicode domain processing needs the future reviewed IDNA adapter.
    UnsupportedIdna,
    /// File URL parsing and platform-specific rules are not implemented.
    UnsupportedFile,
    /// Blob URL registry and origin rules are not implemented.
    UnsupportedBlob,
    /// Input, serialized output or stored path segments exceed configured limits.
    LimitExceeded,
    /// Cooperative cancellation was requested.
    Cancelled,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingScheme => "URL requires an absolute scheme or a usable base",
            Self::OpaqueBase => "opaque URL bases only accept fragment references",
            Self::InvalidHost => "invalid URL host",
            Self::InvalidIpv4 => "invalid URL IPv4 address",
            Self::InvalidIpv6 => "invalid URL IPv6 address",
            Self::InvalidPort => "invalid URL port",
            Self::UnsupportedIdna => "Unicode domain processing is not implemented",
            Self::UnsupportedFile => "file URLs are not implemented",
            Self::UnsupportedBlob => "blob URLs are not implemented",
            Self::LimitExceeded => "URL resource limit exceeded",
            Self::Cancelled => "URL parsing cancelled",
        })
    }
}

impl std::error::Error for ParseError {}
