//! Bounded HTML loading for development commands.
//!
//! The command line grants exactly one file path, or one `http(s)` URL when
//! origins are granted with `--allow-origin` (see [`crate::network`]). Page
//! content may name other local subresources only inside that file's
//! directory tree; a network document's subresources go through its
//! network session and policies. Scripts run only when the caller opts in.
//! Encoding selection uses a bounded prefix (and the response charset).
//! Input is read in fixed
//! chunks, so the whole file is never buffered, and every stage is bounded:
//! file bytes, parser token/document/diagnostic limits and dump output bytes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use rustic_base::{CancellationToken, ClockOverflow, EscapedText, ManualClock};
use rustic_css::{
    BoxSizing, CssError, CssLimits, CssParser, Length, LineHeight, MediaEnvironment, PropertyId,
    PropertyValue,
};
use rustic_dom::{Document, DumpError};
use rustic_engine::{
    ClassicFetchError, ClassicScriptLimits, ClassicScriptLoad, ComputedStyleRecord,
    ComputedStyleSnapshot, DocumentStartError, Engine, EngineDocument, LayoutGeometry,
    LayoutReadError, LayoutSource, ScriptLoadError, ScriptStop, TaskLimits,
};
use rustic_html::{
    DocumentMode, Encoding, LOCAL_ENCODING_PREFIX_BYTES, ParseError, ParserLimits, Scripting,
    select_local_encoding, sniff_declared_encoding,
};
use rustic_style::{
    ComputeContext, ComputedStyle, FetchError, MatchContext, NoFetch, SheetStatus, StyleLoadLimits,
    StyleLoader, StylesheetFetcher, document_cascade_with, dump_computed_styles, style_tree,
};

use crate::network::{InlinePolicy, NetworkLoadError, ResourceKind, Session, SessionDocument};
use crate::stylesheets::{
    AppliedLocalFiles, LocalStylesheetFetcher, MAX_APPLIED_URL_BYTES, file_url,
};

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
    /// `--click X,Y` points (CSS pixels), delivered as trusted primary
    /// clicks after the load's script turns.
    pub clicks: [Option<(f64, f64)>; MAX_CLICKS],
    /// `--scroll X,Y`: the viewport's scroll position for painting.
    pub scroll: (f64, f64),
    /// `--scroll-box N=X,Y`: scroll container positions by fragment index.
    pub scroll_boxes: [Option<(usize, f64, f64)>; MAX_CLICKS],
    /// `render --report-scroll`: print the viewport position the page
    /// was painted at (`scroll X,Y`) on standard output (ADR 0065).
    pub report_scroll: bool,
    /// `render --report-a11y`: print the page's encoded accessibility tree
    /// (`a11y HEX`) on standard output for the window's platform bridge
    /// (ADR 0073).
    pub report_a11y: bool,
    /// `render --frames N`: rendering opportunities to paint, one frame
    /// every [`FRAME_INTERVAL`] of the manual clock (1 by default).
    pub frames: usize,
    /// `render --live` (renderer only, TD-182): after loading, keep the
    /// page and answer the broker's input and frame requests until it
    /// closes the channel.
    pub live: bool,
    /// `--devtools-allow LIST`: what an automation session may do
    /// (ADR 0072); `None` unless given.
    pub devtools: Option<rustic_devtools::Grants>,
}

/// The manual clock's step between `--frames` (60 per second).
pub const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
/// Most `--frames`.
pub const MAX_FRAMES: usize = 600;

/// Most `--click` points one command takes.
pub const MAX_CLICKS: usize = 8;

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
            clicks: [None; MAX_CLICKS],
            scroll: (0.0, 0.0),
            scroll_boxes: [None; MAX_CLICKS],
            frames: 1,
            report_scroll: false,
            report_a11y: false,
            live: false,
            devtools: None,
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
    /// A network document could not be loaded.
    Network(NetworkLoadError),
    /// The broker (profile, network or the other process) failed.
    Broker(String),
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
            Self::Network(error) => error.fmt(f),
            Self::Broker(error) => f.write_str(error),
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
            Self::Network(error) => Some(error),
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

/// A local page and the resource bytes applied during its script turns.
#[derive(Debug)]
pub struct LoadedDocument {
    document: EngineDocument,
    applied_files: Option<AppliedLocalFiles>,
    network: Option<Rc<dyn Session>>,
    /// Where the page's scripts left the scroll positions (ADR 0064).
    scroll: rustic_engine::ScrollState,
    /// `render --frames`: each frame's pixels and what it repainted.
    frames: Vec<(rustic_raster::Surface, FrameReport)>,
    /// Focus and form control state the page's scripts left (ADR 0073).
    live: rustic_a11y::LiveState,
    /// The same control state, for painting.
    controls: ControlStates,
}

/// How one `--frames` frame was painted.
#[derive(Clone, Copy, Debug)]
pub enum FrameReport {
    /// Damaged tiles repainted over the previous frame.
    Damage(rustic_raster::FrameStats),
    /// Through the scene: cached content tiles and the overlay (ADR 0064).
    Scene(rustic_raster::SceneStats),
}

impl LoadedDocument {
    pub fn parsed(&self) -> &rustic_html::ParsedDocument {
        self.document.parsed()
    }

    pub fn cancellation(&self) -> &CancellationToken {
        self.document.cancellation()
    }

    /// `render --frames`: the painted frames and their repaint statistics.
    pub fn frames(&self) -> &[(rustic_raster::Surface, FrameReport)] {
        &self.frames
    }

    fn layout_input<'a>(
        &'a self,
        path: &'a Path,
        limits: LoaderLimits,
        view: rustic_layout::View,
    ) -> LayoutInput<'a> {
        LayoutInput {
            scroll: Some(&self.scroll),
            controls: Some(&self.controls),
            ..LayoutInput::new(
                &self.document,
                path,
                limits,
                view,
                self.applied_files.as_ref(),
                self.network.as_ref(),
            )
        }
    }
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
) -> Result<LoadedDocument, LoadError> {
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
    Ok(LoadedDocument {
        document: load.finish()?,
        applied_files: None,
        network: None,
        scroll: rustic_engine::ScrollState::default(),
        live: rustic_a11y::LiveState::default(),
        controls: ControlStates::new(),
        frames: Vec::new(),
    })
}

/// Read the granted local file for a renderer: canonical path, regular
/// file, at most `max_file_bytes`, with the same errors as a direct load.
pub fn read_granted_file(
    path: &Path,
    limits: LoaderLimits,
) -> Result<(PathBuf, Vec<u8>), LoadError> {
    if limits.max_file_bytes == 0 || limits.read_chunk_bytes == 0 || limits.max_dump_bytes == 0 {
        return Err(LoadError::InvalidLimits);
    }
    check_regular_file(fs::metadata(path).map_err(LoadError::Io)?, limits)?;
    let canonical = fs::canonicalize(path).map_err(LoadError::Io)?;
    let file = File::open(&canonical).map_err(LoadError::Io)?;
    check_regular_file(file.metadata().map_err(LoadError::Io)?, limits)?;
    let mut bytes = Vec::new();
    file.take(limits.max_file_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(LoadError::Io)?;
    if bytes.len() > limits.max_file_bytes {
        return Err(LoadError::FileTooLarge {
            limit: limits.max_file_bytes,
        });
    }
    Ok((canonical, bytes))
}

/// A snapshot sized for every stylesheet and image a document may apply.
fn applied_snapshot(limits: LoaderLimits) -> AppliedLocalFiles {
    AppliedLocalFiles::new(
        limits
            .stylesheets
            .max_sheets
            .saturating_add(limits.images.max_images),
        limits
            .stylesheets
            .max_total_bytes
            .saturating_add(limits.images.max_file_bytes),
    )
}

/// Navigate to an `http(s)` URL with the granted origins and build its
/// document. Its response URL is the base URL, its `Content-Type` charset
/// the transport encoding, and its subresources load through the same
/// session under the document's CSP and mixed-content checks. Every
/// stylesheet and image is pinned on first read, so repeated layout reads
/// see the same bytes.
pub fn load_network_document(
    engine: &mut Engine,
    url: &str,
    limits: LoaderLimits,
) -> Result<LoadedDocument, LoadError> {
    if limits.max_file_bytes == 0 || limits.read_chunk_bytes == 0 || limits.max_dump_bytes == 0 {
        return Err(LoadError::InvalidLimits);
    }
    let cancellation = CancellationToken::default();
    let network: SessionDocument =
        crate::network::navigate(url, limits.max_file_bytes, cancellation)
            .map_err(LoadError::Network)?
            .into();
    load_session_document(engine, &network, limits)
}

/// Load a document whose bytes and subresources come through `network`'s
/// session: a network response in this process, or, in a renderer, any
/// document the broker granted (a local file included).
pub fn load_session_document(
    engine: &mut Engine,
    network: &SessionDocument,
    limits: LoaderLimits,
) -> Result<LoadedDocument, LoadError> {
    if limits.max_file_bytes == 0 || limits.read_chunk_bytes == 0 || limits.max_dump_bytes == 0 {
        return Err(LoadError::InvalidLimits);
    }
    if network.bytes.borrow().len() > limits.max_file_bytes {
        return Err(LoadError::FileTooLarge {
            limit: limits.max_file_bytes,
        });
    }
    let mut parser_limits = limits.parser;
    parser_limits.tokenizer.max_input_bytes = limits.max_file_bytes;
    let mut forced = None;
    loop {
        let (document, selected) = if limits.run_scripts {
            load_scripted_network_document(engine, network, limits, parser_limits, forced)?
        } else {
            load_unscripted_network_document(engine, network, limits, parser_limits, forced)?
        };
        // HTML "change the encoding": a declaration the prescan did not see
        // replaces a tentative encoding by loading the document again with
        // the declared one, now certain; it happens at most once.
        network.session.finish().map_err(LoadError::Broker)?;
        if let Selected::Tentative(used) = selected {
            let declared = first_meta_declaration(document.parsed().document());
            if let Some(declared) = declared.filter(|&declared| declared != used) {
                eprintln!(
                    "rustic: reloading the document as {} (declared after the prescanned prefix)",
                    declared.name()
                );
                drop(document);
                forced = Some(declared);
                continue;
            }
        }
        return Ok(document);
    }
}

fn load_unscripted_network_document(
    engine: &mut Engine,
    network: &SessionDocument,
    limits: LoaderLimits,
    parser_limits: ParserLimits,
    forced: Option<Encoding>,
) -> Result<(LoadedDocument, Selected), LoadError> {
    let mut load = engine
        .begin_document(parser_limits)
        .map_err(|error| match error {
            DocumentStartError::Parser(error) => LoadError::Parse(error),
            other => LoadError::Engine(other),
        })?;
    let cancellation = load.cancellation().expect("admitted load").clone();
    let selected = feed_network_document(
        network,
        limits,
        &cancellation,
        Scripting::Disabled,
        forced,
        |bytes, encoding| {
            if let Some(encoding) = encoding {
                load.set_initial_encoding(encoding)?;
            }
            Ok(load.feed(bytes)?)
        },
    )?;
    let document = LoadedDocument {
        document: load.finish()?,
        applied_files: Some(applied_snapshot(limits)),
        network: Some(Rc::clone(&network.session)),
        scroll: rustic_engine::ScrollState::default(),
        live: rustic_a11y::LiveState::default(),
        controls: ControlStates::new(),
        frames: Vec::new(),
    };
    Ok((document, selected))
}

/// The encoding a network document was decoded with, and its HTML
/// "confidence".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Selected {
    /// From a byte order mark, the response charset or a reload.
    Certain,
    /// From the `<meta>` prescan or the locale fallback.
    Tentative(Encoding),
}

/// The first `<meta>` encoding declaration in tree order (template
/// contents are not in the tree), as the tree builder would act on it.
fn first_meta_declaration(document: &Document) -> Option<Encoding> {
    use crate::reftest::{attribute, is_html};
    for (node, _) in document.walk(document.root()).ok()? {
        let Some(element) = rustic_style::DomElement::new(document, node) else {
            continue;
        };
        if !is_html(element, "meta") {
            continue;
        }
        let declared = rustic_html::meta_charset(
            attribute(element, "charset").as_deref(),
            attribute(element, "http-equiv").as_deref(),
            attribute(element, "content").as_deref(),
        );
        if declared.is_some() {
            return declared;
        }
    }
    None
}

/// Feed a network document's bytes like [`read_document_file`], with the
/// HTML encoding sniffing algorithm: a byte order mark, then the response
/// charset (both certain), then the `<meta>` prescan of the bounded prefix,
/// then the locale's fallback encoding (both tentative). `forced` is the
/// encoding of a reload after "change the encoding".
fn feed_network_document(
    network: &SessionDocument,
    limits: LoaderLimits,
    cancellation: &CancellationToken,
    scripting: Scripting,
    forced: Option<Encoding>,
    mut feed: impl FnMut(&[u8], Option<Encoding>) -> Result<(), LoadError>,
) -> Result<Selected, LoadError> {
    check_cancelled(cancellation)?;
    // A byte order mark needs three bytes; the `<meta>` prescan, needed
    // unless the encoding is already certain, waits for 1024 bytes or the
    // end of the body.
    let wanted = if network.session.is_local()
        || (forced.is_none() && network.transport_encoding.is_none())
    {
        LOCAL_ENCODING_PREFIX_BYTES
    } else {
        3
    };
    while network.bytes.borrow().len() < wanted
        && pull_document_chunk(network, limits, cancellation)?
    {}
    let (has_bom, prefix) = {
        let bytes = network.bytes.borrow();
        let prefix = bytes[..bytes.len().min(LOCAL_ENCODING_PREFIX_BYTES)].to_vec();
        (rustic_encoding::bom_sniff(&bytes).is_some(), prefix)
    };
    let (encoding, selected) = if network.session.is_local() {
        // A local file: BOM or `<meta>` in the prefix, else UTF-8, as
        // `read_document_file` decides; there is no reload.
        let encoding = select_local_encoding(&prefix, scripting, cancellation.clone())?;
        (encoding, Selected::Certain)
    } else {
        match (forced, network.transport_encoding) {
            (Some(forced), _) if !has_bom => (forced, Selected::Certain),
            (_, Some(transport)) if !has_bom => (transport, Selected::Certain),
            _ => match sniff_declared_encoding(&prefix, scripting, cancellation.clone())? {
                Some(encoding) if has_bom => (encoding, Selected::Certain),
                Some(encoding) => (encoding, Selected::Tentative(encoding)),
                None => {
                    let fallback =
                        rustic_html::locale_fallback_encoding(crate::network::fallback_locale());
                    (fallback, Selected::Tentative(fallback))
                }
            },
        }
    };
    // Feed what has arrived, then each piece as it arrives (TD-165). A
    // reload after a late `<meta>` feeds the bytes kept from this pass.
    let mut encoding = Some(encoding);
    let chunk_bytes = limits.read_chunk_bytes.max(1);
    let mut fed = 0usize;
    loop {
        let chunk = {
            let bytes = network.bytes.borrow();
            let end = bytes.len().min(fed.saturating_add(chunk_bytes));
            bytes[fed..end].to_vec()
        };
        if chunk.is_empty() {
            if pull_document_chunk(network, limits, cancellation)? {
                continue;
            }
            if fed == 0 {
                feed(&[], encoding.take())?;
            }
            break;
        }
        check_cancelled(cancellation)?;
        fed += chunk.len();
        feed(&chunk, encoding.take())?;
    }
    check_cancelled(cancellation)?;
    Ok(selected)
}

/// Append the next piece of a streamed document body; `false` at its end.
fn pull_document_chunk(
    network: &SessionDocument,
    limits: LoaderLimits,
    cancellation: &CancellationToken,
) -> Result<bool, LoadError> {
    check_cancelled(cancellation)?;
    let too_large = || LoadError::FileTooLarge {
        limit: limits.max_file_bytes,
    };
    let chunk = match network.session.next_document_chunk() {
        Ok(Some(chunk)) => chunk,
        Ok(None) => return Ok(false),
        Err(NetworkLoadError::TooLarge(_)) => return Err(too_large()),
        Err(error) => return Err(LoadError::Network(error)),
    };
    let mut bytes = network.bytes.borrow_mut();
    if chunk.len() > limits.max_file_bytes.saturating_sub(bytes.len()) {
        return Err(too_large());
    }
    bytes.extend_from_slice(&chunk);
    Ok(true)
}

fn load_scripted_document(
    engine: &mut Engine,
    path: &Path,
    limits: LoaderLimits,
    parser_limits: ParserLimits,
) -> Result<LoadedDocument, LoadError> {
    let path = fs::canonicalize(path).map_err(LoadError::Io)?;
    check_regular_file(fs::metadata(&path).map_err(LoadError::Io)?, limits)?;
    let url = file_url(&path).ok_or(ScriptLoadError::InvalidDocumentUrl)?;
    let mut load = engine.begin_local_scripted_document_with_clock(
        parser_limits,
        TaskLimits::default(),
        url.clone(),
        ClassicScriptLimits::default(),
        ManualClock::default(),
    )?;
    load.set_fetch_host(Rc::new(crate::network::LocalFetch(url.clone())))?;
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
    let source = ScriptedSource {
        path: path.clone(),
        url,
        network: None,
    };
    run_scripted_load(load, fetch, source, limits, |feed| {
        read_document_file(&path, limits, &cancellation, Scripting::Enabled, feed)
    })
}

fn load_scripted_network_document(
    engine: &mut Engine,
    network: &SessionDocument,
    limits: LoaderLimits,
    parser_limits: ParserLimits,
    forced: Option<Encoding>,
) -> Result<(LoadedDocument, Selected), LoadError> {
    let session = crate::devtools::observe(Rc::clone(&network.session));
    let url = session.document_url().clone();
    // A brokered local file keeps the local script grant (its directory).
    let mut load = if session.is_local() {
        engine.begin_local_scripted_document_with_clock(
            parser_limits,
            TaskLimits::default(),
            url.clone(),
            ClassicScriptLimits::default(),
            ManualClock::default(),
        )?
    } else {
        engine.begin_network_scripted_document_with_clock(
            parser_limits,
            TaskLimits::default(),
            url.clone(),
            ClassicScriptLimits::default(),
            ManualClock::default(),
        )?
    };
    if !session.csp().is_empty() {
        load.set_script_policy(Box::new(InlinePolicy(Rc::clone(&session))))?;
        load.set_string_compilation_allowed(session.allows_eval())?;
        load.set_wasm_compilation_policy(session.csp().check_wasm_compilation())?;
    }
    load.set_fetch_host(Rc::new(crate::network::SessionFetch(Rc::clone(&session))))?;
    if let Some(storage) = session.storage_host().map_err(LoadError::Broker)? {
        load.set_storage_host(storage)?;
    }
    let cancellation = load.cancellation().expect("admitted load").clone();
    let fetch = LocalStylesheetFetcher::for_network(
        Rc::clone(&session),
        limits.read_chunk_bytes,
        cancellation.clone(),
    );
    let source = ScriptedSource {
        path: PathBuf::new(),
        url,
        network: Some(session),
    };
    let mut selected = Selected::Certain;
    let document = run_scripted_load(load, fetch, source, limits, |feed| {
        selected = feed_network_document(
            network,
            limits,
            &cancellation,
            Scripting::Enabled,
            forced,
            feed,
        )?;
        Ok(())
    })?;
    Ok((document, selected))
}

