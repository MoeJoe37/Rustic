//! Fetch's main fetch for `http`/`https` over [`Transport`] (Fetch §4.1
//! "main fetch", §4.3 "HTTP fetch", §4.4 "HTTP-redirect fetch", §4.5
//! "HTTP-network-or-cache fetch" without the HTTP cache).
//!
//! [`Fetcher`] owns the [`CookieJar`] and [`PreflightCache`] of one
//! profile and runs a [`FetchRequest`] to a [`FetchResponse`]: request
//! modes, response tainting, the `Origin` header, credentials and cookies,
//! CORS-preflight requests, the CORS check on every hop, redirect
//! tainting, the Cross-Origin-Resource-Policy check and filtered responses.
//! A script-facing binding exposes only the filtered view; the engine may
//! read an opaque response's body through [`FetchResponse::into_internal`]
//! for rendering (images, no-CORS scripts), never for script.
//!
//! Every hop goes back through [`Transport::start`], so the
//! [`NetworkPolicy`](crate::NetworkPolicy) is checked for each URL.

use std::fmt;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustic_url::{Origin, Url};

use crate::cookie::{CookieContext, CookieJar};
use crate::cors::{
    CorsError, CredentialsMode, PreflightCache, cors_check, cors_unsafe_request_header_names,
    exposed_header_names, is_cors_safelisted_method, is_cors_safelisted_response_header_name,
    is_forbidden_method, is_forbidden_response_header_name, normalize_method, preflight_headers,
    validate_preflight,
};
use crate::redirect::{
    MAX_REDIRECTS, RedirectError, RedirectMode, location_url, rewrite_for_redirect,
};
use crate::referrer::determine_referrer;
use crate::site::{PublicSuffixList, Site, is_potentially_trustworthy_url};
use crate::{
    Exchange, Headers, NetEvent, Request, RequestError, ResponseHead, Transport, TransportError,
};

/// Fetch request "mode".
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RequestMode {
    /// A navigation; responses are basic.
    Navigate,
    /// Cross-origin URLs are network errors.
    SameOrigin,
    /// Cross-origin responses are opaque.
    #[default]
    NoCors,
    /// Cross-origin responses need CORS.
    Cors,
}

/// Fetch "response tainting".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseTainting {
    /// Same-origin (or navigation).
    Basic,
    /// Cross-origin with CORS.
    Cors,
    /// Cross-origin without CORS.
    Opaque,
}

/// Fetch response "type".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseType {
    /// Same-origin: everything but `Set-Cookie` is visible.
    Basic,
    /// CORS: safelisted and exposed headers are visible.
    Cors,
    /// No-CORS cross-origin: nothing is visible.
    Opaque,
    /// A redirect returned in manual mode: nothing is visible.
    OpaqueRedirect,
}

pub use crate::referrer::ReferrerPolicy;

/// Fetch request "destination", as sent in `Sec-Fetch-Dest`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Destination {
    /// `fetch()`, XHR and other script-initiated requests.
    #[default]
    Empty,
    /// A top-level document.
    Document,
    /// A nested document.
    Iframe,
    /// A stylesheet.
    Style,
    /// A classic or module script.
    Script,
    /// An image.
    Image,
    /// A web font.
    Font,
}

impl Destination {
    /// The `Sec-Fetch-Dest` token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Document => "document",
            Self::Iframe => "iframe",
            Self::Style => "style",
            Self::Script => "script",
            Self::Image => "image",
            Self::Font => "font",
        }
    }
}

/// A Fetch request before it runs.
#[derive(Clone, Debug)]
pub struct FetchRequest {
    /// The method; the six standard methods are normalized to uppercase.
    pub method: String,
    /// The URL to fetch.
    pub url: Url,
    /// Author or engine header fields. The fetcher adds `Origin`,
    /// `Cookie` and a default `Accept`; callers apply header guards.
    pub headers: Headers,
    /// The request body.
    pub body: Vec<u8>,
    /// The request's origin (its client's origin).
    pub origin: Origin,
    /// The mode.
    pub mode: RequestMode,
    /// The credentials mode.
    pub credentials: CredentialsMode,
    /// The redirect mode.
    pub redirect: RedirectMode,
    /// The referrer policy (updated by a redirect's `Referrer-Policy`).
    pub referrer_policy: ReferrerPolicy,
    /// The referrer URL (the client's URL), or `None` for no referrer.
    pub referrer: Option<Url>,
    /// The destination.
    pub destination: Destination,
    /// Whether a navigation was caused by user activation (`Sec-Fetch-User`).
    pub user_activation: bool,
    /// Fetch "use-CORS-preflight flag" (upload listeners).
    pub use_cors_preflight: bool,
    /// The origin whose site is the request's "site for cookies"
    /// (RFC 6265bis §5.2); `None` for a browser-initiated navigation.
    pub site_for_cookies: Option<Origin>,
    /// The top-level site, for partitioned cookies; navigations use their
    /// own URL's site.
    pub top_level_site: Option<Site>,
}

