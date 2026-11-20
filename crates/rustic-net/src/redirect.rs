//! Fetch's HTTP-redirect handling (Fetch §4.4 "HTTP-redirect fetch") as a
//! state machine without I/O, plus a blocking driver over [`Transport`].
//!
//! Each hop is a fresh [`Request`] that goes back through
//! [`Transport::start`], so the [`NetworkPolicy`](crate::NetworkPolicy) is
//! checked again for every URL in the chain.

use std::fmt;
use std::time::Duration;

use rustic_url::Url;

use crate::{Exchange, Headers, NetEvent, Request, ResponseHead, Transport, TransportError};

/// Fetch's limit: a twenty-first redirect is a network error.
pub const MAX_REDIRECTS: usize = 20;

/// Request header names removed when a redirect turns the request into a
/// `GET` without a body (Fetch "request-body-header name").
const REQUEST_BODY_HEADERS: [&str; 4] = [
    "content-encoding",
    "content-language",
    "content-location",
    "content-type",
];

/// What to do with a redirect response (Fetch request "redirect mode").
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RedirectMode {
    /// Follow redirects.
    #[default]
    Follow,
    /// A redirect is a network error.
    Error,
    /// Return the redirect response itself to the caller.
    Manual,
}

/// Why a redirect chain failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RedirectError {
    /// More than [`MAX_REDIRECTS`] redirects.
    TooManyRedirects,
    /// `Location` is not a valid URL relative to the response URL, or
    /// appears more than once with different values.
    InvalidLocation,
    /// The redirect target is not `http` or `https`.
    UnsupportedScheme,
    /// The request mode forbids redirects.
    RedirectForbidden,
}

impl fmt::Display for RedirectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TooManyRedirects => "too many redirects",
            Self::InvalidLocation => "invalid redirect location",
            Self::UnsupportedScheme => "redirect to a scheme other than http or https",
            Self::RedirectForbidden => "redirect not allowed by the request's redirect mode",
        })
    }
}

impl std::error::Error for RedirectError {}

/// The next step after a response head.
#[derive(Debug)]
pub enum RedirectStep {
    /// Not a redirect (or a manual-mode redirect): this is the response.
    Final,
    /// Discard the current response and send [`RedirectChain::request`],
    /// which now targets the redirect location.
    Follow,
}

/// The state of one request's redirect chain.
#[derive(Debug)]
pub struct RedirectChain {
    request: Request,
    mode: RedirectMode,
    url_list: Vec<Url>,
}

impl RedirectChain {
    /// Start a chain for `request`.
    pub fn new(request: Request, mode: RedirectMode) -> Self {
        let url_list = vec![request.url().clone()];
        Self {
            request,
            mode,
            url_list,
        }
    }

    /// The request to send now.
    pub fn request(&self) -> &Request {
        &self.request
    }

    /// Every URL requested so far, the current one last (Fetch "URL list").
    pub fn url_list(&self) -> &[Url] {
        &self.url_list
    }

    /// Redirects followed so far.
    pub fn redirect_count(&self) -> usize {
        self.url_list.len() - 1
    }

    /// Decide what to do with the response to [`Self::request`].
    pub fn on_response(&mut self, head: &ResponseHead) -> Result<RedirectStep, RedirectError> {
        let Some(location) = location_url(head, self.request.url())? else {
            return Ok(RedirectStep::Final);
        };
        match self.mode {
            RedirectMode::Manual => return Ok(RedirectStep::Final),
            RedirectMode::Error => return Err(RedirectError::RedirectForbidden),
            RedirectMode::Follow => {}
        }
        if !matches!(location.scheme(), "http" | "https") {
            return Err(RedirectError::UnsupportedScheme);
        }
        if self.redirect_count() == MAX_REDIRECTS {
            return Err(RedirectError::TooManyRedirects);
        }
        let mut next = self.request.clone();
        let cross_origin = !self
            .request
            .url()
            .origin()
            .is_same_origin(&location.origin());
        let (method, headers, body) = next.parts_mut();
        rewrite_for_redirect(head.status(), cross_origin, method, headers, body);
        next.set_url(location.clone());
        self.url_list.push(location);
        self.request = next;
        Ok(RedirectStep::Follow)
    }
}

