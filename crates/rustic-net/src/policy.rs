use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};

use rustic_url::{Host, Url};

/// Which origins the transport may contact, and at which socket address.
///
/// This is the only network authority `rustic-net` has; there is no
/// default route. An origin is reached either at a loopback address the
/// user gave (a fixture server, ADR 0051) or, since M7 (TD-157), at the
/// addresses the system resolver returns for its host, of which only
/// public unicast addresses are used: a resolved name never reaches
/// loopback, private, link-local or other special-purpose addresses, so a
/// page cannot turn DNS into a path to the user's own machine or network
/// (DNS rebinding).
#[derive(Clone, Default)]
pub struct NetworkPolicy {
    routes: Vec<Route>,
    /// Every `http`/`https` origin may be resolved (`--allow-remote`).
    any_remote: bool,
}

#[derive(Clone)]
struct Route {
    scheme: String,
    host: String,
    port: u16,
    /// A fixed loopback address, or `None` to resolve the host.
    address: Option<SocketAddr>,
}

/// Resolves a host name to socket addresses (the system resolver, or a
/// fixed table in tests).
pub type Resolver = dyn Fn(&str, u16) -> std::io::Result<Vec<SocketAddr>> + Send + Sync;

/// The system resolver (`getaddrinfo` and its platform equivalents).
pub fn system_resolver(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    Ok((host, port).to_socket_addrs()?.collect())
}

/// Whether `ip` is a public unicast address: not loopback, private,
/// shared (CGNAT), link-local, unique-local, documentation, benchmarking,
/// multicast, reserved or unspecified (RFC 6890 special-purpose
/// registries), looking through IPv4-mapped, NAT64 and 6to4 forms.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..128).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..32).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 168)
        || (a == 198 && (18..20).contains(&b))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    let embedded = |high: u16, low: u16| {
        Ipv4Addr::new((high >> 8) as u8, high as u8, (low >> 8) as u8, low as u8)
    };
    // IPv4-mapped (::ffff:a.b.c.d) and NAT64 (64:ff9b::a.b.c.d).
    if segments[..5] == [0; 5] && segments[5] == 0xffff
        || segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0]
    {
        return is_public_v4(embedded(segments[6], segments[7]));
    }
    // 6to4 (2002:a.b.c.d::/48).
    if segments[0] == 0x2002 {
        return is_public_v4(embedded(segments[1], segments[2]));
    }
    // Global unicast only (2000::/3), minus documentation (2001:db8::/32)
    // and the IETF protocol block below 2001:200::/23 except Teredo.
    (segments[0] & 0xe000) == 0x2000
        && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
        && !(segments[0] == 0x2001 && segments[1] < 0x0200 && segments[1] != 0)
}

impl NetworkPolicy {
    /// A policy that allows nothing.
    pub fn deny_all() -> Self {
        Self::default()
    }

    /// Allow the origin of `origin` (scheme, host and port; the rest of the
    /// URL is ignored) and route it to `address`, which must be loopback
    /// (a fixture server the user runs). A later call for the same origin
    /// replaces the route.
    pub fn allow(&mut self, origin: &Url, address: SocketAddr) -> Result<(), PolicyError> {
        if !address.ip().is_loopback() {
            return Err(PolicyError::NotLoopback);
        }
        self.insert(origin, Some(address))
    }

    /// Allow the origin of `origin` at the public addresses its host
    /// resolves to.
    pub fn allow_resolved(&mut self, origin: &Url) -> Result<(), PolicyError> {
        self.insert(origin, None)
    }

    /// Allow every `http`/`https` origin at the public addresses its host
    /// resolves to (explicit routes still win for their origins).
    pub fn allow_any_remote(&mut self) {
        self.any_remote = true;
    }

    /// Allow the origin of `origin` at `address` only if that address is
    /// public: the network process re-checks a resolved route this way
    /// (defence in depth; it never resolves names itself).
    pub fn allow_public(&mut self, origin: &Url, address: SocketAddr) -> Result<(), PolicyError> {
        if !is_public(address.ip()) {
            return Err(PolicyError::NotPublic);
        }
        self.insert(origin, Some(address))
    }

