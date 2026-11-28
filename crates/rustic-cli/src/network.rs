//! Loading documents and subresources over HTTP(S) for the development
//! command line (M6, NET-02/NET-05).
//!
//! The network is off unless the user grants origins: `--allow-origin
//! ORIGIN=ADDR` routes one `http`/`https` origin to a loopback socket
//! address (a fixture server, ADR 0051), `--allow-origin ORIGIN` lets one
//! origin resolve through the system resolver, and `--allow-remote` lets
//! every origin resolve (TD-157); resolved names reach only public
//! addresses, never loopback or private ones. `--ca-file PEM` replaces the platform trust store with the
//! given roots, for fixture servers with their own certificate authority.
//! Without a grant an `http(s)` argument fails before anything is sent.
//!
//! A document arrives through [`rustic_net::navigation::Navigation`]; its
//! response URL is the base URL and origin, its `Content-Type` charset the
//! transport encoding, and its policy container the CSP every subresource
//! is checked against. Each stylesheet, script and image request then
//! passes, on every hop, the document's CSP and the mixed-content check;
//! responses are checked for status and, for scripts and stylesheets,
//! for their MIME type (`X-Content-Type-Options: nosniff`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Duration;

use rustic_base::{CancellationToken, EscapedText};
use rustic_encoding::Encoding;
use rustic_net::cookie::{CookieJar, CookieLimits};
use rustic_net::cors::CredentialsMode;
use rustic_net::csp::{CspList, FetchDirective, Verdict};
use rustic_net::fetch::{
    Destination, FetchRequest, FetchResponse, Fetcher, NetworkError, RequestMode,
};
use rustic_net::mime::{MimeType, extract_mime_type};
use rustic_net::mixed_content::{self, MixedContent};
use rustic_net::navigation::{
    DocumentKind, Navigation, NavigationError, NavigationOutcome, NavigationRequest,
    PolicyContainer,
};
use rustic_net::referrer::ReferrerPolicy;
use rustic_net::site::{PublicSuffixList, Site};
use rustic_net::tls::TlsConfig;
use rustic_net::{NetworkPolicy, Transport, TransportLimits};
use rustic_style::FetchError;
use rustic_url::{Origin, Url};

/// Most `--allow-origin` grants per run.
const MAX_ORIGIN_GRANTS: usize = 64;
/// Largest `--ca-file` accepted.
const MAX_CA_FILE_BYTES: u64 = 1024 * 1024;
/// How long to wait for a response head, and for each piece of a body.
pub(crate) const NETWORK_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const MAX_STREAMED_FETCH_BODIES: usize = 16;
/// Most distinct stylesheet charsets remembered per document.
const MAX_REMEMBERED_CHARSETS: usize = 4096;

/// The user's network grants, parsed once from the arguments.
#[derive(Default)]
struct NetworkConfig {
    /// Granted origins: a fixed loopback address, or `None` to resolve.
    routes: Vec<(Url, Option<SocketAddr>)>,
    /// `--allow-remote`: every origin may resolve.
    any_remote: bool,
    ca_pem: Option<Vec<u8>>,
    /// The user's locale, for the HTML fallback encoding (`--locale`).
    locale: Option<String>,
    /// Where navigations that turn into downloads are saved
    /// (`--download-dir`); without it downloads are refused.
    download_dir: Option<std::path::PathBuf>,
}

static CONFIG: OnceLock<NetworkConfig> = OnceLock::new();

/// Remove `--allow-origin`, `--ca-file` and `--locale` options from `args` and keep
/// them for the run. Values are validated here, so a bad grant is a usage
/// error before any command runs.
pub fn configure(args: Vec<OsString>) -> Result<Vec<OsString>, String> {
    let mut output = Vec::new();
    let mut config = NetworkConfig::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            output.push(arg);
            output.extend(args);
            break;
        }
        if arg == "--allow-origin" {
            let value = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or("--allow-origin requires ORIGIN=ADDRESS")?;
            if config.routes.len() >= MAX_ORIGIN_GRANTS {
                return Err("at most 64 --allow-origin grants are accepted".into());
            }
            config.routes.push(parse_grant(&value)?);
        } else if arg == "--allow-remote" {
            config.any_remote = true;
        } else if arg == "--ca-file" {
            let path = args.next().ok_or("--ca-file requires a PEM file")?;
            let metadata = std::fs::metadata(&path).map_err(|e| format!("--ca-file: {e}"))?;
            if !metadata.is_file() || metadata.len() > MAX_CA_FILE_BYTES {
                return Err("--ca-file must be a regular file of at most 1 MiB".into());
            }
            let pem = std::fs::read(&path).map_err(|e| format!("--ca-file: {e}"))?;
            TlsConfig::with_roots_pem(&pem).map_err(|e| format!("--ca-file: {e}"))?;
            config.ca_pem = Some(pem);
        } else if arg == "--download-dir" {
            let dir = args.next().ok_or("--download-dir requires a directory")?;
            let dir = std::path::PathBuf::from(dir);
            if !dir.is_dir() {
                return Err("--download-dir must name an existing directory".into());
            }
            config.download_dir = Some(dir);
        } else if arg == "--locale" {
            let locale = args
                .next()
                .and_then(|value| value.into_string().ok())
                .filter(|value| {
                    (1..=35).contains(&value.len())
                        && value
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                })
                .ok_or("--locale requires a language tag such as en or zh-TW")?;
            config.locale = Some(locale);
        } else {
            output.push(arg);
        }
    }
    if CONFIG.set(config).is_err() {
        return Err("network options configured twice".into());
    }
    Ok(output)
}

/// `ORIGIN=ADDRESS`, for example `http://a.test=127.0.0.1:8080` (a
/// loopback fixture), or `ORIGIN` alone, for example `https://example.com`
/// (resolved, public addresses only).
fn parse_grant(value: &str) -> Result<(Url, Option<SocketAddr>), String> {
    let invalid = || {
        format!(
            "--allow-origin {}: expected an http(s) ORIGIN, or ORIGIN=ADDRESS with a loopback \
             address",
            EscapedText(value)
        )
    };
    let (origin, address) = match value.rsplit_once('=') {
        Some((origin, address)) => (origin, Some(address)),
        None => (value, None),
    };
    let origin = Url::parse(origin, None).map_err(|_| invalid())?;
    let mut policy = NetworkPolicy::deny_all();
    let address = match address {
        Some(address) => {
            let address: SocketAddr = address.parse().map_err(|_| invalid())?;
            policy.allow(&origin, address).map_err(|_| invalid())?;
            Some(address)
        }
        None => {
            policy.allow_resolved(&origin).map_err(|_| invalid())?;
            None
        }
    };
    Ok((origin, address))
}

/// The locale whose HTML fallback encoding a network document without an
/// encoding declaration uses: `--locale`, else `en` (windows-1252), so
/// output does not depend on the environment.
pub fn fallback_locale() -> &'static str {
    CONFIG
        .get()
        .and_then(|config| config.locale.as_deref())
        .unwrap_or("en")
}

/// The `--locale` the user gave, if any (a broker passes it on).
pub fn configured_locale() -> Option<&'static str> {
    CONFIG.get().and_then(|config| config.locale.as_deref())
}