impl FetchRequest {
    /// A `GET` with defaults: no-CORS, same-origin credentials, follow.
    pub fn new(url: Url, origin: Origin) -> Self {
        Self {
            method: "GET".to_owned(),
            url,
            headers: Headers::new(),
            body: Vec::new(),
            site_for_cookies: Some(origin.clone()),
            origin,
            mode: RequestMode::NoCors,
            credentials: CredentialsMode::SameOrigin,
            redirect: RedirectMode::Follow,
            referrer_policy: ReferrerPolicy::default(),
            referrer: None,
            destination: Destination::Empty,
            user_activation: false,
            use_cors_preflight: false,
            top_level_site: None,
        }
    }

    /// A browser-initiated top-level navigation to `url` (typed by the
    /// user: no referrer, `Sec-Fetch-Site: none`).
    pub fn navigation(url: Url) -> Self {
        Self {
            mode: RequestMode::Navigate,
            destination: Destination::Document,
            user_activation: true,
            credentials: CredentialsMode::Include,
            redirect: RedirectMode::Manual,
            site_for_cookies: None,
            ..Self::new(url, Origin::opaque())
        }
    }
}

/// Why a fetch is a network error. Messages carry no URL or content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkError {
    /// A hop failed in the transport, including a policy block.
    Transport(TransportError),
    /// The transport refused the request (bad method or header).
    Request(RequestError),
    /// The redirect chain was invalid.
    Redirect(RedirectError),
    /// A response head did not arrive within the head timeout.
    HeadTimeout,
    /// No body bytes arrived within the timeout.
    BodyTimeout,
    /// The method is `CONNECT`, `TRACE` or `TRACK`.
    ForbiddenMethod,
    /// The scheme is not `http` or `https`.
    UnsupportedScheme,
    /// A `same-origin` request reached another origin.
    CrossOriginSameOriginMode,
    /// A `no-cors` request with a redirect mode other than `follow`.
    NoCorsRedirectMode,
    /// The CORS check failed on a response.
    Cors(CorsError),
    /// The CORS-preflight request failed.
    Preflight(CorsError),
    /// A CORS redirect to a URL with credentials.
    RedirectWithCredentials,
    /// `Cross-Origin-Resource-Policy` blocked a no-CORS response.
    BlockedByResourcePolicy,
    /// The client's policy (CSP or mixed content) refused a hop.
    BlockedByClientPolicy,
}

impl fmt::Display for NetworkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => error.fmt(formatter),
            Self::Request(error) => error.fmt(formatter),
            Self::Redirect(error) => error.fmt(formatter),
            Self::Cors(error) | Self::Preflight(error) => error.fmt(formatter),
            Self::HeadTimeout => formatter.write_str("no response head before the timeout"),
            Self::BodyTimeout => formatter.write_str("no body progress before the timeout"),
            Self::ForbiddenMethod => formatter.write_str("forbidden request method"),
            Self::UnsupportedScheme => formatter.write_str("scheme cannot be fetched"),
            Self::CrossOriginSameOriginMode => {
                formatter.write_str("cross-origin request in same-origin mode")
            }
            Self::NoCorsRedirectMode => {
                formatter.write_str("no-cors requests must follow redirects")
            }
            Self::RedirectWithCredentials => {
                formatter.write_str("CORS redirect to a URL with credentials")
            }
            Self::BlockedByResourcePolicy => {
                formatter.write_str("blocked by Cross-Origin-Resource-Policy")
            }
            Self::BlockedByClientPolicy => {
                formatter.write_str("blocked by the document's content policy")
            }
        }
    }
}

