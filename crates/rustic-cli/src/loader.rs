//! Local-only bounded HTML loading for development commands.
//!
//! The command line grants exactly one file path. Page content may name other
//! local subresources only inside that file's directory tree. Scripts run
//! only when the caller opts in. Encoding selection uses a bounded local
//! prefix. The loader has no network access.
//! Input is read in fixed
//! chunks, so the whole file is never buffered, and every stage is bounded:
//! file bytes, parser token/document/diagnostic limits and dump output bytes.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustic_base::{CancellationToken, ClockOverflow, EscapedText, ManualClock};
use rustic_css::{CssError, CssLimits, CssParser, MediaEnvironment, PropertyId};
use rustic_dom::{Document, DumpError};
use rustic_engine::{
    ClassicFetchError, ClassicScriptLimits, ClassicScriptLoad, DocumentStartError, Engine,
    EngineDocument, LayoutGeometry, LayoutReadError, LayoutSource, ScriptLoadError, ScriptStop,
    TaskLimits,
};
use rustic_html::{
    DocumentMode, Encoding, LOCAL_ENCODING_PREFIX_BYTES, ParseError, ParserLimits, Scripting,
    select_local_encoding,
};
use rustic_style::{
    ComputeContext, FetchError, MatchContext, NoFetch, SheetStatus, StyleLoadLimits, StyleLoader,
    StylesheetFetcher, document_cascade_with, dump_computed_styles, style_tree,
};

use crate::stylesheets::{LocalStylesheetFetcher, file_url};
use rustic_bindings::page::ComputedStyleSnapshot;

/// Default maximum file size, matching the tokenizer's default input budget.
pub const DEFAULT_MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
/// Default bytes read from the file and fed to the parser per step.
pub const DEFAULT_READ_CHUNK_BYTES: usize = 64 * 1024;
/// Default maximum UTF-8 bytes of dump output.
pub const DEFAULT_MAX_DUMP_BYTES: usize = 64 * 1024 * 1024;
const MAX_SCRIPT_TIMER_TURNS: usize = 256;

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
    /// Execute the page's supported scripts before producing output.
    pub run_scripts: bool,
    /// View used by scripts that read layout during parsing and callbacks.
    pub script_view: rustic_layout::View,
    /// External stylesheet budgets for `dump-style`.
    pub stylesheets: StyleLoadLimits,
    /// Anonymous flow normalization budgets for `dump-formatting`.
    pub formatting: rustic_layout::FormattingLimits,
    /// `<img>` loading budgets for layout and paint.
    pub images: crate::images::ImageLimits,
}

impl Default for LoaderLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            read_chunk_bytes: DEFAULT_READ_CHUNK_BYTES,
            max_dump_bytes: DEFAULT_MAX_DUMP_BYTES,
            parser: ParserLimits::default(),
            run_scripts: false,
            script_view: default_view(),
            stylesheets: StyleLoadLimits::default(),
            formatting: rustic_layout::FormattingLimits::default(),
            images: crate::images::ImageLimits::default(),
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
    /// This document's work was cancelled.
    Cancelled,
    /// The engine could not admit a new document.
    Engine(DocumentStartError),
    /// The scripted parser or realm stopped.
    Script(ScriptLoadError),
    /// A script is still waiting with no local request or timer to service.
    ScriptStalled,
    /// Pending timers exceeded the scripted snapshot's turn budget.
    ScriptTurnLimit,
    /// The scripted snapshot's monotonic clock overflowed.
    ScriptClock(ClockOverflow),
    /// The CSS parser could not be initialized.
    Css(CssError),
    /// The parser rejected the input or exceeded one of its budgets.
    Parse(ParseError),
    /// The tree dump would exceed its output limit.
    Dump(DumpError),
    /// Principal box construction failed or exceeded its limits.
    Layout(rustic_layout::LayoutError),
    /// The block-only reference layout failed.
    Flow(rustic_layout::FlowError),
    /// Anonymous flow normalization failed.
    Formatting(rustic_layout::FormattingError),
    /// Display list construction failed.
    Paint(rustic_paint::PaintError),
    /// Rasterization failed or exceeded its limits.
    Raster(rustic_raster::RasterError),
    /// The PNG would exceed the output limit.
    Png(rustic_raster::PngError),
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
            Self::Engine(error) => error.fmt(f),
            Self::Script(error) => error.fmt(f),
            Self::ScriptStalled => {
                f.write_str("page scripts are still waiting with no pending local request or timer")
            }
            Self::ScriptTurnLimit => write!(
                f,
                "page scripts exceeded the {MAX_SCRIPT_TIMER_TURNS}-timer-turn limit"
            ),
            Self::ScriptClock(error) => error.fmt(f),
            Self::Css(error) => error.fmt(f),
            Self::Parse(error) => error.fmt(f),
            Self::Dump(error) => error.fmt(f),
            Self::Layout(error) => error.fmt(f),
            Self::Flow(error) => error.fmt(f),
            Self::Formatting(error) => error.fmt(f),
            Self::Paint(error) => error.fmt(f),
            Self::Raster(error) => error.fmt(f),
            Self::Png(error) => error.fmt(f),
        }
    }
}

