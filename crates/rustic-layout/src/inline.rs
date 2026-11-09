//! Inline formatting context content (CSS 2.2 §9.4.2, CSS Text 3 §4):
//! collection of text and inline boxes, white-space processing,
//! text-transform, bidi levels, UAX #14 break opportunities and shaping.
//!
//! Everything here is independent of the available width, so it is built
//! once per block container and reused by intrinsic sizing, float retries
//! and line breaking ([`crate::lines`]).

use std::ops::Range;

use rustic_css::{
    Direction, Display, FamilyName, FontStyle, Position, PropertyId as P, PropertyValue as V,
    Spacing, TextCase, VerticalAlign, VerticalAlignKeyword, WhiteSpace,
};
use rustic_dom::NodeId;
use rustic_style::ComputedStyle;
use rustic_text::bidi::{BidiInfo, Level};
use rustic_text::segment::{BreakOpportunity, line_breaks};
use rustic_text::shape::{ShapeOptions, ShapedGlyph, is_word_separator, shape};
use rustic_text::{FaceQuery, FaceStyle, FamilyRef, FontDatabase};

use crate::flow::{FlowError, UnsupportedFlow, Work};
use crate::sizing::{box_edges, lp};
use crate::tree::HtmlTag;
use crate::{BoxKind, BoxTree};

/// Font-derived vertical metrics of one style, in CSS pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FontBox {
    pub ascent: f64,
    pub descent: f64,
    pub line_height: f64,
    pub x_height: f64,
    pub size: f64,
}

impl FontBox {
    /// Half-leading: space above the ascent and below the descent.
    pub(crate) fn half_leading(&self) -> f64 {
        (self.line_height - (self.ascent + self.descent)) / 2.0
    }
}

/// `vertical-align` of an inline box (CSS 2.2 §10.8.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Align {
    /// Baseline shift downwards from the parent baseline, in pixels.
    Shift(f64),
    TextTop,
    TextBottom,
    Middle,
    Top,
    Bottom,
}

/// A non-atomic inline box (`display:inline`).
#[derive(Clone, Debug)]
pub(crate) struct InlineBox<'a> {
    pub source: usize,
    pub node: NodeId,
    pub style: &'a ComputedStyle,
    pub parent: Option<usize>,
    pub depth: usize,
    pub font: FontBox,
    pub align: Align,
    /// Margin, border and padding widths: (left, right) pairs.
    pub margin: (f64, f64),
    pub border: (f64, f64),
    pub padding: (f64, f64),
    /// Top/bottom border and padding, outside the content area.
    pub border_v: (f64, f64),
    pub padding_v: (f64, f64),
    pub direction: Direction,
    /// Relative-positioning shift, applied after line layout.
    pub relative: (f64, f64),
}

impl InlineBox<'_> {
    /// Width of the start (`open`) or end edge: margin + border + padding.
    pub(crate) fn edge(&self, open: bool) -> f64 {
        let left = self.margin.0 + self.border.0 + self.padding.0;
        let right = self.margin.1 + self.border.1 + self.padding.1;
        match (open, self.direction) {
            (true, Direction::Ltr) | (false, Direction::Rtl) => left,
            _ => right,
        }
    }
}

/// One text box's contribution.
#[derive(Clone, Debug)]
pub(crate) struct TextItem<'a> {
    pub source: usize,
    pub node: NodeId,
    pub style: &'a ComputedStyle,
    pub parent: Option<usize>,
    pub chain: Vec<usize>,
    pub font: FontBox,
}

/// How a cluster behaves at a line edge (CSS Text 3 §4.1.2–4.1.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Space {
    /// Ordinary content.
    None,
    /// A collapsible space: removed at the start and end of a line.
    Collapsible,
    /// A preserved space under `pre-wrap`: hangs at the end of a line.
    Hanging,
    /// A preserved space that neither collapses nor hangs.
    Preserved,
    /// A preserved tab; its width is the distance to the next tab stop.
    Tab(f64),
    /// A forced line break (preserved newline or `<br>`).
    Forced,
}

