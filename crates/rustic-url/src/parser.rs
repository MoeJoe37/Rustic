use std::borrow::Cow;

use rustic_base::CancellationToken;

use rustic_encoding::Encoding;

use crate::diagnostics::{Diagnostic, MAX_DIAGNOSTICS, ParseReport};
use crate::encoding::{self, EncodeSet};
use crate::record::default_port;
use crate::{Host, ParseError, Url, UrlPath, ValidationError, ipv4, ipv6};

mod setter;

pub use setter::UrlComponent;

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
    query_encoding: Option<Encoding>,
}

impl UrlParser {
    /// Configure limits and a shared cancellation signal for this parser.
    pub fn new(limits: UrlLimits, cancellation: CancellationToken) -> Self {
        Self {
            limits,
            cancellation,
            query_encoding: None,
        }
    }

    /// Use `encoding` (its output encoding) for the queries of special URLs
    /// other than `ws:` and `wss:`, as the URL Standard's "percent-encode
    /// after encoding" requires when a document in a legacy encoding parses
    /// a URL. A scalar value the encoding cannot represent becomes
    /// `%26%23NNN%3B`. Every other component, and every other URL, stays
    /// UTF-8. The default is UTF-8 everywhere.
    pub fn with_query_encoding(mut self, encoding: Encoding) -> Self {
        let output = encoding.output_encoding();
        self.query_encoding = (output != Encoding::Utf8).then_some(output);
        self
    }

    /// Parse without changing the supplied base, including when validation fails.
    pub fn parse(&self, input: &str, base: Option<&Url>) -> Result<Url, ParseError> {
        Run::new(self, None).parse(input, base)
    }

    /// Parse exactly as [`Self::parse`] does, also reporting validation errors.
    ///
    /// The result is identical to `parse`; diagnostics never change it. At most
    /// [`MAX_DIAGNOSTICS`] diagnostics are kept, each a kind plus byte offset.
    pub fn parse_with_diagnostics(&self, input: &str, base: Option<&Url>) -> ParseReport {
        let mut run = Run::new(self, Some(Sink::default()));
        let result = run.parse(input, base);
        let sink = run.sink.take().unwrap_or_default();
        ParseReport::new(result, sink.items, sink.truncated)
    }
}

/// Collected diagnostics and the map from preprocessed to original offsets.
#[derive(Default)]
struct Sink {
    /// Address and length of the preprocessed input that parser slices borrow.
    start: usize,
    len: usize,
    /// Bytes trimmed from the front of the original input.
    leading: usize,
    /// Offsets, within the trimmed input, of removed tabs and newlines.
    removed: Vec<usize>,
    items: Vec<Diagnostic>,
    truncated: bool,
}

impl Sink {
    fn push(&mut self, kind: ValidationError, original_offset: usize) {
        if self.items.len() >= MAX_DIAGNOSTICS {
            self.truncated = true;
        } else {
            self.items.push(Diagnostic::new(kind, original_offset));
        }
    }

    fn original(&self, preprocessed: usize) -> usize {
        let mut trimmed = preprocessed;
        for &removed in &self.removed {
            if removed > trimmed {
                break;
            }
            trimmed += 1;
        }
        self.leading + trimmed
    }
}

/// State for one parse. Diagnostics are only gathered when `sink` is present.
struct Run<'p> {
    limits: &'p UrlLimits,
    cancellation: &'p CancellationToken,
    sink: Option<Sink>,
    query_encoding: Option<Encoding>,
    /// The query as the parser saw it, kept only when a legacy query
    /// encoding may apply once the scheme of the result is known.
    raw_query: Option<String>,
}

impl<'p> Run<'p> {
    fn new(parser: &'p UrlParser, sink: Option<Sink>) -> Self {
        Self {
            limits: &parser.limits,
            cancellation: &parser.cancellation,
            sink,
            query_encoding: parser.query_encoding,
            raw_query: None,
        }
    }