/// Whether any origin is routed to a loopback fixture address (an
/// AppContainer cannot reach those, TD-176).
#[cfg(windows)]
pub(crate) fn has_loopback_grants() -> bool {
    CONFIG
        .get()
        .is_some_and(|config| config.routes.iter().any(|(_, address)| address.is_some()))
}

/// Whether any `--allow-origin` grant was given.
pub fn has_grants() -> bool {
    CONFIG
        .get()
        .is_some_and(|config| !config.routes.is_empty() || config.any_remote)
}

/// Whether a command-line argument names an `http:` or `https:` URL.
pub fn is_network_argument(argument: &OsStr) -> bool {
    let Some(text) = argument.to_str() else {
        return false;
    };
    let lower = text.get(..8).unwrap_or(text).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Why a network document could not be loaded.
#[derive(Debug)]
pub enum NetworkLoadError {
    /// The argument is not a valid URL.
    InvalidUrl,
    /// No `--allow-origin` grants were given.
    NoGrants,
    /// The TLS configuration could not be built.
    Tls(String),
    /// The navigation failed.
    Navigation(NavigationError),
    /// Reading the body failed.
    Body(NetworkError),
    /// The response was a download, not a document.
    Download(String),
    /// The download was saved here.
    Saved(String),
    /// Saving the download failed.
    DownloadFailed(String),
    /// A 204 or 205 response: there is no document.
    NoContent,
    /// The response is not HTML.
    NotHtml(String),
    /// The body exceeded `--max-input-bytes`.
    TooLarge(usize),
    /// A streamed body stopped part way (TD-165).
    BodyFailed(String),
    /// The profile could not be read.
    Profile(String),
}

impl fmt::Display for NetworkLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl => formatter.write_str("not a valid URL"),
            Self::NoGrants => formatter
                .write_str("network loading needs --allow-origin ORIGIN[=ADDRESS] for each origin, or --allow-remote"),
            Self::Tls(error) => write!(formatter, "TLS setup failed: {error}"),
            Self::Navigation(error) => write!(formatter, "navigation failed: {error}"),
            Self::Body(error) => write!(formatter, "reading the response failed: {error}"),
            Self::Download(name) => write!(
                formatter,
                "the response is a download ({}), not a document",
                EscapedText(name)
            ),
            Self::Saved(path) => write!(
                formatter,
                "the response is a download, not a document; saved as {}",
                EscapedText(path)
            ),
            Self::DownloadFailed(error) => write!(formatter, "the download was not saved: {error}"),
            Self::NoContent => formatter.write_str("the response has no content (204 or 205)"),
            Self::NotHtml(mime) => write!(
                formatter,
                "the response is {}, not an HTML document",
                EscapedText(mime)
            ),
            Self::TooLarge(limit) => write!(formatter, "the response exceeds {limit} bytes"),
            Self::BodyFailed(error) => write!(formatter, "reading the response failed: {error}"),
            Self::Profile(error) => write!(formatter, "profile: {error}"),
        }
    }
}

impl std::error::Error for NetworkLoadError {}

/// A committed network document: its bytes and the session its
/// subresources load through.
#[derive(Debug)]
pub struct NetworkDocument {
    pub(crate) session: Rc<NetworkSession>,
    pub(crate) bytes: Vec<u8>,
    /// The `Content-Type` charset, the HTML "transport layer" encoding.
    pub(crate) transport_encoding: Option<Encoding>,
    /// The document's policy container (CSP and referrer policy).
    pub(crate) policy: PolicyContainer,
}

/// A document ready to parse, with the session its loads go through.
#[derive(Debug)]
pub(crate) struct SessionDocument {
    pub(crate) session: Rc<dyn Session>,
    /// The body so far: a streamed document grows as the parser reads it
    /// ([`Session::next_document_chunk`]).
    pub(crate) bytes: RefCell<Vec<u8>>,
    /// The transport-layer encoding (response `charset`).
    pub(crate) transport_encoding: Option<Encoding>,
}

impl From<NetworkDocument> for SessionDocument {
    fn from(document: NetworkDocument) -> Self {
        Self {
            session: document.session,
            bytes: RefCell::new(document.bytes),
            transport_encoding: document.transport_encoding,
        }
    }
}

/// What a subresource request is for.
#[derive(Clone, Debug)]
pub(crate) enum ResourceKind {
    /// A `<link rel=stylesheet>` or `@import`; in a quirks-mode document
    /// a same-origin sheet may have any MIME type.
    Stylesheet { quirks: bool },
    /// An `<img>`.
    Image,
    /// An `@font-face` source: CORS mode with same-origin credentials
    /// (CSS Fonts 4 §4.8.2).
    Font,
    /// A classic or module script.
    Script {
        nonce: Option<String>,
        parser_inserted: bool,
        module: bool,
    },
}

/// The connection state one network document's subresources share.
pub(crate) struct NetworkSession {
    /// Shared with the session's frames (one cookie jar and cache).
    fetcher: Rc<RefCell<Fetcher>>,
    live_fetch_bodies: Rc<Cell<usize>>,
    document_url: Url,
    origin: Origin,
    csp: CspList,
    secure: bool,
    referrer_policy: ReferrerPolicy,
    cancellation: CancellationToken,
    charsets: RefCell<HashMap<String, String>>,
    /// The document's COOP, COEP and origin-keying (ADR 0057).
    isolation: rustic_net::isolation::IsolationPolicy,
    /// For a frame's document: the top-level site and the site for
    /// cookies of its requests (ADR 0062); `None` at the top level.
    frame: Option<(Site, Origin)>,
    /// Subresource fetches started ahead of use (TD-165).
    inflight: RefCell<Inflight>,
    /// The rest of a navigated document's body, read as the parser asks
    /// for it (TD-165), and how many more bytes it may have.
    document_rest: RefCell<Option<DocumentRest>>,
}

impl fmt::Debug for NetworkSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NetworkSession")
            .field("policies", &self.csp.policies().len())
            .field("secure", &self.secure)
            .finish_non_exhaustive()
    }
}

/// What a document's loads go through: the network session in the
/// process that owns the network, or an IPC proxy in a renderer.
pub(crate) trait Session: fmt::Debug {
    /// The document URL (its fallback base URL and origin).
    fn document_url(&self) -> &Url;
    /// The document's CSP, for decisions the renderer makes itself.
    fn csp(&self) -> &CspList;
    /// Whether the document is a local file (UTF-8 fallback, no network).
    fn is_local(&self) -> bool {
        false
    }
    /// Whether the document may compile strings (`'unsafe-eval'`).
    fn allows_eval(&self) -> bool {
        let verdict = self.csp().check_eval();
        report(&verdict);
        verdict.allowed()
    }
    /// Fetch one subresource, at most `max_bytes`.
    fn fetch(
        &self,
        url: &Url,
        kind: &ResourceKind,
        max_bytes: usize,
    ) -> Result<Vec<u8>, FetchError>;
    /// Start fetching subresources the document will ask for, side by
    /// side, so [`Self::fetch`] finds them in flight or done (TD-165).
    /// A session that cannot does nothing, and each loads on request.
    fn prefetch(&self, _requests: &[(Url, ResourceKind)]) {}
    /// The `charset` label of a subresource already fetched.
    fn charset_of(&self, url: &Url) -> Option<String>;
    /// Run a script `fetch()`.
    fn script_fetch(
        &self,
        request: rustic_engine::ScriptRequest,
    ) -> Result<rustic_engine::ScriptResponse, rustic_engine::NetworkFailure>;
    /// The document's Web Storage, or `None` for an opaque origin.
    fn storage_host(&self) -> Result<Option<Rc<dyn rustic_engine::StorageHost>>, String>;
    /// Persist what the load changed (cookies, the profile).
    fn finish(&self) -> Result<(), String>;
    /// The next piece of the document body, `None` at its end (TD-165):
    /// a navigated document streams into the parser as it arrives.
    fn next_document_chunk(&self) -> Result<Option<Vec<u8>>, NetworkLoadError> {
        Ok(None)
    }
    /// Open a popup the page asked for (ADR 0062): only a split run's
    /// broker can, in a renderer of its own.
    fn open_popup(&self, _popup: &rustic_engine::PopupRequest) -> Result<(), String> {
        Err("popups open only in a split run (TD-178)".to_owned())
    }
}

