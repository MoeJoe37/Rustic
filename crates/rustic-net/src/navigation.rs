//! Navigation (HTML §7.4 "navigate", the fetch and response parts): a
//! state machine that turns a [`NavigationRequest`] into a committed
//! document, a download or nothing (204/205), following redirects one hop
//! at a time over a [`Fetcher`], with cancellation between hops.
//!
//! It also decides what the new document inherits (HTML "determining the
//! origin", "determining navigation params policy container"):
//! `about:blank` and `about:srcdoc` take the initiator's origin and a copy
//! of its policy container, `data:` gets an opaque origin, and an HTTP(S)
//! response gets its URL's origin with a policy container built from its
//! headers. Session history lives in the engine; the
//! [`CommittedNavigation`] is what a history entry records.

use std::fmt;
use std::time::Duration;

use rustic_base::CancellationToken;
use rustic_encoding::Encoding;
use rustic_url::{Origin, Url};

use crate::cors::CredentialsMode;
use crate::data_url::process_data_url;
use crate::fetch::{Destination, FetchRequest, Fetcher, NetworkError, ResponseType, read_all};
use crate::mime::{MimeType, extract_mime_type, sniff_unknown};
use crate::redirect::{
    MAX_REDIRECTS, RedirectError, RedirectMode, location_url, rewrite_for_redirect,
};
use crate::referrer::ReferrerPolicy;
use crate::{Exchange, Headers, NetEvent};

/// HTML "policy container": the policies a document carries and passes to
/// documents that inherit them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PolicyContainer {
    /// The document's referrer policy.
    pub referrer_policy: ReferrerPolicy,
    /// `Content-Security-Policy` header values, in order (enforced by
    /// [`crate::csp`]).
    pub csp: Vec<String>,
    /// `Content-Security-Policy-Report-Only` values (reported, not
    /// enforced).
    pub csp_report_only: Vec<String>,
}

impl PolicyContainer {
    /// HTML "create a policy container from a fetch response".
    pub fn from_response(headers: &Headers) -> Self {
        let text = |value: &[u8]| {
            value
                .iter()
                .map(|&byte| char::from(byte))
                .collect::<String>()
        };
        Self {
            referrer_policy: ReferrerPolicy::from_headers(headers).unwrap_or_default(),
            csp: headers
                .get_all("content-security-policy")
                .map(text)
                .collect(),
            csp_report_only: headers
                .get_all("content-security-policy-report-only")
                .map(text)
                .collect(),
        }
    }
}

/// The document or browser UI that started a navigation.
#[derive(Clone, Debug)]
pub struct Initiator {
    /// The initiator document's origin.
    pub origin: Origin,
    /// The initiator document's URL (the referrer).
    pub url: Url,
    /// The initiator document's policy container.
    pub policy: PolicyContainer,
}

/// What to navigate to and how.
#[derive(Clone, Debug)]
pub struct NavigationRequest {
    /// The URL.
    pub url: Url,
    /// `GET` or `POST`.
    pub method: String,
    /// The request body (form `POST`).
    pub body: Vec<u8>,
    /// The body's `Content-Type`.
    pub content_type: Option<String>,
    /// `None` for a browser-initiated navigation (address bar, bookmark).
    pub initiator: Option<Initiator>,
    /// Whether user activation caused the navigation.
    pub user_activation: bool,
    /// The `srcdoc` text for `about:srcdoc`.
    pub srcdoc: Option<String>,
}

impl NavigationRequest {
    /// A browser-initiated `GET` of `url`.
    pub fn browser(url: Url) -> Self {
        Self {
            url,
            method: "GET".to_owned(),
            body: Vec::new(),
            content_type: None,
            initiator: None,
            user_activation: true,
            srcdoc: None,
        }
    }