    fn parse(&mut self, input: &str, base: Option<&Url>) -> Result<Url, ParseError> {
        self.checkpoint()?;
        if input.len() > self.limits.max_input_bytes {
            return Err(ParseError::LimitExceeded);
        }
        let input = self.preprocess(input);
        if let Some(sink) = &mut self.sink {
            sink.start = input.as_ptr().addr();
            sink.len = input.len();
        }
        let mut url = self.dispatch(&input, base)?;
        if let (Some(encoding), Some(raw)) = (self.query_encoding, self.raw_query.take()) {
            // "If encoding is not UTF-8 and url is not special or url's scheme
            // is ws or wss, set encoding to UTF-8": only these keep it.
            if url.query.is_some()
                && matches!(url.scheme.as_str(), "ftp" | "file" | "http" | "https")
            {
                url.query = Some(encoding::encode_legacy_query(&raw, encoding));
            }
        }
        self.validate_record(&url)?;
        Ok(url)
    }

    fn validate_record(&self, url: &Url) -> Result<(), ParseError> {
        self.checkpoint()?;
        if url.serialized_len()? > self.limits.max_serialized_bytes {
            return Err(ParseError::LimitExceeded);
        }
        if matches!(&url.path, UrlPath::Segments(segments) if segments.len() > self.limits.max_path_segments)
        {
            return Err(ParseError::LimitExceeded);
        }
        Ok(())
    }

    fn dispatch(&mut self, input: &str, base: Option<&Url>) -> Result<Url, ParseError> {
        let Some((scheme, rest)) = split_scheme(input) else {
            let Some(base) = base else {
                self.report(ValidationError::MissingSchemeNonRelativeUrl, input);
                return Err(ParseError::MissingScheme);
            };
            if base.scheme == "file" {
                return self.file(input, Some(base));
            }
            return self.relative(input, base);
        };
        let scheme = scheme.to_ascii_lowercase();
        match scheme.as_str() {
            "file" => {
                if !rest.starts_with("//") {
                    self.report(ValidationError::SpecialSchemeMissingFollowingSolidus, rest);
                }
                return self.file(rest, base.filter(|base| base.scheme == "file"));
            }
            "blob" => return Err(ParseError::UnsupportedBlob),
            _ => {}
        }
        if default_port(&scheme).is_some() {
            let same_scheme_base = base.filter(|base| base.scheme == scheme);
            if let Some(slashes) = rest.strip_prefix("//") {
                self.extra_slashes(slashes);
            } else {
                self.report(ValidationError::SpecialSchemeMissingFollowingSolidus, rest);
                if let Some(base) = same_scheme_base {
                    return self.relative(rest, base);
                }
                self.extra_slashes(rest);
            }
            self.authority(
                rest.trim_start_matches(|value| separator(value, true)),
                scheme,
                true,
            )
        } else if let Some(rest) = rest.strip_prefix("//") {
            self.authority(rest, scheme, false)
        } else if let Some(path) = rest.strip_prefix('/') {
            let mut url = empty(scheme, UrlPath::Segments(Vec::new()));
            self.suffix(&mut url, path, false, false)?;
            Ok(url)
        } else {
            self.opaque(rest, scheme)
        }
    }

    fn checkpoint(&self) -> Result<(), ParseError> {
        self.cancellation.check().map_err(|_| ParseError::Cancelled)
    }

    /// Record `kind` at the start of `at`, which must borrow the preprocessed input.
    fn report(&mut self, kind: ValidationError, at: &str) {
        if let Some(sink) = &mut self.sink {
            let offset = at.as_ptr().addr().wrapping_sub(sink.start);
            debug_assert!(offset <= sink.len, "diagnostic slice outside the input");
            let offset = sink.original(offset.min(sink.len));
            sink.push(kind, offset);
        }
    }

    /// Each further leading separator after a special scheme's `//`.
    fn extra_slashes(&mut self, input: &str) {
        if self.sink.is_none() {
            return;
        }
        for (index, _) in input
            .char_indices()
            .take_while(|(_, value)| separator(*value, true))
        {
            self.report(
                ValidationError::SpecialSchemeMissingFollowingSolidus,
                &input[index..],
            );
        }
    }

