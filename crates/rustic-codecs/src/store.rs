//! Image resource lifecycle: encoded bytes keyed by URL, header probing
//! for natural sizes, lazy decoding under a shared memory budget, eviction
//! and sticky failure.
//!
//! An entry moves through the states of the HTML image request model:
//! after [`ImageStore::insert`] its dimensions are known from the header
//! alone (*available size*), [`ImageStore::decode`] produces pixels
//! (*completely available*), and any decoding failure makes it *broken*
//! for good. Decoded pixels are charged against `max_decoded_bytes`; when
//! a new decode would exceed it, the least recently used decoded entries
//! drop their pixels (keeping their encoded bytes) and decode again on the
//! next request. Callers that still hold an [`Arc`] keep that copy alive,
//! so the budget bounds what the store retains, not every outstanding
//! reference (TD-83).

use crate::image::{Animation, DecodeLimits, ImageError, ImageFormat, decode_animation, sniff};
use rustic_base::CancellationToken;
use std::collections::HashMap;
use std::sync::Arc;

/// Budgets for one store (typically one document or one process).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoreLimits {
    /// Distinct images kept; later inserts are refused.
    pub max_entries: usize,
    /// Total encoded bytes kept.
    pub max_encoded_bytes: usize,
    /// Total decoded bytes kept across entries.
    pub max_decoded_bytes: usize,
    /// Per-image decoder limits; `max_bytes` is further capped by the
    /// decoded budget.
    pub decode: DecodeLimits,
}

impl Default for StoreLimits {
    fn default() -> Self {
        Self {
            max_entries: 1024,
            max_encoded_bytes: 64 * 1024 * 1024,
            max_decoded_bytes: 256 * 1024 * 1024,
            decode: DecodeLimits::default(),
        }
    }
}

/// Format and natural size read from an image's header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageInfo {
    /// The sniffed format.
    pub format: ImageFormat,
    /// Natural width in pixels, after EXIF orientation.
    pub width: u32,
    /// Natural height in pixels, after EXIF orientation.
    pub height: u32,
}

/// Why an image is not (or no longer) usable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreError {
    /// The store's entry or encoded-byte budget is spent.
    Full,
    /// No entry has this id (it was released).
    Unknown,
    /// Decoding failed or the header was unreadable; the entry is broken.
    Broken(ImageError),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => f.write_str("image store budget exhausted"),
            Self::Unknown => f.write_str("unknown image"),
            Self::Broken(error) => write!(f, "broken image: {error}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// A handle to a stored image.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ImageId(usize);

/// Lifecycle state of an entry.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageState {
    /// Header read; pixels not decoded (or evicted).
    SizeAvailable(ImageInfo),
    /// Pixels decoded and retained.
    Decoded(ImageInfo),
    /// Unusable; requests fail with the recorded error.
    Broken(ImageError),
}

struct Entry {
    key: String,
    bytes: Arc<[u8]>,
    info: Result<ImageInfo, ImageError>,
    decoded: Option<Arc<Animation>>,
    last_use: u64,
}

/// Totals for diagnostics and tests.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StoreStats {
    /// Live entries.
    pub entries: usize,
    /// Encoded bytes held.
    pub encoded_bytes: usize,
    /// Decoded bytes held.
    pub decoded_bytes: usize,
    /// Decodes performed, including re-decodes after eviction.
    pub decodes: usize,
    /// Decoded entries evicted to make room.
    pub evictions: usize,
}

/// Image resources shared by everything that displays them.
pub struct ImageStore {
    limits: StoreLimits,
    entries: Vec<Option<Entry>>,
    by_key: HashMap<String, ImageId>,
    clock: u64,
    stats: StoreStats,
}

impl ImageStore {
    /// An empty store.
    pub fn new(limits: StoreLimits) -> Self {
        Self {
            limits,
            entries: Vec::new(),
            by_key: HashMap::new(),
            clock: 0,
            stats: StoreStats::default(),
        }
    }

    /// Current totals.
    pub fn stats(&self) -> StoreStats {
        self.stats
    }

    /// The id already stored under `key`, if any.
    pub fn lookup(&self, key: &str) -> Option<ImageId> {
        self.by_key.get(key).copied()
    }

