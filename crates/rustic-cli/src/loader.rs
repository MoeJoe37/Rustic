//! Local-only bounded HTML loading for development commands.
//!
//! The command line grants exactly one file path. Page content may name other
//! files only as stylesheets for `dump-style`, which load solely from that
//! file's directory tree (see [`crate::stylesheets`]). Nothing here performs
//! network access, script execution, other subresource loading or encoding
//! detection. Input is read in fixed
//! chunks, so the whole file is never buffered, and every stage is bounded:
//! file bytes, parser token/document/diagnostic limits and dump output bytes.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use rustic_base::CancellationToken;
use rustic_base::EscapedText;
use rustic_css::CssParser;
use rustic_dom::DumpError;
use rustic_html::{DocumentMode, HtmlDocumentParser, ParseError, ParsedDocument, ParserLimits};
use rustic_style::{
    ComputeContext, FetchError, MatchContext, NoFetch, SheetStatus, StyleLoadLimits, StyleLoader,
    StylesheetFetcher, document_cascade_with, dump_computed_styles, style_tree,
};

use crate::stylesheets::{LocalStylesheetFetcher, file_url};

/// Default maximum file size, matching the tokenizer's default input budget.
pub const DEFAULT_MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
/// Default bytes read from the file and fed to the parser per step.
pub const DEFAULT_READ_CHUNK_BYTES: usize = 64 * 1024;
/// Default maximum UTF-8 bytes of dump output.
pub const DEFAULT_MAX_DUMP_BYTES: usize = 64 * 1024 * 1024;

/// Independent budgets for one local load and its diagnostic dump.
#[derive(Clone, Copy, Debug)]
pub struct LoaderLimits {
    /// Maximum file bytes; also becomes the tokenizer's raw input budget.
    pub max_file_bytes: usize,
    /// Read buffer size; changing it never changes the resulting tree.
    pub read_chunk_bytes: usize,
    /// Maximum UTF-8 bytes in the deterministic tree dump.
    pub max_dump_bytes: usize,
    /// Token, document and diagnostic limits for the document parser.
    pub parser: ParserLimits,
    /// External stylesheet budgets for `dump-style`.
    pub stylesheets: StyleLoadLimits,
}

impl Default for LoaderLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            read_chunk_bytes: DEFAULT_READ_CHUNK_BYTES,
            max_dump_bytes: DEFAULT_MAX_DUMP_BYTES,
            parser: ParserLimits::default(),
            stylesheets: StyleLoadLimits::default(),
        }
    }
}

/// Why a local document could not be loaded or dumped.
#[derive(Debug)]
pub enum LoadError {
    /// The argument is empty or looks like a URL rather than a local path.
    NotLocalPath,
    /// A configured limit is zero.
    InvalidLimits,
    /// The path does not name a regular file (directory, device, FIFO, ...).
    NotRegularFile,
    /// The file has more bytes than the configured limit.
    FileTooLarge {
        /// The configured byte limit.
        limit: usize,
    },
    /// The file could not be inspected, opened or read.
    Io(io::Error),
    /// Cancellation was requested between read steps.
    Cancelled,
    /// The parser rejected the input or exceeded one of its budgets.
    Parse(ParseError),
    /// The tree dump would exceed its output limit.
    Dump(DumpError),
    /// Principal box construction failed or exceeded its limits.
    Layout(rustic_layout::LayoutError),
    /// The block-only reference layout failed.
    Flow(rustic_layout::FlowError),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLocalPath => f.write_str(
                "expected a local file path; URLs and network loading are not supported",
            ),
            Self::InvalidLimits => f.write_str("loader limits must be nonzero"),
            Self::NotRegularFile => f.write_str("path is not a regular file"),
            Self::FileTooLarge { limit } => write!(f, "file exceeds the {limit}-byte input limit"),
            Self::Io(error) => write!(f, "cannot read file: {error}"),
            Self::Cancelled => f.write_str("load cancelled"),
            Self::Parse(error) => error.fmt(f),
            Self::Dump(error) => error.fmt(f),
            Self::Layout(error) => error.fmt(f),
            Self::Flow(error) => error.fmt(f),
        }
    }
}

