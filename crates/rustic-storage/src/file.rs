//! Versioned profile files, replaced atomically.
//!
//! Layout: the 8-byte magic `RUSTICPF`, the format version (`u32` little
//! endian), the payload length (`u64`), the payload, then a CRC-32 (IEEE)
//! of everything before it. A writer creates `<name>.tmp`, syncs it, renames
//! it over `<name>` and syncs the directory, so a crash leaves the old file
//! or the new one, never a mix. The checksum detects torn or edited files;
//! it is not a security boundary: the profile directory belongs to the user.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"RUSTICPF";
const HEADER_BYTES: usize = 8 + 4 + 8;
const TRAILER_BYTES: usize = 4;

/// The format version this build writes. Files with a newer version are
/// refused untouched; older versions are migrated when one exists (none
/// does yet).
pub const FORMAT_VERSION: u32 = 1;

/// Why a profile file could not be used.
#[derive(Debug)]
pub enum FileError {
    /// Reading or writing failed.
    Io(io::Error),
    /// The file is larger than the profile's quotas allow.
    TooLarge,
    /// The magic, length or checksum is wrong, or the payload is malformed.
    Corrupt,
    /// A newer build wrote this file.
    NewerVersion(u32),
}

impl fmt::Display for FileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "profile file I/O failed: {error}"),
            Self::TooLarge => formatter.write_str("profile file is larger than its quota"),
            Self::Corrupt => formatter.write_str("profile file is corrupt"),
            Self::NewerVersion(version) => write!(
                formatter,
                "profile file has format version {version}, newer than {FORMAT_VERSION}"
            ),
        }
    }
}

impl std::error::Error for FileError {}

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                0xEDB8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc = CRC_TABLE[usize::from((crc as u8) ^ byte)] ^ (crc >> 8);
    }
    !crc
}

/// Write `payload` as `dir/name`, atomically.
pub(crate) fn write_atomic(dir: &Path, name: &str, payload: &[u8]) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(HEADER_BYTES + payload.len() + TRAILER_BYTES);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(payload);
    let crc = crc32(&bytes);
    bytes.extend_from_slice(&crc.to_le_bytes());
    let temporary = dir.join(format!("{name}.tmp"));
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temporary, dir.join(name))?;
    // Persist the rename itself. Directories cannot be opened for syncing
    // on every platform; the rename is still atomic there.
    if let Ok(directory) = File::open(dir) {
        let _ = directory.sync_all();
    }
    Ok(())
}

/// Read `path`'s payload, at most `max_payload` bytes. A missing file is
/// `Ok(None)`.
pub(crate) fn read_versioned(
    path: &Path,
    max_payload: usize,
) -> Result<Option<Vec<u8>>, FileError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(FileError::Io(error)),
    };
    let limit = max_payload.saturating_add(HEADER_BYTES + TRAILER_BYTES);
    let metadata = file.metadata().map_err(FileError::Io)?;
    if !metadata.is_file() {
        return Err(FileError::Corrupt);
    }
    if metadata.len() > limit as u64 {
        return Err(FileError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(FileError::Io)?;
    if bytes.len() > limit {
        return Err(FileError::TooLarge);
    }
    decode(&bytes).map(Some)
}

fn decode(bytes: &[u8]) -> Result<Vec<u8>, FileError> {
    if bytes.len() < HEADER_BYTES + TRAILER_BYTES || &bytes[..8] != MAGIC {
        return Err(FileError::Corrupt);
    }
    let (body, trailer) = bytes.split_at(bytes.len() - TRAILER_BYTES);
    let stored = u32::from_le_bytes(trailer.try_into().expect("four bytes"));
    let version = u32::from_le_bytes(body[8..12].try_into().expect("four bytes"));
    if crc32(body) != stored {
        return Err(FileError::Corrupt);
    }
    if version > FORMAT_VERSION {
        return Err(FileError::NewerVersion(version));
    }
    if version < FORMAT_VERSION {
        // No older format has been released, so nothing can migrate.
        return Err(FileError::Corrupt);
    }
    let length = u64::from_le_bytes(body[12..20].try_into().expect("eight bytes"));
    let payload = &body[HEADER_BYTES..];
    if length != payload.len() as u64 {
        return Err(FileError::Corrupt);
    }
    Ok(payload.to_vec())
}

/// A bounds-checked reader over a payload.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], FileError> {
        if count > self.bytes.len() {
            return Err(FileError::Corrupt);
        }
        let (taken, rest) = self.bytes.split_at(count);
        self.bytes = rest;
        Ok(taken)
    }

    pub(crate) fn u32(&mut self) -> Result<u32, FileError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("four bytes"),
        ))
    }

    /// A count of items that each need at least `min_item_bytes`, checked
    /// against the bytes left before anything is allocated.
    pub(crate) fn count(&mut self, min_item_bytes: usize) -> Result<usize, FileError> {
        let count = self.u32()? as usize;
        if count.saturating_mul(min_item_bytes.max(1)) > self.bytes.len() {
            return Err(FileError::Corrupt);
        }
        Ok(count)
    }

    pub(crate) fn utf8(&mut self) -> Result<&'a str, FileError> {
        let length = self.count(1)?;
        std::str::from_utf8(self.take(length)?).map_err(|_| FileError::Corrupt)
    }

    pub(crate) fn bytes(&mut self) -> Result<&'a [u8], FileError> {
        let length = self.count(1)?;
        self.take(length)
    }

    pub(crate) fn utf16(&mut self) -> Result<Vec<u16>, FileError> {
        let length = self.count(2)?;
        let bytes = self.take(length * 2)?;
        Ok(bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect())
    }
}

