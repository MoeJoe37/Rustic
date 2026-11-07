use std::borrow::Cow;

use rustic_base::CancellationToken;

use crate::encoding::{self, EncodeSet};
use crate::record::default_port;
use crate::{Host, ParseError, Url, UrlPath};

/// Resource limits for one parse; units count bytes and retained path segments.
///
/// These are not total process-memory limits. Temporary work is linear in the
/// bounded input and any copied base record, including percent-encoding expansion.
#[derive(Clone, Copy, Debug)]
pub struct UrlLimits {
    /// Maximum original UTF-8 input bytes, before whitespace preprocessing.
    pub max_input_bytes: usize,
    /// Maximum serialized record bytes; also bounds a base before it is copied.
    pub max_serialized_bytes: usize,
    /// Maximum retained hierarchical path segments, including empty segments.
    pub max_path_segments: usize,
}

impl Default for UrlLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 64 * 1024,
            max_serialized_bytes: 256 * 1024,
            max_path_segments: 4096,
        }
    }
}

/// Reusable bounded parser with cooperative cancellation checkpoints.
///
/// Checkpoints occur before/after parsing and between structural components/path
/// segments. They do not interrupt allocation or provide a wall-clock timeout.
#[derive(Debug, Default)]
pub struct UrlParser {
    limits: UrlLimits,
    cancellation: CancellationToken,
}

impl UrlParser {
    /// Configure limits and a shared cancellation signal for this parser.
    pub fn new(limits: UrlLimits, cancellation: CancellationToken) -> Self {
        Self {
            limits,
            cancellation,
        }
    }

    /// Parse without changing the supplied base, including when validation fails.
    pub fn parse(&self, input: &str, base: Option<&Url>) -> Result<Url, ParseError> {
        self.checkpoint()?;
        if input.len() > self.limits.max_input_bytes {
            return Err(ParseError::LimitExceeded);
        }
        let input = preprocess(input);
        let url = if let Some((scheme, rest)) = split_scheme(&input) {
            let scheme = scheme.to_ascii_lowercase();
            match scheme.as_str() {
                "file" => return Err(ParseError::UnsupportedFile),
                "blob" => return Err(ParseError::UnsupportedBlob),
                _ => {}
            }
            let special = default_port(&scheme).is_some();
            if special {
                if let Some(base) =
                    base.filter(|base| base.scheme() == scheme && !rest.starts_with("//"))
                {
                    self.relative(rest, base)?
                } else {
                    self.authority(
                        rest.trim_start_matches(|value| separator(value, true)),
                        scheme,
                        true,
                    )?
                }
            } else if let Some(rest) = rest.strip_prefix("//") {
                self.authority(rest, scheme, false)?
            } else if let Some(path) = rest.strip_prefix('/') {
                let mut url = empty(scheme, UrlPath::Segments(Vec::new()));
                self.suffix(&mut url, path, false)?;
                url
            } else {
                self.opaque(rest, scheme)?
            }
        } else {
            self.relative(&input, base.ok_or(ParseError::MissingScheme)?)?
        };
        self.checkpoint()?;
        if url.serialized_len()? > self.limits.max_serialized_bytes {
            return Err(ParseError::LimitExceeded);
        }
        if matches!(&url.path, UrlPath::Segments(segments) if segments.len() > self.limits.max_path_segments)
        {
            return Err(ParseError::LimitExceeded);
        }
        Ok(url)
    }

    fn checkpoint(&self) -> Result<(), ParseError> {
        self.cancellation.check().map_err(|_| ParseError::Cancelled)
    }

    fn copy_base(&self, base: &Url) -> Result<Url, ParseError> {
        self.checkpoint()?;
        if base.serialized_len()? > self.limits.max_serialized_bytes {
            return Err(ParseError::LimitExceeded);
        }
        Ok(base.clone())
    }

