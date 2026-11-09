//! Rustic's original test fonts (CC0, see `fonts/LICENSE.md`), embedded so
//! reference tests and headless dumps have deterministic metrics.

use crate::database::FontDatabase;
use crate::face::FontLimits;

/// The family every bundled face belongs to.
pub const TEST_FAMILY: &str = "Rustic Test";

/// `(file name, bytes)` for every bundled face.
pub const TEST_FONTS: [(&str, &[u8]); 4] = [
    (
        "RusticTest-Regular.ttf",
        include_bytes!("../fonts/RusticTest-Regular.ttf"),
    ),
    (
        "RusticTest-Bold.ttf",
        include_bytes!("../fonts/RusticTest-Bold.ttf"),
    ),
    (
        "RusticTest-Italic.ttf",
        include_bytes!("../fonts/RusticTest-Italic.ttf"),
    ),
    (
        "RusticTestCondensed-Regular.ttf",
        include_bytes!("../fonts/RusticTestCondensed-Regular.ttf"),
    ),
];

/// Generic families mapped to [`TEST_FAMILY`] by [`test_database`].
pub const GENERIC_FAMILIES: [&str; 13] = [
    "serif",
    "sans-serif",
    "monospace",
    "cursive",
    "fantasy",
    "system-ui",
    "ui-serif",
    "ui-sans-serif",
    "ui-monospace",
    "ui-rounded",
    "math",
    "emoji",
    "fangsong",
];

/// A database holding every bundled face, with every generic family and
/// the fallback mapped to [`TEST_FAMILY`].
pub fn test_database() -> FontDatabase {
    let mut database = FontDatabase::new();
    for (name, bytes) in TEST_FONTS {
        if let Err(error) = database.load(bytes.to_vec(), &FontLimits::default()) {
            // The bytes are fixed at build time and covered by tests.
            unreachable!("bundled font {name} failed to parse: {error}");
        }
    }
    for generic in GENERIC_FAMILIES {
        database.set_generic_family(generic, TEST_FAMILY);
    }
    database.set_fallback_family(TEST_FAMILY);
    database
}