    /// HTML §4.10.21.3 "form submission algorithm" for the
    /// `application/x-www-form-urlencoded` enctype: `get` replaces the
    /// action URL's query, `post` sends the entries as the body. `action`
    /// is resolved against the document URL (an empty action is the
    /// document URL). `dialog` submissions do not navigate (`None`).
    pub fn form_submission(
        initiator: Initiator,
        method: &str,
        action: &str,
        entries: &[(String, String)],
        encoding: Encoding,
    ) -> Result<Option<Self>, NavigationError> {
        let action_url = if action.is_empty() {
            initiator.url.clone()
        } else {
            Url::parse(action, Some(&initiator.url)).map_err(|_| NavigationError::InvalidUrl)?
        };
        let serialized = rustic_url::serialize_form(entries, encoding);
        let mut request = Self {
            user_activation: true,
            ..Self::browser(action_url)
        };
        match method.to_ascii_lowercase().as_str() {
            "dialog" => return Ok(None),
            "post" if matches!(request.url.scheme(), "http" | "https") => {
                "POST".clone_into(&mut request.method);
                request.body = serialized.into_bytes();
                request.content_type = Some("application/x-www-form-urlencoded".to_owned());
            }
            _ => {
                let mut text = request.url.serialize_without_fragment();
                if let Some(index) = text.find('?') {
                    text.truncate(index);
                }
                text.push('?');
                text.push_str(&serialized);
                if let Some(fragment) = request.url.fragment() {
                    text.push('#');
                    text.push_str(fragment);
                }
                request.url = Url::parse(&text, None).map_err(|_| NavigationError::InvalidUrl)?;
            }
        }
        request.initiator = Some(initiator);
        Ok(Some(request))
    }
}

/// Why a navigation failed (an error page in a browser).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigationError {
    /// Fetching failed.
    Network(NetworkError),
    /// A URL could not be parsed.
    InvalidUrl,
    /// The scheme cannot be navigated to here (`javascript:` is M9 work,
    /// `file:` only through the CLI's local path).
    UnsupportedScheme,
    /// An initiator tried to navigate the top level to a `data:` URL, or a
    /// redirect led to one (blocked as in Chromium and Firefox).
    DataUrlBlocked,
    /// A `data:` URL failed the data URL processor.
    InvalidDataUrl,
    /// `about:srcdoc` without srcdoc text or an initiator.
    InvalidSrcdoc,
    /// [`Navigation::cancel`] was called.
    Cancelled,
}

impl fmt::Display for NavigationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(error) => error.fmt(formatter),
            Self::InvalidUrl => formatter.write_str("invalid URL"),
            Self::UnsupportedScheme => formatter.write_str("cannot navigate to this scheme"),
            Self::DataUrlBlocked => formatter.write_str("navigation to a data: URL is blocked"),
            Self::InvalidDataUrl => formatter.write_str("invalid data: URL"),
            Self::InvalidSrcdoc => formatter.write_str("about:srcdoc needs srcdoc text"),
            Self::Cancelled => formatter.write_str("navigation cancelled"),
        }
    }
}

impl std::error::Error for NavigationError {}

/// How the committed document is to be loaded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentKind {
    /// Parsed as HTML.
    Html,
    /// Shown as text (a `<pre>` document).
    Text,
}

/// The body of a committed document.
#[derive(Debug)]
pub enum DocumentBody {
    /// Bytes already in memory (`data:`, `about:`), or sniffed bytes
    /// followed by the rest of a network body.
    Bytes(Vec<u8>),
    /// A network body: `prefix` was read for sniffing, `exchange` has the
    /// rest.
    Network {
        /// Bytes read already.
        prefix: Vec<u8>,
        /// The remaining body.
        exchange: Exchange,
    },
}

impl DocumentBody {
    /// Read the whole body, waiting at most `timeout` per piece.
    pub fn read_all(self, timeout: Duration) -> Result<Vec<u8>, NetworkError> {
        match self {
            Self::Bytes(bytes) => Ok(bytes),
            Self::Network {
                mut prefix,
                exchange,
            } => {
                prefix.extend(read_all(&exchange, timeout)?);
                Ok(prefix)
            }
        }
    }
}

/// A navigation response that becomes a document.
#[derive(Debug)]
pub struct CommittedNavigation {
    /// Every URL fetched; the document URL last.
    pub url_list: Vec<Url>,
    /// The HTTP status (200 for local schemes).
    pub status: u16,
    /// The new document's origin.
    pub origin: Origin,
    /// The new document's policy container.
    pub policy: PolicyContainer,
    /// The MIME type the document is loaded as.
    pub mime: MimeType,
    /// HTML or text.
    pub kind: DocumentKind,
    /// The transport encoding from `Content-Type`'s `charset`.
    pub encoding: Option<Encoding>,
    /// Response headers.
    pub headers: Headers,
    /// The body.
    pub body: DocumentBody,
}

