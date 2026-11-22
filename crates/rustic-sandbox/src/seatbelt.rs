//! The macOS renderer and network process sandbox profiles (ADR 0063, **Proposed**: written and
//! unit-tested on Linux, not yet verified on a Mac).
//!
//! macOS confines a process with a Seatbelt profile in the Sandbox Profile
//! Language (SBPL), the mechanism Chromium's and Firefox's macOS sandboxes
//! use. The broker starts the renderer as
//! `/usr/bin/sandbox-exec -p PROFILE EXE __renderer`: `sandbox-exec`
//! applies the profile to itself and then executes the renderer, which
//! inherits it and can never drop it. Starting through `sandbox-exec`
//! needs no `unsafe` code (calling `sandbox_init` would need FFI); the
//! tool is deprecated in Apple's documentation but still ships, and the
//! profile language is the same one `sandbox_init` takes.
//!
//! The profile denies everything by default and allows only what a
//! process needs to start and run on its inherited pipes: executing the
//! renderer binary itself, reading the dynamic loader, system libraries
//! and the dyld shared cache (with the Preboot volume it is found through),
//! reading (not setting) system parameters,
//! and signalling itself. No network, no file writes, no reads of user
//! files, no other processes, no Mach services (so no window server,
//! pasteboard, keychain or launch services).

use std::fmt::Write as _;
use std::path::Path;

/// Read-only system locations a process needs to load: dyld, the system
/// libraries and frameworks, and the dyld shared cache in each place
/// macOS versions keep it (11+: Cryptexes on 13+, `/System/Library/dyld`
/// on 11–12, `/private/var/db/dyld` before).
const SYSTEM_READS: &[&str] = &[
    "/usr/lib",
    "/System/Library/Frameworks",
    "/System/Library/PrivateFrameworks",
    "/System/Library/dyld",
    "/System/Cryptexes/OS",
    // dyld finds the OS cryptex (the shared cache on macOS 13+) through
    // libignition, which reads the Preboot volume's boot objects; both the
    // firmlinked and the resolved forms are named, because Seatbelt
    // matches real paths. Preboot holds no user data.
    "/System/Volumes/Preboot",
    "/private/preboot",
    "/private/var/db/dyld",
];

/// Why a profile could not be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileError {
    /// The executable path is not absolute UTF-8 without control
    /// characters, so it cannot be named safely in the profile.
    UnsafePath,
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "the renderer executable path cannot be named in a sandbox profile (it must be \
             absolute UTF-8 without control characters)",
        )
    }
}

impl std::error::Error for ProfileError {}

