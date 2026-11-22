//! Applying a Seatbelt profile to the calling process (macOS only,
//! ADR 0063).
//!
//! The renderer and the network process confine themselves first thing in
//! `main`, before they read a byte from the broker, with `sandbox_init`.
//! Starting them under `sandbox-exec` instead made dyld itself run
//! confined, and on macOS 15 it aborted while locating the OS cryptex that
//! holds the shared cache (2026-11-22, native CI): the libraries a process
//! needs are already mapped by the time `main` runs, so the profile does
//! not have to cover process start-up.

#![allow(
    unsafe_code,
    reason = "ADR 0063: sandbox_init has no safe std equivalent"
)]

use std::ffi::{CStr, CString, c_char, c_int};
use std::ptr::null_mut;

unsafe extern "C" {
    // libsandbox, part of libSystem: apply `profile` (SBPL source when
    // `flags` is 0) to the calling process, irrevocably.
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> c_int;
    fn sandbox_free_error(errorbuf: *mut c_char);
}

/// Apply the SBPL `profile` to this process. It cannot be undone; every
/// later file, network, process and Mach operation is checked against it.
pub fn enter(profile: &str) -> Result<(), String> {
    let profile = CString::new(profile).map_err(|_| "the sandbox profile contains NUL")?;
    let mut error: *mut c_char = null_mut();
    // SAFETY: `profile` is a NUL-terminated string that outlives the call;
    // `error` is a valid out pointer that libsandbox sets only on failure.
    let result = unsafe { sandbox_init(profile.as_ptr(), 0, &mut error) };
    if result == 0 {
        return Ok(());
    }
    let message = if error.is_null() {
        format!("sandbox_init failed ({result})")
    } else {
        // SAFETY: on failure `error` is a NUL-terminated string allocated
        // by libsandbox, valid until `sandbox_free_error`.
        let text = unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: `error` came from sandbox_init and is freed once.
        unsafe { sandbox_free_error(error) };
        format!("sandbox_init failed: {text}")
    };
    Err(message)
}
