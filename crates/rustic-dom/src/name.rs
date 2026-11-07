use crate::DomError;

/// Namespace used for HTML elements.
pub const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
/// Namespace used for SVG elements.
pub const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
/// Namespace used for MathML elements.
pub const MATHML_NAMESPACE: &str = "http://www.w3.org/1998/Math/MathML";

/// Namespace, optional prefix, and local name supplied by a trusted parser.
///
/// This low-level representation preserves case and does not implement public
/// `createElement`/`createElementNS` validation or HTML name normalization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QualifiedName {
    namespace: Option<Box<str>>,
    prefix: Option<Box<str>>,
    local: Box<str>,
}

impl QualifiedName {
    /// Retain a parsed name; empty namespace/prefix strings become absent.
    pub fn from_parsed(
        namespace: Option<&str>,
        prefix: Option<&str>,
        local: &str,
    ) -> Result<Self, DomError> {
        let namespace = namespace.filter(|value| !value.is_empty());
        let prefix = prefix.filter(|value| !value.is_empty());
        if local.is_empty() || (prefix.is_some() && namespace.is_none()) {
            return Err(DomError::InvalidName);
        }
        Ok(Self {
            namespace: namespace.map(Into::into),
            prefix: prefix.map(Into::into),
            local: local.into(),
        })
    }

    /// An already-normalized HTML local name.
    pub fn html(local: &str) -> Result<Self, DomError> {
        Self::from_parsed(Some(HTML_NAMESPACE), None, local)
    }

    /// Namespace URI; absent and empty namespaces have one representation.
    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    /// Namespace prefix, if present.
    pub fn prefix(&self) -> Option<&str> {
        self.prefix.as_deref()
    }

    /// Local name, preserving the parser's case.
    pub fn local(&self) -> &str {
        &self.local
    }

    pub(crate) fn same_expanded_name(&self, other: &Self) -> bool {
        self.namespace == other.namespace && self.local == other.local
    }

    pub(crate) fn data_bytes(&self) -> Result<usize, DomError> {
        self.local
            .len()
            .checked_add(self.namespace.as_ref().map_or(0, |value| value.len()))
            .and_then(|bytes| {
                bytes.checked_add(self.prefix.as_ref().map_or(0, |value| value.len()))
            })
            .ok_or(DomError::DataLimit)
    }
}
