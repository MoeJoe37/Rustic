//! Anonymous flow boxes and inline continuations, before line layout.

use crate::{BoxKind, BoxTree};
use rustic_base::CancellationToken;
use rustic_css::{Display, Float, Position, PropertyId as P, PropertyValue as V, WhiteSpace};
use rustic_style::ComputedStyle;
use std::{error::Error, fmt};

/// Independent limits for normalization, including temporary generated boxes.
#[derive(Clone, Copy, Debug)]
pub struct FormattingLimits {
    /// Maximum arena boxes allocated, including later-suppressed whitespace.
    pub max_boxes: usize,
    /// Maximum edges from a root in the published formatting forest.
    pub max_depth: usize,
    /// Maximum work units: source/arena visits, sequence entries and text units.
    pub max_steps: usize,
}
impl Default for FormattingLimits {
    fn default() -> Self {
        Self {
            max_boxes: 100_000,
            max_depth: 512,
            max_steps: 16_000_000,
        }
    }
}

/// Formatting role, independent of the element's principal box identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormattingKind {
    /// An ordinary block or an independent flow-root block.
    Block(Display),
    /// Inline box, possibly one of several continuations of one source.
    Inline,
    /// Atomic inline-level block container; descendants cannot split its parent.
    InlineBlock,
    /// Anonymous block wrapping an inline run beside block-level siblings.
    AnonymousBlock,
    /// Unshaped, unmodified UTF-16 text (an anonymous inline box).
    Text,
}
impl FormattingKind {
    fn block(self) -> bool {
        matches!(self, Self::Block(_) | Self::AnonymousBlock)
    }
}

/// Which original inline edges this continuation retains. Splits leave open
/// edges for later decoration layout; this stage does not compute their sizes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InlineEdges {
    /// This is the first continuation of the source inline box.
    pub start: bool,
    /// This is the last continuation of the source inline box.
    pub end: bool,
}

/// A flat formatting box. Indexes refer to its enclosing [`FormattingTree`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormattingBox {
    parent: Option<usize>,
    source: Option<usize>,
    style_source: usize,
    kind: FormattingKind,
    edges: Option<InlineEdges>,
}
impl FormattingBox {
    /// Formatting parent, or `None` at a root.
    pub const fn parent(&self) -> Option<usize> {
        self.parent
    }
    /// Principal-box index, absent for anonymous block wrappers. Continuations
    /// share a source; callers must not treat this as a unique box ID.
    pub const fn source(&self) -> Option<usize> {
        self.source
    }
    /// Principal style inherited by an anonymous box, or the principal source
    /// itself. This can differ from the formatting parent after inline splits.
    pub const fn style_source(&self) -> usize {
        self.style_source
    }
    /// Formatting role.
    pub const fn kind(&self) -> FormattingKind {
        self.kind
    }
    /// Original start/end edges for an inline continuation.
    pub const fn inline_edges(&self) -> Option<InlineEdges> {
        self.edges
    }
}

