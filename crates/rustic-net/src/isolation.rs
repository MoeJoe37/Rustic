//! Site isolation decisions (ISO-01, ADR 0057): which agent cluster and
//! renderer process a document belongs to, when a navigation must change
//! browsing context group, and which cross-origin responses a renderer may
//! receive at all.
//!
//! * Cross-Origin-Opener-Policy (HTML §7.1.3) and Cross-Origin-Embedder-
//!   Policy (HTML §7.1.4) parsing, "cross-origin isolated", and the COOP
//!   matching that decides a browsing context group switch.
//! * `Origin-Agent-Cluster` (HTML §7.3.3.1): origin-keyed agent clusters.
//! * Process keys: one renderer per site for `http(s)` documents (per
//!   origin when origin-keyed or cross-origin isolated); every opaque,
//!   `file:` or `data:` document gets a process of its own.
//! * Opaque Response Blocking (Fetch, "opaque-response blocking", the
//!   2023 ORB proposal as Chromium and Firefox ship it): a no-cors
//!   cross-origin response that is HTML, JSON or XML, or that sniffs as
//!   such, never reaches a renderer, so a compromised renderer cannot read
//!   another site's documents through `<img>` or `<script>`.

use rustic_url::{Origin, Url};

use crate::Headers;
use crate::fetch::Destination;
use crate::mime::{MimeType, extract_mime_type};
use crate::site::{PublicSuffixList, Site};

/// HTML "cross-origin opener policy value".
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OpenerPolicy {
    /// `unsafe-none` (the default).
    #[default]
    UnsafeNone,
    /// `same-origin-allow-popups`.
    SameOriginAllowPopups,
    /// `same-origin`.
    SameOrigin,
    /// `same-origin` together with COEP `require-corp`.
    SameOriginPlusCoep,
    /// `noopener-allow-popups`.
    NoopenerAllowPopups,
}

/// HTML "embedder policy value".
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EmbedderPolicy {
    /// `unsafe-none` (the default).
    #[default]
    UnsafeNone,
    /// `require-corp`: cross-origin no-cors resources need CORP.
    RequireCorp,
    /// `credentialless`: cross-origin no-cors requests omit credentials.
    Credentialless,
}

/// The isolation-related policies a response sets.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IsolationPolicy {
    /// The enforced COOP.
    pub opener: OpenerPolicy,
    /// The enforced COEP.
    pub embedder: EmbedderPolicy,
    /// `Origin-Agent-Cluster: ?1`.
    pub origin_keyed: bool,
}

/// The first token of a structured-field item, without parameters.
fn token(value: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(value).ok()?.trim();
    let item = text.split(';').next()?.trim();
    (!item.is_empty() && !item.contains(',')).then(|| item.to_owned())
}

impl IsolationPolicy {
    /// Parse a response's `Cross-Origin-Opener-Policy`,
    /// `Cross-Origin-Embedder-Policy` and `Origin-Agent-Cluster` headers.
    /// The policies apply only to secure contexts (HTML "obtain a
    /// cross-origin opener policy" step 2), so `secure` gates them.
    pub fn from_headers(headers: &Headers, secure: bool) -> Self {
        let mut policy = Self {
            origin_keyed: headers
                .combined("origin-agent-cluster")
                .is_some_and(|value| value.trim_ascii() == b"?1"),
            ..Self::default()
        };
        if !secure {
            return policy;
        }
        policy.embedder = match headers
            .combined("cross-origin-embedder-policy")
            .and_then(|value| token(&value))
            .as_deref()
        {
            Some("require-corp") => EmbedderPolicy::RequireCorp,
            Some("credentialless") => EmbedderPolicy::Credentialless,
            _ => EmbedderPolicy::UnsafeNone,
        };
        policy.opener = match headers
            .combined("cross-origin-opener-policy")
            .and_then(|value| token(&value))
            .as_deref()
        {
            Some("same-origin") if policy.embedder != EmbedderPolicy::UnsafeNone => {
                OpenerPolicy::SameOriginPlusCoep
            }
            Some("same-origin") => OpenerPolicy::SameOrigin,
            Some("same-origin-allow-popups") => OpenerPolicy::SameOriginAllowPopups,
            Some("noopener-allow-popups") => OpenerPolicy::NoopenerAllowPopups,
            _ => OpenerPolicy::UnsafeNone,
        };
        policy
    }

