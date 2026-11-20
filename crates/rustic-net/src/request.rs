use std::fmt;

use rustic_url::Url;

use crate::headers::{HeaderError, Headers, is_token};

/// Fields the transport writes itself (RFC 9112 framing and connection
/// management). A caller-supplied copy could desynchronize framing between
/// Rustic and an intermediary, so they are refused.
const RESERVED: [&str; 8] = [
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "upgrade",
];

/// One HTTP request: method, absolute `http`/`https` URL, header fields and a
/// complete body.
///
/// Policy (forbidden request headers, CORS, cookies) belongs to Fetch
/// (NET-03). This type only guarantees that the request serializes into a
/// single well-framed HTTP/1.1 message.
#[derive(Clone)]
pub struct Request {
    method: String,
    url: Url,
    headers: Headers,
    body: Vec<u8>,
}

impl Request {
    /// A request with no header fields and an empty body.
    ///
    /// The method must be a token; the URL's scheme must be `http` or
    /// `https`. Userinfo and the fragment are never sent.
    pub fn new(method: &str, url: Url) -> Result<Self, RequestError> {
        if !is_token(method.as_bytes()) {
            return Err(RequestError::InvalidMethod);
        }
        if !matches!(url.scheme(), "http" | "https") {
            return Err(RequestError::UnsupportedScheme);
        }
        if url.host().is_none_or(|host| host.serialize().is_empty()) {
            return Err(RequestError::MissingHost);
        }
        Ok(Self {
            method: method.to_owned(),
            url,
            headers: Headers::new(),
            body: Vec::new(),
        })
    }

    /// Shorthand for a `GET` request.
    pub fn get(url: Url) -> Result<Self, RequestError> {
        Self::new("GET", url)
    }

    /// Append a header field. Framing and connection fields are refused
    /// with [`HeaderError::Reserved`].
    pub fn header(&mut self, name: &str, value: &[u8]) -> Result<&mut Self, RequestError> {
        if RESERVED
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(name))
        {
            return Err(RequestError::Header(HeaderError::Reserved));
        }
        self.headers
            .append(name, value)
            .map_err(RequestError::Header)?;
        Ok(self)
    }

    /// Replace the body. It is sent with a `Content-Length`.
    pub fn set_body(&mut self, body: Vec<u8>) -> &mut Self {
        self.body = body;
        self
    }

    /// Method, header fields and body for redirect rewriting
    /// (`redirect::rewrite_for_redirect`). The method stays a token.
    pub(crate) fn parts_mut(&mut self) -> (&mut String, &mut Headers, &mut Vec<u8>) {
        (&mut self.method, &mut self.headers, &mut self.body)
    }

    /// Point the request at a redirect target.
    pub(crate) fn set_url(&mut self, url: Url) {
        self.url = url;
    }

    /// The method as given.
    pub fn method(&self) -> &str {
        &self.method
    }

    /// The target URL.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Caller-supplied header fields.
    pub fn headers(&self) -> &Headers {
        &self.headers
    }

    /// The body.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub(crate) fn is_head(&self) -> bool {
        self.method == "HEAD"
    }

    /// The request-target in origin form (RFC 9112 §3.2.1): path and query,
    /// never the fragment.
    pub(crate) fn target(&self) -> String {
        let mut target = self.url.pathname();
        if target.is_empty() {
            target.push('/');
        }
        if let Some(query) = self.url.query() {
            target.push('?');
            target.push_str(query);
        }
        target
    }

    /// The `Host` field value: the host plus any non-default port.
    pub(crate) fn host_field(&self) -> String {
        let mut host = self
            .url
            .host()
            .map(|host| host.serialize())
            .unwrap_or_default();
        if let Some(port) = self.url.port() {
            host.push(':');
            host.push_str(&port.to_string());
        }
        host
    }

    /// Serialize the whole message. `Connection: close` is sent until
    /// connection reuse exists (TD-156).
    pub(crate) fn serialize(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(256 + self.body.len());
        output.extend_from_slice(self.method.as_bytes());
        output.push(b' ');
        output.extend_from_slice(self.target().as_bytes());
        output.extend_from_slice(b" HTTP/1.1\r\nHost: ");
        output.extend_from_slice(self.host_field().as_bytes());
        output.extend_from_slice(b"\r\n");
        for (name, value) in self.headers.iter() {
            output.extend_from_slice(name.as_bytes());
            output.extend_from_slice(b": ");
            output.extend_from_slice(value);
            output.extend_from_slice(b"\r\n");
        }
        // RFC 9110 §8.6: a user agent sends Content-Length for a body, and
        // for methods whose requests usually carry one.
        if !self.body.is_empty() || matches!(self.method.as_str(), "POST" | "PUT" | "PATCH") {
            output.extend_from_slice(b"Content-Length: ");
            output.extend_from_slice(self.body.len().to_string().as_bytes());
            output.extend_from_slice(b"\r\n");
        }
        output.extend_from_slice(b"Connection: close\r\n\r\n");
        output.extend_from_slice(&self.body);
        output
    }
}

