//! Host-granted local font files for development rendering.
use rustic_text::{FontDatabase, FontLimits};
use std::{
    ffi::OsString,
    fs::File,
    io::Read,
    path::PathBuf,
    sync::{Arc, OnceLock},
};
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
            if paths.len() >= 64 {
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
    let mut db = FontDatabase::new();
    let mut input = 0usize;
    let mut decoded = 0usize;
    for path in paths {
        let mut bytes = Vec::new();
        let file = File::open(&path).map_err(|e| format!("font file: {e}"))?;
        let remaining = (128 * 1024 * 1024usize)
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
        let index = db.load(bytes, &limits).map_err(|e| e.to_string())?;
        decoded = decoded
            .checked_add(db.faces()[index].byte_len())
            .filter(|&n| n <= 128 * 1024 * 1024)
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
    Ok(output)
}
pub fn explicit() -> Option<Arc<FontDatabase>> {
    CONFIG.get().cloned()
}
pub fn database() -> &'static FontDatabase {
    static DEFAULT: OnceLock<FontDatabase> = OnceLock::new();
    CONFIG.get().map_or_else(
        || DEFAULT.get_or_init(rustic_text::bundled::test_database),
        Arc::as_ref,
    )
}