/// Where a scripted document came from.
struct ScriptedSource {
    /// The canonical file (empty for a network document).
    path: PathBuf,
    /// The document URL.
    url: rustic_url::Url,
    network: Option<Rc<dyn Session>>,
}

type FeedDocument<'a> = dyn FnMut(&[u8], Option<Encoding>) -> Result<(), LoadError> + 'a;

fn run_scripted_load(
    mut load: ClassicScriptLoad<ManualClock>,
    fetch: LocalStylesheetFetcher,
    source: ScriptedSource,
    limits: LoaderLimits,
    read: impl FnOnce(&mut FeedDocument<'_>) -> Result<(), LoadError>,
) -> Result<LoadedDocument, LoadError> {
    let cancellation = load.cancellation().expect("admitted load").clone();
    let applied_files = applied_snapshot(limits);
    // An automation session sets up its debugger and breakpoints before
    // any HTML is parsed (ADR 0072).
    let devtools = crate::devtools::active();
    if let Some(session) = &devtools {
        session.attach(&mut load, &source.url)?;
    }
    let mut requests = ScriptRequests::new(source.url.clone());
    let mut turns = 0;
    read(&mut |bytes, encoding| {
        if let Some(encoding) = encoding {
            load.set_initial_encoding(encoding)?;
            load.set_layout_source(Box::new(LocalLayout {
                path: source.path.clone(),
                limits,
                cancellation: cancellation.clone(),
                encoding,
                applied_files: applied_files.clone(),
                network: source.network.clone(),
            }))?;
        }
        if let Some(session) = &devtools {
            session.note_document_bytes(bytes);
        }
        load.feed(bytes)?;
        // Service requests at parser barriers and EOF, independent of file chunks.
        while load.is_parser_blocked()
            && requests.complete_next(&mut load, &fetch, &applied_files, limits, &mut turns)?
        {
        }
        Ok(())
    })?;
    load.close_input()?;
    let mut document = loop {
        while requests.complete_next(&mut load, &fetch, &applied_files, limits, &mut turns)? {}
        if let Some(document) = load.take_document()? {
            break document;
        }
        let delay = load
            .time_until_next_timer()?
            .ok_or(LoadError::ScriptStalled)?;
        advance_script_timer(load.clock().expect("active load"), delay, &mut turns)?;
        load.run_next()?;
    };
    // The parser is now complete. Local resource discovery and delivery
    // happen in bounded host turns without a second pending-byte buffer.
    queue_discovered_local_resources(
        &mut document,
        &source.url,
        limits,
        &fetch,
        &applied_files,
        &cancellation,
        &mut turns,
    )?;
    // The resource helper already delivered each accepted completion.
    // Retain the original bounded timer loop, including zero-delay timers.
    // Tasks already queued without a deadline (such as unhandled-rejection
    // notifications) run before the clock advances, under their own bound.
    let mut queued_turns = 0usize;
    // A frame sequence lets timers advance with its clock instead.
    let animated = limits.frames > 1 || limits.live;
    let settle_now = |document: &mut rustic_engine::ScriptedDocument<ManualClock>,
                      queued: &mut usize,
                      turns: &mut usize| {
        if animated {
            run_ready(document, queued)
        } else {
            settle(document, queued, turns)
        }
    };
    settle_now(&mut document, &mut queued_turns, &mut turns)?;
    // Trusted host input: each `--click` hits the topmost box at its point
    // and gives the page transient activation (HTML §6.4.3).
    for &(x, y) in limits.clicks.iter().flatten() {
        let state = document.scroll_state();
        let animations = document.animations();
        let list = display_list(LayoutInput {
            scroll: Some(&state),
            animations: Some(&*animations),
            ..LayoutInput::new(
                &document.document(),
                &source.path,
                limits,
                limits.script_view,
                Some(&applied_files),
                source.network.as_ref(),
            )
        })?;
        let point = rustic_layout::CssPx::new(x)
            .and_then(|x| Ok((x, rustic_layout::CssPx::new(y)?)))
            .map(|(x, y)| rustic_layout::CssPoint { x, y });
        let Some(hit) = point.ok().and_then(|point| list.hit_test(point)) else {
            eprintln!("rustic: --click {x},{y} hits nothing");
            continue;
        };
        let event = rustic_engine::InputEvent::Click {
            client_x: x as i32,
            client_y: y as i32,
            modifiers: rustic_engine::Modifiers::default(),
        };
        if document.queue_input(hit.node, event).is_err() {
            eprintln!("rustic: --click {x},{y}: the page took no input");
            continue;
        }
        settle_now(&mut document, &mut queued_turns, &mut turns)?;
    }
    // A static load takes its one rendering opportunity (animation frame
    // callbacks, resize observations) before the automation session reads
    // the page, so the session sees what the dump would.
    if !animated {
        document.offer_rendering_opportunity()?;
        document.run_next_with_render(|_, _| ())?;
    }
    // The automation session drives the settled page until it closes.
    if let Some(session) = &devtools {
        session.loaded(&mut document);
        let host = SessionPage {
            source: &source,
            limits,
            applied_files: &applied_files,
            queued_turns: std::cell::Cell::new(queued_turns),
            turns: std::cell::Cell::new(turns),
        };
        session.serve(&mut document, &host);
    }
    // Popups open in renderers of their own, through the broker (ADR 0062).
    for popup in document.take_popups() {
        let opened = match &source.network {
            Some(session) => session.open_popup(&popup),
            None => Err("popups open only in a split run (TD-178)".to_owned()),
        };
        if let Err(why) = opened {
            eprintln!(
                "rustic: popup {}: {why}",
                EscapedText(&popup.url.serialize())
            );
        }
    }
    if limits.live {
        return Err(live_page(document, &source, limits, &applied_files));
    }
    let mut frames = Vec::new();
    if animated {
        // Each frame: the clock steps, due tasks run, then a rendering
        // opportunity, then the DOM as it stands is painted incrementally.
        let mut painter = rustic_raster::Frames::new(
            limits.script_view.scale(),
            rustic_raster::RasterLimits::default(),
        );
        let mut scene = rustic_raster::Scene::new(
            limits.script_view.scale(),
            rustic_raster::RasterLimits::default(),
        );
        let mut bytes = 0usize;
        for index in 0..limits.frames {
            if index > 0 {
                document
                    .clock()
                    .advance(FRAME_INTERVAL)
                    .map_err(LoadError::ScriptClock)?;
                queued_turns = 0;
                run_ready(&mut document, &mut queued_turns)?;
            }
            document.offer_rendering_opportunity()?;
            document.run_next_with_render(|_, _| ())?;
            let state = document.scroll_state();
            let animations = document.animations();
            let controls = control_states(&document.accessibility_state());
            let engine_document = document.document();
            let input = LayoutInput {
                scroll: Some(&state),
                animations: Some(&*animations),
                controls: Some(&controls),
                ..LayoutInput::new(
                    &engine_document,
                    &source.path,
                    limits,
                    limits.script_view,
                    Some(&applied_files),
                    source.network.as_ref(),
                )
            };
            // Scrolled pages and pages with fixed boxes go through the
            // scene; the rest repaint their damage.
            let (surface, report) = match render_scene(input, &mut scene)? {
                Some((surface, stats)) => {
                    painter = rustic_raster::Frames::new(
                        limits.script_view.scale(),
                        rustic_raster::RasterLimits::default(),
                    );
                    (surface, FrameReport::Scene(stats))
                }
                None => {
                    scene = rustic_raster::Scene::new(
                        limits.script_view.scale(),
                        rustic_raster::RasterLimits::default(),
                    );
                    let list = display_list(input)?;
                    let stats = painter
                        .submit(list, &cancellation)
                        .map_err(LoadError::Raster)?;
                    let surface = painter.surface().expect("a frame was submitted").clone();
                    (surface, FrameReport::Damage(stats))
                }
            };
            drop(engine_document);
            bytes = bytes.saturating_add(surface.premultiplied().len());
            if bytes > limits.max_dump_bytes {
                return Err(LoadError::Dump(DumpError::OutputLimit));
            }
            frames.push((surface, report));
        }
    }
    print_console(&document);
    let scroll = document.scroll_state();
    let live = document.accessibility_state();
    Ok(LoadedDocument {
        document: document.into_document(),
        applied_files: Some(applied_files),
        network: source.network,
        scroll,
        frames,
        controls: control_states(&live),
        live,
    })
}

/// Most script, timer and input turns one live frame runs; the rest wait
/// for the next frame, so a busy page cannot stall the window.
const LIVE_TURNS_PER_FRAME: usize = 1_000;
/// Most the page's clock advances per live frame.
const LIVE_MAX_STEP: Duration = Duration::from_secs(1);

/// A long-lived page (TD-182): the loaded document stays in this
/// renderer, input from the broker is delivered to it, scripts and timers
/// run between frames on a clock the broker advances, and each frame
/// request is answered with the page painted at the asked size, scale and
/// scroll position (scenes, ADR 0064). Ends, with the reason, when the
/// broker closes the channel or the page fails.
fn live_page(
    mut document: rustic_engine::ScriptedDocument<ManualClock>,
    source: &ScriptedSource,
    limits: LoaderLimits,
    applied_files: &AppliedLocalFiles,
) -> LoadError {
    use rustic_ipc::protocol::{
        AssistiveRequest, CompositionPhase, MODIFIER_ALT, MODIFIER_CONTROL, MODIFIER_META,
        MODIFIER_SHIFT, PageInput, Painted, RendererMessage,
    };
    let handled = || crate::renderer::answer_page(&RendererMessage::Handled);
    // The scroll positions come from the page's state (and the user's
    // scrolls applied to it), never from command options.
    let limits = LoaderLimits {
        scroll: (0.0, 0.0),
        scroll_boxes: [None; MAX_CLICKS],
        clicks: [None; MAX_CLICKS],
        report_scroll: false,
        report_a11y: false,
        ..limits
    };
    let raster = rustic_raster::RasterLimits::default();
    let mut view = limits.script_view;
    let mut scene = rustic_raster::Scene::new(view.scale(), raster);
    let mut painter = rustic_raster::Frames::new(view.scale(), raster);
    let cancellation = document.document().cancellation().clone();
    let encoding = document.document().parsed().encoding();
    let modifiers = |bits: u8| rustic_engine::Modifiers {
        control: bits & MODIFIER_CONTROL != 0,
        alt: bits & MODIFIER_ALT != 0,
        shift: bits & MODIFIER_SHIFT != 0,
        meta: bits & MODIFIER_META != 0,
    };
    let run = |document: &mut rustic_engine::ScriptedDocument<ManualClock>| {
        for _ in 0..LIVE_TURNS_PER_FRAME {
            if document.run_next()?.is_none() {
                break;
            }
        }
        Ok::<(), LoadError>(())
    };
    // Test hook: `crash-on-frame:N` / `hang-on-frame:N` make this
    // renderer fail at its Nth frame request; it only changes what this
    // untrusted process does, which the broker must survive anyway.
    let fault = std::env::var("RUSTIC_TEST_LIVE_RENDERER").ok();
    let fault_at = |name: &str| {
        fault
            .as_deref()
            .and_then(|fault| fault.strip_prefix(name))
            .and_then(|n| n.parse::<usize>().ok())
    };
    let (crash_at, hang_at) = (fault_at("crash-on-frame:"), fault_at("hang-on-frame:"));
    let mut frames = 0usize;
    let result = (|| -> Result<(), LoadError> {
        loop {
            let input = crate::renderer::next_page_input().map_err(LoadError::Broker)?;
            if matches!(input, PageInput::Frame(_)) {
                frames += 1;
                if crash_at == Some(frames) {
                    panic!("simulated live renderer crash");
                }
                if hang_at == Some(frames) {
                    loop {
                        std::thread::sleep(Duration::from_secs(3600));
                    }
                }
            }
            match input {
                PageInput::Click {
                    x,
                    y,
                    modifiers: bits,
                } => {
                    let zoom = view.zoom();
                    let (x, y) = (x / zoom, y / zoom);
                    let state = document.scroll_state();
                    let animations = document.animations();
                    let engine_document = document.document();
                    let list = display_list(LayoutInput {
                        scroll: Some(&state),
                        animations: Some(&*animations),
                        ..LayoutInput::new(
                            &engine_document,
                            &source.path,
                            limits,
                            view,
                            Some(applied_files),
                            source.network.as_ref(),
                        )
                    })?;
                    drop(engine_document);
                    let point = rustic_layout::CssPx::new(x)
                        .and_then(|x| Ok((x, rustic_layout::CssPx::new(y)?)))
                        .map(|(x, y)| rustic_layout::CssPoint { x, y });
                    if let Some(hit) = point.ok().and_then(|point| list.hit_test(point)) {
                        let event = rustic_engine::InputEvent::Click {
                            client_x: x as i32,
                            client_y: y as i32,
                            modifiers: modifiers(bits),
                        };
                        // A node the page removed meanwhile takes nothing.
                        if document.queue_input(hit.node, event).is_ok() {
                            run(&mut document)?;
                        }
                    }
                    handled().map_err(LoadError::Broker)?;
                }
                PageInput::Key {
                    down,
                    key,
                    code,
                    repeat,
                    modifiers: bits,
                } => {
                    let Some(target) = document.key_target() else {
                        handled().map_err(LoadError::Broker)?;
                        continue;
                    };
                    let key = rustic_engine::KeyInput {
                        key: rustic_dom::DomString::from(key.as_str()),
                        code: rustic_dom::DomString::from(code.as_str()),
                        repeat,
                        location: 0,
                        modifiers: modifiers(bits),
                    };
                    let event = if down {
                        rustic_engine::InputEvent::KeyDown(key)
                    } else {
                        rustic_engine::InputEvent::KeyUp(key)
                    };
                    if document.queue_input(target, event).is_ok() {
                        run(&mut document)?;
                    }
                    handled().map_err(LoadError::Broker)?;
                }
                PageInput::Composition { phase, text } => {
                    if let Some(target) = document.key_target() {
                        let text = rustic_dom::DomString::from(text.as_str());
                        let step = match phase {
                            CompositionPhase::Start => rustic_engine::Composition::Start,
                            CompositionPhase::Update => rustic_engine::Composition::Update(text),
                            CompositionPhase::End => rustic_engine::Composition::End(text),
                        };
                        let event = rustic_engine::InputEvent::Composition(step);
                        if document.queue_input(target, event).is_ok() {
                            run(&mut document)?;
                        }
                    }
                    handled().map_err(LoadError::Broker)?;
                }
                PageInput::Assistive { node, request } => {
                    let target = {
                        let engine_document = document.document();
                        node_for(engine_document.parsed().document(), node)
                    };
                    if let Some(target) = target {
                        let action = match request {
                            AssistiveRequest::Focus => rustic_engine::AssistiveAction::Focus,
                            AssistiveRequest::Click => rustic_engine::AssistiveAction::Click,
                            AssistiveRequest::SetValue(text) => {
                                rustic_engine::AssistiveAction::SetValue(text.as_str().into())
                            }
                            AssistiveRequest::SetSelection { anchor, focus } => {
                                rustic_engine::AssistiveAction::SetSelection { anchor, focus }
                            }
                        };
                        let event = rustic_engine::InputEvent::Assistive(action);
                        if document.queue_input(target, event).is_ok() {
                            run(&mut document)?;
                        }
                    }
                    handled().map_err(LoadError::Broker)?;
                }
                PageInput::Accessibility => {
                    let live = document.accessibility_state();
                    let state = document.scroll_state();
                    let animations = document.animations();
                    let engine_document = document.document();
                    let tree = tree_of(
                        LayoutInput {
                            scroll: Some(&state),
                            animations: Some(&*animations),
                            ..LayoutInput::new(
                                &engine_document,
                                &source.path,
                                limits,
                                view,
                                Some(applied_files),
                                source.network.as_ref(),
                            )
                        },
                        &live,
                    )?;
                    drop(engine_document);
                    let bytes = rustic_a11y::wire::encode(&tree);
                    crate::renderer::answer_page(&RendererMessage::Accessibility(bytes))
                        .map_err(LoadError::Broker)?;
                }
                PageInput::Frame(ask) => {
                    let next = rustic_layout::View::new(
                        f64::from(ask.width),
                        f64::from(ask.height),
                        ask.scale,
                        ask.zoom,
                    )
                    .map_err(|_| LoadError::Broker("the frame asks for an invalid view".into()))?;
                    if next != view {
                        if next.scale() != view.scale() {
                            scene = rustic_raster::Scene::new(next.scale(), raster);
                            painter = rustic_raster::Frames::new(next.scale(), raster);
                        }
                        view = next;
                        // Script layout reads see the new viewport.
                        document.set_layout_source(Box::new(LocalLayout {
                            path: source.path.clone(),
                            limits: LoaderLimits {
                                script_view: view,
                                ..limits
                            },
                            cancellation: cancellation.clone(),
                            encoding,
                            applied_files: applied_files.clone(),
                            network: source.network.clone(),
                        }))?;
                        document.rendering_inputs_changed()?;
                    }
                    if let Some((x, y)) = ask.scroll {
                        document.scroll_viewport(x, y)?;
                    }
                    let step = Duration::from_micros(ask.elapsed_us).min(LIVE_MAX_STEP);
                    document
                        .clock()
                        .advance(step)
                        .map_err(LoadError::ScriptClock)?;
                    run(&mut document)?;
                    document.offer_rendering_opportunity()?;
                    document.run_next_with_render(|_, _| ())?;
                    print_console(&document);
                    for popup in document.take_popups() {
                        let opened = match &source.network {
                            Some(session) => session.open_popup(&popup),
                            None => Err("popups open only in a split run (TD-178)".to_owned()),
                        };
                        if let Err(why) = opened {
                            eprintln!(
                                "rustic: popup {}: {why}",
                                EscapedText(&popup.url.serialize())
                            );
                        }
                    }
                    let state = document.scroll_state();
                    let animations = document.animations();
                    let live = document.accessibility_state();
                    let controls = control_states(&live);
                    let engine_document = document.document();
                    let input = LayoutInput {
                        scroll: Some(&state),
                        animations: Some(&*animations),
                        controls: Some(&controls),
                        ..LayoutInput::new(
                            &engine_document,
                            &source.path,
                            limits,
                            view,
                            Some(applied_files),
                            source.network.as_ref(),
                        )
                    };
                    // A focused text field: the host's input method opens
                    // at its box (TD-197).
                    let editing =
                        match live.focused {
                            Some(focused)
                                if rustic_a11y::editable(
                                    engine_document.parsed().document(),
                                    focused,
                                ) =>
                            {
                                let tree = tree_of(input, &live)?;
                                let bounds = tree
                                    .nodes
                                    .iter()
                                    .find(|node| node.id.names(focused))
                                    .and_then(|node| node.bounds);
                                Some(bounds.map_or((0.0, 0.0, 0.0, 0.0), |r| {
                                    (r.x, r.y, r.width, r.height)
                                }))
                            }
                            _ => None,
                        };
                    let surface = match render_scene(input, &mut scene)? {
                        Some((surface, _)) => {
                            painter = rustic_raster::Frames::new(view.scale(), raster);
                            surface
                        }
                        None => {
                            scene = rustic_raster::Scene::new(view.scale(), raster);
                            let list = display_list(input)?;
                            painter
                                .submit(list, &cancellation)
                                .map_err(LoadError::Raster)?;
                            painter.surface().expect("a frame was submitted").clone()
                        }
                    };
                    // Find in page: one more layout to locate the matches,
                    // marked over the frame (UI-01).
                    let mut pixels = surface.premultiplied().to_vec();
                    let found = match &ask.find {
                        Some((text, index)) => {
                            let query = crate::find::parse_query(text, *index as usize)
                                .map_err(LoadError::Broker)?;
                            let (tree, layout, ..) = block_layout(input)?;
                            let matches = crate::find::matches(&tree, &layout, &query);
                            let (active, _) = crate::find::report(&matches, query.index);
                            crate::find::highlight(
                                &mut pixels,
                                (surface.width(), surface.height()),
                                &matches,
                                active,
                                state.viewport,
                                view.scale().get(),
                            );
                            Some(crate::find::found(&matches, active))
                        }
                        None => None,
                    };
                    drop(engine_document);
                    let next_frame_us = if document.wants_animation_frame() {
                        Some(FRAME_INTERVAL.as_micros() as u64)
                    } else {
                        document
                            .time_until_next_timer()?
                            .map(|delay| u64::try_from(delay.as_micros()).unwrap_or(u64::MAX))
                    };
                    crate::renderer::answer_page(&RendererMessage::Painted(Painted {
                        surface: rustic_ipc::protocol::Surface {
                            width: surface.width(),
                            height: surface.height(),
                            pixels,
                            layers: Vec::new(),
                        },
                        scroll: state.viewport,
                        next_frame_us,
                        found,
                        editing,
                    }))
                    .map_err(LoadError::Broker)?;
                }
            }
        }
    })();
    match result {
        Ok(()) => LoadError::Cancelled,
        Err(error) => error,
    }
}

/// The loaded page as an automation session drives it, within the
/// load's task and timer-turn budgets.
struct SessionPage<'a> {
    source: &'a ScriptedSource,
    limits: LoaderLimits,
    applied_files: &'a AppliedLocalFiles,
    queued_turns: std::cell::Cell<usize>,
    turns: std::cell::Cell<usize>,
}