/// A borrowed, immutable normal-flow formatting forest. The source snapshot
/// keeps DOM identity and original text; normalized parents need not match DOM
/// parents. No glyphs, line breaks, geometry or paint are computed here.
#[derive(Debug)]
pub struct FormattingTree<'a> {
    principal: &'a BoxTree,
    boxes: Vec<FormattingBox>,
    anonymous_styles: Vec<Option<ComputedStyle>>,
    suppressed: Vec<usize>,
}
impl<'a> FormattingTree<'a> {
    /// Normalize static, non-floating block/flow-root/inline/inline-block
    /// content. Hidden/contents/template handling belongs to [`BoxTree`]. Other
    /// displays and special/replaced/foreign elements fail explicitly (TD-39).
    /// Work and allocation limits also cover inline continuation expansion.
    /// Failure exposes no partial tree; traversal and destruction are iterative.
    pub fn build(
        principal: &'a BoxTree,
        limits: FormattingLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, FormattingError> {
        let mut work = Work {
            steps: 0,
            limits,
            cancellation,
        };
        let source = principal.boxes();
        work.charge(source.len())?;
        let mut children = vec![Vec::new(); source.len()];
        let mut roots = Vec::new();
        for (i, b) in source.iter().enumerate() {
            work.charge(1)?;
            if let Some(p) = b.parent() {
                children[p].push(i);
            } else {
                roots.push(i);
            }
        }
        let mut arena = Vec::<Entry>::new();
        let mut sequences: Vec<Vec<usize>> = vec![Vec::new(); source.len()];
        let mut styles = vec![None; source.len()];
        let mut suppressed = Vec::new();
        for i in (0..source.len()).rev() {
            work.charge(1)?;
            let b = &source[i];
            let style = b
                .style()
                .ok_or(FormattingError::Unsupported { source: i })?;
            let kind = match b.kind() {
                BoxKind::Text => FormattingKind::Text,
                BoxKind::Element(display) => {
                    if b.requires_special_layout()
                        || !matches!(style.get(P::Position), V::Position(Position::Static))
                        || !matches!(style.get(P::Float), V::Float(Float::None))
                    {
                        return Err(FormattingError::Unsupported { source: i });
                    }
                    match display {
                        Display::Block | Display::FlowRoot => FormattingKind::Block(display),
                        Display::Inline => FormattingKind::Inline,
                        Display::InlineBlock => FormattingKind::InlineBlock,
                        _ => return Err(FormattingError::Unsupported { source: i }),
                    }
                }
            };
            let mut items = Vec::new();
            for child in &children[i] {
                work.charge(1)?;
                for item in std::mem::take(&mut sequences[*child]) {
                    work.charge(1)?;
                    items.push(item);
                }
            }
            let mut space = false;
            if kind == FormattingKind::Text {
                styles[i] = Some(style.anonymous_flow(Display::Inline));
                space = matches!(
                    style.get(P::WhiteSpace),
                    V::WhiteSpace(WhiteSpace::Normal | WhiteSpace::Nowrap)
                );
                for &unit in b.text() {
                    work.charge(1)?;
                    space &= matches!(unit, 9 | 10 | 12 | 13 | 32);
                }
            }
            let has_blocks = items.iter().any(|&id| arena[id].box_.kind.block());
            work.charge(items.len())?;
            if kind == FormattingKind::Inline && has_blocks {
                sequences[i] = split_inline(i, items, &mut arena, &mut suppressed, &mut work)?;
            } else {
                if has_blocks {
                    styles[i] = Some(style.anonymous_flow(Display::Block));
                    items = wrap_runs(i, items, &mut arena, &mut suppressed, &mut work)?;
                }
                let entry = Entry {
                    box_: FormattingBox {
                        parent: None,
                        source: Some(i),
                        style_source: i,
                        kind,
                        edges: (kind == FormattingKind::Inline).then_some(InlineEdges {
                            start: true,
                            end: true,
                        }),
                    },
                    children: items,
                    space,
                };
                sequences[i].push(work.allocate(&mut arena, entry)?);
            }
        }
        let mut stack = Vec::new();
        for root in roots.into_iter().rev() {
            for id in sequences[root].iter().rev() {
                work.charge(1)?;
                stack.push((*id, None, 0usize));
            }
        }
        let mut boxes = Vec::new();
        while let Some((id, parent, depth)) = stack.pop() {
            work.charge(1)?;
            if depth > limits.max_depth {
                return Err(FormattingError::DepthLimit);
            }
            let new_id = boxes.len();
            let mut box_ = arena[id].box_.clone();
            box_.parent = parent;
            boxes.push(box_);
            for &child in arena[id].children.iter().rev() {
                work.charge(1)?;
                stack.push((
                    child,
                    Some(new_id),
                    depth.checked_add(1).ok_or(FormattingError::DepthLimit)?,
                ));
            }
        }
        // Reverse source traversal encounters whitespace in reverse parent
        // order. The public list follows principal order for stable diagnostics.
        suppressed.sort_unstable();
        Ok(Self {
            principal,
            boxes,
            anonymous_styles: styles,
            suppressed,
        })
    }

    /// Preorder boxes with snapshot-local parent indexes.
    pub fn boxes(&self) -> &[FormattingBox] {
        &self.boxes
    }
    /// Principal indexes for whitespace-only text omitted at block boundaries.
    /// Its original code units remain available in [`Self::principal`].
    pub fn suppressed_whitespace(&self) -> &[usize] {
        &self.suppressed
    }
    /// The original principal snapshot, shared rather than copied or mutated.
    pub const fn principal(&self) -> &'a BoxTree {
        self.principal
    }
    /// Computed style of a member box. Anonymous boxes inherit supported
    /// inherited properties and reset all other properties to their initials.
    pub fn style(&self, index: usize) -> Option<&ComputedStyle> {
        let b = self.boxes.get(index)?;
        if matches!(
            b.kind,
            FormattingKind::AnonymousBlock | FormattingKind::Text
        ) {
            self.anonymous_styles[b.style_source].as_ref()
        } else {
            self.principal.boxes()[b.style_source].style()
        }
    }
    /// Original UTF-16 for text, empty for other members or an invalid index.
    pub fn text(&self, index: usize) -> &[u16] {
        self.boxes
            .get(index)
            .filter(|b| b.kind == FormattingKind::Text)
            .and_then(|b| b.source)
            .map_or(&[], |i| self.principal.boxes()[i].text())
    }
    /// Deterministic structural dump. No page text, paths or memory addresses;
    /// the byte cap is checked before appending each line.
    pub fn dump(&self, max_bytes: usize) -> Result<String, FormattingError> {
        let mut out = String::new();
        for (i, b) in self.boxes.iter().enumerate() {
            let parent = b.parent.map_or_else(|| "-".into(), |p| p.to_string());
            let source = b.source.map_or_else(|| "-".into(), |s| s.to_string());
            let kind = match b.kind {
                FormattingKind::Block(Display::FlowRoot) => "flow-root",
                FormattingKind::Block(_) => "block",
                FormattingKind::Inline => "inline",
                FormattingKind::InlineBlock => "inline-block",
                FormattingKind::AnonymousBlock => "anonymous-block",
                FormattingKind::Text => "text",
            };
            let mut line = format!(
                "format {i} parent={parent} source={source} style={} kind={kind}",
                b.style_source
            );
            if let Some(edges) = b.edges {
                line.push_str(&format!(
                    " edges={}{}",
                    if edges.start { 's' } else { '-' },
                    if edges.end { 'e' } else { '-' }
                ));
            }
            if b.kind == FormattingKind::Text {
                line.push_str(&format!(" text-units={}", self.text(i).len()));
            }
            line.push('\n');
            append(&mut out, &line, max_bytes)?;
        }
        for source in &self.suppressed {
            append(
                &mut out,
                &format!("suppressed-space source={source}\n"),
                max_bytes,
            )?;
        }
        Ok(out)
    }
}

