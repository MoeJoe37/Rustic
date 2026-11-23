//! The run's browser profile and the Web Storage it gives network
//! documents (NET-06).
//!
//! The profile is ephemeral unless `--profile DIR` names a persistent one
//! (locked while the run uses it, written after each scripted network
//! document) or `--private` asks for a private one, which never writes.
//! A network document's `localStorage` is keyed by its origin and its own
//! site (CLI documents are always top-level); `sessionStorage` lasts for
//! one document load. Local files have no Web Storage: their origin is
//! opaque, so the getters throw `SecurityError`.

use std::cell::RefCell;
use std::ffi::OsString;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use rustic_base::EscapedText;
use rustic_engine::{QuotaExceeded, StorageHost, StorageKind};
use rustic_net::site::PublicSuffixList;
use rustic_storage::{Profile, ProfileError, Quotas, SessionStorage, StorageKey};
use rustic_url::Url;

#[derive(Debug, Default)]
enum ProfileChoice {
    #[default]
    Ephemeral,
    Private,
    Persistent(PathBuf),
}

static CHOICE: OnceLock<ProfileChoice> = OnceLock::new();

/// The open profile, shared by every page of the run: the window's tabs
/// each keep a live page on their own thread (TD-196), and one profile
/// lock and one set of Web Storage areas serve them all.
static PROFILE: Mutex<Option<Arc<Mutex<Profile>>>> = Mutex::new(None);

/// Lock `mutex`, also after a thread panicked while holding it (the
/// profile stays consistent: each operation completes or fails whole).
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Remove `--profile DIR` and `--private` from `args`.
pub fn configure(args: Vec<OsString>) -> Result<Vec<OsString>, String> {
    let mut output = Vec::new();
    let mut choice = ProfileChoice::Ephemeral;
    let mut chosen = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            output.push(arg);
            output.extend(args);
            break;
        }
        let next = if arg == "--profile" {
            let dir = args.next().ok_or("--profile requires a directory")?;
            ProfileChoice::Persistent(PathBuf::from(dir))
        } else if arg == "--private" {
            ProfileChoice::Private
        } else {
            output.push(arg);
            continue;
        };
        if std::mem::replace(&mut chosen, true) {
            return Err("give at most one of --profile and --private".into());
        }
        choice = next;
    }
    CHOICE
        .set(choice)
        .map_err(|_| "profile options configured twice")?;
    Ok(output)
}

/// The profile's label for the browser shell: its directory's name, or
/// what kind it is.
#[cfg_attr(
    not(feature = "window"),
    expect(dead_code, reason = "read by the window shell")
)]
pub fn label() -> String {
    match CHOICE.get_or_init(ProfileChoice::default) {
        ProfileChoice::Ephemeral => "Temporary profile".to_owned(),
        ProfileChoice::Private => "Private".to_owned(),
        ProfileChoice::Persistent(dir) => dir.file_name().map_or_else(
            || dir.to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        ),
    }
}

/// The run's profile, opened on first use.
fn profile() -> Result<Arc<Mutex<Profile>>, ProfileError> {
    {
        let mut cell = locked(&PROFILE);
        if let Some(profile) = cell.as_ref() {
            return Ok(Arc::clone(profile));
        }
        let quotas = Quotas::default();
        let profile = match CHOICE.get_or_init(ProfileChoice::default) {
            ProfileChoice::Ephemeral => Profile::ephemeral(quotas),
            ProfileChoice::Private => Profile::private(quotas),
            ProfileChoice::Persistent(dir) => {
                let (profile, recoveries) = Profile::open(dir, quotas)?;
                for recovery in recoveries {
                    eprintln!(
                        "rustic: profile: {} was unreadable ({}); moved to {}",
                        recovery.file,
                        recovery.reason,
                        EscapedText(&recovery.moved_to.to_string_lossy())
                    );
                }
                profile
            }
        };
        let profile = Arc::new(Mutex::new(profile));
        *cell = Some(Arc::clone(&profile));
        Ok(profile)
    }
}

/// The profile record holding the cookie jar's persistent cookies.
const COOKIE_RECORD: &str = "cookies";

