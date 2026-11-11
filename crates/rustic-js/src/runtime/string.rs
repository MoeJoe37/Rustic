//! String storage (ADR 0015): one byte per code unit when every unit is
//! below 256, UTF-16 otherwise. Semantics are always UTF-16 code units.

use std::hash::{Hash, Hasher};

/// The contents of one JavaScript string.
#[derive(Clone, Debug)]
pub enum JsStr {
    /// Every code unit is below 256 and stored in one byte.
    Latin1(Box<[u8]>),
    /// At least one code unit is 256 or above.
    Utf16(Box<[u16]>),
}

impl JsStr {
    /// The empty string.
    pub fn empty() -> Self {
        JsStr::Latin1(Box::default())
    }

    /// A string from code units, choosing the compact form when possible.
    pub fn from_units(units: &[u16]) -> Self {
        if units.iter().all(|&u| u < 256) {
            // Every unit was checked to be below 256 just above.
            JsStr::Latin1(units.iter().map(|&u| u as u8).collect())
        } else {
            JsStr::Utf16(units.into())
        }
    }

    /// A string from Rust text (each `char` becomes one or two units).
    pub fn from_text(text: &str) -> Self {
        if text.chars().all(|c| u32::from(c) < 256) {
            // Every char was checked to be below 256 just above.
            JsStr::Latin1(text.chars().map(|c| u32::from(c) as u8).collect())
        } else {
            JsStr::Utf16(text.encode_utf16().collect())
        }
    }

    /// Length in UTF-16 code units.
    pub fn len(&self) -> usize {
        match self {
            JsStr::Latin1(b) => b.len(),
            JsStr::Utf16(u) => u.len(),
        }
    }

    /// Whether the string is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The code unit at `index`.
    pub fn unit(&self, index: usize) -> Option<u16> {
        match self {
            JsStr::Latin1(b) => b.get(index).map(|&b| u16::from(b)),
            JsStr::Utf16(u) => u.get(index).copied(),
        }
    }

    /// Every code unit in order.
    pub fn units(&self) -> impl Iterator<Item = u16> + '_ {
        let (latin, wide) = match self {
            JsStr::Latin1(b) => (Some(b.iter().map(|&b| u16::from(b))), None),
            JsStr::Utf16(u) => (None, Some(u.iter().copied())),
        };
        latin
            .into_iter()
            .flatten()
            .chain(wide.into_iter().flatten())
    }

    /// The code units as a vector.
    pub fn to_units(&self) -> Vec<u16> {
        self.units().collect()
    }

    /// Rust text, replacing lone surrogates with U+FFFD.
    pub fn to_text(&self) -> String {
        match self {
            JsStr::Latin1(b) => b.iter().map(|&b| char::from(b)).collect(),
            JsStr::Utf16(u) => String::from_utf16_lossy(u),
        }
    }

    /// The concatenation of `self` and `other`.
    pub fn concat(&self, other: &JsStr) -> JsStr {
        match (self, other) {
            (JsStr::Latin1(a), JsStr::Latin1(b)) => {
                let mut out = Vec::with_capacity(a.len() + b.len());
                out.extend_from_slice(a);
                out.extend_from_slice(b);
                JsStr::Latin1(out.into())
            }
            _ => JsStr::Utf16(self.units().chain(other.units()).collect()),
        }
    }

    /// Bytes of character storage (excluding the 24-byte handle).
    pub fn storage_bytes(&self) -> usize {
        match self {
            JsStr::Latin1(b) => b.len(),
            JsStr::Utf16(u) => u.len() * 2,
        }
    }

    /// Whether the string is a canonical array index: `0` or digits without
    /// a leading zero, at most 2³² − 2 (§6.1.7).
    pub fn array_index(&self) -> Option<u32> {
        let len = self.len();
        if len == 0 || len > 10 || (len > 1 && self.unit(0) == Some(u16::from(b'0'))) {
            return None;
        }
        let mut n: u64 = 0;
        for unit in self.units() {
            let digit = char::from_u32(u32::from(unit))?.to_digit(10)?;
            n = n * 10 + u64::from(digit);
        }
        u32::try_from(n).ok().filter(|&n| n != u32::MAX)
    }
}

impl PartialEq for JsStr {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (JsStr::Latin1(a), JsStr::Latin1(b)) => a == b,
            (JsStr::Utf16(a), JsStr::Utf16(b)) => a == b,
            // A UTF-16 string has a unit above 255 and a Latin-1 one has none.
            _ => false,
        }
    }
}

impl Eq for JsStr {}

impl Hash for JsStr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.len());
        for unit in self.units() {
            state.write_u16(unit);
        }
    }
}

impl PartialOrd for JsStr {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Code-unit order, as the `<` operator compares strings (§7.2.13).
impl Ord for JsStr {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.units().cmp(other.units())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_text_takes_one_byte_per_unit() {
        let s = JsStr::from_text("width: 100px; café");
        assert!(matches!(s, JsStr::Latin1(_)));
        assert_eq!(s.storage_bytes(), s.len());
        let wide = JsStr::from_text("€😀");
        assert!(matches!(wide, JsStr::Utf16(_)));
        assert_eq!(wide.len(), 3);
        assert_eq!(wide.storage_bytes(), 6);
        // A tag and a boxed slice: three words (24 bytes on 64-bit targets).
        assert_eq!(
            std::mem::size_of::<JsStr>(),
            3 * std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn representations_compare_by_units() {
        let a = JsStr::from_units(&[0x61, 0xE9]);
        assert_eq!(a, JsStr::from_text("aé"));
        assert_eq!(a.concat(&JsStr::from_text("€")).to_text(), "aé€");
        assert!(JsStr::from_text("a") < JsStr::from_text("b"));
        // Code-unit order, not code-point order: a surrogate sorts below U+FFFF.
        assert!(JsStr::from_text("😀") < JsStr::from_text("\u{FFFF}"));
        let lone = JsStr::from_units(&[0xD800]);
        assert_eq!(lone.to_text(), "\u{FFFD}");
    }

    #[test]
    fn canonical_array_indices() {
        for (text, index) in [
            ("0", Some(0)),
            ("42", Some(42)),
            ("4294967294", Some(4_294_967_294)),
            ("4294967295", None),
            ("01", None),
            ("", None),
            ("-1", None),
            ("1.0", None),
            ("99999999999", None),
        ] {
            assert_eq!(JsStr::from_text(text).array_index(), index, "{text}");
        }
    }
}
