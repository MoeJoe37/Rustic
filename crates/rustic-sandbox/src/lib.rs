//! Operating-system confinement for renderer processes (M7, ISO-03/ISO-04).
//!
//! The broker decides *whether* a renderer is confined and what it may ask
//! for (ADR 0056–0058); this crate only builds the operating system's
//! restrictions:
//!
//! - [`seccomp`] generates the Linux seccomp-bpf system-call allowlist that
//!   bubblewrap installs in the renderer (ADR 0058). Safe Rust, compiled and
//!   tested on every platform.
//! - [`seatbelt`] builds the macOS renderer and network process Seatbelt
//!   (SBPL) profiles (ADR 0063). Safe Rust, tested on every
//!   platform. On macOS, `enter` applies one to the calling process
//!   through `sandbox_init`, which needs `unsafe` FFI.
//! - `windows` (Windows only) launches a renderer in an AppContainer with no
//!   capabilities, or the network process in one with only outbound
//!   Internet and certificate-verification capabilities, inside a kill-on-close job that admits one process
//!   (ADR 0060). It and the macOS `enter` are the only modules in Rustic
//!   that contain `unsafe` code; every block states its invariants.

pub mod seatbelt;
pub mod seccomp;

#[cfg(target_os = "macos")]
mod seatbelt_enter;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use seatbelt_enter::enter;

#[cfg(windows)]
pub use windows::{Confined, Container, Pipes, spawn_app_container};