    /// HTML "cross-origin isolated": COOP `same-origin` with a COEP.
    pub fn cross_origin_isolated(&self) -> bool {
        self.opener == OpenerPolicy::SameOriginPlusCoep
    }
}

/// HTML "check if COOP values require a browsing context group switch"
/// for a navigation from a document with `current` COOP at
/// `current_origin` to a response with `next` COOP at `next_origin`.
pub fn requires_group_switch(
    is_initial_about_blank: bool,
    current: OpenerPolicy,
    current_origin: &Origin,
    next: OpenerPolicy,
    next_origin: &Origin,
) -> bool {
    if current == OpenerPolicy::UnsafeNone && next == OpenerPolicy::UnsafeNone {
        return false;
    }
    if is_initial_about_blank
        && current == OpenerPolicy::UnsafeNone
        && next == OpenerPolicy::SameOriginAllowPopups
    {
        return false;
    }
    !(current == next && current_origin.is_same_origin(next_origin))
}

/// Which agent cluster (and so which renderer) a document belongs to.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ProcessKey {
    /// Every same-site `http(s)` document, by serialized site.
    Site(String),
    /// An origin-keyed or cross-origin isolated document, by origin.
    Origin(String),
    /// A document that shares its process with nothing (opaque origins,
    /// `file:`, `data:`); the number is unique per document.
    Unique(u64),
}

/// The process key of a document at `url` with `policy`. `next_unique`
/// supplies a fresh number for documents that share with nothing.
pub fn process_key(
    url: &Url,
    policy: &IsolationPolicy,
    suffixes: &PublicSuffixList,
    next_unique: &mut u64,
) -> ProcessKey {
    let origin = url.origin();
    if origin.is_opaque() || !matches!(url.scheme(), "http" | "https") {
        *next_unique += 1;
        return ProcessKey::Unique(*next_unique);
    }
    if policy.origin_keyed || policy.cross_origin_isolated() {
        return ProcessKey::Origin(origin.serialize());
    }
    ProcessKey::Site(Site::serialize(&suffixes.site(&origin)))
}

/// Whether a document with key `current` may stay in its renderer when it
/// navigates to a document with key `next`. A different key, or any
/// `Unique` key, always needs a fresh renderer: renderers never hold two
/// sites, and authority never carries over.
pub fn may_reuse(current: &ProcessKey, next: &ProcessKey) -> bool {
    current == next && !matches!(current, ProcessKey::Unique(_))
}

/// MIME types ORB never lets through to a no-cors cross-origin request:
/// documents and data that no `<img>`, `<script>`, `<audio>` or
/// `<video>` can use.
fn orb_blocked_mime(mime: &MimeType) -> bool {
    let essence = mime.essence();
    let subtype = mime.subtype();
    matches!(
        essence.as_str(),
        "text/html"
            | "text/xml"
            | "application/xml"
            | "text/json"
            | "application/json"
            | "text/plain"
            | "application/pdf"
            | "application/zip"
            | "application/gzip"
            | "application/msword"
            | "text/csv"
            | "application/x-protobuf"
            | "application/dash+xml"
            | "text/event-stream"
    ) || subtype.ends_with("+json")
        || subtype.ends_with("+xml") && essence != "image/svg+xml"
}

/// Whether bytes start like JSON or an HTML/XML document (ORB's sniffing
/// for responses without a usable type).
fn sniffs_as_document(body: &[u8]) -> bool {
    let start = body
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map_or(&[][..], |index| &body[index..]);
    let lower: Vec<u8> = start.iter().take(16).map(u8::to_ascii_lowercase).collect();
    lower.starts_with(b"<!doctype html")
        || lower.starts_with(b"<html")
        || lower.starts_with(b"<?xml")
        || lower.starts_with(b")]}'")
        || lower.starts_with(b"{\"")
        || lower.starts_with(b"[\"")
        || lower.starts_with(b"[{")
}

