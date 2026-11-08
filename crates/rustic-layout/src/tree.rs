//! Bounded, flat principal-box snapshots. No content-controlled recursion.

use rustic_base::CancellationToken;
use rustic_css::{Display, PropertyId, PropertyValue};
use rustic_dom::{Document, DomError, NodeId, NodeKind};
use rustic_style::{ComputedStyle, ElementStyle};
use std::{collections::HashMap, error::Error, fmt};

/// Independent work, box and depth limits. These are not total process memory.
#[derive(Clone, Copy, Debug)]
pub struct LayoutLimits {
    /// Maximum visited DOM nodes and indexed element styles.
    pub max_nodes: usize,
    /// Maximum retained principal/text boxes.
    pub max_boxes: usize,
    /// Maximum copied UTF-16 code units across all text boxes.
    pub max_text_units: usize,
    /// Maximum DOM edges from the selected root, including boxless elements.
    pub max_depth: usize,
}
impl Default for LayoutLimits {
    fn default() -> Self {
        Self {
            max_nodes: 100_000,
            max_boxes: 100_000,
            max_text_units: 8 * 1024 * 1024,
            max_depth: 256,
        }
    }
}

/// A principal element box or an unshaped UTF-16 text run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoxKind {
    /// The computed display value. Formatting-context normalization is later work.
    Element(Display),
    /// Text with no whitespace processing, glyph selection or line layout yet.
    Text,
}

/// One immutable box in a [`BoxTree`], linked by snapshot-local indexes.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxSnapshot {
    node: NodeId,
    parent: Option<usize>,
    kind: BoxKind,
    style: Option<ComputedStyle>,
    text: Vec<u16>,
}
impl BoxSnapshot {
    /// Source node identity, validated while the snapshot was constructed.
    pub const fn node(&self) -> NodeId {
        self.node
    }
    /// Parent index in the same snapshot, or `None` for a top-level box.
    pub const fn parent(&self) -> Option<usize> {
        self.parent
    }
    /// Principal box kind.
    pub const fn kind(&self) -> BoxKind {
        self.kind
    }
    /// Immutable M2 style, or the text run's inherited DOM-parent style.
    pub fn style(&self) -> Option<&ComputedStyle> {
        self.style.as_ref()
    }
    /// Original UTF-16 text, including lone surrogates; empty for elements.
    pub fn text(&self) -> &[u16] {
        &self.text
    }
}