    /// `invalid-URL-unit` for non-URL code points and `%` without two hex digits.
    fn scan_units(&mut self, input: &str) {
        if self.sink.is_none() {
            return;
        }
        let bytes = input.as_bytes();
        for (index, value) in input.char_indices() {
            let invalid = if value == '%' {
                !(bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                    && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit))
            } else {
                !url_code_point(value)
            };
            if invalid {
                self.report(ValidationError::InvalidUrlUnit, &input[index..]);
            }
        }
    }

    fn preprocess<'i>(&mut self, input: &'i str) -> Cow<'i, str> {
        let c0_or_space = |value: char| value <= '\u{20}';
        let trimmed = input.trim_matches(c0_or_space);
        let leading = input.len() - input.trim_start_matches(c0_or_space).len();
        let newline = |value: char| matches!(value, '\t' | '\n' | '\r');
        if let Some(sink) = &mut self.sink {
            sink.leading = leading;
            if trimmed.len() != input.len() {
                let offset = if leading > 0 { 0 } else { trimmed.len() };
                sink.push(ValidationError::InvalidUrlUnit, offset);
            }
            if let Some(first) = trimmed.find(newline) {
                sink.push(ValidationError::InvalidUrlUnit, leading + first);
                sink.removed = trimmed.match_indices(newline).map(|(i, _)| i).collect();
            }
        }
        if trimmed.contains(newline) {
            Cow::Owned(trimmed.chars().filter(|value| !newline(*value)).collect())
        } else {
            Cow::Borrowed(trimmed)
        }
    }

    fn copy_base(&self, base: &Url) -> Result<Url, ParseError> {
        self.checkpoint()?;
        if base.serialized_len()? > self.limits.max_serialized_bytes {
            return Err(ParseError::LimitExceeded);
        }
        Ok(base.clone())
    }

    fn relative(&mut self, input: &str, base: &Url) -> Result<Url, ParseError> {
        if matches!(base.path, UrlPath::Opaque(_)) {
            let Some(fragment) = input.strip_prefix('#') else {
                self.report(ValidationError::MissingSchemeNonRelativeUrl, input);
                return Err(ParseError::OpaqueBase);
            };
            let mut url = self.copy_base(base)?;
            url.fragment = Some(self.fragment(fragment));
            return Ok(url);
        }
        let special = default_port(base.scheme()).is_some();
        let mut chars = input.chars();
        if chars.next().is_some_and(|value| separator(value, special))
            && chars.next().is_some_and(|value| separator(value, special))
        {
            let authority = if special {
                for index in 0..2 {
                    if input.as_bytes()[index] == b'\\' {
                        self.report(ValidationError::InvalidReverseSolidus, &input[index..]);
                    }
                }
                self.extra_slashes(&input[2..]);
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
                url.query = Some(self.query(query, special));
            }
            url.fragment = fragment.map(|fragment| self.fragment(fragment));
            return Ok(url);
        }
        let UrlPath::Segments(segments) = &mut url.path else {
            unreachable!("opaque base handled above")
        };
        let path = if let Some(rest) = self.strip_separator(path, special) {
            segments.clear();
            rest
        } else {
            segments.pop();
            path
        };
        self.path(segments, path, special, false)?;
        url.query = query.map(|query| self.query(query, special));
        url.fragment = fragment.map(|fragment| self.fragment(fragment));
        Ok(url)
    }

    /// Strip one leading separator, reporting a special URL's `\`.
    fn strip_separator<'i>(&mut self, input: &'i str, special: bool) -> Option<&'i str> {
        let first = input
            .chars()
            .next()
            .filter(|value| separator(*value, special))?;
        if first == '\\' {
            self.report(ValidationError::InvalidReverseSolidus, input);
        }
        Some(&input[1..])
    }

    fn authority(&mut self, input: &str, scheme: String, special: bool) -> Result<Url, ParseError> {
        self.checkpoint()?;
        let end = input
            .find(|value| separator(value, special) || matches!(value, '?' | '#'))
            .unwrap_or(input.len());
        let authority = &input[..end];
        let mut url = empty(scheme, UrlPath::Segments(Vec::new()));
        let (host_port, credentials) = if let Some((credentials, host)) = authority.rsplit_once('@')
        {
            if self.sink.is_some() {
                for (index, _) in authority.match_indices('@') {
                    self.report(ValidationError::InvalidCredentials, &authority[index..]);
                }
            }
            let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
            url.username = encoding::encode(user, EncodeSet::UserInfo);
            url.password = encoding::encode(password, EncodeSet::UserInfo);
            (host, true)
        } else {
            (authority, false)
        };
        let (host, port) = if host_port.starts_with('[') {
            let Some(close) = host_port.find(']') else {
                self.report(ValidationError::Ipv6Unclosed, host_port);
                return Err(ParseError::InvalidIpv6);
            };
            let after = &host_port[close + 1..];
            let port = if after.is_empty() {
                None
            } else if let Some(port) = after.strip_prefix(':') {
                Some(port)
            } else {
                self.report(ValidationError::Ipv6Unclosed, host_port);
                return Err(ParseError::InvalidHost);
            };
            (&host_port[..close + 1], port)
        } else if let Some((host, port)) = host_port.rsplit_once(':') {
            (host, Some(port))
        } else {
            (host_port, None)
        };
        let host = self.host(host, special)?;
        if host.is_empty() && (credentials || port.is_some()) {
            self.report(ValidationError::HostMissing, host_port);
            return Err(ParseError::InvalidHost);
        }
        url.host = Some(host);
        if let Some(port) = port.filter(|port| !port.is_empty()) {
            let value = self.port(port)?;
            if Some(value) != default_port(&url.scheme) {
                url.port = Some(value);
            }
        }
        let rest = &input[end..];
        let (path, query, fragment) = split_suffix(rest);
        if !path.is_empty() || special {
            let path = self.strip_separator(path, special).unwrap_or(path);
            let UrlPath::Segments(segments) = &mut url.path else {
                unreachable!("new hierarchical URL")
            };
            self.path(segments, path, special, false)?;
        }
        url.query = query.map(|query| self.query(query, special));
        url.fragment = fragment.map(|fragment| self.fragment(fragment));
        Ok(url)
    }

    fn port(&mut self, port: &str) -> Result<u16, ParseError> {
        if let Some(index) = port.find(|value: char| !value.is_ascii_digit()) {
            self.report(ValidationError::PortInvalid, &port[index..]);
            return Err(ParseError::InvalidPort);
        }
        let mut value = 0u16;
        for byte in port.bytes() {
            let Some(next) = value
                .checked_mul(10)
                .and_then(|value| value.checked_add(u16::from(byte - b'0')))
            else {
                self.report(ValidationError::PortOutOfRange, port);
                return Err(ParseError::InvalidPort);
            };
            value = next;
        }
        Ok(value)
    }

    fn host(&mut self, input: &str, special: bool) -> Result<Host, ParseError> {
        let result = Host::parse(input, special);
        if self.sink.is_some() {
            self.diagnose_host(input, special, &result);
        }
        result
    }

    /// Name the validation errors behind a host result; host offsets are its start.
    fn diagnose_host(&mut self, input: &str, special: bool, result: &Result<Host, ParseError>) {
        let bracketed = input.starts_with('[');
        let error = match result {
            Ok(host) if host.ipv4().is_some() => {
                self.diagnose_ipv4(input);
                return;
            }
            Ok(_) if !special && !bracketed => {
                self.scan_units(input);
                return;
            }
            Ok(_) => return,
            Err(ParseError::InvalidIpv6) => input
                .strip_prefix('[')
                .and_then(|value| value.strip_suffix(']'))
                .map_or(Some(ValidationError::Ipv6Unclosed), |address| {
                    ipv6::analyze(address).err()
                }),
            Err(ParseError::InvalidIpv4) => {
                self.diagnose_ipv4(input);
                return;
            }
            Err(ParseError::InvalidHost) if input.is_empty() => Some(ValidationError::HostMissing),
            Err(ParseError::InvalidHost) if !special => Some(ValidationError::HostInvalidCodePoint),
            Err(ParseError::InvalidHost) => Some(match encoding::decode_host(input) {
                // A non-ASCII domain that UTS #46 rejects (or maps to nothing)
                // fails in domain-to-ASCII, before forbidden code points.
                Ok(decoded)
                    if !decoded.is_ascii()
                        && crate::idna::to_ascii(&decoded).is_ok_and(|ascii| !ascii.is_empty()) =>
                {
                    ValidationError::DomainInvalidCodePoint
                }
                Ok(decoded) if !decoded.is_ascii() => ValidationError::DomainToAscii,
                Ok(_) => ValidationError::DomainInvalidCodePoint,
                Err(_) => ValidationError::DomainToAscii,
            }),
            Err(_) => None,
        };
        if let Some(error) = error {
            self.report(error, input);
        }
    }

    fn diagnose_ipv4(&mut self, input: &str) {
        let Ok(decoded) = encoding::decode_host(input) else {
            return;
        };
        let mut errors = Vec::new();
        if let Err(error) = ipv4::analyze(&decoded.to_ascii_lowercase(), &mut |error| {
            errors.push(error);
        }) {
            errors.push(error);
        }
        for error in errors {
            self.report(error, input);
        }
    }

    /// The file, file slash and file host states of the URL Standard.
    fn file(&mut self, input: &str, base: Option<&Url>) -> Result<Url, ParseError> {
        self.checkpoint()?;
        let mut url = empty("file".into(), UrlPath::Segments(Vec::new()));
        url.host = Some(empty_host());
        if let Some(rest) = self.strip_separator(input, true) {
            if let Some(authority) = self.strip_separator(rest, true) {
                return self.file_host(url, authority);
            }
            if let Some(base) = base {
                let base = self.copy_base(base)?;
                url.host = base.host;
                if let (false, UrlPath::Segments(base_segments)) =
                    (starts_with_drive_letter(rest), base.path)
                {
                    if let Some(drive) = base_segments
                        .into_iter()
                        .next()
                        .filter(|segment| normalized_drive_letter(segment))
                    {
                        url.path = UrlPath::Segments(vec![drive]);
                    }
                }
            }
            return self.file_path(url, rest);
        }
        let Some(base) = base else {
            return self.file_path(url, input);
        };
        let mut url = self.copy_base(base)?;
        url.fragment = None;
        match input.chars().next() {
            None => Ok(url),
            Some('?' | '#') => {
                let (_, query, fragment) = split_suffix(input);
                if let Some(query) = query {
                    url.query = Some(self.query(query, true));
                }
                url.fragment = fragment.map(|fragment| self.fragment(fragment));
                Ok(url)
            }
            Some(_) => {
                url.query = None;
                let UrlPath::Segments(segments) = &mut url.path else {
                    unreachable!("file URLs are hierarchical")
                };
                if starts_with_drive_letter(input) {
                    self.report(ValidationError::FileInvalidWindowsDriveLetter, input);
                    segments.clear();
                } else {
                    shorten(segments, true);
                }
                self.file_path(url, input)
            }
        }
    }

    fn file_host(&mut self, mut url: Url, input: &str) -> Result<Url, ParseError> {
        let end = input.find(['/', '\\', '?', '#']).unwrap_or(input.len());
        let buffer = &input[..end];
        if windows_drive_letter(buffer) {
            // The drive letter becomes the first path segment and the host stays
            // empty. The standard appends it to the path buffer without the
            // path state's code point checks, so it is not rescanned.
            self.report(ValidationError::FileInvalidWindowsDriveLetterHost, buffer);
            url.path = UrlPath::Segments(vec![format!("{}:", &buffer[..1])]);
            let rest = &input[end..];
            return match self.strip_separator(rest, true) {
                Some(path) => self.file_path(url, path),
                None => {
                    let (_, query, fragment) = split_suffix(rest);
                    url.query = query.map(|query| self.query(query, true));
                    url.fragment = fragment.map(|fragment| self.fragment(fragment));
                    Ok(url)
                }
            };
        }
        if !buffer.is_empty() {
            let host = self.host(buffer, true)?;
            if host.domain() != Some("localhost") {
                url.host = Some(host);
            }
        }
        let rest = &input[end..];
        let rest = self.strip_separator(rest, true).unwrap_or(rest);
        self.file_path(url, rest)
    }

    fn file_path(&mut self, mut url: Url, input: &str) -> Result<Url, ParseError> {
        self.suffix(&mut url, input, true, true)?;
        Ok(url)
    }

    fn opaque(&mut self, input: &str, scheme: String) -> Result<Url, ParseError> {
        let (path, query, fragment) = split_suffix(input);
        self.scan_units(path);
        let mut encoded = encoding::encode(path, EncodeSet::Control);
        if (query.is_some() || fragment.is_some()) && encoded.ends_with(' ') {
            encoded.pop();
            encoded.push_str("%20");
        }
        let mut url = empty(scheme, UrlPath::Opaque(encoded));
        url.query = query.map(|query| self.query(query, false));
        url.fragment = fragment.map(|fragment| self.fragment(fragment));
        Ok(url)
    }

    fn suffix(
        &mut self,
        url: &mut Url,
        input: &str,
        special: bool,
        file: bool,
    ) -> Result<(), ParseError> {
        let (path, query, fragment) = split_suffix(input);
        let UrlPath::Segments(segments) = &mut url.path else {
            unreachable!("hierarchical URL")
        };
        self.path(segments, path, special, file)?;
        url.query = query.map(|query| self.query(query, special));
        url.fragment = fragment.map(|fragment| self.fragment(fragment));
        Ok(())
    }

    fn query(&mut self, input: &str, special: bool) -> String {
        self.scan_units(input);
        if self.query_encoding.is_some() {
            self.raw_query = Some(input.to_owned());
        }
        encoding::encode(input, query_set(special))
    }

    fn fragment(&mut self, input: &str) -> String {
        self.scan_units(input);
        encoding::encode(input, EncodeSet::Fragment)
    }

    /// The path state; `file` enables drive-letter normalization and protection.
    fn path(
        &mut self,
        segments: &mut Vec<String>,
        input: &str,
        special: bool,
        file: bool,
    ) -> Result<(), ParseError> {
        let mut start = 0;
        loop {
            self.checkpoint()?;
            let end = input[start..]
                .find(|value| separator(value, special))
                .map(|index| start + index);
            let part = &input[start..end.unwrap_or(input.len())];
            let last = end.is_none();
            if double_dot(part) {
                shorten(segments, file);
                if last {
                    self.push_segment(segments, String::new())?;
                }
            } else if part == "." || part.eq_ignore_ascii_case("%2e") {
                if last {
                    self.push_segment(segments, String::new())?;
                }
            } else {
                self.scan_units(part);
                let segment = if file && segments.is_empty() && windows_drive_letter(part) {
                    format!("{}:", &part[..1])
                } else {
                    encoding::encode(part, EncodeSet::Path)
                };
                self.push_segment(segments, segment)?;
            }
            let Some(end) = end else {
                return Ok(());
            };
            if input.as_bytes()[end] == b'\\' {
                self.report(ValidationError::InvalidReverseSolidus, &input[end..]);
            }
            start = end + 1;
        }
    }

    fn push_segment(&self, segments: &mut Vec<String>, segment: String) -> Result<(), ParseError> {
        if segments.len() >= self.limits.max_path_segments {
            return Err(ParseError::LimitExceeded);
        }
        segments.push(segment);
        Ok(())
    }
}

