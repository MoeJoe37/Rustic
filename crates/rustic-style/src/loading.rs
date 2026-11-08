//! External stylesheets: `@import` resolution through a caller-supplied,
//! policy-controlled fetcher, under explicit budgets.
//!
//! Nothing here touches the file system or the network. The embedder decides
//! what a URL may load by implementing [`StylesheetFetcher`]; the loader only
//! resolves URLs, enforces [`StyleLoadLimits`], refuses import cycles and
//! orders the resulting sheets for the cascade. The import tree is walked
//! with an explicit stack, so a deep chain of imports cannot overflow the call
//! stack even before the depth limit applies.

use std::collections::HashMap;

use rustic_base::SourceSpan;
use rustic_css::{CssParser, MediaQueryList};
use rustic_url::Url;

use crate::{ImportRule, Origin, StyleDiagnostic, StyleSheet};

/// Why a [`StylesheetFetcher`] did not return a sheet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FetchError {
    /// The fetch policy does not allow this URL (scheme, location, ...).
    Blocked,
    /// Nothing exists at the URL.
    NotFound,
    /// The resource is larger than the byte budget offered to the fetcher.
    TooLarge,
    /// The resource exists but could not be read.
    Failed,
    /// The load was cancelled.
    Cancelled,
}

/// Loads stylesheet text for a URL on behalf of the style system.
///
/// Implementations own the policy: which schemes and locations are allowed,
/// how bytes are read, and how they are decoded to text. They must never
/// return more than `max_bytes` UTF-8 bytes; the loader treats a longer
/// result as [`FetchError::TooLarge`].
pub trait StylesheetFetcher {
    /// Fetch and decode the stylesheet at `url`.
    fn fetch(&mut self, url: &Url, max_bytes: usize) -> Result<String, FetchError>;
}

/// A fetcher that allows nothing, for documents loaded without subresources.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoFetch;

impl StylesheetFetcher for NoFetch {
    fn fetch(&mut self, _url: &Url, _max_bytes: usize) -> Result<String, FetchError> {
        Err(FetchError::Blocked)
    }
}

/// Default for [`StyleLoadLimits::max_sheets`].
pub const DEFAULT_MAX_EXTERNAL_SHEETS: usize = 64;
/// Default for [`StyleLoadLimits::max_import_depth`].
pub const DEFAULT_MAX_IMPORT_DEPTH: usize = 16;
/// Default for [`StyleLoadLimits::max_total_bytes`].
pub const DEFAULT_MAX_EXTERNAL_BYTES: usize = 8 * 1024 * 1024;

/// Budgets shared by every external sheet of one document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StyleLoadLimits {
    /// External sheets (linked and imported) that may be fetched; a fetch
    /// that fails still counts.
    pub max_sheets: usize,
    /// Maximum `@import` nesting below a `<style>` or `<link>` sheet.
    pub max_import_depth: usize,
    /// Total UTF-8 bytes of external sheet text, counted once per inclusion.
    pub max_total_bytes: usize,
}

impl Default for StyleLoadLimits {
    fn default() -> Self {
        Self {
            max_sheets: DEFAULT_MAX_EXTERNAL_SHEETS,
            max_import_depth: DEFAULT_MAX_IMPORT_DEPTH,
            max_total_bytes: DEFAULT_MAX_EXTERNAL_BYTES,
        }
    }
}

/// What happened to one stylesheet: a `<style>` or `<link>` element's sheet
/// or an `@import`ed one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SheetStatus {
    /// Its sheet was added to the cascade.
    Applied,
    /// Its `type` attribute is not empty or `text/css`, so it is not CSS.
    NotCss,
    /// An `@import` with a `layer` or `supports()` condition; cascade layers
    /// and `@supports` are not implemented yet.
    UnsupportedCondition,
    /// A `<link>` that is an alternative stylesheet (`rel=alternate`) or
    /// has a `disabled` attribute, so it is not applied by default.
    Disabled,
    /// The URL could not be parsed against the base URL.
    InvalidUrl,
    /// The URL is already being imported further up the chain.
    Cycle,
    /// The import is nested deeper than [`StyleLoadLimits::max_import_depth`].
    DepthLimit,
    /// [`StyleLoadLimits::max_sheets`] or `max_total_bytes` is used up.
    LoadLimit,
    /// The fetcher did not return the sheet.
    Fetch(FetchError),
    /// Its text exceeded the CSS parser's limits.
    ResourceLimit,
}

/// One `@import` found while loading a sheet, with its own outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportReport {
    /// The URL as written in the rule.
    pub url: String,
    /// 1 for an import in the element's sheet, 2 for one in that import, ...
    pub depth: usize,
    /// The `@import` rule's span in the importing sheet's text.
    pub span: SourceSpan,
    /// What happened to it.
    pub status: SheetStatus,
    /// Constructs dropped from the imported sheet (spans are within its text).
    pub diagnostics: Vec<StyleDiagnostic>,
}

/// Fetching state for one document: the fetcher, the remaining budgets and a
/// cache, so a sheet imported twice is fetched once (but parsed and counted
/// against the byte budget at each inclusion).
pub struct StyleLoader<'a> {
    fetcher: &'a mut dyn StylesheetFetcher,
    limits: StyleLoadLimits,
    sheets_used: usize,
    bytes_used: usize,
    cache: HashMap<String, Result<String, FetchError>>,
}

impl std::fmt::Debug for StyleLoader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StyleLoader")
            .field("limits", &self.limits)
            .field("sheets_used", &self.sheets_used)
            .field("bytes_used", &self.bytes_used)
            .finish_non_exhaustive()
    }
}

