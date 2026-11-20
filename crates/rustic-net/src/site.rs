//! Public suffixes, registrable domains and sites (URL Standard §3.2
//! "host public suffix" and "host registrable domain", HTML "site" and
//! "same site").
//!
//! [`PublicSuffixList::bundled`] is the list vendored at a pinned
//! revision (`crates/rustic-net/data`, ADR 0054); a host may supply a newer
//! one through [`PublicSuffixList::parse`].

use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, OnceLock};

use rustic_url::{Host, Origin, Url};

/// A parsed Public Suffix List (publicsuffix.org format).
///
/// Rules are stored lowercase with their wildcard and exception markers.
/// Matching follows the list's published algorithm: an exception rule wins,
/// otherwise the rule with the most labels; with no match the implicit
/// rule `*` applies.
#[derive(Clone, Default)]
pub struct PublicSuffixList(Arc<Rules>);

#[derive(Default)]
struct Rules {
    rules: HashSet<String>,
    wildcards: HashSet<String>,
    exceptions: HashSet<String>,
}

/// The vendored list (publicsuffix/list at the revision in
/// `data/psl_manifest.json`).
const BUNDLED: &str = include_str!("../data/public_suffix_list.dat");

/// Limits for [`PublicSuffixList::parse`].
const MAX_LIST_BYTES: usize = 4 * 1024 * 1024;
const MAX_RULE_LABELS: usize = 127;

/// The list text could not be used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicSuffixError {
    /// The text is larger than 4 MiB.
    TooLarge,
    /// A rule is not a sequence of non-empty labels (1-based line).
    InvalidRule(usize),
}

impl fmt::Display for PublicSuffixError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => formatter.write_str("public suffix list is too large"),
            Self::InvalidRule(line) => {
                write!(formatter, "invalid public suffix rule on line {line}")
            }
        }
    }
}

impl std::error::Error for PublicSuffixError {}

impl PublicSuffixList {
    /// Only the implicit `*` rule: every domain's last label is its
    /// public suffix. For tests that want no list.
    pub fn fallback() -> Self {
        Self::default()
    }

    /// The vendored list, parsed once per process and shared.
    pub fn bundled() -> Self {
        static LIST: OnceLock<PublicSuffixList> = OnceLock::new();
        LIST.get_or_init(|| Self::parse(BUNDLED).expect("the vendored list is valid"))
            .clone()
    }

    /// Parse the list's text format: one rule per line, `//` comments,
    /// rules end at the first whitespace, `*.` wildcards and `!`
    /// exceptions. Unicode rules are converted with the URL Standard's
    /// domain-to-ASCII, because hosts reach this module in ASCII form.
    pub fn parse(text: &str) -> Result<Self, PublicSuffixError> {
        if text.len() > MAX_LIST_BYTES {
            return Err(PublicSuffixError::TooLarge);
        }
        let mut list = Rules::default();
        for (index, line) in text.lines().enumerate() {
            let rule = line.split_whitespace().next().unwrap_or("");
            if rule.is_empty() || rule.starts_with("//") {
                continue;
            }
            let rule = if rule.is_ascii() {
                rule.to_ascii_lowercase()
            } else {
                to_ascii_rule(rule).ok_or(PublicSuffixError::InvalidRule(index + 1))?
            };
            let (set, body) = if let Some(body) = rule.strip_prefix('!') {
                (&mut list.exceptions, body)
            } else if let Some(body) = rule.strip_prefix("*.") {
                (&mut list.wildcards, body)
            } else {
                (&mut list.rules, rule.as_str())
            };
            let labels = body.split('.').count();
            if body
                .split('.')
                .any(|label| label.is_empty() || label.contains('*'))
                || labels > MAX_RULE_LABELS
            {
                return Err(PublicSuffixError::InvalidRule(index + 1));
            }
            set.insert(body.to_owned());
        }
        Ok(Self(Arc::new(list)))
    }