fn append(out: &mut String, line: &str, max_bytes: usize) -> Result<(), FormattingError> {
    if out
        .len()
        .checked_add(line.len())
        .is_none_or(|n| n > max_bytes)
    {
        return Err(FormattingError::OutputLimit);
    }
    out.push_str(line);
    Ok(())
}
struct Entry {
    box_: FormattingBox,
    children: Vec<usize>,
    space: bool,
}
struct Work<'a> {
    steps: usize,
    limits: FormattingLimits,
    cancellation: &'a CancellationToken,
}
impl Work<'_> {
    fn charge(&mut self, n: usize) -> Result<(), FormattingError> {
        self.cancellation
            .check()
            .map_err(|_| FormattingError::Cancelled)?;
        self.steps = self
            .steps
            .checked_add(n)
            .filter(|&n| n <= self.limits.max_steps)
            .ok_or(FormattingError::WorkLimit)?;
        Ok(())
    }
    fn allocate(&mut self, arena: &mut Vec<Entry>, entry: Entry) -> Result<usize, FormattingError> {
        self.charge(1)?;
        if arena.len() >= self.limits.max_boxes {
            return Err(FormattingError::BoxLimit);
        }
        let id = arena.len();
        arena.push(entry);
        Ok(id)
    }
}

// Removing direct collapsible whitespace from a run is safe only when it
// abuts blocks on both sides (or the block container's own edges). Explicit
// empty inline elements are retained: they can carry borders/decorations.
fn discard_space(
    run: &mut Vec<usize>,
    arena: &[Entry],
    suppressed: &mut Vec<usize>,
    work: &mut Work<'_>,
) -> Result<bool, FormattingError> {
    let mut all_space = true;
    for &id in run.iter() {
        work.charge(1)?;
        all_space &= arena[id].space;
    }
    if all_space {
        for id in run.drain(..) {
            work.charge(1)?;
            suppressed.push(arena[id].box_.source.expect("only text is whitespace"));
        }
    }
    Ok(all_space)
}
fn wrap_runs(
    source: usize,
    items: Vec<usize>,
    arena: &mut Vec<Entry>,
    suppressed: &mut Vec<usize>,
    work: &mut Work<'_>,
) -> Result<Vec<usize>, FormattingError> {
    let mut result = Vec::new();
    let mut run = Vec::new();
    for item in items.into_iter().map(Some).chain(std::iter::once(None)) {
        work.charge(1)?;
        if item.is_none_or(|id| arena[id].box_.kind.block()) {
            if !discard_space(&mut run, arena, suppressed, work)? {
                result.push(work.allocate(
                    arena,
                    Entry {
                        box_: FormattingBox {
                            parent: None,
                            source: None,
                            style_source: source,
                            kind: FormattingKind::AnonymousBlock,
                            edges: None,
                        },
                        children: std::mem::take(&mut run),
                        space: false,
                    },
                )?);
            }
            if let Some(id) = item {
                result.push(id);
            }
        } else if let Some(id) = item {
            run.push(id);
        }
    }
    Ok(result)
}
fn split_inline(
    source: usize,
    items: Vec<usize>,
    arena: &mut Vec<Entry>,
    suppressed: &mut Vec<usize>,
    work: &mut Work<'_>,
) -> Result<Vec<usize>, FormattingError> {
    let mut result = Vec::new();
    let mut run = Vec::new();
    let mut pieces = Vec::new();
    let mut after_block = false;
    for item in items.into_iter().map(Some).chain(std::iter::once(None)) {
        work.charge(1)?;
        if item.is_none_or(|id| arena[id].box_.kind.block()) {
            let empty = discard_space(&mut run, arena, suppressed, work)?;
            if !after_block || !empty || item.is_none() {
                let id = work.allocate(
                    arena,
                    Entry {
                        box_: FormattingBox {
                            parent: None,
                            source: Some(source),
                            style_source: source,
                            kind: FormattingKind::Inline,
                            edges: Some(InlineEdges {
                                start: pieces.is_empty(),
                                end: false,
                            }),
                        },
                        children: std::mem::take(&mut run),
                        space: false,
                    },
                )?;
                pieces.push(id);
                result.push(id);
            }
            if let Some(id) = item {
                result.push(id);
            }
            after_block = true;
        } else if let Some(id) = item {
            run.push(id);
        }
    }
    arena[*pieces
        .last()
        .expect("split has leading and trailing pieces")]
    .box_
    .edges
    .as_mut()
    .expect("inline edges")
    .end = true;
    Ok(result)
}

/// A normalization failure. The source snapshot remains intact on every error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormattingError {
    /// Temporary or retained arena boxes exceeded the configured cap.
    BoxLimit,
    /// Published formatting depth exceeded the configured cap.
    DepthLimit,
    /// Normalization work exceeded its configured cap.
    WorkLimit,
    /// Cancellation was requested.
    Cancelled,
    /// A principal source needs another formatting algorithm (TD-39).
    Unsupported {
        /// Principal-box index of the unsupported source.
        source: usize,
    },
    /// Diagnostic output exceeded its byte cap.
    OutputLimit,
}
impl fmt::Display for FormattingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BoxLimit => f.write_str("formatting box limit exceeded"),
            Self::DepthLimit => f.write_str("formatting depth limit exceeded"),
            Self::WorkLimit => f.write_str("formatting work limit exceeded"),
            Self::Cancelled => f.write_str("formatting cancelled"),
            Self::Unsupported { source } => {
                write!(f, "unsupported formatting at principal box {source}")
            }
            Self::OutputLimit => f.write_str("formatting dump output limit exceeded"),
        }
    }
}
impl Error for FormattingError {}