impl Session for NetworkSession {
    fn document_url(&self) -> &Url {
        &self.document_url
    }

    fn csp(&self) -> &CspList {
        &self.csp
    }

    fn fetch(
        &self,
        url: &Url,
        kind: &ResourceKind,
        max_bytes: usize,
    ) -> Result<Vec<u8>, FetchError> {
        self.fetch_resource(url, kind, max_bytes)
    }

    fn prefetch(&self, requests: &[(Url, ResourceKind)]) {
        self.prefetch_resources(requests);
    }

    fn next_document_chunk(&self) -> Result<Option<Vec<u8>>, NetworkLoadError> {
        self.read_document_chunk()
    }

    fn charset_of(&self, url: &Url) -> Option<String> {
        self.charsets.borrow().get(&url.serialize()).cloned()
    }

    fn script_fetch(
        &self,
        request: rustic_engine::ScriptRequest,
    ) -> Result<rustic_engine::ScriptResponse, rustic_engine::NetworkFailure> {
        self.script_fetch_impl(request)
    }

    fn storage_host(&self) -> Result<Option<Rc<dyn rustic_engine::StorageHost>>, String> {
        crate::storage::host_for(&self.document_url).map_err(|error| error.to_string())
    }

    fn finish(&self) -> Result<(), String> {
        self.save_cookies().map_err(|error| error.to_string())
    }
}

/// Unix seconds, the cookie jar's clock.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

/// Navigate to `url` with the configured grants and read the document,
/// at most `max_bytes` of it.
pub fn navigate(
    url: &str,
    max_bytes: usize,
    cancellation: CancellationToken,
) -> Result<NetworkDocument, NetworkLoadError> {
    let url = Url::parse(url, None).map_err(|_| NetworkLoadError::InvalidUrl)?;
    let config = CONFIG.get_or_init(NetworkConfig::default);
    if config.routes.is_empty() && !config.any_remote {
        return Err(NetworkLoadError::NoGrants);
    }
    let mut fetcher = build_fetcher(config, max_bytes)?;
    // A persistent profile's cookies outlive the run.
    let saved =
        crate::storage::saved_cookies().map_err(|e| NetworkLoadError::Profile(e.to_string()))?;
    if let Some(saved) = saved {
        if fetcher
            .cookies_mut()
            .import_persistent(&saved, now())
            .is_none()
        {
            eprintln!("rustic: profile: the saved cookies are malformed; ignored");
        }
    }
    let navigation = Navigation::new(NavigationRequest::browser(url), NETWORK_TIMEOUT);
    let outcome = navigation
        .run(&mut fetcher)
        .map_err(NetworkLoadError::Navigation)?;
    let committed = match outcome {
        NavigationOutcome::Commit(committed) => committed,
        NavigationOutcome::Download(download) => {
            let name = download.sanitized_filename();
            let Some(dir) = &config.download_dir else {
                return Err(NetworkLoadError::Download(name));
            };
            return Err(match save_download(*download, dir, max_bytes) {
                Ok(path) => NetworkLoadError::Saved(path.to_string_lossy().into_owned()),
                Err(error) => NetworkLoadError::DownloadFailed(error.to_string()),
            });
        }
        NavigationOutcome::NoContent => return Err(NetworkLoadError::NoContent),
    };
    if committed.kind != DocumentKind::Html {
        return Err(NetworkLoadError::NotHtml(committed.mime.essence()));
    }
    let document_url = committed.url().clone();
    let transport_encoding = committed.encoding;
    let policy = committed.policy.clone();
    let origin = committed.origin.clone();
    let isolation = rustic_net::isolation::IsolationPolicy::from_headers(
        &committed.headers,
        mixed_content::is_secure_context(&origin, None),
    );
    // The parser reads the rest of the body as it arrives (TD-165).
    let (bytes, rest) = match committed.body {
        rustic_net::navigation::DocumentBody::Bytes(bytes) => (bytes, None),
        rustic_net::navigation::DocumentBody::Network { prefix, exchange } => {
            (prefix, Some(exchange))
        }
    };
    if bytes.len() > max_bytes {
        return Err(NetworkLoadError::TooLarge(max_bytes));
    }
    let mut session = NetworkSession::new(fetcher, document_url, origin, &policy, cancellation);
    session.isolation = isolation;
    session.document_rest = RefCell::new(rest.map(|exchange| DocumentRest {
        exchange,
        left: max_bytes - bytes.len(),
        max_bytes,
    }));
    Ok(NetworkDocument {
        session: Rc::new(session),
        bytes,
        transport_encoding,
        policy,
    })
}

/// Names Windows reserves for devices, in any case and with any extension.
fn is_device_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit())
}

/// Save a download as ADR 0055 requires: a sanitized name, never over an
/// existing file (` (n)` before the extension instead), written to an
/// exclusive `.part` file within `max_bytes` and renamed when complete.
/// The file is never opened or executed.
pub(crate) fn save_download(
    download: rustic_net::navigation::Download,
    dir: &std::path::Path,
    max_bytes: usize,
) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write as _;
    let mut name = download.sanitized_filename();
    if is_device_name(&name) {
        name.insert(0, '_');
    }
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => (name[..dot].to_owned(), name[dot..].to_owned()),
        _ => (name.clone(), String::new()),
    };
    let bytes = download
        .body
        .read_all(NETWORK_TIMEOUT)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    if bytes.len() > max_bytes {
        return Err(std::io::Error::other(format!(
            "larger than {max_bytes} bytes"
        )));
    }
    for attempt in 0..100u32 {
        let candidate = if attempt == 0 {
            name.clone()
        } else {
            format!("{stem} ({attempt}){extension}")
        };
        let target = dir.join(&candidate);
        let part = dir.join(format!("{candidate}.part"));
        if target.exists() {
            continue;
        }
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let written = file.write_all(&bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = std::fs::remove_file(&part);
            return Err(error);
        }
        if target.exists() {
            let _ = std::fs::remove_file(&part);
            continue;
        }
        std::fs::rename(&part, &target)?;
        return Ok(target);
    }
    Err(std::io::Error::other(
        "no free file name after 100 attempts",
    ))
}