    fn insert(&mut self, origin: &Url, address: Option<SocketAddr>) -> Result<(), PolicyError> {
        let (scheme, host, port) = key(origin).ok_or(PolicyError::UnsupportedScheme)?;
        self.routes.retain(|route| {
            (route.scheme.as_str(), route.host.as_str(), route.port)
                != (scheme, host.as_str(), port)
        });
        self.routes.push(Route {
            scheme: scheme.to_owned(),
            host,
            port,
            address,
        });
        Ok(())
    }

    /// The fixed address for `url`'s origin, if it has one.
    pub fn route(&self, url: &Url) -> Option<SocketAddr> {
        let (scheme, host, port) = key(url)?;
        self.routes
            .iter()
            .find(|route| route.scheme == scheme && route.host == host && route.port == port)
            .and_then(|route| route.address)
    }

    /// The address to connect to for `url`: its fixed route, or the first
    /// public address `resolve` gives for its host when the origin may be
    /// resolved. An IP-literal host is used as is, if public.
    pub fn address(&self, url: &Url, resolve: &Resolver) -> Result<SocketAddr, PolicyError> {
        let (scheme, host, port) = key(url).ok_or(PolicyError::UnsupportedScheme)?;
        let route = self
            .routes
            .iter()
            .find(|route| route.scheme == scheme && route.host == host && route.port == port);
        match route {
            Some(Route {
                address: Some(address),
                ..
            }) => return Ok(*address),
            Some(_) => {}
            None if self.any_remote => {}
            None => return Err(PolicyError::NotAllowed),
        }
        let literal = url.host().and_then(|host: &Host| {
            host.ipv4()
                .map(|ip| IpAddr::V4(Ipv4Addr::from(ip)))
                .or_else(|| host.ipv6().map(|ip| IpAddr::V6(Ipv6Addr::from(ip))))
        });
        let candidates = match literal {
            Some(ip) => vec![SocketAddr::new(ip, port)],
            None => resolve(&host, port).map_err(|_| PolicyError::NotResolved)?,
        };
        if candidates.is_empty() {
            return Err(PolicyError::NotResolved);
        }
        candidates
            .into_iter()
            .find(|candidate| is_public(candidate.ip()))
            .ok_or(PolicyError::NotPublic)
    }

    /// Number of allowed origins.
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// Whether nothing is allowed.
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty() && !self.any_remote
    }
}

impl fmt::Debug for NetworkPolicy {
    /// Counts only: the allowlist names hosts the user visits.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NetworkPolicy")
            .field("origins", &self.routes.len())
            .field("any_remote", &self.any_remote)
            .finish()
    }
}

/// Scheme, serialized host and effective port of an `http`/`https` URL.
fn key(url: &Url) -> Option<(&'static str, String, u16)> {
    let (scheme, default_port) = match url.scheme() {
        "http" => ("http", 80),
        "https" => ("https", 443),
        _ => return None,
    };
    let host = url.host()?.serialize();
    Some((scheme, host, url.port().unwrap_or(default_port)))
}

/// An origin could not be allowed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyError {
    /// Only `http` and `https` origins can be allowed.
    UnsupportedScheme,
    /// A fixed address must be loopback; remote hosts are resolved.
    NotLoopback,
    /// The origin is not allowed.
    NotAllowed,
    /// The host name did not resolve.
    NotResolved,
    /// The host resolved only to non-public addresses (loopback, private,
    /// link-local, ...), which a resolved origin never reaches.
    NotPublic,
}

impl fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedScheme => "only http and https origins can be allowed",
            Self::NotLoopback => "a fixed address must be a loopback address",
            Self::NotAllowed => "origin not allowed by the network policy",
            Self::NotResolved => "the host name did not resolve",
            Self::NotPublic => {
                "the host resolved only to local or private addresses, which remote origins never reach"
            }
        })
    }
}

