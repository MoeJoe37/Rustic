//! Mixed Content (W3C Mixed Content Level 2) and secure contexts (W3C
//! Secure Contexts §3) for requests from a document.
//!
//! A secure document's `http:` images, audio and video are upgraded to
//! `https:` (they never load over plain HTTP); every other insecure
//! subresource and `fetch()` is blocked. Top-level navigations are not
//! mixed content.

use rustic_url::{Origin, Url};

use crate::fetch::Destination;
use crate::site::{is_potentially_trustworthy, is_potentially_trustworthy_url};

/// What to do with a request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MixedContent {
    /// Not mixed content.
    Allowed,
    /// Upgradeable mixed content: fetch this `https:` URL instead.
    Upgrade(Url),
    /// Blockable mixed content.
    Blocked,
}

/// HTML "secure context" for a top-level document of `origin`, or a nested
/// one whose parent is (or is not) a secure context.
pub fn is_secure_context(origin: &Origin, parent_is_secure: Option<bool>) -> bool {
    parent_is_secure.unwrap_or(true) && is_potentially_trustworthy(origin)
}

/// Mixed Content §4.1 "should fetching request be blocked as mixed
/// content" with §4.3 upgrades, for a request to `url` from a document that
/// is (`client_secure`) or is not a secure context.
pub fn check(url: &Url, destination: Destination, client_secure: bool) -> MixedContent {
    if !client_secure || destination == Destination::Document || is_potentially_trustworthy_url(url)
    {
        return MixedContent::Allowed;
    }
    if destination == Destination::Image && url.scheme() == "http" {
        return MixedContent::Upgrade(crate::csp::upgrade_url(url));
    }
    MixedContent::Blocked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(input: &str) -> Url {
        Url::parse(input, None).unwrap()
    }

    #[test]
    fn secure_documents_upgrade_images_and_block_the_rest() {
        let image = url("http://cdn.test/a.png");
        assert_eq!(
            check(&image, Destination::Image, true),
            MixedContent::Upgrade(url("https://cdn.test/a.png"))
        );
        for destination in [
            Destination::Script,
            Destination::Style,
            Destination::Empty,
            Destination::Font,
            Destination::Iframe,
        ] {
            assert_eq!(
                check(&url("http://cdn.test/x"), destination, true),
                MixedContent::Blocked
            );
        }
        assert_eq!(
            check(&url("http://cdn.test/x"), Destination::Document, true),
            MixedContent::Allowed
        );
        assert_eq!(
            check(&url("http://cdn.test/x"), Destination::Script, false),
            MixedContent::Allowed
        );
        assert_eq!(
            check(&url("https://cdn.test/x"), Destination::Script, true),
            MixedContent::Allowed
        );
        assert_eq!(
            check(&url("http://127.0.0.1/x"), Destination::Script, true),
            MixedContent::Allowed
        );
        assert_eq!(
            check(&url("http://localhost/x"), Destination::Script, true),
            MixedContent::Allowed
        );
    }

    #[test]
    fn secure_contexts() {
        assert!(is_secure_context(&url("https://a.test").origin(), None));
        assert!(!is_secure_context(&url("http://a.test").origin(), None));
        assert!(is_secure_context(&url("http://localhost").origin(), None));
        assert!(!is_secure_context(
            &url("https://a.test").origin(),
            Some(false)
        ));
        assert!(!is_secure_context(&Origin::opaque(), None));
    }
}
