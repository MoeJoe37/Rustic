//! Rustic's network transport (NET-01).
//!
//! The crate owns HTTP/1.1 (RFC 9112) as Rustic code: request
//! serialization, an incremental response parser that never depends on how
//! input is split, and body framing by `Content-Length`, `chunked` and
//! connection close, all under explicit [`TransportLimits`]. A [`Transport`]
//! runs each [`Request`] as an [`Exchange`] on its own thread and reports
//! [`NetEvent`]s through a bounded window, so a slow consumer stops socket
//! reads instead of growing memory (backpressure). Exchanges honour
//! cancellation, connect, idle and total timeouts.
//!
//! Authority is explicit: a [`NetworkPolicy`] lists the origins that may be
//! contacted and the loopback address each one maps to. There is no DNS
//! and no route to remote hosts before the M7 isolation gate. `https` uses
//! the [`tls`] adapter (rustls with platform certificate verification,
//! ADR 0051); a refused certificate is never bypassed or downgraded to
//! `http`.
//!
//! Above the transport, [`mime`] parses MIME types and extracts a
//! response's `Content-Type` and charset as Fetch does, and [`redirect`]
//! follows HTTP redirects with Fetch's rules, rechecking the policy on every
//! hop.
//!
//! [`fetch::Fetcher`] runs Fetch's main fetch on top (NET-03): request
//! modes, response tainting, CORS checks and preflights ([`cors`]),
//! credentials, the `Origin` header, Cross-Origin-Resource-Policy and
//! filtered responses. Its [`cookie::CookieJar`] implements RFC 6265bis
//! storage and retrieval (NET-04), using [`site`] for registrable domains
//! and same-site decisions.
//!
//! [`fixture::FixtureServer`] is the in-process loopback server that the
//! tests use (ADR 0053). Navigation, the HTTP cache and profiles are later
//! layers (NET-02, NET-04, NET-06).

#![warn(missing_docs)]

mod coding;
pub mod cookie;
pub mod cors;
pub mod csp;
pub mod data_url;
pub mod fetch;
pub mod fixture;
mod headers;
pub mod http_cache;
mod limits;
pub mod mime;
pub mod mixed_content;
pub mod navigation;
mod pipe;
mod policy;
pub mod redirect;
pub mod referrer;
mod request;
mod response;
pub mod site;
pub mod tls;
mod transport;

pub use headers::{HeaderError, Headers, is_token, is_valid_value as is_valid_header_value};
pub use limits::{LimitsError, TransportLimits};
pub use policy::{NetworkPolicy, PolicyError};
pub use request::{Request, RequestError};
pub use response::{HttpVersion, ProtocolError, ResponseHead};
pub use transport::{Exchange, NetEvent, TimeoutKind, Transport, TransportError};