impl CommittedNavigation {
    /// The document URL.
    pub fn url(&self) -> &Url {
        self.url_list.last().expect("non-empty")
    }
}

/// A response handed to the download broker instead of a document. The
/// broker (a host service after M7) decides where bytes go; the engine
/// never writes files itself.
#[derive(Debug)]
pub struct Download {
    /// The response URL.
    pub url: Url,
    /// The response MIME type, when there was one.
    pub mime: Option<MimeType>,
    /// The filename from `Content-Disposition`, unsanitized.
    pub filename: Option<String>,
    /// The body still to be read.
    pub body: DocumentBody,
}

impl Download {
    /// A filename safe for any file system: path separators, controls and
    /// reserved characters replaced, leading dots and trailing dots and
    /// spaces trimmed, at most 255 bytes, `download` when nothing is left.
    pub fn sanitized_filename(&self) -> String {
        let source = self.filename.clone().unwrap_or_else(|| {
            self.url
                .pathname()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned()
        });
        // Keep only the last path component, as browsers do for both
        // separators, so a suggested name never names a directory.
        let source = source
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_owned();
        let mut name: String = source
            .chars()
            .map(|character| {
                if character.is_control() || "/\\:*?\"<>|".contains(character) {
                    '_'
                } else {
                    character
                }
            })
            .collect();
        name = name
            .trim_start_matches('.')
            .trim_end_matches(['.', ' '])
            .trim_start()
            .to_owned();
        while name.len() > 255 {
            name.pop();
        }
        if name.is_empty() {
            "download".to_owned()
        } else {
            name
        }
    }
}

/// What a navigation ended in.
#[derive(Debug)]
pub enum NavigationOutcome {
    /// Create and load a new document.
    Commit(Box<CommittedNavigation>),
    /// Hand the response to the download broker.
    Download(Box<Download>),
    /// 204 or 205: stay on the current document.
    NoContent,
}

/// The state of a [`Navigation`].
#[derive(Debug, Eq, PartialEq)]
pub enum NavigationState {
    /// Created, nothing fetched.
    Pending,
    /// This many redirects followed; the next hop is ready.
    Redirected(usize),
    /// Finished (committed, downloaded, no content or failed).
    Done,
}

/// What one [`Navigation::step`] did.
#[derive(Debug)]
pub enum NavigationStep {
    /// A redirect was followed; call `step` again.
    Redirected(Url),
    /// The navigation finished.
    Finished(NavigationOutcome),
}

/// One navigation in progress.
#[derive(Debug)]
pub struct Navigation {
    request: NavigationRequest,
    url_list: Vec<Url>,
    referrer_policy: ReferrerPolicy,
    state: NavigationState,
    cancel: CancellationToken,
    head_timeout: Duration,
}

impl Navigation {
    /// Start navigating. `head_timeout` bounds waiting for sniffing bytes.
    pub fn new(request: NavigationRequest, head_timeout: Duration) -> Self {
        let referrer_policy = request
            .initiator
            .as_ref()
            .map(|initiator| initiator.policy.referrer_policy)
            .unwrap_or_default();
        Self {
            url_list: vec![request.url.clone()],
            request,
            referrer_policy,
            state: NavigationState::Pending,
            cancel: CancellationToken::default(),
            head_timeout,
        }
    }

    /// The current state.
    pub fn state(&self) -> &NavigationState {
        &self.state
    }

    /// The URLs so far, the next to fetch last.
    pub fn url_list(&self) -> &[Url] {
        &self.url_list
    }

    /// A token that cancels this navigation when triggered (from another
    /// thread, or by a newer navigation of the same navigable).
    pub fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Cancel: the next `step` fails with [`NavigationError::Cancelled`].
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Run to the end.
    pub fn run(mut self, fetcher: &mut Fetcher) -> Result<NavigationOutcome, NavigationError> {
        loop {
            match self.step(fetcher)? {
                NavigationStep::Redirected(_) => {}
                NavigationStep::Finished(outcome) => return Ok(outcome),
            }
        }
    }