impl SessionPage<'_> {
    fn list(
        &self,
        document: &rustic_engine::ScriptedDocument<ManualClock>,
    ) -> Result<rustic_paint::DisplayList, LoadError> {
        let state = document.scroll_state();
        let animations = document.animations();
        let engine_document = document.document();
        display_list(LayoutInput {
            scroll: Some(&state),
            animations: Some(&*animations),
            ..LayoutInput::new(
                &engine_document,
                &self.source.path,
                self.limits,
                self.limits.script_view,
                Some(self.applied_files),
                self.source.network.as_ref(),
            )
        })
    }

    fn settle_counted(
        &self,
        document: &mut rustic_engine::ScriptedDocument<ManualClock>,
    ) -> Result<u64, LoadError> {
        let (mut queued, mut turns) = (self.queued_turns.get(), self.turns.get());
        let before = queued + turns;
        let result = settle(document, &mut queued, &mut turns);
        self.queued_turns.set(queued);
        self.turns.set(turns);
        result.map(|()| (queued + turns - before) as u64)
    }
}

impl crate::devtools::PageHost for SessionPage<'_> {
    fn hit_test(
        &self,
        document: &rustic_engine::ScriptedDocument<ManualClock>,
        x: f64,
        y: f64,
    ) -> Result<Option<rustic_dom::NodeId>, LoadError> {
        let list = self.list(document)?;
        let point = rustic_layout::CssPx::new(x)
            .and_then(|x| Ok((x, rustic_layout::CssPx::new(y)?)))
            .map(|(x, y)| rustic_layout::CssPoint { x, y });
        Ok(point
            .ok()
            .and_then(|point| list.hit_test(point))
            .map(|hit| hit.node))
    }

    fn screenshot(
        &self,
        document: &rustic_engine::ScriptedDocument<ManualClock>,
    ) -> Result<(u32, u32, Vec<u8>), LoadError> {
        let state = document.scroll_state();
        let animations = document.animations();
        let engine_document = document.document();
        let (_, surface) = rasterize_list(LayoutInput {
            scroll: Some(&state),
            animations: Some(&*animations),
            ..LayoutInput::new(
                &engine_document,
                &self.source.path,
                self.limits,
                self.limits.script_view,
                Some(self.applied_files),
                self.source.network.as_ref(),
            )
        })?;
        let png = rustic_raster::encode_png(&surface, self.limits.max_dump_bytes)
            .map_err(LoadError::Png)?;
        Ok((surface.width(), surface.height(), png))
    }

    fn settle(
        &self,
        document: &mut rustic_engine::ScriptedDocument<ManualClock>,
    ) -> Result<u64, LoadError> {
        self.settle_counted(document)
    }

    fn advance(
        &self,
        document: &mut rustic_engine::ScriptedDocument<ManualClock>,
        by: Duration,
    ) -> Result<u64, LoadError> {
        document
            .clock()
            .advance(by)
            .map_err(LoadError::ScriptClock)?;
        self.queued_turns.set(0);
        self.settle_counted(document)
    }
}

/// Run the tasks that are ready now, without advancing the clock.
fn run_ready(
    document: &mut rustic_engine::ScriptedDocument<ManualClock>,
    queued_turns: &mut usize,
) -> Result<(), LoadError> {
    while document.run_next()?.is_some() {
        *queued_turns += 1;
        if *queued_turns == MAX_SCRIPT_TIMER_TURNS {
            return Err(LoadError::ScriptTurnLimit);
        }
    }
    Ok(())
}

/// Run queued tasks and due timers until none remain, within the load's
/// task and timer-turn budgets.
fn settle(
    document: &mut rustic_engine::ScriptedDocument<ManualClock>,
    queued_turns: &mut usize,
    turns: &mut usize,
) -> Result<(), LoadError> {
    loop {
        if *queued_turns == MAX_SCRIPT_TIMER_TURNS {
            return Err(LoadError::ScriptTurnLimit);
        }
        if document.run_next()?.is_some() {
            *queued_turns += 1;
            continue;
        }
        let Some(delay) = document.time_until_next_timer()? else {
            return Ok(());
        };
        advance_script_timer(document.clock(), delay, turns)?;
        document.run_next()?;
    }
}

/// Write the page's console messages and reported errors to standard error.
fn print_console(document: &rustic_engine::ScriptedDocument<ManualClock>) {
    let log = document.take_console_messages();
    for message in &log.messages {
        let level = match message.level {
            rustic_engine::ConsoleLevel::Debug => "debug",
            rustic_engine::ConsoleLevel::Log => "log",
            rustic_engine::ConsoleLevel::Info => "info",
            rustic_engine::ConsoleLevel::Warn => "warn",
            rustic_engine::ConsoleLevel::Error => "error",
        };
        let cut = if message.truncated {
            " [truncated]"
        } else {
            ""
        };
        eprintln!("console.{level}: {}{cut}", message.text.escape_debug());
    }
    if log.dropped > 0 {
        eprintln!("console: {} further messages dropped", log.dropped);
    }
}

