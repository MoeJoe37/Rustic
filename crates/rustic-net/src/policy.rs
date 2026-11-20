use std::fmt;
use std::net::SocketAddr;

use rustic_url::Url;

/// Which origins the transport may contact, and at which socket address.
///
/// This is the only network authority `rustic-net` has: there is no DNS
/// resolution and no default route. Until the M7 isolation gate, every
/// address must be a loopback address, so the development prototype cannot
/// reach the public internet even when a page asks it to (ADR 0051).
#[derive(Clone, Default)]
pub struct NetworkPolicy {
    routes: Vec<Route>,
}

#[derive(Clone)]
struct Route {
    scheme: String,
    host: String,
    port: u16,
    address: SocketAddr,
}

impl NetworkPolicy {
    /// A policy that allows nothing.
    pub fn deny_all() -> Self {
        Self::default()
    }

    /// Allow the origin of `origin` (scheme, host and port; the rest of the
    /// URL is ignored) and route it to `address`. A later call for the same
    /// origin replaces the route.
    pub fn allow(&mut self, origin: &Url, address: SocketAddr) -> Result<(), PolicyError> {
        let (scheme, host, port) = key(origin).ok_or(PolicyError::UnsupportedScheme)?;
        if !address.ip().is_loopback() {
            return Err(PolicyError::NotLoopback);
        }
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

    /// The address for `url`'s origin, if it is allowed.
    pub fn route(&self, url: &Url) -> Option<SocketAddr> {
        let (scheme, host, port) = key(url)?;
        self.routes
            .iter()
            .find(|route| route.scheme == scheme && route.host == host && route.port == port)
            .map(|route| route.address)
    }

    /// Number of allowed origins.
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// Whether nothing is allowed.
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

impl fmt::Debug for NetworkPolicy {
    /// Counts only: the allowlist names hosts the user visits.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NetworkPolicy")
            .field("origins", &self.routes.len())
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
    /// The address is not loopback. Remote addresses wait for M7.
    NotLoopback,
}

impl fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedScheme => "only http and https origins can be allowed",
            Self::NotLoopback => "only loopback addresses can be allowed before M7",
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
        assert_eq!(format!("{policy:?}"), "NetworkPolicy { origins: 2 }");
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
}