/// A flat, preorder box snapshot, distinct from the owning DOM.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxTree {
    boxes: Vec<BoxSnapshot>,
}
impl BoxTree {
    /// Construct principal boxes from an existing computed-style snapshot.
    ///
    /// `display:none` suppresses a subtree, `display:contents` promotes its
    /// children, and templates are inert. Text is copied as UTF-16. Styles must
    /// be unique, live element IDs in this document; each visible element needs
    /// one. No selectors or cascade run here. Visible shadow hosts fail until
    /// flattened-tree/slot assignment exists (TD-37). Anonymous boxes, pseudo
    /// boxes and formatting-context fixups are not yet constructed.
    pub fn build(
        document: &Document,
        root: NodeId,
        styles: &[ElementStyle],
        limits: LayoutLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, LayoutError> {
        cancellation.check().map_err(|_| LayoutError::Cancelled)?;
        document.node(root)?;
        if styles.len() > limits.max_nodes {
            return Err(LayoutError::NodeLimit);
        }
        let mut index = HashMap::new();
        for style in styles {
            cancellation.check().map_err(|_| LayoutError::Cancelled)?;
            if !matches!(document.node(style.node)?.kind(), NodeKind::Element(_))
                || index.insert(style.node, &style.computed).is_some()
            {
                return Err(LayoutError::InvalidStyles);
            }
        }
        let mut boxes = Vec::new();
        let mut parents: Vec<(usize, Option<usize>, Option<&ComputedStyle>)> = Vec::new();
        let mut hidden = None;
        let mut text_units = 0usize;
        for (visited, (id, depth)) in document.walk(root)?.enumerate() {
            cancellation.check().map_err(|_| LayoutError::Cancelled)?;
            if visited >= limits.max_nodes {
                return Err(LayoutError::NodeLimit);
            }
            if depth > limits.max_depth {
                return Err(LayoutError::DepthLimit);
            }
            if hidden.is_some_and(|d| depth > d) {
                continue;
            }
            hidden = None;
            while parents.last().is_some_and(|&(d, _, _)| d >= depth) {
                parents.pop();
            }
            let parent = parents.last().and_then(|&(_, parent, _)| parent);
            let inherited = parents.last().and_then(|&(_, _, style)| style);
            let node = document.node(id)?;
            let (kind, style, text) = match node.kind() {
                NodeKind::Element(element) => {
                    let style = *index.get(&id).ok_or(LayoutError::MissingStyle(id))?;
                    let PropertyValue::Display(display) = style.get(PropertyId::Display) else {
                        return Err(LayoutError::InvalidStyles);
                    };
                    if *display == Display::None {
                        hidden = Some(depth);
                        continue;
                    }
                    if element.shadow_root().is_some() {
                        return Err(LayoutError::ShadowTree);
                    }
                    if *display == Display::Contents {
                        parents.push((depth, parent, Some(style)));
                        continue;
                    }
                    if boxes.len() >= limits.max_boxes {
                        return Err(LayoutError::BoxLimit);
                    }
                    (BoxKind::Element(*display), Some(style.clone()), Vec::new())
                }
                NodeKind::Text(data) | NodeKind::CdataSection(data)
                    if !data.as_units().is_empty() =>
                {
                    if boxes.len() >= limits.max_boxes {
                        return Err(LayoutError::BoxLimit);
                    }
                    text_units = text_units
                        .checked_add(data.as_units().len())
                        .filter(|&n| n <= limits.max_text_units)
                        .ok_or(LayoutError::TextLimit)?;
                    (
                        BoxKind::Text,
                        Some(inherited.cloned().unwrap_or_else(ComputedStyle::initial)),
                        data.as_units().to_vec(),
                    )
                }
                _ => continue,
            };
            let current = boxes.len();
            boxes.push(BoxSnapshot {
                node: id,
                parent,
                kind,
                style,
                text,
            });
            parents.push((depth, Some(current), index.get(&id).copied().or(inherited)));
        }
        Ok(Self { boxes })
    }

    /// Principal/text boxes in preorder. Indexes are valid only in this snapshot.
    pub fn boxes(&self) -> &[BoxSnapshot] {
        &self.boxes
    }

    /// Deterministic, linear-size structural dump within a byte cap. No document
    /// IDs, page text, content-derived names or depth-dependent indentation.
    pub fn dump(&self, max_bytes: usize) -> Result<String, LayoutError> {
        let mut out = String::new();
        for (i, b) in self.boxes.iter().enumerate() {
            let parent = b.parent.map_or_else(|| "-".into(), |p| p.to_string());
            let kind = match b.kind {
                BoxKind::Element(display) => format!("display={display}"),
                BoxKind::Text => format!("text-units={}", b.text.len()),
            };
            let line = format!(
                "box {i} parent={parent} node={}:{} {kind}\n",
                b.node.slot(),
                b.node.generation()
            );
            if line.len() > max_bytes.saturating_sub(out.len()) {
                return Err(LayoutError::OutputLimit);
            }
            out.push_str(&line);
        }
        Ok(out)
    }
}

/// Snapshot construction/dump failure. The source DOM and style slice are unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutError {
    /// Invalid, stale or foreign node identity.
    Node(DomError),
    /// Visited-node or style-index budget exceeded.
    NodeLimit,
    /// Box budget exceeded.
    BoxLimit,
    /// Copied UTF-16 budget exceeded.
    TextLimit,
    /// DOM depth budget exceeded.
    DepthLimit,
    /// Cooperative cancellation.
    Cancelled,
    /// Duplicate or non-element style entry.
    InvalidStyles,
    /// A visible element has no style in the supplied snapshot.
    MissingStyle(NodeId),
    /// Visible shadow content needs a flattened tree and slot assignment.
    ShadowTree,
    /// Dump would exceed its byte limit.
    OutputLimit,
}
impl From<DomError> for LayoutError {
    fn from(value: DomError) -> Self {
        Self::Node(value)
    }
}
impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Node(error) => error.fmt(f),
            other => write!(f, "box snapshot failed: {other:?}"),
        }
    }
}
impl Error for LayoutError {}