impl std::error::Error for NetworkError {}

/// The unfiltered response, for engine-internal consumers.
#[derive(Debug)]
pub struct InternalResponse {
    /// Every URL fetched, the response's URL last.
    pub url_list: Vec<Url>,
    /// The response head as received.
    pub head: ResponseHead,
    /// The exchange, positioned after its head.
    pub exchange: Exchange,
}

/// A filtered Fetch response.
#[derive(Debug)]
pub struct FetchResponse {
    kind: ResponseType,
    headers: Headers,
    internal: InternalResponse,
}

impl FetchResponse {
    /// The response type.
    pub fn response_type(&self) -> ResponseType {
        self.kind
    }

    /// The status: `0` for opaque and opaque-redirect responses.
    pub fn status(&self) -> u16 {
        match self.kind {
            ResponseType::Opaque | ResponseType::OpaqueRedirect => 0,
            ResponseType::Basic | ResponseType::Cors => self.internal.head.status(),
        }
    }

    /// The status message, empty when opaque.
    pub fn status_message(&self) -> &[u8] {
        match self.kind {
            ResponseType::Opaque | ResponseType::OpaqueRedirect => b"",
            ResponseType::Basic | ResponseType::Cors => self.internal.head.reason(),
        }
    }

    /// The visible header fields.
    pub fn headers(&self) -> &Headers {
        &self.headers
    }

    /// The response URL; `None` when opaque.
    pub fn url(&self) -> Option<&Url> {
        match self.kind {
            ResponseType::Opaque => None,
            _ => self.internal.url_list.last(),
        }
    }

    /// Whether the response went through a redirect (visible unless
    /// opaque).
    pub fn redirected(&self) -> bool {
        self.kind != ResponseType::Opaque && self.internal.url_list.len() > 1
    }

    /// Read the whole visible body, waiting at most `timeout` per piece.
    /// Opaque responses have no visible body and yield an empty one
    /// without reading the network.
    pub fn read_body(self, timeout: Duration) -> Result<Vec<u8>, NetworkError> {
        match self.kind {
            ResponseType::Opaque | ResponseType::OpaqueRedirect => Ok(Vec::new()),
            ResponseType::Basic | ResponseType::Cors => read_all(&self.internal.exchange, timeout),
        }
    }

    /// The unfiltered response. Script-facing code must not use this.
    pub fn into_internal(self) -> InternalResponse {
        self.internal
    }
}

/// Read every remaining body event of `exchange`.
pub fn read_all(exchange: &Exchange, timeout: Duration) -> Result<Vec<u8>, NetworkError> {
    let mut body = Vec::new();
    loop {
        match exchange.next_event(Some(timeout)) {
            Some(NetEvent::Body(bytes)) => body.extend_from_slice(&bytes),
            Some(NetEvent::Finished) => return Ok(body),
            Some(NetEvent::Failed(error)) => return Err(NetworkError::Transport(error)),
            Some(NetEvent::Response(_)) => unreachable!("one head per exchange"),
            None => return Err(NetworkError::BodyTimeout),
        }
    }
}

/// Runs Fetch requests for one profile.
pub struct Fetcher {
    transport: Transport,
    cookies: CookieJar,
    preflight: PreflightCache,
    head_timeout: Duration,
    clock: Box<dyn Fn() -> i64 + Send>,
    accept_language: String,
}

impl fmt::Debug for Fetcher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Fetcher")
            .field("cookies", &self.cookies)
            .field("preflight", &self.preflight.len())
            .finish_non_exhaustive()
    }
}

fn system_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

/// State that changes along a redirect chain.
struct Hop {
    method: String,
    headers: Headers,
    body: Vec<u8>,
    url_list: Vec<Url>,
    tainted_origin: bool,
    tainting: ResponseTainting,
    same_site: bool,
    referrer_policy: ReferrerPolicy,
}

impl Fetcher {
    /// A fetcher with an empty cookie jar using `suffixes`, waiting at
    /// most `head_timeout` for each response head.
    pub fn new(transport: Transport, cookies: CookieJar, head_timeout: Duration) -> Self {
        Self {
            transport,
            cookies,
            preflight: PreflightCache::default(),
            head_timeout,
            clock: Box::new(system_seconds),
            accept_language: "en-US,en".to_owned(),
        }
    }

