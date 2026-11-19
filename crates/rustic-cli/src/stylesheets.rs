//! Local-only stylesheet fetching for `dump-style`.
//!
//! The command line grants one HTML file. Its `<link rel=stylesheet>` and
//! `@import` URLs may name other files, but only `file:` URLs inside the
//! directory that contains the granted file (after resolving symbolic links)
//! are read. Everything else, including network URLs, `data:` URLs, other
//! hosts and paths that leave the directory with `..` or a symbolic link, is
//! blocked. Each file is read in bounded chunks within the byte budget the
//! style loader offers, so a file that grows while being read still cannot
//! exceed it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf, Prefix};
use std::rc::Rc;
use std::sync::Arc;

use rustic_base::CancellationToken;
use rustic_encoding::Encoding;
use rustic_style::{FetchError, StylesheetFetcher};
use rustic_url::{Url, UrlPath};

/// The `file:` URL of an absolute, normalized path (as from
/// [`fs::canonicalize`]), or `None` for paths that have no portable `file:`
/// form (UNC and device paths, or names that are not Unicode).
pub fn file_url(path: &Path) -> Option<Url> {
    let mut url = String::from("file://");
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
                    url.push('/');
                    url.push(char::from(drive));
                    url.push(':');
                }
                _ => return None,
            },
            Component::RootDir => {}
            Component::Normal(name) => {
                url.push('/');
                percent_encode(name.to_str()?, &mut url);
            }
            Component::CurDir | Component::ParentDir => return None,
        }
    }
    if url.len() == "file://".len() {
        url.push('/');
    }
    Url::parse(&url, None).ok()
}

fn percent_encode(name: &str, out: &mut String) {
    for &byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
}

fn hex(byte: u8) -> Option<u8> {
    char::from(byte)
        .to_digit(16)
        .and_then(|d| u8::try_from(d).ok())
}

fn percent_decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let (Some(hi), Some(lo)) = (
                bytes.get(i + 1).copied().and_then(hex),
                bytes.get(i + 2).copied().and_then(hex),
            ) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// The local path a `file:` URL names, or `None` if it names no local path
/// (another scheme, a host, an opaque path, or a segment that would change
/// the directory structure once decoded).
pub fn url_path(url: &Url) -> Option<PathBuf> {
    if url.scheme() != "file" || url.host().is_some_and(|h| !h.serialize().is_empty()) {
        return None;
    }
    let UrlPath::Segments(segments) = url.path() else {
        return None;
    };
    let mut decoded = Vec::with_capacity(segments.len());
    for segment in segments {
        let segment = percent_decode(segment)?;
        if segment.contains(['/', '\\', '\0']) || segment == "." || segment == ".." {
            return None;
        }
        decoded.push(segment);
    }
    let mut parts = decoded.into_iter().peekable();
    let mut path = root_for(parts.peek().map(String::as_str))?;
    if cfg!(windows) {
        parts.next();
    }
    for part in parts {
        if !part.is_empty() {
            path.push(part);
        }
    }
    Some(path)
}

/// The filesystem root a URL path starts from: a drive on Windows, `/`
/// elsewhere.
fn root_for(first: Option<&str>) -> Option<PathBuf> {
    if cfg!(windows) {
        let first = first?.as_bytes();
        (first.len() == 2 && first[0].is_ascii_alphabetic() && first[1] == b':')
            .then(|| PathBuf::from(format!("{}:\\", char::from(first[0]))))
    } else {
        Some(PathBuf::from("/"))
    }
}

/// Maximum URL-key size retained in a resource snapshot. Along with the
/// entry count cap, this also bounds bookkeeping for rejected resources.
const MAX_APPLIED_URL_BYTES: usize = 4096;

/// Bytes which have been applied to a scripted local document.
///
/// A layout read may rebuild its cascade and box tree after a DOM mutation,
/// but it must not pick up unrelated changes to resource files. The first
/// fetch freezes each absolute URL's bounded bytes or failure. A host-side
/// completion path may later replace this snapshot, after staging the new
/// resource, and publish the corresponding rendering-input revision.
#[derive(Clone, Debug)]
pub(crate) struct AppliedLocalFiles {
    state: Rc<RefCell<AppliedFileState>>,
}

#[derive(Debug)]
struct AppliedFileState {
    entries: HashMap<String, Result<Arc<[u8]>, FetchError>>,
    bytes: usize,
    max_bytes: usize,
    max_entries: usize,
}