/// The saved persistent cookies, if any.
pub(crate) fn saved_cookies() -> Result<Option<Vec<u8>>, ProfileError> {
    let profile = profile()?;
    let saved = locked(&profile).record(COOKIE_RECORD).map(<[u8]>::to_vec);
    Ok(saved)
}

/// Keep the jar's persistent cookies in the profile, then flush it.
pub(crate) fn save_cookies(bytes: Vec<u8>) -> Result<(), ProfileError> {
    let profile = profile()?;
    if locked(&profile).set_record(COOKIE_RECORD, bytes).is_err() {
        eprintln!("rustic: profile: the cookie jar is over the profile quota; not saved");
    }
    flush()
}

/// Write the profile's changes, if it is persistent.
pub fn flush() -> Result<(), ProfileError> {
    let profile = locked(&PROFILE).clone();
    match profile {
        Some(profile) => locked(&profile).flush(),
        None => Ok(()),
    }
}

/// The Web Storage host for a top-level document at `url`, or `None` when
/// its origin is opaque.
pub(crate) fn host_for(url: &Url) -> Result<Option<Rc<dyn StorageHost>>, ProfileError> {
    let origin = url.origin();
    let site = PublicSuffixList::bundled().site(&origin).serialize();
    let Ok(key) = StorageKey::new(&origin.serialize(), &site) else {
        return Ok(None);
    };
    let profile = profile()?;
    let quotas = locked(&profile).quotas();
    Ok(Some(Rc::new(DocumentStorage {
        profile,
        session: RefCell::new(SessionStorage::new(quotas)),
        key,
    })))
}

/// One document's view of its local and session storage.
#[derive(Debug)]
struct DocumentStorage {
    profile: Arc<Mutex<Profile>>,
    session: RefCell<SessionStorage>,
    key: StorageKey,
}

impl DocumentStorage {
    fn read<T>(
        &self,
        kind: StorageKind,
        f: impl FnOnce(&rustic_storage::StorageArea) -> T,
    ) -> Option<T> {
        match kind {
            StorageKind::Local => locked(&self.profile).local_storage(&self.key).map(f),
            StorageKind::Session => self.session.borrow().area(&self.key).map(f),
        }
    }
}

impl StorageHost for DocumentStorage {
    fn length(&self, kind: StorageKind) -> u32 {
        self.read(kind, |area| u32::try_from(area.len()).unwrap_or(u32::MAX))
            .unwrap_or(0)
    }

    fn key(&self, kind: StorageKind, index: u32) -> Option<Vec<u16>> {
        self.read(kind, |area| area.key(index as usize).map(<[u16]>::to_vec))
            .flatten()
    }

    fn get(&self, kind: StorageKind, key: &[u16]) -> Option<Vec<u16>> {
        self.read(kind, |area| area.get(key).map(<[u16]>::to_vec))
            .flatten()
    }

    fn set(&self, kind: StorageKind, key: &[u16], value: &[u16]) -> Result<(), QuotaExceeded> {
        let result = match kind {
            StorageKind::Local => locked(&self.profile).set_local(&self.key, key, value),
            StorageKind::Session => self.session.borrow_mut().set(&self.key, key, value),
        };
        result.map(|_| ()).map_err(|_| QuotaExceeded)
    }

    fn remove(&self, kind: StorageKind, key: &[u16]) {
        match kind {
            StorageKind::Local => {
                locked(&self.profile).remove_local(&self.key, key);
            }
            StorageKind::Session => {
                self.session.borrow_mut().remove(&self.key, key);
            }
        }
    }

    fn clear(&self, kind: StorageKind) {
        match kind {
            StorageKind::Local => {
                locked(&self.profile).clear_local(&self.key);
            }
            StorageKind::Session => {
                self.session.borrow_mut().clear(&self.key);
            }
        }
    }

    fn keys(&self, kind: StorageKind) -> Vec<Vec<u16>> {
        self.read(kind, |area| area.keys().map(<[u16]>::to_vec).collect())
            .unwrap_or_default()
    }
}