pub(crate) fn put_u32(out: &mut Vec<u8>, value: usize) {
    out.extend_from_slice(
        &u32::try_from(value)
            .expect("bounded by quotas")
            .to_le_bytes(),
    );
}

pub(crate) fn put_utf8(out: &mut Vec<u8>, text: &str) {
    put_u32(out, text.len());
    out.extend_from_slice(text.as_bytes());
}

pub(crate) fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_u32(out, bytes.len());
    out.extend_from_slice(bytes);
}

pub(crate) fn put_utf16(out: &mut Vec<u8>, units: &[u16]) {
    put_u32(out, units.len());
    for unit in units {
        out.extend_from_slice(&unit.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_ieee_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn files_round_trip_and_damage_is_detected() {
        let dir = std::env::temp_dir().join(format!("rustic-storage-file-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        write_atomic(&dir, "data", b"payload").unwrap();
        let path = dir.join("data");
        assert_eq!(read_versioned(&path, 64).unwrap().unwrap(), b"payload");
        assert!(!dir.join("data.tmp").exists());
        assert!(matches!(read_versioned(&path, 3), Err(FileError::TooLarge)));
        let mut bytes = fs::read(&path).unwrap();
        bytes[HEADER_BYTES] ^= 1;
        fs::write(&path, &bytes).unwrap();
        assert!(matches!(read_versioned(&path, 64), Err(FileError::Corrupt)));
        // A newer version with a valid checksum is refused as newer.
        let mut newer = Vec::new();
        newer.extend_from_slice(MAGIC);
        newer.extend_from_slice(&2u32.to_le_bytes());
        newer.extend_from_slice(&0u64.to_le_bytes());
        let crc = crc32(&newer);
        newer.extend_from_slice(&crc.to_le_bytes());
        fs::write(&path, &newer).unwrap();
        assert!(matches!(
            read_versioned(&path, 64),
            Err(FileError::NewerVersion(2))
        ));
        fs::write(&path, b"short").unwrap();
        assert!(matches!(read_versioned(&path, 64), Err(FileError::Corrupt)));
        assert!(read_versioned(&dir.join("missing"), 64).unwrap().is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn readers_check_counts_before_allocating() {
        let mut out = Vec::new();
        put_utf8(&mut out, "ab");
        put_utf16(&mut out, &[0x41, 0xD800]);
        let mut reader = Reader::new(&out);
        assert_eq!(reader.utf8().unwrap(), "ab");
        assert_eq!(reader.utf16().unwrap(), [0x41, 0xD800]);
        assert!(reader.is_empty());
        let huge = u32::MAX.to_le_bytes();
        assert!(matches!(
            Reader::new(&huge).utf16(),
            Err(FileError::Corrupt)
        ));
    }
}