/// Eagerly supply the completed parser's local stylesheet/image resources
/// through the existing bounded host task pump. At most one staged resource
/// is retained awaiting delivery: each pending byte buffer is committed or
/// dropped before another file is read. Failures stay broken/on-demand.
fn queue_discovered_local_resources(
    document: &mut rustic_engine::ScriptedDocument<ManualClock>,
    document_url: &rustic_url::Url,
    limits: LoaderLimits,
    fetch: &LocalStylesheetFetcher,
    applied: &AppliedLocalFiles,
    cancellation: &CancellationToken,
    timer_turns: &mut usize,
) -> Result<usize, LoadError> {
    let mut requests = {
        let parsed = document.document();
        fetch.set_quirks(parsed.parsed().mode() == DocumentMode::Quirks);
        discover_local_resources(parsed.parsed().document(), document_url, limits)
    };
    if let Some(session) = fetch.network() {
        // Sheets a script waited for are already applied; a network
        // resource is fetched once per load.
        requests.retain(|(url, _)| !applied.contains(url));
        prefetch_resources(session, &requests);
    }
    let mut delivered = 0usize;
    for (url, kind) in requests {
        check_cancelled(cancellation)?;
        let staged = match kind {
            LocalResourceKind::Stylesheet => {
                fetch.stage_completion(applied, &url, limits.stylesheets.max_total_bytes)
            }
            LocalResourceKind::Image => crate::images::stage_image_completion(
                fetch,
                applied,
                &url,
                limits.images,
                cancellation,
            ),
        };
        let Ok(staged) = staged else {
            continue;
        };
        let snapshot = applied.clone();
        if document
            .queue_resource_completion(move || snapshot.commit(staged))
            .is_err()
        {
            // Existing renderer reads still use on-demand file loading when
            // admission is exhausted; do not retain an unqueued byte buffer.
            continue;
        }
        // Source alternation may select one ready timer ahead of the queued
        // completion. Once it has run, the resource task is the next
        // immediate source. Bound this by the same two-source alternation,
        // rather than holding every resource's bytes until EOF.
        for _ in 0..2 {
            if *timer_turns >= MAX_SCRIPT_TIMER_TURNS
                && !document.next_task_is_resource_completion()
            {
                return Err(LoadError::ScriptTurnLimit);
            }
            match document.run_next()? {
                Some(rustic_engine::ScriptTurn::ResourceCompletion { applied }) => {
                    delivered += usize::from(applied);
                    break;
                }
                Some(_) => {
                    // A ready timer can precede the resource task under
                    // source alternation. Such JS work must still count
                    // against the existing snapshot timer-turn ceiling.
                    *timer_turns += 1;
                }
                None => break,
            }
        }
    }
    Ok(delivered)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum LocalResourceKind {
    Stylesheet,
    Image,
}

/// Discover local subresource candidates from the completed DOM, bounded by
/// the existing per-kind document fetch limits. The document's first base href
/// is resolved once; the subsequent fetcher enforces the canonical file grant
/// and rejects all unsupported schemes and escaping links.
/// Start a network document's stylesheets and images side by side
/// (TD-165); each is still delivered in document order by whoever asks.
fn prefetch_resources(
    session: &dyn crate::network::Session,
    requests: &[(rustic_url::Url, LocalResourceKind)],
) {
    let ahead: Vec<_> = requests
        .iter()
        .map(|(url, kind)| {
            let kind = match kind {
                // Prefetches are keyed without the mode; the MIME check
                // uses the kind the loader asks for.
                LocalResourceKind::Stylesheet => ResourceKind::Stylesheet { quirks: false },
                LocalResourceKind::Image => ResourceKind::Image,
            };
            (url.clone(), kind)
        })
        .collect();
    session.prefetch(&ahead);
}

fn discover_local_resources(
    document: &Document,
    document_url: &rustic_url::Url,
    limits: LoaderLimits,
) -> Vec<(rustic_url::Url, LocalResourceKind)> {
    use crate::reftest::{attribute, is_html};

    let mut base = document_url.clone();
    if let Ok(walk) = document.walk(document.root()) {
        for (node, _) in walk {
            let Some(element) = rustic_style::DomElement::new(document, node) else {
                continue;
            };
            if is_html(element, "base") {
                if let Some(href) = attribute(element, "href") {
                    if let Ok(url) = rustic_url::Url::parse(&href, Some(document_url)) {
                        base = url;
                    }
                    break;
                }
            }
        }
    }
    let mut requests = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut sheets = 0usize;
    let mut images = 0usize;
    if let Ok(walk) = document.walk(document.root()) {
        for (node, _) in walk {
            let Some(element) = rustic_style::DomElement::new(document, node) else {
                continue;
            };
            let (kind, candidate) = if is_html(element, "link") {
                let rel = attribute(element, "rel").unwrap_or_default();
                let mut is_stylesheet = false;
                let mut alternate = false;
                for token in rel.split([' ', '\t', '\n', '\x0c', '\r']) {
                    is_stylesheet |= token.eq_ignore_ascii_case("stylesheet");
                    alternate |= token.eq_ignore_ascii_case("alternate");
                }
                let css_type = attribute(element, "type").is_none_or(|value| {
                    let value = value
                        .trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\x0c' | '\r'));
                    value.is_empty() || value.eq_ignore_ascii_case("text/css")
                });
                if !is_stylesheet
                    || alternate
                    || !css_type
                    || attribute(element, "disabled").is_some()
                    || sheets >= limits.stylesheets.max_sheets
                {
                    continue;
                }
                (LocalResourceKind::Stylesheet, attribute(element, "href"))
            } else if is_html(element, "img") {
                if images >= limits.images.max_images {
                    continue;
                }
                (LocalResourceKind::Image, attribute(element, "src"))
            } else {
                continue;
            };
            let Some(href) = candidate.filter(|s| !s.is_empty()) else {
                continue;
            };
            let Ok(url) = rustic_url::Url::parse(&href, Some(&base)) else {
                continue;
            };
            // Apply the same bound here as the resource cache, before
            // retaining keys for all discovered candidates. A document
            // with huge repeated URLs must not amplify parser memory.
            let key = url.serialize();
            if key.len() > MAX_APPLIED_URL_BYTES {
                continue;
            }
            // Multiple elements sharing one URL need only one completion
            // per kind. Each URL remains subject to the local file grant.
            if !seen.insert((kind, key)) {
                continue;
            }
            match kind {
                LocalResourceKind::Stylesheet => sheets += 1,
                LocalResourceKind::Image => images += 1,
            }
            requests.push((url, kind));
        }
    }
    requests
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

/// The script requests of a scripted load (TD-134). A network document
/// starts every requested script, and the stylesheets parsed before them,
/// side by side; the load still completes requests in the order the
/// engine asked for them. Before a parser-blocking or deferred script
/// runs, the stylesheets parsed before it are loaded (HTML's "script-
/// blocking style sheets"), and timers that fall due while a request is
/// outstanding run first, as the event loop would run them.
struct ScriptRequests {
    document_url: rustic_url::Url,
    queue: std::collections::VecDeque<rustic_engine::ClassicScriptRequest>,
    /// Stylesheets already started ahead of the parser.
    started: std::collections::HashSet<String>,
    /// Stylesheets already loaded for a script.
    loaded: std::collections::HashSet<String>,
}

impl ScriptRequests {
    fn new(document_url: rustic_url::Url) -> Self {
        Self {
            document_url,
            queue: std::collections::VecDeque::new(),
            started: std::collections::HashSet::new(),
            loaded: std::collections::HashSet::new(),
        }
    }

    /// The stylesheets in the tree parsed so far.
    fn parsed_sheets(
        &self,
        load: &ClassicScriptLoad<ManualClock>,
        limits: LoaderLimits,
    ) -> Vec<rustic_url::Url> {
        let Some(document) = load.live_document() else {
            return Vec::new();
        };
        discover_local_resources(&document, &self.document_url, limits)
            .into_iter()
            .filter(|(_, kind)| *kind == LocalResourceKind::Stylesheet)
            .map(|(url, _)| url)
            .collect()
    }

    /// Complete the next requested script; `false` when none is waiting.
    fn complete_next(
        &mut self,
        load: &mut ClassicScriptLoad<ManualClock>,
        fetch: &LocalStylesheetFetcher,
        applied: &AppliedLocalFiles,
        limits: LoaderLimits,
        turns: &mut usize,
    ) -> Result<bool, LoadError> {
        let mut fresh = Vec::new();
        while let Some(request) = load.next_request() {
            fresh.push(request);
        }
        if let (Some(session), false) = (fetch.network(), fresh.is_empty()) {
            let mut ahead: Vec<_> = fresh
                .iter()
                .map(|request| (request.url.clone(), script_kind(request)))
                .collect();
            for url in self.parsed_sheets(load, limits) {
                if self.started.insert(url.serialize()) {
                    ahead.push((url, ResourceKind::Stylesheet { quirks: false }));
                }
            }
            session.prefetch(&ahead);
        }
        self.queue.extend(fresh);
        let Some(request) = self.queue.pop_front() else {
            return Ok(false);
        };
        let started = std::time::Instant::now();
        if request.mode != rustic_engine::ClassicScriptMode::Async {
            fetch.set_quirks(load.live_document_mode() == Some(DocumentMode::Quirks));
            let max_bytes = limits.stylesheets.max_total_bytes;
            for url in self.parsed_sheets(load, limits) {
                if self.loaded.insert(url.serialize()) {
                    // A failed sheet is recorded too: it no longer blocks.
                    let _ = applied.read(&url, max_bytes, || fetch.fetch_bytes(&url, max_bytes));
                }
            }
        }
        let fetched = match fetch.network() {
            Some(session) => session.fetch(&request.url, &script_kind(&request), request.max_bytes),
            None => fetch.fetch_bytes(&request.url, request.max_bytes),
        };
        let source = match fetched {
            Ok(source) => Ok(source),
            Err(FetchError::Blocked) => Err(ClassicFetchError::Blocked),
            Err(FetchError::NotFound) => Err(ClassicFetchError::NotFound),
            Err(FetchError::TooLarge) => Err(ClassicFetchError::TooLarge),
            Err(FetchError::Failed) => Err(ClassicFetchError::Failed),
            Err(FetchError::Cancelled) => return Err(LoadError::Cancelled),
        };
        // A network document's clock moves with the time spent waiting; a
        // local file arrives at once, so only timers already due run.
        let waited = if fetch.network().is_some() {
            started.elapsed()
        } else {
            Duration::ZERO
        };
        run_timers_within(load, waited, turns)?;
        if let (Some(session), Ok(bytes)) = (crate::devtools::active(), &source) {
            session.note_script(&request.url, bytes);
        }
        let charset = fetch
            .network()
            .and_then(|session| session.charset_of(&request.url))
            .and_then(|label| Encoding::for_label(&label));
        load.complete_script_with_charset(
            request.id,
            source.as_deref().map_err(|error| *error),
            charset,
        )?;
        Ok(true)
    }
}

/// How a script request is fetched.
fn script_kind(request: &rustic_engine::ClassicScriptRequest) -> ResourceKind {
    ResourceKind::Script {
        nonce: request.nonce.clone(),
        parser_inserted: request.parser_inserted,
        module: request.kind == rustic_engine::ScriptResourceKind::JavaScriptModule,
    }
}

/// Run the timers that fall due within `budget` of the load's clock while
/// a request is outstanding, then move the clock past the rest of it. The
/// timer-turn ceiling stops the interleaving early; the load's own timer
/// loop reports it if the page keeps going.
fn run_timers_within(
    load: &mut ClassicScriptLoad<ManualClock>,
    mut budget: Duration,
    turns: &mut usize,
) -> Result<(), LoadError> {
    while *turns < MAX_SCRIPT_TIMER_TURNS {
        let Some(delay) = load.time_until_next_timer()? else {
            break;
        };
        if delay > budget {
            break;
        }
        load.clock()
            .expect("active load")
            .advance(delay)
            .map_err(LoadError::ScriptClock)?;
        budget -= delay;
        *turns += 1;
        load.run_next()?;
    }
    if !budget.is_zero() {
        load.clock()
            .expect("active load")
            .advance(budget)
            .map_err(LoadError::ScriptClock)?;
    }
    Ok(())
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
    parsed: &LoadedDocument,
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
        SheetStatus::BlockedByPolicy => "blocked: Content Security Policy",
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
    parsed: &LoadedDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    let document = parsed.parsed().document();
    let (styles, sheets, _) = compute_styles(parsed.layout_input(path, limits, default_view()))?;
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

/// Computed styles, the document's sheets and its applicable `@font-face` rules.
type Styles = (
    Vec<rustic_style::ElementStyle>,
    Vec<rustic_style::StyleElementSheet>,
    Vec<std::sync::Arc<rustic_style::FontFaceRule>>,
);

/// Share the same local stylesheet policy and M2 reference cascade in all dumps.
fn compute_styles(input: LayoutInput<'_>) -> Result<Styles, LoadError> {
    let LayoutInput {
        document,
        mode,
        encoding,
        path,
        limits,
        view,
        cancellation,
        applied_files,
        network,
        ..
    } = input;
    let node_error = |error| LoadError::Dump(DumpError::Node(error));
    check_cancelled(cancellation)?;
    let parser =
        CssParser::new(CssLimits::default(), cancellation.clone()).map_err(LoadError::Css)?;
    let (document_url, fetcher) = match network {
        Some(session) => (
            Some(session.document_url().clone()),
            Some(LocalStylesheetFetcher::for_network(
                Rc::clone(session),
                limits.read_chunk_bytes,
                cancellation.clone(),
            )),
        ),
        None => {
            let canonical = fs::canonicalize(path).map_err(LoadError::Io)?;
            let fetcher = canonical.parent().and_then(|root| {
                LocalStylesheetFetcher::new(
                    root.to_path_buf(),
                    limits.read_chunk_bytes,
                    cancellation.clone(),
                )
            });
            (file_url(&canonical), fetcher)
        }
    };
    let mut local = fetcher.map(|fetcher| {
        let fetcher = fetcher.with_environment_encoding(encoding);
        fetcher.set_quirks(mode == DocumentMode::Quirks);
        match applied_files {
            Some(applied) => fetcher.with_applied_files(applied.clone()),
            None => fetcher,
        }
    });
    let mut no_fetch = NoFetch;
    let fetcher: &mut dyn StylesheetFetcher = match local.as_mut() {
        Some(local) => local,
        None => &mut no_fetch,
    };
    if let (Some(session), Some(url)) = (network, document_url.as_ref()) {
        // Only the cascade reads here, so only its sheets start early.
        let mut sheets = discover_local_resources(document, url, limits);
        sheets.retain(|(_, kind)| matches!(kind, LocalResourceKind::Stylesheet));
        prefetch_resources(session.as_ref(), &sheets);
    }
    let mut loader = StyleLoader::new(fetcher, limits.stylesheets).with_document_encoding(encoding);
    if let Some(session) = network.filter(|session| !session.csp().is_empty()) {
        loader =
            loader.with_inline_policy(Rc::new(crate::network::StylePolicy(Rc::clone(session))));
    }
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
    let styles = match input.animations {
        Some(animations) => rustic_style::style_tree_animated(
            document,
            document.root(),
            &cascade,
            &parser,
            &cx,
            &compute,
            &HashMap::new(),
            &mut animations.borrow_mut(),
        ),
        None => style_tree(document, document.root(), &cascade, &parser, &cx, &compute),
    }
    .map_err(node_error)?;
    check_cancelled(cancellation)?;
    let font_faces = cascade.font_faces().into_iter().cloned().collect();
    Ok((styles, sheets, font_faces))
}

/// Dump an initial principal-box snapshot, before anonymous/line box construction.
/// This has no layout positions, shaped text, images or paint output yet.
pub fn dump_boxes(
    parsed: &LoadedDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    box_tree(parsed.layout_input(path, limits, default_view()))?
        .dump(limits.max_dump_bytes)
        .map_err(LoadError::Layout)
}

/// Normalize flow boxes and dump anonymous wrappers/inline continuations.
pub fn dump_formatting(
    parsed: &LoadedDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    let principal = box_tree(parsed.layout_input(path, limits, default_view()))?;
    rustic_layout::FormattingTree::build(&principal, limits.formatting, parsed.cancellation())
        .and_then(|tree| tree.dump(limits.max_dump_bytes))
        .map_err(LoadError::Formatting)
}

fn box_tree(input: LayoutInput<'_>) -> Result<rustic_layout::BoxTree, LoadError> {
    box_tree_with_styles(input).map(|(tree, ..)| tree)
}

type StyledTree = (
    rustic_layout::BoxTree,
    Vec<rustic_style::ElementStyle>,
    Vec<std::sync::Arc<rustic_style::FontFaceRule>>,
);

fn box_tree_with_styles(input: LayoutInput<'_>) -> Result<StyledTree, LoadError> {
    let (styles, _, font_faces) = compute_styles(input)?;
    let tree = rustic_layout::BoxTree::build_with_controls(
        input.document,
        input.document.root(),
        &styles,
        layout_limits(input.limits),
        input.controls.unwrap_or(&ControlStates::new()),
        input.cancellation,
    )
    .map_err(LoadError::Layout)?;
    Ok((tree, styles, font_faces))
}

/// The view every dump command uses: a 1280×720 window at ratio 1 and
/// 100% zoom.
pub fn default_view() -> rustic_layout::View {
    rustic_layout::View::new(1280.0, 720.0, 1.0, 1.0).expect("the default view is valid")
}

fn layout_limits(limits: LoaderLimits) -> rustic_layout::LayoutLimits {
    let mut layout = rustic_layout::LayoutLimits::default();
    layout.max_nodes = limits.parser.document.max_nodes;
    layout.max_boxes = limits.parser.document.max_nodes;
    layout.max_depth = limits.parser.document.max_depth;
    layout
}

/// Compute used block rectangles with adjoining-margin collapse for the static subset.
/// Unsupported formatting returns no partial output; this does not paint pixels.
pub fn dump_layout(
    parsed: &LoadedDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    let (_, layout, ..) = block_layout(parsed.layout_input(path, limits, default_view()))?;
    layout.dump(limits.max_dump_bytes).map_err(LoadError::Flow)
}

/// The accessibility tree (a11y.semantics, ADR 0073) of the laid-out
/// page: what rendered, where (viewport coordinates after scrolling) and
/// the page's live focus and control state.
pub(crate) fn accessibility_tree(
    parsed: &LoadedDocument,
    path: &Path,
    limits: LoaderLimits,
    view: rustic_layout::View,
) -> Result<rustic_a11y::AxTree, LoadError> {
    tree_of(parsed.layout_input(path, limits, view), &parsed.live)
}

fn tree_of(
    input: LayoutInput<'_>,
    live: &rustic_a11y::LiveState,
) -> Result<rustic_a11y::AxTree, LoadError> {
    let rendering = match block_layout(input) {
        Ok((tree, layout, viewport, ..)) => {
            let layout = scrolled(layout, viewport, input.limits, input.scroll)?;
            rendering_of(&tree, Some(&layout))?
        }
        // The block reference cannot lay out everything yet (form controls
        // are special elements): what generated boxes still decides what
        // is hidden, without bounds.
        Err(LoadError::Flow(_) | LoadError::Formatting(_)) => {
            let (tree, ..) = box_tree_with_styles(input)?;
            rendering_of(&tree, None)?
        }
        Err(error) => return Err(error),
    };
    Ok(rustic_a11y::build(
        input.document,
        Some(&rendering),
        live,
        rustic_a11y::TreeLimits::default(),
    ))
}

/// The live DOM node an accessibility id names, if it is still there.
fn node_for(document: &Document, id: u64) -> Option<rustic_dom::NodeId> {
    let id = rustic_a11y::AxId(id);
    document
        .walk(document.root())
        .ok()?
        .map(|(node, _)| node)
        .find(|&node| id.names(node))
}

/// Which DOM nodes generated boxes, their fragments' union in CSS pixels,
/// and which elements are `visibility: hidden`.
fn rendering_of(
    tree: &rustic_layout::BoxTree,
    layout: Option<&rustic_layout::BlockLayout>,
) -> Result<rustic_a11y::Rendering, LoadError> {
    use rustic_css::{PropertyId, PropertyValue, Visibility};
    let mut rendering = rustic_a11y::Rendering::new();
    let boxes = tree.boxes();
    for snapshot in boxes {
        rendering.mark_rendered(snapshot.node());
        if matches!(snapshot.kind(), rustic_layout::BoxKind::Element(_))
            && snapshot.style().is_some_and(|style| {
                !matches!(
                    style.get(PropertyId::Visibility),
                    PropertyValue::Visibility(Visibility::Visible)
                )
            })
        {
            rendering.mark_invisible(snapshot.node());
        }
    }
    let Some(layout) = layout else {
        return Ok(rendering);
    };
    let fragments = layout.fragment_tree().map_err(LoadError::Flow)?;
    for node in fragments.nodes() {
        let source = match node.kind() {
            rustic_layout::FragmentKind::Block(i) => {
                layout.fragments().get(i).map(|f| f.source_box())
            }
            rustic_layout::FragmentKind::Inline(i) => {
                layout.inlines().get(i).map(|f| f.source_box())
            }
            rustic_layout::FragmentKind::Text(i) => layout.texts().get(i).map(|f| f.source_box()),
            rustic_layout::FragmentKind::Line(_) => None,
        };
        let Some(snapshot) = source.and_then(|i| boxes.get(i)) else {
            continue;
        };
        let rect = node.rect();
        rendering.add_fragment(
            snapshot.node(),
            rustic_a11y::Rect {
                x: rect.origin().x.get(),
                y: rect.origin().y.get(),
                width: rect.size().width().get(),
                height: rect.size().height().get(),
            },
        );
    }
    Ok(rendering)
}

/// `dump-a11y`: the accessibility tree as text.
pub fn dump_a11y(
    parsed: &LoadedDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    accessibility_tree(parsed, path, limits, default_view())?
        .dump(limits.max_dump_bytes)
        .map_err(|_| LoadError::Dump(DumpError::OutputLimit))
}

/// The fonts every layout and paint command uses: the bundled test faces,
/// with every generic family and the fallback mapped to them. No system
/// fonts are read.
fn fonts() -> &'static rustic_text::FontDatabase {
    crate::fonts::database()
}

/// The fonts one document lays out and paints with: [`fonts`], plus the
/// faces its `@font-face` rules load ([`crate::web_fonts`]).
type DocumentFonts = std::borrow::Cow<'static, rustic_text::FontDatabase>;

/// Load the `@font-face` sources of a document under the image policy:
/// through the applied snapshot (and network session) when there is one,
/// else as local files in the document's directory.
fn document_fonts(
    rules: &[std::sync::Arc<rustic_style::FontFaceRule>],
    path: &Path,
    limits: LoaderLimits,
    cancellation: &CancellationToken,
    applied: Option<(&AppliedLocalFiles, Option<&dyn crate::network::Session>)>,
) -> DocumentFonts {
    use std::borrow::Cow;
    if rules.is_empty() {
        return Cow::Borrowed(fonts());
    }
    let root = path.parent();
    let chunk = limits.read_chunk_bytes;
    let read_local = |url: &rustic_url::Url, max: usize| -> Result<Vec<u8>, FetchError> {
        let file = crate::reftest::resolve(&url.serialize(), None, root)
            .map_err(|_| FetchError::Blocked)?;
        crate::images::read_bounded(&file, max, chunk).ok_or(FetchError::Failed)
    };
    let loaded = crate::web_fonts::database(
        rules,
        fonts(),
        crate::web_fonts::WebFontLimits::default(),
        |url, max| {
            if cancellation.check().is_err() {
                return None;
            }
            match applied {
                Some((applied, network)) => applied
                    .read(url, max, || match network {
                        Some(session) => {
                            session.fetch(url, &crate::network::ResourceKind::Font, max)
                        }
                        None => read_local(url, max),
                    })
                    .ok()
                    .map(|bytes| bytes.to_vec()),
                None => read_local(url, max).ok(),
            }
        },
    );
    loaded.map_or(Cow::Borrowed(fonts()), Cow::Owned)
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
    applied_files: Option<&'a AppliedLocalFiles>,
    network: Option<&'a Rc<dyn Session>>,
    /// The scroll positions scripts left, for painting.
    scroll: Option<&'a rustic_engine::ScrollState>,
    /// The document's running animations, for a scripted page's frames.
    animations: Option<&'a RefCell<rustic_style::AnimationController>>,
    /// Form controls' live values and checkedness, for a scripted page.
    controls: Option<&'a ControlStates>,
}

/// Form controls' live state by element (TD-219).
type ControlStates = HashMap<rustic_dom::NodeId, rustic_layout::ControlState>;

/// The live control state scripts and input left, for box construction:
/// text fields' current values and checkable inputs' checkedness.
fn control_states(live: &rustic_a11y::LiveState) -> ControlStates {
    live.controls
        .iter()
        .map(|(&node, control)| {
            (
                node,
                rustic_layout::ControlState {
                    value: control.value.as_ref().map(|v| v.encode_utf16().collect()),
                    checked: control
                        .checked
                        .map(|checked| checked == rustic_a11y::Checked::True),
                },
            )
        })
        .collect()
}

impl<'a> LayoutInput<'a> {
    fn new(
        parsed: &'a EngineDocument,
        path: &'a Path,
        limits: LoaderLimits,
        view: rustic_layout::View,
        applied_files: Option<&'a AppliedLocalFiles>,
        network: Option<&'a Rc<dyn Session>>,
    ) -> Self {
        Self {
            document: parsed.parsed().document(),
            mode: parsed.parsed().mode(),
            encoding: parsed.parsed().encoding(),
            path,
            limits,
            view,
            cancellation: parsed.cancellation(),
            applied_files,
            network,
            scroll: None,
            animations: None,
            controls: None,
        }
    }
}

#[derive(Clone, Debug)]
struct LocalLayout {
    path: PathBuf,
    limits: LoaderLimits,
    cancellation: CancellationToken,
    encoding: Encoding,
    applied_files: AppliedLocalFiles,
    network: Option<Rc<dyn Session>>,
}

// CSSOM lists supported longhands in lexicographical order before custom properties.
fn cssom_longhands() -> &'static [PropertyId] {
    static ORDER: std::sync::LazyLock<Vec<PropertyId>> = std::sync::LazyLock::new(|| {
        let mut properties = PropertyId::ALL.to_vec();
        properties.sort_unstable_by_key(|property| property.name());
        properties
    });
    &ORDER
}

// CSSOM §9 uses the existing untransformed fragment for box-dependent values.
#[derive(Clone, Copy)]
struct CssomBox {
    size: rustic_layout::CssSize,
    margin: rustic_layout::Edges,
    padding: rustic_layout::Edges,
}

impl CssomBox {
    fn from_fragment(
        fragment: rustic_layout::BlockFragment,
        style: &ComputedStyle,
    ) -> Result<Self, LayoutReadError> {
        let size = if matches!(
            style.get(PropertyId::BoxSizing),
            PropertyValue::BoxSizing(BoxSizing::BorderBox)
        ) {
            fragment.border().size()
        } else {
            fragment.content().size()
        };
        let (margin, padding) = fragment
            .resolved_edges(style)
            .map_err(|_| LayoutReadError::Unavailable)?;
        Ok(Self {
            size,
            margin,
            padding,
        })
    }

    fn length(self, property: PropertyId) -> Option<rustic_layout::CssPx> {
        Some(match property {
            PropertyId::Width => self.size.width(),
            PropertyId::Height => self.size.height(),
            PropertyId::MarginTop => self.margin.top,
            PropertyId::MarginRight => self.margin.right,
            PropertyId::MarginBottom => self.margin.bottom,
            PropertyId::MarginLeft => self.margin.left,
            PropertyId::PaddingTop => self.padding.top,
            PropertyId::PaddingRight => self.padding.right,
            PropertyId::PaddingBottom => self.padding.bottom,
            PropertyId::PaddingLeft => self.padding.left,
            _ => return None,
        })
    }
}

fn cssom_value(
    style: &ComputedStyle,
    layout_box: Option<CssomBox>,
    property: PropertyId,
) -> String {
    if let Some(pixels) = layout_box.and_then(|b| b.length(style.physical(property))) {
        return Length::px(pixels.get()).to_string();
    }
    let computed = style.get(property);
    if let PropertyValue::LineHeight(LineHeight::Number(factor)) = computed {
        // Keep the serialized length finite for very large valid factors.
        let font_size = style.font_size_px().min(f64::MAX);
        return Length::px((factor * font_size).min(f64::MAX)).to_string();
    }
    computed.to_string()
}

impl LayoutSource for LocalLayout {
    fn measure_layout(
        &self,
        document: &Document,
        mode: DocumentMode,
    ) -> Result<LayoutGeometry, LayoutReadError> {
        self.measure(document, mode, None)
    }

    fn measure_layout_animated(
        &self,
        document: &Document,
        mode: DocumentMode,
        animations: &RefCell<rustic_style::AnimationController>,
    ) -> Result<LayoutGeometry, LayoutReadError> {
        self.measure(document, mode, Some(animations))
    }
}

impl LocalLayout {
    fn measure(
        &self,
        document: &Document,
        mode: DocumentMode,
        animations: Option<&RefCell<rustic_style::AnimationController>>,
    ) -> Result<LayoutGeometry, LayoutReadError> {
        let (tree, layout, viewport, loaded, styles, _) = block_layout(LayoutInput {
            document,
            mode,
            encoding: self.encoding,
            path: &self.path,
            limits: self.limits,
            view: self.limits.script_view,
            cancellation: &self.cancellation,
            applied_files: Some(&self.applied_files),
            network: self.network.as_ref(),
            scroll: None,
            animations,
            controls: None,
        })
        .map_err(|_| LayoutReadError::Unavailable)?;
        // Public block fragments omit anonymous wrappers and include atomic
        // inlines. Ordinary inline fragments retain their computed values.
        let fragments: HashMap<_, _> = layout
            .fragments()
            .iter()
            .map(|fragment| (fragment.node(), fragment))
            .collect();
        let mut box_sizes = HashMap::new();
        for style in &styles {
            self.cancellation
                .check()
                .map_err(|_| LayoutReadError::Unavailable)?;
            let Some(fragment) = fragments.get(&style.node) else {
                continue;
            };
            let (_, padding) = fragment
                .resolved_edges(&style.computed)
                .map_err(|_| LayoutReadError::Unavailable)?;
            let size = |size: rustic_layout::CssSize| [size.width().get(), size.height().get()];
            box_sizes.insert(
                style.node,
                rustic_engine::BoxSizes {
                    content: size(fragment.content().size()),
                    border: size(fragment.border().size()),
                    padding: [padding.left.get(), padding.top.get()],
                },
            );
        }
        let (scroll_ranges, viewport_scroll_range, scroll_chains) =
            scroll_geometry(&layout, viewport);
        Ok(LayoutGeometry {
            box_sizes,
            offset_widths: layout
                .offset_widths(&tree, &self.cancellation)
                .map_err(|_| LayoutReadError::Unavailable)?,
            client_rects: rustic_paint::client_rects(&tree, &layout, &self.cancellation)
                .map_err(|_| LayoutReadError::Unavailable)?,
            computed_styles: styles
                .into_iter()
                .map(|style| {
                    self.cancellation
                        .check()
                        .map_err(|_| LayoutReadError::Unavailable)?;
                    let layout_box = fragments
                        .get(&style.node)
                        .map(|&&fragment| CssomBox::from_fragment(fragment, &style.computed))
                        .transpose()?;
                    // Serialized on the first declaration read, not per flush (TD-148).
                    let computed = style.computed;
                    Ok((
                        style.node,
                        ComputedStyleSnapshot::lazy(move || ComputedStyleRecord {
                            longhands: cssom_longhands()
                                .iter()
                                .map(|&property| {
                                    (
                                        property.name(),
                                        cssom_value(&computed, layout_box, property),
                                    )
                                })
                                .collect(),
                            custom_properties: computed
                                .custom_properties()
                                .iter()
                                .map(|(name, value)| (name.to_owned(), value.to_string()))
                                .collect(),
                        }),
                    ))
                })
                .collect::<Result<_, LayoutReadError>>()?,
            scroll_ranges,
            viewport_scroll_range,
            viewport_size: [viewport.width().get(), viewport.height().get()],
            scroll_chains,
            images: image_states(&loaded),
        })
    }
}

/// The current request of each `img` the layout's image load requested.
fn image_states(
    loaded: &crate::images::LoadedImages,
) -> HashMap<rustic_dom::NodeId, rustic_engine::ImageState> {
    let sizes: HashMap<_, _> = loaded.sizes.iter().copied().collect();
    loaded
        .sources
        .iter()
        .map(|(&node, url)| {
            let natural_size = loaded.images.contains_key(&node).then(|| {
                sizes
                    .get(&node)
                    .and_then(|size| Some([size.width()?.get(), size.height()?.get()]))
            });
            (
                node,
                rustic_engine::ImageState {
                    current_src: url.clone(),
                    natural_size: natural_size.flatten(),
                },
            )
        })
        .collect()
}

/// Scroll ranges and chains for script scrolling (ADR 0064): each scroll
/// container's range by element, the viewport's, and for each element
/// whether the viewport moves it and which containers do.
#[allow(
    clippy::type_complexity,
    reason = "the three geometry maps, built together"
)]
fn scroll_geometry(
    layout: &rustic_layout::BlockLayout,
    viewport: rustic_layout::CssSize,
) -> (
    HashMap<rustic_dom::NodeId, [f64; 4]>,
    [f64; 4],
    HashMap<rustic_dom::NodeId, (bool, Vec<rustic_dom::NodeId>)>,
) {
    let flat =
        |range: rustic_layout::ScrollRange| [range.min.0, range.min.1, range.max.0, range.max.1];
    let fragments = layout.fragments();
    let mut ranges = HashMap::new();
    let mut chains: Vec<(bool, Vec<rustic_dom::NodeId>)> = Vec::with_capacity(fragments.len());
    let mut by_node = HashMap::new();
    for (index, fragment) in fragments.iter().enumerate() {
        if let Some(range) = layout.scroll_range(index) {
            ranges.insert(fragment.node(), flat(range));
        }
        let fixed = fragment.scheme() == rustic_layout::PositionScheme::Fixed;
        let chain = match fragment.containing_block() {
            _ if fixed => (false, Vec::new()),
            Some(c) if c < index => {
                let (viewport, mut containers) = chains[c].clone();
                if layout.scroll_range(c).is_some() {
                    containers.push(fragments[c].node());
                }
                (viewport, containers)
            }
            _ => (true, Vec::new()),
        };
        by_node
            .entry(fragment.node())
            .or_insert_with(|| chain.clone());
        chains.push(chain);
    }
    (
        ranges,
        flat(layout.viewport_scroll_range(viewport)),
        by_node,
    )
}

