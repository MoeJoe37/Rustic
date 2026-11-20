//! Profiles: ephemeral, private and persistent browsing state.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::area::{StorageArea, StorageChange, StorageError, cost};
use crate::file::{
    FileError, Reader, put_bytes, put_u32, put_utf8, put_utf16, read_versioned, write_atomic,
};
use crate::key::StorageKey;

const LOCK_FILE: &str = "LOCK";
const LOCAL_STORAGE_FILE: &str = "local-storage";
const RECORDS_FILE: &str = "records";
/// Longest name of a service record.
const MAX_RECORD_NAME_BYTES: usize = 64;

/// Byte limits for one profile. Usage counts two bytes per UTF-16 code
/// unit of Web Storage keys and values, plus the bytes of service records.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Quotas {
    /// Most bytes one storage key's local or session storage may use (HTML
    /// suggests 5 MiB per origin; Firefox and Safari use 5 MiB).
    pub per_key_bytes: usize,
    /// Most bytes all of the profile's storage may use together.
    pub total_bytes: usize,
    /// Most storage keys with local storage.
    pub max_keys: usize,
}

impl Default for Quotas {
    fn default() -> Self {
        Self {
            per_key_bytes: 5 * 1024 * 1024,
            total_bytes: 256 * 1024 * 1024,
            max_keys: 4096,
        }
    }
}

/// What kind of profile it is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileKind {
    /// In memory only; dropped with the process.
    Ephemeral,
    /// In memory only, never written, and cleared when dropped (private
    /// browsing). Behaves like [`Self::Ephemeral`] to pages.
    Private,
    /// Stored in a directory and written by [`Profile::flush`].
    Persistent,
}

/// Why a profile could not be opened or written.
#[derive(Debug)]
pub enum ProfileError {
    /// Creating, locking, reading or writing the directory failed.
    Io(io::Error),
    /// Another process holds the profile's lock file.
    Locked,
    /// A newer build wrote a file; it is left untouched and the profile is
    /// not opened, so nothing overwrites it.
    NewerFormat(u32),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "profile I/O failed: {error}"),
            Self::Locked => formatter.write_str(
                "the profile is in use by another process (remove its LOCK file if that \
                 process has exited)",
            ),
            Self::NewerFormat(version) => write!(
                formatter,
                "the profile was written by a newer Rustic (format {version})"
            ),
        }
    }
}

impl std::error::Error for ProfileError {}

/// A file that could not be trusted when the profile opened.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recovery {
    /// The file's name in the profile.
    pub file: String,
    /// Why it was set aside.
    pub reason: String,
    /// Where it was moved; the profile starts without its contents.
    pub moved_to: PathBuf,
}

/// An exclusive claim on a profile directory, released when dropped.
///
/// The claim is a `LOCK` file created exclusively, holding the process
/// id. A process that crashes leaves it behind; the user removes it
/// (TD-166).
#[derive(Debug)]
pub struct ProfileLock {
    path: PathBuf,
}

impl ProfileLock {
    fn acquire(dir: &Path) -> Result<Self, ProfileError> {
        let path = dir.join(LOCK_FILE);
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(ProfileError::Locked);
            }
            Err(error) => return Err(ProfileError::Io(error)),
        };
        let lock = Self { path };
        writeln!(file, "{}", std::process::id()).map_err(ProfileError::Io)?;
        Ok(lock)
    }
}

impl Drop for ProfileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// One user's browsing state: local storage by storage key, and opaque
/// records other services keep (such as the cookie jar).
#[derive(Debug)]
pub struct Profile {
    kind: ProfileKind,
    dir: Option<PathBuf>,
    quotas: Quotas,
    local: BTreeMap<StorageKey, StorageArea>,
    records: BTreeMap<String, Vec<u8>>,
    used: usize,
    dirty: bool,
    // Dropped last, after any final state is gone.
    _lock: Option<ProfileLock>,
}

impl Profile {
    /// A profile kept in memory.
    pub fn ephemeral(quotas: Quotas) -> Self {
        Self::in_memory(ProfileKind::Ephemeral, quotas)
    }

    /// A private-browsing profile: in memory, never written.
    pub fn private(quotas: Quotas) -> Self {
        Self::in_memory(ProfileKind::Private, quotas)
    }

    fn in_memory(kind: ProfileKind, quotas: Quotas) -> Self {
        Self {
            kind,
            dir: None,
            quotas,
            local: BTreeMap::new(),
            records: BTreeMap::new(),
            used: 0,
            dirty: false,
            _lock: None,
        }
    }