impl Error for LoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::Dump(error) => Some(error),
            Self::Layout(error) => Some(error),
            Self::Flow(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ParseError> for LoadError {
    fn from(value: ParseError) -> Self {
        Self::Parse(value)
    }
}

/// Accept a command-line argument only as a local filesystem path.
///
/// Anything starting with a URL scheme of two or more characters followed by
/// `:` (for example `https:`, `file:` or `data:`) is rejected. Single-letter
/// prefixes remain paths so Windows drive letters such as `C:\page.html` work;
/// a relative file whose name contains a colon can be written as `./a:b.html`.
pub fn local_path(argument: &OsStr) -> Result<PathBuf, LoadError> {
    if argument.is_empty() || looks_like_url(argument) {
        return Err(LoadError::NotLocalPath);
    }
    Ok(PathBuf::from(argument))
}

fn looks_like_url(argument: &OsStr) -> bool {
    let bytes = argument.as_encoded_bytes();
    let Some(colon) = bytes.iter().position(|&byte| byte == b':') else {
        return false;
    };
    let scheme = &bytes[..colon];
    scheme.len() >= 2
        && scheme[0].is_ascii_alphabetic()
        && scheme
            .iter()
            .all(|&byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

/// Read one regular file in bounded chunks and build its inert document.
///
/// The file is checked before opening (so FIFOs and devices are never opened)
/// and again through the open handle. Its size is enforced while reading, so a
/// file that grows after inspection still cannot exceed the limit. Any failure
/// returns no partial document.
pub fn load_document(
    path: &Path,
    limits: LoaderLimits,
    cancellation: CancellationToken,
) -> Result<ParsedDocument, LoadError> {
    if limits.max_file_bytes == 0 || limits.read_chunk_bytes == 0 || limits.max_dump_bytes == 0 {
        return Err(LoadError::InvalidLimits);
    }
    check_regular_file(fs::metadata(path).map_err(LoadError::Io)?, limits)?;
    let mut file = File::open(path).map_err(LoadError::Io)?;
    check_regular_file(file.metadata().map_err(LoadError::Io)?, limits)?;

    let mut parser_limits = limits.parser;
    parser_limits.tokenizer.max_input_bytes = limits.max_file_bytes;
    let mut parser = HtmlDocumentParser::new(parser_limits, cancellation.clone())?;

    let mut buffer = vec![0; limits.read_chunk_bytes.min(limits.max_file_bytes)];
    let mut total = 0usize;
    loop {
        cancellation.check().map_err(|_| LoadError::Cancelled)?;
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(LoadError::Io(error)),
        };
        total = total
            .checked_add(read)
            .filter(|&total| total <= limits.max_file_bytes)
            .ok_or(LoadError::FileTooLarge {
                limit: limits.max_file_bytes,
            })?;
        parser.feed(&buffer[..read])?;
    }
    cancellation.check().map_err(|_| LoadError::Cancelled)?;
    Ok(parser.finish()?)
}

fn check_regular_file(metadata: fs::Metadata, limits: LoaderLimits) -> Result<(), LoadError> {
    if !metadata.is_file() {
        return Err(LoadError::NotRegularFile);
    }
    if metadata.len() > limits.max_file_bytes as u64 {
        return Err(LoadError::FileTooLarge {
            limit: limits.max_file_bytes,
        });
    }
    Ok(())
}

/// Deterministic preorder tree dump of the whole document, never HTML.
pub fn dump_dom(
    parsed: &ParsedDocument,
    _path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    let document = parsed.document();
    document
        .dump_tree(document.root(), limits.max_dump_bytes)
        .map_err(LoadError::Dump)
}

fn status_text(status: SheetStatus) -> &'static str {
    match status {
        SheetStatus::Applied => "applied",
        SheetStatus::NotCss => "skipped: not CSS",
        SheetStatus::UnsupportedCondition => "skipped: layer() and supports() not supported",
        SheetStatus::Disabled => "skipped: alternate or disabled",
        SheetStatus::InvalidUrl => "skipped: invalid URL",
        SheetStatus::Cycle => "skipped: import cycle",
        SheetStatus::DepthLimit => "skipped: import depth limit",
        SheetStatus::LoadLimit => "skipped: stylesheet budget exhausted",
        SheetStatus::Fetch(FetchError::Blocked) => {
            "skipped: blocked (only files beside the document load)"
        }
        SheetStatus::Fetch(FetchError::NotFound) => "skipped: not found",
        SheetStatus::Fetch(FetchError::TooLarge) => "skipped: stylesheet budget exceeded",
        SheetStatus::Fetch(FetchError::Failed) => "skipped: unreadable",
        SheetStatus::Fetch(FetchError::Cancelled) => "skipped: cancelled",
        SheetStatus::ResourceLimit => "skipped: CSS parser limit",
    }
}

fn push_diagnostics(out: &mut String, indent: &str, diagnostics: &[rustic_style::StyleDiagnostic]) {
    for diagnostic in diagnostics {
        let range = diagnostic.span.range();
        out.push_str(&format!(
            "{indent}  dropped {:?} at bytes {}..{}\n",
            diagnostic.kind, range.start, range.end
        ));
    }
}

/// Deterministic computed-style dump of the whole document: a line per
/// `<style>` and `<link rel=stylesheet>` element with its status, its
/// `@import`s and dropped constructs, then every element with its non-initial
/// computed values (see [`rustic_style::dump_computed_styles`]). Uses the
/// HTML user-agent sheet and a 1280×720 viewport.
///
/// External stylesheets load only from files inside the directory that
/// contains `path` (see [`LocalStylesheetFetcher`]), within
/// [`LoaderLimits::stylesheets`]. Hrefs and import URLs are printed as
/// written, escaped, so the dump never depends on where the files live.
pub fn dump_style(
    parsed: &ParsedDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    let document = parsed.document();
    let (styles, sheets) = compute_styles(parsed, path, limits)?;
    let mut out = String::new();
    for (index, sheet) in sheets.iter().enumerate() {
        let status = status_text(sheet.status);
        match &sheet.href {
            Some(href) => out.push_str(&format!(
                "@link {} {}: {status}\n",
                index + 1,
                EscapedText(href)
            )),
            None => out.push_str(&format!("@style {}: {status}\n", index + 1)),
        }
        push_diagnostics(&mut out, "", &sheet.diagnostics);
        for import in &sheet.imports {
            let indent = "  ".repeat(import.depth);
            out.push_str(&format!(
                "{indent}@import {}: {}\n",
                EscapedText(&import.url),
                status_text(import.status)
            ));
            push_diagnostics(&mut out, &indent, &import.diagnostics);
        }
        if out.len() > limits.max_dump_bytes {
            return Err(LoadError::Dump(DumpError::OutputLimit));
        }
    }
    let remaining = limits
        .max_dump_bytes
        .checked_sub(out.len())
        .ok_or(LoadError::Dump(DumpError::OutputLimit))?;
    let elements = dump_computed_styles(document, &styles, remaining)
        .ok_or(LoadError::Dump(DumpError::OutputLimit))?;
    out.push_str(&elements);
    Ok(out)
}

/// Share the same local stylesheet policy and M2 reference cascade in all dumps.
fn compute_styles(
    parsed: &ParsedDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<
    (
        Vec<rustic_style::ElementStyle>,
        Vec<rustic_style::StyleElementSheet>,
    ),
    LoadError,
> {
    let document = parsed.document();
    let node_error = |error| LoadError::Dump(DumpError::Node(error));
    let parser = CssParser::default();
    let canonical = fs::canonicalize(path).map_err(LoadError::Io)?;
    let document_url = file_url(&canonical);
    let mut local = canonical.parent().and_then(|root| {
        LocalStylesheetFetcher::new(
            root.to_path_buf(),
            limits.read_chunk_bytes,
            CancellationToken::default(),
        )
    });
    let mut no_fetch = NoFetch;
    let fetcher: &mut dyn StylesheetFetcher = match local.as_mut() {
        Some(local) => local,
        None => &mut no_fetch,
    };
    let mut loader = StyleLoader::new(fetcher, limits.stylesheets);
    let (cascade, sheets) =
        document_cascade_with(document, &parser, document_url.as_ref(), &mut loader)
            .map_err(node_error)?;
    let mut cx = MatchContext::html();
    cx.quirks = parsed.mode() == DocumentMode::Quirks;
    let styles = style_tree(
        document,
        document.root(),
        &cascade,
        &parser,
        &cx,
        &ComputeContext::default(),
    )
    .map_err(node_error)?;
    Ok((styles, sheets))
}

/// Dump an initial principal-box snapshot, before anonymous/line box construction.
/// This has no layout positions, shaped text, images or paint output yet.
pub fn dump_boxes(
    parsed: &ParsedDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    box_tree(parsed, path, limits)?
        .dump(limits.max_dump_bytes)
        .map_err(LoadError::Layout)
}

fn box_tree(
    parsed: &ParsedDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<rustic_layout::BoxTree, LoadError> {
    let (styles, _) = compute_styles(parsed, path, limits)?;
    rustic_layout::BoxTree::build(
        parsed.document(),
        parsed.document().root(),
        &styles,
        layout_limits(limits),
        &CancellationToken::default(),
    )
    .map_err(LoadError::Layout)
}

fn layout_limits(limits: LoaderLimits) -> rustic_layout::LayoutLimits {
    rustic_layout::LayoutLimits {
        max_nodes: limits.parser.document.max_nodes,
        max_boxes: limits.parser.document.max_nodes,
        max_depth: limits.parser.document.max_depth,
        ..rustic_layout::LayoutLimits::default()
    }
}

/// Compute used block rectangles with adjoining-margin collapse for the static subset.
/// Unsupported formatting returns no partial output; this does not paint pixels.
pub fn dump_layout(
    parsed: &ParsedDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    let tree = box_tree(parsed, path, limits)?;
    let compute = ComputeContext::default();
    let viewport = rustic_layout::CssSize::new(compute.viewport_width, compute.viewport_height)
        .map_err(|error| LoadError::Flow(error.into()))?;
    let layout = rustic_layout::BlockLayout::compute(
        &tree,
        viewport,
        layout_limits(limits),
        &CancellationToken::default(),
    )
    .map_err(LoadError::Flow)?;
    layout.dump(limits.max_dump_bytes).map_err(LoadError::Flow)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct TempFile(PathBuf);

    impl TempFile {
        fn new(contents: &[u8]) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "rustic-loader-{}-{}.html",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::write(&path, contents).expect("write temporary fixture");
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    const SOURCE: &[u8] = b"<!doctype html><title>A &amp; B</title><p>one<p>two<div>three</div>";

    fn dump(file: &TempFile, limits: LoaderLimits) -> Result<String, LoadError> {
        let parsed = load_document(&file.0, limits, CancellationToken::default())?;
        dump_dom(&parsed, &file.0, limits)
    }

    #[test]
    fn rejects_urls_but_keeps_paths_and_drive_letters() {
        for url in [
            "https://example.com/",
            "file:///etc/passwd",
            "data:text/html,<p>",
            "about:blank",
            "javascript:alert(1)",
            "view-source:x",
            "",
        ] {
            assert!(
                matches!(local_path(OsStr::new(url)), Err(LoadError::NotLocalPath)),
                "{url:?}"
            );
        }
        for path in [
            "page.html",
            "./a:b.html",
            "../dir/page.html",
            "/tmp/page.html",
            "C:\\page.html",
            "c:/page.html",
            "1abc:x",
        ] {
            assert_eq!(
                local_path(OsStr::new(path)).expect(path),
                PathBuf::from(path)
            );
        }
    }

    #[test]
    fn read_chunk_size_does_not_change_the_tree() {
        let file = TempFile::new(SOURCE);
        let expected = dump(&file, LoaderLimits::default()).expect("load");
        assert!(expected.contains("\"three\""), "{expected}");
        for chunk in 1..=SOURCE.len() + 1 {
            let limits = LoaderLimits {
                read_chunk_bytes: chunk,
                ..LoaderLimits::default()
            };
            assert_eq!(
                dump(&file, limits).expect("load"),
                expected,
                "chunk {chunk}"
            );
        }
    }

    #[test]
    fn file_byte_limit_is_exact() {
        let file = TempFile::new(SOURCE);
        let exact = LoaderLimits {
            max_file_bytes: SOURCE.len(),
            read_chunk_bytes: 5,
            ..LoaderLimits::default()
        };
        assert!(dump(&file, exact).is_ok());
        let under = LoaderLimits {
            max_file_bytes: SOURCE.len() - 1,
            ..exact
        };
        assert!(matches!(
            dump(&file, under),
            Err(LoadError::FileTooLarge { limit }) if limit == SOURCE.len() - 1
        ));
    }

    #[test]
    fn dump_byte_limit_is_exact() {
        let file = TempFile::new(SOURCE);
        let size = dump(&file, LoaderLimits::default()).expect("load").len();
        let exact = LoaderLimits {
            max_dump_bytes: size,
            ..LoaderLimits::default()
        };
        assert_eq!(dump(&file, exact).expect("exact").len(), size);
        let under = LoaderLimits {
            max_dump_bytes: size - 1,
            ..exact
        };
        assert!(matches!(
            dump(&file, under),
            Err(LoadError::Dump(DumpError::OutputLimit))
        ));
    }

    #[test]
    fn parser_budgets_apply_to_loaded_files() {
        let file = TempFile::new(SOURCE);
        let mut limits = LoaderLimits::default();
        limits.parser.document.max_nodes = 4;
        assert!(matches!(
            dump(&file, limits),
            Err(LoadError::Parse(ParseError::Document(_)))
        ));
    }

    #[test]
    fn unsupported_markup_fails_without_a_partial_tree() {
        let file = TempFile::new(b"<p>before<?target data?>after");
        assert!(matches!(
            dump(&file, LoaderLimits::default()),
            Err(LoadError::Parse(ParseError::Unsupported(_)))
        ));
    }

    #[test]
    fn rejects_directories_missing_files_and_zero_limits() {
        let directory = std::env::temp_dir();
        assert!(matches!(
            load_document(
                &directory,
                LoaderLimits::default(),
                CancellationToken::default()
            ),
            Err(LoadError::NotRegularFile)
        ));
        let missing = directory.join("rustic-loader-definitely-missing.html");
        assert!(matches!(
            load_document(&missing, LoaderLimits::default(), CancellationToken::default()),
            Err(LoadError::Io(error)) if error.kind() == io::ErrorKind::NotFound
        ));
        let file = TempFile::new(SOURCE);
        for limits in [
            LoaderLimits {
                max_file_bytes: 0,
                ..LoaderLimits::default()
            },
            LoaderLimits {
                read_chunk_bytes: 0,
                ..LoaderLimits::default()
            },
            LoaderLimits {
                max_dump_bytes: 0,
                ..LoaderLimits::default()
            },
        ] {
            assert!(matches!(dump(&file, limits), Err(LoadError::InvalidLimits)));
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_endless_devices_before_opening() {
        assert!(matches!(
            load_document(
                Path::new("/dev/zero"),
                LoaderLimits::default(),
                CancellationToken::default()
            ),
            Err(LoadError::NotRegularFile)
        ));
    }

    #[test]
    fn cancellation_stops_before_reading() {
        let file = TempFile::new(SOURCE);
        let token = CancellationToken::default();
        token.cancel();
        assert!(matches!(
            load_document(&file.0, LoaderLimits::default(), token),
            Err(LoadError::Cancelled)
        ));
    }
}