    /// Replace the `Accept-Language` value (default `en-US,en`, a value
    /// shared by many users rather than the system's list, for privacy).
    pub fn set_accept_language(&mut self, value: &str) {
        value.clone_into(&mut self.accept_language);
    }

    /// Replace the wall clock used for cookie expiry (Unix seconds).
    pub fn set_clock(&mut self, clock: impl Fn() -> i64 + Send + 'static) {
        self.clock = Box::new(clock);
    }

    /// The cookie jar.
    pub fn cookies(&self) -> &CookieJar {
        &self.cookies
    }

    /// The cookie jar, mutably (for `document.cookie` and profile
    /// management).
    pub fn cookies_mut(&mut self) -> &mut CookieJar {
        &mut self.cookies
    }

    /// The CORS-preflight cache.
    pub fn preflight_cache(&self) -> &PreflightCache {
        &self.preflight
    }

    fn suffixes(&self) -> &PublicSuffixList {
        self.cookies.suffixes()
    }

    /// Run `request` (Fetch "main fetch" and what it calls).
    pub fn fetch(&mut self, request: FetchRequest) -> Result<FetchResponse, NetworkError> {
        self.fetch_checked(request, &mut |_, _| true)
    }

    /// Like [`Self::fetch`], asking `allow(url, redirect_count)` before
    /// every hop, the first included, so a client's policies (CSP, mixed
    /// content) are rechecked after each redirect before anything is sent
    /// (Fetch main fetch step 7 and §4.4 step 20). A refusal is
    /// [`NetworkError::BlockedByClientPolicy`].
    pub fn fetch_checked(
        &mut self,
        request: FetchRequest,
        allow: &mut dyn FnMut(&Url, usize) -> bool,
    ) -> Result<FetchResponse, NetworkError> {
        let method = normalize_method(&request.method);
        if is_forbidden_method(&method) {
            return Err(NetworkError::ForbiddenMethod);
        }
        if request.mode == RequestMode::NoCors && request.redirect != RedirectMode::Follow {
            return Err(NetworkError::NoCorsRedirectMode);
        }
        let mut hop = Hop {
            method,
            headers: request.headers.clone(),
            body: request.body.clone(),
            url_list: vec![request.url.clone()],
            tainted_origin: false,
            tainting: ResponseTainting::Basic,
            same_site: true,
            referrer_policy: request.referrer_policy,
        };
        loop {
            let current = hop.url_list.last().expect("non-empty").clone();
            if !matches!(current.scheme(), "http" | "https") {
                return Err(NetworkError::UnsupportedScheme);
            }
            if !allow(&current, hop.url_list.len() - 1) {
                return Err(NetworkError::BlockedByClientPolicy);
            }
            hop.same_site &= request
                .site_for_cookies
                .as_ref()
                .is_none_or(|site| self.suffixes().is_same_site(site, &current.origin()));
            // Main fetch step 12.
            let same_origin = current.origin().is_same_origin(&request.origin)
                && hop.tainting == ResponseTainting::Basic;
            if !same_origin {
                match request.mode {
                    RequestMode::Navigate => {}
                    RequestMode::SameOrigin => return Err(NetworkError::CrossOriginSameOriginMode),
                    RequestMode::NoCors => hop.tainting = ResponseTainting::Opaque,
                    RequestMode::Cors => hop.tainting = ResponseTainting::Cors,
                }
            }
            let serialized_origin = if hop.tainted_origin {
                "null".to_owned()
            } else {
                request.origin.serialize()
            };
            if hop.tainting == ResponseTainting::Cors {
                self.preflight_if_needed(&request, &hop, &current, &serialized_origin)?;
            }
            let (head, exchange) =
                self.network_fetch(&request, &hop, &current, &serialized_origin)?;
            if hop.tainting == ResponseTainting::Cors {
                if let Err(error) =
                    cors_check(&serialized_origin, request.credentials, head.headers())
                {
                    self.preflight.clear_for(&serialized_origin, &current);
                    return Err(NetworkError::Cors(error));
                }
            }
            if request.mode == RequestMode::NoCors {
                self.resource_policy_check(&request.origin, &current, head.headers())?;
            }
            let location = location_url(&head, &current).map_err(NetworkError::Redirect)?;
            let Some(location) = location else {
                return Ok(self.filtered(&request, hop, head, exchange, false));
            };
            match request.redirect {
                RedirectMode::Manual => {
                    return Ok(self.filtered(&request, hop, head, exchange, true));
                }
                RedirectMode::Error => {
                    return Err(NetworkError::Redirect(RedirectError::RedirectForbidden));
                }
                RedirectMode::Follow => {}
            }
            drop(exchange);
            // Referrer Policy §8.2 "set request's referrer policy on redirect".
            if let Some(policy) = ReferrerPolicy::from_headers(head.headers()) {
                hop.referrer_policy = policy;
            }
            self.follow(&request, &mut hop, &current, location, head.status())?;
        }
    }