/// One unit of line content in logical order.
#[derive(Clone, Debug)]
pub(crate) enum Piece {
    Cluster {
        chars: Range<usize>,
        text: usize,
        face: usize,
        glyphs: Vec<ShapedGlyph>,
        width: f64,
        space: Space,
        /// A U+0020 that can absorb justification.
        justifiable: bool,
    },
    Edge {
        ibox: usize,
        open: bool,
        width: f64,
        at: usize,
    },
}

impl Piece {
    pub(crate) fn space(&self) -> Space {
        match self {
            Self::Cluster { space, .. } => *space,
            Self::Edge { .. } => Space::None,
        }
    }
}

/// Where a character came from: its text item and UTF-16 range in that box.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Source {
    pub text: usize,
    pub start: usize,
    pub end: usize,
}

/// The width-independent content of one inline formatting context.
#[derive(Debug)]
pub(crate) struct Ifc<'a> {
    pub root: &'a ComputedStyle,
    pub root_font: FontBox,
    pub direction: Direction,
    pub boxes: Vec<InlineBox<'a>>,
    pub texts: Vec<TextItem<'a>>,
    pub chars: Vec<char>,
    pub sources: Vec<Source>,
    pub pieces: Vec<Piece>,
    /// Whether a line may end after piece `k` (and whether it must).
    pub breaks: Vec<Option<BreakOpportunity>>,
    pub bidi: BidiInfo,
    /// Whether any piece needs a line (content, edges or preserved spaces).
    pub has_content: bool,
}

fn white_space(style: &ComputedStyle) -> Result<WhiteSpace, FlowError> {
    match style.get(P::WhiteSpace) {
        V::WhiteSpace(w) => Ok(*w),
        _ => Err(FlowError::InvalidSnapshot),
    }
}

pub(crate) fn direction_of(style: &ComputedStyle) -> Result<Direction, FlowError> {
    match style.get(P::Direction) {
        V::Direction(d) => Ok(*d),
        _ => Err(FlowError::InvalidSnapshot),
    }
}

fn wraps(w: WhiteSpace) -> bool {
    !matches!(w, WhiteSpace::Nowrap | WhiteSpace::Pre)
}

fn collapses_spaces(w: WhiteSpace) -> bool {
    matches!(
        w,
        WhiteSpace::Normal | WhiteSpace::Nowrap | WhiteSpace::PreLine
    )
}

fn preserves_breaks(w: WhiteSpace) -> bool {
    !matches!(w, WhiteSpace::Normal | WhiteSpace::Nowrap)
}

/// The face chain and vertical metrics of a style's font.
pub(crate) fn font_of(
    style: &ComputedStyle,
    fonts: &FontDatabase,
) -> Result<(Vec<usize>, FontBox), FlowError> {
    let families: &[FamilyName] = match style.get(P::FontFamily) {
        V::FontFamily(f) => f,
        _ => return Err(FlowError::InvalidSnapshot),
    };
    let weight = match style.get(P::FontWeight) {
        V::FontWeight(rustic_css::FontWeight::Number(n)) => *n,
        V::FontWeight(rustic_css::FontWeight::Bold) => 700.0,
        V::FontWeight(_) => 400.0,
        _ => return Err(FlowError::InvalidSnapshot),
    };
    let slope = match style.get(P::FontStyle) {
        V::FontStyle(FontStyle::Italic) => FaceStyle::Italic,
        V::FontStyle(FontStyle::Oblique) => FaceStyle::Oblique,
        V::FontStyle(FontStyle::Normal) => FaceStyle::Normal,
        _ => return Err(FlowError::InvalidSnapshot),
    };
    let stretch = match style.get(P::FontStretch) {
        V::FontStretch(rustic_css::FontStretch::Percentage(p)) => *p,
        V::FontStretch(rustic_css::FontStretch::Keyword(k)) => k.percentage(),
        _ => return Err(FlowError::InvalidSnapshot),
    };
    let query = FaceQuery {
        weight,
        style: slope,
        stretch,
    };
    let chain = fonts.fallback_chain(
        families.iter().map(|f| match f {
            FamilyName::Named(n) => FamilyRef::Named(n),
            FamilyName::Generic(g) => FamilyRef::Generic(g.name()),
        }),
        &query,
    );
    let size = style.font_size_px();
    let (ascent, descent, gap) = match chain.first().and_then(|&i| fonts.face(i)) {
        Some(face) => {
            let m = face.metrics();
            let scale = size / f64::from(face.units_per_em());
            (
                f64::from(m.ascent) * scale,
                -f64::from(m.descent) * scale,
                f64::from(m.line_gap.max(0)) * scale,
            )
        }
        // No face at all: CSS still needs a strut; use common proportions.
        None => (0.8 * size, 0.2 * size, 0.0),
    };
    let line_height = match style.get(P::LineHeight) {
        V::LineHeight(rustic_css::LineHeight::Normal) => ascent + descent + gap,
        V::LineHeight(rustic_css::LineHeight::Number(n)) => n * size,
        V::LineHeight(rustic_css::LineHeight::LengthPercentage(v)) => lp(v, size)?,
        _ => return Err(FlowError::InvalidSnapshot),
    };
    Ok((
        chain,
        FontBox {
            ascent,
            descent,
            line_height: line_height.max(0.0),
            x_height: style.font_metrics().x_height * size,
            size,
        },
    ))
}

