//! The Core Text font service (macOS): a font made by name, or the system
//! UI font, reports the URL of its file, as WebKit and Chromium's Core
//! Text font managers use it. `objc2-core-text` 0.3.2 provides the
//! bindings (ADR 0078).
#![allow(
    unsafe_code,
    reason = "Core Text is a C API: its bindings mark calls unsafe (ADR 0078)"
)]

use std::path::PathBuf;

use objc2_core_foundation::{CFArray, CFRetained, CFString, CFURL};
use objc2_core_text::{CTFont, CTFontDescriptor, CTFontSymbolicTraits, CTFontUIFontType};

use crate::Style;

/// Fonts are made at this size; the file does not depend on it.
const SIZE: f64 = 12.0;

fn traits(style: Style) -> CTFontSymbolicTraits {
    let mut traits = CTFontSymbolicTraits::empty();
    if style.bold {
        traits |= CTFontSymbolicTraits::BoldTrait;
    }
    if style.italic {
        traits |= CTFontSymbolicTraits::ItalicTrait;
    }
    traits
}

/// `font` in `style`: itself for the regular face, else the family's face
/// with those traits, which Core Text returns only when one exists.
fn styled(font: CFRetained<CTFont>, style: Style) -> Option<CFRetained<CTFont>> {
    if style == Style::REGULAR {
        return Some(font);
    }
    let wanted = traits(style);
    let mask = CTFontSymbolicTraits::BoldTrait | CTFontSymbolicTraits::ItalicTrait;
    // SAFETY: a null matrix means the identity; the font is live.
    let face = unsafe { font.copy_with_symbolic_traits(0.0, std::ptr::null(), wanted, mask) }?;
    // SAFETY: a method without arguments on a live font.
    let actual = unsafe { face.symbolic_traits() };
    actual.contains(wanted).then_some(face)
}

fn family_name(font: &CTFont) -> String {
    // SAFETY: a method without arguments on a live font.
    unsafe { font.family_name() }.to_string()
}

fn file_of(font: &CTFont) -> Option<PathBuf> {
    // SAFETY: the attribute key is Core Text's own constant.
    let key = unsafe { objc2_core_text::kCTFontURLAttribute };
    // SAFETY: `key` is a valid font attribute name.
    let url = unsafe { font.attribute(key) }?;
    url.downcast::<CFURL>().ok()?.to_file_path()
}

/// The file of `family` in `style`. Core Text substitutes another family
/// for a name it does not know, so the answer must name the family asked.
pub(crate) fn font_file(family: &str, style: Style) -> Option<PathBuf> {
    let name = CFString::from_str(family);
    // SAFETY: a null matrix means the identity; `name` lives for the call.
    let font = unsafe { CTFont::with_name(&name, SIZE, std::ptr::null()) };
    if !family_name(&font).eq_ignore_ascii_case(family) {
        return None;
    }
    let face = styled(font, style)?;
    file_of(&face)
}

fn system_font() -> Option<CFRetained<CTFont>> {
    // SAFETY: plain arguments; no language means the user's language.
    unsafe { CTFont::new_ui_font_for_language(CTFontUIFontType::System, SIZE, None) }
}

/// The family of the system UI font.
pub(crate) fn system_ui_family() -> Option<String> {
    let font = system_font()?;
    Some(family_name(&font))
}

/// The file of the system UI font in `style`.
pub(crate) fn system_ui_file(style: Style) -> Option<PathBuf> {
    let face = styled(system_font()?, style)?;
    file_of(&face)
}

/// The files of the system font's default cascade list (the fonts Core
/// Text falls back to for other scripts), at most `limit`.
pub(crate) fn cascade_files(limit: usize) -> Vec<PathBuf> {
    let Some(font) = system_font() else {
        return Vec::new();
    };
    // SAFETY: no language list means the user's preferred languages.
    let Some(list) = (unsafe { font.default_cascade_list_for_languages(None) }) else {
        return Vec::new();
    };
    // SAFETY: Core Text documents the cascade list as an array of
    // CTFontDescriptor references.
    let list: CFRetained<CFArray<CTFontDescriptor>> = unsafe { CFRetained::cast_unchecked(list) };
    let mut files = Vec::new();
    for index in 0..list.len() {
        if files.len() >= limit {
            break;
        }
        let Some(descriptor) = list.get(index) else {
            break;
        };
        // SAFETY: the attribute key is Core Text's own constant.
        let key = unsafe { objc2_core_text::kCTFontURLAttribute };
        // SAFETY: `key` is a valid descriptor attribute name.
        let Some(url) = (unsafe { descriptor.attribute(key) }) else {
            continue;
        };
        if let Some(path) = url
            .downcast::<CFURL>()
            .ok()
            .and_then(|url| url.to_file_path())
        {
            if !files.contains(&path) {
                files.push(path);
            }
        }
    }
    files
}
