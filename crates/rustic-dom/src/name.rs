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

/// Whether `value` matches the XML 1.0 (Fifth Edition) `Name` production.
pub(crate) fn is_xml_name(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(is_name_start) && chars.all(is_name_char)
}

fn is_name_start(c: char) -> bool {
    matches!(c,
        ':' | 'A'..='Z' | '_' | 'a'..='z'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || matches!(c,
            '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

#[cfg(test)]
mod tests {
    use super::is_xml_name;

    #[test]
    fn xml_name_production_boundaries() {
        for name in [
            "a",
            "xml-stylesheet",
            "_x.1",
            ":",
            "é",
            "a\u{B7}",
            "\u{10000}",
        ] {
            assert!(is_xml_name(name), "{name:?}");
        }
        for name in [
            "", "1a", "-a", ".a", "a b", "a?", "\u{B7}", "\u{D7}", "\u{FFFE}",
        ] {
            assert!(!is_xml_name(name), "{name:?}");
        }
    }
}
