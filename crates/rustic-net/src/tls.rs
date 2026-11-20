//! The TLS adapter (ADR 0051): rustls 0.23 with the `ring` provider,
//! TLS 1.2 and 1.3, and certificate verification by the platform
//! (`rustls-platform-verifier`) or by an explicit root set.
//!
//! There is deliberately no way to skip verification: every
//! [`TlsConfig`] verifies the chain, the validity period and the server
//! name. A failure ends the exchange with
//! [`TransportError::Certificate`](crate::TransportError::Certificate) and
//! is never retried over `http`.
//!
//! On Android the host must initialize the platform verifier's JNI bridge
//! before the first `https` request (MOBILE.md).

use std::fmt;
use std::sync::Arc;

use rustic_url::Url;
use rustls::client::WantsClientCert;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, ConfigBuilder, RootCertStore};
use rustls_platform_verifier::BuilderVerifierExt;

/// How `https` certificates are verified. Cheap to clone.
#[derive(Clone)]
pub struct TlsConfig {
    client: Arc<ClientConfig>,
    kind: &'static str,
}

impl fmt::Debug for TlsConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TlsConfig")
            .field("roots", &self.kind)
            .finish()
    }
}

/// A TLS configuration could not be built.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TlsSetupError {
    /// The platform verifier is unavailable (for example, Android before
    /// its JNI bridge is initialized).
    Platform,
    /// No certificate could be read from the PEM input.
    NoCertificates,
    /// A PEM certificate is not a usable trust anchor.
    InvalidRoot,
}

impl fmt::Display for TlsSetupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Platform => "the platform certificate verifier is unavailable",
            Self::NoCertificates => "no PEM certificates found",
            Self::InvalidRoot => "a root certificate is not a valid trust anchor",
        })
    }
}

impl std::error::Error for TlsSetupError {}

fn builder() -> rustls::ConfigBuilder<ClientConfig, rustls::WantsVerifier> {
    ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .expect("ring supports TLS 1.2 and 1.3")
}

fn finish(builder: ConfigBuilder<ClientConfig, WantsClientCert>, kind: &'static str) -> TlsConfig {
    let mut client = builder.with_no_client_auth();
    // HTTP/1.1 only until HTTP/2 exists (TD-156).
    client.alpn_protocols = vec![b"http/1.1".to_vec()];
    TlsConfig {
        client: Arc::new(client),
        kind,
    }
}

impl TlsConfig {
    /// Verify with the operating system's trust store and policy:
    /// Schannel on Windows, Security.framework on Apple platforms, the
    /// Android system verifier, and system roots with webpki elsewhere.
    pub fn platform() -> Result<Self, TlsSetupError> {
        let builder = builder()
            .with_platform_verifier()
            .map_err(|_| TlsSetupError::Platform)?;
        Ok(finish(builder, "platform"))
    }

    /// Verify against exactly the PEM certificates given (enterprise
    /// roots, or a test CA). The platform store is not consulted.
    pub fn with_roots_pem(pem: &[u8]) -> Result<Self, TlsSetupError> {
        let mut roots = RootCertStore::empty();
        let mut count = 0;
        for certificate in CertificateDer::pem_slice_iter(pem) {
            let certificate = certificate.map_err(|_| TlsSetupError::NoCertificates)?;
            roots
                .add(certificate)
                .map_err(|_| TlsSetupError::InvalidRoot)?;
            count += 1;
        }
        if count == 0 {
            return Err(TlsSetupError::NoCertificates);
        }
        Ok(finish(builder().with_root_certificates(roots), "custom"))
    }

    /// A client connection for `url`, with the URL host as server name
    /// (SNI for domains; IP addresses are verified against IP SANs).
    pub(crate) fn connect(&self, url: &Url) -> Result<ClientConnection, CertificateError> {
        let host = url.host().ok_or(CertificateError::Other)?;
        let name = match (host.ipv4(), host.ipv6()) {
            (Some(octets), _) => ServerName::IpAddress(std::net::IpAddr::from(octets).into()),
            (_, Some(segments)) => ServerName::IpAddress(std::net::IpAddr::from(segments).into()),
            _ => ServerName::try_from(host.serialize())
                .map_err(|_| CertificateError::NotValidForName)?,
        };
        ClientConnection::new(Arc::clone(&self.client), name).map_err(|_| CertificateError::Other)
    }
}

/// Why a server's certificate was refused. There is no override.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CertificateError {
    /// The chain does not lead to a trusted root.
    UnknownIssuer,
    /// The certificate is expired or not yet valid.
    Expired,
    /// The certificate does not cover the requested host.
    NotValidForName,
    /// The certificate was revoked.
    Revoked,
    /// The certificate is malformed or unsuitable for a TLS server.
    Invalid,
    /// Another verification failure.
    Other,
}

impl fmt::Display for CertificateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnknownIssuer => "certificate issuer is not trusted",
            Self::Expired => "certificate is expired or not yet valid",
            Self::NotValidForName => "certificate is not valid for this host",
            Self::Revoked => "certificate was revoked",
            Self::Invalid => "certificate is invalid",
            Self::Other => "certificate verification failed",
        })
    }
}

impl std::error::Error for CertificateError {}

/// What a TLS-level `io::Error` means for the transport.
pub(crate) enum TlsFailure {
    /// Certificate verification failed.
    Certificate(CertificateError),
    /// Any other handshake or record-layer failure.
    Handshake,
}

/// Classify an I/O error raised by rustls, or `None` for a plain socket
/// error.
pub(crate) fn classify(error: &std::io::Error) -> Option<TlsFailure> {
    let tls = error.get_ref()?.downcast_ref::<rustls::Error>()?;
    Some(match tls {
        rustls::Error::InvalidCertificate(reason) => {
            use rustls::CertificateError as Reason;
            TlsFailure::Certificate(match reason {
                Reason::UnknownIssuer
                | Reason::BadSignature
                | Reason::UnhandledCriticalExtension => CertificateError::UnknownIssuer,
                Reason::Expired
                | Reason::ExpiredContext { .. }
                | Reason::NotValidYet
                | Reason::NotValidYetContext { .. } => CertificateError::Expired,
                Reason::NotValidForName | Reason::NotValidForNameContext { .. } => {
                    CertificateError::NotValidForName
                }
                Reason::Revoked => CertificateError::Revoked,
                Reason::BadEncoding
                | Reason::InvalidPurpose
                | Reason::InvalidPurposeContext { .. } => CertificateError::Invalid,
                _ => CertificateError::Other,
            })
        }
        _ => TlsFailure::Handshake,
    })
}
