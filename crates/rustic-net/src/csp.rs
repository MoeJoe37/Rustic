//! Content Security Policy Level 3 (W3C CSP3, Editor's Draft 2026) for the
//! loading and execution paths Rustic implements: fetch directives with
//! their fallbacks, source-list matching (`*`, schemes, hosts with
//! wildcards, ports and paths, `'self'`, `'none'`), nonces and hashes for
//! inline scripts and styles, `'unsafe-inline'`, `'unsafe-hashes'`,
//! `'strict-dynamic'`, `'unsafe-eval'`, `form-action`, `base-uri`,
//! `frame-ancestors`, `sandbox` and `upgrade-insecure-requests`, with
//! report-only policies producing violations without blocking.
//!
//! Violation reports are returned to the caller (the console); sending them
//! (`report-uri`, `report-to`) is TD-163.

use std::fmt;

use ring::digest;
use rustic_url::{Origin, Url};

use crate::navigation::PolicyContainer;

/// Enforce or only report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Disposition {
    /// `Content-Security-Policy`.
    Enforce,
    /// `Content-Security-Policy-Report-Only`.
    Report,
}

/// One parsed policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Policy {
    directives: Vec<(String, Vec<String>)>,
    disposition: Disposition,
}

/// Most directives in one policy and tokens in one directive; the rest are
/// ignored (Chromium has no limit; these bound hostile headers).
const MAX_DIRECTIVES: usize = 128;
const MAX_TOKENS: usize = 1024;

impl Policy {
    /// §2.2.1 "parse a serialized CSP": directives split on `;`, names
    /// lowercased, the first of a repeated directive wins, values split on
    /// ASCII whitespace.
    pub fn parse(serialized: &str, disposition: Disposition) -> Self {
        let mut directives: Vec<(String, Vec<String>)> = Vec::new();
        for token in serialized.split(';') {
            let token = token.trim_matches(is_ascii_whitespace);
            if token.is_empty() {
                continue;
            }
            let mut parts = token
                .split(is_ascii_whitespace)
                .filter(|part| !part.is_empty());
            let name = parts.next().expect("non-empty").to_ascii_lowercase();
            if !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                continue;
            }
            if directives.iter().any(|(existing, _)| *existing == name)
                || directives.len() == MAX_DIRECTIVES
            {
                continue;
            }
            directives.push((name, parts.take(MAX_TOKENS).map(str::to_owned).collect()));
        }
        Self {
            directives,
            disposition,
        }
    }

    /// The value of `name`, if present.
    pub fn directive(&self, name: &str) -> Option<&[String]> {
        self.directives
            .iter()
            .find(|(directive, _)| directive == name)
            .map(|(_, value)| value.as_slice())
    }

    /// Enforce or report.
    pub fn disposition(&self) -> Disposition {
        self.disposition
    }

    /// The first present directive of `fallbacks`, with its name.
    fn effective<'a>(&'a self, fallbacks: &[&'static str]) -> Option<(&'static str, &'a [String])> {
        fallbacks
            .iter()
            .find_map(|&name| self.directive(name).map(|value| (name, value)))
    }
}

fn is_ascii_whitespace(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\x0c' | '\r' | ' ')
}

/// The fetch directives Rustic consults, by request destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FetchDirective {
    /// `<script src>`, module scripts.
    ScriptElement,
    /// Stylesheets.
    StyleElement,
    /// Images.
    Image,
    /// Fonts.
    Font,
    /// `fetch()`, XHR, WebSocket, EventSource.
    Connect,
    /// Nested browsing contexts.
    Frame,
    /// Media.
    Media,
    /// Workers.
    Worker,
}

impl FetchDirective {
    /// §6.8.3 fallback lists.
    fn fallbacks(self) -> &'static [&'static str] {
        match self {
            Self::ScriptElement => &["script-src-elem", "script-src", "default-src"],
            Self::StyleElement => &["style-src-elem", "style-src", "default-src"],
            Self::Image => &["img-src", "default-src"],
            Self::Font => &["font-src", "default-src"],
            Self::Connect => &["connect-src", "default-src"],
            Self::Frame => &["frame-src", "child-src", "default-src"],
            Self::Media => &["media-src", "default-src"],
            Self::Worker => &["worker-src", "child-src", "script-src", "default-src"],
        }
    }
}

/// What kind of inline content is checked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InlineKind {
    /// `<script>` text.
    ScriptElement,
    /// An event handler attribute or `javascript:` URL.
    ScriptAttribute,
    /// `<style>` text.
    StyleElement,
    /// A `style` attribute.
    StyleAttribute,
}