/// Fetch §4.4 steps 12–13 on a request being redirected: `POST` after 301
/// or 302 and anything but `GET`/`HEAD` after 303 become a bodiless `GET`
/// without request-body headers, and a cross-origin hop drops
/// `Authorization` (Fetch "CORS non-wildcard request-header name").
pub(crate) fn rewrite_for_redirect(
    status: u16,
    cross_origin: bool,
    method: &mut String,
    headers: &mut Headers,
    body: &mut Vec<u8>,
) {
    if (matches!(status, 301 | 302) && method == "POST")
        || (status == 303 && !matches!(method.as_str(), "GET" | "HEAD"))
    {
        "GET".clone_into(method);
        body.clear();
        for name in REQUEST_BODY_HEADERS {
            headers.remove(name);
        }
    }
    if cross_origin {
        headers.remove("authorization");
    }
}

/// Fetch "location URL": `None` when the status is not a redirect or there
/// is no `Location`; the request URL's fragment is kept when the location
/// has none.
pub(crate) fn location_url(
    head: &ResponseHead,
    current: &Url,
) -> Result<Option<Url>, RedirectError> {
    if !matches!(head.status(), 301 | 302 | 303 | 307 | 308) {
        return Ok(None);
    }
    let mut values = head.headers().get_all("location");
    let Some(first) = values.next() else {
        return Ok(None);
    };
    if values.any(|other| other != first) {
        return Err(RedirectError::InvalidLocation);
    }
    // Header bytes are isomorphic-decoded; the URL parser percent-encodes.
    let text: String = first.iter().map(|&byte| char::from(byte)).collect();
    let location = Url::parse(&text, Some(current)).map_err(|_| RedirectError::InvalidLocation)?;
    match (location.fragment(), current.fragment()) {
        (None, Some(fragment)) => Url::parse(&format!("{}#{fragment}", location.serialize()), None)
            .map(Some)
            .map_err(|_| RedirectError::InvalidLocation),
        _ => Ok(Some(location)),
    }
}

/// A response after its redirect chain.
#[derive(Debug)]
pub struct Fetched {
    /// Every URL requested, the response's URL last.
    pub url_list: Vec<Url>,
    /// The final response head.
    pub head: ResponseHead,
    /// The final exchange, positioned after its head; read the body with
    /// [`Exchange::next_event`].
    pub exchange: Exchange,
}

/// Why [`fetch_following_redirects`] failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FetchError {
    /// A hop failed in the transport (including a policy block).
    Transport(TransportError),
    /// The redirect chain itself was invalid.
    Redirect(RedirectError),
    /// A hop's response head did not arrive within `head_timeout`.
    HeadTimeout,
}

impl fmt::Display for FetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => error.fmt(formatter),
            Self::Redirect(error) => error.fmt(formatter),
            Self::HeadTimeout => formatter.write_str("no response head before the timeout"),
        }
    }
}

impl std::error::Error for FetchError {}

/// Run a request and its redirects, blocking up to `head_timeout` for each
/// hop's response head. Redirect bodies are discarded by dropping their
/// exchanges. Event-driven callers use [`RedirectChain`] directly.
pub fn fetch_following_redirects(
    transport: &Transport,
    request: Request,
    mode: RedirectMode,
    head_timeout: Duration,
) -> Result<Fetched, FetchError> {
    let mut chain = RedirectChain::new(request, mode);
    loop {
        let exchange = transport
            .start(chain.request().clone())
            .map_err(FetchError::Transport)?;
        let head = match exchange.next_event(Some(head_timeout)) {
            Some(NetEvent::Response(head)) => head,
            Some(NetEvent::Failed(error)) => return Err(FetchError::Transport(error)),
            Some(NetEvent::Body(_) | NetEvent::Finished) => {
                unreachable!("an exchange reports its head before any body or end")
            }
            None => return Err(FetchError::HeadTimeout),
        };
        match chain.on_response(&head).map_err(FetchError::Redirect)? {
            RedirectStep::Final => {
                return Ok(Fetched {
                    url_list: chain.url_list,
                    head,
                    exchange,
                });
            }
            RedirectStep::Follow => drop(exchange),
        }
    }
}