    fn relative(&self, input: &str, base: &Url) -> Result<Url, ParseError> {
        if matches!(base.path, UrlPath::Opaque(_)) {
            let fragment = input.strip_prefix('#').ok_or(ParseError::OpaqueBase)?;
            let mut url = self.copy_base(base)?;
            url.fragment = Some(encoding::encode(fragment, EncodeSet::Fragment));
            return Ok(url);
        }
        let special = default_port(base.scheme()).is_some();
        let mut chars = input.chars();
        if chars.next().is_some_and(|value| separator(value, special))
            && chars.next().is_some_and(|value| separator(value, special))
        {
            let authority = if special {
                input.trim_start_matches(|value| separator(value, true))
            } else {
                &input[2..]
            };
            return self.authority(authority, base.scheme.clone(), special);
        }
        let mut url = self.copy_base(base)?;
        url.fragment = None;
        let (path, query, fragment) = split_suffix(input);
        if path.is_empty() {
            if let Some(query) = query {
                url.query = Some(encoding::encode(query, query_set(special)));
            }
            url.fragment = fragment.map(|fragment| encoding::encode(fragment, EncodeSet::Fragment));
            return Ok(url);
        }
        let UrlPath::Segments(segments) = &mut url.path else {
            unreachable!("opaque base handled above")
        };
        let path = if path.starts_with(|value| separator(value, special)) {
            segments.clear();
            &path[1..]
        } else {
            segments.pop();
            path
        };
        self.path(segments, path, special)?;
        url.query = query.map(|query| encoding::encode(query, query_set(special)));
        url.fragment = fragment.map(|fragment| encoding::encode(fragment, EncodeSet::Fragment));
        Ok(url)
    }

    fn authority(&self, input: &str, scheme: String, special: bool) -> Result<Url, ParseError> {
        self.checkpoint()?;
        let end = input
            .find(|value| separator(value, special) || matches!(value, '?' | '#'))
            .unwrap_or(input.len());
        let authority = &input[..end];
        let mut url = empty(scheme, UrlPath::Segments(Vec::new()));
        let (host_port, credentials) = if let Some((credentials, host)) = authority.rsplit_once('@')
        {
            let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
            url.username = encoding::encode(user, EncodeSet::UserInfo);
            url.password = encoding::encode(password, EncodeSet::UserInfo);
            (host, true)
        } else {
            (authority, false)
        };
        let (host, port) = if host_port.starts_with('[') {
            let close = host_port.find(']').ok_or(ParseError::InvalidIpv6)?;
            let after = &host_port[close + 1..];
            let port = if after.is_empty() {
                None
            } else {
                Some(after.strip_prefix(':').ok_or(ParseError::InvalidHost)?)
            };
            (&host_port[..close + 1], port)
        } else if let Some((host, port)) = host_port.rsplit_once(':') {
            (host, Some(port))
        } else {
            (host_port, None)
        };
        let host = Host::parse(host, special)?;
        if host.is_empty() && (credentials || port.is_some()) {
            return Err(ParseError::InvalidHost);
        }
        url.host = Some(host);
        if let Some(port) = port.filter(|port| !port.is_empty()) {
            if !port.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(ParseError::InvalidPort);
            }
            let mut value = 0u16;
            for byte in port.bytes() {
                value = value
                    .checked_mul(10)
                    .and_then(|value| value.checked_add(u16::from(byte - b'0')))
                    .ok_or(ParseError::InvalidPort)?;
            }
            if Some(value) != default_port(&url.scheme) {
                url.port = Some(value);
            }
        }
        let rest = &input[end..];
        let (path, query, fragment) = split_suffix(rest);
        if !path.is_empty() || special {
            let path = if path.starts_with(|value| separator(value, special)) {
                &path[1..]
            } else {
                path
            };
            let UrlPath::Segments(segments) = &mut url.path else {
                unreachable!("new hierarchical URL")
            };
            self.path(segments, path, special)?;
        }
        url.query = query.map(|query| encoding::encode(query, query_set(special)));
        url.fragment = fragment.map(|fragment| encoding::encode(fragment, EncodeSet::Fragment));
        Ok(url)
    }

    fn opaque(&self, input: &str, scheme: String) -> Result<Url, ParseError> {
        let (path, query, fragment) = split_suffix(input);
        let mut encoded = encoding::encode(path, EncodeSet::Control);
        if (query.is_some() || fragment.is_some()) && encoded.ends_with(' ') {
            encoded.pop();
            encoded.push_str("%20");
        }
        let mut url = empty(scheme, UrlPath::Opaque(encoded));
        url.query = query.map(|query| encoding::encode(query, EncodeSet::Query));
        url.fragment = fragment.map(|fragment| encoding::encode(fragment, EncodeSet::Fragment));
        Ok(url)
    }

    fn suffix(&self, url: &mut Url, input: &str, special: bool) -> Result<(), ParseError> {
        let (path, query, fragment) = split_suffix(input);
        let UrlPath::Segments(segments) = &mut url.path else {
            unreachable!("hierarchical URL")
        };
        self.path(segments, path, special)?;
        url.query = query.map(|query| encoding::encode(query, query_set(special)));
        url.fragment = fragment.map(|fragment| encoding::encode(fragment, EncodeSet::Fragment));
        Ok(())
    }

    fn path(
        &self,
        segments: &mut Vec<String>,
        input: &str,
        special: bool,
    ) -> Result<(), ParseError> {
        let mut parts = input.split(|value| separator(value, special)).peekable();
        while let Some(part) = parts.next() {
            self.checkpoint()?;
            let last = parts.peek().is_none();
            if double_dot(part) {
                segments.pop();
                if last {
                    self.push_segment(segments, String::new())?;
                }
            } else if part == "." || part.eq_ignore_ascii_case("%2e") {
                if last {
                    self.push_segment(segments, String::new())?;
                }
            } else {
                self.push_segment(segments, encoding::encode(part, EncodeSet::Path))?;
            }
        }
        Ok(())
    }

    fn push_segment(&self, segments: &mut Vec<String>, segment: String) -> Result<(), ParseError> {
        if segments.len() >= self.limits.max_path_segments {
            return Err(ParseError::LimitExceeded);
        }
        segments.push(segment);
        Ok(())
    }
}

