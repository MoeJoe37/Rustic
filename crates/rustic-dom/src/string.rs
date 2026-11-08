/// An owned sequence of UTF-16 code units, preserving unpaired surrogates.
///
/// Conversion to UTF-8 is deliberately not implicit or lossy. Future Web IDL
/// bindings must choose the conversion required by their declared string type.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DomString(Vec<u16>);

impl DomString {
    /// Construct without normalizing or replacing any code unit.
    pub fn from_units(units: Vec<u16>) -> Self {
        Self(units)
    }

    /// Raw UTF-16 code units, including any unpaired surrogates.
    pub fn as_units(&self) -> &[u16] {
        &self.0
    }

    pub(crate) fn data_bytes(&self) -> usize {
        // A real allocation of u16s cannot exceed usize::MAX bytes.
        std::mem::size_of_val(self.as_units())
    }

    pub(crate) fn reserve_units(&mut self, additional: usize) -> Result<(), crate::DomError> {
        self.0
            .try_reserve(additional)
            .map_err(|_| crate::DomError::AllocationFailed)
    }

    pub(crate) fn replace_reserved(
        &mut self,
        range: std::ops::Range<usize>,
        value: &[u16],
        new_len: usize,
    ) {
        let old_len = self.0.len();
        // The document validated the range/final length and reserved capacity
        // before committing accounting. These moves cannot allocate or call out.
        if new_len > old_len {
            self.0.resize(new_len, 0);
        }
        let inserted_end = range.start + value.len(); // At most validated new_len.
        self.0.copy_within(range.end..old_len, inserted_end);
        self.0[range.start..inserted_end].copy_from_slice(value);
        self.0.truncate(new_len);
    }

    pub(crate) fn append_utf8_reserved(&mut self, value: &str) {
        self.0.extend(value.encode_utf16());
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