    /// Open (creating if needed) and lock the persistent profile in `dir`.
    /// Files that fail their checksum, are malformed or break the quotas
    /// are moved aside and listed; the profile starts without them.
    pub fn open(dir: &Path, quotas: Quotas) -> Result<(Self, Vec<Recovery>), ProfileError> {
        fs::create_dir_all(dir).map_err(ProfileError::Io)?;
        let lock = ProfileLock::acquire(dir)?;
        let mut profile = Self {
            kind: ProfileKind::Persistent,
            dir: Some(dir.to_owned()),
            quotas,
            local: BTreeMap::new(),
            records: BTreeMap::new(),
            used: 0,
            dirty: false,
            _lock: Some(lock),
        };
        let mut recoveries = Vec::new();
        let max = quotas.total_bytes.saturating_mul(2);
        for name in [LOCAL_STORAGE_FILE, RECORDS_FILE] {
            let path = dir.join(name);
            let result = read_versioned(&path, max).and_then(|payload| match payload {
                None => Ok(()),
                Some(payload) if name == LOCAL_STORAGE_FILE => profile.load_local(&payload),
                Some(payload) => profile.load_records(&payload),
            });
            match result {
                Ok(()) => {}
                Err(FileError::NewerVersion(version)) => {
                    return Err(ProfileError::NewerFormat(version));
                }
                Err(error) => {
                    // Start this store empty, keeping the rest.
                    if name == LOCAL_STORAGE_FILE {
                        profile.local.clear();
                    } else {
                        profile.records.clear();
                    }
                    profile.used = profile.recount();
                    recoveries.push(quarantine(&path, name, &error)?);
                }
            }
        }
        Ok((profile, recoveries))
    }

    fn recount(&self) -> usize {
        let local: usize = self.local.values().map(StorageArea::bytes).sum();
        let records: usize = self.records.values().map(Vec::len).sum();
        local + records
    }

    fn load_local(&mut self, payload: &[u8]) -> Result<(), FileError> {
        let mut reader = Reader::new(payload);
        let areas = reader.count(8)?;
        if areas > self.quotas.max_keys {
            return Err(FileError::TooLarge);
        }
        for _ in 0..areas {
            let key = StorageKey::decode(reader.utf8()?).ok_or(FileError::Corrupt)?;
            let entries = reader.count(8)?;
            let mut area = StorageArea::new();
            for _ in 0..entries {
                let name = reader.utf16()?;
                let value = reader.utf16()?;
                area.insert_loaded(name, value);
            }
            if area.bytes() > self.quotas.per_key_bytes {
                return Err(FileError::TooLarge);
            }
            self.used += area.bytes();
            if self.local.insert(key, area).is_some() {
                return Err(FileError::Corrupt);
            }
        }
        if !reader.is_empty() || self.used > self.quotas.total_bytes {
            return Err(FileError::Corrupt);
        }
        Ok(())
    }

    fn load_records(&mut self, payload: &[u8]) -> Result<(), FileError> {
        let mut reader = Reader::new(payload);
        let records = reader.count(8)?;
        for _ in 0..records {
            let name = reader.utf8()?.to_owned();
            let bytes = reader.bytes()?.to_vec();
            self.used += bytes.len();
            if name.len() > MAX_RECORD_NAME_BYTES || self.records.insert(name, bytes).is_some() {
                return Err(FileError::Corrupt);
            }
        }
        if !reader.is_empty() || self.used > self.quotas.total_bytes {
            return Err(FileError::Corrupt);
        }
        Ok(())
    }

    /// The profile's kind.
    pub fn kind(&self) -> ProfileKind {
        self.kind
    }

    /// The quotas.
    pub fn quotas(&self) -> Quotas {
        self.quotas
    }

    /// Bytes used by all storage in the profile.
    pub fn used_bytes(&self) -> usize {
        self.used
    }

    /// Whether there are changes [`Self::flush`] has not written.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The local storage area of `key`, if it holds anything.
    pub fn local_storage(&self, key: &StorageKey) -> Option<&StorageArea> {
        self.local.get(key)
    }

    /// The storage keys with local storage.
    pub fn local_storage_keys(&self) -> impl Iterator<Item = &StorageKey> {
        self.local.keys()
    }

    /// `localStorage.setItem` for `key`.
    pub fn set_local(
        &mut self,
        key: &StorageKey,
        name: &[u16],
        value: &[u16],
    ) -> Result<Option<StorageChange>, StorageError> {
        if !self.local.contains_key(key) && self.local.len() >= self.quotas.max_keys {
            return Err(StorageError::QuotaExceeded);
        }
        let room = self.quotas.total_bytes.saturating_sub(self.used);
        let area = self.local.entry(key.clone()).or_default();
        let before = area.bytes();
        let result = area.set(name, value, self.quotas.per_key_bytes, room);
        let after = area.bytes();
        if area.is_empty() {
            self.local.remove(key);
        }
        self.used = self.used - before + after;
        let change = result?;
        self.dirty |= change.is_some();
        Ok(change)
    }

    /// `localStorage.removeItem` for `key`.
    pub fn remove_local(&mut self, key: &StorageKey, name: &[u16]) -> Option<StorageChange> {
        let area = self.local.get_mut(key)?;
        let before = area.bytes();
        let change = area.remove(name)?;
        self.used -= before - area.bytes();
        if area.is_empty() {
            self.local.remove(key);
        }
        self.dirty = true;
        Some(change)
    }

    /// `localStorage.clear()` for `key`.
    pub fn clear_local(&mut self, key: &StorageKey) -> Option<StorageChange> {
        let mut area = self.local.remove(key)?;
        self.used -= area.bytes();
        self.dirty = true;
        area.clear()
    }