fn build_fetcher(config: &NetworkConfig, max_bytes: usize) -> Result<Fetcher, NetworkLoadError> {
    let mut policy = NetworkPolicy::deny_all();
    if config.any_remote {
        policy.allow_any_remote();
    }
    for (origin, address) in &config.routes {
        match address {
            Some(address) => policy.allow(origin, *address),
            None => policy.allow_resolved(origin),
        }
        .map_err(|_| NetworkLoadError::NoGrants)?;
    }
    let limits = TransportLimits {
        max_body_bytes: u64::try_from(max_bytes).unwrap_or(u64::MAX),
        ..TransportLimits::default()
    };
    let jar = CookieJar::new(PublicSuffixList::bundled(), CookieLimits::default());
    if !crate::broker::single_process() {
        // Split run: exchanges (sockets, TLS, HTTP parsing) run in the
        // network process; this process keeps the decisions (ISO-07).
        let transport = crate::netproc::transport(policy, limits, config.ca_pem.as_deref())
            .map_err(NetworkLoadError::Tls)?;
        return Ok(Fetcher::new(transport, jar, NETWORK_TIMEOUT));
    }
    let transport =
        Transport::new(policy, limits).map_err(|e| NetworkLoadError::Tls(e.to_string()))?;
    let tls = match &config.ca_pem {
        Some(pem) => TlsConfig::with_roots_pem(pem),
        None => TlsConfig::platform(),
    }
    .map_err(|e| NetworkLoadError::Tls(e.to_string()))?;
    Ok(Fetcher::new(transport.with_tls(tls), jar, NETWORK_TIMEOUT))
}

impl NetworkSession {
    fn new(
        fetcher: Fetcher,
        document_url: Url,
        origin: Origin,
        policy: &PolicyContainer,
        cancellation: CancellationToken,
    ) -> Self {
        let secure = mixed_content::is_secure_context(&origin, None);
        Self {
            fetcher: Rc::new(RefCell::new(fetcher)),
            live_fetch_bodies: Rc::new(Cell::new(0)),
            document_url,
            origin,
            csp: CspList::from_container(policy),
            secure,
            referrer_policy: policy.referrer_policy,
            cancellation,
            charsets: RefCell::new(HashMap::new()),
            isolation: rustic_net::isolation::IsolationPolicy::default(),
            frame: None,
            inflight: RefCell::default(),
            document_rest: RefCell::new(None),
        }
    }

    /// The document's origin.
    pub(crate) fn origin(&self) -> &Origin {
        &self.origin
    }

    /// The document's COOP, COEP and origin-keying.
    pub(crate) fn isolation(&self) -> rustic_net::isolation::IsolationPolicy {
        self.isolation
    }

