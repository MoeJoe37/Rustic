//! HTML directionality reads `Bidi_Class` from `rustic-text`; IDNA's Bidi
//! Rule keeps its own generated copy in `rustic-url`. Both come from the same
//! pinned Unicode version, and this test keeps them from drifting (TD-29).

use rustic_text::unicode::{self, StrongDirection};
use rustic_url::idna;

#[test]
fn text_and_idna_strong_directions_agree_for_every_scalar_value() {
    let mut mismatches = Vec::new();
    for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
        let text = unicode::strong_direction(c);
        let url = idna::strong_direction(c).map(|d| match d {
            idna::StrongDirection::Ltr => StrongDirection::Ltr,
            idna::StrongDirection::Rtl => StrongDirection::Rtl,
        });
        if text != url && mismatches.len() < 16 {
            mismatches.push((u32::from(c), text, url));
        }
    }
    assert!(mismatches.is_empty(), "first mismatches: {mismatches:x?}");
}
