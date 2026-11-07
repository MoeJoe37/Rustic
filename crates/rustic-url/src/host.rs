use std::fmt;

use crate::encoding::{self, EncodeSet};
use crate::{ParseError, ipv4, ipv6};

/// Canonical validated host; construction is restricted to the URL parser.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct Host(HostKind);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum HostKind {
    Domain(String),
    Ipv4(u32),
    Ipv6([u16; 8]),
    Opaque(String),
    Empty,
}

impl Host {
    pub(crate) fn parse(input: &str, special: bool) -> Result<Self, ParseError> {
        if input.starts_with('[') {
            let address = input
                .strip_prefix('[')
                .and_then(|value| value.strip_suffix(']'))
                .ok_or(ParseError::InvalidIpv6)?;
            return ipv6::parse(address).map(|address| Self(HostKind::Ipv6(address)));
        }
        if input.is_empty() {
            return if special {
                Err(ParseError::InvalidHost)
            } else {
                Ok(Self(HostKind::Empty))
            };
        }
        if !special {
            if input.chars().any(forbidden_host) {
                return Err(ParseError::InvalidHost);
            }
            return Ok(Self(HostKind::Opaque(encoding::encode(
                input,
                EncodeSet::Control,
            ))));
        }
        let decoded = encoding::decode_host(input)?;
        if decoded.chars().any(|value| {
            forbidden_host(value) || value <= '\u{1f}' || value == '\u{7f}' || value == '%'
        }) {
            return Err(ParseError::InvalidHost);
        }
        if !decoded.is_ascii() {
            return Err(ParseError::UnsupportedIdna);
        }
        let domain = decoded.to_ascii_lowercase();
        if ipv4::ends_in_number(&domain) {
            return ipv4::parse(&domain).map(|address| Self(HostKind::Ipv4(address)));
        }
        Ok(Self(HostKind::Domain(domain)))
    }

    /// Host serialization, including brackets around IPv6 addresses.
    pub fn serialize(&self) -> String {
        match &self.0 {
            HostKind::Domain(value) | HostKind::Opaque(value) => value.clone(),
            HostKind::Ipv4(value) => std::net::Ipv4Addr::from(*value).to_string(),
            HostKind::Ipv6(value) => format!("[{}]", ipv6::serialize(value)),
            HostKind::Empty => String::new(),
        }
    }

    /// Canonical ASCII domain for a special URL, if this is a domain host.
    pub fn domain(&self) -> Option<&str> {
        match &self.0 {
            HostKind::Domain(value) => Some(value),
            _ => None,
        }
    }

    /// Network-order IPv4 bytes, if this is an IPv4 host.
    pub fn ipv4(&self) -> Option<[u8; 4]> {
        match self.0 {
            HostKind::Ipv4(value) => Some(value.to_be_bytes()),
            _ => None,
        }
    }

    /// Eight network-order IPv6 pieces, if this is an IPv6 host.
    pub fn ipv6(&self) -> Option<[u16; 8]> {
        match self.0 {
            HostKind::Ipv6(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        matches!(self.0, HostKind::Empty)
    }
}

impl fmt::Debug for Host {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

fn forbidden_host(value: char) -> bool {
    matches!(
        value,
        '\0' | '\t'
            | '\n'
            | '\r'
            | ' '
            | '#'
            | '/'
            | ':'
            | '<'
            | '>'
            | '?'
            | '@'
            | '['
            | '\\'
            | ']'
            | '^'
            | '|'
    )
}
