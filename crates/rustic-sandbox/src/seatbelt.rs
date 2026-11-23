//! The macOS renderer and network process sandbox profiles (ADR 0063,
//! verified on native macOS x64 and ARM64).
//!
//! macOS confines a process with a Seatbelt profile in the Sandbox Profile
//! Language (SBPL), the mechanism Chromium's and Firefox's macOS sandboxes
//! use. The renderer (or network process) applies its profile to itself
//! with `sandbox_init` (`crate::enter`, macOS only) before it reads
//! anything from the broker, and can never drop it.
//!
//! The profile denies everything by default and allows only what a
//! process needs to start and run on its inherited pipes: executing the
//! renderer binary itself, reading the dynamic loader, system libraries
//! and the dyld shared cache (with the Preboot volume it is found through),
//! reading (not setting) system parameters other than the machine's
//! names and those about other processes and the network,
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

/// System parameters a confined process may not read (TD-183): the
/// machine's names and identifiers.
pub const HIDDEN_SYSCTLS: &[&str] = &[
    "kern.hostname",
    "kern.nisdomainname",
    "kern.hostid",
    "kern.uuid",
    "kern.bootsessionuuid",
];

/// Parameter families a confined process may not read (TD-183): other
/// processes (`kern.proc.*` lists them, `kern.procargs*` their arguments
/// and environments) and the network (routes, interfaces, addresses).
pub const HIDDEN_SYSCTL_PREFIXES: &[&str] = &["kern.proc.", "kern.procargs", "net."];

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

/// IOKit user-client classes the Metal drivers open: the IOSurface root
/// (shared GPU memory), the accelerator families of Apple silicon (AGX),
/// Intel, AMD and the virtual machine display (paravirtualized Metal),
/// and graphics power control. The same classes Chromium's GPU process
/// profile allows.
const GPU_IOKIT_CLASSES: &[&str] = &[
    "IOSurfaceRootUserClient",
    "IOAccelerator",
    "IOAccelContext",
    "IOAccelContext2",
    "IOAccelDevice",
    "IOAccelDevice2",
    "IOAccelSharedUserClient",
    "IOAccelSharedUserClient2",
    "IOAccelSubmitter2",
    "AGXDeviceUserClient",
    "AGXSharedUserClient",
    "AGXCommandQueue",
    "AppleParavirtDeviceUserClient",
    "AppleGraphicsControlClient",
    "AppleGraphicsPolicyClient",
    "AppleMGPUPowerControlClient",
    "AppleIntelMEUserClient",
    "AppleSNBFBUserClient",
    "AMDRadeonX4000_AMDAccelDevice",
    "AMDRadeonX5000_AMDAccelDevice",
    "AMDRadeonX6000_AMDAccelDevice",
];

/// Mach services Metal reaches: the shader compiler (it compiles in its
/// own process), the OpenGL/Metal compiler cache service and GPU memory
/// accounting.
const GPU_SERVICES: &[&str] = &[
    "com.apple.MTLCompilerService",
    "com.apple.cvmsServ",
    "com.apple.gpumemd.source",
];

/// Driver bundles Metal loads its device plug-ins from (read only).
const GPU_READS: &[&str] = &[
    "/System/Library/Extensions",
    "/Library/Apple/System/Library/Extensions",
    "/Library/GPUBundles",
];