    /// Fetch the current URL once.
    pub fn step(&mut self, fetcher: &mut Fetcher) -> Result<NavigationStep, NavigationError> {
        if self.state == NavigationState::Done {
            return Err(NavigationError::Cancelled);
        }
        let result = self.step_inner(fetcher);
        if !matches!(result, Ok(NavigationStep::Redirected(_))) {
            self.state = NavigationState::Done;
        }
        result
    }

    fn step_inner(&mut self, fetcher: &mut Fetcher) -> Result<NavigationStep, NavigationError> {
        if self.cancel.is_cancelled() {
            return Err(NavigationError::Cancelled);
        }
        let url = self.url_list.last().expect("non-empty").clone();
        match url.scheme() {
            "http" | "https" => {}
            "about" | "data" if self.url_list.len() > 1 => {
                return Err(NavigationError::DataUrlBlocked);
            }
            "about" => return self.local_about(&url).map(NavigationStep::Finished),
            "data" => return self.local_data(&url).map(NavigationStep::Finished),
            _ => return Err(NavigationError::UnsupportedScheme),
        }
        let initiator = self.request.initiator.as_ref();
        let mut fetch = FetchRequest::navigation(url.clone());
        fetch.method.clone_from(&self.request.method);
        fetch.body.clone_from(&self.request.body);
        fetch.user_activation = self.request.user_activation;
        fetch.referrer_policy = self.referrer_policy;
        fetch.credentials = CredentialsMode::Include;
        fetch.redirect = RedirectMode::Manual;
        fetch.destination = Destination::Document;
        if let Some(content_type) = &self.request.content_type {
            fetch
                .headers
                .append("Content-Type", content_type.as_bytes())
                .map_err(|_| NavigationError::InvalidUrl)?;
        }
        if let Some(initiator) = initiator {
            fetch.origin = initiator.origin.clone();
            fetch.referrer = Some(initiator.url.clone());
            fetch.site_for_cookies = Some(initiator.origin.clone());
        }
        let response = fetcher.fetch(fetch).map_err(NavigationError::Network)?;
        if self.cancel.is_cancelled() {
            return Err(NavigationError::Cancelled);
        }
        if response.response_type() == ResponseType::OpaqueRedirect {
            let internal = response.into_internal();
            let location = location_url(&internal.head, &url)
                .map_err(|error| NavigationError::Network(NetworkError::Redirect(error)))?
                .expect("an opaque redirect has a location");
            if self.url_list.len() > MAX_REDIRECTS {
                return Err(NavigationError::Network(NetworkError::Redirect(
                    RedirectError::TooManyRedirects,
                )));
            }
            if !matches!(location.scheme(), "http" | "https") {
                return Err(if location.scheme() == "data" {
                    NavigationError::DataUrlBlocked
                } else {
                    NavigationError::Network(NetworkError::Redirect(
                        RedirectError::UnsupportedScheme,
                    ))
                });
            }
            if let Some(policy) = ReferrerPolicy::from_headers(internal.head.headers()) {
                self.referrer_policy = policy;
            }
            let mut headers = Headers::new();
            if let Some(content_type) = &self.request.content_type {
                headers
                    .append("Content-Type", content_type.as_bytes())
                    .map_err(|_| NavigationError::InvalidUrl)?;
            }
            let cross_origin = !url.origin().is_same_origin(&location.origin());
            rewrite_for_redirect(
                internal.head.status(),
                cross_origin,
                &mut self.request.method,
                &mut headers,
                &mut self.request.body,
            );
            if !headers.contains("content-type") {
                self.request.content_type = None;
            }
            self.url_list.push(location.clone());
            self.state = NavigationState::Redirected(self.url_list.len() - 1);
            return Ok(NavigationStep::Redirected(location));
        }
        let internal = response.into_internal();
        let status = internal.head.status();
        if matches!(status, 204 | 205) {
            return Ok(NavigationStep::Finished(NavigationOutcome::NoContent));
        }
        let headers = internal.head.headers().clone();
        let nosniff = headers
            .get("x-content-type-options")
            .is_some_and(|value| value.len() >= 7 && value[..7].eq_ignore_ascii_case(b"nosniff"));
        let mut prefix = Vec::new();
        let mut finished = false;
        let supplied = extract_mime_type(&headers);
        let mime = match supplied.clone() {
            Some(mime) => mime,
            None if nosniff => MimeType::parse("text/plain").expect("valid"),
            None => {
                // Read up to the sniffing window, or the whole body if shorter.
                while prefix.len() < 1445 {
                    match internal.exchange.next_event(Some(self.head_timeout)) {
                        Some(NetEvent::Body(bytes)) => prefix.extend_from_slice(&bytes),
                        Some(NetEvent::Finished) => {
                            finished = true;
                            break;
                        }
                        None => break,
                        Some(NetEvent::Failed(error)) => {
                            return Err(NavigationError::Network(NetworkError::Transport(error)));
                        }
                        Some(NetEvent::Response(_)) => unreachable!("one head per exchange"),
                    }
                }
                MimeType::parse(sniff_unknown(&prefix)).expect("sniffed types are valid")
            }
        };
        let body = if finished {
            DocumentBody::Bytes(prefix)
        } else {
            DocumentBody::Network {
                prefix,
                exchange: internal.exchange,
            }
        };
        let attachment = headers
            .get("content-disposition")
            .is_some_and(|value| disposition(value).0);
        let kind = match mime.essence().as_str() {
            _ if attachment => None,
            "text/html" => Some(DocumentKind::Html),
            "text/plain" | "text/css" | "text/javascript" | "application/json" => {
                Some(DocumentKind::Text)
            }
            _ => None,
        };
        let Some(kind) = kind else {
            return Ok(NavigationStep::Finished(NavigationOutcome::Download(
                Box::new(Download {
                    url: internal.url_list.last().expect("non-empty").clone(),
                    mime: supplied,
                    filename: headers
                        .get("content-disposition")
                        .and_then(|value| disposition(value).1),
                    body,
                }),
            )));
        };
        let policy = PolicyContainer::from_response(&headers);
        let url_list = self.url_list.clone();
        let origin = if crate::csp::sandboxes_origin(&policy.csp) {
            Origin::opaque()
        } else {
            url.origin()
        };
        Ok(NavigationStep::Finished(NavigationOutcome::Commit(
            Box::new(CommittedNavigation {
                url_list,
                status,
                origin,
                encoding: mime.charset(),
                policy,
                mime,
                kind,
                headers,
                body,
            }),
        )))
    }

