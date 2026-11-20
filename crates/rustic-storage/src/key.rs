//! Storage keys (HTML §"storage key", Storage Standard §2.1, with
//! Chromium-style partitioning by top-level site).

use std::fmt;

/// Longest serialized origin or site accepted in a key.
const MAX_PART_BYTES: usize = 2048;

/// Which bucket of storage a document uses: its origin, partitioned under
/// the site of its top-level document. First-party storage has the same
/// site as its origin; a cross-site frame's storage is separate.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StorageKey {
    origin: String,
    top_level_site: String,
}

/// Why a storage key could not be made.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageKeyError {
    /// Opaque origins (`null`) have no storage.
    OpaqueOrigin,
    /// A part is empty, too long or has control characters.
    Invalid,
}

impl fmt::Display for StorageKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::OpaqueOrigin => "opaque origins have no storage",
            Self::Invalid => "invalid storage key",
        })
    }
}

impl std::error::Error for StorageKeyError {}

impl StorageKey {
    /// A key for the serialized `origin` (`https://a.example:8443`) under
    /// the serialized top-level site (`https://example`). Opaque origins
    /// (`null`) are refused: HTML gives them no storage that outlives the
    /// document.
    pub fn new(origin: &str, top_level_site: &str) -> Result<Self, StorageKeyError> {
        if origin == "null" || top_level_site == "null" {
            return Err(StorageKeyError::OpaqueOrigin);
        }
        for part in [origin, top_level_site] {
            if part.is_empty() || part.len() > MAX_PART_BYTES || part.chars().any(char::is_control)
            {
                return Err(StorageKeyError::Invalid);
            }
        }
        Ok(Self {
            origin: origin.to_owned(),
            top_level_site: top_level_site.to_owned(),
        })
    }

    /// The serialized origin.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// The serialized top-level site the storage is partitioned under.
    pub fn top_level_site(&self) -> &str {
        &self.top_level_site
    }

    pub(crate) fn encode(&self) -> String {
        format!("{} {}", self.origin, self.top_level_site)
    }

    pub(crate) fn decode(text: &str) -> Option<Self> {
        let (origin, site) = text.split_once(' ')?;
        Self::new(origin, site).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_refuse_opaque_and_malformed_parts_and_round_trip() {
        let key = StorageKey::new("https://a.example", "https://example").unwrap();
        assert_eq!(StorageKey::decode(&key.encode()), Some(key.clone()));
        assert_eq!(
            StorageKey::new("null", "https://example"),
            Err(StorageKeyError::OpaqueOrigin)
        );
        assert_eq!(
            StorageKey::new("", "https://example"),
            Err(StorageKeyError::Invalid)
        );
        assert_eq!(
            StorageKey::new("https://a\n", "https://example"),
            Err(StorageKeyError::Invalid)
        );
        let third_party = StorageKey::new("https://a.example", "https://other").unwrap();
        assert_ne!(key, third_party);
    }
}