impl InlineKind {
    fn fallbacks(self) -> &'static [&'static str] {
        match self {
            Self::ScriptElement => &["script-src-elem", "script-src", "default-src"],
            Self::ScriptAttribute => &["script-src-attr", "script-src", "default-src"],
            Self::StyleElement => &["style-src-elem", "style-src", "default-src"],
            Self::StyleAttribute => &["style-src-attr", "style-src", "default-src"],
        }
    }

    fn is_script(self) -> bool {
        matches!(self, Self::ScriptElement | Self::ScriptAttribute)
    }

    fn is_attribute(self) -> bool {
        matches!(self, Self::ScriptAttribute | Self::StyleAttribute)
    }
}

/// A violation (§2.4), for the console and later reporting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Violation {
    /// The effective directive that was violated.
    pub directive: &'static str,
    /// The blocked URL (without fragment), or `inline` / `eval`.
    pub blocked: String,
    /// Whether the policy enforced (blocked) or only reported.
    pub disposition: Disposition,
}

impl fmt::Display for Violation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verb = match self.disposition {
            Disposition::Enforce => "Refused",
            Disposition::Report => "[Report Only] Would refuse",
        };
        write!(
            formatter,
            "{verb} to load or run '{}' because it violates the Content Security Policy directive '{}'",
            self.blocked, self.directive
        )
    }
}

/// The result of a check: whether it is allowed, and every violation
/// (enforced or reported).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Verdict {
    /// Whether an enforced policy blocked it.
    pub blocked: bool,
    /// Violations of enforced and report-only policies.
    pub violations: Vec<Violation>,
}

impl Verdict {
    fn record(&mut self, policy: &Policy, directive: &'static str, blocked: &str) {
        if policy.disposition == Disposition::Enforce {
            self.blocked = true;
        }
        self.violations.push(Violation {
            directive,
            blocked: blocked.to_owned(),
            disposition: policy.disposition,
        });
    }

    /// Whether nothing blocked it.
    pub fn allowed(&self) -> bool {
        !self.blocked
    }
}

/// A document's CSP list.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CspList {
    policies: Vec<Policy>,
}

impl CspList {
    /// Parse a policy container's header values; each value may hold
    /// several comma-separated policies.
    pub fn from_container(container: &PolicyContainer) -> Self {
        let mut policies = Vec::new();
        for (values, disposition) in [
            (&container.csp, Disposition::Enforce),
            (&container.csp_report_only, Disposition::Report),
        ] {
            for value in values {
                for serialized in value.split(',') {
                    let policy = Policy::parse(serialized, disposition);
                    if !policy.directives.is_empty() {
                        policies.push(policy);
                    }
                }
            }
        }
        Self { policies }
    }

    /// The policies.
    pub fn policies(&self) -> &[Policy] {
        &self.policies
    }

    /// Whether there are no policies.
    pub fn is_empty(&self) -> bool {
        self.policies.is_empty()
    }

    /// §4.1.3 "should request be blocked by CSP" for a fetch to `url`
    /// from a document of origin `self_origin`; `redirect_count` > 0 skips
    /// path matching (§6.7.2.8 step 4).
    pub fn check_request(
        &self,
        url: &Url,
        directive: FetchDirective,
        self_origin: &Origin,
        redirect_count: usize,
    ) -> Verdict {
        self.check_request_with_nonce(url, directive, self_origin, redirect_count, None, false)
    }

    /// Like [`Self::check_request`], for a script or style element with a
    /// `nonce` attribute, or a script loaded by a trusted script under
    /// `'strict-dynamic'` (`parser_inserted` false).
    pub fn check_request_with_nonce(
        &self,
        url: &Url,
        directive: FetchDirective,
        self_origin: &Origin,
        redirect_count: usize,
        nonce: Option<&str>,
        parser_inserted: bool,
    ) -> Verdict {
        let mut verdict = Verdict::default();
        let blocked = url.serialize_without_fragment();
        for policy in &self.policies {
            let Some((name, list)) = policy.effective(directive.fallbacks()) else {
                continue;
            };
            if nonce.is_some_and(|nonce| matches_nonce(list, nonce))
                && matches!(
                    directive,
                    FetchDirective::ScriptElement | FetchDirective::StyleElement
                )
            {
                continue;
            }
            if directive == FetchDirective::ScriptElement && has_keyword(list, "'strict-dynamic'") {
                // Only nonce/hash-trusted or script-inserted scripts load.
                if !parser_inserted {
                    continue;
                }
                verdict.record(policy, name, &blocked);
                continue;
            }
            if !matches_source_list(url, list, self_origin, redirect_count) {
                verdict.record(policy, name, &blocked);
            }
        }
        verdict
    }

