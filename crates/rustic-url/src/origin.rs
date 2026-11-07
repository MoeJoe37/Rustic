use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::record::default_port;
use crate::{Host, Url};

/// A tuple or unique process-local opaque origin.
///
/// Opaque serialization is always `null`, but independent opaque origins are not
/// equal. Clone an assigned origin to preserve its identity. A navigation layer
/// must assign/retain document origins and handle inheritance/sandboxing itself.
#[derive(Clone)]
pub struct Origin(OriginKind);

#[derive(Clone)]
enum OriginKind {
    Tuple {
        scheme: String,
        host: Host,
        port: Option<u16>,
    },
    Opaque(Arc<()>),
}

impl Origin {
    /// Create an independent opaque origin without serializing its identity.
    pub fn opaque() -> Self {
        Self(OriginKind::Opaque(Arc::new(())))
    }

    pub(crate) fn for_url(url: &Url) -> Self {
        if default_port(url.scheme()).is_some() {
            Self(OriginKind::Tuple {
                scheme: url.scheme().to_owned(),
                host: url.host().expect("special URLs have a host").clone(),
                port: url.port(),
            })
        } else {
            Self::opaque()
        }
    }

    /// Whether this is an opaque origin rather than a scheme/host/port tuple.
    pub fn is_opaque(&self) -> bool {
        matches!(self.0, OriginKind::Opaque(_))
    }

    /// Structured same-origin comparison; never compares `null` strings.
    pub fn is_same_origin(&self, other: &Self) -> bool {
        self == other
    }

    /// Origin serialization. `null` is a display value, not a reusable identity.
    pub fn serialize(&self) -> String {
        match &self.0 {
            OriginKind::Opaque(_) => "null".into(),
            OriginKind::Tuple { scheme, host, port } => {
                let mut output = format!("{scheme}://{}", host.serialize());
                if let Some(port) = port {
                    use std::fmt::Write;
                    write!(output, ":{port}").expect("String write");
                }
                output
            }
        }
    }
}

impl PartialEq for Origin {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (
                OriginKind::Tuple {
                    scheme: a,
                    host: b,
                    port: c,
                },
                OriginKind::Tuple {
                    scheme: x,
                    host: y,
                    port: z,
                },
            ) => (a, b, c) == (x, y, z),
            (OriginKind::Opaque(a), OriginKind::Opaque(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl Eq for Origin {}

impl Hash for Origin {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match &self.0 {
            OriginKind::Tuple { scheme, host, port } => {
                0u8.hash(state);
                (scheme, host, port).hash(state);
            }
            OriginKind::Opaque(identity) => {
                1u8.hash(state);
                Arc::as_ptr(identity).hash(state);
            }
        }
    }
}

impl fmt::Debug for Origin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            OriginKind::Opaque(_) => formatter.write_str("OpaqueOrigin"),
            OriginKind::Tuple { scheme, host, port } => formatter
                .debug_struct("TupleOrigin")
                .field("scheme", scheme)
                .field("host", host)
                .field("port", port)
                .finish(),
        }
    }
}