/// Opaque Response Blocking for a no-cors response to a request with
/// `destination` from `client` for `url`: `true` means the renderer gets
/// a network error instead. Same-origin responses and CORS responses are
/// never blocked here; stylesheets and scripts have their own MIME checks
/// and are checked again with `text/plain` and the document types.
pub fn opaque_response_blocked(
    client: &Origin,
    url: &Url,
    destination: Destination,
    headers: &Headers,
    body: &[u8],
) -> bool {
    if client.is_same_origin(&url.origin()) {
        return false;
    }
    let mime = extract_mime_type(headers);
    let nosniff = headers
        .combined("x-content-type-options")
        .is_some_and(|value| value.trim_ascii().eq_ignore_ascii_case(b"nosniff"));
    match &mime {
        Some(mime) if orb_blocked_mime(mime) => return true,
        Some(mime) if nosniff => {
            // A nosniff response must carry a type its destination uses.
            let essence = mime.essence();
            let fits = match destination {
                Destination::Image => mime.kind() == "image",
                Destination::Script => mime.is_javascript(),
                Destination::Style => essence == "text/css",
                _ => true,
            };
            if !fits {
                return true;
            }
        }
        _ => {}
    }
    // An image, media or script under a usable type passes; an unknown or
    // missing type is sniffed.
    let usable = mime.as_ref().is_some_and(|mime| {
        matches!(mime.kind(), "image" | "audio" | "video")
            || mime.is_javascript()
            || mime.essence() == "text/css"
    });
    !usable && sniffs_as_document(body)
}