fn spacing(style: &ComputedStyle, property: P, size: f64) -> Result<f64, FlowError> {
    match style.get(property) {
        V::Spacing(Spacing::Normal) => Ok(0.0),
        V::Spacing(Spacing::LengthPercentage(v)) => Ok(lp(v, size)?),
        _ => Err(FlowError::InvalidSnapshot),
    }
}

fn align_of(style: &ComputedStyle, parent: &FontBox) -> Result<Align, FlowError> {
    Ok(match style.get(P::VerticalAlign) {
        V::VerticalAlign(VerticalAlign::Keyword(k)) => match k {
            VerticalAlignKeyword::Baseline => Align::Shift(0.0),
            // UA-defined offsets (CSS 2.2 §10.8.1), as in common engines.
            VerticalAlignKeyword::Sub => Align::Shift(parent.size / 5.0),
            VerticalAlignKeyword::Super => Align::Shift(-parent.size / 3.0),
            VerticalAlignKeyword::TextTop => Align::TextTop,
            VerticalAlignKeyword::TextBottom => Align::TextBottom,
            VerticalAlignKeyword::Middle => Align::Middle,
            VerticalAlignKeyword::Top => Align::Top,
            VerticalAlignKeyword::Bottom => Align::Bottom,
        },
        V::VerticalAlign(VerticalAlign::LengthPercentage(v)) => Align::Shift(-lp(v, 0.0)?),
        _ => return Err(FlowError::InvalidSnapshot),
    })
}

/// CSS Text 3 §2.1 `text-transform`, after white-space collapsing.
fn transform(style: &ComputedStyle, c: char, word_start: bool, out: &mut Vec<char>) {
    let t = match style.get(P::TextTransform) {
        V::TextTransform(t) => *t,
        _ => rustic_css::TextTransform::default(),
    };
    let start = out.len();
    match t.case {
        Some(TextCase::Uppercase) => out.extend(c.to_uppercase()),
        Some(TextCase::Lowercase) => out.extend(c.to_lowercase()),
        // Title case is approximated by uppercase (TD-50).
        Some(TextCase::Capitalize) if word_start => out.extend(c.to_uppercase()),
        _ => out.push(c),
    }
    if t.full_width {
        for ch in &mut out[start..] {
            *ch = match *ch {
                ' ' => '\u{3000}',
                '!'..='~' => char::from_u32(u32::from(*ch) + 0xFEE0).unwrap_or(*ch),
                other => other,
            };
        }
    }
}

/// One step of the logical content: text from a box, or an inline box edge.
enum Event {
    Text { item: usize, chars: Range<usize> },
    Open(usize),
    Close(usize),
}

/// Character flags recorded by white-space processing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Normal,
    Collapsible,
    Preserved,
    Tab,
    Forced,
}

struct Collector<'a, 't, 'f> {
    tree: &'t BoxTree,
    fonts: &'f FontDatabase,
    ifc: Ifc<'a>,
    kinds: Vec<Kind>,
    events: Vec<Event>,
    /// The last emitted character is a collapsible space.
    after_space: bool,
    /// Skip collapsible spaces after a preserved segment break (`pre-line`).
    after_break: bool,
}

