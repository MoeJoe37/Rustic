//! Loading documents and subresources over HTTP(S) for the development
//! command line (M6, NET-02/NET-05).
//!
//! The network is off unless the user grants origins: `--allow-origin
//! ORIGIN=ADDR` routes one `http`/`https` origin to a loopback socket
//! address (the transport has no DNS and refuses other addresses, ADR
//! 0051), and `--ca-file PEM` replaces the platform trust store with the
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

use std::cell::RefCell;
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
use rustic_net::fetch::{Destination, FetchRequest, Fetcher, NetworkError, RequestMode};
use rustic_net::mime::{MimeType, extract_mime_type};
use rustic_net::mixed_content::{self, MixedContent};
use rustic_net::navigation::{
    DocumentKind, Navigation, NavigationError, NavigationOutcome, NavigationRequest,
    PolicyContainer,
};
use rustic_net::referrer::ReferrerPolicy;
use rustic_net::site::PublicSuffixList;
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
/// Most distinct stylesheet charsets remembered per document.
const MAX_REMEMBERED_CHARSETS: usize = 4096;

/// The user's network grants, parsed once from the arguments.
#[derive(Default)]
struct NetworkConfig {
    routes: Vec<(Url, SocketAddr)>,
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

/// `ORIGIN=ADDRESS`, for example `http://a.test=127.0.0.1:8080`.
fn parse_grant(value: &str) -> Result<(Url, SocketAddr), String> {
    let invalid = || {
        format!(
            "--allow-origin {}: expected ORIGIN=ADDRESS with an http(s) origin and a loopback \
             address",
            EscapedText(value)
        )
    };
    let (origin, address) = value.rsplit_once('=').ok_or_else(invalid)?;
    let origin = Url::parse(origin, None).map_err(|_| invalid())?;
    let address: SocketAddr = address.parse().map_err(|_| invalid())?;
    NetworkPolicy::deny_all()
        .allow(&origin, address)
        .map_err(|_| invalid())?;
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

/// Whether any `--allow-origin` grant was given.
pub fn has_grants() -> bool {
    CONFIG.get().is_some_and(|config| !config.routes.is_empty())
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
    /// The profile could not be read.
    Profile(String),
}

impl fmt::Display for NetworkLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl => formatter.write_str("not a valid URL"),
            Self::NoGrants => formatter
                .write_str("network loading needs --allow-origin ORIGIN=ADDRESS for each origin"),
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
    pub(crate) bytes: Vec<u8>,
    /// The transport-layer encoding (response `charset`).
    pub(crate) transport_encoding: Option<Encoding>,
}

impl From<NetworkDocument> for SessionDocument {
    fn from(document: NetworkDocument) -> Self {
        Self {
            session: document.session,
            bytes: document.bytes,
            transport_encoding: document.transport_encoding,
        }
    }
}

/// What a subresource request is for.
#[derive(Clone, Debug)]
pub(crate) enum ResourceKind {
    /// A `<link rel=stylesheet>` or `@import`.
    Stylesheet,
    /// An `<img>`.
    Image,
    /// A classic or module script.
    Script {
        nonce: Option<String>,
        parser_inserted: bool,
        module: bool,
    },
}

/// The connection state one network document's subresources share.
pub(crate) struct NetworkSession {
    fetcher: RefCell<Fetcher>,
    document_url: Url,
    origin: Origin,
    csp: CspList,
    secure: bool,
    referrer_policy: ReferrerPolicy,
    cancellation: CancellationToken,
    charsets: RefCell<HashMap<String, String>>,
    /// The document's COOP, COEP and origin-keying (ADR 0057).
    isolation: rustic_net::isolation::IsolationPolicy,
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
    if config.routes.is_empty() {
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
    let bytes = committed
        .body
        .read_all(NETWORK_TIMEOUT)
        .map_err(NetworkLoadError::Body)?;
    if bytes.len() > max_bytes {
        return Err(NetworkLoadError::TooLarge(max_bytes));
    }
    let mut session = NetworkSession::new(fetcher, document_url, origin, &policy, cancellation);
    session.isolation = isolation;
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
    for (origin, address) in &config.routes {
        policy
            .allow(origin, *address)
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
            fetcher: RefCell::new(fetcher),
            document_url,
            origin,
            csp: CspList::from_container(policy),
            secure,
            referrer_policy: policy.referrer_policy,
            cancellation,
            charsets: RefCell::new(HashMap::new()),
            isolation: rustic_net::isolation::IsolationPolicy::default(),
        }
    }