impl AppliedLocalFiles {
    pub(crate) fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            state: Rc::new(RefCell::new(AppliedFileState {
                entries: HashMap::new(),
                bytes: 0,
                max_bytes,
                max_entries,
            })),
        }
    }

    /// Resolve once without exposing a partly fetched resource. The caller
    /// checks the URL security policy before returning bytes from `read`.
    /// Later reads still respect their own per-request size limits.
    pub(crate) fn read(
        &self,
        url: &Url,
        max_bytes: usize,
        fetch: impl FnOnce() -> Result<Vec<u8>, FetchError>,
    ) -> Result<Arc<[u8]>, FetchError> {
        let key = url.serialize();
        if key.len() > MAX_APPLIED_URL_BYTES {
            return Err(FetchError::TooLarge);
        }
        {
            let state = self.state.borrow();
            if let Some(result) = state.entries.get(&key) {
                return result.clone().and_then(|bytes| {
                    if bytes.len() > max_bytes {
                        Err(FetchError::TooLarge)
                    } else {
                        Ok(bytes)
                    }
                });
            }
            // A missing entry still counts against the resource cap. Reject
            // before performing I/O when this snapshot cannot retain it.
            if state.entries.len() >= state.max_entries {
                return Err(FetchError::TooLarge);
            }
        }
        let result = fetch().and_then(|bytes| {
            if bytes.len() > max_bytes {
                Err(FetchError::TooLarge)
            } else {
                Ok(Arc::<[u8]>::from(bytes))
            }
        });
        let mut state = self.state.borrow_mut();
        if state.entries.len() >= state.max_entries {
            return Err(FetchError::TooLarge);
        }
        let result = result.and_then(|bytes| {
            let remaining = state.max_bytes.saturating_sub(state.bytes);
            if bytes.len() > remaining {
                Err(FetchError::TooLarge)
            } else {
                state.bytes += bytes.len();
                Ok(bytes)
            }
        });
        // Cancellation must not poison a retryable resource snapshot.
        if result != Err(FetchError::Cancelled) {
            state.entries.insert(key, result.clone());
        }
        result
    }
}

/// Reads stylesheets from files under one directory.
#[derive(Debug)]
pub struct LocalStylesheetFetcher {
    /// The canonical directory every stylesheet must be inside.
    root: PathBuf,
    /// `root` as it appears after a round trip through a `file:` URL, for a
    /// lexical check before the file system is touched.
    url_root: PathBuf,
    chunk_bytes: usize,
    cancellation: CancellationToken,
    environment_encoding: Encoding,
    applied: Option<AppliedLocalFiles>,
}

impl LocalStylesheetFetcher {
    /// A fetcher confined to `root`, which must be a canonical directory.
    /// Returns `None` when `root` has no `file:` URL form.
    pub fn new(root: PathBuf, chunk_bytes: usize, cancellation: CancellationToken) -> Option<Self> {
        let url_root = url_path(&file_url(&root)?)?;
        Some(Self {
            root,
            url_root,
            chunk_bytes: chunk_bytes.max(1),
            cancellation,
            environment_encoding: Encoding::Utf8,
            applied: None,
        })
    }

    pub(crate) fn with_environment_encoding(mut self, encoding: Encoding) -> Self {
        self.environment_encoding = encoding;
        self
    }

    pub(crate) fn with_applied_files(mut self, applied: AppliedLocalFiles) -> Self {
        self.applied = Some(applied);
        self
    }

    fn read(&self, path: &Path, max_bytes: usize) -> Result<Vec<u8>, FetchError> {
        let io_error = |error: io::Error| match error.kind() {
            io::ErrorKind::NotFound => FetchError::NotFound,
            _ => FetchError::Failed,
        };
        // Lexical containment first, then again after symbolic links resolve.
        if !path.starts_with(&self.url_root) {
            return Err(FetchError::Blocked);
        }
        let canonical = fs::canonicalize(path).map_err(io_error)?;
        if !canonical.starts_with(&self.root) {
            return Err(FetchError::Blocked);
        }
        let check = |metadata: fs::Metadata| {
            if !metadata.is_file() {
                Err(FetchError::Failed)
            } else if metadata.len() > max_bytes as u64 {
                Err(FetchError::TooLarge)
            } else {
                Ok(())
            }
        };
        check(fs::metadata(&canonical).map_err(io_error)?)?;
        let mut file = File::open(&canonical).map_err(io_error)?;
        check(file.metadata().map_err(io_error)?)?;
        let mut buffer = vec![0; self.chunk_bytes.min(max_bytes.max(1))];
        let mut bytes = Vec::new();
        loop {
            if self.cancellation.check().is_err() {
                return Err(FetchError::Cancelled);
            }
            let read = match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(FetchError::Failed),
            };
            if read > max_bytes - bytes.len() {
                return Err(FetchError::TooLarge);
            }
            bytes.extend_from_slice(&buffer[..read]);
        }
        Ok(bytes)
    }

    pub(crate) fn fetch_bytes(&self, url: &Url, max_bytes: usize) -> Result<Vec<u8>, FetchError> {
        if self.cancellation.check().is_err() {
            return Err(FetchError::Cancelled);
        }
        // No policy bypass on a cached hit: a URL with a disallowed scheme,
        // host or encoded path is rejected before looking in the snapshot.
        let path = url_path(url).ok_or(FetchError::Blocked)?;
        if let Some(applied) = &self.applied {
            applied.read(url, max_bytes, || self.read(&path, max_bytes)).map(|b| b.to_vec())
        } else {
            self.read(&path, max_bytes)
        }
    }
}