    /// §6.7.3.3 "does element match source list for type and source" for
    /// inline content with an optional `nonce` and its `source` text.
    pub fn check_inline(&self, kind: InlineKind, nonce: Option<&str>, source: &str) -> Verdict {
        let mut verdict = Verdict::default();
        for policy in &self.policies {
            let Some((name, list)) = policy.effective(kind.fallbacks()) else {
                continue;
            };
            if !inline_allowed(list, kind, nonce, source) {
                verdict.record(policy, name, "inline");
            }
        }
        verdict
    }

    /// §4.4 "EnsureCSPDoesNotBlockStringCompilation": `'unsafe-eval'` in
    /// the effective `script-src`.
    pub fn check_eval(&self) -> Verdict {
        let mut verdict = Verdict::default();
        for policy in &self.policies {
            let Some((name, list)) = policy.effective(&["script-src", "default-src"]) else {
                continue;
            };
            if !has_keyword(list, "'unsafe-eval'") {
                verdict.record(policy, name, "eval");
            }
        }
        verdict
    }

    /// `form-action` for a submission to `url` (no fallback).
    pub fn check_form_action(&self, url: &Url, self_origin: &Origin) -> Verdict {
        self.check_named("form-action", url, self_origin)
    }

    /// `base-uri` for a `<base href>` of `url` (no fallback).
    pub fn check_base_uri(&self, url: &Url, self_origin: &Origin) -> Verdict {
        self.check_named("base-uri", url, self_origin)
    }

    /// `frame-ancestors` for a response embedded under `ancestors`
    /// (closest first); the policy belongs to the embedded response.
    pub fn check_frame_ancestors(&self, ancestors: &[Origin], self_origin: &Origin) -> Verdict {
        let mut verdict = Verdict::default();
        for policy in &self.policies {
            let Some(list) = policy.directive("frame-ancestors") else {
                continue;
            };
            for ancestor in ancestors {
                let url = Url::parse(&format!("{}/", ancestor.serialize()), None);
                let allowed = url.is_ok_and(|url| matches_source_list(&url, list, self_origin, 0));
                if !allowed {
                    verdict.record(policy, "frame-ancestors", &ancestor.serialize());
                    break;
                }
            }
        }
        verdict
    }

    fn check_named(&self, name: &'static str, url: &Url, self_origin: &Origin) -> Verdict {
        let mut verdict = Verdict::default();
        for policy in &self.policies {
            if let Some(list) = policy.directive(name) {
                if !matches_source_list(url, list, self_origin, 0) {
                    verdict.record(policy, name, &url.serialize_without_fragment());
                }
            }
        }
        verdict
    }

    /// Whether an enforced policy has `upgrade-insecure-requests`.
    pub fn upgrades_insecure_requests(&self) -> bool {
        self.policies.iter().any(|policy| {
            policy.disposition == Disposition::Enforce
                && policy.directive("upgrade-insecure-requests").is_some()
        })
    }

    /// The tokens of every enforced `sandbox` directive (empty token list
    /// means fully sandboxed), or `None` without one.
    pub fn sandbox(&self) -> Option<Vec<String>> {
        let mut found = None::<Vec<String>>;
        for policy in &self.policies {
            if policy.disposition != Disposition::Enforce {
                continue;
            }
            if let Some(tokens) = policy.directive("sandbox") {
                let tokens: Vec<String> = tokens
                    .iter()
                    .map(|token| token.to_ascii_lowercase())
                    .collect();
                found = Some(match found {
                    // Several policies: only tokens all of them allow.
                    Some(previous) => previous
                        .into_iter()
                        .filter(|token| tokens.contains(token))
                        .collect(),
                    None => tokens,
                });
            }
        }
        found
    }
}

/// Whether the enforced `sandbox` directives of these header values make
/// the document's origin opaque (no `allow-same-origin`).
pub fn sandboxes_origin(csp: &[String]) -> bool {
    let container = PolicyContainer {
        csp: csp.to_vec(),
        ..PolicyContainer::default()
    };
    CspList::from_container(&container)
        .sandbox()
        .is_some_and(|tokens| !tokens.iter().any(|token| token == "allow-same-origin"))
}

