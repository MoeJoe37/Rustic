//! Rustic's Android host glue (ISO-08, ADR 0059).
//!
//! The app's main process is the broker; each renderer is a bound
//! `RendererService` declared `android:isolatedProcess="true"`, so Android
//! runs it under its own UID with no permissions, no network, no access to
//! the app's files and the SELinux `isolated_app` domain. The broker hands
//! the service one end of a socket pair over Binder, and `rustic-ipc` runs
//! over that stream unchanged.
//!
//! The JNI surface takes and returns integers only (file descriptors,
//! flags, statuses), so no JNI environment calls are needed. The Java side
//! is `android/` in the repository.
//!
//! - [`probe`] tries what a compromised renderer would (read and list the
//!   app's files, connect a socket, start a shell); the isolated service
//!   must be denied all of it (the same probes as the desktop sandboxes).
//! - The broker side runs one document command against a hosted renderer
//!   (`rustic_cli::run_hosted`) and writes its output to a pipe.

use std::io::Read;
use std::net::{SocketAddr, TcpStream};
use std::process::Command;
use std::time::Duration;

#[cfg(unix)]
mod jni;

/// The app's package, whose private files the probes try to reach.
pub const PACKAGE: &str = "dev.rustic.android";

/// [`probe`] bit: a private file of the app could be read.
pub const READ_APP_FILE: i32 = 1;
/// [`probe`] bit: the app's files directory could be listed.
pub const LIST_APP_FILES: i32 = 2;
/// [`probe`] bit: a TCP connection to loopback succeeded.
pub const CONNECT_LOOPBACK: i32 = 4;
/// [`probe`] bit: a shell could be started.
pub const SPAWN_SHELL: i32 = 8;

/// The app's files directory.
pub fn files_dir() -> String {
    format!("/data/data/{PACKAGE}/files")
}

/// Try what a compromised renderer would and return the bits of what
/// succeeded (0 when everything is denied). `port` is a loopback port the
/// broker listens on.
pub fn probe(port: u16) -> i32 {
    let dir = files_dir();
    let mut allowed = 0;
    let mut secret = String::new();
    if std::fs::File::open(format!("{dir}/secret.txt"))
        .and_then(|mut file| file.read_to_string(&mut secret))
        .is_ok()
    {
        allowed |= READ_APP_FILE;
    }
    if std::fs::read_dir(&dir).is_ok() {
        allowed |= LIST_APP_FILES;
    }
    let loopback = SocketAddr::from(([127, 0, 0, 1], port));
    if TcpStream::connect_timeout(&loopback, Duration::from_secs(2)).is_ok() {
        allowed |= CONNECT_LOOPBACK;
    }
    if Command::new("/system/bin/sh")
        .args(["-c", "exit 0"])
        .status()
        .is_ok_and(|status| status.success())
    {
        allowed |= SPAWN_SHELL;
    }
    allowed
}