/// Remove the last segment, except a file URL's lone normalized drive letter.
fn shorten(segments: &mut Vec<String>, file: bool) {
    if file && segments.len() == 1 && normalized_drive_letter(&segments[0]) {
        return;
    }
    segments.pop();
}

/// An ASCII letter followed by `:` or `|`.
fn windows_drive_letter(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && matches!(bytes[1], b':' | b'|')
}

/// An ASCII letter followed by `:`.
fn normalized_drive_letter(value: &str) -> bool {
    windows_drive_letter(value) && value.ends_with(':')
}

/// A drive letter followed by the end of input or `/`, `\`, `?` or `#`.
fn starts_with_drive_letter(value: &str) -> bool {
    value.len() >= 2
        && value.is_char_boundary(2)
        && windows_drive_letter(&value[..2])
        && value[2..]
            .bytes()
            .next()
            .is_none_or(|byte| matches!(byte, b'/' | b'\\' | b'?' | b'#'))
}

/// The empty host that file URLs use for local files.
fn empty_host() -> Host {
    Host::parse("", false).expect("the empty opaque host is always valid")
}

/// A URL code point, as defined by the URL Standard.
fn url_code_point(value: char) -> bool {
    let noncharacter =
        matches!(value, '\u{fdd0}'..='\u{fdef}') || (value as u32) & 0xfffe == 0xfffe;
    value.is_ascii_alphanumeric()
        || "!$&'()*+,-./:;=?@_~".contains(value)
        || (value >= '\u{a0}' && !noncharacter)
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
