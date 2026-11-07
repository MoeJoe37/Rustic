/// An owned sequence of UTF-16 code units, preserving unpaired surrogates.
///
/// Conversion to UTF-8 is deliberately not implicit or lossy. Future Web IDL
/// bindings must choose the conversion required by their declared string type.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DomString(Box<[u16]>);

impl DomString {
    /// Construct without normalizing or replacing any code unit.
    pub fn from_units(units: Vec<u16>) -> Self {
        Self(units.into_boxed_slice())
    }

    /// Raw UTF-16 code units, including any unpaired surrogates.
    pub fn as_units(&self) -> &[u16] {
        &self.0
    }

    pub(crate) fn data_bytes(&self) -> usize {
        // A real allocation of u16s cannot exceed usize::MAX bytes.
        std::mem::size_of_val(self.as_units())
    }
}

impl From<&str> for DomString {
    fn from(value: &str) -> Self {
        Self::from_units(value.encode_utf16().collect())
    }
}

impl From<String> for DomString {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}
