//! Reviewed adapters to operating-system services (platform.native,
//! ADR 0078).
//!
//! The browser process (the broker) asks the platform's own font service
//! which file holds a family in a style: DirectWrite on Windows and Core
//! Text on macOS. The answers are only paths. The broker reads and checks
//! the files itself, within its byte budgets, and hands their bytes to
//! renderers (which open no files), so a font service never parses web
//! content and never runs in a renderer. On Linux and the BSDs the font
//! service is fontconfig, which the broker asks through `fc-match`; this
//! crate then answers `None` everywhere and the per-OS file tables of
//! `rustic_text::native` remain the fallback on every platform.
//!
//! The services are C and COM APIs, so their calls are `unsafe`. That code
//! is confined to the per-platform modules, every block states why it is
//! sound, and the crate is otherwise safe Rust.

use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod core_text;
#[cfg(windows)]
mod direct_write;

/// The longest family name sent to a font service.
pub const MAX_FAMILY_NAME: usize = 256;

/// One of the four faces a CSS generic family needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style {
    /// A bold (700) face rather than a regular (400) one.
    pub bold: bool,
    /// An italic or oblique face rather than an upright one.
    pub italic: bool,
}

impl Style {
    /// The regular face.
    pub const REGULAR: Style = Style {
        bold: false,
        italic: false,
    };
    /// The four faces, regular first, in the order hosts grant them.
    pub const ALL: [Style; 4] = [
        Style::REGULAR,
        Style {
            bold: true,
            italic: false,
        },
        Style {
            bold: false,
            italic: true,
        },
        Style {
            bold: true,
            italic: true,
        },
    ];
}

/// The platform font service this build asks, if any.
pub fn font_service() -> Option<&'static str> {
    if cfg!(windows) {
        Some("DirectWrite")
    } else if cfg!(target_os = "macos") {
        Some("Core Text")
    } else {
        None
    }
}

/// The file the platform's font service uses for `family` in `style`.
///
/// `None` when the service is absent, the family is not installed, the
/// family has no real face in that style (the service would only simulate
/// it, as synthetic bold or a slanted regular), or the face is not a local
/// file (a cloud font not downloaded yet, or a memory-only font).
pub fn font_file(family: &str, style: Style) -> Option<PathBuf> {
    if family.is_empty() || family.len() > MAX_FAMILY_NAME || family.contains('\0') {
        return None;
    }
    let path = platform_font_file(family, style)?;
    local_file(path)
}

/// The file of the platform's user-interface font (CSS `system-ui`) in
/// `style`: the message font on Windows, the system font on macOS.
pub fn system_ui_file(style: Style) -> Option<PathBuf> {
    local_file(platform_system_ui_file(style)?)
}

/// The family name of the platform's user-interface font, as the service
/// reports it.
pub fn system_ui_family() -> Option<String> {
    platform_system_ui_family().filter(|name| !name.is_empty() && name.len() <= MAX_FAMILY_NAME)
}

/// Files the font service falls back to for characters the system font
/// lacks (other scripts, symbols), in its order, at most `limit`.
/// Windows hosts use the fallback table of `rustic_text::native`, so this
/// is empty there.
pub fn fallback_files(limit: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for path in platform_fallback_files(limit) {
        if files.len() >= limit {
            break;
        }
        if let Some(path) = local_file(path) {
            if !files.contains(&path) {
                files.push(path);
            }
        }
    }
    files
}

/// Only absolute paths to existing regular files leave this crate.
fn local_file(path: PathBuf) -> Option<PathBuf> {
    (path.is_absolute() && path.is_file()).then_some(path)
}

#[cfg(windows)]
fn platform_font_file(family: &str, style: Style) -> Option<PathBuf> {
    direct_write::font_file(family, style)
}

#[cfg(target_os = "macos")]
fn platform_font_file(family: &str, style: Style) -> Option<PathBuf> {
    core_text::font_file(family, style)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_font_file(_family: &str, _style: Style) -> Option<PathBuf> {
    None
}

#[cfg(windows)]
fn platform_system_ui_family() -> Option<String> {
    direct_write::message_font_family()
}

#[cfg(target_os = "macos")]
fn platform_system_ui_family() -> Option<String> {
    core_text::system_ui_family()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_system_ui_family() -> Option<String> {
    None
}

#[cfg(windows)]
fn platform_system_ui_file(style: Style) -> Option<PathBuf> {
    direct_write::font_file(&direct_write::message_font_family()?, style)
}

#[cfg(target_os = "macos")]
fn platform_system_ui_file(style: Style) -> Option<PathBuf> {
    core_text::system_ui_file(style)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_system_ui_file(_style: Style) -> Option<PathBuf> {
    None
}

#[cfg(target_os = "macos")]
fn platform_fallback_files(limit: usize) -> Vec<PathBuf> {
    core_text::cascade_files(limit)
}

#[cfg(not(target_os = "macos"))]
fn platform_fallback_files(_limit: usize) -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_family_names_never_reach_the_service() {
        assert_eq!(font_file("", Style::REGULAR), None);
        assert_eq!(font_file("Arial\0Bold", Style::REGULAR), None);
        assert_eq!(
            font_file(&"a".repeat(MAX_FAMILY_NAME + 1), Style::REGULAR),
            None
        );
    }

    #[test]
    fn relative_and_missing_paths_are_dropped() {
        assert_eq!(local_file(PathBuf::from("fonts/arial.ttf")), None);
        assert_eq!(
            local_file(std::env::temp_dir().join("rustic-no-such-font.ttf")),
            None
        );
    }

    #[test]
    fn the_service_is_named_only_where_one_is_asked() {
        let expected = if cfg!(windows) {
            Some("DirectWrite")
        } else if cfg!(target_os = "macos") {
            Some("Core Text")
        } else {
            None
        };
        assert_eq!(font_service(), expected);
        assert_eq!(Style::ALL[0], Style::REGULAR);
    }
}