/// A laid-out document and what painting it needs.
type Laidout = (
    rustic_layout::BoxTree,
    rustic_layout::BlockLayout,
    rustic_layout::CssSize,
    crate::images::LoadedImages,
    Vec<rustic_style::ElementStyle>,
    DocumentFonts,
);

fn block_layout(input: LayoutInput<'_>) -> Result<Laidout, LoadError> {
    let (mut tree, styles, font_faces) = box_tree_with_styles(input)?;
    let LayoutInput {
        document,
        path,
        limits,
        view,
        cancellation,
        applied_files,
        network,
        ..
    } = input;
    let canonical = match network {
        Some(_) => PathBuf::new(),
        None => fs::canonicalize(path).map_err(LoadError::Io)?,
    };
    let viewport = view.viewport();
    let view_info = crate::srcset::ViewInfo {
        width: viewport.width().get(),
        height: viewport.height().get(),
        ratio: view.scale().get(),
    };
    let load =
        |tree: &rustic_layout::BoxTree, deferred: &HashSet<rustic_dom::NodeId>| match applied_files
        {
            Some(applied) => crate::images::load_applied(
                document,
                tree,
                &canonical,
                limits.images,
                crate::images::ImageRead {
                    chunk_bytes: limits.read_chunk_bytes,
                    view: view_info,
                    deferred,
                },
                cancellation,
                Some((applied, network.map(|session| &**session))),
            ),
            None => crate::images::load_applied(
                document,
                tree,
                &canonical,
                limits.images,
                crate::images::ImageRead {
                    chunk_bytes: limits.read_chunk_bytes,
                    view: view_info,
                    deferred,
                },
                cancellation,
                None,
            ),
        };
    // Lazy images (HTML §2.5.7) wait until layout puts them near the
    // viewport; each round that loads some can bring more into view.
    let mut deferred = crate::images::lazy_images(document, &tree);
    let mut images = load(&tree, &deferred);
    let fonts = document_fonts(
        &font_faces,
        &canonical,
        limits,
        cancellation,
        applied_files.map(|applied| (applied, network.map(|session| &**session))),
    );
    let compute = |tree: &rustic_layout::BoxTree| {
        rustic_layout::BlockLayout::compute_with_fonts(
            tree,
            viewport,
            layout_limits(limits),
            &fonts,
            cancellation,
        )
        .map_err(LoadError::Flow)
    };
    let mut layout = None;
    for _ in 0..=MAX_LAZY_ROUNDS {
        if !images.sizes.is_empty() {
            tree.set_natural_sizes(&images.sizes)
                .map_err(LoadError::Layout)?;
        }
        let laid = compute(&tree)?;
        let near = if deferred.is_empty() {
            HashSet::new()
        } else {
            near_viewport(&laid, viewport, limits, input.scroll, &deferred)?
        };
        layout = Some(laid);
        if near.is_empty() {
            break;
        }
        deferred.retain(|node| !near.contains(node));
        images = load(&tree, &deferred);
    }
    let layout = layout.expect("at least one round");
    Ok((tree, layout, viewport, images, styles, fonts))
}

/// Layout rounds that may load more lazy images; the rest stay unloaded.
const MAX_LAZY_ROUNDS: usize = 4;

/// The `candidates` whose border box, at the page's scroll positions, is
/// within [`crate::images::LAZY_LOAD_MARGIN`] of the viewport.
fn near_viewport(
    layout: &rustic_layout::BlockLayout,
    viewport: rustic_layout::CssSize,
    limits: LoaderLimits,
    script: Option<&rustic_engine::ScrollState>,
    candidates: &HashSet<rustic_dom::NodeId>,
) -> Result<HashSet<rustic_dom::NodeId>, LoadError> {
    let scrolled;
    let layout = match scroll_positions(layout, limits, script) {
        Some(positions) => {
            scrolled = layout
                .scrolled(viewport, &positions)
                .map_err(LoadError::Flow)?;
            &scrolled
        }
        None => layout,
    };
    let margin = crate::images::LAZY_LOAD_MARGIN;
    Ok(layout
        .fragments()
        .iter()
        .filter(|fragment| candidates.contains(&fragment.node()))
        .filter(|fragment| {
            let border = fragment.border();
            near_rect(
                border.origin().x.get(),
                border.origin().y.get(),
                border.size().width().get(),
                border.size().height().get(),
                (viewport.width().get(), viewport.height().get()),
                margin,
            )
        })
        .map(|fragment| fragment.node())
        .collect())
}

/// Whether the rectangle at (`x`, `y`) of `width`×`height` comes within
/// `margin` of a viewport of size `view` at the origin.
fn near_rect(x: f64, y: f64, width: f64, height: f64, view: (f64, f64), margin: f64) -> bool {
    x + width >= -margin && y + height >= -margin && x <= view.0 + margin && y <= view.1 + margin
}

/// The layout with the command's `--scroll` and `--scroll-box` positions
/// applied (ADR 0064); the unscrolled layout when there are none.
/// The command's and the scripts' scroll positions (ADR 0064): where
/// scripts left the page, with `--scroll` and `--scroll-box` on top;
/// `None` when nothing is scrolled.
fn scroll_positions(
    layout: &rustic_layout::BlockLayout,
    limits: LoaderLimits,
    script: Option<&rustic_engine::ScrollState>,
) -> Option<rustic_layout::ScrollPositions> {
    let script = script.filter(|state| **state != rustic_engine::ScrollState::default());
    if limits.scroll == (0.0, 0.0)
        && limits.scroll_boxes.iter().all(Option::is_none)
        && script.is_none()
    {
        return None;
    }
    let mut positions = rustic_layout::ScrollPositions::new();
    if let Some(state) = script {
        positions.set_viewport(state.viewport.0, state.viewport.1);
        for (index, fragment) in layout.fragments().iter().enumerate() {
            if let Some(&(x, y)) = state.elements.get(&fragment.node()) {
                positions.set_box(index, x, y);
            }
        }
    }
    if limits.scroll != (0.0, 0.0) {
        positions.set_viewport(limits.scroll.0, limits.scroll.1);
    }
    for &(index, x, y) in limits.scroll_boxes.iter().flatten() {
        positions.set_box(index, x, y);
    }
    Some(positions)
}

/// The layout with the command's and the scripts' scroll positions
/// applied; the unscrolled layout when there are none.
fn scrolled(
    layout: rustic_layout::BlockLayout,
    viewport: rustic_layout::CssSize,
    limits: LoaderLimits,
    script: Option<&rustic_engine::ScrollState>,
) -> Result<rustic_layout::BlockLayout, LoadError> {
    match scroll_positions(&layout, limits, script) {
        Some(positions) => layout
            .scrolled(viewport, &positions)
            .map_err(LoadError::Flow),
        None => Ok(layout),
    }
}

/// A page split into scene layers (ADR 0064): page segments and scroll
/// containers' contents at unscrolled page coordinates over the whole
/// scrollable area, in paint order, and fixed and viewport-sticky boxes
/// as an overlay at window coordinates.
struct SceneLists {
    layers: Vec<(rustic_paint::DisplayList, rustic_raster::LayerKind)>,
    overlay: Option<rustic_paint::DisplayList>,
    offset: (u32, u32),
    size: (u32, u32),
}

impl SceneLists {
    fn frame(&self) -> rustic_raster::SceneFrame<'_> {
        rustic_raster::SceneFrame {
            layers: self
                .layers
                .iter()
                .map(|(list, kind)| rustic_raster::SceneLayer { list, kind: *kind })
                .collect(),
            overlay: self.overlay.as_ref(),
            offset: self.offset,
            size: self.size,
        }
    }
}

/// Which layer a box paints into.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LayerClass {
    Page,
    /// The contents of the scroll container with this fragment index.
    Container(usize),
    Overlay,
}

/// Each command's layer class (groups, clips and transforms take their
/// root box's; their ends the matching begin's); `None` if unbalanced.
fn command_classes(
    list: &rustic_paint::DisplayList,
    class_of: &dyn Fn(usize) -> LayerClass,
) -> Option<Vec<(LayerClass, usize)>> {
    use rustic_paint::DisplayCommand as C;
    let mut stack: Vec<LayerClass> = Vec::new();
    let mut out = Vec::with_capacity(list.commands().len());
    for command in list.commands() {
        let class = match command {
            C::Fill(item) => class_of(item.source_box),
            C::BeginGroup { source_box, .. }
            | C::BeginClip { source_box, .. }
            | C::BeginTransform { source_box, .. } => class_of(*source_box),
            C::EndGroup | C::EndClip | C::EndTransform => *stack.last()?,
        };
        // The depth outside this command.
        let depth = match command {
            C::EndGroup | C::EndClip | C::EndTransform => {
                stack.pop();
                stack.len()
            }
            _ => stack.len(),
        };
        if matches!(
            command,
            C::BeginGroup { .. } | C::BeginClip { .. } | C::BeginTransform { .. }
        ) {
            stack.push(class);
        }
        out.push((class, depth));
    }
    stack.is_empty().then_some(out)
}

/// Whether `class`'s commands form one run that starts and ends outside
/// every group, without rounded clips.
fn layerable(
    list: &rustic_paint::DisplayList,
    classes: &[(LayerClass, usize)],
    class: LayerClass,
) -> bool {
    let positions: Vec<usize> = classes
        .iter()
        .enumerate()
        .filter(|(_, (c, _))| *c == class)
        .map(|(index, _)| index)
        .collect();
    let (Some(&first), Some(&last)) = (positions.first(), positions.last()) else {
        return true;
    };
    let contiguous = last - first + 1 == positions.len();
    let outside = classes[first].1 == 0 && classes[last].1 == 0;
    let square = list.commands()[first..=last]
        .iter()
        .all(|command| match command {
            rustic_paint::DisplayCommand::Fill(item) => item.rounded_clip.is_none(),
            _ => true,
        });
    contiguous && outside && square
}

