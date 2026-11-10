//! Explicit host-granted, bounded local font discovery. No implicit I/O.
use crate::{Face, FontDatabase, FontLimits};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};
/// Limits for one atomic discovery operation.
#[derive(Clone, Copy, Debug)]
pub struct DiscoveryLimits {
    /// Files and directories visited.
    pub max_entries: usize,
    /// Directory nesting below each granted root.
    pub max_depth: usize,
    /// Faces added in one operation.
    pub max_faces: usize,
    /// Aggregate input and separately decoded font bytes.
    pub max_bytes: usize,
}
impl Default for DiscoveryLimits {
    fn default() -> Self {
        Self {
            max_entries: 16_384,
            max_depth: 16,
            max_faces: 512,
            max_bytes: 128 * 1024 * 1024,
        }
    }
}
/// Discovery accounting. Invalid font files are rejected individually.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscoveryReport {
    /// Visited filesystem entries.
    pub entries: usize,
    /// Successfully loaded faces.
    pub loaded: usize,
    /// Invalid font files.
    pub rejected: usize,
    /// Bytes read from recognized font files.
    pub bytes: usize,
}
/// I/O failure or budget exhaustion. Neither publishes partial faces.
#[derive(Debug)]
pub enum DiscoveryError {
    /// Host filesystem error.
    Io(std::io::Error),
    /// Named budget was exhausted.
    Limit(&'static str),
}
impl From<std::io::Error> for DiscoveryError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "font discovery: {e}"),
            Self::Limit(n) => write!(f, "font discovery limit: {n}"),
        }
    }
}
impl std::error::Error for DiscoveryError {}
impl FontDatabase {
    /// Discover only these host-granted paths, deterministically and without
    /// following symlinks. Publish all accepted faces together on success.
    pub fn discover(
        &mut self,
        roots: &[PathBuf],
        limits: &DiscoveryLimits,
        font_limits: &FontLimits,
    ) -> Result<DiscoveryReport, DiscoveryError> {
        if roots.len() > limits.max_entries {
            return Err(DiscoveryError::Limit("entries"));
        }
        let mut roots = roots.to_vec();
        roots.sort();
        let mut todo: Vec<_> = roots.into_iter().rev().map(|p| (p, 0usize)).collect();
        let mut report = DiscoveryReport::default();
        let mut pending = Vec::new();
        let mut decoded = 0usize;
        while let Some((path, depth)) = todo.pop() {
            report.entries += 1;
            if report.entries > limits.max_entries {
                return Err(DiscoveryError::Limit("entries"));
            }
            let meta = fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if depth >= limits.max_depth {
                    return Err(DiscoveryError::Limit("depth"));
                }
                let mut children = Vec::new();
                for entry in fs::read_dir(&path)? {
                    if report.entries + todo.len() + children.len() >= limits.max_entries {
                        return Err(DiscoveryError::Limit("entries"));
                    }
                    children.push(entry?.path());
                }
                children.sort();
                todo.extend(children.into_iter().rev().map(|p| (p, depth + 1)));
                continue;
            }
            if !meta.is_file() || !is_font(&path) {
                continue;
            }
            let remaining = limits
                .max_bytes
                .saturating_sub(report.bytes)
                .min(font_limits.max_font_bytes);
            if meta.len() > remaining as u64 {
                return Err(DiscoveryError::Limit("bytes"));
            }
            let mut bytes = Vec::new();
            fs::File::open(&path)?
                .take(remaining as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > remaining {
                return Err(DiscoveryError::Limit("bytes"));
            }
            report.bytes += bytes.len();
            match Face::parse(bytes, font_limits) {
                Ok(face) => {
                    if pending.len() >= limits.max_faces {
                        return Err(DiscoveryError::Limit("faces"));
                    }
                    decoded = decoded
                        .checked_add(face.byte_len())
                        .filter(|&n| n <= limits.max_bytes)
                        .ok_or(DiscoveryError::Limit("decoded bytes"))?;
                    pending.push(face);
                }
                Err(_) => report.rejected += 1,
            }
        }
        report.loaded = pending.len();
        self.append_faces(pending);
        Ok(report)
    }
}
fn is_font(p: &Path) -> bool {
    p.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        ["ttf", "otf", "woff", "woff2"]
            .iter()
            .any(|e| s.eq_ignore_ascii_case(e))
    })
}
/// Candidate native font directories. Returning paths grants no filesystem
/// access; the host chooses which to pass to [`FontDatabase::discover`].
pub fn system_font_roots() -> Vec<PathBuf> {
    let paths: &[&str] = if cfg!(target_os = "windows") {
        &["C:\\Windows\\Fonts"]
    } else if cfg!(target_os = "macos") {
        &["/System/Library/Fonts", "/Library/Fonts"]
    } else if cfg!(target_os = "android") {
        &["/system/fonts"]
    } else if cfg!(target_os = "ios") {
        &["/System/Library/Fonts"]
    } else {
        &["/usr/share/fonts", "/usr/local/share/fonts"]
    };
    paths.iter().map(PathBuf::from).collect()
}