/// HTML "cross-origin embedder policy allows" for a no-cors response: a
/// `require-corp` embedder needs `Cross-Origin-Resource-Policy` on every
/// cross-origin resource.
pub fn embedder_allows(
    embedder: EmbedderPolicy,
    client: &Origin,
    url: &Url,
    headers: &Headers,
) -> bool {
    if embedder != EmbedderPolicy::RequireCorp || client.is_same_origin(&url.origin()) {
        return true;
    }
    headers.contains("cross-origin-resource-policy")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(fields: &[(&str, &str)]) -> Headers {
        let mut headers = Headers::new();
        for (name, value) in fields {
            headers.append(name, value.as_bytes()).unwrap();
        }
        headers
    }

    fn url(input: &str) -> Url {
        Url::parse(input, None).unwrap()
    }

    #[test]
    fn policies_parse_and_apply_only_to_secure_contexts() {
        let isolated = headers(&[
            ("Cross-Origin-Opener-Policy", "same-origin; report-to=\"x\""),
            ("Cross-Origin-Embedder-Policy", "require-corp"),
            ("Origin-Agent-Cluster", "?1"),
        ]);
        let policy = IsolationPolicy::from_headers(&isolated, true);
        assert!(policy.cross_origin_isolated());
        assert!(policy.origin_keyed);
        let insecure = IsolationPolicy::from_headers(&isolated, false);
        assert_eq!(insecure.opener, OpenerPolicy::UnsafeNone);
        assert_eq!(insecure.embedder, EmbedderPolicy::UnsafeNone);
        assert!(insecure.origin_keyed);
        let popups = headers(&[("Cross-Origin-Opener-Policy", "same-origin-allow-popups")]);
        assert_eq!(
            IsolationPolicy::from_headers(&popups, true).opener,
            OpenerPolicy::SameOriginAllowPopups
        );
        let invalid = headers(&[
            ("Cross-Origin-Opener-Policy", "same-origin, unsafe-none"),
            ("Cross-Origin-Embedder-Policy", "bogus"),
        ]);
        assert_eq!(
            IsolationPolicy::from_headers(&invalid, true),
            IsolationPolicy::default()
        );
    }

    #[test]
    fn coop_decides_browsing_context_group_switches() {
        let a = url("https://a.test/").origin();
        let b = url("https://b.test/").origin();
        use OpenerPolicy::*;
        assert!(!requires_group_switch(
            false, UnsafeNone, &a, UnsafeNone, &b
        ));
        assert!(requires_group_switch(false, UnsafeNone, &a, SameOrigin, &a));
        assert!(!requires_group_switch(
            false, SameOrigin, &a, SameOrigin, &a
        ));
        assert!(requires_group_switch(false, SameOrigin, &a, SameOrigin, &b));
        assert!(!requires_group_switch(
            true,
            UnsafeNone,
            &a,
            SameOriginAllowPopups,
            &b
        ));
        assert!(requires_group_switch(
            false,
            SameOriginPlusCoep,
            &a,
            SameOrigin,
            &a
        ));
    }

    #[test]
    fn process_keys_follow_sites_origins_and_opaque_documents() {
        let suffixes = PublicSuffixList::parse("test\nco.uk\n").unwrap();
        let mut unique = 0;
        let mut key = |input: &str, policy: IsolationPolicy| {
            process_key(&url(input), &policy, &suffixes, &mut unique)
        };
        let plain = IsolationPolicy::default();
        let a = key("https://www.a.test/x", plain);
        assert_eq!(a, ProcessKey::Site("https://a.test".into()));
        assert_eq!(key("https://mail.a.test/", plain), a);
        assert_ne!(key("http://www.a.test/", plain), a);
        assert_ne!(
            key("https://b.co.uk/", plain),
            key("https://c.co.uk/", plain)
        );
        let keyed = IsolationPolicy {
            origin_keyed: true,
            ..plain
        };
        assert_eq!(
            key("https://www.a.test/", keyed),
            ProcessKey::Origin("https://www.a.test".into())
        );
        let first = key("file:///a.html", plain);
        let second = key("file:///a.html", plain);
        assert_ne!(first, second);
        assert!(matches!(
            key("data:text/html,x", plain),
            ProcessKey::Unique(_)
        ));
        assert!(may_reuse(&a, &a.clone()));
        assert!(!may_reuse(&first, &first.clone()));
        assert!(!may_reuse(&a, &ProcessKey::Site("https://b.test".into())));
    }

    #[test]
    fn orb_blocks_cross_origin_documents_and_data() {
        let client = url("https://a.test/").origin();
        let other = url("https://b.test/data");
        let check = |fields: &[(&str, &str)], body: &[u8], destination| {
            opaque_response_blocked(&client, &other, destination, &headers(fields), body)
        };
        use Destination::*;
        assert!(check(&[("Content-Type", "application/json")], b"{}", Image));
        assert!(check(&[("Content-Type", "text/html")], b"<p>", Script));
        assert!(check(
            &[("Content-Type", "application/ld+json")],
            b"{}",
            Image
        ));
        assert!(check(
            &[("Content-Type", "application/rss+xml")],
            b"<",
            Image
        ));
        assert!(!check(&[("Content-Type", "image/svg+xml")], b"<svg", Image));
        assert!(!check(&[("Content-Type", "image/png")], b"\x89PNG", Image));
        assert!(!check(
            &[("Content-Type", "text/javascript")],
            b"x()",
            Script
        ));
        assert!(check(&[], b"  <!DOCTYPE html><p>secret", Image));
        assert!(check(&[], b"{\"secret\": 1}", Image));
        assert!(!check(&[], b"GIF89a", Image));
        assert!(check(
            &[
                ("Content-Type", "text/javascript"),
                ("X-Content-Type-Options", "nosniff")
            ],
            b"x",
            Image
        ));
        // Same-origin responses are never ORB's business.
        assert!(!opaque_response_blocked(
            &client,
            &url("https://a.test/data"),
            Image,
            &headers(&[("Content-Type", "application/json")]),
            b"{}"
        ));
    }

    #[test]
    fn require_corp_needs_resource_policy_cross_origin() {
        let client = url("https://a.test/").origin();
        let other = url("https://b.test/x.png");
        let none = headers(&[]);
        let corp = headers(&[("Cross-Origin-Resource-Policy", "cross-origin")]);
        assert!(!embedder_allows(
            EmbedderPolicy::RequireCorp,
            &client,
            &other,
            &none
        ));
        assert!(embedder_allows(
            EmbedderPolicy::RequireCorp,
            &client,
            &other,
            &corp
        ));
        assert!(embedder_allows(
            EmbedderPolicy::UnsafeNone,
            &client,
            &other,
            &none
        ));
        assert!(embedder_allows(
            EmbedderPolicy::RequireCorp,
            &client,
            &url("https://a.test/y"),
            &none
        ));
    }
}