    /// Number of rules of every kind.
    pub fn len(&self) -> usize {
        self.0.rules.len() + self.0.wildcards.len() + self.0.exceptions.len()
    }

    /// Whether only the implicit rule applies.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The public suffix of an ASCII, lowercase domain (no trailing dot
    /// handling beyond the URL Standard's: a trailing dot is kept on the
    /// result). `None` for an empty domain.
    pub fn public_suffix<'d>(&self, domain: &'d str) -> Option<&'d str> {
        let (trimmed, dot) = match domain.strip_suffix('.') {
            Some(trimmed) => (trimmed, 1),
            None => (domain, 0),
        };
        if trimmed.is_empty() || trimmed.split('.').any(str::is_empty) {
            return None;
        }
        // Byte offsets where each suffix starts, longest first.
        let mut starts = vec![0];
        starts.extend(trimmed.match_indices('.').map(|(index, _)| index + 1));
        let rules = &*self.0;
        let mut best = None::<usize>;
        for (position, &start) in starts.iter().enumerate() {
            let suffix = &trimmed[start..];
            if rules.exceptions.contains(suffix) {
                // The suffix is the exception rule minus its first label.
                let next = starts.get(position + 1).copied();
                return next.map(|next| &domain[next..trimmed.len() + dot]);
            }
            if best.is_none() {
                if rules.rules.contains(suffix) {
                    best = Some(start);
                } else if position > 0 && rules.wildcards.contains(suffix) {
                    // `*.suffix` covers one more label than `suffix`.
                    best = Some(starts[position - 1]);
                }
            }
        }
        let start = best.unwrap_or(*starts.last().expect("at least one label"));
        Some(&domain[start..trimmed.len() + dot])
    }

    /// URL Standard "host registrable domain": the public suffix plus one
    /// more label, or `None` when the domain is itself a public suffix.
    pub fn registrable_domain<'d>(&self, domain: &'d str) -> Option<&'d str> {
        let suffix = self.public_suffix(domain)?;
        let head = domain[..domain.len() - suffix.len()].strip_suffix('.')?;
        let start = head.rfind('.').map_or(0, |index| index + 1);
        Some(&domain[start..])
    }

    /// HTML "obtain a site" for an origin.
    pub fn site(&self, origin: &Origin) -> Site {
        let serialized = origin.serialize();
        let Some(url) = (!origin.is_opaque())
            .then(|| Url::parse(&serialized, None).ok())
            .flatten()
        else {
            return Site(SiteKind::Opaque(origin.clone()));
        };
        let host = url.host().expect("tuple origins have a host");
        Site(SiteKind::Scheme {
            scheme: url.scheme().to_owned(),
            host: self.site_host(host),
        })
    }

    /// The host part of a site: the registrable domain when there is one,
    /// otherwise the host itself.
    pub fn site_host(&self, host: &Host) -> String {
        match host.domain() {
            Some(domain) => self.registrable_domain(domain).unwrap_or(domain).to_owned(),
            None => host.serialize(),
        }
    }

    /// HTML "same site": equal sites.
    pub fn is_same_site(&self, a: &Origin, b: &Origin) -> bool {
        self.site(a) == self.site(b)
    }

    /// HTML "schemelessly same site".
    pub fn is_schemelessly_same_site(&self, a: &Origin, b: &Origin) -> bool {
        match (self.site(a).0, self.site(b).0) {
            (SiteKind::Opaque(a), SiteKind::Opaque(b)) => a == b,
            (SiteKind::Scheme { host: a, .. }, SiteKind::Scheme { host: b, .. }) => a == b,
            _ => false,
        }
    }
}

