//! Host-granted font files for rendering.
//!
//! `--font FILE` options are removed from the arguments before the command
//! runs. The first granted face's family serves every generic family and the
//! fallback; the other granted faces join per-cluster fallback.
//!
//! `--system-fonts` (the default for `view`) grants the platform's own
//! fonts instead (platform.native, ADR 0074 and 0078): fontconfig's answers
//! on Linux and the BSDs, DirectWrite's on Windows and Core Text's on macOS
//! (`rustic_native`), then the files [`rustic_text::native`] lists, each CSS generic family mapped
//! to the family the platform uses for it, plus fallback faces for other
//! scripts. The broker reads them (a renderer opens no files) within
//! [`SYSTEM_FONT_BYTES`]; members of font collections are extracted.

use rustic_text::{FontDatabase, FontLimits};
use std::{
    ffi::OsString,
    fs::File,
    io::Read,
    path::PathBuf,
    sync::{Arc, OnceLock},
};

/// Total bytes read from, and decoded for, all granted font files.
const MAX_TOTAL_FONT_BYTES: usize = 128 * 1024 * 1024;
/// Most `--font` grants per run.
const MAX_FONT_GRANTS: usize = 64;

static CONFIG: OnceLock<Arc<FontDatabase>> = OnceLock::new();

/// Most bytes of system font files granted in one run.
pub const SYSTEM_FONT_BYTES: usize = 64 * 1024 * 1024;

/// Extract and validate explicit font grants before publishing configuration.
pub fn configure(args: Vec<OsString>) -> Result<Vec<OsString>, String> {
    let mut output = Vec::new();
    let mut paths = Vec::new();
    // `view` shows pages to a person: their platform's fonts by default.
    let mut system = args.first().is_some_and(|arg| arg == "view");
    let mut asked_system = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            output.push(arg);
            output.extend(args);
            break;
        }
        if arg == "--system-fonts" {
            system = true;
            asked_system = true;
            continue;
        }
        if arg == "--test-fonts" {
            system = false;
            continue;
        }
        if arg == "--font" {
            let value = args
                .next()
                .filter(|a| !a.to_string_lossy().starts_with("--"))
                .ok_or("--font requires a local font file")?;
            if paths.len() >= MAX_FONT_GRANTS {
                return Err("at most 64 explicit font files are accepted".into());
            }
            paths.push(PathBuf::from(value));
        } else {
            output.push(arg);
        }
    }
    if paths.is_empty() {
        if system {
            let (files, families) = system_fonts();
            if files.is_empty() {
                if asked_system {
                    return Err("--system-fonts: no usable system fonts were found".into());
                }
                eprintln!("rustic: no usable system fonts were found; using the test fonts");
            } else {
                configure_granted(files, families)?;
            }
        }
        return Ok(output);
    }
    if asked_system {
        return Err("give either --font files or --system-fonts".into());
    }
    let limits = FontLimits::default();
    let mut files = Vec::new();
    let mut input = 0usize;
    for path in paths {
        let mut bytes = Vec::new();
        let file = File::open(&path).map_err(|e| format!("font file: {e}"))?;
        let remaining = MAX_TOTAL_FONT_BYTES
            .saturating_sub(input)
            .min(limits.max_font_bytes);
        if file.metadata().map_err(|e| e.to_string())?.len() > remaining as u64 {
            return Err("font input byte limit exceeded".into());
        }
        file.take(remaining as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > remaining {
            return Err("font input byte limit exceeded".into());
        }
        input += bytes.len();
        files.push(bytes);
    }
    configure_bytes(files)?;
    Ok(output)
}

/// The raw bytes of the granted font files, which a broker hands to its
/// renderer (a renderer opens no files).
pub fn granted_bytes() -> &'static [Vec<u8>] {
    GRANTED.get().map_or(&[], Vec::as_slice)
}

static GRANTED: OnceLock<Vec<Vec<u8>>> = OnceLock::new();

/// The `generic=family` mappings for the granted fonts (system fonts).
pub fn granted_families() -> &'static [String] {
    FAMILIES.get().map_or(&[], Vec::as_slice)
}

static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();

