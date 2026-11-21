//! Operating-system confinement for renderer processes (M7, ISO-03/ISO-04).
//!
//! The broker decides *whether* a renderer is confined and what it may ask
//! for (ADR 0056–0058); this crate only builds the operating system's
//! restrictions:
//!
//! - [`seccomp`] generates the Linux seccomp-bpf system-call allowlist that
//!   bubblewrap installs in the renderer (ADR 0058). Safe Rust, compiled and
//!   tested on every platform.
//! - [`seatbelt`] builds the macOS renderer's Seatbelt (SBPL) profile,
//!   started through `sandbox-exec` (ADR 0063, Proposed: not yet verified
//!   on a Mac). Safe Rust, tested on every platform.
//! - `windows` (Windows only) launches a renderer in an AppContainer with no
//!   capabilities, inside a kill-on-close job that admits one process
//!   (ADR 0060). It is the only module in Rustic that contains `unsafe`
//!   code; every block states its invariants.

pub mod seatbelt;
pub mod seccomp;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{Confined, Pipes, spawn_app_container};
