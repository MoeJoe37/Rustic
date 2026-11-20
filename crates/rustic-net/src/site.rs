//! Public suffixes, registrable domains and sites (URL Standard §3.2
//! "host public suffix" and "host registrable domain", HTML "site" and
//! "same site").
//!
//! Rustic ships no Public Suffix List. A host supplies the list's text
//! through [`PublicSuffixList::parse`] (ADR 0054); without one,
//! [`PublicSuffixList::fallback`] applies the list's implicit `*` rule
//! only, so every domain's last label is its public suffix. That is
//! correct for the loopback test origins the transport can reach and is
//! recorded as TD-159 until a reviewed list is bundled.

use std::collections::HashSet;
use std::fmt;

use rustic_url::{Host, Origin, Url};

/// A parsed Public Suffix List (publicsuffix.org format).
///
/// Rules are stored lowercase with their wildcard and exception markers.
/// Matching follows the list's published algorithm: an exception rule wins,
/// otherwise the rule with the most labels; with no match the implicit
/// rule `*` applies.
#[derive(Clone, Default)]
pub struct PublicSuffixList {
    rules: HashSet<String>,
    wildcards: HashSet<String>,
    exceptions: HashSet<String>,
}

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
    /// Only the implicit `*` rule (TD-159).
    pub fn fallback() -> Self {
        Self::default()
    }

    /// Parse the list's text format: one rule per line, `//` comments,
    /// rules end at the first whitespace, `*.` wildcards and `!`
    /// exceptions. Rules must already be in ASCII (punycode) form; the
    /// published list is, apart from IDN rules, which are skipped when
    /// they contain non-ASCII characters because hosts reach this module
    /// already in their ASCII form.
    pub fn parse(text: &str) -> Result<Self, PublicSuffixError> {
        if text.len() > MAX_LIST_BYTES {
            return Err(PublicSuffixError::TooLarge);
        }
        let mut list = Self::default();
        for (index, line) in text.lines().enumerate() {
            let rule = line.split_whitespace().next().unwrap_or("");
            if rule.is_empty() || rule.starts_with("//") {
                continue;
            }
            if !rule.is_ascii() {
                continue;
            }
            let rule = rule.to_ascii_lowercase();
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
        Ok(list)
    }

    /// Number of rules of every kind.
    pub fn len(&self) -> usize {
        self.rules.len() + self.wildcards.len() + self.exceptions.len()
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
        let mut best = None::<usize>;
        for (position, &start) in starts.iter().enumerate() {
            let suffix = &trimmed[start..];
            if self.exceptions.contains(suffix) {
                // The suffix is the exception rule minus its first label.
                let next = starts.get(position + 1).copied();
                return next.map(|next| &domain[next..trimmed.len() + dot]);
            }
            if best.is_none() {
                if self.rules.contains(suffix) {
                    best = Some(start);
                } else if position > 0 && self.wildcards.contains(suffix) {
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
        // IDN rules in Unicode form are skipped, not errors.
        assert_eq!(PublicSuffixList::parse("公司.cn\ncn").unwrap().len(), 1);
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
