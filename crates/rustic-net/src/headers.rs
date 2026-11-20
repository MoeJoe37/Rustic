use std::fmt;

/// An ordered list of HTTP header fields.
///
/// Names keep the case they were written with and compare ASCII
/// case-insensitively. Values are byte strings without leading or trailing
/// whitespace and never contain NUL, CR or LF (RFC 9110 §5.5, Fetch "header
/// value"). Repeated names stay separate entries in arrival order.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct Headers {
    entries: Vec<(String, Vec<u8>)>,
}

impl Headers {
    /// An empty list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a field after validating its name and value. Surrounding
    /// spaces and tabs are removed from the value.
    pub fn append(&mut self, name: &str, value: &[u8]) -> Result<(), HeaderError> {
        if !is_token(name.as_bytes()) {
            return Err(HeaderError::InvalidName);
        }
        let value = trim_whitespace(value);
        if !is_valid_value(value) {
            return Err(HeaderError::InvalidValue);
        }
        self.entries.push((name.to_owned(), value.to_vec()));
        Ok(())
    }

    /// The first value for `name`.
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.get_all(name).next()
    }

    /// Every value for `name`, in order.
    pub fn get_all<'a, 'n>(
        &'a self,
        name: &'n str,
    ) -> impl Iterator<Item = &'a [u8]> + use<'a, 'n> {
        self.entries
            .iter()
            .filter(move |(entry, _)| entry.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_slice())
    }

    /// Fetch's "get": every value for `name` joined with `", "`.
    pub fn combined(&self, name: &str) -> Option<Vec<u8>> {
        let mut values = self.get_all(name);
        let mut output = values.next()?.to_vec();
        for value in values {
            output.extend_from_slice(b", ");
            output.extend_from_slice(value);
        }
        Some(output)
    }

    /// Whether any field has this name.
    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Fields in order, as (name, value).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_slice()))
    }

    /// Number of fields.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no fields.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Remove every field named `name`.
    pub fn remove(&mut self, name: &str) {
        self.entries
            .retain(|(entry, _)| !entry.eq_ignore_ascii_case(name));
    }

    pub(crate) fn last_value_mut(&mut self) -> Option<&mut Vec<u8>> {
        self.entries.last_mut().map(|(_, value)| value)
    }
}

impl fmt::Debug for Headers {
    /// Names only: values can carry cookies and credentials.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(self.entries.iter().map(|(name, _)| name))
            .finish()
    }
}

/// A header field could not be added.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeaderError {
    /// The name is empty or not an RFC 9110 token.
    InvalidName,
    /// The value contains NUL, CR or LF.
    InvalidValue,
    /// The transport writes this field itself (`Host`, `Content-Length`,
    /// `Transfer-Encoding`, `Connection`, `TE`, `Upgrade`, `Trailer`,
    /// `Keep-Alive`), so a caller cannot supply it.
    Reserved,
}

impl fmt::Display for HeaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidName => "invalid header name",
            Self::InvalidValue => "invalid header value",
            Self::Reserved => "header is set by the transport",
        })
    }
}

impl std::error::Error for HeaderError {}

/// RFC 9110 §5.6.2 `token`.
pub fn is_token(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes
            .iter()
            .all(|&byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

/// Whether bytes are a valid field value (Fetch "header value": no NUL,
/// CR or LF, no leading or trailing HTTP whitespace is required by callers).
pub fn is_valid_value(bytes: &[u8]) -> bool {
    !bytes.iter().any(|&byte| matches!(byte, 0 | b'\r' | b'\n'))
}

pub(crate) fn trim_whitespace(mut bytes: &[u8]) -> &[u8] {
    while let [b' ' | b'\t', rest @ ..] = bytes {
        bytes = rest;
    }
    while let [rest @ .., b' ' | b'\t'] = bytes {
        bytes = rest;
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_compare_case_insensitively_and_values_are_trimmed() {
        let mut headers = Headers::new();
        headers.append("Accept", b"  text/html\t").unwrap();
        headers.append("accept", b"*/*").unwrap();
        assert_eq!(headers.get("ACCEPT"), Some(&b"text/html"[..]));
        assert_eq!(headers.combined("accept").unwrap(), b"text/html, */*");
        assert_eq!(headers.len(), 2);
        assert!(headers.combined("missing").is_none());
        assert_eq!(format!("{headers:?}"), r#"["Accept", "accept"]"#);
    }

    #[test]
    fn invalid_names_and_values_are_rejected_without_change() {
        let mut headers = Headers::new();
        for name in ["", "a b", "a:b", "é", "a\r"] {
            assert_eq!(headers.append(name, b"x"), Err(HeaderError::InvalidName));
        }
        for value in [&b"a\rb"[..], b"a\nb", b"a\0b"] {
            assert_eq!(headers.append("x", value), Err(HeaderError::InvalidValue));
        }
        assert!(headers.is_empty());
        headers.append("x", b"").unwrap();
        assert_eq!(headers.get("x"), Some(&b""[..]));
    }
}
