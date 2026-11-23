//! `rustic-renderer`: the commands of `rustic` without the desktop window
//! stack (TD-183, ADR 0074).
//!
//! On macOS the broker starts renderers and the network process from this
//! executable when it sits beside `rustic`: `rustic` links AppKit for
//! `rustic view`, whose initializers run before `main` and so before the
//! child confines itself with its Seatbelt profile. This binary links
//! none of the window system, so nothing runs before the profile but the
//! system libraries every process loads. Build it on its own
//! (`cargo build -p rustic-renderer`): built together with the rest of
//! the workspace, Cargo unifies features and links the window stack in.

fn main() -> std::process::ExitCode {
    rustic_cli::main()
}