/// Upgrade Insecure Requests §4.1: `http:`→`https:`, `ws:`→`wss:`, keeping
/// a non-default port.
pub fn upgrade_url(url: &Url) -> Url {
    let scheme = match url.scheme() {
        "http" => "https",
        "ws" => "wss",
        _ => return url.clone(),
    };
    let text = url.serialize();
    let rest = &text[url.scheme().len()..];
    Url::parse(&format!("{scheme}{rest}"), None).unwrap_or_else(|_| url.clone())
}

fn has_keyword(list: &[String], keyword: &str) -> bool {
    list.iter().any(|token| token.eq_ignore_ascii_case(keyword))
}

fn matches_nonce(list: &[String], nonce: &str) -> bool {
    !nonce.is_empty()
        && list.iter().any(|token| {
            token.len() > 8
                && token[..7].eq_ignore_ascii_case("'nonce-")
                && token.ends_with('\'')
                && &token[7..token.len() - 1] == nonce
        })
}

/// Base64 of the SHA-256/384/512 of `source` matches a hash source.
fn matches_hash(list: &[String], source: &str) -> bool {
    for token in list {
        let Some(inner) = token
            .strip_prefix('\'')
            .and_then(|token| token.strip_suffix('\''))
        else {
            continue;
        };
        let Some((algorithm, value)) = inner.split_once('-') else {
            continue;
        };
        let algorithm = match algorithm.to_ascii_lowercase().as_str() {
            "sha256" => &digest::SHA256,
            "sha384" => &digest::SHA384,
            "sha512" => &digest::SHA512,
            _ => continue,
        };
        // base64url values are accepted as base64 (§2.3.1 note).
        let normalized: String = value
            .chars()
            .map(|character| match character {
                '-' => '+',
                '_' => '/',
                other => other,
            })
            .collect();
        let expected = base64(digest::digest(algorithm, source.as_bytes()).as_ref());
        if normalized.trim_end_matches('=') == expected.trim_end_matches('=') {
            return true;
        }
    }
    false
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                output.push(char::from(
                    ALPHABET[usize::try_from((value >> shift) & 63).expect("6 bits")],
                ));
            } else {
                output.push('=');
            }
        }
    }
    output
}

fn inline_allowed(list: &[String], kind: InlineKind, nonce: Option<&str>, source: &str) -> bool {
    if !kind.is_attribute() && nonce.is_some_and(|nonce| matches_nonce(list, nonce)) {
        return true;
    }
    let hashes_apply = !kind.is_attribute() || has_keyword(list, "'unsafe-hashes'");
    if hashes_apply && matches_hash(list, source) {
        return true;
    }
    // §6.7.3.2: 'unsafe-inline' is ignored when a nonce or hash source (or,
    // for scripts, 'strict-dynamic') is present.
    let has_nonce_or_hash = list.iter().any(|token| {
        let lower = token.to_ascii_lowercase();
        lower.starts_with("'nonce-")
            || lower.starts_with("'sha256-")
            || lower.starts_with("'sha384-")
            || lower.starts_with("'sha512-")
    });
    let strict = kind.is_script() && has_keyword(list, "'strict-dynamic'");
    has_keyword(list, "'unsafe-inline'") && !has_nonce_or_hash && !strict
}

/// §6.7.2.5 "does url match source list in origin with redirect count".
pub fn matches_source_list(
    url: &Url,
    list: &[String],
    origin: &Origin,
    redirect_count: usize,
) -> bool {
    list.iter()
        .any(|token| matches_expression(url, token, origin, redirect_count))
}

fn origin_parts(origin: &Origin) -> Option<(String, String, Option<u16>)> {
    if origin.is_opaque() {
        return None;
    }
    let url = Url::parse(&format!("{}/", origin.serialize()), None).ok()?;
    Some((url.scheme().to_owned(), url.host()?.serialize(), url.port()))
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        _ => None,
    }
}

/// §6.7.2.6 "scheme-part match": equal, or an upgrade.
fn scheme_part_match(source: &str, url: &str) -> bool {
    source.eq_ignore_ascii_case(url)
        || (source.eq_ignore_ascii_case("http") && url == "https")
        || (source.eq_ignore_ascii_case("ws") && matches!(url, "wss" | "http" | "https"))
        || (source.eq_ignore_ascii_case("wss") && url == "https")
}

