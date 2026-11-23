//! Redaction of credentials in network diagnostics (ADR 0069).
//!
//! The renderer never holds cookies or HTTP authentication: the broker
//! keeps them. These helpers are a second line: whatever a diagnostic
//! carries, credential-bearing headers and URL user information are
//! replaced by [`REDACTED`] before a line leaves the session.

/// The text that replaces a redacted value.
pub const REDACTED: &str = "<redacted>";

/// Whether a header's value is a credential: cookies, HTTP
/// authentication and the common API-key and token headers.
pub fn sensitive_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "cookie2"
            | "set-cookie"
            | "set-cookie2"
            | "x-api-key"
            | "x-auth-token"
            | "x-csrf-token"
            | "x-xsrf-token"
    )
}

/// A header value as it may appear in a diagnostic.
pub fn header_value<'a>(name: &str, value: &'a str) -> &'a str {
    if sensitive_header(name) {
        REDACTED
    } else {
        value
    }
}

/// `url` with any user name and password replaced by [`REDACTED`].
pub fn url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_owned();
    };
    let authority_start = scheme_end + 3;
    let rest = &url[authority_start..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    match authority.rfind('@') {
        Some(at) => format!(
            "{}{REDACTED}@{}{}",
            &url[..authority_start],
            &authority[at + 1..],
            &rest[authority_end..]
        ),
        None => url.to_owned(),
    }
}