    /// `about:blank` and `about:srcdoc` inherit from the initiator.
    fn local_about(&self, url: &Url) -> Result<NavigationOutcome, NavigationError> {
        let initiator = self.request.initiator.as_ref();
        let (body, origin, policy) = match url.serialize_without_fragment().as_str() {
            "about:blank" => (
                Vec::new(),
                initiator.map_or_else(Origin::opaque, |initiator| initiator.origin.clone()),
                initiator
                    .map(|initiator| initiator.policy.clone())
                    .unwrap_or_default(),
            ),
            "about:srcdoc" => {
                let (Some(initiator), Some(srcdoc)) = (initiator, &self.request.srcdoc) else {
                    return Err(NavigationError::InvalidSrcdoc);
                };
                (
                    srcdoc.clone().into_bytes(),
                    initiator.origin.clone(),
                    initiator.policy.clone(),
                )
            }
            _ => return Err(NavigationError::UnsupportedScheme),
        };
        Ok(NavigationOutcome::Commit(Box::new(CommittedNavigation {
            url_list: self.url_list.clone(),
            status: 200,
            origin,
            policy,
            mime: MimeType::parse("text/html;charset=utf-8").expect("valid"),
            kind: DocumentKind::Html,
            encoding: Some(Encoding::Utf8),
            headers: Headers::new(),
            body: DocumentBody::Bytes(body),
        })))
    }

