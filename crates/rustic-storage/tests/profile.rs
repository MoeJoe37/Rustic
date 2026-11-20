//! Profiles: partitioned local storage, quotas, persistence across opens,
//! the lock, corruption recovery, newer formats, private and session
//! storage.

use std::fs;
use std::path::PathBuf;

use rustic_storage::{
    Profile, ProfileError, ProfileKind, Quotas, SessionStorage, StorageError, StorageKey,
};

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn key(origin: &str, site: &str) -> StorageKey {
    StorageKey::new(origin, site).unwrap()
}

/// An emoji (two code units) and a lone surrogate: three code units.
fn lone_surrogate() -> Vec<u16> {
    let mut value = units("\u{1F600}");
    value.push(0xD800);
    value
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rustic-storage-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&path);
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn storage_is_partitioned_by_top_level_site_and_cleared_per_origin() {
    let mut profile = Profile::ephemeral(Quotas::default());
    let first_party = key("https://a.example", "https://example");
    let embedded = key("https://a.example", "https://other.test");
    profile
        .set_local(&first_party, &units("k"), &units("first"))
        .unwrap();
    profile
        .set_local(&embedded, &units("k"), &units("third"))
        .unwrap();
    let get = |profile: &Profile, key: &StorageKey| {
        profile
            .local_storage(key)
            .and_then(|area| area.get(&units("k")))
            .map(<[u16]>::to_vec)
    };
    assert_eq!(get(&profile, &first_party), Some(units("first")));
    assert_eq!(get(&profile, &embedded), Some(units("third")));
    let other = key("https://b.example", "https://example");
    profile.set_local(&other, &units("k"), &units("b")).unwrap();
    assert_eq!(profile.clear_origin("https://a.example"), 2);
    assert_eq!(get(&profile, &first_party), None);
    assert_eq!(get(&profile, &other), Some(units("b")));
    assert_eq!(profile.used_bytes(), 4);
}

#[test]
fn per_key_total_and_key_count_quotas_hold_exactly() {
    let quotas = Quotas {
        per_key_bytes: 8,
        total_bytes: 12,
        max_keys: 2,
    };
    let mut profile = Profile::ephemeral(quotas);
    let a = key("https://a.test", "https://a.test");
    let b = key("https://b.test", "https://b.test");
    let c = key("https://c.test", "https://c.test");
    // 8 bytes: exactly the per-key quota.
    profile.set_local(&a, &units("ab"), &units("cd")).unwrap();
    assert_eq!(
        profile.set_local(&a, &units("e"), &[]),
        Err(StorageError::QuotaExceeded)
    );
    // 4 more bytes: exactly the total.
    profile.set_local(&b, &units("x"), &units("y")).unwrap();
    assert_eq!(profile.used_bytes(), 12);
    assert_eq!(
        profile.set_local(&b, &units("x"), &units("yy")),
        Err(StorageError::QuotaExceeded)
    );
    // A third key is refused even with no bytes.
    profile.remove_local(&b, &units("x")).unwrap();
    profile.set_local(&b, &units("x"), &[]).unwrap();
    assert_eq!(
        profile.set_local(&c, &[], &[]),
        Err(StorageError::QuotaExceeded)
    );
    assert_eq!(profile.used_bytes(), 10);
    // Records count against the same total.
    assert_eq!(
        profile.set_record("cookies", vec![0; 3]),
        Err(StorageError::QuotaExceeded)
    );
    profile.set_record("cookies", vec![0; 2]).unwrap();
}