/// The GPU process profile (ADR 0071, TD-186) for the binary at `exe`
/// (canonical, as for [`renderer_profile`]): the renderer profile plus
/// what Metal needs to open a device: the graphics drivers' IOKit user
/// clients, reading IOKit properties (adapter discovery), the shader
/// compiler and GPU memory services, and reading the driver bundles. No
/// network, no writes, no user files, no window server.
pub fn gpu_profile(exe: &Path) -> Result<String, ProfileError> {
    let mut profile = base_profile(exe, "Rustic GPU process (ADR 0068, ADR 0071)")?;
    profile.push_str(
        "(allow iokit-get-properties)
(allow iokit-open
",
    );
    for class in GPU_IOKIT_CLASSES {
        let _ = writeln!(profile, "    (iokit-user-client-class {})", literal(class));
    }
    profile.push_str(
        ")
(allow mach-lookup
",
    );
    for service in GPU_SERVICES {
        let _ = writeln!(profile, "    (global-name {})", literal(service));
    }
    profile.push_str(
        ")
(allow file-read*
",
    );
    for path in GPU_READS {
        let _ = writeln!(profile, "    (subpath {})", literal(path));
    }
    profile.push_str(")\n");
    // IOSurface reads the GPU policies of the process's main bundle
    // (`IOSurfaceClientCopyGPUPolicies`), and CoreFoundation finds that
    // bundle by examining the executable's directory and its parents;
    // without it the bundle is null and IOSurface crashes. Metadata only:
    // names and attributes of those directories and the bundle files next
    // to the binary, not their contents elsewhere.
    if let Some(directory) = exe.parent() {
        let directory = directory.to_str().ok_or(ProfileError::UnsafePath)?;
        profile.push_str("(allow file-read-metadata\n");
        let _ = writeln!(profile, "    (subpath {})", literal(directory));
        let mut ancestor = Path::new(directory).parent();
        while let Some(path) = ancestor {
            if let Some(text) = path.to_str() {
                let _ = writeln!(profile, "    (literal {})", literal(text));
            }
            ancestor = path.parent();
        }
        profile.push_str(")\n");
    }
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
    // Nothing but the process's own binary may be executed, and it may
    // not fork.
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
    // System parameters are readable except those that identify the
    // machine or reveal other processes and the network (TD-183). The
    // libraries' own reads are open-ended (dyld's libignition reads boot
    // parameters whose names are not documented), so the profile denies
    // what matters instead of listing what is needed. Nothing can be set.
    profile.push_str("(allow sysctl-read\n    (require-all\n");
    for name in HIDDEN_SYSCTLS {
        let _ = writeln!(
            profile,
            "        (require-not (sysctl-name {}))",
            literal(name)
        );
    }
    for prefix in HIDDEN_SYSCTL_PREFIXES {
        let _ = writeln!(
            profile,
            "        (require-not (sysctl-name-prefix {}))",
            literal(prefix)
        );
    }
    profile.push_str("    ))\n(allow signal (target self))\n");
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
        // System parameters that identify the machine or reveal other
        // processes and the network stay hidden (TD-183).
        for name in ["kern.hostname", "kern.uuid"] {
            assert!(
                profile.contains(&format!("(require-not (sysctl-name \"{name}\"))")),
                "{profile}"
            );
        }
        for prefix in ["kern.proc.", "kern.procargs", "net."] {
            assert!(
                profile.contains(&format!("(require-not (sysctl-name-prefix \"{prefix}\"))")),
                "{profile}"
            );
        }
        assert!(!profile.contains("(allow sysctl-read)"), "{profile}");
        // Balanced parentheses: one form per allow or deny.
        let open = profile.matches('(').count();
        assert_eq!(open, profile.matches(')').count());
    }

    #[test]
    fn the_gpu_profile_adds_only_graphics_drivers_and_metal_services() {
        let exe = Path::new("/Applications/Rustic.app/rustic");
        let renderer = renderer_profile(exe).unwrap();
        let gpu = gpu_profile(exe).unwrap();
        let added = gpu
            .lines()
            .filter(|line| !renderer.lines().any(|kept| kept == *line))
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "(iokit-user-client-class \"IOSurfaceRootUserClient\")",
            "(iokit-user-client-class \"AGXDeviceUserClient\")",
            "(global-name \"com.apple.MTLCompilerService\")",
            "(subpath \"/System/Library/Extensions\")",
            "(subpath \"/Applications/Rustic.app\")",
        ] {
            assert!(added.contains(expected), "{expected} missing in\n{added}");
        }
        // No network, writes, forks, user files, window server or
        // pasteboard, and IOKit only by named class.
        for forbidden in [
            "network",
            "file-write",
            "process-fork",
            "/Users",
            "windowserver",
            "pasteboard",
            "(allow iokit-open)",
        ] {
            assert!(!added.contains(forbidden), "{forbidden} in\n{added}");
        }
        assert!(gpu.contains("(deny default)"));
        assert_eq!(gpu.matches('(').count(), gpu.matches(')').count());
        assert_eq!(gpu_profile(Path::new("rel")), Err(ProfileError::UnsafePath));
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