    /// Store encoded bytes under `key` (normally the resolved URL) and read
    /// the header. The same key returns the existing entry without storing
    /// the bytes again. An unreadable header still creates an entry, in the
    /// broken state.
    pub fn insert(&mut self, key: &str, bytes: Vec<u8>) -> Result<ImageId, StoreError> {
        if let Some(id) = self.lookup(key) {
            return Ok(id);
        }
        if self.stats.entries >= self.limits.max_entries
            || bytes.len() > self.limits.max_encoded_bytes - self.stats.encoded_bytes
        {
            return Err(StoreError::Full);
        }
        let info = probe(&bytes).and_then(|info| {
            let d = self.limits.decode;
            // EXIF orientation may swap the axes the decoder checks.
            let side = d.max_width.max(d.max_height);
            if info.width > side || info.height > side {
                Err(ImageError::Limit)
            } else {
                Ok(info)
            }
        });
        self.stats.entries += 1;
        self.stats.encoded_bytes += bytes.len();
        let id = ImageId(self.entries.len());
        self.entries.push(Some(Entry {
            key: key.to_owned(),
            bytes: bytes.into(),
            info,
            decoded: None,
            last_use: 0,
        }));
        self.by_key.insert(key.to_owned(), id);
        Ok(id)
    }

    /// The entry's lifecycle state.
    pub fn state(&self, id: ImageId) -> Result<ImageState, StoreError> {
        let entry = self.entry(id)?;
        Ok(match (&entry.info, &entry.decoded) {
            (Err(error), _) => ImageState::Broken(*error),
            (Ok(info), Some(_)) => ImageState::Decoded(*info),
            (Ok(info), None) => ImageState::SizeAvailable(*info),
        })
    }

    /// Natural size from the header, without decoding.
    pub fn info(&self, id: ImageId) -> Result<ImageInfo, StoreError> {
        self.entry(id)?.info.map_err(StoreError::Broken)
    }

    /// Decoded frames, decoding (or re-decoding after eviction) as needed.
    /// A failure marks the entry broken; cancellation does not.
    pub fn decode(
        &mut self,
        id: ImageId,
        cancellation: &CancellationToken,
    ) -> Result<Arc<Animation>, StoreError> {
        self.clock += 1;
        let clock = self.clock;
        let entry = self.entry_mut(id)?;
        entry.info.map_err(StoreError::Broken)?;
        entry.last_use = clock;
        if let Some(decoded) = &entry.decoded {
            return Ok(Arc::clone(decoded));
        }
        let bytes = Arc::clone(&entry.bytes);
        let mut limits = self.limits.decode;
        limits.max_bytes = limits.max_bytes.min(self.limits.max_decoded_bytes);
        let result = decode_animation(&bytes, limits, cancellation);
        let animation = match result {
            Ok(animation) => Arc::new(animation),
            Err(ImageError::Cancelled) => return Err(StoreError::Broken(ImageError::Cancelled)),
            Err(error) => {
                self.entry_mut(id)?.info = Err(error);
                return Err(StoreError::Broken(error));
            }
        };
        let size = animation.byte_len();
        self.stats.decodes += 1;
        // The decoder's byte limit is at most the whole budget, so after
        // evicting every other entry the new pixels always fit.
        self.make_room(size, id);
        self.stats.decoded_bytes += size;
        self.entry_mut(id)?.decoded = Some(Arc::clone(&animation));
        Ok(animation)
    }

    /// Drop the decoded pixels of `id`, keeping its encoded bytes.
    pub fn evict(&mut self, id: ImageId) {
        if let Ok(entry) = self.entry_mut(id) {
            if let Some(decoded) = entry.decoded.take() {
                self.stats.decoded_bytes -= decoded.byte_len();
                self.stats.evictions += 1;
            }
        }
    }

    /// Forget `id` entirely (no document refers to it any more).
    pub fn release(&mut self, id: ImageId) {
        self.evict(id);
        if let Some(entry) = self.entries.get_mut(id.0).and_then(Option::take) {
            self.stats.entries -= 1;
            self.stats.encoded_bytes -= entry.bytes.len();
            self.by_key.remove(&entry.key);
        }
    }

    /// Evict least recently used decoded entries other than `keep` until
    /// `size` more bytes fit.
    fn make_room(&mut self, size: usize, keep: ImageId) {
        while size > self.limits.max_decoded_bytes - self.stats.decoded_bytes {
            let victim = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(i, e)| e.as_ref().map(|e| (i, e)))
                .filter(|(i, e)| *i != keep.0 && e.decoded.is_some())
                .min_by_key(|(_, e)| e.last_use)
                .map(|(i, _)| ImageId(i));
            match victim {
                Some(victim) => self.evict(victim),
                None => break,
            }
        }
    }

    fn entry(&self, id: ImageId) -> Result<&Entry, StoreError> {
        self.entries
            .get(id.0)
            .and_then(Option::as_ref)
            .ok_or(StoreError::Unknown)
    }

    fn entry_mut(&mut self, id: ImageId) -> Result<&mut Entry, StoreError> {
        self.entries
            .get_mut(id.0)
            .and_then(Option::as_mut)
            .ok_or(StoreError::Unknown)
    }
}

fn be16(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
    ])))
}