/// An SBPL string literal: quoted, with `\` and `"` escaped.
fn literal(path: &str) -> String {
    let mut quoted = String::with_capacity(path.len() + 2);
    quoted.push('"');
    for character in path.chars() {
        if matches!(character, '\\' | '"') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

/// Mach services the network process may look up: the certificate trust
/// daemons, so the platform verifier (`rustls-platform-verifier`, which
/// calls `SecTrustEvaluateWithError`) can evaluate a chain. Evaluation
/// runs inside `trustd`, so no keychain file is opened here.
const TRUST_SERVICES: &[&str] = &[
    "com.apple.trustd",
    "com.apple.trustd.agent",
    "com.apple.SecurityServer",
    "com.apple.ocspd",
];

/// The renderer profile for the renderer binary at `exe`, which must be
/// the canonical path (Seatbelt matches resolved paths, and `/var` is a
/// link to `/private/var`).
pub fn renderer_profile(exe: &Path) -> Result<String, ProfileError> {
    base_profile(exe, "Rustic renderer (ADR 0063)")
}

/// The network process profile (ADR 0061, TD-176) for the binary at
/// `exe` (canonical, as for [`renderer_profile`]): the renderer profile
/// plus outbound connections to IP addresses (TCP and UDP; Unix-domain
/// sockets stay denied) and the certificate trust daemons. No
/// file beyond the renderer's is readable and nothing is writable.
pub fn network_profile(exe: &Path) -> Result<String, ProfileError> {
    let mut profile = base_profile(exe, "Rustic network process (ADR 0061, ADR 0063)")?;
    profile.push_str("(allow network-outbound (remote ip \"*:*\"))\n(allow mach-lookup\n");
    for service in TRUST_SERVICES {
        let _ = writeln!(profile, "    (global-name {})", literal(service));
    }
    profile.push_str(")\n");
    Ok(profile)
}

fn base_profile(exe: &Path, title: &str) -> Result<String, ProfileError> {
    let exe = exe.to_str().ok_or(ProfileError::UnsafePath)?;
    if !exe.starts_with('/') || exe.chars().any(char::is_control) {
        return Err(ProfileError::UnsafePath);
    }
    let exe = literal(exe);
    let mut profile = format!(
        "(version 1)\n\
         ; {title}. Everything not allowed below is denied.\n\
         (deny default)\n"
    );
    // `sandbox-exec` executes the process under this profile; nothing
    // else may be executed, and the process may not fork.
    let _ = writeln!(profile, "(allow process-exec (literal {exe}))");
    let _ = writeln!(profile, "(allow file-read* (literal {exe}))");
    profile.push_str("(allow file-read*\n");
    for path in SYSTEM_READS {
        let _ = writeln!(profile, "    (subpath {})", literal(path));
    }
    profile.push_str("    (literal \"/dev/urandom\"))\n");
    // dyld resolves its search paths through the parent directories.
    profile.push_str(
        "(allow file-read-metadata\n    (literal \"/\")\n    (literal \"/private\")\n    \
         (literal \"/private/var\")\n    (literal \"/private/var/db\")\n    \
         (literal \"/System\")\n    (literal \"/System/Volumes\")\n    \
         (literal \"/System/Library\")\n    (literal \"/Library\")\n    (literal \"/usr\"))\n",
    );
    // Reading system parameters is allowed in full: dyld's libignition
    // reads boot parameters whose names are not documented (the first
    // native run aborted in it with a name list). Nothing can be set.
    // Narrowing this to logged names is TD-174 follow-up work.
    profile.push_str("(allow sysctl-read)\n(allow signal (target self))\n");
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_denies_by_default_and_names_only_the_renderer() {
        let profile = renderer_profile(Path::new("/Applications/Rustic.app/rustic")).unwrap();
        assert!(profile.starts_with("(version 1)\n"));
        assert!(profile.contains("(deny default)"));
        assert!(
            profile.contains("(allow process-exec (literal \"/Applications/Rustic.app/rustic\"))")
        );
        // Nothing grants networking, writes, other processes, Mach
        // services or user files.
        for forbidden in [
            "network",
            "file-write",
            "process-fork",
            "mach-lookup",
            "iokit",
            "/Users",
            "(allow default)",
        ] {
            assert!(!profile.contains(forbidden), "{forbidden} in\n{profile}");
        }
        // Balanced parentheses: one form per allow or deny.
        let open = profile.matches('(').count();
        assert_eq!(open, profile.matches(')').count());
    }

    #[test]
    fn paths_are_quoted_or_refused() {
        let profile = renderer_profile(Path::new("/tmp/a \"b\\c/rustic")).unwrap();
        assert!(profile.contains(r#"(literal "/tmp/a \"b\\c/rustic")"#));
        for bad in ["relative/rustic", "/tmp/new\nline", "/tmp/nul\0x", ""] {
            assert_eq!(
                renderer_profile(Path::new(bad)),
                Err(ProfileError::UnsafePath),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_network_profile_adds_only_ip_connections_and_trust_services() {
        let exe = Path::new("/Applications/Rustic.app/rustic");
        let renderer = renderer_profile(exe).unwrap();
        let network = network_profile(exe).unwrap();
        let added = network
            .lines()
            .filter(|line| !renderer.lines().any(|kept| kept == *line))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            added.contains("(allow network-outbound (remote ip \"*:*\"))"),
            "{added}"
        );
        assert!(
            added.contains("(global-name \"com.apple.trustd\")"),
            "{added}"
        );
        // No Unix sockets, inbound connections, writes, forks or files.
        for forbidden in [
            "unix-socket",
            "network-inbound",
            "network-bind",
            "file-write",
            "file-read",
            "process-fork",
            "/Users",
        ] {
            assert!(!added.contains(forbidden), "{forbidden} in\n{added}");
        }
        assert!(network.contains("(deny default)"));
        assert_eq!(network.matches('(').count(), network.matches(')').count());
        assert_eq!(
            network_profile(Path::new("rel")),
            Err(ProfileError::UnsafePath)
        );
    }
}