impl StylesheetFetcher for LocalStylesheetFetcher {
    /// Read a stylesheet and decode it with CSS Syntax §3.2: a byte order
    /// mark, then a leading `@charset` rule, then the referring document's
    /// encoding select the decoder, and errors become U+FFFD. Local files
    /// have no transport `charset`.
    fn fetch(&mut self, url: &Url, max_bytes: usize) -> Result<String, FetchError> {
        let bytes = self.fetch_bytes(url, max_bytes)?;
        let text = rustic_css::decode_stylesheet(&bytes, None, Some(self.environment_encoding));
        // Replacement characters can make the text longer than the bytes.
        if text.len() > max_bytes {
            return Err(FetchError::TooLarge);
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applied_snapshot_pins_bytes_failures_and_enforces_each_budget() {
        let snapshot = AppliedLocalFiles::new(3, 5);
        let first = Url::parse("file:///page/first.css", None).unwrap();
        let missing = Url::parse("file:///page/missing.css", None).unwrap();
        let other = Url::parse("file:///page/other.css", None).unwrap();
        let overflow = Url::parse("file:///page/overflow.css", None).unwrap();

        assert_eq!(
            snapshot.read(&first, 100, || Ok(b"body".to_vec())).unwrap().as_ref(),
            b"body",
        );
        // A hit never reads the changed underlying file and still respects
        // the per-request limit offered by the stylesheet/image loader.
        assert_eq!(
            snapshot.read(&first, 100, || panic!("refetched applied bytes"))
                .unwrap().as_ref(),
            b"body",
        );
        assert_eq!(
            snapshot.read(&first, 3, || panic!("read past cache")).unwrap_err(),
            FetchError::TooLarge,
        );
        assert_eq!(
            snapshot.read(&missing, 100, || Err(FetchError::NotFound)),
            Err(FetchError::NotFound),
        );
        assert_eq!(
            snapshot.read(&missing, 100, || panic!("refetched missing file")),
            Err(FetchError::NotFound),
        );
        // Only one byte remains in the aggregate applied-byte budget.
        assert_eq!(
            snapshot.read(&other, 100, || Ok(b"extra".to_vec())),
            Err(FetchError::TooLarge),
        );
        // Failed applied resources consume an entry but not decoded bytes.
        assert_eq!(
            snapshot.read(&overflow, 100, || panic!("exceeded resource count")),
            Err(FetchError::TooLarge),
        );
        assert_eq!(
            snapshot.read(&first, 100, || panic!("evicted stable bytes"))
                .unwrap().as_ref(),
            b"body",
        );
    }

    #[test]
    fn cancellation_does_not_poison_an_applied_resource() {
        let snapshot = AppliedLocalFiles::new(1, 8);
        let url = Url::parse("file:///page/image.png", None).unwrap();
        assert_eq!(
            snapshot.read(&url, 8, || Err(FetchError::Cancelled)),
            Err(FetchError::Cancelled),
        );
        assert_eq!(
            snapshot.read(&url, 8, || Ok(b"updated".to_vec()))
                .unwrap().as_ref(),
            b"updated",
        );
    }

    #[test]
    fn file_urls_round_trip_paths() {
        let root = if cfg!(windows) {
            PathBuf::from(r"C:\Pages\a b\100%\é.html")
        } else {
            PathBuf::from("/pages/a b/100%/é#?.html")
        };
        let url = file_url(&root).unwrap();
        assert!(url.serialize().starts_with("file:///"));
        assert_eq!(url_path(&url).unwrap(), root);
    }

    #[test]
    fn non_local_urls_name_no_path() {
        for input in [
            "https://example.test/a.css",
            "data:text/css,p{}",
            "file://server/share/a.css",
            "file:///dir/a%2Fb.css",
            "file:///dir/a%5Cb.css",
            "file:///dir/a%00.css",
            "file:///dir/%FF.css",
        ] {
            let url = Url::parse(input, None).unwrap();
            assert_eq!(url_path(&url), None, "{input}");
        }
    }

    #[test]
    fn dot_segments_are_resolved_by_the_url_parser_first() {
        let base = Url::parse("file:///root/dir/page.html", None).unwrap();
        let url = Url::parse("../%2e%2E/x.css", Some(&base)).unwrap();
        assert_eq!(url.serialize(), "file:///x.css");
    }
}
