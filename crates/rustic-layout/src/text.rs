//! Public line, text and inline-box fragments (REN-03b).

use std::collections::HashMap;
use std::ops::Range;

use rustic_dom::NodeId;
use rustic_text::GlyphId;

use crate::flow::{FlowError, Work};
use crate::inline::Ifc;
use crate::lines::LineSet;
use crate::{CssPoint, CssPx, CssRect, CssSize};

/// One line box (CSS 2.2 §9.4.2): the band between floats it was laid out
/// in, and its height from the inline boxes on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineFragment {
    pub(crate) block: usize,
    pub(crate) rect: CssRect,
    pub(crate) baseline: CssPx,
    pub(crate) rotation: GlyphRotation,
}

impl LineFragment {
    /// The block fragment whose content this line belongs to.
    pub const fn block(self) -> usize {
        self.block
    }
    /// The line box: available band horizontally, line height vertically.
    pub const fn rect(self) -> CssRect {
        self.rect
    }
    /// The root inline box's baseline: absolute y, or absolute x on a
    /// vertical line.
    pub const fn baseline(self) -> CssPx {
        self.baseline
    }
    /// How text on this line is turned ([`GlyphRotation::None`] on a
    /// horizontal line).
    pub const fn rotation(self) -> GlyphRotation {
        self.rotation
    }
}

/// One positioned glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PositionedGlyph {
    /// Glyph index in [`TextFragment::face`].
    pub glyph: GlyphId,
    /// Pen position (absolute x of the glyph origin) in CSS pixels.
    pub x: f64,
    /// Absolute glyph origin y, including OpenType mark/cursive offsets.
    pub y: f64,
}

/// A run of glyphs from one text box, face and bidi level on one line.
#[derive(Clone, Debug, PartialEq)]
pub struct TextFragment {
    pub(crate) line: usize,
    pub(crate) node: NodeId,
    pub(crate) source_box: usize,
    pub(crate) text: Range<usize>,
    pub(crate) rect: CssRect,
    pub(crate) baseline: CssPx,
    pub(crate) level: u8,
    pub(crate) face: usize,
    pub(crate) font_size: f64,
    pub(crate) glyphs: Vec<PositionedGlyph>,
    pub(crate) parent: Option<usize>,
    pub(crate) rotation: GlyphRotation,
    /// Upright on a vertical line: [`GlyphRotation::None`] once the run is
    /// turned back to physical coordinates.
    pub(crate) upright: bool,
}

/// How a text fragment's glyphs are turned (CSS Writing Modes 4 §5.1):
/// horizontal text, or sideways in a vertical writing mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GlyphRotation {
    /// Upright on a horizontal line.
    #[default]
    None,
    /// Turned 90° clockwise: the line runs top to bottom with glyph tops
    /// to the right (`vertical-rl`, `vertical-lr`, `sideways-rl`).
    Clockwise,
    /// Turned 90° counterclockwise: the line runs bottom to top with glyph
    /// tops to the left (`sideways-lr`).
    Counterclockwise,
}

impl TextFragment {
    /// Index into [`crate::BlockLayout::lines`].
    pub const fn line(&self) -> usize {
        self.line
    }
    /// Source text node (`<br>`/`<wbr>` element for their break characters).
    pub const fn node(&self) -> NodeId {
        self.node
    }
    /// Principal box index of the text (whose style paints it).
    pub const fn source_box(&self) -> usize {
        self.source_box
    }
    /// UTF-16 range in the source text box.
    pub fn text(&self) -> Range<usize> {
        self.text.clone()
    }
    /// Content area: advance width by the font's ascent plus descent.
    pub const fn rect(&self) -> CssRect {
        self.rect
    }
    /// Absolute baseline y.
    pub const fn baseline(&self) -> CssPx {
        self.baseline
    }
    /// Resolved bidi level (odd is right to left).
    pub const fn bidi_level(&self) -> u8 {
        self.level
    }
    /// Face index in the [`rustic_text::FontDatabase`] used for layout.
    pub const fn face(&self) -> usize {
        self.face
    }
    /// Font size in CSS pixels.
    pub const fn font_size(&self) -> f64 {
        self.font_size
    }
    /// Glyphs in line order (left to right on a horizontal line).
    pub fn glyphs(&self) -> &[PositionedGlyph] {
        &self.glyphs
    }
    /// How the glyphs are turned about their origins.
    pub const fn rotation(&self) -> GlyphRotation {
        self.rotation
    }
    /// Whether the glyphs stand upright on a vertical (`vertical-rl` or
    /// `vertical-lr`) line: their rotation is [`GlyphRotation::None`] while
    /// the line runs top to bottom, and each glyph origin is its
    /// horizontal origin, centered on the line.
    pub const fn upright(&self) -> bool {
        self.upright
    }
    /// The enclosing inline box fragment on the same line (an index into
    /// [`crate::BlockLayout::inlines`]); `None` when the text sits directly
    /// in the line's root inline box.
    pub const fn parent(&self) -> Option<usize> {
        self.parent
    }
}

