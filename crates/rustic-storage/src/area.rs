//! One Web Storage area (HTML §12.2 "the Storage interface").

use std::collections::BTreeMap;
use std::fmt;

/// Why a storage write failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageError {
    /// The write would pass the key's or the profile's quota
    /// (`QuotaExceededError` in script).
    QuotaExceeded,
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the storage quota has been exceeded")
    }
}

impl std::error::Error for StorageError {}

/// What a successful mutation changed, for the `storage` event (HTML
/// §12.2.4). A `key` of `None` means the area was cleared.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageChange {
    /// The key changed, or `None` for `clear()`.
    pub key: Option<Vec<u16>>,
    /// The previous value.
    pub old_value: Option<Vec<u16>>,
    /// The new value, or `None` for a removal.
    pub new_value: Option<Vec<u16>>,
}

/// A storage area: a map from UTF-16 keys to UTF-16 values with a byte
/// quota. Usage is two bytes per code unit of every key and value, so a
/// 5 MiB quota holds about 2.6 million code units.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StorageArea {
    entries: BTreeMap<Vec<u16>, Vec<u16>>,
    bytes: usize,
}

pub(crate) fn cost(key: &[u16], value: &[u16]) -> usize {
    key.len().saturating_add(value.len()).saturating_mul(2)
}

impl StorageArea {
    /// An empty area.
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of key/value pairs (`length`).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the area holds nothing.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bytes charged against the quota.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// The `index`th key (`key(n)`). The order is stable while the set of
    /// keys does not change, as HTML requires.
    pub fn key(&self, index: usize) -> Option<&[u16]> {
        self.entries.keys().nth(index).map(Vec::as_slice)
    }

    /// The keys in order.
    pub fn keys(&self) -> impl Iterator<Item = &[u16]> {
        self.entries.keys().map(Vec::as_slice)
    }

    /// The value of `key` (`getItem`).
    pub fn get(&self, key: &[u16]) -> Option<&[u16]> {
        self.entries.get(key).map(Vec::as_slice)
    }

    /// Whether `key` is present.
    pub fn contains(&self, key: &[u16]) -> bool {
        self.entries.contains_key(key)
    }

    /// Store `value` under `key` (`setItem`) if the area stays within
    /// `quota` bytes and `room` more bytes are available to the profile.
    /// Setting a key to its current value changes nothing and reports no
    /// change. On failure the area is unchanged.
    pub fn set(
        &mut self,
        key: &[u16],
        value: &[u16],
        quota: usize,
        room: usize,
    ) -> Result<Option<StorageChange>, StorageError> {
        let old = self.entries.get(key);
        if old.is_some_and(|old| old.as_slice() == value) {
            return Ok(None);
        }
        let freed = old.map_or(0, |old| cost(key, old));
        let added = cost(key, value);
        let after = self
            .bytes
            .checked_sub(freed)
            .and_then(|bytes| bytes.checked_add(added))
            .ok_or(StorageError::QuotaExceeded)?;
        if after > quota || added.saturating_sub(freed) > room {
            return Err(StorageError::QuotaExceeded);
        }
        let old_value = self.entries.insert(key.to_vec(), value.to_vec());
        self.bytes = after;
        Ok(Some(StorageChange {
            key: Some(key.to_vec()),
            old_value,
            new_value: Some(value.to_vec()),
        }))
    }

    /// Remove `key` (`removeItem`), reporting a change only if it existed.
    pub fn remove(&mut self, key: &[u16]) -> Option<StorageChange> {
        let old = self.entries.remove(key)?;
        self.bytes -= cost(key, &old);
        Some(StorageChange {
            key: Some(key.to_vec()),
            old_value: Some(old),
            new_value: None,
        })
    }

    /// Remove everything (`clear()`), reporting a change only if the area
    /// was not already empty.
    pub fn clear(&mut self) -> Option<StorageChange> {
        if self.entries.is_empty() {
            return None;
        }
        self.entries.clear();
        self.bytes = 0;
        Some(StorageChange {
            key: None,
            old_value: None,
            new_value: None,
        })
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = (&[u16], &[u16])> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_slice(), value.as_slice()))
    }

    /// Insert while loading a file; the caller checks the quota.
    pub(crate) fn insert_loaded(&mut self, key: Vec<u16>, value: Vec<u16>) {
        let added = cost(&key, &value);
        if let Some(old) = self.entries.insert(key.clone(), value) {
            self.bytes -= cost(&key, &old);
        }
        self.bytes += added;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn set_get_remove_clear_and_changes() {
        let mut area = StorageArea::new();
        let change = area
            .set(&units("a"), &units("1"), 100, 100)
            .unwrap()
            .unwrap();
        assert_eq!(change.old_value, None);
        assert_eq!(area.bytes(), 4);
        assert_eq!(area.set(&units("a"), &units("1"), 100, 100), Ok(None));
        let change = area
            .set(&units("a"), &units("22"), 100, 100)
            .unwrap()
            .unwrap();
        assert_eq!(change.old_value, Some(units("1")));
        assert_eq!(area.bytes(), 6);
        area.set(&units("b"), &[], 100, 100).unwrap();
        assert_eq!(area.key(0), Some(units("a").as_slice()));
        assert_eq!(area.key(1), Some(units("b").as_slice()));
        assert_eq!(area.key(2), None);
        assert_eq!(area.remove(&units("zz")), None);
        assert_eq!(area.remove(&units("a")).unwrap().new_value, None);
        assert_eq!(area.bytes(), 2);
        assert!(area.clear().is_some());
        assert!(area.clear().is_none());
        assert_eq!(area.bytes(), 0);
    }

    #[test]
    fn quota_boundaries_leave_the_area_unchanged() {
        let mut area = StorageArea::new();
        // Exactly at the quota: key 2 units + value 3 units = 10 bytes.
        area.set(&units("kk"), &units("vvv"), 10, 10).unwrap();
        assert_eq!(
            area.set(&units("x"), &[], 10, 10),
            Err(StorageError::QuotaExceeded)
        );
        // Replacing with a shorter value frees room.
        area.set(&units("kk"), &units("v"), 10, 0).unwrap();
        assert_eq!(area.bytes(), 6);
        // The profile's remaining room limits growth even under the quota.
        assert_eq!(
            area.set(&units("kk"), &units("vvv"), 10, 3),
            Err(StorageError::QuotaExceeded)
        );
        assert_eq!(area.get(&units("kk")), Some(units("v").as_slice()));
        assert_eq!(area.bytes(), 6);
    }
}
