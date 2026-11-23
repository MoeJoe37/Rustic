//! The platform font services answer with real, parsable font files on
//! the CI's Windows and macOS runners (x64 and ARM64), and with nothing
//! on other platforms (ADR 0078).

use rustic_native::{Style, fallback_files, font_file, font_service, system_ui_file};
use rustic_text::{Face, FontLimits, collection_face, collection_len};

/// The family names of the faces in a font file (every member of a
/// collection).
fn families(path: &std::path::Path) -> Vec<String> {
    let bytes = std::fs::read(path).expect("a readable font file");
    let limits = FontLimits::default();
    let faces = match collection_len(&bytes) {
        Some(count) => (0..count)
            .filter_map(|index| collection_face(&bytes, index, &limits).ok())
            .collect(),
        None => vec![bytes],
    };
    faces
        .into_iter()
        .filter_map(|face| Face::parse(face, &limits).ok())
        .map(|face| face.family().to_owned())
        .collect()
}

/// A family every Windows and macOS release ships, with all four faces.
const SHIPPED: &str = if cfg!(windows) { "Arial" } else { "Helvetica" };

#[test]
fn the_font_service_names_parsable_files_of_the_family_asked() {
    if font_service().is_none() {
        assert_eq!(font_file(SHIPPED, Style::REGULAR), None);
        assert_eq!(system_ui_file(Style::REGULAR), None);
        assert!(fallback_files(8).is_empty());
        return;
    }
    let regular = font_file(SHIPPED, Style::REGULAR).expect("the shipped family");
    assert!(regular.is_absolute(), "{}", regular.display());
    let names = families(&regular);
    assert!(
        names.iter().any(|name| name.eq_ignore_ascii_case(SHIPPED)),
        "{} holds {names:?}",
        regular.display()
    );
    let bold = font_file(SHIPPED, Style::ALL[1]).expect("a real bold face");
    if cfg!(windows) {
        // Arial's faces are separate files; Helvetica's share a collection.
        assert_ne!(bold, regular);
    }
}

#[test]
fn unknown_families_are_not_substituted() {
    assert_eq!(
        font_file("Rustic No Such Family 7f3a", Style::REGULAR),
        None
    );
}

#[test]
fn the_system_ui_font_and_fallbacks_are_local_font_files() {
    if font_service().is_none() {
        return;
    }
    let ui = system_ui_file(Style::REGULAR).expect("the system UI font");
    // The host parses it; here it is enough that the service named a font.
    let extension = ui.extension().and_then(|e| e.to_str()).unwrap_or_default();
    assert!(
        ["ttf", "ttc", "otf", "otc"]
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known)),
        "{}",
        ui.display()
    );
    assert!(rustic_native::system_ui_family().is_some());
    let fallbacks = fallback_files(8);
    assert!(fallbacks.len() <= 8);
    if cfg!(target_os = "macos") {
        assert!(!fallbacks.is_empty(), "Core Text lists a cascade");
    }
}