/// The result of loading one sheet and everything it imports.
#[derive(Debug)]
pub(crate) struct LoadedTree {
    /// Imported sheets then the root sheet, in cascade order.
    pub(crate) sheets: Vec<StyleSheet>,
    /// Every `@import` in preorder.
    pub(crate) imports: Vec<ImportReport>,
}

struct Frame {
    sheet: StyleSheet,
    imports: Vec<ImportRule>,
    next: usize,
    base: Option<Url>,
    key: Option<String>,
    /// Media query lists that restrict this sheet: its own import's list
    /// and every ancestor's (including the element's `media` attribute).
    media: Vec<MediaQueryList>,
}

impl<'a> StyleLoader<'a> {
    /// A loader that fetches through `fetcher` within `limits`.
    pub fn new(fetcher: &'a mut dyn StylesheetFetcher, limits: StyleLoadLimits) -> Self {
        Self {
            fetcher,
            limits,
            sheets_used: 0,
            bytes_used: 0,
            cache: HashMap::new(),
        }
    }

    /// External sheets fetched so far, including failed fetches.
    pub fn sheets_used(&self) -> usize {
        self.sheets_used
    }

    /// External sheet bytes parsed so far.
    pub fn bytes_used(&self) -> usize {
        self.bytes_used
    }

    /// Fetch `url` within the remaining budget and return its text.
    pub(crate) fn fetch(&mut self, url: &Url) -> Result<String, SheetStatus> {
        if self.sheets_used >= self.limits.max_sheets {
            return Err(SheetStatus::LoadLimit);
        }
        let remaining = self.limits.max_total_bytes.saturating_sub(self.bytes_used);
        if remaining == 0 {
            return Err(SheetStatus::LoadLimit);
        }
        self.sheets_used += 1;
        let key = url.serialize_without_fragment();
        let result = match self.cache.get(&key) {
            Some(cached) => cached.clone(),
            None => {
                let mut result = self.fetcher.fetch(url, remaining);
                if result.as_ref().is_ok_and(|text| text.len() > remaining) {
                    result = Err(FetchError::TooLarge);
                }
                self.cache.insert(key, result.clone());
                result
            }
        };
        let text = result.map_err(SheetStatus::Fetch)?;
        if text.len() > remaining {
            // A cached sheet included again after the budget shrank.
            return Err(SheetStatus::LoadLimit);
        }
        self.bytes_used += text.len();
        Ok(text)
    }

    /// Parse `source` (an element's sheet, or a linked sheet fetched from
    /// `url`) and load its imports depth first. Imported sheets precede the
    /// sheet that imports them, as CSS requires. `media` (the element's
    /// `media` attribute) restricts every sheet in the tree.
    pub(crate) fn load_tree(
        &mut self,
        parser: &CssParser,
        source: &str,
        base: Option<&Url>,
        url: Option<&Url>,
        media: Option<MediaQueryList>,
    ) -> Result<(LoadedTree, Vec<StyleDiagnostic>), rustic_css::CssError> {
        let (sheet, imports, diagnostics) =
            StyleSheet::parse_with_imports(parser, source, Origin::Author)?;
        let mut stack = vec![Frame {
            sheet,
            imports,
            next: 0,
            base: base.cloned(),
            key: url.map(Url::serialize_without_fragment),
            media: media.into_iter().collect(),
        }];
        let mut tree = LoadedTree {
            sheets: Vec::new(),
            imports: Vec::new(),
        };
        while let Some(frame) = stack.last_mut() {
            let Some(import) = frame.imports.get(frame.next).cloned() else {
                let mut done = stack.pop().expect("the loop saw a frame");
                for media in done.media {
                    done.sheet.restrict_to(media);
                }
                tree.sheets.push(done.sheet);
                continue;
            };
            frame.next += 1;
            let base = frame.base.clone();
            let mut media = frame.media.clone();
            media.extend(import.media.clone());
            let depth = stack.len();
            let mut report = ImportReport {
                url: import.url.clone(),
                depth,
                span: import.span,
                status: SheetStatus::Applied,
                diagnostics: Vec::new(),
            };
            match self.import(parser, &import, base.as_ref(), depth, &stack, media) {
                Ok((frame, diagnostics)) => {
                    report.diagnostics = diagnostics;
                    stack.push(frame);
                }
                Err(status) => report.status = status,
            }
            tree.imports.push(report);
        }
        Ok((tree, diagnostics))
    }

    fn import(
        &mut self,
        parser: &CssParser,
        import: &ImportRule,
        base: Option<&Url>,
        depth: usize,
        stack: &[Frame],
        media: Vec<MediaQueryList>,
    ) -> Result<(Frame, Vec<StyleDiagnostic>), SheetStatus> {
        if import.layer || import.supports {
            return Err(SheetStatus::UnsupportedCondition);
        }
        let url = Url::parse(&import.url, base).map_err(|_| SheetStatus::InvalidUrl)?;
        let key = url.serialize_without_fragment();
        if stack.iter().any(|f| f.key.as_deref() == Some(key.as_str())) {
            return Err(SheetStatus::Cycle);
        }
        if depth > self.limits.max_import_depth {
            return Err(SheetStatus::DepthLimit);
        }
        let text = self.fetch(&url)?;
        let (sheet, imports, diagnostics) =
            StyleSheet::parse_with_imports(parser, &text, Origin::Author)
                .map_err(|_| SheetStatus::ResourceLimit)?;
        Ok((
            Frame {
                sheet,
                imports,
                next: 0,
                base: Some(url),
                key: Some(key),
                media,
            },
            diagnostics,
        ))
    }
}