    /// `data:` documents: only browser-initiated at the top level; opaque
    /// origin; policy container copied from the initiator.
    fn local_data(&self, url: &Url) -> Result<NavigationOutcome, NavigationError> {
        if self.request.initiator.is_some() {
            return Err(NavigationError::DataUrlBlocked);
        }
        let data = process_data_url(url).ok_or(NavigationError::InvalidDataUrl)?;
        let kind = match data.mime.essence().as_str() {
            "text/html" => DocumentKind::Html,
            "text/plain" => DocumentKind::Text,
            _ => {
                return Ok(NavigationOutcome::Download(Box::new(Download {
                    url: url.clone(),
                    mime: Some(data.mime),
                    filename: None,
                    body: DocumentBody::Bytes(data.body),
                })));
            }
        };
        Ok(NavigationOutcome::Commit(Box::new(CommittedNavigation {
            url_list: self.url_list.clone(),
            status: 200,
            origin: Origin::opaque(),
            policy: PolicyContainer::default(),
            encoding: data.mime.charset(),
            mime: data.mime,
            kind,
            headers: Headers::new(),
            body: DocumentBody::Bytes(data.body),
        })))
    }
}

/// RFC 6266 `Content-Disposition`: whether it is `attachment`, and the
/// filename (`filename*` with UTF-8 or ISO-8859-1 preferred over
/// `filename`).
pub fn disposition(value: &[u8]) -> (bool, Option<String>) {
    let text: String = value.iter().map(|&byte| char::from(byte)).collect();
    let mut parts = text.split(';');
    let kind = parts.next().unwrap_or_default().trim();
    let attachment = kind.eq_ignore_ascii_case("attachment");
    let mut plain = None;
    let mut extended = None;
    for part in parts {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        if name == "filename*" {
            extended = decode_ext_value(value);
        } else if name == "filename" {
            plain = Some(
                value
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .unwrap_or(value)
                    .replace("\\\"", "\""),
            );
        }
    }
    (attachment, extended.or(plain))
}

/// RFC 8187 `ext-value`: `charset'language'percent-encoded`.
fn decode_ext_value(value: &str) -> Option<String> {
    let mut pieces = value.splitn(3, '\'');
    let charset = pieces.next()?.to_ascii_lowercase();
    let _language = pieces.next()?;
    let encoded = pieces.next()?;
    let hex = |byte: u8| char::from(byte).to_digit(16);
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes.get(index + 1).copied().and_then(hex)?;
            let low = bytes.get(index + 2).copied().and_then(hex)?;
            decoded.push(u8::try_from(high * 16 + low).expect("two hex digits"));
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    match charset.as_str() {
        "utf-8" => String::from_utf8(decoded).ok(),
        "iso-8859-1" => Some(decoded.iter().map(|&byte| char::from(byte)).collect()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_disposition() {
        assert_eq!(disposition(b"inline"), (false, None));
        assert_eq!(
            disposition(b"Attachment; filename=\"a b.txt\""),
            (true, Some("a b.txt".into()))
        );
        assert_eq!(
            disposition(b"attachment; filename=plain.txt; filename*=UTF-8''%E2%82%AC%20rates.txt"),
            (true, Some("€ rates.txt".into()))
        );
        assert_eq!(
            disposition(b"attachment; filename*=iso-8859-1'en'%A3.txt"),
            (true, Some("£.txt".into()))
        );
        assert_eq!(
            disposition(b"attachment; filename*=bogus''x; filename=y"),
            (true, Some("y".into()))
        );
    }

    #[test]
    fn download_names_are_sanitized() {
        let download = |filename: Option<&str>, url: &str| Download {
            url: Url::parse(url, None).unwrap(),
            mime: None,
            filename: filename.map(str::to_owned),
            body: DocumentBody::Bytes(Vec::new()),
        };
        assert_eq!(
            download(Some("../../etc/passwd"), "http://a.test/").sanitized_filename(),
            "passwd"
        );
        assert_eq!(
            download(Some("..."), "http://a.test/").sanitized_filename(),
            "download"
        );
        assert_eq!(
            download(None, "http://a.test/dir/report.pdf").sanitized_filename(),
            "report.pdf"
        );
        assert_eq!(
            download(None, "http://a.test/").sanitized_filename(),
            "download"
        );
        assert_eq!(
            download(Some("a\u{0}b:c. "), "http://a.test/").sanitized_filename(),
            "a_b_c"
        );
        assert_eq!(
            download(Some(&"x".repeat(300)), "http://a.test/")
                .sanitized_filename()
                .len(),
            255
        );
    }
}