/// Build the scene layers for a page, or `None` when it cannot be layered
/// (overlay boxes not painted last, or an RTL viewport range).
fn scene_lists(
    input: LayoutInput<'_>,
    tree: &rustic_layout::BoxTree,
    layout: &rustic_layout::BlockLayout,
    viewport: rustic_layout::CssSize,
    images: &HashMap<rustic_dom::NodeId, std::sync::Arc<rustic_paint::Image>>,
    fonts: &rustic_text::FontDatabase,
) -> Result<Option<SceneLists>, LoadError> {
    use rustic_layout::PositionScheme;
    use rustic_raster::LayerKind;
    let factor = rustic_layout::CssPx::new(1.0)
        .map(|one| one.to_device(input.view.scale()).get())
        .map_err(|_| LoadError::InvalidLimits)?;
    let range = layout.viewport_scroll_range(viewport);
    if range.min != (0.0, 0.0) {
        return Ok(None);
    }
    let snap = |v: f64| (v * factor).round() / factor;
    let device = |v: f64| (v * factor).round().max(0.0) as u32;
    let mut positions = scroll_positions(layout, input.limits, input.scroll).unwrap_or_default();
    let (vx, vy) = range.clamp(positions.viewport());
    let (vx, vy) = (snap(vx), snap(vy));
    positions.set_viewport(vx, vy);
    let fragments = layout.fragments();
    let mut fragment_of_box = vec![None; tree.boxes().len()];
    for (index, fragment) in fragments.iter().enumerate() {
        if let Some(slot) = fragment_of_box.get_mut(fragment.source_box()) {
            slot.get_or_insert(index);
        }
    }
    // Fragment classes along the containing-block chain: fixed and
    // viewport-sticky boxes are the overlay, the contents of a top-level
    // scroll container (no scroll container above it) its layer.
    let mut fragment_class: Vec<LayerClass> = Vec::with_capacity(fragments.len());
    let mut scrolled_above: Vec<bool> = Vec::with_capacity(fragments.len());
    for (index, fragment) in fragments.iter().enumerate() {
        let parent = fragment.containing_block().filter(|&c| c < index);
        let own_overlay = match fragment.scheme() {
            PositionScheme::Fixed => true,
            PositionScheme::Sticky => fragment
                .sticky()
                .is_some_and(|constraint| constraint.scroll_container().is_none()),
            _ => false,
        };
        let inherited = match parent {
            Some(c) if fragment_class[c] == LayerClass::Overlay => LayerClass::Overlay,
            Some(c)
                if fragment_class[c] == LayerClass::Page
                    && layout.scroll_range(c).is_some()
                    && !scrolled_above[c] =>
            {
                LayerClass::Container(c)
            }
            Some(c) => fragment_class[c],
            None => LayerClass::Page,
        };
        let class = if own_overlay {
            LayerClass::Overlay
        } else {
            inherited
        };
        fragment_class.push(class);
        scrolled_above
            .push(parent.is_some_and(|c| scrolled_above[c] || layout.scroll_range(c).is_some()));
    }
    // Other boxes (inline, text) follow their block: inside a container's
    // own block they are its contents.
    let mut box_class = vec![LayerClass::Page; tree.boxes().len()];
    for index in 0..box_class.len() {
        box_class[index] = match fragment_of_box[index] {
            Some(f) => fragment_class[f],
            None => match tree.boxes()[index].parent().filter(|&p| p < index) {
                Some(p) => match fragment_of_box[p] {
                    Some(f)
                        if fragment_class[f] == LayerClass::Page
                            && layout.scroll_range(f).is_some()
                            && !scrolled_above[f] =>
                    {
                        LayerClass::Container(f)
                    }
                    _ => box_class[p],
                },
                None => LayerClass::Page,
            },
        };
    }
    let resources = rustic_paint::Resources {
        fonts: Some(fonts),
        images: images.clone(),
    };
    let build = |layout: &rustic_layout::BlockLayout, area: rustic_layout::CssSize| {
        rustic_paint::DisplayList::build_with(
            input.document,
            tree,
            layout,
            area,
            &resources,
            rustic_paint::PaintLimits::default(),
            input.cancellation,
        )
        .map_err(LoadError::Paint)
    };
    let containers: Vec<usize> = (0..fragments.len())
        .filter(|&f| {
            fragment_class[f] == LayerClass::Page
                && layout.scroll_range(f).is_some_and(|r| r.min == (0.0, 0.0))
                && !scrolled_above[f]
        })
        .collect();
    // Every layer paints over one area: the page's scrollable extent and
    // each container's whole contents (unscrolled, top-level containers
    // move only with the viewport, which is at zero here).
    let (mut right, mut bottom) = (
        viewport.width().get() + range.max.0,
        viewport.height().get() + range.max.1,
    );
    for &c in &containers {
        if let Some(extent) = fragments[c].scrollable_overflow() {
            right = right.max(extent.origin().x.get() + extent.size().width().get());
            bottom = bottom.max(extent.origin().y.get() + extent.size().height().get());
        }
    }
    let area = rustic_layout::CssSize::new(right.ceil(), bottom.ceil())
        .map_err(|_| LoadError::InvalidLimits)?;
    // Which containers can be layers: find out with them unscrolled.
    let mut unscrolled = positions.clone();
    unscrolled.set_viewport(0.0, 0.0);
    for &c in &containers {
        unscrolled.set_box(c, 0.0, 0.0);
    }
    let probe = build(
        &layout
            .scrolled(viewport, &unscrolled)
            .map_err(LoadError::Flow)?,
        area,
    )?;
    let probe_class = |source: usize| box_class.get(source).copied().unwrap_or(LayerClass::Page);
    let Some(classes) = command_classes(&probe, &probe_class) else {
        return Ok(None);
    };
    // The overlay must come last, outside every group.
    if let Some(first) = classes.iter().position(|(c, _)| *c == LayerClass::Overlay) {
        if classes[first].1 != 0
            || classes[first..]
                .iter()
                .any(|(c, _)| *c != LayerClass::Overlay)
        {
            return Ok(None);
        }
    }
    let layered: Vec<usize> = containers
        .iter()
        .copied()
        .filter(|&c| layerable(&probe, &classes, LayerClass::Container(c)))
        .collect();
    // Containers that are not layered paint into the page, scrolled.
    let class_of = |source: usize| match box_class.get(source).copied() {
        Some(LayerClass::Container(c)) if !layered.contains(&c) => LayerClass::Page,
        Some(class) => class,
        None => LayerClass::Page,
    };
    let mut page_positions = positions.clone();
    page_positions.set_viewport(0.0, 0.0);
    for &c in &layered {
        page_positions.set_box(c, 0.0, 0.0);
    }
    let page_layout = layout
        .scrolled(viewport, &page_positions)
        .map_err(LoadError::Flow)?;
    let page = build(&page_layout, area)?;
    let Some(classes) = command_classes(&page, &class_of) else {
        return Ok(None);
    };
    let mut layers = Vec::new();
    let mut start = 0;
    while start < classes.len() {
        let class = classes[start].0;
        let mut end = start;
        while end < classes.len() && classes[end].0 == class {
            end += 1;
        }
        match class {
            LayerClass::Overlay => break,
            LayerClass::Page => {
                let list = page.retain(|index, _| (start..end).contains(&index));
                let list = if layers.is_empty() {
                    list
                } else {
                    list.over_transparent()
                };
                layers.push((list, LayerKind::Page));
            }
            LayerClass::Container(c) => {
                if layers.is_empty() {
                    layers.push((page.retain(|_, _| false), LayerKind::Page));
                }
                let own = build(&page_layout.without_clips_above(c), area)?;
                let Some(own_classes) = command_classes(&own, &class_of) else {
                    return Ok(None);
                };
                let list = own
                    .retain(|index, _| own_classes[index].0 == LayerClass::Container(c))
                    .over_transparent();
                let fragment = page_layout.fragments()[c];
                let padding = fragment.padding();
                let (mut x0, mut y0) = (padding.origin().x.get(), padding.origin().y.get());
                let (mut x1, mut y1) = (
                    x0 + padding.size().width().get(),
                    y0 + padding.size().height().get(),
                );
                let clip = fragment.clip();
                if let Some(r) = clip.x {
                    x0 = x0.max(r.start.get());
                    x1 = x1.min(r.end.get());
                }
                if let Some(r) = clip.y {
                    y0 = y0.max(r.start.get());
                    y1 = y1.min(r.end.get());
                }
                let (px, py) = (device(x0), device(y0));
                let port = rustic_raster::DeviceRect {
                    x: px,
                    y: py,
                    width: device(x1).saturating_sub(px),
                    height: device(y1).saturating_sub(py),
                };
                let at = layout
                    .scroll_range(c)
                    .map_or((0.0, 0.0), |r| r.clamp(positions.of_box(c)));
                let scroll = (device(snap(at.0)), device(snap(at.1)));
                positions.set_box(c, snap(at.0), snap(at.1));
                if port.width > 0 && port.height > 0 {
                    layers.push((list, LayerKind::Scroller { port, scroll }));
                }
            }
        }
        start = end;
    }
    if layers.is_empty() {
        layers.push((page.retain(|_, _| false), LayerKind::Page));
    }
    let window = rustic_raster::surface_size(&build(layout, viewport)?, input.view.scale())
        .map_err(LoadError::Raster)?;
    let overlay = if classes.iter().any(|(c, _)| *c == LayerClass::Overlay) {
        let scrolled = layout
            .scrolled(viewport, &positions)
            .map_err(LoadError::Flow)?;
        let list = build(&scrolled, viewport)?;
        let Some(overlay_classes) = command_classes(&list, &class_of) else {
            return Ok(None);
        };
        Some(
            list.retain(|index, _| overlay_classes[index].0 == LayerClass::Overlay)
                .over_transparent(),
        )
    } else {
        None
    };
    Ok(Some(SceneLists {
        layers,
        overlay,
        offset: (device(vx), device(vy)),
        size: window,
    }))
}

/// Render the page through a scene when it is scrolled or has an overlay
/// (ADR 0064: the scene is the reference for scrolled pages); `None`
/// otherwise, or when it cannot be layered.
fn render_scene(
    input: LayoutInput<'_>,
    scene: &mut rustic_raster::Scene,
) -> Result<Option<(rustic_raster::Surface, rustic_raster::SceneStats)>, LoadError> {
    let (tree, layout, viewport, images, _, fonts) = block_layout(input)?;
    let scrolled_viewport = scroll_positions(&layout, input.limits, input.scroll).is_some();
    let fixed = layout.fragments().iter().any(|f| {
        f.scheme() == rustic_layout::PositionScheme::Fixed
            || f.sticky().is_some_and(|c| c.scroll_container().is_none())
    });
    if !scrolled_viewport && !fixed {
        return Ok(None);
    }
    let Some(lists) = scene_lists(input, &tree, &layout, viewport, &images.images, &fonts)? else {
        return Ok(None);
    };
    scene
        .submit(lists.frame(), input.cancellation)
        .map(Some)
        .map_err(LoadError::Raster)
}

fn display_list(input: LayoutInput<'_>) -> Result<rustic_paint::DisplayList, LoadError> {
    let (tree, layout, viewport, images, _, fonts) = block_layout(input)?;
    let layout = scrolled(layout, viewport, input.limits, input.scroll)?;
    let resources = rustic_paint::Resources {
        fonts: Some(&fonts),
        images: images.images,
    };
    rustic_paint::DisplayList::build_with(
        input.document,
        &tree,
        &layout,
        viewport,
        &resources,
        rustic_paint::PaintLimits::default(),
        input.cancellation,
    )
    .map_err(LoadError::Paint)
}

/// Dump the display list for the block-only layout reference.
pub fn dump_paint(
    parsed: &LoadedDocument,
    path: &Path,
    limits: LoaderLimits,
) -> Result<String, LoadError> {
    display_list(parsed.layout_input(path, limits, default_view()))?
        .dump(limits.max_dump_bytes)
        .map_err(LoadError::Paint)
}