    /// Navigate a popup this document opened to `url`: a top-level
    /// navigation it initiated with user activation, sharing the cookie
    /// jar and cache, without a referrer for `noreferrer`.
    pub(crate) fn open_popup(
        &self,
        url: &Url,
        noreferrer: bool,
        max_bytes: usize,
    ) -> Result<NetworkDocument, NetworkLoadError> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(NetworkLoadError::Navigation(
                NavigationError::UnsupportedScheme,
            ));
        }
        let request = NavigationRequest {
            initiator: Some(rustic_net::navigation::Initiator {
                origin: self.origin.clone(),
                url: self.document_url.clone(),
                policy: PolicyContainer {
                    referrer_policy: if noreferrer {
                        ReferrerPolicy::NoReferrer
                    } else {
                        self.referrer_policy
                    },
                    ..PolicyContainer::default()
                },
            }),
            user_activation: true,
            ..NavigationRequest::browser(url.clone())
        };
        let outcome = {
            let mut fetcher = self
                .fetcher
                .try_borrow_mut()
                .map_err(|_| NetworkLoadError::NoContent)?;
            Navigation::new(request, NETWORK_TIMEOUT)
                .run(&mut fetcher)
                .map_err(NetworkLoadError::Navigation)?
        };
        let committed = match outcome {
            NavigationOutcome::Commit(committed) => committed,
            NavigationOutcome::Download(download) => {
                return Err(NetworkLoadError::Download(download.sanitized_filename()));
            }
            NavigationOutcome::NoContent => return Err(NetworkLoadError::NoContent),
        };
        if committed.kind != DocumentKind::Html {
            return Err(NetworkLoadError::NotHtml(committed.mime.essence()));
        }
        let document_url = committed.url().clone();
        let origin = committed.origin.clone();
        let isolation = rustic_net::isolation::IsolationPolicy::from_headers(
            &committed.headers,
            mixed_content::is_secure_context(&origin, None),
        );
        let transport_encoding = committed.encoding;
        let policy = committed.policy.clone();
        let bytes = committed
            .body
            .read_all(NETWORK_TIMEOUT)
            .map_err(NetworkLoadError::Body)?;
        if bytes.len() > max_bytes {
            return Err(NetworkLoadError::TooLarge(max_bytes));
        }
        let secure = mixed_content::is_secure_context(&origin, None);
        let session = Self {
            fetcher: Rc::clone(&self.fetcher),
            live_fetch_bodies: Rc::clone(&self.live_fetch_bodies),
            document_url,
            csp: CspList::from_container(&policy),
            secure,
            referrer_policy: policy.referrer_policy,
            cancellation: self.cancellation.clone(),
            charsets: RefCell::new(HashMap::new()),
            isolation,
            frame: None,
            inflight: RefCell::default(),
            document_rest: RefCell::new(None),
            origin,
        };
        Ok(NetworkDocument {
            session: Rc::new(session),
            bytes,
            transport_encoding,
            policy,
        })
    }

    /// The top-level site and whether the ancestors so far are all
    /// same-site with it, for a frame of this document.
    pub(crate) fn frame_context(&self) -> (Site, bool) {
        match &self.frame {
            Some((site, for_cookies)) => (site.clone(), !for_cookies.is_opaque()),
            None => (PublicSuffixList::bundled().site(&self.origin), true),
        }
    }

    /// Navigate an `iframe` of this document to `url` (ADR 0062): the
    /// document's CSP `frame-src` and mixed-content rules on every hop, the
    /// response's `frame-ancestors` or `X-Frame-Options`, and the
    /// document's COEP. The frame's own session shares this one's cookie
    /// jar and cache, as a third party when `container` says so.
    pub(crate) fn open_frame(
        &self,
        url: &Url,
        container: rustic_net::navigation::FrameContainer,
        max_bytes: usize,
    ) -> Result<NetworkDocument, NetworkLoadError> {
        let allowed = |hop: &Url, redirects: usize| {
            if hop.scheme() != "data"
                && mixed_content::check(hop, Destination::Iframe, self.secure)
                    == MixedContent::Blocked
            {
                eprintln!("rustic: mixed content: blocked insecure iframe navigation");
                return false;
            }
            let verdict =
                self.csp
                    .check_request(hop, FetchDirective::Frame, &self.origin, redirects);
            report(&verdict);
            verdict.allowed()
        };
        if !allowed(url, 0) {
            return Err(NetworkLoadError::Navigation(NavigationError::FrameRefused));
        }
        let target = match mixed_content::check(url, Destination::Iframe, self.secure) {
            MixedContent::Upgrade(upgraded) => upgraded,
            _ => url.clone(),
        };
        let top_level_site = container.top_level_site.clone();
        let for_cookies = if container.same_site_ancestors {
            container
                .ancestors
                .last()
                .cloned()
                .unwrap_or_else(Origin::opaque)
        } else {
            Origin::opaque()
        };
        let request = NavigationRequest {
            initiator: Some(rustic_net::navigation::Initiator {
                origin: self.origin.clone(),
                url: self.document_url.clone(),
                policy: PolicyContainer {
                    referrer_policy: self.referrer_policy,
                    ..PolicyContainer::default()
                },
            }),
            user_activation: false,
            container: Some(container),
            ..NavigationRequest::browser(target)
        };
        let mut navigation = Navigation::new(request, NETWORK_TIMEOUT);
        let mut redirects = 0;
        let outcome = loop {
            let mut fetcher = self
                .fetcher
                .try_borrow_mut()
                .map_err(|_| NetworkLoadError::NoContent)?;
            match navigation
                .step(&mut fetcher)
                .map_err(NetworkLoadError::Navigation)?
            {
                rustic_net::navigation::NavigationStep::Redirected(next) => {
                    redirects += 1;
                    drop(fetcher);
                    if !allowed(&next, redirects) {
                        navigation.cancel();
                        return Err(NetworkLoadError::Navigation(NavigationError::FrameRefused));
                    }
                }
                rustic_net::navigation::NavigationStep::Finished(outcome) => break outcome,
            }
        };
        let committed = match outcome {
            NavigationOutcome::Commit(committed) => committed,
            NavigationOutcome::Download(download) => {
                return Err(NetworkLoadError::Download(download.sanitized_filename()));
            }
            NavigationOutcome::NoContent => return Err(NetworkLoadError::NoContent),
        };
        if committed.kind != DocumentKind::Html {
            return Err(NetworkLoadError::NotHtml(committed.mime.essence()));
        }
        let document_url = committed.url().clone();
        let origin = committed.origin.clone();
        let isolation = rustic_net::isolation::IsolationPolicy::from_headers(
            &committed.headers,
            mixed_content::is_secure_context(&origin, None),
        );
        // HTML "check a navigation response's adherence to its embedder
        // policy": a require-corp embedder takes only frames that set a
        // COEP, and cross-origin ones that also send CORP.
        if self.isolation.embedder == rustic_net::isolation::EmbedderPolicy::RequireCorp
            && (isolation.embedder == rustic_net::isolation::EmbedderPolicy::UnsafeNone
                || !rustic_net::isolation::embedder_allows(
                    self.isolation.embedder,
                    &self.origin,
                    &document_url,
                    &committed.headers,
                ))
        {
            eprintln!(
                "rustic: Cross-Origin-Embedder-Policy: blocked a frame without a compatible \
                 embedder policy"
            );
            return Err(NetworkLoadError::Navigation(NavigationError::FrameRefused));
        }
        let transport_encoding = committed.encoding;
        let policy = committed.policy.clone();
        let bytes = committed
            .body
            .read_all(NETWORK_TIMEOUT)
            .map_err(NetworkLoadError::Body)?;
        if bytes.len() > max_bytes {
            return Err(NetworkLoadError::TooLarge(max_bytes));
        }
        let secure = mixed_content::is_secure_context(&origin, None);
        let session = Self {
            fetcher: Rc::clone(&self.fetcher),
            live_fetch_bodies: Rc::clone(&self.live_fetch_bodies),
            document_url,
            csp: CspList::from_container(&policy),
            secure,
            referrer_policy: policy.referrer_policy,
            cancellation: self.cancellation.clone(),
            charsets: RefCell::new(HashMap::new()),
            isolation,
            frame: Some((top_level_site, for_cookies)),
            inflight: RefCell::default(),
            document_rest: RefCell::new(None),
            origin,
        };
        Ok(NetworkDocument {
            session: Rc::new(session),
            bytes,
            transport_encoding,
            policy,
        })
    }

    /// Save the jar's persistent cookies in the profile.
    pub(crate) fn save_cookies(&self) -> Result<(), rustic_storage::ProfileError> {
        let bytes = self.fetcher.borrow().cookies().export_persistent(now());
        crate::storage::save_cookies(bytes)
    }

    /// Fetch one subresource of the document, at most `max_bytes`. A
    /// resource [`Session::prefetch`] already started is taken from the
    /// fetches in flight; anything else is fetched now.
    fn fetch_resource(
        &self,
        url: &Url,
        kind: &ResourceKind,
        max_bytes: usize,
    ) -> Result<Vec<u8>, FetchError> {
        if self.cancellation.check().is_err() {
            return Err(FetchError::Cancelled);
        }
        if !matches!(url.scheme(), "http" | "https") {
            return Err(FetchError::Blocked);
        }
        let started = self
            .inflight
            .borrow_mut()
            .ids
            .remove(&resource_key(url, kind));
        let result = match started {
            Some(id) => self.wait_for(id)?,
            None => {
                let (request, mut allow) = self.resource_request(url, kind)?;
                let mut fetcher = self
                    .fetcher
                    .try_borrow_mut()
                    .map_err(|_| FetchError::Failed)?;
                fetcher.fetch_checked(request, &mut *allow)
            }
        };
        self.finish_resource(url, kind, max_bytes, result)
    }

    /// The next piece of the navigated document's body (see
    /// [`Session::next_document_chunk`]), waiting at most
    /// [`NETWORK_TIMEOUT`] and checking for cancellation as it waits.
    fn read_document_chunk(&self) -> Result<Option<Vec<u8>>, NetworkLoadError> {
        let mut slot = self.document_rest.borrow_mut();
        let Some(rest) = slot.as_mut() else {
            return Ok(None);
        };
        let deadline = std::time::Instant::now() + NETWORK_TIMEOUT;
        let event = loop {
            if self.cancellation.check().is_err() {
                *slot = None;
                return Err(NetworkLoadError::BodyFailed("cancelled".to_owned()));
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                *slot = None;
                return Err(NetworkLoadError::Body(NetworkError::BodyTimeout));
            }
            let wait = (deadline - now).min(Duration::from_millis(100));
            if let Some(event) = rest.exchange.next_event(Some(wait)) {
                break event;
            }
        };
        match event {
            rustic_net::NetEvent::Body(bytes) => {
                if bytes.len() > rest.left {
                    let max_bytes = rest.max_bytes;
                    *slot = None;
                    return Err(NetworkLoadError::TooLarge(max_bytes));
                }
                rest.left -= bytes.len();
                Ok(Some(bytes))
            }
            rustic_net::NetEvent::Finished => {
                *slot = None;
                Ok(None)
            }
            rustic_net::NetEvent::Failed(error) => {
                *slot = None;
                Err(NetworkLoadError::Body(NetworkError::Transport(error)))
            }
            rustic_net::NetEvent::Response(_) => {
                *slot = None;
                Err(NetworkLoadError::BodyFailed(
                    "a second response head".to_owned(),
                ))
            }
        }
    }

    /// Start fetching `requests` side by side (TD-165), for
    /// [`Self::fetch_resource`] to collect later.
    fn prefetch_resources(&self, requests: &[(Url, ResourceKind)]) {
        for (url, kind) in requests {
            if !matches!(url.scheme(), "http" | "https") {
                continue;
            }
            let key = resource_key(url, kind);
            if self.inflight.borrow().ids.contains_key(&key)
                || self.inflight.borrow().ids.len() >= MAX_PREFETCHES
            {
                continue;
            }
            // A request refused before sending is refused again, and
            // reported, when the loader asks for it.
            let Ok((request, allow)) = self.resource_request(url, kind) else {
                continue;
            };
            let Ok(mut fetcher) = self.fetcher.try_borrow_mut() else {
                return;
            };
            let mut inflight = self.inflight.borrow_mut();
            if let Ok(id) = inflight.controller.start(&mut fetcher, request, allow) {
                inflight.ids.insert(key, id);
            }
        }
    }

    /// Advance the fetches in flight until `id` finishes, keeping the
    /// others' results.
    fn wait_for(&self, id: u64) -> Result<Result<FetchResponse, NetworkError>, FetchError> {
        loop {
            if let Some(result) = self.inflight.borrow_mut().done.remove(&id) {
                return Ok(result);
            }
            if self.cancellation.check().is_err() {
                self.inflight.borrow_mut().controller.cancel(id);
                return Err(FetchError::Cancelled);
            }
            let mut fetcher = self
                .fetcher
                .try_borrow_mut()
                .map_err(|_| FetchError::Failed)?;
            let mut inflight = self.inflight.borrow_mut();
            if inflight.controller.is_empty() {
                return Err(FetchError::Failed);
            }
            let until = std::time::Instant::now() + Duration::from_millis(100);
            let finished = inflight.controller.wait(&mut fetcher, Some(until));
            inflight.done.extend(finished);
        }
    }

    /// The Fetch request for a subresource, and the client policy (mixed
    /// content and CSP) asked before each of its hops.
    fn resource_request(
        &self,
        url: &Url,
        kind: &ResourceKind,
    ) -> Result<(FetchRequest, Box<HopPolicy>), FetchError> {
        let (destination, directive) = match kind {
            ResourceKind::Stylesheet { .. } => (Destination::Style, FetchDirective::StyleElement),
            ResourceKind::Image => (Destination::Image, FetchDirective::Image),
            ResourceKind::Font => (Destination::Font, FetchDirective::Font),
            ResourceKind::Script { .. } => (Destination::Script, FetchDirective::ScriptElement),
        };
        let (nonce, parser_inserted) = match kind {
            ResourceKind::Script {
                nonce,
                parser_inserted,
                ..
            } => (nonce.as_deref(), *parser_inserted),
            _ => (None, true),
        };
        let target = match mixed_content::check(url, destination, self.secure) {
            MixedContent::Allowed => url.clone(),
            MixedContent::Upgrade(upgraded) => upgraded,
            MixedContent::Blocked => {
                eprintln!(
                    "rustic: mixed content: blocked insecure {} request",
                    destination.as_str()
                );
                return Err(FetchError::Blocked);
            }
        };
        // HTML: classic elements without `crossorigin` are no-cors with
        // credentials; module scripts are CORS, same-origin credentials.
        let module = matches!(
            kind,
            ResourceKind::Script { module: true, .. } | ResourceKind::Font
        );
        let request = FetchRequest {
            mode: if module {
                RequestMode::Cors
            } else {
                RequestMode::NoCors
            },
            credentials: if module {
                CredentialsMode::SameOrigin
            } else if self.isolation.embedder
                == rustic_net::isolation::EmbedderPolicy::Credentialless
                && !target.origin().is_same_origin(&self.origin)
            {
                // COEP credentialless: cross-origin no-cors requests omit
                // credentials.
                CredentialsMode::Omit
            } else {
                CredentialsMode::Include
            },
            destination,
            referrer: Some(self.document_url.clone()),
            referrer_policy: self.referrer_policy,
            top_level_site: self.frame.as_ref().map(|(site, _)| site.clone()),
            site_for_cookies: self.frame.as_ref().map_or_else(
                || Some(self.origin.clone()),
                |(_, for_cookies)| Some(for_cookies.clone()),
            ),
            ..FetchRequest::new(target, self.origin.clone())
        };
        let secure = self.secure;
        let csp = self.csp.clone();
        let origin = self.origin.clone();
        let nonce = nonce.map(str::to_owned);
        let allow = Box::new(move |hop: &Url, redirects: usize| {
            if mixed_content::check(hop, destination, secure) == MixedContent::Blocked {
                return false;
            }
            let verdict = csp.check_request_with_nonce(
                hop,
                directive,
                &origin,
                redirects,
                nonce.as_deref(),
                parser_inserted,
            );
            report(&verdict);
            verdict.allowed()
        });
        Ok((request, allow))
    }

    /// The checks on a subresource's response, and its body.
    fn finish_resource(
        &self,
        url: &Url,
        kind: &ResourceKind,
        max_bytes: usize,
        result: Result<FetchResponse, NetworkError>,
    ) -> Result<Vec<u8>, FetchError> {
        let destination = match kind {
            ResourceKind::Stylesheet { .. } => Destination::Style,
            ResourceKind::Image => Destination::Image,
            ResourceKind::Font => Destination::Font,
            ResourceKind::Script { .. } => Destination::Script,
        };
        // CORS-mode requests: no opaque bodies, so no ORB or CORP checks.
        let module = matches!(
            kind,
            ResourceKind::Script { module: true, .. } | ResourceKind::Font
        );
        let response = result.map_err(|error| match error {
            NetworkError::BlockedByClientPolicy
            | NetworkError::BlockedByResourcePolicy
            | NetworkError::Cors(_)
            | NetworkError::Preflight(_)
            | NetworkError::Transport(rustic_net::TransportError::Blocked) => FetchError::Blocked,
            _ => FetchError::Failed,
        })?;
        // Opaque responses (status 0) carry a body the renderer may use
        // (images, classic scripts and stylesheets loaded no-cors).
        let same_origin = matches!(
            response.response_type(),
            rustic_net::fetch::ResponseType::Basic | rustic_net::fetch::ResponseType::Cors
        );
        let internal = response.into_internal();
        let status = internal.head.status();
        if status == 404 || status == 410 {
            return Err(FetchError::NotFound);
        }
        if !(200..=299).contains(&status) {
            return Err(FetchError::Failed);
        }
        let headers = internal.head.headers();
        let mime = extract_mime_type(headers);
        let nosniff = headers.get("x-content-type-options").is_some_and(|value| {
            value
                .split(|&byte| byte == b',')
                .next()
                .is_some_and(|first| first.trim_ascii().eq_ignore_ascii_case(b"nosniff"))
        });
        check_mime(kind, mime.as_ref(), nosniff, same_origin)?;
        let final_url = internal
            .url_list
            .last()
            .cloned()
            .unwrap_or_else(|| url.clone());
        if !module
            && !rustic_net::isolation::embedder_allows(
                self.isolation.embedder,
                &self.origin,
                &final_url,
                headers,
            )
        {
            eprintln!("rustic: COEP require-corp: blocked a cross-origin resource without CORP");
            return Err(FetchError::Blocked);
        }
        let bytes = rustic_net::fetch::read_all(&internal.exchange, NETWORK_TIMEOUT)
            .map_err(|_| FetchError::Failed)?;
        if bytes.len() > max_bytes {
            return Err(FetchError::TooLarge);
        }
        // Opaque Response Blocking: another site's documents and data never
        // reach the renderer through a no-cors request.
        if !module
            && rustic_net::isolation::opaque_response_blocked(
                &self.origin,
                &final_url,
                destination,
                headers,
                &bytes,
            )
        {
            eprintln!(
                "rustic: ORB: blocked a cross-origin response that is not a {}",
                destination.as_str()
            );
            return Err(FetchError::Blocked);
        }
        if matches!(
            kind,
            ResourceKind::Stylesheet { .. } | ResourceKind::Script { .. }
        ) {
            let label = mime.as_ref().and_then(|mime| mime.parameter("charset"));
            let mut charsets = self.charsets.borrow_mut();
            if let Some(label) = label.filter(|_| charsets.len() < MAX_REMEMBERED_CHARSETS) {
                charsets.insert(url.serialize(), label.to_owned());
            }
        }
        Ok(bytes)
    }
}