    /// Fetch §4.4 for `follow` mode.
    fn follow(
        &self,
        request: &FetchRequest,
        hop: &mut Hop,
        current: &Url,
        location: Url,
        status: u16,
    ) -> Result<(), NetworkError> {
        if !matches!(location.scheme(), "http" | "https") {
            return Err(NetworkError::Redirect(RedirectError::UnsupportedScheme));
        }
        if hop.url_list.len() > MAX_REDIRECTS {
            return Err(NetworkError::Redirect(RedirectError::TooManyRedirects));
        }
        let has_credentials = !location.username().is_empty() || !location.password().is_empty();
        let location_origin = location.origin();
        if has_credentials
            && ((request.mode == RequestMode::Cors
                && !request.origin.is_same_origin(&location_origin))
                || hop.tainting == ResponseTainting::Cors)
        {
            return Err(NetworkError::RedirectWithCredentials);
        }
        let current_origin = current.origin();
        if !current_origin.is_same_origin(&location_origin)
            && !request.origin.is_same_origin(&current_origin)
        {
            hop.tainted_origin = true;
        }
        rewrite_for_redirect(
            status,
            !current_origin.is_same_origin(&location_origin),
            &mut hop.method,
            &mut hop.headers,
            &mut hop.body,
        );
        hop.url_list.push(location);
        Ok(())
    }

    /// Main fetch step 12.4 and Fetch §4.8: preflight unless the cache
    /// already covers the method and every unsafe header.
    fn preflight_if_needed(
        &mut self,
        request: &FetchRequest,
        hop: &Hop,
        current: &Url,
        origin: &str,
    ) -> Result<(), NetworkError> {
        let unsafe_names = cors_unsafe_request_header_names(&hop.headers);
        if !request.use_cors_preflight
            && is_cors_safelisted_method(&hop.method)
            && unsafe_names.is_empty()
        {
            return Ok(());
        }
        let credentialed = request.credentials == CredentialsMode::Include;
        let now = Instant::now();
        let partition = self.partition_key(request);
        let method_cached = (is_cors_safelisted_method(&hop.method) && !request.use_cors_preflight)
            || self.preflight.allows_method_in(
                &partition,
                origin,
                current,
                credentialed,
                &hop.method,
                now,
            );
        let cached = method_cached
            && unsafe_names.iter().all(|name| {
                self.preflight.allows_header_in(
                    &partition,
                    origin,
                    current,
                    credentialed,
                    name,
                    now,
                )
            });
        if cached {
            return Ok(());
        }
        let mut preflight =
            Request::new("OPTIONS", current.clone()).map_err(NetworkError::Request)?;
        for (name, value) in preflight_headers(&hop.method, &hop.headers).iter() {
            preflight
                .header(name, value)
                .map_err(NetworkError::Request)?;
        }
        preflight
            .header("Origin", origin.as_bytes())
            .map_err(NetworkError::Request)?;
        let (head, _exchange) = self.send(preflight)?;
        let grant = validate_preflight(
            head.status(),
            head.headers(),
            origin,
            request.credentials,
            &hop.method,
            &hop.headers,
            request.use_cors_preflight,
        )
        .map_err(NetworkError::Preflight)?;
        self.preflight
            .insert_in(&partition, origin, current, credentialed, &grant, now);
        Ok(())
    }

    /// Fetch "determine the network partition key": the top-level site, or
    /// for a top-level client the site of its own origin.
    fn partition_key(&self, request: &FetchRequest) -> String {
        match &request.top_level_site {
            Some(site) => site.serialize(),
            None => self.suffixes().site(&request.origin).serialize(),
        }
    }