fn empty(scheme: String, path: UrlPath) -> Url {
    Url {
        scheme,
        username: String::new(),
        password: String::new(),
        host: None,
        port: None,
        path,
        query: None,
        fragment: None,
    }
}

fn preprocess(input: &str) -> Cow<'_, str> {
    let trimmed = input.trim_matches(|value| value <= '\u{20}');
    if trimmed.contains(['\t', '\n', '\r']) {
        Cow::Owned(
            trimmed
                .chars()
                .filter(|value| !matches!(value, '\t' | '\n' | '\r'))
                .collect(),
        )
    } else {
        Cow::Borrowed(trimmed)
    }
}

fn split_scheme(input: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = input.split_once(':')?;
    if !scheme
        .as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_alphabetic())
        || !scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
    {
        return None;
    }
    Some((scheme, rest))
}

fn split_suffix(input: &str) -> (&str, Option<&str>, Option<&str>) {
    let (before_fragment, fragment) = input
        .split_once('#')
        .map_or((input, None), |(rest, fragment)| (rest, Some(fragment)));
    let (path, query) = before_fragment
        .split_once('?')
        .map_or((before_fragment, None), |(path, query)| (path, Some(query)));
    (path, query, fragment)
}

fn separator(value: char, special: bool) -> bool {
    value == '/' || (special && value == '\\')
}
fn query_set(special: bool) -> EncodeSet {
    if special {
        EncodeSet::SpecialQuery
    } else {
        EncodeSet::Query
    }
}
fn double_dot(value: &str) -> bool {
    ["..", ".%2e", "%2e.", "%2e%2e"]
        .iter()
        .any(|dot| value.eq_ignore_ascii_case(dot))
}