fn matches_expression(url: &Url, token: &str, origin: &Origin, redirect_count: usize) -> bool {
    let lower = token.to_ascii_lowercase();
    if lower == "*" {
        let own_scheme = origin_parts(origin).map(|(scheme, _, _)| scheme);
        return matches!(url.scheme(), "http" | "https" | "ws" | "wss")
            || own_scheme.as_deref() == Some(url.scheme());
    }
    if lower == "'self'" {
        let Some((scheme, host, port)) = origin_parts(origin) else {
            return false;
        };
        let Some(url_host) = url.host().map(rustic_url::Host::serialize) else {
            return false;
        };
        if url_host != host {
            return false;
        }
        let url_port = url.port().or_else(|| default_port(url.scheme()));
        let own_port = port.or_else(|| default_port(&scheme));
        let same = scheme == url.scheme() && url_port == own_port;
        let upgrade = (scheme == "http"
            && url.scheme() == "https"
            && (url_port == Some(443) || url_port == own_port))
            || (scheme == "http" && matches!(url.scheme(), "ws" | "wss"))
            || (scheme == "https" && url.scheme() == "wss");
        return same || upgrade;
    }
    if lower.starts_with('\'') {
        // Keywords, nonces and hashes never match URLs.
        return false;
    }
    // scheme-source: `https:`.
    if let Some(scheme) = lower.strip_suffix(':') {
        if !scheme.is_empty()
            && scheme
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"+-.".contains(&byte))
        {
            return scheme_part_match(scheme, url.scheme());
        }
    }
    host_source_match(url, &lower, origin, redirect_count)
}

/// §6.7.2.7 host-source.
fn host_source_match(url: &Url, source: &str, origin: &Origin, redirect_count: usize) -> bool {
    let Some(url_host) = url.host().map(rustic_url::Host::serialize) else {
        return false;
    };
    let (scheme, rest) = match source.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, source),
    };
    match scheme {
        Some(scheme) => {
            if !scheme_part_match(scheme, url.scheme()) {
                return false;
            }
        }
        None => {
            // No scheme: the protected resource's scheme, with upgrades.
            let own = origin_parts(origin).map(|(scheme, _, _)| scheme);
            let ok = match own.as_deref() {
                Some("http") => matches!(url.scheme(), "http" | "https"),
                Some(own) => own == url.scheme(),
                None => false,
            };
            if !ok {
                return false;
            }
        }
    }
    let path_start = rest.find('/').unwrap_or(rest.len());
    let (authority, path) = rest.split_at(path_start);
    let (host, port) = if authority.starts_with('[') {
        match authority.find(']') {
            Some(end) => (&authority[..=end], authority[end + 1..].strip_prefix(':')),
            None => return false,
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    // host-part match.
    let host_ok = if let Some(suffix) = host.strip_prefix("*.") {
        url_host.ends_with(&format!(".{suffix}"))
    } else if host == "*" {
        true
    } else {
        host == url_host
    };
    // Hosts that are IP addresses match only literally, never by wildcard.
    if !host_ok || (host.starts_with("*.") && url.host().is_some_and(|h| h.domain().is_none())) {
        return false;
    }
    let url_port = url.port().or_else(|| default_port(url.scheme()));
    let port_ok = match port {
        Some("*") => true,
        Some(port) => {
            let Ok(port) = port.parse::<u16>() else {
                return false;
            };
            Some(port) == url_port
                || (port == 80 && matches!(url.scheme(), "https" | "wss") && url_port == Some(443))
        }
        None => {
            let source_scheme = scheme
                .map(str::to_owned)
                .or_else(|| origin_parts(origin).map(|(scheme, _, _)| scheme));
            let expected = source_scheme.as_deref().and_then(default_port);
            url_port == expected
                || (expected == Some(80)
                    && url_port == Some(443)
                    && matches!(url.scheme(), "https" | "wss"))
        }
    };
    if !port_ok {
        return false;
    }
    if path.is_empty() || redirect_count > 0 {
        return true;
    }
    let url_path = url.pathname();
    let decoded_source = percent_decode_text(path);
    let decoded_url = percent_decode_text(&url_path);
    if decoded_source.ends_with('/') {
        decoded_url.starts_with(&decoded_source)
    } else {
        decoded_url == decoded_source
    }
}

fn percent_decode_text(text: &str) -> String {
    let bytes = text.as_bytes();
    let hex = |byte: u8| char::from(byte).to_digit(16);
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if let (Some(high), Some(low)) = (
                bytes.get(index + 1).copied().and_then(hex),
                bytes.get(index + 2).copied().and_then(hex),
            ) {
                output.push(u8::try_from(high * 16 + low).expect("two hex digits"));
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

#[cfg(test)]
mod tests;