fn le16(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_le_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
    ])))
}

fn le24(b: &[u8], at: usize) -> Option<u32> {
    Some(
        u32::from(*b.get(at)?) | u32::from(*b.get(at + 1)?) << 8 | u32::from(*b.get(at + 2)?) << 16,
    )
}

/// Read the format and natural size from the header only.
pub fn probe(bytes: &[u8]) -> Result<ImageInfo, ImageError> {
    let format = sniff(bytes).ok_or(ImageError::Signature)?;
    let truncated = ImageError::Truncated;
    let (width, height) = match format {
        ImageFormat::Png => {
            if bytes.get(12..16) != Some(b"IHDR") {
                return Err(ImageError::Invalid);
            }
            let be32 = |at: usize| -> Option<u32> {
                Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
            };
            (be32(16).ok_or(truncated)?, be32(20).ok_or(truncated)?)
        }
        ImageFormat::Gif => (
            le16(bytes, 6).ok_or(truncated)?,
            le16(bytes, 8).ok_or(truncated)?,
        ),
        ImageFormat::WebP => {
            let kind = bytes.get(12..16).ok_or(truncated)?;
            match kind {
                b"VP8 " => (
                    le16(bytes, 26).ok_or(truncated)? & 0x3fff,
                    le16(bytes, 28).ok_or(truncated)? & 0x3fff,
                ),
                b"VP8L" => {
                    let b: [u8; 4] = bytes
                        .get(21..25)
                        .ok_or(truncated)?
                        .try_into()
                        .ok()
                        .ok_or(truncated)?;
                    let bits = u32::from_le_bytes(b);
                    ((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
                }
                b"VP8X" => (
                    le24(bytes, 24).ok_or(truncated)? + 1,
                    le24(bytes, 27).ok_or(truncated)? + 1,
                ),
                _ => return Err(ImageError::Invalid),
            }
        }
        ImageFormat::Jpeg => jpeg_size(bytes)?,
    };
    if width == 0 || height == 0 {
        return Err(ImageError::Invalid);
    }
    Ok(ImageInfo {
        format,
        width,
        height,
    })
}

/// Walk JPEG segments to the frame header, noting EXIF orientation.
fn jpeg_size(bytes: &[u8]) -> Result<(u32, u32), ImageError> {
    let mut pos = 2;
    let mut transpose = false;
    loop {
        while bytes.get(pos).ok_or(ImageError::Truncated)? == &0xff
            && bytes.get(pos + 1) == Some(&0xff)
        {
            pos += 1;
        }
        if bytes.get(pos) != Some(&0xff) {
            return Err(ImageError::Invalid);
        }
        let marker = *bytes.get(pos + 1).ok_or(ImageError::Truncated)?;
        let len = be16(bytes, pos + 2).ok_or(ImageError::Truncated)? as usize;
        if len < 2 {
            return Err(ImageError::Invalid);
        }
        let body = bytes
            .get(pos + 4..pos + 2 + len)
            .ok_or(ImageError::Truncated)?;
        match marker {
            0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                let h = be16(body, 1).ok_or(ImageError::Invalid)?;
                let w = be16(body, 3).ok_or(ImageError::Invalid)?;
                return Ok(if transpose { (h, w) } else { (w, h) });
            }
            0xe1 => {
                if let Some(o) = crate::jpeg::exif_orientation(body) {
                    transpose = o >= 5;
                }
            }
            0xd8..=0xda => return Err(ImageError::Invalid),
            _ => {}
        }
        pos += 2 + len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gif(width: u8, height: u8) -> Vec<u8> {
        // A one-color GIF: palette [red, black], LZW clear, index 0 × n, EOI.
        let mut g = b"GIF89a".to_vec();
        g.extend_from_slice(&[width, 0, height, 0, 0x80, 0, 0, 255, 0, 0, 0, 0, 0]);
        g.extend_from_slice(&[0x2c, 0, 0, 0, 0, width, 0, height, 0, 0, 2]);
        // Clear (4), then literal 0 for every pixel, then EOI (5), 3 bits
        // each; the table never grows past 8 codes because we clear often.
        let mut codes = Vec::new();
        for _ in 0..usize::from(width) * usize::from(height) {
            codes.extend_from_slice(&[4, 0]);
        }
        codes.push(5);
        let mut data = Vec::new();
        let (mut acc, mut n) = (0u32, 0);
        for c in codes {
            acc |= c << n;
            n += 3;
            while n >= 8 {
                data.push(acc as u8);
                acc >>= 8;
                n -= 8;
            }
        }
        if n > 0 {
            data.push(acc as u8);
        }
        for chunk in data.chunks(255) {
            g.push(chunk.len() as u8);
            g.extend_from_slice(chunk);
        }
        g.extend_from_slice(&[0, 0x3b]);
        g
    }

    fn limits(decoded: usize) -> StoreLimits {
        StoreLimits {
            max_entries: 4,
            max_encoded_bytes: 4096,
            max_decoded_bytes: decoded,
            decode: DecodeLimits::default(),
        }
    }

    #[test]
    fn entries_move_from_size_available_to_decoded_and_dedupe_by_key() {
        let mut store = ImageStore::new(limits(1 << 20));
        let id = store.insert("a.gif", gif(3, 2)).unwrap();
        let info = store.info(id).unwrap();
        assert_eq!(
            (info.width, info.height, info.format),
            (3, 2, ImageFormat::Gif)
        );
        assert_eq!(store.state(id), Ok(ImageState::SizeAvailable(info)));
        assert_eq!(store.insert("a.gif", gif(9, 9)), Ok(id));
        let cancel = CancellationToken::default();
        let frames = store.decode(id, &cancel).unwrap();
        assert_eq!(frames.frames[0].image.pixels[..4], [255, 0, 0, 255]);
        assert_eq!(store.state(id), Ok(ImageState::Decoded(info)));
        assert_eq!(store.stats().decoded_bytes, 24);
        assert!(Arc::ptr_eq(&frames, &store.decode(id, &cancel).unwrap()));
        assert_eq!(store.stats().decodes, 1);
    }

    #[test]
    fn the_decoded_budget_evicts_least_recently_used_entries() {
        // Each 4×4 image takes 64 bytes; the budget holds two.
        let mut store = ImageStore::new(limits(128));
        let cancel = CancellationToken::default();
        let ids: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|k| store.insert(k, gif(4, 4)).unwrap())
            .collect();
        store.decode(ids[0], &cancel).unwrap();
        store.decode(ids[1], &cancel).unwrap();
        store.decode(ids[0], &cancel).unwrap(); // a is now most recent
        store.decode(ids[2], &cancel).unwrap(); // evicts b
        assert!(matches!(
            store.state(ids[1]),
            Ok(ImageState::SizeAvailable(_))
        ));
        assert!(matches!(store.state(ids[0]), Ok(ImageState::Decoded(_))));
        assert_eq!(store.stats().decoded_bytes, 128);
        assert_eq!(store.stats().evictions, 1);
        // b decodes again on demand.
        store.decode(ids[1], &cancel).unwrap();
        assert_eq!(store.stats().decodes, 4);
        // An image larger than the whole budget is broken.
        let big = store.insert("big", gif(9, 9)).unwrap();
        assert_eq!(
            store.decode(big, &cancel),
            Err(StoreError::Broken(ImageError::Limit))
        );
        assert!(store.stats().decoded_bytes <= 128);
    }

    #[test]
    fn failures_are_sticky_and_budgets_refuse_inserts() {
        let mut store = ImageStore::new(limits(1 << 20));
        let cancel = CancellationToken::default();
        let mut bad = gif(2, 2);
        let len = bad.len();
        bad.truncate(len - 6);
        let id = store.insert("bad", bad).unwrap();
        assert!(store.info(id).is_ok());
        assert!(matches!(
            store.decode(id, &cancel),
            Err(StoreError::Broken(_))
        ));
        assert!(matches!(store.state(id), Ok(ImageState::Broken(_))));
        let junk = store.insert("junk", b"not an image".to_vec()).unwrap();
        assert_eq!(
            store.info(junk),
            Err(StoreError::Broken(ImageError::Signature))
        );
        // Cancellation does not break an entry.
        let ok = store.insert("ok", gif(2, 2)).unwrap();
        let cancelled = CancellationToken::default();
        cancelled.cancel();
        assert!(store.decode(ok, &cancelled).is_err());
        assert!(store.decode(ok, &cancel).is_ok());
        assert!(store.insert("big", vec![0; 5000]).is_err());
        store.insert("d", gif(1, 1)).unwrap();
        assert_eq!(store.insert("e", gif(1, 1)), Err(StoreError::Full));
        store.release(junk);
        assert_eq!(store.state(junk), Err(StoreError::Unknown));
        assert!(store.insert("e", gif(1, 1)).is_ok());
        assert_eq!(store.stats().entries, 4);
    }

    #[test]
    fn probing_reads_every_header() {
        assert_eq!(probe(&gif(7, 5)).map(|i| (i.width, i.height)), Ok((7, 5)));
        assert_eq!(probe(b"GIF89a\x01"), Err(ImageError::Truncated));
        assert_eq!(probe(b"\xff\xd8\xff\xd9\x00\x02"), Err(ImageError::Invalid));
        assert_eq!(probe(b"\xff\xd8\xff\xd9"), Err(ImageError::Truncated));
    }
}