/// Domain-to-ASCII for a Unicode rule, keeping a `!` or `*.` marker.
fn to_ascii_rule(rule: &str) -> Option<String> {
    let (marker, body) = if let Some(body) = rule.strip_prefix('!') {
        ("!", body)
    } else if let Some(body) = rule.strip_prefix("*.") {
        ("*.", body)
    } else {
        ("", rule)
    };
    let url = Url::parse(&format!("http://{body}/"), None).ok()?;
    let domain = url.host()?.domain()?.to_owned();
    Some(format!("{marker}{domain}"))
}

impl fmt::Debug for PublicSuffixList {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublicSuffixList")
            .field("rules", &self.len())
            .finish()
    }
}

/// HTML "site": an opaque origin, or a scheme and a registrable domain
/// (or host).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Site(SiteKind);

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
enum SiteKind {
    Opaque(Origin),
    Scheme { scheme: String, host: String },
}

impl Site {
    /// Parse a non-opaque site's serialization (`scheme://host`), as
    /// saved in a profile.
    pub fn parse_serialized(text: &str) -> Option<Self> {
        let (scheme, host) = text.split_once("://")?;
        let valid = |part: &str| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_graphic() && byte != b'/' && byte != b'\\')
        };
        (valid(scheme) && valid(host) && scheme.bytes().all(|byte| byte.is_ascii_lowercase())).then(
            || {
                Self(SiteKind::Scheme {
                    scheme: scheme.to_owned(),
                    host: host.to_owned(),
                })
            },
        )
    }

    /// HTML "serialize a site": `null` or `scheme://host`.
    pub fn serialize(&self) -> String {
        match &self.0 {
            SiteKind::Opaque(_) => "null".to_owned(),
            SiteKind::Scheme { scheme, host } => format!("{scheme}://{host}"),
        }
    }
}

/// Secure Contexts §3.1 "potentially trustworthy origin", for the schemes
/// the network layer sees.
pub fn is_potentially_trustworthy(origin: &Origin) -> bool {
    if origin.is_opaque() {
        return false;
    }
    let Ok(url) = Url::parse(&origin.serialize(), None) else {
        return false;
    };
    if matches!(url.scheme(), "https" | "wss") {
        return true;
    }
    let host = url.host().expect("tuple origins have a host");
    if let Some([127, ..]) = host.ipv4() {
        return true;
    }
    if host.ipv6() == Some([0, 0, 0, 0, 0, 0, 0, 1]) {
        return true;
    }
    host.domain().is_some_and(|domain| {
        let domain = domain.strip_suffix('.').unwrap_or(domain);
        domain == "localhost" || domain.ends_with(".localhost")
    })
}

