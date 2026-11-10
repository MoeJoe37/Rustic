//! Bounded, flat principal-box snapshots. No content-controlled recursion.

use rustic_base::CancellationToken;
use rustic_css::{Display, PropertyId, PropertyValue};
use rustic_dom::{Document, DomError, NodeId, NodeKind};
use rustic_style::{ComputedStyle, ElementStyle};
use std::{collections::HashMap, error::Error, fmt, sync::Arc};

use crate::CssPx;
use crate::replaced::{NaturalSize, canvas_size};

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
    /// The computed display value, before separate formatting normalization.
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
    // Shared, not copied: a text run reuses its DOM parent's style, so many
    // text boxes cost one style rather than one multi-kilobyte copy each.
    style: Option<Arc<ComputedStyle>>,
    text: Vec<u16>,
    special_layout: bool,
    html_tag: HtmlTag,
    natural: Option<NaturalSize>,
    language: Option<Arc<str>>,
}

/// HTML elements whose `overflow` can propagate to the viewport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HtmlTag {
    /// An HTML `html` element that is the document element.
    DocumentElement,
    /// An HTML `body` element.
    Body,
    /// An HTML `br` element (a forced line break inside inline content).
    Br,
    /// An HTML `wbr` element (a line break opportunity).
    Wbr,
    /// Anything else, including text.
    Other,
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
        self.style.as_deref()
    }
    /// Original UTF-16 text, including lone surrogates; empty for elements.
    pub fn text(&self) -> &[u16] {
        &self.text
    }

    /// Normalized inherited HTML/XML language, if explicitly known.
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// For a replaced element (`img`, `canvas`, `video`, `iframe`, `embed`,
    /// `object`), its natural size: `img` starts at 0×0 until the embedder
    /// supplies a decoded size, `canvas` uses its attributes and the others
    /// have none (300×150 by default).
    pub fn natural_size(&self) -> Option<NaturalSize> {
        self.natural
    }

    pub(crate) fn requires_special_layout(&self) -> bool {
        self.special_layout
    }

    pub(crate) fn html_tag(&self) -> HtmlTag {
        self.html_tag
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
    /// flattened-tree/slot assignment exists (TD-37). Anonymous flow boxes are
    /// derived separately by `FormattingTree`; pseudo boxes and other
    /// formatting-context fixups remain pending.
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
        let mut parents: Vec<(usize, Option<usize>, Option<Arc<ComputedStyle>>)> = Vec::new();
        let initial = Arc::new(ComputedStyle::initial());
        let mut hidden = None;
        let mut text_units = 0usize;
        let mut languages: Vec<(usize, Option<Arc<str>>)> = Vec::new();
        let mut initial_language = None;
        let mut ancestors = 0usize;
        let mut ancestor = document.node(root)?.parent();
        while let Some(id) = ancestor {
            cancellation.check().map_err(|_| LayoutError::Cancelled)?;
            ancestors = ancestors
                .checked_add(1)
                .filter(|&n| n < limits.max_nodes)
                .ok_or(LayoutError::NodeLimit)?;
            let node = document.node(id)?;
            if let Some(lang) = explicit_language(node.kind()) {
                initial_language = lang;
                break;
            }
            ancestor = node.parent();
        }
        for (visited, (id, depth)) in document.walk(root)?.enumerate() {
            cancellation.check().map_err(|_| LayoutError::Cancelled)?;
            if visited >= limits.max_nodes.saturating_sub(ancestors) {
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
            let inherited = parents.last().and_then(|(_, _, style)| style.clone());
            let node = document.node(id)?;
            while languages.last().is_some_and(|(d, _)| *d >= depth) {
                languages.pop();
            }
            let language = explicit_language(node.kind()).unwrap_or_else(|| {
                languages
                    .last()
                    .map_or_else(|| initial_language.clone(), |(_, l)| l.clone())
            });
            languages.push((depth, language.clone()));
            // A computed `display:block` cannot turn a replaced element, a
            // native control or foreign content into a generic CSS block.
            let special_layout = match node.kind() {
                NodeKind::Element(element) => {
                    element.name().namespace() != Some(rustic_dom::HTML_NAMESPACE)
                        || matches!(
                            element.name().local().to_ascii_lowercase().as_str(),
                            "img"
                                | "video"
                                | "audio"
                                | "iframe"
                                | "embed"
                                | "object"
                                | "canvas"
                                | "input"
                                | "textarea"
                                | "select"
                                | "button"
                                | "meter"
                                | "progress"
                                | "br"
                                | "wbr"
                                | "fieldset"
                                | "legend"
                                | "details"
                                | "summary"
                                | "dialog"
                                | "slot"
                        )
                }
                _ => false,
            };
            let natural = match node.kind() {
                NodeKind::Element(element)
                    if element.name().namespace() == Some(rustic_dom::HTML_NAMESPACE) =>
                {
                    let attribute = |name: &str| {
                        element
                            .attributes()
                            .iter()
                            .find(|a| {
                                a.name().namespace().is_none()
                                    && a.name().local().eq_ignore_ascii_case(name)
                            })
                            .map(|a| a.value().as_units())
                    };
                    match element.name().local().to_ascii_lowercase().as_str() {
                        "img" => Some(NaturalSize::from_dimensions(CssPx::ZERO, CssPx::ZERO)),
                        "canvas" => Some(canvas_size(attribute("width"), attribute("height"))),
                        "video" | "iframe" | "embed" | "object" => Some(NaturalSize::NONE),
                        _ => None,
                    }
                }
                _ => None,
            };
            let html_tag = match node.kind() {
                NodeKind::Element(element)
                    if element.name().namespace() == Some(rustic_dom::HTML_NAMESPACE) =>
                {
                    let local = element.name().local();
                    let document_parent = node.parent().is_some_and(|p| {
                        document
                            .node(p)
                            .is_ok_and(|p| matches!(p.kind(), NodeKind::Document))
                    });
                    if local.eq_ignore_ascii_case("html") && document_parent {
                        HtmlTag::DocumentElement
                    } else if local.eq_ignore_ascii_case("body") {
                        HtmlTag::Body
                    } else if local.eq_ignore_ascii_case("br") {
                        HtmlTag::Br
                    } else if local.eq_ignore_ascii_case("wbr") {
                        HtmlTag::Wbr
                    } else {
                        HtmlTag::Other
                    }
                }
                _ => HtmlTag::Other,
            };
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
                        parents.push((depth, parent, Some(Arc::new(style.clone()))));
                        continue;
                    }
                    if boxes.len() >= limits.max_boxes {
                        return Err(LayoutError::BoxLimit);
                    }
                    (
                        BoxKind::Element(*display),
                        Arc::new(style.clone()),
                        Vec::new(),
                    )
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
                        inherited.clone().unwrap_or_else(|| Arc::clone(&initial)),
                        data.as_units().to_vec(),
                    )
                }
                _ => continue,
            };
            let current = boxes.len();
            // Descendant text inherits an element's own style; a text node has
            // no children, so its entry only closes the stack level.
            let shared = match kind {
                BoxKind::Element(_) => Some(Arc::clone(&style)),
                BoxKind::Text => inherited,
            };
            boxes.push(BoxSnapshot {
                node: id,
                language,
                parent,
                kind,
                style: Some(style),
                text,
                special_layout,
                html_tag,
                natural: natural.filter(|_| matches!(kind, BoxKind::Element(_))),
            });
            parents.push((depth, Some(current), shared));
        }
        Ok(Self { boxes })
    }

    /// Supply natural sizes for replaced elements, typically decoded image
    /// dimensions. Every node must be a replaced box in this snapshot; on an
    /// error nothing is changed.
    pub fn set_natural_sizes(
        &mut self,
        sizes: &[(NodeId, NaturalSize)],
    ) -> Result<(), LayoutError> {
        let wanted: HashMap<NodeId, NaturalSize> = sizes.iter().copied().collect();
        let mut found = 0usize;
        for b in &self.boxes {
            if b.natural.is_some() && wanted.contains_key(&b.node) {
                found += 1;
            }
        }
        if found != wanted.len() {
            let missing = sizes
                .iter()
                .find(|(node, _)| {
                    !self
                        .boxes
                        .iter()
                        .any(|b| b.node == *node && b.natural.is_some())
                })
                .map_or(sizes[0].0, |(node, _)| *node);
            return Err(LayoutError::NotReplaced(missing));
        }
        for b in &mut self.boxes {
            if let (Some(size), Some(_)) = (wanted.get(&b.node), b.natural) {
                b.natural = Some(*size);
            }
        }
        Ok(())
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
    /// A natural size was supplied for a node that is not a replaced box.
    NotReplaced(NodeId),
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

fn explicit_language(kind: &NodeKind) -> Option<Option<Arc<str>>> {
    let NodeKind::Element(element) = kind else {
        return None;
    };
    let attribute = element
        .attributes()
        .iter()
        .find(|a| a.name().local().eq_ignore_ascii_case("lang") && a.name().namespace().is_none())
        .or_else(|| {
            element.attributes().iter().find(|a| {
                a.name().local() == "lang"
                    && a.name().namespace() == Some("http://www.w3.org/XML/1998/namespace")
            })
        })?;
    let units = attribute.value().as_units();
    if units.is_empty() || units.len() > 63 {
        return Some(None);
    }
    let s = String::from_utf16(units).ok()?;
    if !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Some(None);
    }
    Some(Some(Arc::from(s.to_ascii_lowercase())))
}
