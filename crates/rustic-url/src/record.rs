use std::fmt::{self, Write};

use crate::{Host, Origin, ParseError, UrlParser};

/// URL path segments or an opaque path, already encoded by the parser.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum UrlPath {
    /// Ordered segments; serialization adds a slash before each segment.
    Segments(Vec<String>),
    /// A non-hierarchical path that cannot resolve ordinary relative references.
    Opaque(String),
}

/// A validated immutable URL record. Parse a new record to resolve a reference.
///
/// Equality compares records, not origins. `Debug` intentionally omits userinfo,
/// paths, queries and fragments. Explicit serialization includes credentials.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct Url {
    pub(crate) scheme: String,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) host: Option<Host>,
    pub(crate) port: Option<u16>,
    pub(crate) path: UrlPath,
    pub(crate) query: Option<String>,
    pub(crate) fragment: Option<String>,
}

impl Url {
    /// Parse using default limits and an optional already-parsed base.
    pub fn parse(input: &str, base: Option<&Self>) -> Result<Self, ParseError> {
        UrlParser::default().parse(input, base)
    }

    /// Lowercase ASCII scheme without a colon.
    pub fn scheme(&self) -> &str {
        &self.scheme
    }
    /// Encoded username; serialization is explicit because this can be sensitive.
    pub fn username(&self) -> &str {
        &self.username
    }
    /// Encoded password; never included in `Debug`.
    pub fn password(&self) -> &str {
        &self.password
    }
    /// Canonical host, distinguishing absent and empty hosts.
    pub fn host(&self) -> Option<&Host> {
        self.host.as_ref()
    }
    /// Non-default explicit port; default ports are canonicalized to `None`.
    pub fn port(&self) -> Option<u16> {
        self.port
    }
    /// Structured path, preserving the distinction between segments and opacity.
    pub fn path(&self) -> &UrlPath {
        &self.path
    }
    /// Encoded query without `?`; absence and an empty query remain distinct.
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }
    /// Encoded fragment without `#`; absence and an empty fragment remain distinct.
    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }

    /// URL path serialization without query/fragment or the protective `/.` prefix.
    pub fn pathname(&self) -> String {
        let mut output = String::new();
        self.write_path(&mut output)
            .expect("String writes cannot fail");
        output
    }

    /// Serialize the entire record, including any username/password.
    pub fn serialize(&self) -> String {
        self.serialize_with(true, true)
    }
    /// Serialize without userinfo; query/path content can still be sensitive.
    pub fn serialize_without_credentials(&self) -> String {
        self.serialize_with(false, true)
    }
    /// Serialize without the fragment, including any username/password.
    pub fn serialize_without_fragment(&self) -> String {
        self.serialize_with(true, false)
    }

    /// Derive a tuple origin or a **new** opaque origin for this URL.
    ///
    /// Repeated derivation for an opaque-path URL produces distinct origins.
    /// Document/navigation code must retain its assigned origin and clone that
    /// value when sharing identity; URL string equality never supplies it.
    pub fn origin(&self) -> Origin {
        Origin::for_url(self)
    }

    fn serialize_with(&self, credentials: bool, fragment: bool) -> String {
        let mut output = String::new();
        self.write_record(&mut output, credentials, fragment)
            .expect("String writes cannot fail");
        output
    }

    pub(crate) fn serialized_len(&self) -> Result<usize, ParseError> {
        let mut count = Counter(0);
        self.write_record(&mut count, true, true)
            .map_err(|_| ParseError::LimitExceeded)?;
        Ok(count.0)
    }

    fn write_record(
        &self,
        writer: &mut impl Write,
        credentials: bool,
        fragment: bool,
    ) -> fmt::Result {
        write!(writer, "{}:", self.scheme)?;
        if let Some(host) = &self.host {
            writer.write_str("//")?;
            if credentials && (!self.username.is_empty() || !self.password.is_empty()) {
                writer.write_str(&self.username)?;
                if !self.password.is_empty() {
                    write!(writer, ":{}", self.password)?;
                }
                writer.write_char('@')?;
            }
            writer.write_str(&host.serialize())?;
            if let Some(port) = self.port {
                write!(writer, ":{port}")?;
            }
        } else if matches!(&self.path, UrlPath::Segments(segments) if segments.len() > 1 && segments[0].is_empty())
        {
            writer.write_str("/.")?;
        }
        self.write_path(writer)?;
        if let Some(query) = &self.query {
            write!(writer, "?{query}")?;
        }
        if fragment {
            if let Some(fragment) = &self.fragment {
                write!(writer, "#{fragment}")?;
            }
        }
        Ok(())
    }

    fn write_path(&self, writer: &mut impl Write) -> fmt::Result {
        match &self.path {
            UrlPath::Opaque(path) => writer.write_str(path),
            UrlPath::Segments(segments) => {
                for segment in segments {
                    write!(writer, "/{segment}")?;
                }
                Ok(())
            }
        }
    }
}

impl fmt::Debug for Url {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Url")
            .field("scheme", &self.scheme)
            .field("host", &self.host)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

struct Counter(usize);
impl Write for Counter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.0 = self.0.checked_add(text.len()).ok_or(fmt::Error)?;
        Ok(())
    }
}

pub(crate) fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "ftp" => Some(21),
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    }
}