/// Hit-test each point (CSS pixels from the viewport origin) against the
/// display list and report one line per point: the box, node and element
/// name found, or `none` outside the viewport.
pub fn hit_test(
    parsed: &LoadedDocument,
    path: &Path,
    limits: LoaderLimits,
    points: &[(f64, f64)],
) -> Result<String, LoadError> {
    let list = display_list(parsed.layout_input(path, limits, default_view()))?;
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

/// Rasterize the display list at `scale` (no frames; the tests' direct
/// path).
#[cfg(test)]
pub fn render_surface(
    parsed: &LoadedDocument,
    path: &Path,
    limits: LoaderLimits,
    view: rustic_layout::View,
) -> Result<rustic_raster::Surface, LoadError> {
    render_list(parsed, path, limits, view).map(|(_, surface)| surface)
}

/// Rasterize the display list at the view's scale, keeping the list.
pub fn render_list(
    parsed: &LoadedDocument,
    path: &Path,
    limits: LoaderLimits,
    view: rustic_layout::View,
) -> Result<(rustic_paint::DisplayList, rustic_raster::Surface), LoadError> {
    rasterize_list(parsed.layout_input(path, limits, view))
}

fn rasterize_list(
    input: LayoutInput<'_>,
) -> Result<(rustic_paint::DisplayList, rustic_raster::Surface), LoadError> {
    let list = display_list(input)?;
    let surface = rustic_raster::rasterize(
        &list,
        input.view.scale(),
        rustic_raster::RasterLimits::default(),
        input.cancellation,
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

/// An `iframe` of the document: its element and the frame it asks for.
struct FrameSite {
    node: rustic_dom::NodeId,
    url: String,
    srcdoc: Option<String>,
    sandbox: Option<Vec<String>>,
    /// `loading=lazy`: opened only near the viewport (HTML §2.5.7).
    lazy: bool,
}

/// The document's `iframe` elements in tree order, with `src` resolved
/// against the document base URL (an empty or invalid `src` is
/// `about:blank`; `srcdoc` wins over `src`).
fn frame_sites(document: &Document, document_url: Option<&rustic_url::Url>) -> Vec<FrameSite> {
    use crate::reftest::{attribute, is_html};
    let mut sites = Vec::new();
    let Ok(walk) = document.walk(document.root()) else {
        return sites;
    };
    let mut base: Option<Option<rustic_url::Url>> = None;
    let mut found = Vec::new();
    for (id, _) in walk {
        let Some(element) = rustic_style::DomElement::new(document, id) else {
            continue;
        };
        if is_html(element, "base") && base.is_none() {
            if let Some(href) = attribute(element, "href") {
                base = Some(rustic_url::Url::parse(&href, document_url).ok());
            }
        } else if is_html(element, "iframe") {
            found.push((
                id,
                attribute(element, "src"),
                attribute(element, "srcdoc"),
                attribute(element, "sandbox"),
                crate::images::is_lazy(element),
            ));
        }
    }
    let base = base.flatten().or_else(|| document_url.cloned());
    for (node, src, srcdoc, sandbox, lazy) in found {
        let url = if srcdoc.is_some() {
            "about:srcdoc".to_owned()
        } else {
            src.filter(|src| !src.trim().is_empty())
                .and_then(|src| rustic_url::Url::parse(src.trim(), base.as_ref()).ok())
                .map_or_else(|| "about:blank".to_owned(), |url| url.serialize())
        };
        let sandbox = sandbox.map(|value| {
            value
                .split(['\t', '\n', '\x0c', '\r', ' '])
                .filter(|token| !token.is_empty())
                .map(str::to_ascii_lowercase)
                .collect()
        });
        sites.push(FrameSite {
            node,
            url,
            srcdoc,
            sandbox,
            lazy,
        });
    }
    sites
}

/// Asks for a frame to be loaded and rendered elsewhere (ADR 0062).
pub(crate) trait FrameHost {
    /// A handle for the frame's layer, or `None` when it is not shown.
    fn open(&self, request: rustic_ipc::protocol::FrameRequest) -> Option<u32>;
}

/// One color, as the content of every pixel of a frame's content box.
fn solid(color: u8) -> std::sync::Arc<rustic_paint::Image> {
    std::sync::Arc::new(
        rustic_paint::Image::new(1, 1, vec![color, color, color, 255]).expect("one pixel"),
    )
}

/// Rasterize the document with its `iframe`s as layers (ADR 0062). Each
/// frame the host accepts is painted opaque black in the returned
/// surface, and its layer's coverage is how much painting it opaque white
/// instead adds to each pixel, so whoever holds the frame's pixels can
/// composite them exactly under everything painted above it, through
/// clips and opacity. The frame's pixels never reach this process. Frames
/// the host refuses, frames under a transform (TD-177) and frames past
/// [`rustic_ipc::protocol::MAX_FRAMES`] paint white.
pub(crate) fn render_layers(
    parsed: &LoadedDocument,
    path: &Path,
    limits: LoaderLimits,
    view: rustic_layout::View,
    host: &dyn FrameHost,
) -> Result<rustic_ipc::protocol::Surface, LoadError> {
    let mut marks = None;
    let mut surface = render_unmarked_layers(parsed, path, limits, view, host, &mut marks)?;
    if let Some(marks) = marks {
        crate::find::highlight(
            &mut surface.pixels,
            (surface.width, surface.height),
            &marks.matches,
            marks.active,
            marks.scroll,
            marks.factor,
        );
    }
    Ok(surface)
}

/// Find-in-page matches to mark on a rendered surface (UI-01).
struct FindMarks {
    matches: Vec<crate::find::Rect>,
    active: Option<usize>,
    scroll: (f64, f64),
    factor: f64,
}

fn render_unmarked_layers(
    parsed: &LoadedDocument,
    path: &Path,
    limits: LoaderLimits,
    view: rustic_layout::View,
    host: &dyn FrameHost,
    marks: &mut Option<FindMarks>,
) -> Result<rustic_ipc::protocol::Surface, LoadError> {
    use rustic_ipc::protocol::{FrameLayer, FrameRequest, MAX_FRAMES, Surface};
    use rustic_paint::{DisplayCommand, ItemRole, Shape};
    let input = parsed.layout_input(path, limits, view);
    let cancellation = input.cancellation;
    let (tree, layout, viewport, images, _, fonts) = block_layout(input)?;
    // The position the scene paints: clamped, snapped to device pixels.
    let range = layout.viewport_scroll_range(viewport);
    let at = scroll_positions(&layout, limits, input.scroll)
        .map_or((0.0, 0.0), |positions| range.clamp(positions.viewport()));
    let factor = rustic_layout::CssPx::new(1.0)
        .map(|one| one.to_device(view.scale()).get())
        .unwrap_or(1.0);
    let snap = |v: f64| (v * factor).round() / factor;
    if limits.report_scroll {
        let line = format!("scroll {},{}\n", snap(at.0), snap(at.1));
        crate::renderer::emit_stdout(line.as_bytes()).map_err(LoadError::Io)?;
    }
    if limits.report_a11y {
        let tree = accessibility_tree(parsed, path, limits, view)?;
        let bytes = rustic_a11y::wire::encode(&tree);
        let mut line = String::with_capacity(bytes.len() * 2 + 6);
        line.push_str("a11y ");
        for byte in bytes {
            line.push_str(&format!("{byte:02x}"));
        }
        line.push('\n');
        crate::renderer::emit_stdout(line.as_bytes()).map_err(LoadError::Io)?;
    }
    if let Some(query) = crate::find::query() {
        let matches = crate::find::matches(&tree, &layout, &query);
        let (active, line) = crate::find::report(&matches, query.index);
        crate::renderer::emit_stdout(line.as_bytes()).map_err(LoadError::Io)?;
        *marks = Some(FindMarks {
            matches,
            active,
            scroll: (snap(at.0), snap(at.1)),
            factor,
        });
    }
    let layout = scrolled(layout, viewport, limits, input.scroll)?;
    let document_url = input.network.map(|session| session.document_url().clone());
    let sites = frame_sites(input.document, document_url.as_ref());
    if sites.is_empty() {
        let mut scene =
            rustic_raster::Scene::new(view.scale(), rustic_raster::RasterLimits::default());
        if let Some((surface, _)) = render_scene(input, &mut scene)? {
            return Ok(Surface {
                width: surface.width(),
                height: surface.height(),
                pixels: surface.premultiplied().to_vec(),
                layers: Vec::new(),
            });
        }
    }
    let build = |images: HashMap<rustic_dom::NodeId, std::sync::Arc<rustic_paint::Image>>| {
        let resources = rustic_paint::Resources {
            fonts: Some(&fonts),
            images,
        };
        rustic_paint::DisplayList::build_with(
            input.document,
            &tree,
            &layout,
            viewport,
            &resources,
            rustic_paint::PaintLimits::default(),
            cancellation,
        )
        .map_err(LoadError::Paint)
    };
    let raster = |list: &rustic_paint::DisplayList| {
        rustic_raster::rasterize(
            list,
            view.scale(),
            rustic_raster::RasterLimits::default(),
            cancellation,
        )
        .map_err(LoadError::Raster)
    };
    let with = |colors: &[(rustic_dom::NodeId, u8)]| {
        let mut all = images.images.clone();
        for &(node, color) in colors {
            all.insert(node, solid(color));
        }
        all
    };
    if sites.is_empty() {
        let base = raster(&build(with(&[]))?)?;
        return Ok(Surface {
            width: base.width(),
            height: base.height(),
            pixels: base.premultiplied().to_vec(),
            layers: Vec::new(),
        });
    }
    // Where each frame's content box is painted, outside any transform.
    let probe = build(with(
        &sites.iter().map(|site| (site.node, 0)).collect::<Vec<_>>(),
    ))?;
    let mut depth = 0usize;
    let mut placed: HashMap<rustic_dom::NodeId, rustic_layout::CssRect> = HashMap::new();
    let mut transformed = std::collections::HashSet::new();
    for command in probe.commands() {
        match command {
            DisplayCommand::BeginTransform { .. } => depth += 1,
            DisplayCommand::EndTransform => depth = depth.saturating_sub(1),
            DisplayCommand::Fill(item) if item.role == ItemRole::Image => {
                if let Shape::Image { rect, .. } = item.shape {
                    if sites.iter().any(|site| site.node == item.node) {
                        if depth > 0 {
                            transformed.insert(item.node);
                        }
                        placed.entry(item.node).or_insert(rect);
                    }
                }
            }
            _ => {}
        }
    }
    let factor = rustic_layout::CssPx::new(1.0)
        .map(|one| one.to_device(view.scale()).get())
        .unwrap_or(1.0);
    let mut opened: Vec<(rustic_dom::NodeId, u32, rustic_layout::CssRect)> = Vec::new();
    for site in &sites {
        let Some(&rect) = placed.get(&site.node) else {
            continue;
        };
        if transformed.contains(&site.node) {
            eprintln!(
                "rustic: an iframe under a CSS transform is not composited yet (TD-177); it \
                 paints white"
            );
            continue;
        }
        if opened.len() >= MAX_FRAMES {
            eprintln!("rustic: more than {MAX_FRAMES} iframes; the rest paint white");
            break;
        }
        let size = |css: f64| (css.round().clamp(0.0, 16_384.0)) as u32;
        let (width, height) = (
            size(rect.size().width().get()),
            size(rect.size().height().get()),
        );
        if width == 0 || height == 0 {
            continue;
        }
        let viewport_size = (viewport.width().get(), viewport.height().get());
        if site.lazy
            && !near_rect(
                rect.origin().x.get(),
                rect.origin().y.get(),
                rect.size().width().get(),
                rect.size().height().get(),
                viewport_size,
                crate::images::LAZY_LOAD_MARGIN,
            )
        {
            // A lazy frame far from the viewport is not loaded yet; it
            // paints as an empty (white) frame.
            continue;
        }
        let request = FrameRequest {
            url: site.url.clone(),
            srcdoc: site.srcdoc.clone(),
            sandbox: site.sandbox.clone(),
            width,
            height,
        };
        if let Some(handle) = host.open(request) {
            opened.push((site.node, handle, rect));
        }
    }
    let colors = |white: Option<rustic_dom::NodeId>| {
        sites
            .iter()
            .map(|site| {
                let layered = opened.iter().any(|&(node, _, _)| node == site.node);
                let color = if !layered || white == Some(site.node) {
                    255
                } else {
                    0
                };
                (site.node, color)
            })
            .collect::<Vec<_>>()
    };
    let base_list = build(with(&colors(None)))?;
    let base = raster(&base_list)?;
    let mut layers = Vec::new();
    for &(node, handle, rect) in &opened {
        let device = |css: f64| css * factor;
        let left = device(rect.origin().x.get()).floor().max(0.0);
        let top = device(rect.origin().y.get()).floor().max(0.0);
        let right = device(rect.origin().x.get() + rect.size().width().get())
            .ceil()
            .min(f64::from(base.width()));
        let bottom = device(rect.origin().y.get() + rect.size().height().get())
            .ceil()
            .min(f64::from(base.height()));
        if right <= left || bottom <= top {
            continue;
        }
        let region = rustic_raster::DeviceRect {
            x: left as u32,
            y: top as u32,
            width: (right - left) as u32,
            height: (bottom - top) as u32,
        };
        let white = rustic_raster::rasterize_region(
            &build(with(&colors(Some(node))))?,
            view.scale(),
            region,
            rustic_raster::RasterLimits::default(),
            cancellation,
        )
        .map_err(LoadError::Raster)?;
        let mut coverage = Vec::with_capacity((region.width * region.height) as usize);
        for row in 0..region.height {
            for column in 0..region.width {
                let at = |surface: &rustic_raster::Surface, x: u32, y: u32| {
                    let index = ((y * surface.width() + x) * 4 + 1) as usize;
                    surface.premultiplied()[index]
                };
                let lit = at(&white, column, row);
                let dark = at(&base, region.x + column, region.y + row);
                coverage.push(lit.saturating_sub(dark));
            }
        }
        layers.push(FrameLayer {
            handle,
            x: region.x,
            y: region.y,
            width: region.width,
            height: region.height,
            coverage,
        });
    }
    Ok(Surface {
        width: base.width(),
        height: base.height(),
        pixels: base.premultiplied().to_vec(),
        layers,
    })
}

/// Refuses every frame.
struct NoFrames;

impl FrameHost for NoFrames {
    fn open(&self, _: rustic_ipc::protocol::FrameRequest) -> Option<u32> {
        None
    }
}

/// Render to PNG in this process, where frames have no renderer of their
/// own: they paint white (TD-178).
pub fn render_png_without_frames(
    parsed: &LoadedDocument,
    path: &Path,
    limits: LoaderLimits,
    view: rustic_layout::View,
) -> Result<Vec<u8>, LoadError> {
    let surface = render_layers(parsed, path, limits, view, &NoFrames)?;
    let surface = rustic_raster::Surface::from_premultiplied(
        surface.width,
        surface.height,
        surface.pixels,
        rustic_raster::RasterLimits::default(),
    )
    .ok_or(LoadError::Raster(rustic_raster::RasterError::SurfaceSize))?;
    rustic_raster::encode_png(&surface, limits.max_dump_bytes).map_err(LoadError::Png)
}

/// Rasterize like `render_surface` and encode the result as PNG, within
/// the output byte limit.
#[cfg(test)]
pub fn render_png(
    parsed: &LoadedDocument,
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

    #[test]
    fn lazy_loads_start_within_the_margin_of_the_viewport() {
        let view = (800.0, 600.0);
        assert!(near_rect(0.0, 1850.0, 10.0, 10.0, view, 1250.0));
        assert!(!near_rect(0.0, 1851.0, 10.0, 10.0, view, 1250.0));
        assert!(near_rect(-1260.0, 0.0, 10.0, 10.0, view, 1250.0));
        assert!(!near_rect(-1261.0, 0.0, 10.0, 10.0, view, 1250.0));
    }

    fn test_url(path: &Path) -> rustic_url::Url {
        file_url(&fs::canonicalize(path).unwrap()).unwrap()
    }

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

    #[test]
    fn parser_scripts_get_live_cssom_values_from_the_renderers_cascade() {
        let file = TempFile::new(
            br#"<!doctype html>
                <style>
                    body { color: navy; --Theme: winter }
                    #box { width: 21px }
                    #box.wide { width: 42px; --Theme: summer }
                </style>
                <div id="box"></div>
                <script>
                    const box = document.querySelector('#box');
                    const held = getComputedStyle(box);
                    const detached = document.createElement('span');
                    const detachedStyle = getComputedStyle(detached);
                    const before = held.getPropertyValue('width');
                    const namedBefore = held.width;
                    const backgroundAlias = held.backgroundColor ===
                        held.getPropertyValue('background-color');
                    const floatAlias = held.cssFloat === held.getPropertyValue('float');
                    const fontAlias = held.fontSize === held.getPropertyValue('font-size');
                    const inherited = held.getPropertyValue('color');
                    const originalTheme = held.getPropertyValue('--Theme');
                    const missing = held.getPropertyValue('--theme');
                    const indexCountBefore = held.length;
                    let sorted = true;
                    for (let i = 1; i < indexCountBefore - 1; i++) {
                        if (held[i - 1] >= held[i] || held.item(i) !== held[i]) sorted = false;
                    }
                    const initialIndices = sorted && held[indexCountBefore - 1] === '--Theme' &&
                        Object.keys(held).length === held.length;
                    box.className = 'wide';
                    box.setAttribute('class', 'wide');
                    const after = held.getPropertyValue('width');
                    const namedAfter = held.width;
                    const updatedTheme = held.getPropertyValue('--Theme');
                    document.querySelector('style').firstChild.data =
                        '#box { width: 63px; color: red; --Theme: autumn; --extra: yes }';
                    const indexCountAfter = held.length;
                    const indicesUpdated = indexCountAfter === indexCountBefore + 1 &&
                        held[indexCountAfter - 1] === '--extra' &&
                        held.item(indexCountAfter - 1) === held[indexCountAfter - 1] &&
                        Object.keys(held).length === held.length;
                    const afterSheetEdit = held.getPropertyValue('width');
                    const namedAfterSheetEdit = held.width;
                    const property = { toString() {
                        box.setAttribute('style', 'width: 84px');
                        return 'width';
                    } };
                    const afterReentry = held.getPropertyValue(property);
                    const namedAfterReentry = held.width;
                    const good = [
                        held instanceof CSSStyleDeclaration,
                        held.length > 0,
                        held.item(0) === 'align-content',
                        before === '21px',
                        namedBefore === before,
                        after === '42px',
                        namedAfter === after,
                        afterSheetEdit === '63px',
                        namedAfterSheetEdit === afterSheetEdit,
                        afterReentry === '84px',
                        namedAfterReentry === afterReentry,
                        backgroundAlias, floatAlias, fontAlias,
                        initialIndices, indicesUpdated,
                        inherited !== '',
                        originalTheme.includes('winter'),
                        updatedTheme.includes('summer'),
                        missing === '',
                        held.getPropertyValue('nonexistent') === '',
                        detachedStyle.getPropertyValue('color') === '',
                        getComputedStyle(document.body).getPropertyValue('width') !== '',
                    ].every(Boolean);
                    document.body.setAttribute('data-cssom', good ? 'ok' : 'wrong');
                </script>"#,
        );
        let limits = LoaderLimits {
            run_scripts: true,
            ..LoaderLimits::default()
        };
        let parsed = load_document(&mut Engine::new(1), &file.0, limits).unwrap();
        let output = dump_dom(&parsed, &file.0, limits).unwrap();
        assert!(
            output.contains("local=\"data-cssom\" value=\"ok\""),
            "{output}"
        );
    }

    #[test]
    fn cssom_resolves_used_sizes_and_keeps_boxless_values() {
        let file = TempFile::new(
            br#"<!doctype html>
                <style>
                    #parent { width: 201px; height: 81px; font-size: 14px; line-height: 1.25 }
                    #box {
                        width: 50%; height: 50%; padding: 3px; border: 2px solid;
                        transform: scale(2);
                    }
                    #box.bordered { box-sizing: border-box; max-width: 90px }
                    #box.hidden { display: none }
                    #inline, #boxless, #hidden { width: 50%; height: 50% }
                    #boxless { display: contents }
                    #hidden { display: none }
                    #atomic { display: inline-block; width: 50%; height: 10px }
                    #inline { font-size: 20px }
                    #hidden { line-height: normal }
                </style>
                <div id="parent">
                    <div id="box"></div>
                    <span id="inline">text</span>
                    <span id="atomic"></span>
                    <div id="boxless"></div>
                    <div id="hidden"></div>
                </div>
                <script>
                    const parent = document.querySelector('#parent');
                    const box = document.querySelector('#box');
                    const held = getComputedStyle(box);
                    const inline = getComputedStyle(document.querySelector('#inline'));
                    const atomic = getComputedStyle(document.querySelector('#atomic'));
                    const contents = getComputedStyle(document.querySelector('#boxless'));
                    const hidden = getComputedStyle(document.querySelector('#hidden'));
                    const reads = [held.getPropertyValue('width'), held.height, held.lineHeight,
                                   box.offsetWidth, box.getBoundingClientRect().width,
                                   inline.width, inline.height, inline.lineHeight,
                                   atomic.width, atomic.height, contents.width, contents.height,
                                   hidden.width, hidden.height, hidden.lineHeight];
                    box.setAttribute('class', 'bordered');
                    reads.push(held.width, held.height, box.offsetWidth);
                    parent.setAttribute('style', 'width: 141px; height: 101px');
                    reads.push(held.width, held.height, box.offsetWidth);
                    box.setAttribute('class', 'hidden');
                    reads.push(held.width, held.height, box.offsetWidth);
                    box.setAttribute('style', 'line-height: 1e308');
                    const largeLineHeight = held.lineHeight;
                    reads.push(largeLineHeight.endsWith('px') &&
                               Number.isFinite(parseFloat(largeLineHeight)));
                    document.body.setAttribute('data-resolved', reads.join(','));
                </script>"#,
        );
        let mut engine = Engine::new(1);
        let limits = LoaderLimits {
            run_scripts: true,
            ..LoaderLimits::default()
        };
        let parsed = load_document(&mut engine, &file.0, limits).unwrap();
        let output = dump_dom(&parsed, &file.0, limits).unwrap();
        assert!(
            output.contains(concat!(
                "local=\"data-resolved\" value=\"",
                "100.5px,40.5px,17.5px,111,221,50%,50%,25px,100.5px,10px,50%,50%,50%,50%,normal,",
                "90px,40.5px,90,70.5px,50.5px,71,50%,50%,0,true\"",
            )),
            "{output}"
        );
    }

    #[test]
    fn cssom_resolves_box_edges_and_serializes_four_sided_shorthands() {
        let file = TempFile::new(
            br#"<!doctype html>
                <style>
                    #parent { position: relative; width: 200px; padding: 10px; }
                    .box { width: 80px; height: 10px; padding: 5% 2.5%; margin: auto; }
                    #box { border-width: 1px 2px 3px; border-style: solid dashed; }
                    #box { border-color: red blue red green; }
                    #absolute { position: absolute; left: 0; right: 0; }
                </style>
                <div id="parent">
                    <div id="box" class="box"></div>
                    <span id="absolute" class="box"></span>
                </div>
                <script>
                    const parent = document.getElementById('parent');
                    const box = document.getElementById('box');
                    const held = getComputedStyle(box);
                    const absolute = getComputedStyle(document.getElementById('absolute'));
                    const read = name => held.getPropertyValue(name);
                    const initial = [read('MARGIN'), read('padding'), read('border-width'),
                        read('border-style'), read('border-color'), held.marginRight,
                        absolute.getPropertyValue('margin'), absolute.paddingTop,
                        [...held].includes('margin'), read('unsupported-property')];
                    document.body.setAttribute('data-edges', initial.join('|'));
                    box.setAttribute('style', 'margin: 3px 4% -2px 5%');
                    parent.setAttribute('style', 'width: 150px');
                    document.body.setAttribute('data-updated',
                        [read('margin'), held.marginRight, held.marginLeft, read('padding')].join('|'));
                    box.setAttribute('style', 'display: none; margin: 3px 4% -2px 5%');
                    document.body.setAttribute('data-boxless',
                        [read('margin'), read('padding')].join('|'));
                    const detached = getComputedStyle(document.createElement('span'));
                    document.body.setAttribute('data-detached', detached.getPropertyValue('padding'));
                </script>"#,
        );
        let mut engine = Engine::new(1);
        let limits = LoaderLimits {
            run_scripts: true,
            ..LoaderLimits::default()
        };
        let parsed = load_document(&mut engine, &file.0, limits).unwrap();
        let output = dump_dom(&parsed, &file.0, limits).unwrap();
        for expected in [
            concat!(
                "local=\"data-edges\" value=\"0px 53px|10px 5px|1px 2px 3px|solid dashed|",
                "rgb(255, 0, 0) rgb(0, 0, 255) rgb(255, 0, 0) rgb(0, 128, 0)|",
                "53px|0px 64.5px|11px|false|\"",
            ),
            "local=\"data-updated\" value=\"3px 6px -2px 7.5px|6px|7.5px|7.5px 3.75px\"",
            "local=\"data-boxless\" value=\"3px 4% -2px 5%|5% 2.5%\"",
            "local=\"data-detached\" value=\"\"",
        ] {
            assert!(
                output.contains(expected),
                "{expected} missing from {output}"
            );
        }
    }

    #[test]
    fn final_scripted_output_uses_the_stylesheet_and_image_applied_at_load() {
        let sheet = TempFile::new(b"img { border: 3px solid red }");
        let png = rustic_raster::encode_png(
            &rustic_raster::Surface::new(10, 4, rustic_raster::RasterLimits::default()).unwrap(),
            usize::MAX,
        )
        .unwrap();
        let image = TempFile::new(&png);
        let page = TempFile::new(
            format!(
                "<!doctype html><link rel=stylesheet href='{}'><img src='{}'>",
                sheet.0.file_name().unwrap().to_str().unwrap(),
                image.0.file_name().unwrap().to_str().unwrap(),
            )
            .as_bytes(),
        );
        let mut engine = Engine::new(2);
        let limits = LoaderLimits {
            run_scripts: true,
            ..LoaderLimits::default()
        };
        let scripted = load_document(&mut engine, &page.0, limits).unwrap();
        let plain = LoaderLimits::default();
        let reference = load_document(&mut engine, &page.0, plain).unwrap();
        let pixels = |doc, limits| render_png(doc, &page.0, limits, default_view()).unwrap();
        let expected = pixels(&reference, plain);
        fs::remove_file(&image.0).unwrap();
        assert_ne!(pixels(&reference, plain), expected);
        assert_eq!(pixels(&scripted, limits), expected);
        fs::write(&sheet.0, b"img { display: none }").unwrap();
        assert_eq!(pixels(&scripted, limits), expected);
    }

    #[test]
    fn parser_resource_discovery_respects_link_filters_base_and_kind_limits() {
        let file = TempFile::new(
            br#"<!doctype html>
              <base href=".">
              <link rel="alternate stylesheet" href="skip.css">
              <link rel="stylesheet" disabled href="disabled.css">
              <link rel="stylesheet" type="text/plain" href="wrong.css">
              <link rel="StyleSheet" href="site.css">
              <link rel="stylesheet" href="site.css">
              <link rel="stylesheet" href="over-limit.css">
              <img src="first.png">
              <img src="first.png">
              <img src="over-limit.png">"#,
        );
        let mut engine = Engine::new(1);
        let limits = LoaderLimits {
            stylesheets: StyleLoadLimits {
                max_sheets: 1,
                ..StyleLoadLimits::default()
            },
            images: crate::images::ImageLimits {
                max_images: 1,
                ..crate::images::ImageLimits::default()
            },
            ..LoaderLimits::default()
        };
        let parsed = load_document(&mut engine, &file.0, limits).unwrap();
        let urls = discover_local_resources(parsed.parsed().document(), &test_url(&file.0), limits);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0].1, LocalResourceKind::Stylesheet);
        assert_eq!(urls[1].1, LocalResourceKind::Image);
        assert!(urls[0].0.serialize().ends_with("/site.css"));
        assert!(urls[1].0.serialize().ends_with("/first.png"));

        // With more capacity, repeated references must not consume slots:
        // the next distinct stylesheet and image are still discovered.
        let wider = LoaderLimits {
            stylesheets: StyleLoadLimits {
                max_sheets: 2,
                ..StyleLoadLimits::default()
            },
            images: crate::images::ImageLimits {
                max_images: 2,
                ..crate::images::ImageLimits::default()
            },
            ..LoaderLimits::default()
        };
        let unique =
            discover_local_resources(parsed.parsed().document(), &test_url(&file.0), wider);
        assert_eq!(unique.len(), 4);
        assert_eq!(unique[0].1, LocalResourceKind::Stylesheet);
        assert_eq!(unique[1].1, LocalResourceKind::Stylesheet);
        assert_eq!(unique[2].1, LocalResourceKind::Image);
        assert_eq!(unique[3].1, LocalResourceKind::Image);
        assert!(unique[1].0.serialize().ends_with("/over-limit.css"));
        assert!(unique[3].0.serialize().ends_with("/over-limit.png"));
    }

    #[test]
    fn completed_parser_auto_delivers_css_and_png_with_bounded_host_turns() {
        let stylesheet = TempFile::new(b"#box { width: 19px }");
        let image_bytes = rustic_raster::encode_png(
            &rustic_raster::Surface::new(4, 3, rustic_raster::RasterLimits::default()).unwrap(),
            usize::MAX,
        )
        .unwrap();
        let image = TempFile::new(&image_bytes);
        let html = format!(
            "<!doctype html><link rel=stylesheet href=\"{}\">\
             <div id=box></div><img id=photo src=\"{}\">",
            stylesheet.0.file_name().unwrap().to_str().unwrap(),
            image.0.file_name().unwrap().to_str().unwrap(),
        );
        let page = TempFile::new(html.as_bytes());
        let mut engine = Engine::new(1);
        let limits = LoaderLimits::default();
        let parsed = load_document(&mut engine, &page.0, limits).unwrap();
        let token = parsed.cancellation().clone();
        let encoding = parsed.parsed().encoding();
        let applied = AppliedLocalFiles::new(
            limits.stylesheets.max_sheets + limits.images.max_images,
            limits.stylesheets.max_total_bytes + limits.images.max_file_bytes,
        );
        let fetch = LocalStylesheetFetcher::new(
            fs::canonicalize(page.0.parent().unwrap()).unwrap(),
            limits.read_chunk_bytes,
            token.clone(),
        )
        .unwrap();
        let mut scripted = parsed
            .document
            .into_scripted_with_clock(TaskLimits::default(), ManualClock::default())
            .unwrap();
        // A spent JS budget still admits resource-only turns.
        let mut timer_turns = MAX_SCRIPT_TIMER_TURNS;
        let delivered = queue_discovered_local_resources(
            &mut scripted,
            &test_url(&page.0),
            limits,
            &fetch,
            &applied,
            &token,
            &mut timer_turns,
        )
        .unwrap();
        assert_eq!(timer_turns, MAX_SCRIPT_TIMER_TURNS);
        assert_eq!(delivered, 2);
        assert_eq!(scripted.run_next(), Ok(None));

        // Earlier immediate work gives a ready timer priority. An exhausted
        // host must stop before entering JS or consuming that timer.
        let timer = scripted
            .schedule_script(
                Duration::ZERO,
                rustic_js::Source::from_str_limited("0", 1).unwrap(),
            )
            .unwrap();
        assert!(matches!(
            queue_discovered_local_resources(
                &mut scripted,
                &test_url(&page.0),
                limits,
                &fetch,
                &applied,
                &token,
                &mut timer_turns,
            ),
            Err(LoadError::ScriptTurnLimit)
        ));
        assert_eq!(scripted.cancel_timer(timer), Ok(true));
        assert_eq!(
            scripted.run_next(),
            Ok(Some(rustic_engine::ScriptTurn::ResourceCompletion {
                applied: true
            }))
        );

        // If the physical file changes after the parser completion, the
        // applied bytes remain stable until a new validated host task.
        fs::write(&stylesheet.0, b"#box { width: 99px }").unwrap();
        let changed_png = rustic_raster::encode_png(
            &rustic_raster::Surface::new(8, 3, rustic_raster::RasterLimits::default()).unwrap(),
            usize::MAX,
        )
        .unwrap();
        fs::write(&image.0, changed_png).unwrap();
        scripted
            .set_layout_source(Box::new(LocalLayout {
                path: page.0.clone(),
                limits,
                cancellation: token,
                encoding,
                applied_files: applied,
                network: None,
            }))
            .unwrap();
        scripted
            .queue_script(
                rustic_js::Source::from_str_limited(
                    "const box = document.querySelector('#box');
             const image = document.querySelector('#photo');
             if (getComputedStyle(box).width !== '19px' ||
                 image.offsetWidth !== 4)
                 throw 'resource completion was not pinned';",
                    4096,
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            scripted.run_next(),
            Ok(Some(rustic_engine::ScriptTurn::Completed))
        );
    }

    #[test]
    fn staged_css_and_image_completions_publish_new_layout_and_invalidate_frames() {
        let stylesheet = TempFile::new(b"#box { width: 18px }");
        let first_png = rustic_raster::encode_png(
            &rustic_raster::Surface::new(2, 3, rustic_raster::RasterLimits::default()).unwrap(),
            usize::MAX,
        )
        .unwrap();
        let second_png = rustic_raster::encode_png(
            &rustic_raster::Surface::new(6, 3, rustic_raster::RasterLimits::default()).unwrap(),
            usize::MAX,
        )
        .unwrap();
        let image = TempFile::new(&first_png);
        let html = format!(
            "<!doctype html><link rel=stylesheet href=\"{}\">\
             <div id=box></div><img id=photo src=\"{}\">",
            stylesheet.0.file_name().unwrap().to_str().unwrap(),
            image.0.file_name().unwrap().to_str().unwrap(),
        );
        let page = TempFile::new(html.as_bytes());
        let limits = LoaderLimits::default();
        let mut engine = Engine::new(1);
        let parsed = load_document(&mut engine, &page.0, limits).unwrap();
        let mode = parsed.parsed().mode();
        let encoding = parsed.parsed().encoding();
        let token = parsed.cancellation().clone();
        let (box_id, image_id) = {
            let document = parsed.parsed().document();
            let find_id = |name: &str| {
                document
                    .walk(document.root())
                    .unwrap()
                    .find_map(|(node, _)| {
                        let element = rustic_style::DomElement::new(document, node)?;
                        (crate::reftest::attribute(element, "id").as_deref() == Some(name))
                            .then_some(node)
                    })
                    .unwrap()
            };
            (find_id("box"), find_id("photo"))
        };
        let applied = AppliedLocalFiles::new(
            limits.stylesheets.max_sheets + limits.images.max_images,
            limits.stylesheets.max_total_bytes + limits.images.max_file_bytes,
        );
        let layout = LocalLayout {
            path: page.0.clone(),
            limits,
            cancellation: token.clone(),
            encoding,
            applied_files: applied.clone(),
            network: None,
        };
        let get_width = |geometry: &LayoutGeometry, node| {
            geometry
                .computed_styles
                .get(&node)
                .unwrap()
                .record()
                .longhands
                .iter()
                .find(|(name, _)| *name == "width")
                .unwrap()
                .1
                .clone()
        };
        let initial = layout
            .measure_layout(parsed.parsed().document(), mode)
            .unwrap();
        assert_eq!(get_width(&initial, box_id), "18px");
        assert_eq!(get_width(&initial, image_id), "2px");
        assert_eq!(initial.offset_widths.get(&image_id), Some(&2));

        let mut scripted = parsed
            .document
            .into_scripted_with_clock(TaskLimits::default(), ManualClock::default())
            .unwrap();
        scripted
            .set_layout_source(Box::new(layout.clone()))
            .unwrap();
        let render = |scripted: &mut rustic_engine::ScriptedDocument<ManualClock>| match scripted
            .run_next_with_render(|_, _| ())
            .unwrap()
        {
            Some(rustic_engine::DocumentTurn::Rendered { frame, .. }) => frame,
            other => panic!("expected rendering after completion, got {other:?}"),
        };
        let measure = |scripted: &rustic_engine::ScriptedDocument<ManualClock>| {
            let document = scripted.document();
            layout
                .measure_layout(document.parsed().document(), mode)
                .unwrap()
        };
        scripted.offer_rendering_opportunity().unwrap();
        let previous = render(&mut scripted);

        // Prepare changed local files. The resource read does *not* commit them.
        fs::write(&stylesheet.0, b"#box { width: 43px }").unwrap();
        // A broken replacement must fail validation before publication.
        fs::write(&image.0, b"not a decodable image").unwrap();
        let fetcher = LocalStylesheetFetcher::new(
            fs::canonicalize(page.0.parent().unwrap()).unwrap(),
            limits.read_chunk_bytes,
            token.clone(),
        )
        .unwrap();
        let css_url = file_url(&fs::canonicalize(&stylesheet.0).unwrap()).unwrap();
        let image_url = file_url(&fs::canonicalize(&image.0).unwrap()).unwrap();
        let staged_css = fetcher
            .stage_completion(&applied, &css_url, limits.stylesheets.max_total_bytes)
            .unwrap();
        // Another request for the same URL starts before publication. Once
        // the first completion wins, this one must not publish stale bytes.
        let superseded_css = fetcher
            .stage_completion(&applied, &css_url, limits.stylesheets.max_total_bytes)
            .unwrap();
        assert_eq!(
            crate::images::stage_image_completion(
                &fetcher,
                &applied,
                &image_url,
                limits.images,
                &token,
            )
            .unwrap_err(),
            FetchError::Failed,
        );
        assert_eq!(measure(&scripted).offset_widths.get(&image_id), Some(&2));
        fs::write(&image.0, &second_png).unwrap();
        // Encoded bytes fitting the budget is not enough: decoded RGBA
        // output must fit before the new source becomes applied.
        let too_small = crate::images::ImageLimits {
            max_decoded_bytes: 8,
            ..limits.images
        };
        assert_eq!(
            crate::images::stage_image_completion(
                &fetcher, &applied, &image_url, too_small, &token,
            )
            .unwrap_err(),
            FetchError::TooLarge,
        );
        let staged_image = crate::images::stage_image_completion(
            &fetcher,
            &applied,
            &image_url,
            limits.images,
            &token,
        )
        .unwrap();
        assert_eq!(
            fetcher
                .stage_completion(
                    &applied,
                    &rustic_url::Url::parse("https://outside.example/image.png", None).unwrap(),
                    limits.images.max_file_bytes,
                )
                .unwrap_err(),
            FetchError::Blocked,
        );
        let before_apply = measure(&scripted);
        assert_eq!(get_width(&before_apply, box_id), "18px");
        assert_eq!(get_width(&before_apply, image_id), "2px");
        assert_eq!(before_apply.offset_widths.get(&image_id), Some(&2));

        // A rejected host completion leaves the original render valid.
        scripted
            .queue_resource_completion(|| Err::<(), _>(FetchError::Blocked))
            .unwrap();
        assert_eq!(
            scripted.run_next(),
            Ok(Some(rustic_engine::ScriptTurn::ResourceCompletion {
                applied: false
            }))
        );
        assert_eq!(scripted.accept_rendered_frame(previous), Ok(true));

        scripted.offer_rendering_opportunity().unwrap();
        let stale_css = render(&mut scripted);
        let css_cache = applied.clone();
        scripted
            .queue_resource_completion(move || css_cache.commit(staged_css))
            .unwrap();
        // The bounded host task pump publishes the resource without a direct
        // completion API call or another script execution.
        assert_eq!(
            scripted.run_next(),
            Ok(Some(rustic_engine::ScriptTurn::ResourceCompletion {
                applied: true
            }))
        );
        assert_eq!(scripted.accept_rendered_frame(stale_css), Ok(false));
        let applied_css = measure(&scripted);
        assert_eq!(get_width(&applied_css, box_id), "43px");
        assert_eq!(get_width(&applied_css, image_id), "2px");
        assert_eq!(applied_css.offset_widths.get(&image_id), Some(&2));
        // A successful completion automatically offers the next render.
        let stable_css = render(&mut scripted);
        let stale_cache = applied.clone();
        scripted
            .queue_resource_completion(move || stale_cache.commit(superseded_css))
            .unwrap();
        assert_eq!(
            scripted.run_next(),
            Ok(Some(rustic_engine::ScriptTurn::ResourceCompletion {
                applied: false
            }))
        );
        assert_eq!(scripted.accept_rendered_frame(stable_css), Ok(true));

        scripted.offer_rendering_opportunity().unwrap();
        let stale_image = render(&mut scripted);
        let image_cache = applied.clone();
        scripted
            .queue_resource_completion(move || image_cache.commit(staged_image))
            .unwrap();
        assert_eq!(
            scripted.run_next(),
            Ok(Some(rustic_engine::ScriptTurn::ResourceCompletion {
                applied: true
            }))
        );
        assert_eq!(scripted.accept_rendered_frame(stale_image), Ok(false));
        let applied_both = measure(&scripted);
        assert_eq!(get_width(&applied_both, box_id), "43px");
        assert_eq!(get_width(&applied_both, image_id), "6px");
        assert_eq!(applied_both.offset_widths.get(&image_id), Some(&6));
        let fresh = render(&mut scripted);
        assert_eq!(scripted.accept_rendered_frame(fresh), Ok(true));
    }

    #[test]
    fn scripted_layout_reuses_applied_stylesheet_and_image_bytes_after_file_changes() {
        let stylesheet = TempFile::new(b"#box { width: 21px }");
        let first_png = rustic_raster::encode_png(
            &rustic_raster::Surface::new(3, 2, rustic_raster::RasterLimits::default()).unwrap(),
            usize::MAX,
        )
        .unwrap();
        let replacement_png = rustic_raster::encode_png(
            &rustic_raster::Surface::new(7, 2, rustic_raster::RasterLimits::default()).unwrap(),
            usize::MAX,
        )
        .unwrap();
        let image = TempFile::new(&first_png);
        let html = format!(
            "<!doctype html><link rel=stylesheet href=\"{}\">\
             <div id=box></div><img id=photo src=\"{}\">",
            stylesheet.0.file_name().unwrap().to_str().unwrap(),
            image.0.file_name().unwrap().to_str().unwrap(),
        );
        let page = TempFile::new(html.as_bytes());
        let limits = LoaderLimits::default();
        let mut engine = Engine::new(1);
        let document = load_document(&mut engine, &page.0, limits).unwrap();
        let parsed = document.parsed();
        let tree = parsed.document();
        let mode = parsed.mode();
        let element_id = |name: &str| {
            tree.walk(tree.root())
                .unwrap()
                .find_map(|(node, _)| {
                    let element = rustic_style::DomElement::new(tree, node)?;
                    (crate::reftest::attribute(element, "id").as_deref() == Some(name))
                        .then_some(node)
                })
                .unwrap()
        };
        let box_node = element_id("box");
        let image_node = element_id("photo");
        let applied = AppliedLocalFiles::new(
            limits.stylesheets.max_sheets + limits.images.max_images,
            limits.stylesheets.max_total_bytes + limits.images.max_file_bytes,
        );
        let layout = LocalLayout {
            path: page.0.clone(),
            limits,
            cancellation: document.cancellation().clone(),
            encoding: parsed.encoding(),
            applied_files: applied,
            network: None,
        };
        let width = |snapshot: &LayoutGeometry| {
            snapshot
                .computed_styles
                .get(&box_node)
                .unwrap()
                .record()
                .longhands
                .iter()
                .find(|(name, _)| *name == "width")
                .unwrap()
                .1
                .clone()
        };
        let initial = layout.measure_layout(tree, mode).unwrap();
        assert_eq!(width(&initial), "21px");
        assert_eq!(initial.offset_widths.get(&image_node), Some(&3));

        // These file-system edits are *not* resource-completion deliveries.
        // A later DOM/layout recomputation must not silently apply them.
        fs::write(&stylesheet.0, b"#box { width: 67px }").unwrap();
        fs::write(&image.0, replacement_png).unwrap();
        let after_replacement = layout.measure_layout(tree, mode).unwrap();
        assert_eq!(width(&after_replacement), "21px");
        assert_eq!(after_replacement.offset_widths.get(&image_node), Some(&3));

        fs::remove_file(&stylesheet.0).unwrap();
        fs::remove_file(&image.0).unwrap();
        let after_removal = layout.measure_layout(tree, mode).unwrap();
        assert_eq!(width(&after_removal), "21px");
        assert_eq!(after_removal.offset_widths.get(&image_node), Some(&3));
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
        let mut engine = Engine::new(1);
        let parsed = load_document(&mut engine, &file.0, limits).unwrap();
        assert!(parsed.parsed().truncated());
        assert_eq!(parsed.parsed().document().node_count(), 4);
        limits.parser.tokenizer.max_tokens = 1;
        assert!(matches!(
            dump(&file, limits),
            Err(LoadError::Parse(ParseError::Tokenizer(_)))
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
        let mut scripts = document
            .document
            .into_scripted(TaskLimits::default())
            .unwrap();
        scripts.offer_rendering_opportunity().unwrap();

        let Some(DocumentTurn::Rendered { frame, output }) = scripts
            .run_next_with_render(|document, _| {
                let (_, surface) = rasterize_list(LayoutInput::new(
                    document, &file.0, limits, view, None, None,
                ))?;
                rustic_raster::encode_png(&surface, limits.max_dump_bytes).map_err(LoadError::Png)
            })
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