/// The client policy asked before each hop of a subresource fetch.
type HopPolicy = dyn FnMut(&Url, usize) -> bool;

/// The unread rest of a navigated document's body.
struct DocumentRest {
    exchange: rustic_net::Exchange,
    /// Bytes it may still have.
    left: usize,
    /// The whole document's limit, for the error.
    max_bytes: usize,
}

/// Most subresource fetches one document starts ahead of use: each holds
/// a transfer thread and up to one body, so the rest load on request.
const MAX_PREFETCHES: usize = 32;

/// Fetches a document started ahead of use, and those that finished
/// while another was awaited.
#[derive(Default)]
struct Inflight {
    controller: rustic_net::fetch::FetchController<'static>,
    ids: HashMap<String, u64>,
    done: HashMap<u64, Result<FetchResponse, NetworkError>>,
}

/// What identifies a subresource request: its URL and how it is fetched.
fn resource_key(url: &Url, kind: &ResourceKind) -> String {
    let kind = match kind {
        ResourceKind::Stylesheet { .. } => "style".to_owned(),
        ResourceKind::Image => "image".to_owned(),
        ResourceKind::Font => "font".to_owned(),
        ResourceKind::Script {
            nonce,
            parser_inserted,
            module,
        } => format!(
            "script {module} {parser_inserted} {}",
            nonce.as_deref().unwrap_or("")
        ),
    };
    format!("{kind} {}", url.serialize())
}