/// Secure Contexts "potentially trustworthy URL".
pub fn is_potentially_trustworthy_url(url: &Url) -> bool {
    matches!(url.serialize().as_str(), "about:blank" | "about:srcdoc")
        || url.scheme() == "data"
        || is_potentially_trustworthy(&url.origin())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "// comment\n\
        com\n\
        uk\n\
        co.uk\n\
        *.ck\n\
        !www.ck\n\
        jp\n\
        *.kawasaki.jp\n\
        !city.kawasaki.jp\n\
        github.io   trailing text ignored\n";

    fn list() -> PublicSuffixList {
        PublicSuffixList::parse(LIST).unwrap()
    }

    #[test]
    fn suffixes_follow_rules_wildcards_and_exceptions() {
        let list = list();
        assert_eq!(list.len(), 9);
        for (domain, suffix, registrable) in [
            ("example.com", "com", Some("example.com")),
            ("a.b.example.com", "com", Some("example.com")),
            ("com", "com", None),
            ("example.co.uk", "co.uk", Some("example.co.uk")),
            ("co.uk", "co.uk", None),
            ("www.example.ck", "example.ck", Some("www.example.ck")),
            ("example.ck", "example.ck", None),
            ("www.ck", "ck", Some("www.ck")),
            ("a.www.ck", "ck", Some("www.ck")),
            ("city.kawasaki.jp", "kawasaki.jp", Some("city.kawasaki.jp")),
            ("a.b.kawasaki.jp", "b.kawasaki.jp", Some("a.b.kawasaki.jp")),
            ("user.github.io", "github.io", Some("user.github.io")),
            ("unlisted.zz", "zz", Some("unlisted.zz")),
            ("example.com.", "com.", Some("example.com.")),
            ("zz", "zz", None),
        ] {
            assert_eq!(list.public_suffix(domain), Some(suffix), "{domain}");
            assert_eq!(list.registrable_domain(domain), registrable, "{domain}");
        }
        assert_eq!(list.public_suffix(""), None);
        assert_eq!(list.public_suffix("a..b"), None);
    }

    #[test]
    fn the_fallback_uses_the_implicit_rule() {
        let list = PublicSuffixList::fallback();
        assert!(list.is_empty());
        assert_eq!(
            list.registrable_domain("a.b.example.test"),
            Some("example.test")
        );
        assert_eq!(list.registrable_domain("localhost"), None);
    }

    #[test]
    fn invalid_rules_and_oversized_lists_are_rejected() {
        assert_eq!(
            PublicSuffixList::parse("com\na..b\n").unwrap_err(),
            PublicSuffixError::InvalidRule(2)
        );
        assert_eq!(
            PublicSuffixList::parse("a.*.b").unwrap_err(),
            PublicSuffixError::InvalidRule(1)
        );
        let big = "a\n".repeat(MAX_LIST_BYTES / 2 + 1);
        assert_eq!(
            PublicSuffixList::parse(&big).unwrap_err(),
            PublicSuffixError::TooLarge
        );
        // IDN rules are converted to their ASCII form.
        let idn = PublicSuffixList::parse("公司.cn\ncn").unwrap();
        assert_eq!(idn.public_suffix("a.xn--55qx5d.cn"), Some("xn--55qx5d.cn"));
    }

    fn origin(input: &str) -> Origin {
        Url::parse(input, None).unwrap().origin()
    }

    #[test]
    fn sites_and_same_site() {
        let list = list();
        let a = origin("https://a.example.co.uk:8443");
        let b = origin("https://b.example.co.uk");
        let c = origin("http://b.example.co.uk");
        let d = origin("https://other.co.uk");
        assert_eq!(list.site(&a).serialize(), "https://example.co.uk");
        assert!(list.is_same_site(&a, &b));
        assert!(!list.is_same_site(&a, &c));
        assert!(list.is_schemelessly_same_site(&a, &c));
        assert!(!list.is_same_site(&a, &d));
        let ip = origin("http://127.0.0.1:8080");
        assert_eq!(list.site(&ip).serialize(), "http://127.0.0.1");
        let opaque = Origin::opaque();
        assert_eq!(list.site(&opaque).serialize(), "null");
        assert!(list.is_same_site(&opaque, &opaque.clone()));
        assert!(!list.is_same_site(&opaque, &Origin::opaque()));
        assert!(!list.is_schemelessly_same_site(&opaque, &ip));
    }

    #[test]
    fn potentially_trustworthy_origins() {
        for (input, expected) in [
            ("https://example.com", true),
            ("http://example.com", false),
            ("http://127.0.0.1:81", true),
            ("http://127.9.9.9", true),
            ("http://[::1]", true),
            ("http://localhost:8000", true),
            ("http://a.localhost", true),
            ("http://localhost.example", false),
            ("ws://example.com", false),
            ("wss://example.com", true),
        ] {
            assert_eq!(
                is_potentially_trustworthy(&origin(input)),
                expected,
                "{input}"
            );
        }
        assert!(!is_potentially_trustworthy(&Origin::opaque()));
        let url = |input| Url::parse(input, None).unwrap();
        assert!(is_potentially_trustworthy_url(&url("about:blank")));
        assert!(is_potentially_trustworthy_url(&url("data:text/plain,a")));
        assert!(!is_potentially_trustworthy_url(&url("about:other")));
    }
}
