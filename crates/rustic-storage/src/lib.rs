//! Browser profiles and the storage they hold (NET-06).
//!
//! A [`Profile`] is one user's browsing state. It is *ephemeral* (memory
//! only, the default for tests and the CLI), *private* (memory only, never
//! written, cleared when dropped) or *persistent* (a directory guarded by a
//! [`ProfileLock`], with versioned, checksummed files replaced atomically).
//! Corrupt or unreadable files are moved aside and reported as
//! [`Recovery`] entries; their contents are never trusted.
//!
//! Storage is keyed by a [`StorageKey`]: the origin and the top-level site
//! it is partitioned under, so a third-party frame's storage is separate
//! from the same origin's first-party storage. Every key has a quota, and
//! the profile has a total quota ([`Quotas`]).
//!
//! [`StorageArea`] implements the HTML Web Storage model (`localStorage`
//! and `sessionStorage`) over UTF-16 keys and values. Local storage lives
//! in the profile and persists with it; [`SessionStorage`] belongs to one
//! top-level browsing session and is never written to disk.
//!
//! The crate does no networking and knows no sites: callers compute the
//! serialized origin and top-level site (`rustic-net`'s `site` module) and
//! hand them in.

mod area;
mod file;
mod key;
mod profile;

pub use area::{StorageArea, StorageChange, StorageError};
pub use file::{FORMAT_VERSION, FileError};
pub use key::{StorageKey, StorageKeyError};
pub use profile::{
    Profile, ProfileError, ProfileKind, ProfileLock, Quotas, Recovery, SessionStorage,
};