impl<'a> Collector<'a, '_, '_> {
    fn push_char(&mut self, c: char, kind: Kind, source: Source, style: &ComputedStyle) {
        let word_start = self.ifc.chars.last().is_none_or(|p| !p.is_alphanumeric());
        let before = self.ifc.chars.len();
        if kind == Kind::Normal {
            transform(style, c, word_start, &mut self.ifc.chars);
        } else {
            self.ifc.chars.push(c);
        }
        let added = self.ifc.chars.len() - before;
        self.kinds.extend(std::iter::repeat_n(kind, added));
        self.ifc.sources.extend(std::iter::repeat_n(source, added));
        self.after_space = kind == Kind::Collapsible;
    }

    /// White-space phase I for one text box (CSS Text 3 §4.1.1).
    fn text(
        &mut self,
        item: usize,
        units: &[u16],
        style: &'a ComputedStyle,
        work: &mut Work<'_>,
    ) -> Result<(), FlowError> {
        let ws = white_space(style)?;
        let start = self.ifc.chars.len();
        let mut offset = 0;
        for decoded in char::decode_utf16(units.iter().copied()) {
            work.charge(1)?;
            let (c, len) = match decoded {
                Ok(c) => (c, c.len_utf16()),
                Err(_) => ('\u{FFFD}', 1),
            };
            let source = Source {
                text: item,
                start: offset,
                end: offset + len,
            };
            offset += len;
            let c = if c == '\r' { ' ' } else { c };
            match c {
                '\n' if preserves_breaks(ws) => {
                    if collapses_spaces(ws) {
                        self.trim_trailing_spaces();
                    }
                    self.push_char('\n', Kind::Forced, source, style);
                    self.after_break = collapses_spaces(ws);
                }
                ' ' | '\t' | '\n' if collapses_spaces(ws) => {
                    if self.after_space || self.after_break {
                        continue;
                    }
                    self.push_char(' ', Kind::Collapsible, source, style);
                }
                '\t' => {
                    self.push_char('\t', Kind::Tab, source, style);
                    self.after_break = false;
                }
                ' ' => {
                    self.push_char(' ', Kind::Preserved, source, style);
                    self.after_break = false;
                }
                _ => {
                    self.push_char(c, Kind::Normal, source, style);
                    self.after_break = false;
                }
            }
        }
        let end = self.ifc.chars.len();
        if end > start {
            self.events.push(Event::Text {
                item,
                chars: start..end,
            });
        }
        Ok(())
    }

    /// Remove collapsible spaces just before a preserved segment break,
    /// unless an inline box edge separates them (kept for simplicity).
    fn trim_trailing_spaces(&mut self) {
        while self.kinds.last() == Some(&Kind::Collapsible) {
            let Some(Event::Text { chars, .. }) = self.events.last_mut() else {
                break;
            };
            if chars.end != self.ifc.chars.len() {
                break;
            }
            self.kinds.pop();
            self.ifc.chars.pop();
            self.ifc.sources.pop();
            chars.end -= 1;
            if chars.start == chars.end {
                self.events.pop();
            }
        }
        // Spaces still pending in the current box are trimmed by the caller.
    }

    fn special(
        &mut self,
        c: char,
        source: usize,
        style: &'a ComputedStyle,
        parent: Option<usize>,
    ) -> Result<(), FlowError> {
        let item = self.add_text_item(source, style, parent)?;
        let start = self.ifc.chars.len();
        let kind = if c == '\n' {
            Kind::Forced
        } else {
            Kind::Normal
        };
        self.ifc.chars.push(c);
        self.kinds.push(kind);
        self.ifc.sources.push(Source {
            text: item,
            start: 0,
            end: 0,
        });
        self.after_space = false;
        self.after_break = kind == Kind::Forced;
        self.events.push(Event::Text {
            item,
            chars: start..start + 1,
        });
        Ok(())
    }