/// One `display:inline` box's fragment on one line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InlineFragment {
    pub(crate) line: usize,
    pub(crate) node: NodeId,
    pub(crate) source_box: usize,
    pub(crate) border: CssRect,
    pub(crate) padding: CssRect,
    pub(crate) content: CssRect,
    pub(crate) starts: bool,
    pub(crate) ends: bool,
    pub(crate) parent: Option<usize>,
}

impl InlineFragment {
    /// Index into [`crate::BlockLayout::lines`].
    pub const fn line(self) -> usize {
        self.line
    }
    /// Source element.
    pub const fn node(self) -> NodeId {
        self.node
    }
    /// Principal box index.
    pub const fn source_box(self) -> usize {
        self.source_box
    }
    /// Border box; vertically the content area plus padding and borders
    /// (which do not affect the line height).
    pub const fn border(self) -> CssRect {
        self.border
    }
    /// Padding box.
    pub const fn padding(self) -> CssRect {
        self.padding
    }
    /// Content area (the first available font's ascent plus descent).
    pub const fn content(self) -> CssRect {
        self.content
    }
    /// Whether the box's inline-start edge is on this fragment.
    pub const fn starts(self) -> bool {
        self.starts
    }
    /// Whether the box's inline-end edge is on this fragment.
    pub const fn ends(self) -> bool {
        self.ends
    }
    /// The enclosing inline box fragment on the same line, if any.
    pub const fn parent(self) -> Option<usize> {
        self.parent
    }
}

/// Global inline-fragment index of (line in the converted set, inline box).
pub(crate) type InlineIndex = HashMap<(usize, usize), usize>;

fn rect(x: f64, y: f64, w: f64, h: f64) -> Result<CssRect, FlowError> {
    Ok(CssRect::new(
        CssPoint {
            x: CssPx::new(x)?,
            y: CssPx::new(y)?,
        },
        CssSize::new(w.max(0.0), h.max(0.0))?,
    )?)
}

/// Move one context's lines (relative to its content origin) to absolute
/// coordinates and append them.
#[allow(clippy::too_many_arguments, reason = "three output lists plus context")]
pub(crate) fn convert(
    ifc: &Ifc<'_>,
    set: LineSet,
    origin: (f64, f64),
    block: usize,
    lines: &mut Vec<LineFragment>,
    texts: &mut Vec<TextFragment>,
    inlines: &mut Vec<InlineFragment>,
    work: &mut Work<'_>,
) -> Result<InlineIndex, FlowError> {
    let base = lines.len();
    // Inline fragments first, so text can name its enclosing box; a box's
    // parent precedes it on each line (boxes are in preorder).
    let mut index = InlineIndex::new();
    for f in &set.inlines {
        index.insert((f.line, f.ibox), inlines.len() + index.len());
    }
    let (ox, oy) = origin;
    for l in &set.lines {
        work.charge(1)?;
        lines.push(LineFragment {
            block,
            rect: rect(l.x + ox, l.y + oy, l.width, l.height)?,
            baseline: CssPx::new(l.baseline + oy)?,
            rotation: GlyphRotation::None,
        });
    }
    for f in set.inlines {
        work.charge(1)?;
        let b = &ifc.boxes[f.ibox];
        let r = |(x, y, w, h): (f64, f64, f64, f64)| rect(x + ox, y + oy, w, h);
        let parent = b.parent.and_then(|p| index.get(&(f.line, p)).copied());
        inlines.push(InlineFragment {
            line: base + f.line,
            node: b.node,
            source_box: b.source,
            border: r(f.border)?,
            padding: r(f.padding)?,
            content: r(f.content)?,
            starts: f.starts,
            ends: f.ends,
            parent,
        });
    }
    for t in set.texts {
        work.charge(t.glyphs.len())?;
        // Forced breaks and invisible break hints draw nothing.
        if t.glyphs.is_empty() {
            continue;
        }
        let item = &ifc.texts[t.text];
        let mut glyphs = Vec::with_capacity(t.glyphs.len());
        for (glyph, x, y) in t.glyphs {
            CssPx::new(x + ox)?;
            CssPx::new(y + oy)?;
            glyphs.push(PositionedGlyph {
                glyph,
                x: x + ox,
                y: y + oy,
            });
        }
        texts.push(TextFragment {
            line: base + t.line,
            node: item.node,
            source_box: item.source,
            text: t.utf16.0..t.utf16.1,
            rect: rect(t.x + ox, t.top + oy, t.width, t.height)?,
            baseline: CssPx::new(t.baseline + oy)?,
            level: t.level,
            face: t.face,
            font_size: t.size,
            glyphs,
            parent: item.parent.and_then(|p| index.get(&(t.line, p)).copied()),
            rotation: GlyphRotation::None,
            upright: t.upright,
        });
    }
    Ok(index)
}
