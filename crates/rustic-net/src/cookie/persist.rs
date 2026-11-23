//! Saving persistent cookies with a profile (RFC 6265bis §5.7: session
//! cookies end with the session; cookies with an expiry outlive it).
//!
//! The format is `RCJ1`, a count, then per cookie length-prefixed fields.
//! It lives inside a checksummed profile record, but import still checks
//! every field and limit: a profile file is never trusted as config.

use super::{Cookie, CookieJar, SameSite};
use crate::site::Site;

const MAGIC: &[u8; 4] = b"RCJ1";
/// Longest field accepted on import (RFC 6265bis caps name plus value at
/// 4096 bytes and attributes at 1024).
const MAX_FIELD_BYTES: usize = 4096;

fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        if count > self.0.len() {
            return None;
        }
        let (taken, rest) = self.0.split_at(count);
        self.0 = rest;
        Some(taken)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn field(&mut self) -> Option<&'a str> {
        let length = self.u32()? as usize;
        if length > MAX_FIELD_BYTES {
            return None;
        }
        std::str::from_utf8(self.take(length)?).ok()
    }
}

impl CookieJar {
    /// Serialize the cookies that should outlive the session: those with
    /// an expiry after `now`.
    pub fn export_persistent(&self, now: i64) -> Vec<u8> {
        let kept: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|cookie| cookie.expiry.is_some() && !cookie.is_expired(now))
            .collect();
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&(kept.len() as u32).to_le_bytes());
        for cookie in kept {
            put(&mut out, cookie.name.as_bytes());
            put(&mut out, cookie.value.as_bytes());
            put(&mut out, cookie.domain.as_bytes());
            put(&mut out, cookie.path.as_bytes());
            let expiry = cookie.expiry.expect("filtered").to_string();
            put(&mut out, expiry.as_bytes());
            let flags = [
                u8::from(cookie.host_only),
                u8::from(cookie.secure_only),
                u8::from(cookie.http_only),
                match cookie.same_site {
                    SameSite::Strict => 0,
                    SameSite::Lax => 1,
                    SameSite::None => 2,
                    SameSite::Default => 3,
                },
            ];
            put(&mut out, &flags);
            let partition = cookie.partition.as_ref().map(Site::serialize);
            put(&mut out, partition.unwrap_or_default().as_bytes());
        }
        out
    }

    /// Add cookies saved by [`Self::export_persistent`], dropping any that
    /// expired by `now` or fail a check, then apply the jar's limits.
    /// Returns how many were restored, or `None` if the data is malformed
    /// (the jar is then unchanged).
    pub fn import_persistent(&mut self, bytes: &[u8], now: i64) -> Option<usize> {
        let mut reader = Reader(bytes);
        if reader.take(4)? != MAGIC {
            return None;
        }
        let count = reader.u32()? as usize;
        if count > self.limits.total.saturating_mul(2) {
            return None;
        }
        let mut restored = Vec::new();
        for _ in 0..count {
            let name = reader.field()?.to_owned();
            let value = reader.field()?.to_owned();
            let domain = reader.field()?.to_ascii_lowercase();
            let path = reader.field()?.to_owned();
            let expiry: i64 = reader.field()?.parse().ok()?;
            let flags = reader.field()?.as_bytes();
            let [host_only, secure_only, http_only, same_site] = flags else {
                return None;
            };
            let same_site = match same_site {
                0 => SameSite::Strict,
                1 => SameSite::Lax,
                2 => SameSite::None,
                3 => SameSite::Default,
                _ => return None,
            };
            let partition = match reader.field()? {
                "" => None,
                text => Some(Site::parse_serialized(text)?),
            };
            if domain.is_empty() || !path.starts_with('/') || name.len() + value.len() > 4096 {
                return None;
            }
            restored.push(Cookie {
                name,
                value,
                expiry: Some(expiry),
                domain,
                path,
                creation: 0,
                // A restored cookie's age is unknown, so it gets no
                // Lax-allowing-unsafe window.
                created_at: i64::MIN,
                last_access: 0,
                host_only: *host_only == 1,
                secure_only: *secure_only == 1,
                http_only: *http_only == 1,
                same_site,
                partition,
            });
        }
        if !reader.0.is_empty() {
            return None;
        }
        let mut added = 0;
        for mut cookie in restored {
            if cookie.is_expired(now) || self.cookies.iter().any(|old| old.same_identity(&cookie)) {
                continue;
            }
            cookie.creation = self.tick();
            cookie.last_access = cookie.creation;
            self.cookies.push(cookie);
            added += 1;
        }
        self.evict(now);
        Some(added)
    }
}