    fn add_text_item(
        &mut self,
        source: usize,
        style: &'a ComputedStyle,
        parent: Option<usize>,
    ) -> Result<usize, FlowError> {
        let (chain, font) = font_of(style, self.fonts)?;
        self.ifc.texts.push(TextItem {
            source,
            node: self.tree.boxes()[source].node(),
            style,
            parent,
            chain,
            font,
        });
        Ok(self.ifc.texts.len() - 1)
    }
}

/// Collect, process, analyze and shape one inline formatting context made
/// of `children` (box indexes) of a block container styled `root`.
pub(crate) fn build<'a>(
    tree: &'a BoxTree,
    ends: &[usize],
    root: &'a ComputedStyle,
    children: &[usize],
    fonts: &FontDatabase,
    work: &mut Work<'_>,
) -> Result<Ifc<'a>, FlowError> {
    let direction = direction_of(root)?;
    let (_, root_font) = font_of(root, fonts)?;
    let mut c = Collector {
        tree,
        fonts,
        ifc: Ifc {
            root,
            root_font,
            direction,
            boxes: Vec::new(),
            texts: Vec::new(),
            chars: Vec::new(),
            sources: Vec::new(),
            pieces: Vec::new(),
            breaks: Vec::new(),
            bidi: BidiInfo::new(&[], None),
            has_content: false,
        },
        kinds: Vec::new(),
        events: Vec::new(),
        after_space: false,
        after_break: true,
    };
    let boxes = tree.boxes();
    for &child in children {
        // (box end, inline box index) of open inline boxes.
        let mut open: Vec<(usize, usize)> = Vec::new();
        let mut k = child;
        while k < ends[child] {
            work.charge(1)?;
            while open.last().is_some_and(|&(end, _)| end <= k) {
                if let Some((_, b)) = open.pop() {
                    c.events.push(Event::Close(b));
                }
            }
            let parent = open.last().map(|&(_, b)| b);
            let b = &boxes[k];
            let style = b.style().ok_or(FlowError::InvalidSnapshot)?;
            let fail = |reason| FlowError::Unsupported {
                node: b.node(),
                reason,
            };
            match b.kind() {
                BoxKind::Text => {
                    let item = c.add_text_item(k, style, parent)?;
                    c.text(item, b.text(), style, work)?;
                }
                BoxKind::Element(Display::Inline) => {
                    if matches!(style.get(P::Position), V::Position(Position::Sticky)) {
                        return Err(fail(UnsupportedFlow::Positioning));
                    }
                    if matches!(
                        style.get(P::Position),
                        V::Position(Position::Absolute | Position::Fixed)
                    ) || !matches!(style.get(P::Float), V::Float(rustic_css::Float::None))
                    {
                        return Err(fail(UnsupportedFlow::FormattingContext));
                    }
                    match (b.requires_special_layout(), b.html_tag()) {
                        (true, HtmlTag::Br) => {
                            c.special('\n', k, style, parent)?;
                            k = ends[k];
                            continue;
                        }
                        (true, HtmlTag::Wbr) => {
                            c.special('\u{200B}', k, style, parent)?;
                            k = ends[k];
                            continue;
                        }
                        (true, _) => return Err(fail(UnsupportedFlow::SpecialElement)),
                        (false, _) => {}
                    }
                    let index = inline_box(&mut c.ifc, tree, k, style, parent, fonts)?;
                    c.events.push(Event::Open(index));
                    c.after_break = false;
                    open.push((ends[k], index));
                }
                BoxKind::Element(_) => return Err(fail(UnsupportedFlow::FormattingContext)),
            }
            k += 1;
        }
        while let Some((_, b)) = open.pop() {
            c.events.push(Event::Close(b));
        }
    }
    let Collector {
        mut ifc,
        kinds,
        events,
        ..
    } = c;
    ifc.bidi = BidiInfo::new(
        &ifc.chars,
        Some(if direction == Direction::Rtl {
            Level::RTL
        } else {
            Level::LTR
        }),
    );
    shape_events(&mut ifc, &kinds, &events, fonts, work)?;
    ifc.breaks = break_opportunities(&ifc, work)?;
    ifc.has_content = ifc.pieces.iter().any(|p| match p {
        Piece::Cluster { space, .. } => *space != Space::Collapsible,
        Piece::Edge { width, .. } => *width != 0.0,
    });
    Ok(ifc)
}