    /// Save the jar's persistent cookies in the profile.
    pub(crate) fn save_cookies(&self) -> Result<(), rustic_storage::ProfileError> {
        let bytes = self.fetcher.borrow().cookies().export_persistent(now());
        crate::storage::save_cookies(bytes)
    }

    /// Fetch one subresource of the document, at most `max_bytes`.
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
        let (destination, directive) = match kind {
            ResourceKind::Stylesheet => (Destination::Style, FetchDirective::StyleElement),
            ResourceKind::Image => (Destination::Image, FetchDirective::Image),
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
        let module = matches!(kind, ResourceKind::Script { module: true, .. });
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
            top_level_site: None,
            ..FetchRequest::new(target, self.origin.clone())
        };
        let mut fetcher = self
            .fetcher
            .try_borrow_mut()
            .map_err(|_| FetchError::Failed)?;
        let response = fetcher
            .fetch_checked(request, &mut |hop, redirects| {
                if mixed_content::check(hop, destination, self.secure) == MixedContent::Blocked {
                    return false;
                }
                let verdict = self.csp.check_request_with_nonce(
                    hop,
                    directive,
                    &self.origin,
                    redirects,
                    nonce,
                    parser_inserted,
                );
                report(&verdict);
                verdict.allowed()
            })
            .map_err(|error| match error {
                NetworkError::BlockedByClientPolicy
                | NetworkError::BlockedByResourcePolicy
                | NetworkError::Cors(_)
                | NetworkError::Preflight(_)
                | NetworkError::Transport(rustic_net::TransportError::Blocked) => {
                    FetchError::Blocked
                }
                _ => FetchError::Failed,
            })?;
        drop(fetcher);
        // Opaque responses (status 0) carry a body the renderer may use
        // (images, classic scripts and stylesheets loaded no-cors).
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
        check_mime(kind, mime.as_ref(), nosniff)?;
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
        if matches!(kind, ResourceKind::Stylesheet | ResourceKind::Script { .. }) {
            let label = mime.as_ref().and_then(|mime| mime.parameter("charset"));
            let mut charsets = self.charsets.borrow_mut();
            if let Some(label) = label.filter(|_| charsets.len() < MAX_REMEMBERED_CHARSETS) {
                charsets.insert(url.serialize(), label.to_owned());
            }
        }
        Ok(bytes)
    }
}

/// Fetch §2.2.6 "should response to request be blocked due to its MIME
/// type" and "due to nosniff", and HTML's stylesheet type check (a
/// stylesheet must be `text/css` in a standards-mode document; quirks
/// mode's same-origin exception is not implemented).
fn check_mime(
    kind: &ResourceKind,
    mime: Option<&MimeType>,
    nosniff: bool,
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
        ResourceKind::Stylesheet => {
            if !mime.is_some_and(|mime| mime.essence() == "text/css") {
                eprintln!("rustic: skipped a stylesheet whose MIME type is not text/css");
                return Err(FetchError::Blocked);
            }
        }
        ResourceKind::Image => {}
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
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| (name.to_owned(), value.to_vec()))
            .collect();
        let body = response
            .read_body(NETWORK_TIMEOUT)
            .map_err(|_| rustic_engine::NetworkFailure)?;
        Ok(rustic_engine::ScriptResponse {
            kind,
            url,
            redirected,
            status,
            status_text,
            headers,
            body,
        })
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
        assert!(parse_grant("http://a.test").is_err());
        assert!(parse_grant("http://a.test=localhost:80").is_err());
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
        let mime = |input: &str| MimeType::parse(input).unwrap();
        assert!(check_mime(&script, Some(&mime("text/javascript")), true).is_ok());
        assert!(check_mime(&script, Some(&mime("text/plain")), false).is_ok());
        assert!(check_mime(&script, Some(&mime("text/plain")), true).is_err());
        assert!(check_mime(&script, Some(&mime("image/png")), false).is_err());
        assert!(check_mime(&script, None, true).is_err());
        let style = ResourceKind::Stylesheet;
        assert!(check_mime(&style, Some(&mime("text/css; charset=utf-8")), false).is_ok());
        assert!(check_mime(&style, Some(&mime("text/plain")), false).is_err());
        assert!(check_mime(&ResourceKind::Image, None, true).is_ok());
    }
}
