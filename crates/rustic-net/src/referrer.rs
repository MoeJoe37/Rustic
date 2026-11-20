//! Referrer Policy (W3C Referrer Policy, §3 policies, §8.1 "parse a
//! referrer policy from a Referrer-Policy header", §8.3 "determine
//! request's referrer", §8.4 "strip url for use as a referrer").

use rustic_url::Url;

use crate::Headers;
use crate::mime::split_header_value;
use crate::site::is_potentially_trustworthy_url;

/// A referrer policy. The empty policy is represented by `None` where a
/// header or attribute carried no valid token.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum ReferrerPolicy {
    /// `no-referrer`.
    NoReferrer,
    /// `no-referrer-when-downgrade`.
    NoReferrerWhenDowngrade,
    /// `same-origin`.
    SameOrigin,
    /// `origin`.
    Origin,
    /// `strict-origin`.
    StrictOrigin,
    /// `origin-when-cross-origin`.
    OriginWhenCrossOrigin,
    /// `strict-origin-when-cross-origin`, the default.
    #[default]
    StrictOriginWhenCrossOrigin,
    /// `unsafe-url`.
    UnsafeUrl,
}

impl ReferrerPolicy {
    /// One policy token (ASCII case-insensitive), as in a `referrerpolicy`
    /// attribute.
    pub fn from_token(token: &str) -> Option<Self> {
        Some(match token.to_ascii_lowercase().as_str() {
            "no-referrer" => Self::NoReferrer,
            "no-referrer-when-downgrade" => Self::NoReferrerWhenDowngrade,
            "same-origin" => Self::SameOrigin,
            "origin" => Self::Origin,
            "strict-origin" => Self::StrictOrigin,
            "origin-when-cross-origin" => Self::OriginWhenCrossOrigin,
            "strict-origin-when-cross-origin" => Self::StrictOriginWhenCrossOrigin,
            "unsafe-url" => Self::UnsafeUrl,
            _ => return None,
        })
    }

    /// §8.1: the last recognized token of every `Referrer-Policy` value.
    pub fn from_headers(headers: &Headers) -> Option<Self> {
        let combined = headers.combined("referrer-policy")?;
        split_header_value(&combined)
            .iter()
            .rev()
            .find_map(|token| Self::from_token(token))
    }

    /// The token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoReferrer => "no-referrer",
            Self::NoReferrerWhenDowngrade => "no-referrer-when-downgrade",
            Self::SameOrigin => "same-origin",
            Self::Origin => "origin",
            Self::StrictOrigin => "strict-origin",
            Self::OriginWhenCrossOrigin => "origin-when-cross-origin",
            Self::StrictOriginWhenCrossOrigin => "strict-origin-when-cross-origin",
            Self::UnsafeUrl => "unsafe-url",
        }
    }
}

/// The longest `Referer` sent in full; longer ones fall back to the
/// origin (Chromium's and Firefox's limit, Referrer Policy §8.3 step 6).
pub const MAX_REFERRER_LENGTH: usize = 4096;

/// §8.4 "strip url for use as a referrer": `None` for non-HTTP(S) URLs;
/// credentials and the fragment are removed, and with `origin_only` the
/// path and query too.
pub fn strip_for_referrer(url: &Url, origin_only: bool) -> Option<String> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if origin_only {
        return Some(format!("{}/", url.origin().serialize()));
    }
    let mut text = url.serialize_without_credentials();
    if let Some(index) = text.find('#') {
        text.truncate(index);
    }
    Some(text)
}

