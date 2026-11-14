//! Unicode algorithms shared by the engine's URL and JavaScript components.

mod normalize;
mod tables;

pub use normalize::{NormalizationForm, combining_class, is_nfc, nfc, normalize_utf16};