    /// HTTP-network-or-cache fetch without the cache: headers, cookies,
    /// the request itself and `Set-Cookie` processing.
    fn network_fetch(
        &mut self,
        request: &FetchRequest,
        hop: &Hop,
        current: &Url,
        serialized_origin: &str,
    ) -> Result<(ResponseHead, Exchange), NetworkError> {
        let mut wire = Request::new(&hop.method, current.clone()).map_err(NetworkError::Request)?;
        for (name, value) in hop.headers.iter() {
            wire.header(name, value).map_err(NetworkError::Request)?;
        }
        wire.set_body(hop.body.clone());
        if !hop.headers.contains("accept") {
            let accept: &[u8] = if request.mode == RequestMode::Navigate {
                b"text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
            } else {
                b"*/*"
            };
            wire.header("Accept", accept)
                .map_err(NetworkError::Request)?;
        }
        if let Some(origin) = origin_header(request, hop, current, serialized_origin) {
            wire.header("Origin", origin.as_bytes())
                .map_err(NetworkError::Request)?;
        }
        if let Some(referrer) = request
            .referrer
            .as_ref()
            .and_then(|referrer| determine_referrer(referrer, current, hop.referrer_policy))
        {
            wire.header("Referer", referrer.as_bytes())
                .map_err(NetworkError::Request)?;
        }
        if !hop.headers.contains("accept-language") {
            wire.header("Accept-Language", self.accept_language.as_bytes())
                .map_err(NetworkError::Request)?;
        }
        if is_potentially_trustworthy_url(current) {
            for (name, value) in self.fetch_metadata(request, hop) {
                wire.header(name, value.as_bytes())
                    .map_err(NetworkError::Request)?;
            }
        }
        let include_credentials = request.credentials == CredentialsMode::Include
            || (request.credentials == CredentialsMode::SameOrigin
                && hop.tainting == ResponseTainting::Basic);
        let now = (self.clock)();
        let top_level_site = if request.mode == RequestMode::Navigate {
            Some(self.suffixes().site(&current.origin()))
        } else {
            request.top_level_site.clone()
        };
        let context = CookieContext {
            url: current,
            same_site: hop.same_site,
            top_level_navigation: request.mode == RequestMode::Navigate,
            safe_method: matches!(hop.method.as_str(), "GET" | "HEAD" | "OPTIONS"),
            http_api: true,
            top_level_site,
        };
        if include_credentials {
            if let Some(cookie) = self.cookies.cookie_header(&context, now) {
                wire.header("Cookie", cookie.as_bytes())
                    .map_err(NetworkError::Request)?;
            }
        }
        let (head, exchange) = self.send(wire)?;
        if include_credentials {
            for value in head.headers().get_all("set-cookie") {
                let text: String = value.iter().map(|&byte| char::from(byte)).collect();
                // A rejected cookie leaves the response unchanged.
                let _ = self.cookies.set_cookie(&text, &context, now);
            }
        }
        Ok((head, exchange))
    }

    /// Fetch Metadata §5: `Sec-Fetch-Dest`, `-Mode`, `-Site` (most
    /// restrictive along the redirect chain) and `-User`.
    fn fetch_metadata(&self, request: &FetchRequest, hop: &Hop) -> Vec<(&'static str, String)> {
        let site = if request.mode == RequestMode::Navigate && request.site_for_cookies.is_none() {
            "none"
        } else {
            let mut site = "same-origin";
            for url in &hop.url_list {
                let target = url.origin();
                if request.origin.is_same_origin(&target) {
                    continue;
                }
                if self.suffixes().is_same_site(&request.origin, &target) {
                    site = "same-site";
                } else {
                    site = "cross-site";
                    break;
                }
            }
            site
        };
        let mode = match request.mode {
            RequestMode::Navigate => "navigate",
            RequestMode::SameOrigin => "same-origin",
            RequestMode::NoCors => "no-cors",
            RequestMode::Cors => "cors",
        };
        let mut headers = vec![
            ("Sec-Fetch-Dest", request.destination.as_str().to_owned()),
            ("Sec-Fetch-Mode", mode.to_owned()),
            ("Sec-Fetch-Site", site.to_owned()),
        ];
        if request.mode == RequestMode::Navigate && request.user_activation {
            headers.push(("Sec-Fetch-User", "?1".to_owned()));
        }
        headers
    }