/// Configure fonts from granted file contents and `generic=family`
/// mappings (the broker read the files; a renderer gets both over IPC).
/// Without mappings the first face's family serves every generic.
pub fn configure_granted(files: Vec<Vec<u8>>, families: Vec<String>) -> Result<(), String> {
    if families.is_empty() {
        return configure_bytes(files);
    }
    if files.is_empty()
        || files.len() > MAX_FONT_GRANTS
        || files.iter().map(Vec::len).sum::<usize>() > MAX_TOTAL_FONT_BYTES
        || families.len() > rustic_text::native::GENERICS.len()
    {
        return Err("font input byte limit exceeded".into());
    }
    let limits = FontLimits::default();
    let mut db = FontDatabase::new();
    for bytes in &files {
        db.load(bytes.clone(), &limits).map_err(|e| e.to_string())?;
    }
    let mut fallback = None;
    for mapping in &families {
        let (generic, family) = mapping
            .split_once('=')
            .filter(|(generic, _)| rustic_text::native::GENERICS.contains(generic))
            .ok_or("malformed font family mapping")?;
        if !db.faces().iter().any(|face| {
            face.families()
                .iter()
                .any(|f| f.eq_ignore_ascii_case(family))
        }) {
            return Err("a font family mapping names no granted face".into());
        }
        db.set_generic_family(generic, family);
        if generic == "sans-serif" {
            fallback = Some(family.to_owned());
        }
    }
    let fallback = fallback.unwrap_or_else(|| db.faces()[0].family().to_owned());
    db.set_fallback_family(&fallback);
    CONFIG
        .set(Arc::new(db))
        .map_err(|_| "fonts already configured")?;
    let _ = GRANTED.set(files);
    let _ = FAMILIES.set(families);
    Ok(())
}

/// Configure fonts from file contents (the broker read them).
pub fn configure_bytes(files: Vec<Vec<u8>>) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }
    if files.len() > MAX_FONT_GRANTS
        || files.iter().map(Vec::len).sum::<usize>() > MAX_TOTAL_FONT_BYTES
    {
        return Err("font input byte limit exceeded".into());
    }
    let limits = FontLimits::default();
    let mut db = FontDatabase::new();
    let mut decoded = 0usize;
    for bytes in &files {
        let index = db.load(bytes.clone(), &limits).map_err(|e| e.to_string())?;
        decoded = decoded
            .checked_add(db.faces()[index].byte_len())
            .filter(|&n| n <= MAX_TOTAL_FONT_BYTES)
            .ok_or("decoded font byte limit exceeded")?;
    }
    let family = db.faces()[0].family().to_owned();
    for generic in [
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
        "emoji",
        "math",
        "fangsong",
    ] {
        db.set_generic_family(generic, &family);
    }
    db.set_fallback_family(&family);
    CONFIG
        .set(Arc::new(db))
        .map_err(|_| "fonts already configured")?;
    let _ = GRANTED.set(files);
    Ok(())
}
pub fn explicit() -> Option<Arc<FontDatabase>> {
    CONFIG.get().cloned()
}

/// The granted fonts, or the bundled test fonts without grants.
pub fn database() -> &'static FontDatabase {
    static DEFAULT: OnceLock<FontDatabase> = OnceLock::new();
    CONFIG.get().map_or_else(
        || DEFAULT.get_or_init(rustic_text::bundled::test_database),
        Arc::as_ref,
    )
}

/// Expand a native path: `%WINDIR%` and a leading `~`.
fn expand(root: &str) -> Option<PathBuf> {
    if let Some(rest) = root.strip_prefix("%WINDIR%") {
        let windir = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot"))?;
        return Some(PathBuf::from(format!(
            "{}{rest}",
            PathBuf::from(windir).to_string_lossy()
        )));
    }
    if let Some(rest) = root.strip_prefix('~') {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        return Some(PathBuf::from(format!(
            "{}{rest}",
            PathBuf::from(home).to_string_lossy()
        )));
    }
    Some(PathBuf::from(root))
}

/// The first existing regular file named `name` under the native roots.
fn find_native(roots: &[PathBuf], name: &str) -> Option<PathBuf> {
    roots
        .iter()
        .map(|root| root.join(name))
        .find(|path| std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) || path.is_file())
}