    /// Remove every storage key of `origin` in every partition, as
    /// `Clear-Site-Data: "storage"` does. Returns how many keys were
    /// cleared.
    pub fn clear_origin(&mut self, origin: &str) -> usize {
        let keys: Vec<StorageKey> = self
            .local
            .keys()
            .filter(|key| key.origin() == origin)
            .cloned()
            .collect();
        for key in &keys {
            self.clear_local(key);
        }
        keys.len()
    }

    /// A service's opaque record, such as the serialized cookie jar.
    pub fn record(&self, name: &str) -> Option<&[u8]> {
        self.records.get(name).map(Vec::as_slice)
    }

    /// Replace a service's record, within the profile's total quota.
    pub fn set_record(&mut self, name: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        if name.is_empty() || name.len() > MAX_RECORD_NAME_BYTES {
            return Err(StorageError::QuotaExceeded);
        }
        let old = self.records.get(name).map_or(0, Vec::len);
        let after = (self.used - old)
            .checked_add(bytes.len())
            .filter(|&after| after <= self.quotas.total_bytes)
            .ok_or(StorageError::QuotaExceeded)?;
        self.records.insert(name.to_owned(), bytes);
        self.used = after;
        self.dirty = true;
        Ok(())
    }

    /// Write a persistent profile's changes atomically. Ephemeral and
    /// private profiles write nothing.
    pub fn flush(&mut self) -> Result<(), ProfileError> {
        let Some(dir) = self.dir.as_ref().filter(|_| self.dirty) else {
            return Ok(());
        };
        let mut local = Vec::new();
        put_u32(&mut local, self.local.len());
        for (key, area) in &self.local {
            put_utf8(&mut local, &key.encode());
            put_u32(&mut local, area.len());
            for (name, value) in area.entries() {
                put_utf16(&mut local, name);
                put_utf16(&mut local, value);
            }
        }
        let mut records = Vec::new();
        put_u32(&mut records, self.records.len());
        for (name, bytes) in &self.records {
            put_utf8(&mut records, name);
            put_bytes(&mut records, bytes);
        }
        write_atomic(dir, LOCAL_STORAGE_FILE, &local).map_err(ProfileError::Io)?;
        write_atomic(dir, RECORDS_FILE, &records).map_err(ProfileError::Io)?;
        self.dirty = false;
        Ok(())
    }
}

fn quarantine(path: &Path, name: &str, error: &FileError) -> Result<Recovery, ProfileError> {
    let dir = path.parent().expect("profile files are in a directory");
    let mut attempt = 0u32;
    let moved_to = loop {
        let candidate = dir.join(format!("{name}.corrupt-{attempt}"));
        if !candidate.exists() {
            break candidate;
        }
        attempt = attempt.checked_add(1).ok_or(ProfileError::Locked)?;
    };
    fs::rename(path, &moved_to).map_err(ProfileError::Io)?;
    Ok(Recovery {
        file: name.to_owned(),
        reason: error.to_string(),
        moved_to,
    })
}

/// The `sessionStorage` areas of one top-level browsing session (HTML
/// §12.2.2): partitioned by storage key like local storage, kept only in
/// memory, and copied when a page opens an auxiliary browsing context.
#[derive(Clone, Debug, Default)]
pub struct SessionStorage {
    quotas: Quotas,
    areas: BTreeMap<StorageKey, StorageArea>,
    used: usize,
}

impl SessionStorage {
    /// An empty session.
    pub fn new(quotas: Quotas) -> Self {
        Self {
            quotas,
            areas: BTreeMap::new(),
            used: 0,
        }
    }

    /// The area of `key`, if it holds anything.
    pub fn area(&self, key: &StorageKey) -> Option<&StorageArea> {
        self.areas.get(key)
    }

    /// `sessionStorage.setItem` for `key`.
    pub fn set(
        &mut self,
        key: &StorageKey,
        name: &[u16],
        value: &[u16],
    ) -> Result<Option<StorageChange>, StorageError> {
        if !self.areas.contains_key(key) && self.areas.len() >= self.quotas.max_keys {
            return Err(StorageError::QuotaExceeded);
        }
        let room = self.quotas.total_bytes.saturating_sub(self.used);
        let area = self.areas.entry(key.clone()).or_default();
        let before = area.bytes();
        let result = area.set(name, value, self.quotas.per_key_bytes, room);
        self.used = self.used - before + area.bytes();
        if area.is_empty() {
            self.areas.remove(key);
        }
        result
    }

    /// `sessionStorage.removeItem` for `key`.
    pub fn remove(&mut self, key: &StorageKey, name: &[u16]) -> Option<StorageChange> {
        let area = self.areas.get_mut(key)?;
        let change = area.remove(name)?;
        self.used -= cost(name, change.old_value.as_deref().unwrap_or_default());
        if area.is_empty() {
            self.areas.remove(key);
        }
        Some(change)
    }

    /// `sessionStorage.clear()` for `key`.
    pub fn clear(&mut self, key: &StorageKey) -> Option<StorageChange> {
        let mut area = self.areas.remove(key)?;
        self.used -= area.bytes();
        area.clear()
    }
}