    fn send(&self, request: Request) -> Result<(ResponseHead, Exchange), NetworkError> {
        let exchange = self
            .transport
            .start(request)
            .map_err(NetworkError::Transport)?;
        match exchange.next_event(Some(self.head_timeout)) {
            Some(NetEvent::Response(head)) => Ok((head, exchange)),
            Some(NetEvent::Failed(error)) => Err(NetworkError::Transport(error)),
            Some(NetEvent::Body(_) | NetEvent::Finished) => {
                unreachable!("an exchange reports its head before any body or end")
            }
            None => Err(NetworkError::HeadTimeout),
        }
    }

    /// Fetch "cross-origin resource policy internal check" for no-CORS
    /// requests.
    fn resource_policy_check(
        &self,
        origin: &Origin,
        current: &Url,
        headers: &Headers,
    ) -> Result<(), NetworkError> {
        let Some(policy) = headers.combined("cross-origin-resource-policy") else {
            return Ok(());
        };
        let target = current.origin();
        let blocked = match policy.as_slice() {
            b"same-origin" => !origin.is_same_origin(&target),
            b"same-site" => {
                !self.suffixes().is_schemelessly_same_site(origin, &target)
                    || (origin.serialize().starts_with("https:") && current.scheme() != "https")
            }
            _ => false,
        };
        if blocked {
            Err(NetworkError::BlockedByResourcePolicy)
        } else {
            Ok(())
        }
    }

    fn filtered(
        &self,
        request: &FetchRequest,
        hop: Hop,
        head: ResponseHead,
        exchange: Exchange,
        opaque_redirect: bool,
    ) -> FetchResponse {
        let kind = if opaque_redirect {
            ResponseType::OpaqueRedirect
        } else {
            match hop.tainting {
                ResponseTainting::Basic => ResponseType::Basic,
                ResponseTainting::Cors => ResponseType::Cors,
                ResponseTainting::Opaque => ResponseType::Opaque,
            }
        };
        let mut headers = Headers::new();
        match kind {
            ResponseType::Basic => {
                for (name, value) in head.headers().iter() {
                    if !is_forbidden_response_header_name(name) {
                        headers
                            .append(name, value)
                            .expect("received fields are valid");
                    }
                }
            }
            ResponseType::Cors => {
                let exposed = exposed_header_names(head.headers(), request.credentials);
                for (name, value) in head.headers().iter() {
                    if is_cors_safelisted_response_header_name(name, &exposed) {
                        headers
                            .append(name, value)
                            .expect("received fields are valid");
                    }
                }
            }
            ResponseType::Opaque | ResponseType::OpaqueRedirect => {}
        }
        FetchResponse {
            kind,
            headers,
            internal: InternalResponse {
                url_list: hop.url_list,
                head,
                exchange,
            },
        }
    }
}

/// Fetch §3.1 "append a request `Origin` header".
fn origin_header(
    request: &FetchRequest,
    hop: &Hop,
    current: &Url,
    serialized: &str,
) -> Option<String> {
    if hop.tainting == ResponseTainting::Cors {
        return Some(serialized.to_owned());
    }
    if matches!(hop.method.as_str(), "GET" | "HEAD") {
        return None;
    }
    if request.mode == RequestMode::Cors {
        return Some(serialized.to_owned());
    }
    let downgrade = serialized.starts_with("https:") && !is_potentially_trustworthy_url(current);
    let null = match hop.referrer_policy {
        ReferrerPolicy::NoReferrer => true,
        ReferrerPolicy::NoReferrerWhenDowngrade
        | ReferrerPolicy::StrictOrigin
        | ReferrerPolicy::StrictOriginWhenCrossOrigin => downgrade,
        ReferrerPolicy::SameOrigin => !request.origin.is_same_origin(&current.origin()),
        ReferrerPolicy::Origin
        | ReferrerPolicy::OriginWhenCrossOrigin
        | ReferrerPolicy::UnsafeUrl => false,
    };
    Some(if null {
        "null".to_owned()
    } else {
        serialized.to_owned()
    })
}