impl std::error::Error for PolicyError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(input: &str) -> Url {
        Url::parse(input, None).unwrap()
    }

    #[test]
    fn routes_match_scheme_host_and_effective_port() {
        let local: SocketAddr = "127.0.0.1:4000".parse().unwrap();
        let other: SocketAddr = "[::1]:4001".parse().unwrap();
        let mut policy = NetworkPolicy::deny_all();
        policy
            .allow(&url("http://App.Test/ignored?x#y"), local)
            .unwrap();
        policy.allow(&url("http://app.test:8080"), other).unwrap();
        assert_eq!(policy.route(&url("http://app.test:80/page")), Some(local));
        assert_eq!(policy.route(&url("http://app.test:8080/")), Some(other));
        assert_eq!(policy.route(&url("https://app.test/")), None);
        assert_eq!(policy.route(&url("http://sub.app.test/")), None);
        assert_eq!(policy.route(&url("ftp://app.test/")), None);
        policy.allow(&url("http://app.test"), other).unwrap();
        assert_eq!(policy.route(&url("http://app.test/")), Some(other));
        assert_eq!(policy.len(), 2);
        assert_eq!(
            format!("{policy:?}"),
            "NetworkPolicy { origins: 2, any_remote: false }"
        );
    }

    #[test]
    fn refuses_remote_addresses_and_other_schemes() {
        let mut policy = NetworkPolicy::deny_all();
        assert_eq!(
            policy.allow(&url("http://a.test"), "192.0.2.1:80".parse().unwrap()),
            Err(PolicyError::NotLoopback)
        );
        assert_eq!(
            policy.allow(&url("ws://a.test"), "127.0.0.1:80".parse().unwrap()),
            Err(PolicyError::UnsupportedScheme)
        );
        assert!(policy.is_empty());
    }

    fn table(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
        let ips: &[&str] = match host {
            "public.test" => &["10.0.0.1", "93.184.216.34"],
            "rebind.test" => &["127.0.0.1", "192.168.1.1", "::1", "fd00::1"],
            "v6.test" => &["2606:4700::1"],
            _ => return Err(std::io::ErrorKind::NotFound.into()),
        };
        Ok(ips
            .iter()
            .map(|ip| SocketAddr::new(ip.parse().unwrap(), port))
            .collect())
    }

    #[test]
    fn resolved_origins_reach_only_public_addresses() {
        let mut policy = NetworkPolicy::deny_all();
        policy.allow_resolved(&url("https://public.test")).unwrap();
        policy.allow_resolved(&url("https://rebind.test")).unwrap();
        assert_eq!(
            policy.address(&url("https://public.test/x"), &table),
            Ok("93.184.216.34:443".parse().unwrap())
        );
        assert_eq!(
            policy.address(&url("https://rebind.test/"), &table),
            Err(PolicyError::NotPublic)
        );
        // Not granted, and a name that does not resolve.
        assert_eq!(
            policy.address(&url("https://v6.test/"), &table),
            Err(PolicyError::NotAllowed)
        );
        policy.allow_any_remote();
        assert_eq!(
            policy.address(&url("https://v6.test:8443/"), &table),
            Ok("[2606:4700::1]:8443".parse().unwrap())
        );
        assert_eq!(
            policy.address(&url("https://nowhere.test/"), &table),
            Err(PolicyError::NotResolved)
        );
        // IP literals skip the resolver but not the public check.
        assert_eq!(
            policy.address(&url("http://127.0.0.1:8080/"), &table),
            Err(PolicyError::NotPublic)
        );
        assert_eq!(
            policy.address(&url("http://[2606:4700::2]/"), &table),
            Ok("[2606:4700::2]:80".parse().unwrap())
        );
        // A fixed loopback route still wins for its origin.
        let fixture: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        policy.allow(&url("http://rebind.test"), fixture).unwrap();
        assert_eq!(
            policy.address(&url("http://rebind.test/"), &table),
            Ok(fixture)
        );
        // The network process's re-check accepts public addresses only.
        let mut check = NetworkPolicy::deny_all();
        assert_eq!(
            check.allow_public(&url("https://a.test"), "192.168.0.2:443".parse().unwrap()),
            Err(PolicyError::NotPublic)
        );
        assert!(
            check
                .allow_public(&url("https://a.test"), "93.184.216.34:443".parse().unwrap())
                .is_ok()
        );
    }

    #[test]
    fn special_purpose_addresses_are_not_public() {
        for ip in [
            "0.0.0.0",
            "10.1.2.3",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.8",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.0.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fe80::1",
            "fc00::1",
            "fd12::1",
            "ff02::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "2002:c0a8:101::1",
            "100::1",
            "2001:10::1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "100.128.0.1",
            "172.32.0.1",
            "2606:4700::1",
            "2a00:1450::1",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "2001::1",
        ] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }
}
