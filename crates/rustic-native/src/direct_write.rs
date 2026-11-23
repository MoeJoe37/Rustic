//! The DirectWrite font service (Windows): the system font collection
//! names the file of a family's face, as Chromium's DirectWrite font
//! manager does. `windows` 0.62.2 provides the COM bindings (ADR 0078).
#![allow(
    unsafe_code,
    reason = "DirectWrite is a COM API: every call goes through a vtable (ADR 0078)"
)]

use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};

use windows::{
    Win32::{
        Graphics::DirectWrite::{
            DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_SIMULATIONS_NONE, DWRITE_FONT_STRETCH_NORMAL,
            DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD,
            DWRITE_FONT_WEIGHT_NORMAL, DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection,
            IDWriteFontFile, IDWriteLocalFontFileLoader,
        },
        UI::WindowsAndMessaging::{
            NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
            SystemParametersInfoW,
        },
    },
    core::{BOOL, HSTRING, Interface},
};

use crate::Style;

/// The longest file path accepted from the service, in UTF-16 units.
const MAX_PATH_UNITS: u32 = 32 * 1024;

fn system_collection() -> Option<IDWriteFontCollection> {
    // SAFETY: DWriteCreateFactory takes no pointers from us; the
    // binding checks the HRESULT and returns an owned interface.
    let factory: IDWriteFactory =
        unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }.ok()?;
    let mut collection = None;
    // SAFETY: `collection` is a valid out slot for one interface pointer,
    // which the binding takes ownership of.
    unsafe { factory.GetSystemFontCollection(&mut collection, false) }.ok()?;
    collection
}

/// The file of `family` in `style` in the system font collection.
pub(crate) fn font_file(family: &str, style: Style) -> Option<PathBuf> {
    let collection = system_collection()?;
    let name = HSTRING::from(family);
    let mut index = 0u32;
    let mut exists = BOOL(0);
    // SAFETY: `name` is a NUL-terminated wide string that outlives the
    // call; `index` and `exists` are valid out pointers.
    unsafe { collection.FindFamilyName(&name, &mut index, &mut exists) }.ok()?;
    if !exists.as_bool() {
        return None;
    }
    // SAFETY: `index` was just returned by FindFamilyName for this
    // collection.
    let family = unsafe { collection.GetFontFamily(index) }.ok()?;
    let weight = if style.bold {
        DWRITE_FONT_WEIGHT_BOLD
    } else {
        DWRITE_FONT_WEIGHT_NORMAL
    };
    let slant = if style.italic {
        DWRITE_FONT_STYLE_ITALIC
    } else {
        DWRITE_FONT_STYLE_NORMAL
    };
    // SAFETY: plain enum arguments; the binding returns an owned font.
    let font =
        unsafe { family.GetFirstMatchingFont(weight, DWRITE_FONT_STRETCH_NORMAL, slant) }.ok()?;
    // SAFETY: a method without arguments on a live font.
    let simulations = unsafe { font.GetSimulations() };
    if simulations != DWRITE_FONT_SIMULATIONS_NONE {
        // Only synthetic bold or oblique: the family has no such face.
        return None;
    }
    // SAFETY: a method without arguments on a live font.
    let face = unsafe { font.CreateFontFace() }.ok()?;
    let mut count = 0u32;
    // SAFETY: with no array, GetFiles only writes the file count.
    unsafe { face.GetFiles(&mut count, None) }.ok()?;
    if count != 1 {
        return None;
    }
    let mut files: [Option<IDWriteFontFile>; 1] = [None];
    // SAFETY: `files` holds `count` (one) interface slots, which the
    // binding fills with owned references.
    unsafe { face.GetFiles(&mut count, Some(files.as_mut_ptr())) }.ok()?;
    local_path(files[0].take()?)
}

/// The path of a font file the local loader serves; `None` for any other
/// loader (memory fonts, downloadable fonts).
fn local_path(file: IDWriteFontFile) -> Option<PathBuf> {
    let mut key: *mut core::ffi::c_void = core::ptr::null_mut();
    let mut key_size = 0u32;
    // SAFETY: both are valid out pointers; the key stays owned by `file`,
    // which outlives every use of it below.
    unsafe { file.GetReferenceKey(&mut key, &mut key_size) }.ok()?;
    if key.is_null() {
        return None;
    }
    // SAFETY: a method without arguments on a live font file.
    let loader = unsafe { file.GetLoader() }.ok()?;
    let local: IDWriteLocalFontFileLoader = loader.cast().ok()?;
    // SAFETY: `key` and `key_size` came from this file's own loader.
    let units = unsafe { local.GetFilePathLengthFromKey(key, key_size) }.ok()?;
    if units == 0 || units > MAX_PATH_UNITS {
        return None;
    }
    let mut path = vec![0u16; units as usize + 1];
    // SAFETY: `path` has room for the reported length plus the NUL, and
    // the binding passes its length to the service.
    unsafe { local.GetFilePathFromKey(key, key_size, &mut path) }.ok()?;
    let end = path.iter().position(|&unit| unit == 0)?;
    Some(PathBuf::from(OsString::from_wide(&path[..end])))
}

/// The family of the message font (Chromium's `system-ui` on Windows).
pub(crate) fn message_font_family() -> Option<String> {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: u32::try_from(std::mem::size_of::<NONCLIENTMETRICSW>()).ok()?,
        ..Default::default()
    };
    // SAFETY: `metrics` is a NONCLIENTMETRICSW whose cbSize is its own
    // size, as SPI_GETNONCLIENTMETRICS requires, and lives for the call.
    unsafe {
        SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            Some((&raw mut metrics).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .ok()?;
    let face = &metrics.lfMessageFont.lfFaceName;
    let end = face
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(face.len());
    String::from_utf16(&face[..end])
        .ok()
        .filter(|name| !name.is_empty())
}