#[test]
fn persistent_profiles_survive_reopening_and_are_locked() {
    let dir = TempDir::new("persist");
    let site = key("https://a.test", "https://a.test");
    {
        let (mut profile, recoveries) = Profile::open(&dir.0, Quotas::default()).unwrap();
        assert!(recoveries.is_empty());
        assert_eq!(profile.kind(), ProfileKind::Persistent);
        assert!(matches!(
            Profile::open(&dir.0, Quotas::default()),
            Err(ProfileError::Locked)
        ));
        profile
            .set_local(&site, &units("name"), &lone_surrogate())
            .unwrap();
        profile.set_record("cookies", b"jar".to_vec()).unwrap();
        assert!(profile.is_dirty());
        profile.flush().unwrap();
        assert!(!profile.is_dirty());
    }
    let (profile, recoveries) = Profile::open(&dir.0, Quotas::default()).unwrap();
    assert!(recoveries.is_empty());
    // Lone surrogates survive: values are UTF-16 code units.
    assert_eq!(
        profile.local_storage(&site).unwrap().get(&units("name")),
        Some(lone_surrogate().as_slice())
    );
    assert_eq!(profile.record("cookies"), Some(b"jar".as_slice()));
    assert_eq!(profile.used_bytes(), 2 * (4 + 3) + 3);
}

#[test]
fn corrupt_files_are_quarantined_and_newer_formats_refused() {
    let dir = TempDir::new("corrupt");
    let site = key("https://a.test", "https://a.test");
    {
        let (mut profile, _) = Profile::open(&dir.0, Quotas::default()).unwrap();
        profile.set_local(&site, &units("k"), &units("v")).unwrap();
        profile.set_record("cookies", b"jar".to_vec()).unwrap();
        profile.flush().unwrap();
    }
    let path = dir.0.join("local-storage");
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 5;
    bytes[last] ^= 0x40;
    fs::write(&path, &bytes).unwrap();
    {
        let (profile, recoveries) = Profile::open(&dir.0, Quotas::default()).unwrap();
        assert_eq!(recoveries.len(), 1);
        assert_eq!(recoveries[0].file, "local-storage");
        assert!(recoveries[0].moved_to.exists());
        assert!(!path.exists());
        assert!(profile.local_storage(&site).is_none());
        // The intact store is kept.
        assert_eq!(profile.record("cookies"), Some(b"jar".as_slice()));
    }
    // A quota lowered below the stored data also sets the file aside.
    {
        let (mut profile, _) = Profile::open(&dir.0, Quotas::default()).unwrap();
        profile
            .set_local(&site, &units("k"), &units("vvvv"))
            .unwrap();
        profile.flush().unwrap();
    }
    let small = Quotas {
        per_key_bytes: 4,
        ..Quotas::default()
    };
    let (_, recoveries) = Profile::open(&dir.0, small).unwrap();
    assert_eq!(recoveries.len(), 1);
    // A file from a newer build blocks opening and stays as it was.
    let mut newer = b"RUSTICPF".to_vec();
    newer.extend_from_slice(&7u32.to_le_bytes());
    newer.extend_from_slice(&0u64.to_le_bytes());
    let crc = crc32(&newer);
    newer.extend_from_slice(&crc.to_le_bytes());
    fs::write(dir.0.join("records"), &newer).unwrap();
    assert!(matches!(
        Profile::open(&dir.0, Quotas::default()),
        Err(ProfileError::NewerFormat(7))
    ));
    assert_eq!(fs::read(dir.0.join("records")).unwrap(), newer);
    // The failed open released its lock.
    assert!(!dir.0.join("LOCK").exists());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[test]
fn private_and_ephemeral_profiles_write_nothing() {
    for mut profile in [
        Profile::private(Quotas::default()),
        Profile::ephemeral(Quotas::default()),
    ] {
        let site = key("https://a.test", "https://a.test");
        profile.set_local(&site, &units("k"), &units("v")).unwrap();
        profile.flush().unwrap();
        assert!(profile.local_storage(&site).is_some());
    }
    assert_eq!(
        Profile::private(Quotas::default()).kind(),
        ProfileKind::Private
    );
}

#[test]
fn session_storage_is_separate_and_copyable() {
    let mut session = SessionStorage::new(Quotas::default());
    let site = key("https://a.test", "https://a.test");
    session.set(&site, &units("k"), &units("v")).unwrap();
    let copy = session.clone();
    session.remove(&site, &units("k")).unwrap();
    assert!(session.area(&site).is_none());
    assert_eq!(
        copy.area(&site).unwrap().get(&units("k")),
        Some(units("v").as_slice())
    );
    let profile = Profile::ephemeral(Quotas::default());
    assert!(profile.local_storage(&site).is_none());
    assert!(session.clear(&site).is_none());
}