impl fmt::Debug for Request {
    /// The method and scheme only; the URL and body can be private.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Request")
            .field("method", &self.method)
            .field("scheme", &self.url.scheme())
            .field("headers", &self.headers)
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// A request could not be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {
    /// The method is not a token.
    InvalidMethod,
    /// The scheme is neither `http` nor `https`.
    UnsupportedScheme,
    /// The URL has no host.
    MissingHost,
    /// A header field was refused.
    Header(HeaderError),
}

impl fmt::Display for RequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMethod => formatter.write_str("invalid request method"),
            Self::UnsupportedScheme => formatter.write_str("only http and https can be requested"),
            Self::MissingHost => formatter.write_str("request URL has no host"),
            Self::Header(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RequestError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(input: &str) -> Url {
        Url::parse(input, None).unwrap()
    }

    #[test]
    fn serializes_origin_form_target_host_and_framing() {
        let mut request = Request::new(
            "POST",
            url("http://user:pw@example.test:8080/a%20b?q=1#frag"),
        )
        .unwrap();
        request.header("Accept", b"text/html").unwrap();
        request.set_body(b"hello".to_vec());
        let text = String::from_utf8(request.serialize()).unwrap();
        assert_eq!(
            text,
            "POST /a%20b?q=1 HTTP/1.1\r\nHost: example.test:8080\r\nAccept: text/html\r\n\
             Content-Length: 5\r\nConnection: close\r\n\r\nhello"
        );
        let get = Request::get(url("http://example.test")).unwrap();
        assert_eq!(
            String::from_utf8(get.serialize()).unwrap(),
            "GET / HTTP/1.1\r\nHost: example.test\r\nConnection: close\r\n\r\n"
        );
        let empty_post = Request::new("POST", url("http://[::1]:80/")).unwrap();
        assert!(
            String::from_utf8(empty_post.serialize())
                .unwrap()
                .contains("Host: [::1]\r\nContent-Length: 0\r\n")
        );
    }

    #[test]
    fn refuses_bad_methods_schemes_and_framing_headers() {
        assert_eq!(
            Request::new("GE T", url("http://a.test")).unwrap_err(),
            RequestError::InvalidMethod
        );
        assert_eq!(
            Request::get(url("ftp://a.test")).unwrap_err(),
            RequestError::UnsupportedScheme
        );
        let mut request = Request::get(url("http://a.test")).unwrap();
        for name in [
            "Host",
            "content-LENGTH",
            "Transfer-Encoding",
            "Connection",
            "TE",
        ] {
            assert_eq!(
                request.header(name, b"x").unwrap_err(),
                RequestError::Header(HeaderError::Reserved)
            );
        }
        assert_eq!(
            request.header("X-A", b"a\r\nInjected: 1").unwrap_err(),
            RequestError::Header(HeaderError::InvalidValue)
        );
        assert!(request.headers().is_empty());
        assert_eq!(
            format!("{request:?}"),
            r#"Request { method: "GET", scheme: "http", headers: [], body_len: 0 }"#
        );
    }
}