/// Fetch §2.2.6 "should response to request be blocked due to its MIME
/// type" and "due to nosniff", and HTML's stylesheet type check (a
/// stylesheet must be `text/css` in a standards-mode document; quirks
/// mode's same-origin exception is not implemented).
fn check_mime(
    kind: &ResourceKind,
    mime: Option<&MimeType>,
    nosniff: bool,
    same_origin: bool,
) -> Result<(), FetchError> {
    match kind {
        ResourceKind::Script { .. } => {
            if mime.is_some_and(MimeType::is_blocked_for_script)
                || (nosniff && !mime.is_some_and(MimeType::is_javascript))
            {
                eprintln!("rustic: blocked a script with a non-JavaScript MIME type");
                return Err(FetchError::Blocked);
            }
        }
        ResourceKind::Stylesheet { quirks } => {
            // A quirks-mode document takes a CORS-same-origin sheet of any
            // type as CSS (HTML's link processing, CSSOM's style resource
            // fetch), unless `nosniff` blocks it first (Fetch).
            let css = mime.is_some_and(|mime| mime.essence() == "text/css");
            if !css && !(*quirks && same_origin && !nosniff) {
                eprintln!("rustic: skipped a stylesheet whose MIME type is not text/css");
                return Err(FetchError::Blocked);
            }
        }
        ResourceKind::Image | ResourceKind::Font => {}
    }
    Ok(())
}

/// Print CSP violations to standard error, the CLI's console.
fn report(verdict: &Verdict) {
    for violation in &verdict.violations {
        eprintln!("rustic: CSP: {violation}");
    }
}

/// The engine's inline-script CSP decisions for a network document.
#[derive(Debug)]
pub(crate) struct InlinePolicy(pub(crate) Rc<dyn Session>);

// The engine keeps the policy only for the load, on this thread.
impl rustic_engine::ScriptPolicy for InlinePolicy {
    fn allows_inline_script(&self, nonce: Option<&str>, source: &str) -> bool {
        let verdict =
            self.0
                .csp()
                .check_inline(rustic_net::csp::InlineKind::ScriptElement, nonce, source);
        report(&verdict);
        verdict.allowed()
    }

    fn allows_handler_attribute(&self, source: &str) -> bool {
        let verdict =
            self.0
                .csp()
                .check_inline(rustic_net::csp::InlineKind::ScriptAttribute, None, source);
        report(&verdict);
        verdict.allowed()
    }
}

/// `fetch()` for a local file: URLs resolve against the file, and every
/// fetch is a network error (local documents have no network, and
/// `file:` fetches are network errors in Fetch).
#[derive(Debug)]
pub(crate) struct LocalFetch(pub(crate) Url);

impl rustic_engine::FetchHost for LocalFetch {
    fn base_url(&self) -> Option<Url> {
        Some(self.0.clone())
    }

    fn fetch(
        &self,
        _: rustic_engine::ScriptRequest,
    ) -> Result<rustic_engine::ScriptResponse, rustic_engine::NetworkFailure> {
        Err(rustic_engine::NetworkFailure)
    }
}

/// `fetch()` for a document, through its session.
#[derive(Debug)]
pub(crate) struct SessionFetch(pub(crate) Rc<dyn Session>);

impl rustic_engine::FetchHost for SessionFetch {
    fn base_url(&self) -> Option<Url> {
        Some(self.0.document_url().clone())
    }

    fn fetch(
        &self,
        request: rustic_engine::ScriptRequest,
    ) -> Result<rustic_engine::ScriptResponse, rustic_engine::NetworkFailure> {
        self.0.script_fetch(request)
    }
}

