//! Host-granted local font files for development rendering.
//!
//! `--font FILE` options are removed from the arguments before the command
//! runs. The first granted face's family serves every generic family and the
//! fallback; the other granted faces join per-cluster fallback.

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

/// Extract and validate explicit font grants before publishing configuration.
pub fn configure(args: Vec<OsString>) -> Result<Vec<OsString>, String> {
    let mut output = Vec::new();
    let mut paths = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            output.push(arg);
            output.extend(args);
            break;
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
        return Ok(output);
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