impl Error for LoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Engine(error) => Some(error),
            Self::Script(error) => Some(error),
            Self::ScriptClock(error) => Some(error),
            Self::Css(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::Dump(error) => Some(error),
            Self::Layout(error) => Some(error),
            Self::Flow(error) => Some(error),
            Self::Formatting(error) => Some(error),
            Self::Paint(error) => Some(error),
            Self::Raster(error) => Some(error),
            Self::Png(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ParseError> for LoadError {
    fn from(value: ParseError) -> Self {
        Self::Parse(value)
    }
}

impl From<ScriptLoadError> for LoadError {
    fn from(error: ScriptLoadError) -> Self {
        match error {
            ScriptLoadError::Stopped(ScriptStop::DocumentCancelled) => Self::Cancelled,
            error => Self::Script(error),
        }
    }
}

impl From<ScriptStop> for LoadError {
    fn from(error: ScriptStop) -> Self {
        ScriptLoadError::from(error).into()
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

/// Read one regular file in bounded chunks and build its document.
///
/// The file is checked before opening (so FIFOs and devices are never opened)
/// and again through the open handle. Its size is enforced while reading, so a
/// file that grows after inspection still cannot exceed the limit. Any failure
/// returns no partial document.
pub fn load_document(
    engine: &mut Engine,
    path: &Path,
    limits: LoaderLimits,
) -> Result<EngineDocument, LoadError> {
    if limits.max_file_bytes == 0 || limits.read_chunk_bytes == 0 || limits.max_dump_bytes == 0 {
        return Err(LoadError::InvalidLimits);
    }
    let mut parser_limits = limits.parser;
    parser_limits.tokenizer.max_input_bytes = limits.max_file_bytes;
    if limits.run_scripts {
        return load_scripted_document(engine, path, limits, parser_limits);
    }
    let mut load = engine
        .begin_document(parser_limits)
        .map_err(|error| match error {
            DocumentStartError::Parser(error) => LoadError::Parse(error),
            other => LoadError::Engine(other),
        })?;
    let cancellation = load.cancellation().expect("admitted load").clone();
    read_document_file(
        path,
        limits,
        &cancellation,
        Scripting::Disabled,
        |bytes, encoding| {
            if let Some(encoding) = encoding {
                load.set_initial_encoding(encoding)?;
            }
            Ok(load.feed(bytes)?)
        },
    )?;
    Ok(load.finish()?)
}

fn load_scripted_document(
    engine: &mut Engine,
    path: &Path,
    limits: LoaderLimits,
    parser_limits: ParserLimits,
) -> Result<EngineDocument, LoadError> {
    let path = fs::canonicalize(path).map_err(LoadError::Io)?;
    check_regular_file(fs::metadata(&path).map_err(LoadError::Io)?, limits)?;
    let url = file_url(&path).ok_or(ScriptLoadError::InvalidDocumentUrl)?;
    let mut load = engine.begin_local_scripted_document_with_clock(
        parser_limits,
        TaskLimits::default(),
        url,
        ClassicScriptLimits::default(),
        ManualClock::default(),
    )?;
    let cancellation = load.cancellation().expect("admitted load").clone();
    let root = path
        .parent()
        .expect("a canonical regular file has a parent");
    let fetch = LocalStylesheetFetcher::new(
        root.to_owned(),
        limits.read_chunk_bytes,
        cancellation.clone(),
    )
    .ok_or(ScriptLoadError::InvalidDocumentUrl)?;
    read_document_file(
        &path,
        limits,
        &cancellation,
        Scripting::Enabled,
        |bytes, encoding| {
            if let Some(encoding) = encoding {
                load.set_initial_encoding(encoding)?;
                load.set_layout_source(Box::new(LocalLayout {
                    path: path.clone(),
                    limits,
                    cancellation: cancellation.clone(),
                    encoding,
                }))?;
            }
            load.feed(bytes)?;
            // Service requests at parser barriers and EOF, independent of file chunks.
            while load.is_parser_blocked() && complete_next_script_request(&mut load, &fetch)? {}
            Ok(())
        },
    )?;
    load.close_input()?;
    let mut turns = 0;
    let mut document = loop {
        while complete_next_script_request(&mut load, &fetch)? {}
        if let Some(document) = load.take_document()? {
            break document;
        }
        let delay = load
            .time_until_next_timer()?
            .ok_or(LoadError::ScriptStalled)?;
        advance_script_timer(load.clock().expect("active load"), delay, &mut turns)?;
        load.run_next()?;
    };
    while let Some(delay) = document.time_until_next_timer()? {
        advance_script_timer(document.clock(), delay, &mut turns)?;
        document.run_next()?;
    }
    document.offer_rendering_opportunity()?;
    document.run_next_with_render(|_, _| ())?;
    Ok(document.into_document())
}

fn advance_script_timer(
    clock: &ManualClock,
    delay: Duration,
    turns: &mut usize,
) -> Result<(), LoadError> {
    if *turns == MAX_SCRIPT_TIMER_TURNS {
        return Err(LoadError::ScriptTurnLimit);
    }
    clock.advance(delay).map_err(LoadError::ScriptClock)?;
    *turns += 1;
    Ok(())
}

fn complete_next_script_request(
    load: &mut ClassicScriptLoad<ManualClock>,
    fetch: &LocalStylesheetFetcher,
) -> Result<bool, LoadError> {
    let Some(request) = load.next_request() else {
        return Ok(false);
    };
    let source = match fetch.fetch_bytes(&request.url, request.max_bytes) {
        Ok(source) => Ok(source),
        Err(FetchError::Blocked) => Err(ClassicFetchError::Blocked),
        Err(FetchError::NotFound) => Err(ClassicFetchError::NotFound),
        Err(FetchError::TooLarge) => Err(ClassicFetchError::TooLarge),
        Err(FetchError::Failed) => Err(ClassicFetchError::Failed),
        Err(FetchError::Cancelled) => return Err(LoadError::Cancelled),
    };
    load.complete_script(request.id, source.as_deref().map_err(|error| *error))?;
    Ok(true)
}

fn read_document_file(
    path: &Path,
    limits: LoaderLimits,
    cancellation: &CancellationToken,
    scripting: Scripting,
    mut feed: impl FnMut(&[u8], Option<Encoding>) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    check_cancelled(cancellation)?;
    check_regular_file(fs::metadata(path).map_err(LoadError::Io)?, limits)?;
    let mut file = File::open(path).map_err(LoadError::Io)?;
    check_regular_file(file.metadata().map_err(LoadError::Io)?, limits)?;

    let mut prefix = vec![0; LOCAL_ENCODING_PREFIX_BYTES.min(limits.max_file_bytes)];
    let chunk_bytes = limits.read_chunk_bytes.min(limits.max_file_bytes);
    let mut total = 0;
    while total < prefix.len() {
        let end = total.saturating_add(chunk_bytes).min(prefix.len());
        let read = read_document_chunk(&mut file, &mut prefix[total..end], cancellation)?;
        if read == 0 {
            break;
        }
        total += read;
    }
    prefix.truncate(total);
    let mut encoding = Some(select_local_encoding(
        &prefix,
        scripting,
        cancellation.clone(),
    )?);
    if prefix.is_empty() {
        feed(&[], encoding.take())?;
    }
    for chunk in prefix.chunks(chunk_bytes) {
        feed(chunk, encoding.take())?;
    }
    let mut buffer = vec![0; chunk_bytes];
    loop {
        let read = read_document_chunk(&mut file, &mut buffer, cancellation)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read)
            .filter(|&total| total <= limits.max_file_bytes)
            .ok_or(LoadError::FileTooLarge {
                limit: limits.max_file_bytes,
            })?;
        feed(&buffer[..read], None)?;
    }
    check_cancelled(cancellation)
}

fn read_document_chunk(
    file: &mut File,
    buffer: &mut [u8],
    cancellation: &CancellationToken,
) -> Result<usize, LoadError> {
    loop {
        check_cancelled(cancellation)?;
        match file.read(buffer) {
            Ok(read) => return Ok(read),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(LoadError::Io(error)),
        }
    }
}

pub fn check_cancelled(cancellation: &CancellationToken) -> Result<(), LoadError> {
    cancellation.check().map_err(|_| LoadError::Cancelled)
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
    parsed: &EngineDocument,
    _path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    let document = parsed.parsed().document();
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
    parsed: &EngineDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    let document = parsed.parsed().document();
    let (styles, sheets) = compute_styles(LayoutInput::new(parsed, path, limits, default_view()))?;
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
    input: LayoutInput<'_>,
) -> Result<
    (
        Vec<rustic_style::ElementStyle>,
        Vec<rustic_style::StyleElementSheet>,
    ),
    LoadError,
> {
    let LayoutInput {
        document,
        mode,
        encoding,
        path,
        limits,
        view,
        cancellation,
    } = input;
    let node_error = |error| LoadError::Dump(DumpError::Node(error));
    check_cancelled(cancellation)?;
    let parser =
        CssParser::new(CssLimits::default(), cancellation.clone()).map_err(LoadError::Css)?;
    let canonical = fs::canonicalize(path).map_err(LoadError::Io)?;
    let document_url = file_url(&canonical);
    let mut local = canonical.parent().and_then(|root| {
        LocalStylesheetFetcher::new(
            root.to_path_buf(),
            limits.read_chunk_bytes,
            cancellation.clone(),
        )
        .map(|fetcher| fetcher.with_environment_encoding(encoding))
    });
    let mut no_fetch = NoFetch;
    let fetcher: &mut dyn StylesheetFetcher = match local.as_mut() {
        Some(local) => local,
        None => &mut no_fetch,
    };
    let mut loader = StyleLoader::new(fetcher, limits.stylesheets);
    let (mut cascade, sheets) =
        document_cascade_with(document, &parser, document_url.as_ref(), &mut loader)
            .map_err(node_error)?;
    // Media queries and viewport units see the view's layout viewport.
    let viewport = view.viewport();
    let (width, height) = (viewport.width().get(), viewport.height().get());
    let compute = ComputeContext {
        viewport_width: width,
        viewport_height: height,
        ..crate::fonts::explicit().map_or_else(ComputeContext::default, |fonts| {
            ComputeContext::with_fonts(fonts)
        })
    };
    let mut environment = MediaEnvironment::screen(width, height);
    environment.resolution = view.scale().get();
    // Font-relative units in media queries use the same granted fonts.
    environment.initial_font = compute.initial_font_ratios();
    cascade.set_environment(environment);
    let mut cx = MatchContext::html();
    cx.quirks = mode == DocumentMode::Quirks;
    let styles = style_tree(document, document.root(), &cascade, &parser, &cx, &compute)
        .map_err(node_error)?;
    check_cancelled(cancellation)?;
    Ok((styles, sheets))
}

/// Dump an initial principal-box snapshot, before anonymous/line box construction.
/// This has no layout positions, shaped text, images or paint output yet.
pub fn dump_boxes(
    parsed: &EngineDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    box_tree(LayoutInput::new(parsed, path, limits, default_view()))?
        .dump(limits.max_dump_bytes)
        .map_err(LoadError::Layout)
}

/// Normalize flow boxes and dump anonymous wrappers/inline continuations.
pub fn dump_formatting(
    parsed: &EngineDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    let principal = box_tree(LayoutInput::new(parsed, path, limits, default_view()))?;
    rustic_layout::FormattingTree::build(&principal, limits.formatting, parsed.cancellation())
        .and_then(|tree| tree.dump(limits.max_dump_bytes))
        .map_err(LoadError::Formatting)
}

fn box_tree(input: LayoutInput<'_>) -> Result<rustic_layout::BoxTree, LoadError> {
    box_tree_with_styles(input).map(|(tree, _)| tree)
}

fn box_tree_with_styles(
    input: LayoutInput<'_>,
) -> Result<(rustic_layout::BoxTree, Vec<rustic_style::ElementStyle>), LoadError> {
    let (styles, _) = compute_styles(input)?;
    let tree = rustic_layout::BoxTree::build(
        input.document,
        input.document.root(),
        &styles,
        layout_limits(input.limits),
        input.cancellation,
    )
    .map_err(LoadError::Layout)?;
    Ok((tree, styles))
}

/// The view every dump command uses: a 1280×720 window at ratio 1 and
/// 100% zoom.
pub fn default_view() -> rustic_layout::View {
    rustic_layout::View::new(1280.0, 720.0, 1.0, 1.0).expect("the default view is valid")
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
    parsed: &EngineDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    let (_, layout, _, _, _) = block_layout(LayoutInput::new(parsed, path, limits, default_view()))?;
    layout.dump(limits.max_dump_bytes).map_err(LoadError::Flow)
}

/// The fonts every layout and paint command uses: the bundled test faces,
/// with every generic family and the fallback mapped to them. No system
/// fonts are read.
fn fonts() -> &'static rustic_text::FontDatabase {
    crate::fonts::database()
}

#[derive(Clone, Copy)]
struct LayoutInput<'a> {
    document: &'a Document,
    mode: DocumentMode,
    encoding: Encoding,
    path: &'a Path,
    limits: LoaderLimits,
    view: rustic_layout::View,
    cancellation: &'a CancellationToken,
}

impl<'a> LayoutInput<'a> {
    fn new(
        parsed: &'a EngineDocument,
        path: &'a Path,
        limits: LoaderLimits,
        view: rustic_layout::View,
    ) -> Self {
        Self {
            document: parsed.parsed().document(),
            mode: parsed.parsed().mode(),
            encoding: parsed.parsed().encoding(),
            path,
            limits,
            view,
            cancellation: parsed.cancellation(),
        }
    }
}

#[derive(Debug)]
struct LocalLayout {
    path: PathBuf,
    limits: LoaderLimits,
    cancellation: CancellationToken,
    encoding: Encoding,
}

impl LayoutSource for LocalLayout {
    fn measure_layout(
        &self,
        document: &Document,
        mode: DocumentMode,
    ) -> Result<LayoutGeometry, LayoutReadError> {
        let (tree, layout, _, _, styles) = block_layout(LayoutInput {
            document,
            mode,
            encoding: self.encoding,
            path: &self.path,
            limits: self.limits,
            view: self.limits.script_view,
            cancellation: &self.cancellation,
        })
        .map_err(|_| LayoutReadError::Unavailable)?;
        Ok(LayoutGeometry {
            offset_widths: layout
                .offset_widths(&tree, &self.cancellation)
                .map_err(|_| LayoutReadError::Unavailable)?,
            client_rects: rustic_paint::client_rects(&tree, &layout, &self.cancellation)
                .map_err(|_| LayoutReadError::Unavailable)?,
            computed_styles: styles
                .into_iter()
                .map(|style| {
                    let longhands = PropertyId::ALL
                        .iter()
                        .map(|&property| {
                            (property.name(), style.computed.get(property).to_string())
                        })
                        .collect();
                    let custom_properties = style
                        .computed
                        .custom_properties()
                        .iter()
                        .map(|(name, value)| (name.to_owned(), value.to_string()))
                        .collect();
                    (style.node, ComputedStyleSnapshot { longhands, custom_properties })
                })
                .collect(),
        })
    }
}

fn block_layout(
    input: LayoutInput<'_>,
) -> Result<
    (
        rustic_layout::BoxTree,
        rustic_layout::BlockLayout,
        rustic_layout::CssSize,
        crate::images::LoadedImages,
        Vec<rustic_style::ElementStyle>,
    ),
    LoadError,
> {
    let (mut tree, styles) = box_tree_with_styles(input)?;
    let LayoutInput {
        document,
        path,
        limits,
        view,
        cancellation,
        ..
    } = input;
    let canonical = fs::canonicalize(path).map_err(LoadError::Io)?;
    let images = crate::images::load(
        document,
        &tree,
        &canonical,
        limits.images,
        limits.read_chunk_bytes,
        cancellation,
    );
    if !images.sizes.is_empty() {
        tree.set_natural_sizes(&images.sizes)
            .map_err(LoadError::Layout)?;
    }
    let viewport = view.viewport();
    let layout = rustic_layout::BlockLayout::compute_with_fonts(
        &tree,
        viewport,
        layout_limits(limits),
        fonts(),
        cancellation,
    )
    .map_err(LoadError::Flow)?;
    Ok((tree, layout, viewport, images, styles))
}

fn display_list(
    parsed: &EngineDocument,
    path: &Path,
    limits: LoaderLimits,
    view: rustic_layout::View,
) -> Result<rustic_paint::DisplayList, LoadError> {
    let (tree, layout, viewport, images, _) =
        block_layout(LayoutInput::new(parsed, path, limits, view))?;
    let resources = rustic_paint::Resources {
        fonts: Some(fonts()),
        images: images.images,
    };
    rustic_paint::DisplayList::build_with(
        parsed.parsed().document(),
        &tree,
        &layout,
        viewport,
        &resources,
        rustic_paint::PaintLimits::default(),
        parsed.cancellation(),
    )
    .map_err(LoadError::Paint)
}

/// Dump the display list for the block-only layout reference.
pub fn dump_paint(
    parsed: &EngineDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    display_list(parsed, path, limits, default_view())?
        .dump(limits.max_dump_bytes)
        .map_err(LoadError::Paint)
}

/// Hit-test each point (CSS pixels from the viewport origin) against the
/// display list and report one line per point: the box, node and element
/// name found, or `none` outside the viewport.
pub fn hit_test(
    parsed: &EngineDocument,
    path: &Path,
    limits: LoaderLimits,
    points: &[(f64, f64)],
) -> Result<String, LoadError> {
    let list = display_list(parsed, path, limits, default_view())?;
    let mut out = String::new();
    for &(x, y) in points {
        let point = rustic_layout::CssPx::new(x)
            .and_then(|x| Ok((x, rustic_layout::CssPx::new(y)?)))
            .map(|(x, y)| rustic_layout::CssPoint { x, y });
        let line = match point.ok().and_then(|point| list.hit_test(point)) {
            Some(hit) => {
                let name = match parsed.parsed().document().node(hit.node).map(|n| n.kind()) {
                    Ok(rustic_dom::NodeKind::Element(e)) => e.name().local().to_string(),
                    _ => String::from("?"),
                };
                format!(
                    "hit {x},{y} box={} node={}:{} {name}\n",
                    hit.source_box,
                    hit.node.slot(),
                    hit.node.generation()
                )
            }
            None => format!("hit {x},{y} none\n"),
        };
        if out.len() + line.len() > limits.max_dump_bytes {
            return Err(LoadError::Dump(DumpError::OutputLimit));
        }
        out.push_str(&line);
    }
    Ok(out)
}

/// Rasterize the display list at `scale`. Only block backgrounds and solid
/// borders are painted.
pub fn render_surface(
    parsed: &EngineDocument,
    path: &Path,
    limits: LoaderLimits,
    view: rustic_layout::View,
) -> Result<rustic_raster::Surface, LoadError> {
    render_list(parsed, path, limits, view).map(|(_, surface)| surface)
}

/// Rasterize like [`render_surface`], keeping the display list.
pub fn render_list(
    parsed: &EngineDocument,
    path: &Path,
    limits: LoaderLimits,
    view: rustic_layout::View,
) -> Result<(rustic_paint::DisplayList, rustic_raster::Surface), LoadError> {
    let list = display_list(parsed, path, limits, view)?;
    let surface = rustic_raster::rasterize(
        &list,
        view.scale(),
        rustic_raster::RasterLimits::default(),
        parsed.cancellation(),
    )
    .map_err(LoadError::Raster)?;
    Ok((list, surface))
}

/// Whether repainting the tiles [`rustic_raster::damage`] reports between
/// `old` and `new` over `old`'s surface gives exactly `new`'s surface.
pub fn damage_repaints_exactly(
    (old, old_surface): &(rustic_paint::DisplayList, rustic_raster::Surface),
    (new, new_surface): &(rustic_paint::DisplayList, rustic_raster::Surface),
    scale: rustic_layout::DeviceScale,
    cancellation: &CancellationToken,
) -> Result<bool, LoadError> {
    if (old_surface.width(), old_surface.height()) != (new_surface.width(), new_surface.height()) {
        // A new size repaints the whole surface: nothing to compare.
        return Ok(true);
    }
    let regions = rustic_raster::damage(old, new, scale).map_err(LoadError::Raster)?;
    let mut surface = old_surface.clone();
    rustic_raster::repaint(
        &mut surface,
        new,
        scale,
        &regions,
        rustic_raster::RasterLimits::default(),
        cancellation,
    )
    .map_err(LoadError::Raster)?;
    Ok(surface.premultiplied() == new_surface.premultiplied())
}

/// Rasterize like [`render_surface`] and encode the result as PNG, within
/// the output byte limit.
pub fn render_png(
    parsed: &EngineDocument,
    path: &Path,
    limits: LoaderLimits,
    view: rustic_layout::View,
) -> Result<Vec<u8>, LoadError> {
    let surface = render_surface(parsed, path, limits, view)?;
    rustic_raster::encode_png(&surface, limits.max_dump_bytes).map_err(LoadError::Png)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct TempFile(PathBuf);

    #[test]
    fn local_encoding_replays_before_the_first_script() {
        let stylesheet = TempFile::new(b".caf\xe9 { width: 41px; height: 5px }");
        let classic = TempFile::new(b"var classic = 'r\xe9sum\xe9';");
        let mut html = b"<!doctype html><body><div id=box class=caf\xe9></div>
            <script>
              var visits = (globalThis.visits || 0) + 1;
              var word = 'caf\xe9';
            </script><!--"
            .to_vec();
        html.extend(std::iter::repeat_n(b'x', 1500));
        html.extend_from_slice(
            b"--><meta http-equiv=content-type content=\"text/html; charset='latin1'\">",
        );
        html.extend_from_slice(
            format!(
                "<link rel=stylesheet href=\"{}\"><script src=\"{}\"></script>",
                stylesheet.0.file_name().unwrap().to_str().unwrap(),
                classic.0.file_name().unwrap().to_str().unwrap(),
            )
            .as_bytes(),
        );
        html.extend_from_slice(
            br#"<script>
            document.body.setAttribute('data-result', [
                document.characterSet, document.charset, document.inputEncoding,
                visits, word, classic, document.querySelector('#box').offsetWidth,
                document.implementation.createHTMLDocument('').characterSet,
            ].join('|'));
        </script>"#,
        );
        let file = TempFile::new(&html);
        for read_chunk_bytes in [1, 3, 999, 65536] {
            let limits = LoaderLimits {
                run_scripts: true,
                read_chunk_bytes,
                ..LoaderLimits::default()
            };
            let document = load_document(&mut Engine::new(1), &file.0, limits).unwrap();
            assert_eq!(document.parsed().encoding(), Encoding::Windows1252);
            let tree = dump_dom(&document, &file.0, limits).unwrap();
            assert!(tree.contains("local=\"data-result\" value=\"windows-1252|windows-1252|windows-1252|1|café|résumé|41|UTF-8\""), "{tree}");
        }
    }

    #[test]
    fn parser_scripts_read_the_renderers_layout_at_the_selected_view() {
        let file = TempFile::new(
            br#"<!DOCTYPE html>
                <style>
                    #box { width: 25vw; height: 10px; border: 2px solid }
                    #box.wide { width: 50vw; transform: scale(2) }
                    #hidden { display: none }
                    #inline { padding: 2px; border: 1px solid }
                </style>
                <div id="box"></div>
                <div id="hidden"></div>
                <span id="inline">text</span>
                <script>
                    const box = document.querySelector('#box');
                    const hidden = document.querySelector('#hidden');
                    const inline = document.querySelector('#inline');
                    const detached = document.implementation.createHTMLDocument('').body;
                    const width = box.offsetWidth;
                    const before = box.getBoundingClientRect();
                    box.setAttribute('class', 'wide');
                    const after = box.getBoundingClientRect();
                    const dimensions = [before.width, after.width, after.height, after.left];
                    const rectType = after instanceof DOMRect && after instanceof DOMRectReadOnly;
                    after.width = 7;
                    document.body.setAttribute('data-layout', [
                        width, box.offsetWidth, hidden.offsetWidth, detached.offsetWidth,
                        inline.offsetWidth > 6, box instanceof HTMLElement,
                        dimensions.join(':'), rectType, after.width,
                        box.getBoundingClientRect().width,
                        detached.getBoundingClientRect().width,
                    ].join(','));
                </script>"#,
        );
        let limits = LoaderLimits {
            run_scripts: true,
            script_view: rustic_layout::View::new(320.0, 240.0, 1.0, 1.0).unwrap(),
            ..LoaderLimits::default()
        };
        let parsed = load_document(&mut Engine::new(1), &file.0, limits).unwrap();
        let tree = dump_dom(&parsed, &file.0, limits).unwrap();
        assert!(
            tree.contains(
                "local=\"data-layout\" value=\"84,164,0,0,true,true,84:328:28:-74,true,7,328,0\""
            ),
            "{tree}"
        );
    }

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
        let mut engine = Engine::new(1);
        let parsed = load_document(&mut engine, &file.0, limits)?;
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
    fn processing_instructions_survive_the_local_load() {
        let file = TempFile::new(b"<p>before<?target data?>after");
        let output = dump(&file, LoaderLimits::default()).unwrap();
        assert!(output.contains("processing-instruction target=\"target\" data=\"data\""));
    }

    #[test]
    fn rejects_directories_missing_files_and_zero_limits() {
        let directory = std::env::temp_dir();
        assert!(matches!(
            load_document(&mut Engine::new(1), &directory, LoaderLimits::default(),),
            Err(LoadError::NotRegularFile)
        ));
        let missing = directory.join("rustic-loader-definitely-missing.html");
        assert!(matches!(
            load_document(&mut Engine::new(1), &missing, LoaderLimits::default()),
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
                &mut Engine::new(1),
                Path::new("/dev/zero"),
                LoaderLimits::default(),
            ),
            Err(LoadError::NotRegularFile)
        ));
    }

    #[test]
    fn scheduled_rendering_turn_produces_the_reference_png() {
        use rustic_engine::{DocumentTurn, TaskLimits};

        let mut engine = Engine::new(1);
        let file = TempFile::new(SOURCE);
        let limits = LoaderLimits::default();
        let view = default_view();
        let document = load_document(&mut engine, &file.0, limits).expect("load");
        let expected = render_png(&document, &file.0, limits, view).expect("direct render");
        let mut scripts = document.into_scripted(TaskLimits::default()).unwrap();
        scripts.offer_rendering_opportunity().unwrap();

        let Some(DocumentTurn::Rendered { frame, output }) = scripts
            .run_next_with_render(|document, _| render_png(document, &file.0, limits, view))
            .unwrap()
        else {
            panic!("expected the scheduled rendering turn");
        };
        assert!(scripts.accept_rendered_frame(frame).unwrap());
        assert_eq!(output.expect("scheduled render"), expected);
    }

    #[test]
    fn shutdown_stops_document_loading_and_rendering() {
        let mut engine = Engine::new(1);
        let file = TempFile::new(SOURCE);
        let limits = LoaderLimits::default();
        let document = load_document(&mut engine, &file.0, limits).expect("load");
        engine.shutdown();
        assert!(matches!(
            render_png(&document, &file.0, limits, default_view()),
            Err(LoadError::Cancelled)
        ));
        assert!(matches!(
            load_document(&mut engine, Path::new("never-open-this-path"), limits),
            Err(LoadError::Engine(DocumentStartError::Shutdown))
        ));
    }
}