/// Languages whose writing systems get a fontconfig fallback face, most
/// widely used first so the budget keeps them.
const FALLBACK_LANGUAGES: [&str; 13] = [
    "ar", "he", "ru", "el", "hi", "bn", "th", "zh-cn", "ja", "ko", "hy", "ka", "am",
];

/// Ask fontconfig for the file it would use for `pattern`: the platform's
/// own answer on Linux and the BSDs. `None` without fontconfig.
fn fontconfig(pattern: &str) -> Option<PathBuf> {
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        return None;
    }
    let output = std::process::Command::new("fc-match")
        .args(["--format=%{file}", pattern])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.len() > 4096 {
        return None;
    }
    let path = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
    let font = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["ttf", "otf", "ttc", "otc", "woff", "woff2"]
            .iter()
            .any(|known| e.eq_ignore_ascii_case(known))
    });
    (path.is_absolute() && font && path.is_file()).then_some(path)
}

/// Most fallback files taken from the platform font service's list.
const MAX_SERVICE_FALLBACKS: usize = 8;

/// The files the platform font service (DirectWrite on Windows, Core Text
/// on macOS; ADR 0078) uses for a generic family's first installed
/// family, regular first, then bold, italic and bold italic where the
/// family has them. Empty without a service or an installed family.
fn font_service(candidate: &rustic_text::native::Candidate) -> Vec<PathBuf> {
    use rustic_native::Style;
    let styled = |find: &dyn Fn(Style) -> Option<PathBuf>| -> Vec<PathBuf> {
        let Some(regular) = find(Style::REGULAR) else {
            return Vec::new();
        };
        let mut paths = vec![regular];
        for style in &Style::ALL[1..] {
            if let Some(path) = find(*style).filter(|path| !paths.contains(path)) {
                paths.push(path);
            }
        }
        paths
    };
    if candidate.generic == "system-ui" {
        let paths = styled(&rustic_native::system_ui_file);
        if !paths.is_empty() {
            return paths;
        }
    }
    candidate
        .families
        .iter()
        .map(|family| styled(&|style| rustic_native::font_file(family, style)))
        .find(|paths| !paths.is_empty())
        .unwrap_or_default()
}

/// One font file's faces as standalone font data: collection members
/// (up to four, the first family's styles) extracted, others as they are.
fn faces_of(bytes: Vec<u8>, limits: &FontLimits, members: usize) -> Vec<(String, Vec<u8>)> {
    let Some(count) = rustic_text::collection_len(&bytes) else {
        return rustic_text::Face::parse(bytes.clone(), limits)
            .map(|face| vec![(face.family().to_owned(), bytes)])
            .unwrap_or_default();
    };
    let mut faces: Vec<(String, Vec<u8>)> = Vec::new();
    for index in 0..count.min(16) {
        let Ok(member) = rustic_text::collection_face(&bytes, index, limits) else {
            continue;
        };
        let Ok(face) = rustic_text::Face::parse(member.clone(), limits) else {
            continue;
        };
        let family = face.family().to_owned();
        if faces.first().is_some_and(|(first, _)| *first != family) {
            continue;
        }
        faces.push((family, member));
        if faces.len() >= members {
            break;
        }
    }
    faces
}