fn inline_box<'a>(
    ifc: &mut Ifc<'a>,
    tree: &BoxTree,
    k: usize,
    style: &'a ComputedStyle,
    parent: Option<usize>,
    fonts: &FontDatabase,
) -> Result<usize, FlowError> {
    let (_, font) = font_of(style, fonts)?;
    let parent_font = parent.map_or(ifc.root_font, |p| ifc.boxes[p].font);
    // Percentages of inline box padding/margins resolve against the
    // containing block width, which is unknown here (TD-50): they use 0.
    let (padding, border) = box_edges(style, 0.0)?;
    let margin =
        |p| -> Result<f64, FlowError> { Ok(crate::sizing::auto(style, p, 0.0)?.unwrap_or(0.0)) };
    let relative = if matches!(style.get(P::Position), V::Position(Position::Relative)) {
        let side = |p| crate::sizing::auto(style, p, 0.0);
        let (l, r, t, bo) = (
            side(P::Left)?,
            side(P::Right)?,
            side(P::Top)?,
            side(P::Bottom)?,
        );
        let x = match (l, r) {
            (Some(l), _) => l,
            (None, Some(r)) => -r,
            _ => 0.0,
        };
        let y = match (t, bo) {
            (Some(t), _) => t,
            (None, Some(b)) => -b,
            _ => 0.0,
        };
        (x, y)
    } else {
        (0.0, 0.0)
    };
    let depth = parent.map_or(0, |p| ifc.boxes[p].depth + 1);
    ifc.boxes.push(InlineBox {
        source: k,
        node: tree.boxes()[k].node(),
        style,
        parent,
        depth,
        font,
        align: align_of(style, &parent_font)?,
        margin: (margin(P::MarginLeft)?, margin(P::MarginRight)?),
        border: (border.left.get(), border.right.get()),
        padding: (padding.left.get(), padding.right.get()),
        border_v: (border.top.get(), border.bottom.get()),
        padding_v: (padding.top.get(), padding.bottom.get()),
        direction: direction_of(style)?,
        relative,
    });
    Ok(ifc.boxes.len() - 1)
}

/// Turn events into pieces, shaping each text event per bidi level run.
fn shape_events(
    ifc: &mut Ifc<'_>,
    kinds: &[Kind],
    events: &[Event],
    fonts: &FontDatabase,
    work: &mut Work<'_>,
) -> Result<(), FlowError> {
    let levels = ifc.bidi.levels().to_vec();
    for event in events {
        match *event {
            Event::Open(b) | Event::Close(b) => {
                let open = matches!(event, Event::Open(_));
                ifc.pieces.push(Piece::Edge {
                    ibox: b,
                    open,
                    width: ifc.boxes[b].edge(open),
                    at: 0,
                });
            }
            Event::Text { item, ref chars } => {
                let style = ifc.texts[item].style;
                let size = ifc.texts[item].font.size;
                let letter = spacing(style, P::LetterSpacing, size)?;
                let word = spacing(style, P::WordSpacing, size)?;
                let mut start = chars.start;
                while start < chars.end {
                    // A run of one level and one kind class (spaces/tabs/breaks
                    // are shaped one by one so their kind stays exact).
                    let special = kinds[start] != Kind::Normal;
                    let mut end = start + 1;
                    while !special
                        && end < chars.end
                        && levels[end] == levels[start]
                        && kinds[end] == Kind::Normal
                    {
                        end += 1;
                    }
                    work.charge(end - start)?;
                    let options = ShapeOptions {
                        size,
                        rtl: levels[start].is_rtl(),
                        letter_spacing: letter,
                        word_spacing: word,
                    };
                    let chain = &ifc.texts[item].chain;
                    let clusters =
                        shape(fonts, chain, &ifc.chars[start..end], &options).map_err(|_| {
                            FlowError::Unsupported {
                                node: ifc.texts[item].node,
                                reason: UnsupportedFlow::TextLayout,
                            }
                        })?;
                    for cluster in clusters {
                        let range = cluster.range.start + start..cluster.range.end + start;
                        let first = ifc.chars[range.start];
                        let space = match kinds[range.start] {
                            Kind::Normal => Space::None,
                            Kind::Collapsible => Space::Collapsible,
                            Kind::Preserved => match white_space(style)? {
                                WhiteSpace::PreWrap => Space::Hanging,
                                _ => Space::Preserved,
                            },
                            Kind::Tab => {
                                // tab-size: 8 spaces (CSS Text 3 §4.2).
                                let space = shape(fonts, chain, &[' '], &options)
                                    .ok()
                                    .and_then(|c| c.first().map(|c| c.advance))
                                    .unwrap_or(size / 4.0);
                                Space::Tab(8.0 * (space + letter))
                            }
                            Kind::Forced => Space::Forced,
                        };
                        let (glyphs, width) = match space {
                            Space::Forced | Space::Tab(_) => (Vec::new(), 0.0),
                            _ => (cluster.glyphs, cluster.advance),
                        };
                        ifc.pieces.push(Piece::Cluster {
                            justifiable: first == ' ' || is_word_separator(first),
                            chars: range,
                            text: item,
                            face: cluster.face,
                            glyphs,
                            width,
                            space,
                        });
                    }
                    start = end;
                }
            }
        }
    }
    // Record each edge's character position (the next character's index).
    let mut next = ifc.chars.len();
    for piece in ifc.pieces.iter_mut().rev() {
        match piece {
            Piece::Cluster { chars, .. } => next = chars.start,
            Piece::Edge { at, .. } => *at = next,
        }
    }
    Ok(())
}