/// §8.3 "determine request's referrer": the `Referer` value for a request
/// to `target` from `referrer` under `policy`, or `None`.
pub fn determine_referrer(referrer: &Url, target: &Url, policy: ReferrerPolicy) -> Option<String> {
    let full = strip_for_referrer(referrer, false)?;
    let origin = strip_for_referrer(referrer, true)?;
    let full = if full.len() > MAX_REFERRER_LENGTH {
        origin.clone()
    } else {
        full
    };
    let downgrade =
        is_potentially_trustworthy_url(referrer) && !is_potentially_trustworthy_url(target);
    let same_origin = referrer.origin().is_same_origin(&target.origin());
    match policy {
        ReferrerPolicy::NoReferrer => None,
        ReferrerPolicy::Origin => Some(origin),
        ReferrerPolicy::UnsafeUrl => Some(full),
        ReferrerPolicy::StrictOrigin => (!downgrade).then_some(origin),
        ReferrerPolicy::StrictOriginWhenCrossOrigin => {
            if same_origin {
                Some(full)
            } else {
                (!downgrade).then_some(origin)
            }
        }
        ReferrerPolicy::SameOrigin => same_origin.then_some(full),
        ReferrerPolicy::OriginWhenCrossOrigin => Some(if same_origin { full } else { origin }),
        ReferrerPolicy::NoReferrerWhenDowngrade => (!downgrade).then_some(full),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(input: &str) -> Url {
        Url::parse(input, None).unwrap()
    }

    #[test]
    fn header_parsing_takes_the_last_valid_token() {
        let mut headers = Headers::new();
        assert_eq!(ReferrerPolicy::from_headers(&headers), None);
        headers
            .append("Referrer-Policy", b"no-referrer, bogus")
            .unwrap();
        assert_eq!(
            ReferrerPolicy::from_headers(&headers),
            Some(ReferrerPolicy::NoReferrer)
        );
        headers.append("Referrer-Policy", b"Unsafe-URL").unwrap();
        assert_eq!(
            ReferrerPolicy::from_headers(&headers),
            Some(ReferrerPolicy::UnsafeUrl)
        );
        let mut empty = Headers::new();
        empty.append("Referrer-Policy", b"").unwrap();
        assert_eq!(ReferrerPolicy::from_headers(&empty), None);
        for policy in [
            ReferrerPolicy::NoReferrer,
            ReferrerPolicy::NoReferrerWhenDowngrade,
            ReferrerPolicy::SameOrigin,
            ReferrerPolicy::Origin,
            ReferrerPolicy::StrictOrigin,
            ReferrerPolicy::OriginWhenCrossOrigin,
            ReferrerPolicy::StrictOriginWhenCrossOrigin,
            ReferrerPolicy::UnsafeUrl,
        ] {
            assert_eq!(ReferrerPolicy::from_token(policy.as_str()), Some(policy));
        }
    }

    #[test]
    fn stripping_removes_credentials_and_fragments() {
        assert_eq!(
            strip_for_referrer(&url("https://u:p@a.test:8443/p/q?x=1#frag"), false).as_deref(),
            Some("https://a.test:8443/p/q?x=1")
        );
        assert_eq!(
            strip_for_referrer(&url("https://u:p@a.test:8443/p/q?x=1#frag"), true).as_deref(),
            Some("https://a.test:8443/")
        );
        assert_eq!(strip_for_referrer(&url("data:text/html,x"), false), None);
        assert_eq!(strip_for_referrer(&url("file:///tmp/a"), false), None);
    }

    #[test]
    fn every_policy_against_same_cross_and_downgrade_targets() {
        let from = url("https://a.test/page?q=1#f");
        let same = url("https://a.test/other");
        let cross = url("https://b.test/");
        let down = url("http://b.test/");
        let full = Some("https://a.test/page?q=1".to_owned());
        let origin = Some("https://a.test/".to_owned());
        use ReferrerPolicy::*;
        for (policy, expected) in [
            (NoReferrer, [None, None, None]),
            (NoReferrerWhenDowngrade, [full.clone(), full.clone(), None]),
            (SameOrigin, [full.clone(), None, None]),
            (Origin, [origin.clone(), origin.clone(), origin.clone()]),
            (StrictOrigin, [origin.clone(), origin.clone(), None]),
            (
                OriginWhenCrossOrigin,
                [full.clone(), origin.clone(), origin.clone()],
            ),
            (
                StrictOriginWhenCrossOrigin,
                [full.clone(), origin.clone(), None],
            ),
            (UnsafeUrl, [full.clone(), full.clone(), full.clone()]),
        ] {
            let actual =
                [&same, &cross, &down].map(|target| determine_referrer(&from, target, policy));
            assert_eq!(actual, expected, "{policy:?}");
        }
        let long = url(&format!(
            "https://a.test/{}",
            "x".repeat(MAX_REFERRER_LENGTH)
        ));
        assert_eq!(
            determine_referrer(&long, &same, UnsafeUrl).as_deref(),
            Some("https://a.test/")
        );
        // Loopback http is potentially trustworthy: no downgrade to it.
        assert!(determine_referrer(&from, &url("http://127.0.0.1/"), StrictOrigin).is_some());
    }
}