impl NetworkSession {
    /// `fetch()` from the document: the same session, with the document's
    /// CSP (`connect-src`) and mixed-content check on every hop.
    fn script_fetch_impl(
        &self,
        request: rustic_engine::ScriptRequest,
    ) -> Result<rustic_engine::ScriptResponse, rustic_engine::NetworkFailure> {
        let session = self;
        if session.cancellation.check().is_err() {
            return Err(rustic_engine::NetworkFailure);
        }
        let mut headers = rustic_net::Headers::new();
        for (name, value) in &request.headers {
            headers
                .append(name, value)
                .map_err(|_| rustic_engine::NetworkFailure)?;
        }
        let fetch_request = FetchRequest {
            method: request.method,
            headers,
            body: request.body.unwrap_or_default(),
            mode: match request.mode.as_str() {
                "same-origin" => RequestMode::SameOrigin,
                "no-cors" => RequestMode::NoCors,
                _ => RequestMode::Cors,
            },
            credentials: match request.credentials.as_str() {
                "omit" => CredentialsMode::Omit,
                "include" => CredentialsMode::Include,
                _ => CredentialsMode::SameOrigin,
            },
            redirect: match request.redirect.as_str() {
                "error" => rustic_net::redirect::RedirectMode::Error,
                "manual" => rustic_net::redirect::RedirectMode::Manual,
                _ => rustic_net::redirect::RedirectMode::Follow,
            },
            referrer: Some(session.document_url.clone()),
            referrer_policy: ReferrerPolicy::from_token(&request.referrer_policy)
                .unwrap_or(session.referrer_policy),
            destination: Destination::Empty,
            top_level_site: session.frame.as_ref().map(|(site, _)| site.clone()),
            site_for_cookies: session.frame.as_ref().map_or_else(
                || Some(session.origin.clone()),
                |(_, for_cookies)| Some(for_cookies.clone()),
            ),
            ..FetchRequest::new(request.url, session.origin.clone())
        };
        let mut fetcher = session
            .fetcher
            .try_borrow_mut()
            .map_err(|_| rustic_engine::NetworkFailure)?;
        let response = fetcher
            .fetch_checked(fetch_request, &mut |hop, redirects| {
                if mixed_content::check(hop, Destination::Empty, session.secure)
                    != MixedContent::Allowed
                {
                    eprintln!("rustic: mixed content: blocked an insecure fetch()");
                    return false;
                }
                let verdict = session.csp.check_request(
                    hop,
                    FetchDirective::Connect,
                    &session.origin,
                    redirects,
                );
                report(&verdict);
                verdict.allowed()
            })
            .map_err(|_| rustic_engine::NetworkFailure)?;
        drop(fetcher);
        let kind = match response.response_type() {
            rustic_net::fetch::ResponseType::Basic => "basic",
            rustic_net::fetch::ResponseType::Cors => "cors",
            rustic_net::fetch::ResponseType::Opaque => "opaque",
            rustic_net::fetch::ResponseType::OpaqueRedirect => "opaqueredirect",
        };
        let url = response.url().cloned();
        let redirected = response.redirected();
        let status = response.status();
        let status_text = response.status_message().to_vec();
        let headers: Vec<_> = response
            .headers()
            .iter()
            .map(|(name, value)| (name.to_owned(), value.to_vec()))
            .collect();
        let mut content_type = headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-type"));
        let wasm = content_type
            .next()
            .is_some_and(|(_, value)| value.trim_ascii().eq_ignore_ascii_case(b"application/wasm"))
            && content_type.next().is_none();
        let in_flight = session.live_fetch_bodies.get();
        let (body, reader) =
            if wasm && matches!(kind, "basic" | "cors") && in_flight < MAX_STREAMED_FETCH_BODIES {
                session.live_fetch_bodies.set(in_flight + 1);
                let reader = WasmResponseReader {
                    response,
                    live_bodies: Rc::clone(&session.live_fetch_bodies),
                };
                (
                    Vec::new(),
                    Some(Rc::new(reader) as Rc<dyn rustic_engine::ScriptBodyReader>),
                )
            } else {
                let body = response
                    .read_body(NETWORK_TIMEOUT)
                    .map_err(|_| rustic_engine::NetworkFailure)?;
                (body, None)
            };
        Ok(rustic_engine::ScriptResponse {
            kind,
            url,
            redirected,
            status,
            status_text,
            headers,
            body,
            reader,
        })
    }
}

#[derive(Debug)]
struct WasmResponseReader {
    response: FetchResponse,
    live_bodies: Rc<Cell<usize>>,
}

impl Drop for WasmResponseReader {
    fn drop(&mut self) {
        self.live_bodies.set(self.live_bodies.get() - 1);
    }
}

impl rustic_engine::ScriptBodyReader for WasmResponseReader {
    fn next_chunk(&self) -> Result<Option<Vec<u8>>, rustic_engine::NetworkFailure> {
        self.response
            .next_body_chunk(NETWORK_TIMEOUT)
            .map_err(|_| rustic_engine::NetworkFailure)
    }
}

/// The document's CSP for inline `<style>` and `style` attributes.
#[derive(Debug)]
pub(crate) struct StylePolicy(pub(crate) Rc<dyn Session>);

impl rustic_style::InlineStylePolicy for StylePolicy {
    fn allows_style_element(&self, nonce: Option<&str>, text: &str) -> bool {
        let verdict =
            self.0
                .csp()
                .check_inline(rustic_net::csp::InlineKind::StyleElement, nonce, text);
        report(&verdict);
        verdict.allowed()
    }

    fn allows_style_attribute(&self, value: &str) -> bool {
        let verdict =
            self.0
                .csp()
                .check_inline(rustic_net::csp::InlineKind::StyleAttribute, None, value);
        report(&verdict);
        verdict.allowed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_need_http_origins_and_loopback_addresses() {
        assert!(parse_grant("http://a.test=127.0.0.1:8080").is_ok());
        assert!(parse_grant("https://a.test:8443=[::1]:8443").is_ok());
        assert!(parse_grant("http://a.test=192.0.2.1:80").is_err());
        assert!(parse_grant("ftp://a.test=127.0.0.1:21").is_err());
        assert!(parse_grant("http://a.test=localhost:80").is_err());
        // An origin alone resolves (TD-157).
        assert_eq!(parse_grant("https://a.test").unwrap().1, None);
        assert!(parse_grant("ftp://a.test").is_err());
        assert!(parse_grant("not a url").is_err());
    }

    #[test]
    fn network_arguments_are_http_urls() {
        assert!(is_network_argument(OsStr::new("http://a.test/")));
        assert!(is_network_argument(OsStr::new("HTTPS://a.test/")));
        assert!(!is_network_argument(OsStr::new("file:///a.html")));
        assert!(!is_network_argument(OsStr::new("page.html")));
    }

    #[test]
    fn script_and_style_mime_checks() {
        let script = ResourceKind::Script {
            nonce: None,
            parser_inserted: true,
            module: false,
        };
        let check = |kind, mime: Option<&str>, nosniff, same_origin| {
            let mime = mime.map(|input| MimeType::parse(input).unwrap());
            check_mime(kind, mime.as_ref(), nosniff, same_origin).is_ok()
        };
        assert!(check(&script, Some("text/javascript"), true, true));
        assert!(check(&script, Some("text/plain"), false, true));
        assert!(!check(&script, Some("text/plain"), true, true));
        assert!(!check(&script, Some("image/png"), false, true));
        assert!(!check(&script, None, true, true));
        let style = ResourceKind::Stylesheet { quirks: false };
        assert!(check(&style, Some("text/css; charset=utf-8"), false, true));
        assert!(!check(&style, Some("text/plain"), false, true));
        // Quirks mode takes a same-origin sheet of any type, unless
        // nosniff; a cross-origin one still needs text/css.
        let quirks = ResourceKind::Stylesheet { quirks: true };
        assert!(check(&quirks, Some("text/plain"), false, true));
        assert!(check(&quirks, None, false, true));
        assert!(!check(&quirks, Some("text/plain"), true, true));
        assert!(!check(&quirks, Some("text/plain"), false, false));
        assert!(check(&quirks, Some("text/css"), true, false));
        assert!(check(&ResourceKind::Image, None, true, true));
    }
}