/// The nearest common inline ancestor's `white-space` decides soft wraps
/// between two characters (CSS Text 3 §5.1).
fn wrap_style(ifc: &Ifc<'_>, a: usize, b: usize) -> Result<WhiteSpace, FlowError> {
    let (ta, tb) = (ifc.sources[a].text, ifc.sources[b].text);
    if ta == tb {
        return white_space(ifc.texts[ta].style);
    }
    let mut x = ifc.texts[ta].parent;
    let mut y = ifc.texts[tb].parent;
    let depth = |v: Option<usize>| v.map_or(0, |i| ifc.boxes[i].depth + 1);
    while depth(x) > depth(y) {
        x = x.and_then(|i| ifc.boxes[i].parent);
    }
    while depth(y) > depth(x) {
        y = y.and_then(|i| ifc.boxes[i].parent);
    }
    while x != y {
        x = x.and_then(|i| ifc.boxes[i].parent);
        y = y.and_then(|i| ifc.boxes[i].parent);
    }
    white_space(x.map_or(ifc.root, |i| ifc.boxes[i].style))
}

fn break_opportunities(
    ifc: &Ifc<'_>,
    work: &mut Work<'_>,
) -> Result<Vec<Option<BreakOpportunity>>, FlowError> {
    work.charge(ifc.chars.len())?;
    let uax14 = line_breaks(&ifc.chars);
    let at_char = |e: usize| -> Result<Option<BreakOpportunity>, FlowError> {
        if e == 0 || e >= ifc.chars.len() {
            return Ok(None);
        }
        Ok(match uax14[e] {
            BreakOpportunity::Mandatory => Some(BreakOpportunity::Mandatory),
            o => {
                let ws = wrap_style(ifc, e - 1, e)?;
                let spaces = ws == WhiteSpace::BreakSpaces && ifc.chars[e - 1] == ' ';
                (wraps(ws) && (o == BreakOpportunity::Allowed || spaces))
                    .then_some(BreakOpportunity::Allowed)
            }
        })
    };
    let n = ifc.pieces.len();
    let mut breaks = vec![None; n];
    for (k, slot) in breaks.iter_mut().enumerate() {
        let next_close = matches!(ifc.pieces.get(k + 1), Some(Piece::Edge { open: false, .. }));
        *slot = match &ifc.pieces[k] {
            Piece::Cluster { chars, space, .. } => {
                if next_close {
                    None
                } else if *space == Space::Forced {
                    Some(BreakOpportunity::Mandatory)
                } else {
                    at_char(chars.end)?
                }
            }
            Piece::Edge {
                open: false, at, ..
            } if !next_close => at_char(*at)?,
            Piece::Edge { .. } => None,
        };
    }
    Ok(breaks)
}