/// The platform's fonts for the CSS generic families and fallback, read
/// within [`SYSTEM_FONT_BYTES`] and [`MAX_FONT_GRANTS`]: the file contents
/// and `generic=family` mappings. Unreadable or unparsable files are
/// skipped; an empty result means none was usable.
pub fn system_fonts() -> (Vec<Vec<u8>>, Vec<String>) {
    let native = rustic_text::native::native();
    let roots: Vec<PathBuf> = native.roots.iter().filter_map(|r| expand(r)).collect();
    let limits = FontLimits::default();
    let mut files: Vec<Vec<u8>> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut families: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut take = |path: &PathBuf, members: usize, files: &mut Vec<Vec<u8>>| -> Option<String> {
        if seen.contains(path) || files.len() >= MAX_FONT_GRANTS {
            return None;
        }
        seen.push(path.clone());
        let size = usize::try_from(std::fs::metadata(path).ok()?.len()).ok()?;
        if size > limits.max_font_bytes || total.saturating_add(size) > SYSTEM_FONT_BYTES {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        let mut first = None;
        for (family, face) in faces_of(bytes, &limits, members) {
            if files.len() >= MAX_FONT_GRANTS || total + face.len() > SYSTEM_FONT_BYTES {
                break;
            }
            total += face.len();
            files.push(face);
            first.get_or_insert(family);
        }
        first
    };
    for candidate in native.generics {
        // fontconfig's choice first, where the platform has it.
        let asked: Vec<PathBuf> = ["", ":bold", ":italic", ":bold:italic"]
            .iter()
            .filter_map(|style| fontconfig(&format!("{}{style}", candidate.generic)))
            .collect();
        // Then the platform font service (DirectWrite, Core Text).
        let asked = if asked.is_empty() {
            font_service(candidate)
        } else {
            asked
        };
        let sets: Vec<Vec<PathBuf>> = if asked.is_empty() {
            candidate
                .sets
                .iter()
                .filter_map(|set| {
                    let regular = find_native(&roots, set[0])?;
                    let mut paths = vec![regular];
                    paths.extend(set[1..].iter().filter_map(|name| find_native(&roots, name)));
                    Some(paths)
                })
                .take(1)
                .collect()
        } else {
            vec![asked]
        };
        for set in sets {
            let mut family = None;
            for path in &set {
                let got = take(path, 4, &mut files);
                if family.is_none() {
                    family = got;
                }
            }
            if let Some(family) = family {
                families.push(format!("{}={family}", candidate.generic));
            }
        }
    }
    for path in rustic_native::fallback_files(MAX_SERVICE_FALLBACKS) {
        take(&path, 1, &mut files);
    }
    for name in native.fallbacks {
        if let Some(path) = find_native(&roots, name) {
            take(&path, 1, &mut files);
        }
    }
    // fontconfig names a face per writing system, so text the generic
    // families do not cover (Hebrew beside a Latin-only sans-serif) falls
    // back to a face the platform chose for it.
    for language in FALLBACK_LANGUAGES {
        if let Some(path) = fontconfig(&format!("sans-serif:lang={language}")) {
            take(&path, 1, &mut files);
        }
    }
    if !files.is_empty() {
        // Generics the platform has no family of its own for borrow one.
        for generic in rustic_text::native::GENERICS {
            let mapped = |g: &str| {
                families
                    .iter()
                    .find(|m| m.split_once('=').is_some_and(|(k, _)| k == g))
                    .cloned()
            };
            if mapped(generic).is_none() {
                if let Some(borrowed) = mapped(rustic_text::native::borrowed(generic)) {
                    let family = borrowed
                        .split_once('=')
                        .map(|(_, f)| f.to_owned())
                        .unwrap_or_default();
                    families.push(format!("{generic}={family}"));
                }
            }
        }
        if families.is_empty() {
            files.clear();
        }
    }
    (files, families)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_fonts_map_every_generic_within_the_budget() {
        let (files, families) = system_fonts();
        if files.is_empty() {
            // A machine without fonts: nothing is granted or mapped.
            assert!(families.is_empty());
            return;
        }
        assert!(files.len() <= MAX_FONT_GRANTS);
        assert!(files.iter().map(Vec::len).sum::<usize>() <= SYSTEM_FONT_BYTES);
        for generic in rustic_text::native::GENERICS {
            assert!(
                families
                    .iter()
                    .any(|m| m.starts_with(&format!("{generic}="))),
                "{generic} unmapped in {families:?}"
            );
        }
        // Every mapped family names a granted face.
        let limits = FontLimits::default();
        let names: Vec<String> = files
            .iter()
            .filter_map(|bytes| rustic_text::Face::parse(bytes.clone(), &limits).ok())
            .flat_map(|face| face.families().to_vec())
            .collect();
        for mapping in &families {
            let family = mapping.split_once('=').unwrap().1;
            assert!(
                names.iter().any(|n| n.eq_ignore_ascii_case(family)),
                "{mapping}"
            );
        }
    }

    #[test]
    fn native_paths_expand_and_unknown_files_are_not_found() {
        assert_eq!(
            expand("/usr/share/fonts"),
            Some(PathBuf::from("/usr/share/fonts"))
        );
        let roots = vec![std::env::temp_dir()];
        assert_eq!(find_native(&roots, "rustic-no-such-font.ttf"), None);
    }
}
