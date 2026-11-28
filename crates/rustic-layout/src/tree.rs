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
#[non_exhaustive]
pub struct LayoutLimits {
    /// Maximum visited DOM nodes and indexed element styles.
    pub max_nodes: usize,
    /// Maximum retained principal/text boxes.
    pub max_boxes: usize,
    /// Maximum copied UTF-16 code units across all text boxes.
    pub max_text_units: usize,
    /// Maximum DOM edges from the selected root, including boxless elements.
    pub max_depth: usize,
    /// Units of flow work (box visits, float scans, containing-chain steps
    /// and formatting-root retries) one layout may spend before it fails with
    /// [`crate::FlowError::WorkLimit`]. Defaults to [`crate::MAX_FLOW_WORK`].
    pub max_flow_work: usize,
}
impl Default for LayoutLimits {
    fn default() -> Self {
        Self {
            max_nodes: 100_000,
            max_boxes: 100_000,
            max_text_units: 8 * 1024 * 1024,
            max_depth: 256,
            max_flow_work: crate::MAX_FLOW_WORK,
        }
    }
}
impl LayoutLimits {
    /// These limits with `max_nodes` replaced.
    #[must_use]
    pub const fn with_max_nodes(mut self, value: usize) -> Self {
        self.max_nodes = value;
        self
    }
    /// These limits with `max_boxes` replaced.
    #[must_use]
    pub const fn with_max_boxes(mut self, value: usize) -> Self {
        self.max_boxes = value;
        self
    }
    /// These limits with `max_text_units` replaced.
    #[must_use]
    pub const fn with_max_text_units(mut self, value: usize) -> Self {
        self.max_text_units = value;
        self
    }
    /// These limits with `max_depth` replaced.
    #[must_use]
    pub const fn with_max_depth(mut self, value: usize) -> Self {
        self.max_depth = value;
        self
    }
    /// These limits with `max_flow_work` replaced.
    #[must_use]
    pub const fn with_max_flow_work(mut self, value: usize) -> Self {
        self.max_flow_work = value;
        self
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
    /// HTML `colspan` and `rowspan` of a `td`/`th` (`rowspan` 0 spans the
    /// rest of the row group), or `span` of a `col`/`colgroup` as the first
    /// value; (1, 1) for every other box.
    span: (u32, u32),
    /// A generated `outside` list marker (CSS Lists 3 §3.3): hangs before
    /// the first line of its list item.
    outside_marker: bool,
    /// The anonymous block holding a multi-column container's content
    /// (`flow::multicol`).
    column_wrapper: bool,
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

    /// HTML table cell spans: (`colspan` 1..=1000, `rowspan` 0..=65534).
    pub(crate) const fn table_span(&self) -> (u32, u32) {
        self.span
    }
    /// A copy under `parent` with `style` and `kind` (writing-mode
    /// layout's transposed boxes). `plain` drops what makes a box special
    /// (replaced content, HTML element roles, text, spans).
    pub(crate) fn derived(
        &self,
        parent: Option<usize>,
        style: Option<Arc<ComputedStyle>>,
        kind: BoxKind,
        plain: bool,
    ) -> Self {
        let mut copy = self.clone();
        copy.parent = parent;
        copy.style = style;
        copy.kind = kind;
        if plain {
            copy.text = Vec::new();
            copy.special_layout = false;
            copy.html_tag = HtmlTag::Other;
            copy.natural = None;
            copy.span = (1, 1);
            copy.outside_marker = false;
            copy.column_wrapper = false;
        }
        copy
    }

    /// Whether this is the anonymous block holding a multi-column
    /// container's content.
    pub(crate) const fn is_column_wrapper(&self) -> bool {
        self.column_wrapper
    }

    /// Whether this text box is a generated `outside` list marker.
    pub(crate) const fn is_outside_marker(&self) -> bool {
        self.outside_marker
    }

    /// The shared style, for copies that keep it.
    pub(crate) fn shared_style(&self) -> Option<&Arc<ComputedStyle>> {
        self.style.as_ref()
    }
}

/// HTML "rules for parsing non-negative integers", saturating.
fn non_negative_integer(units: &[u16]) -> Option<u32> {
    let is_space = |u: u16| matches!(u, 0x09 | 0x0A | 0x0C | 0x0D | 0x20);
    let mut rest = units
        .iter()
        .copied()
        .skip_while(|&u| is_space(u))
        .peekable();
    if rest.peek() == Some(&u16::from(b'+')) {
        rest.next();
    }
    let mut value: Option<u32> = None;
    for u in rest {
        let Some(digit) = char::from_u32(u32::from(u)).and_then(|c| c.to_digit(10)) else {
            break;
        };
        value = Some(value.unwrap_or(0).saturating_mul(10).saturating_add(digit));
    }
    value
}

/// HTML table processing model: `colspan` is 1..=1000 (0 and errors are
/// 1) and `rowspan` 0..=65534 (errors are 1).
fn cell_spans(colspan: Option<&[u16]>, rowspan: Option<&[u16]>) -> (u32, u32) {
    let columns = colspan
        .and_then(non_negative_integer)
        .filter(|&n| n > 0)
        .map_or(1, |n| n.min(1000));
    let rows = rowspan
        .and_then(non_negative_integer)
        .map_or(1, |n| n.min(65_534));
    (columns, rows)
}

/// A form control's live state when it differs from what its content
/// attributes say: the current value of a text field (its dirty value, set
/// by script or typing) and the checkedness of a checkbox or radio button.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ControlState {
    /// The current value, for controls that show one.
    pub value: Option<Vec<u16>>,
    /// The checkedness, for checkboxes and radio buttons.
    pub checked: Option<bool>,
}

/// A flat, preorder box snapshot, distinct from the owning DOM.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxTree {
    boxes: Vec<BoxSnapshot>,
    /// Live checkedness of checkable inputs, for paint.
    checked: HashMap<NodeId, bool>,
}
impl BoxTree {
    /// Construct principal boxes from an existing computed-style snapshot.
    ///
    /// `display:none` suppresses a subtree, `display:contents` promotes its
    /// children, and templates are inert. Text is copied as UTF-16. Styles must
    /// be unique, live element IDs in this document; each visible element needs
    /// one. The flat tree determines the children of shadow hosts and slots.
    /// Anonymous flow boxes are
    /// derived separately by `FormattingTree`; pseudo boxes and other
    /// formatting-context fixups remain pending.
    pub fn build(
        document: &Document,
        root: NodeId,
        styles: &[ElementStyle],
        limits: LayoutLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, LayoutError> {
        Self::build_with_controls(
            document,
            root,
            styles,
            limits,
            &HashMap::new(),
            cancellation,
        )
    }

    /// [`BoxTree::build`] with the live state of form controls, which takes
    /// the place of their `value` and `checked` content attributes.
    pub fn build_with_controls(
        document: &Document,
        root: NodeId,
        styles: &[ElementStyle],
        limits: LayoutLimits,
        controls: &HashMap<NodeId, ControlState>,
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
        let mut ordinals = Ordinals::default();
        let mut rubies: Vec<Ruby> = Vec::new();
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
        let slots = document.slot_map()?;
        for (visited, (id, depth)) in slots.flat_walk(root)?.enumerate() {
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
            let mut parent = parents.last().and_then(|&(_, parent, _)| parent);
            let inherited = parents.last().and_then(|(_, _, style)| style.clone());
            while rubies.last().is_some_and(|r| r.depth >= depth) {
                rubies.pop();
            }
            let node = document.node(id)?;
            // A closed `details` renders only its first `summary` (HTML
            // §15.5.3: the rest is in a slot that is not rendered).
            if id != root && hidden_in_closed_details(document, id, node) {
                hidden = Some(depth);
                continue;
            }
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
                    // MathML lays out as CSS boxes from the UA sheet.
                    !matches!(
                        element.name().namespace(),
                        Some(rustic_dom::HTML_NAMESPACE | rustic_dom::MATHML_NAMESPACE)
                    ) || matches!(
                        element.name().local().to_ascii_lowercase().as_str(),
                        "img"
                            | "video"
                            | "audio"
                            | "iframe"
                            | "embed"
                            | "object"
                            | "canvas"
                            | "br"
                            | "wbr"
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
                // An outermost `svg` is a replaced element (SVG 2 §5.1.2).
                NodeKind::Element(element)
                    if element.name().namespace() == Some(rustic_dom::SVG_NAMESPACE)
                        && element.name().local() == "svg"
                        && !node.parent().is_some_and(|p| {
                            matches!(document.node(p).map(|n| n.kind()),
                                Ok(NodeKind::Element(e)) if e.name().namespace() == Some(rustic_dom::SVG_NAMESPACE))
                        }) =>
                {
                    Some(svg_natural_size(element))
                }
                _ => None,
            };
            let span = match node.kind() {
                NodeKind::Element(element)
                    if element.name().namespace() == Some(rustic_dom::HTML_NAMESPACE)
                        && (element.name().local().eq_ignore_ascii_case("td")
                            || element.name().local().eq_ignore_ascii_case("th")) =>
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
                    cell_spans(attribute("colspan"), attribute("rowspan"))
                }
                NodeKind::Element(element)
                    if element.name().namespace() == Some(rustic_dom::HTML_NAMESPACE)
                        && (element.name().local().eq_ignore_ascii_case("col")
                            || element.name().local().eq_ignore_ascii_case("colgroup")) =>
                {
                    let span = element
                        .attributes()
                        .iter()
                        .find(|a| {
                            a.name().namespace().is_none()
                                && a.name().local().eq_ignore_ascii_case("span")
                        })
                        .map(|a| a.value().as_units());
                    (cell_spans(span, None).0, 1)
                }
                _ => (1, 1),
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
                NodeKind::Element(_) => {
                    let style = *index.get(&id).ok_or(LayoutError::MissingStyle(id))?;
                    let PropertyValue::Display(display) = style.get(PropertyId::Display) else {
                        return Err(LayoutError::InvalidStyles);
                    };
                    if *display == Display::None {
                        hidden = Some(depth);
                        continue;
                    }
                    if *display == Display::Contents {
                        parents.push((depth, parent, Some(Arc::new(style.clone()))));
                        continue;
                    }
                    if boxes.len() >= limits.max_boxes {
                        return Err(LayoutError::BoxLimit);
                    }
                    // A list item is a block box with a marker (CSS Lists 3).
                    let display = match display {
                        Display::ListItem => Display::Block,
                        other => *other,
                    };
                    (
                        BoxKind::Element(display),
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
            // A ruby's children are grouped into base–annotation pairs,
            // each an anonymous inline flex column with the annotation on
            // top (CSS Ruby 1 §2, approximated).
            if let Some(ruby) = rubies
                .last_mut()
                .filter(|r| r.depth + 1 == depth && parent == Some(r.index))
            {
                let annotation = matches!(node.kind(), NodeKind::Element(e)
                    if e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE) && e.name().local() == "rt");
                // Whitespace between pairs is discarded.
                if kind == BoxKind::Text
                    && text
                        .iter()
                        .all(|&u| matches!(u, 0x09 | 0x0A | 0x0C | 0x0D | 0x20))
                {
                    continue;
                }
                if ruby.pair.is_none() || (!annotation && ruby.after_annotation) {
                    if boxes.len() >= limits.max_boxes {
                        return Err(LayoutError::BoxLimit);
                    }
                    ruby.pair = Some(boxes.len());
                    ruby.after_annotation = false;
                    boxes.push(BoxSnapshot {
                        node: ruby.node,
                        language: language.clone(),
                        parent: Some(ruby.index),
                        kind: BoxKind::Element(Display::InlineFlex),
                        style: Some(Arc::clone(&ruby.pair_style)),
                        text: Vec::new(),
                        special_layout: false,
                        html_tag: HtmlTag::Other,
                        natural: None,
                        span: (1, 1),
                        outside_marker: false,
                        column_wrapper: false,
                    });
                }
                ruby.after_annotation |= annotation;
                parent = ruby.pair;
            }
            let current = boxes.len();
            // Descendant text inherits an element's own style; a text node has
            // no children, so its entry only closes the stack level.
            let shared = match kind {
                BoxKind::Element(_) => Some(Arc::clone(&style)),
                BoxKind::Text => inherited,
            };
            let control = match node.kind() {
                NodeKind::Element(element) if matches!(kind, BoxKind::Element(_)) => control_text(
                    document,
                    id,
                    element,
                    controls.get(&id).and_then(|c| c.value.as_deref()),
                ),
                _ => None,
            };
            boxes.push(BoxSnapshot {
                node: id,
                language: language.clone(),
                parent,
                kind,
                style: Some(Arc::clone(&style)),
                text,
                special_layout,
                html_tag,
                natural: natural.filter(|_| matches!(kind, BoxKind::Element(_))),
                span,
                outside_marker: false,
                column_wrapper: false,
            });
            if matches!(node.kind(), NodeKind::Element(e)
                if e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE) && e.name().local() == "ruby")
                && kind == BoxKind::Element(Display::Inline)
            {
                let mut pair_style = (*style).clone();
                pair_style.set(
                    PropertyId::Display,
                    PropertyValue::Display(Display::InlineFlex),
                );
                pair_style.set(
                    PropertyId::FlexDirection,
                    PropertyValue::FlexDirection(rustic_css::FlexDirection::ColumnReverse),
                );
                pair_style.set(
                    PropertyId::AlignItems,
                    PropertyValue::Alignment(rustic_css::Alignment::new(
                        rustic_css::AlignKeyword::Center,
                    )),
                );
                rubies.push(Ruby {
                    depth,
                    index: current,
                    node: id,
                    pair: None,
                    after_annotation: false,
                    pair_style: Arc::new(pair_style),
                });
            }
            let item = matches!(
                (node.kind(), style.get(PropertyId::Display)),
                (
                    NodeKind::Element(_),
                    PropertyValue::Display(Display::ListItem)
                )
            );
            if item {
                let ordinal = ordinals.next(document, id);
                if let Some((units, outside)) = marker_text(&style, ordinal) {
                    if boxes.len() >= limits.max_boxes {
                        return Err(LayoutError::BoxLimit);
                    }
                    text_units = text_units
                        .checked_add(units.len())
                        .filter(|&n| n <= limits.max_text_units)
                        .ok_or(LayoutError::TextLimit)?;
                    // The marker keeps its trailing space.
                    let mut marker = (*style).clone();
                    marker.set(
                        PropertyId::WhiteSpace,
                        PropertyValue::WhiteSpace(rustic_css::WhiteSpace::Pre),
                    );
                    boxes.push(BoxSnapshot {
                        node: id,
                        language: language.clone(),
                        parent: Some(current),
                        kind: BoxKind::Text,
                        style: Some(Arc::new(marker)),
                        text: units,
                        special_layout: false,
                        html_tag: HtmlTag::Other,
                        natural: None,
                        span: (1, 1),
                        outside_marker: outside,
                        column_wrapper: false,
                    });
                }
            }
            if natural.is_some()
                && matches!(node.kind(), NodeKind::Element(e) if e.name().namespace() == Some(rustic_dom::SVG_NAMESPACE))
            {
                // Paint draws the SVG content; it makes no CSS boxes.
                hidden = Some(depth);
                continue;
            }
            if let (NodeKind::Element(e), BoxKind::Element(_)) = (node.kind(), kind) {
                if e.name().namespace() == Some(rustic_dom::MATHML_NAMESPACE)
                    && matches!(e.name().local(), "msqrt" | "mroot")
                {
                    // The radical sign hangs before the radicand.
                    if boxes.len() >= limits.max_boxes {
                        return Err(LayoutError::BoxLimit);
                    }
                    boxes.push(BoxSnapshot {
                        node: id,
                        language: language.clone(),
                        parent: Some(current),
                        kind: BoxKind::Text,
                        style: Some(Arc::clone(&style)),
                        text: vec![0x221A],
                        special_layout: false,
                        html_tag: HtmlTag::Other,
                        natural: None,
                        span: (1, 1),
                        outside_marker: true,
                        column_wrapper: false,
                    });
                }
            }
            if let Some(control) = control {
                // A control's rendering replaces its children: the value,
                // label or selected option becomes one text run of its own.
                hidden = Some(depth);
                if let Some((units, placeholder)) = control {
                    if boxes.len() >= limits.max_boxes {
                        return Err(LayoutError::BoxLimit);
                    }
                    text_units = text_units
                        .checked_add(units.len())
                        .filter(|&n| n <= limits.max_text_units)
                        .ok_or(LayoutError::TextLimit)?;
                    let text_style = if placeholder {
                        let mut muted = (*style).clone();
                        muted.set(
                            PropertyId::Color,
                            PropertyValue::Color(rustic_css::Color::rgb(117, 117, 117)),
                        );
                        Arc::new(muted)
                    } else {
                        Arc::clone(&style)
                    };
                    boxes.push(BoxSnapshot {
                        node: id,
                        language,
                        parent: Some(current),
                        kind: BoxKind::Text,
                        style: Some(text_style),
                        text: units,
                        special_layout: false,
                        html_tag: HtmlTag::Other,
                        natural: None,
                        span: (1, 1),
                        outside_marker: false,
                        column_wrapper: false,
                    });
                }
                continue;
            }
            if let BoxKind::Element(Display::Block | Display::FlowRoot | Display::InlineBlock) =
                kind
            {
                if is_multicol(&style) {
                    if boxes.len() >= limits.max_boxes {
                        return Err(LayoutError::BoxLimit);
                    }
                    let wrapper = boxes.len();
                    boxes.push(BoxSnapshot {
                        node: id,
                        language,
                        parent: Some(current),
                        kind: BoxKind::Element(Display::Block),
                        style: Some(Arc::new(style.anonymous_flow(Display::Block))),
                        text: Vec::new(),
                        special_layout: false,
                        html_tag: HtmlTag::Other,
                        natural: None,
                        span: (1, 1),
                        outside_marker: false,
                        column_wrapper: true,
                    });
                    parents.push((depth, Some(wrapper), shared));
                    continue;
                }
            }
            parents.push((depth, Some(current), shared));
        }
        let checked = controls
            .iter()
            .filter_map(|(&node, state)| Some((node, state.checked?)))
            .collect();
        Ok(Self { boxes, checked })
    }

    /// The live checkedness of a checkable input, when it was supplied to
    /// [`BoxTree::build_with_controls`].
    pub fn checked(&self, node: NodeId) -> Option<bool> {
        self.checked.get(&node).copied()
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

    /// A tree of already ordered boxes (writing-mode layout's copies).
    pub(crate) fn from_boxes(boxes: Vec<BoxSnapshot>) -> Self {
        Self {
            boxes,
            checked: HashMap::new(),
        }
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

/// Whether `node` is a child of a closed HTML `details` other than its
/// first `summary` child.
fn hidden_in_closed_details(document: &Document, id: NodeId, node: &rustic_dom::Node) -> bool {
    let html = |n: &rustic_dom::Node, local: &str| {
        matches!(n.kind(), NodeKind::Element(e)
            if e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE) && e.name().local() == local)
    };
    let Some(parent) = node.parent() else {
        return false;
    };
    let Ok(details) = document.node(parent) else {
        return false;
    };
    let closed = matches!(details.kind(), NodeKind::Element(e)
        if e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE)
            && e.name().local() == "details"
            && !e.attributes().iter().any(|a| a.name().namespace().is_none() && a.name().local() == "open"));
    if !closed {
        return false;
    }
    let first_summary = document.children(parent).ok().and_then(|mut children| {
        children.find(|&child| document.node(child).is_ok_and(|n| html(n, "summary")))
    });
    first_summary != Some(id)
}

/// Whether `style` makes a block container a multi-column container.
fn is_multicol(style: &ComputedStyle) -> bool {
    let count = !matches!(
        style.get(PropertyId::ColumnCount),
        PropertyValue::ZIndex(rustic_css::ZIndex::Auto)
    );
    let width = !matches!(
        style.get(PropertyId::ColumnWidth),
        PropertyValue::LengthPercentageOrAuto(rustic_css::LengthPercentageOrAuto::Auto)
    );
    count || width
}

/// An open HTML `ruby` element whose children are being paired.
struct Ruby {
    depth: usize,
    index: usize,
    node: NodeId,
    /// The current pair's box.
    pair: Option<usize>,
    /// Whether the current pair has its annotation already.
    after_annotation: bool,
    pair_style: Arc<ComputedStyle>,
}

/// The ordinal values of list items (HTML §4.4.8, `ol` `start`,
/// `reversed` and `li` `value`), counted per parent in tree order.
#[derive(Default)]
struct Ordinals {
    /// Parent → (its next ordinal, the step).
    next: HashMap<NodeId, (i64, i64)>,
}

impl Ordinals {
    fn next(&mut self, document: &Document, item: NodeId) -> i64 {
        let Some(parent) = document.node(item).ok().and_then(|n| n.parent()) else {
            return 1;
        };
        let html_attribute = |id: NodeId, name: &str| match document.node(id).map(|n| n.kind()) {
            Ok(NodeKind::Element(e))
                if e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE) =>
            {
                e.attributes()
                    .iter()
                    .find(|a| a.name().namespace().is_none() && a.name().local() == name)
                    .map(|a| String::from_utf16_lossy(a.value().as_units()))
            }
            _ => None,
        };
        let is_ol = matches!(document.node(parent).map(|n| n.kind()),
            Ok(NodeKind::Element(e)) if e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE)
                && e.name().local() == "ol");
        let (next, step) = *self.next.entry(parent).or_insert_with(|| {
            let reversed = is_ol && html_attribute(parent, "reversed").is_some();
            let start = if is_ol {
                html_attribute(parent, "start").as_deref().and_then(integer)
            } else {
                None
            };
            if reversed {
                let items = document.children(parent).map_or(0, |children| {
                    children
                        .filter(|&child| {
                            matches!(document.node(child).map(|n| n.kind()),
                            Ok(NodeKind::Element(e)) if e.name().local() == "li")
                        })
                        .count()
                });
                (start.unwrap_or(items as i64), -1)
            } else {
                (start.unwrap_or(1), 1)
            }
        });
        let value = html_attribute(item, "value")
            .as_deref()
            .and_then(integer)
            .unwrap_or(next);
        self.next.insert(parent, (value.saturating_add(step), step));
        value
    }
}

/// The HTML rules for parsing integers (leading whitespace, a sign, digits).
fn integer(value: &str) -> Option<i64> {
    let rest = value.trim_start_matches(['\t', '\n', '\x0c', '\r', ' ']);
    let (negative, rest) = match rest.as_bytes().first() {
        Some(b'-') => (true, &rest[1..]),
        Some(b'+') => (false, &rest[1..]),
        _ => (false, rest),
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let n = digits
        .parse::<i64>()
        .unwrap_or(i64::from(i32::MAX))
        .min(i64::from(i32::MAX));
    Some(if negative { -n } else { n })
}

/// A list item's marker text and whether it hangs outside: the counter
/// in the item's `list-style-type`, with its suffix (CSS Counter Styles 3
/// §6). `None` for `list-style-type: none`.
fn marker_text(style: &ComputedStyle, ordinal: i64) -> Option<(Vec<u16>, bool)> {
    use rustic_css::{ListStylePosition, ListStyleType as T};
    let PropertyValue::ListStyleType(kind) = style.get(PropertyId::ListStyleType) else {
        return None;
    };
    let outside = matches!(
        style.get(PropertyId::ListStylePosition),
        PropertyValue::ListStylePosition(ListStylePosition::Outside)
    );
    let text = match kind {
        T::None => return None,
        T::Disc => "\u{2022} ".to_owned(),
        T::Circle => "\u{25E6} ".to_owned(),
        T::Square => "\u{25AA} ".to_owned(),
        T::DisclosureOpen => "\u{25BE} ".to_owned(),
        T::DisclosureClosed => "\u{25B8} ".to_owned(),
        numeric => format!("{}. ", counter(*numeric, ordinal)),
    };
    Some((text.encode_utf16().collect(), outside))
}

/// `ordinal` in a numeric or alphabetic counter style, falling back to
/// `decimal` outside the style's range.
fn counter(kind: rustic_css::ListStyleType, ordinal: i64) -> String {
    use rustic_css::ListStyleType as T;
    let alphabetic = |letters: &[char]| {
        if ordinal < 1 {
            return ordinal.to_string();
        }
        let mut n = ordinal;
        let mut out = Vec::new();
        while n > 0 {
            n -= 1;
            out.push(letters[(n % letters.len() as i64) as usize]);
            n /= letters.len() as i64;
        }
        out.iter().rev().collect()
    };
    let roman = |upper: bool| {
        if !(1..=3999).contains(&ordinal) {
            return ordinal.to_string();
        }
        const DIGITS: [(i64, &str); 13] = [
            (1000, "m"),
            (900, "cm"),
            (500, "d"),
            (400, "cd"),
            (100, "c"),
            (90, "xc"),
            (50, "l"),
            (40, "xl"),
            (10, "x"),
            (9, "ix"),
            (5, "v"),
            (4, "iv"),
            (1, "i"),
        ];
        let mut n = ordinal;
        let mut out = String::new();
        for (value, digits) in DIGITS {
            while n >= value {
                out.push_str(digits);
                n -= value;
            }
        }
        if upper { out.to_ascii_uppercase() } else { out }
    };
    let latin: Vec<char> = ('a'..='z').collect();
    match kind {
        T::DecimalLeadingZero if (0..10).contains(&ordinal) => format!("0{ordinal}"),
        T::DecimalLeadingZero if (-9..0).contains(&ordinal) => format!("-0{}", -ordinal),
        T::LowerRoman => roman(false),
        T::UpperRoman => roman(true),
        T::LowerAlpha | T::LowerLatin => alphabetic(&latin),
        T::UpperAlpha | T::UpperLatin => alphabetic(&latin).to_uppercase(),
        T::LowerGreek => alphabetic(&[
            'α', 'β', 'γ', 'δ', 'ε', 'ζ', 'η', 'θ', 'ι', 'κ', 'λ', 'μ', 'ν', 'ξ', 'ο', 'π', 'ρ',
            'σ', 'τ', 'υ', 'φ', 'χ', 'ψ', 'ω',
        ]),
        _ => ordinal.to_string(),
    }
}

/// An outermost `svg`'s natural size: its `width` and `height` attributes
/// when they are absolute lengths, and the ratio of its `viewBox` (or of
/// those dimensions).
fn svg_natural_size(element: &rustic_dom::Element) -> NaturalSize {
    let attribute = |name: &str| {
        element
            .attributes()
            .iter()
            .find(|a| a.name().namespace().is_none() && a.name().local() == name)
            .map(|a| String::from_utf16_lossy(a.value().as_units()))
    };
    let length = |name: &str| {
        let value = attribute(name)?;
        let value = value.trim();
        let number = value.strip_suffix("px").unwrap_or(value).trim();
        number
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .and_then(|n| CssPx::new(n).ok())
    };
    let view_box = attribute("viewBox").and_then(|v| {
        let n: Vec<f64> = v
            .split(|c: char| c.is_ascii_whitespace() || c == ',')
            .filter(|t| !t.is_empty())
            .map(str::parse::<f64>)
            .collect::<Result<_, _>>()
            .ok()?;
        (n.len() == 4 && n[2] > 0.0 && n[3] > 0.0 && n.iter().all(|x| x.is_finite()))
            .then(|| n[2] / n[3])
    });
    let (width, height) = (length("width"), length("height"));
    let ratio = view_box.or_else(|| match (width, height) {
        (Some(w), Some(h)) if w.get() > 0.0 && h.get() > 0.0 => Some(w.get() / h.get()),
        _ => None,
    });
    NaturalSize::new(width, height, ratio).unwrap_or(NaturalSize::NONE)
}

/// A zero-width space: the text of an empty field, which keeps one line's
/// height.
const STRUT: [u16; 1] = [0x200B];

/// For an HTML control whose rendering replaces its children (`input`,
/// `select`, `meter`, `progress`), the text it shows, if any, and whether
/// that text is a placeholder; `None` for every other element.
#[allow(
    clippy::option_option,
    reason = "outer: a control at all; inner: whether it shows text"
)]
fn control_text(
    document: &Document,
    id: NodeId,
    element: &rustic_dom::Element,
    live_value: Option<&[u16]>,
) -> Option<Option<(Vec<u16>, bool)>> {
    if element.name().namespace() != Some(rustic_dom::HTML_NAMESPACE) {
        return None;
    }
    let attribute = |e: &rustic_dom::Element, name: &str| {
        e.attributes()
            .iter()
            .find(|a| a.name().namespace().is_none() && a.name().local() == name)
            .map(|a| a.value().as_units().to_vec())
    };
    let text = |s: &str| Some((s.encode_utf16().collect::<Vec<u16>>(), false));
    match element.name().local() {
        "meter" | "progress" => Some(None),
        "input" => {
            let kind = attribute(element, "type")
                .map(|t| String::from_utf16_lossy(&t).to_ascii_lowercase())
                .unwrap_or_default();
            let value = match live_value {
                // A text field's live value replaces its `value` attribute.
                Some(live) => Some(live.to_vec()).filter(|v| !v.is_empty()),
                None => attribute(element, "value").filter(|v| !v.is_empty()),
            };
            Some(match kind.as_str() {
                "checkbox" | "radio" | "range" | "color" | "hidden" | "image" => None,
                "submit" => value.map(|v| (v, false)).or_else(|| text("Submit")),
                "reset" => value.map(|v| (v, false)).or_else(|| text("Reset")),
                "button" => value.map(|v| (v, false)),
                "file" => text("Choose File  No file chosen"),
                "password" => match value {
                    Some(v) => Some((vec![0x2022; v.len()], false)),
                    None => attribute(element, "placeholder").map(|p| (p, true)),
                },
                _ => match value {
                    // Line breaks are stripped from single-line values.
                    Some(v) => Some((
                        v.into_iter().filter(|&u| u != 0x0A && u != 0x0D).collect(),
                        false,
                    )),
                    None => attribute(element, "placeholder")
                        .filter(|p| !p.is_empty())
                        .map(|p| (p, true))
                        .or(Some((STRUT.to_vec(), false))),
                },
            })
        }
        "select" => {
            // The first selected option, else the first option (a list box
            // with `multiple` or `size` shows the same, TD-219).
            let mut first = None;
            let mut selected = None;
            if let Ok(walk) = document.walk(id) {
                for (option, _) in walk {
                    let Ok(NodeKind::Element(e)) = document.node(option).map(|n| n.kind()) else {
                        continue;
                    };
                    if e.name().namespace() != Some(rustic_dom::HTML_NAMESPACE)
                        || e.name().local() != "option"
                    {
                        continue;
                    }
                    first.get_or_insert(option);
                    if attribute(e, "selected").is_some() {
                        selected = Some(option);
                        break;
                    }
                }
            }
            let Some(option) = selected.or(first) else {
                return Some(Some((STRUT.to_vec(), false)));
            };
            let label = match document.node(option).map(|n| n.kind()) {
                Ok(NodeKind::Element(e)) => attribute(e, "label").filter(|l| !l.is_empty()),
                _ => None,
            };
            let label = label.unwrap_or_else(|| option_text(document, option));
            Some(Some((
                if label.is_empty() {
                    STRUT.to_vec()
                } else {
                    label
                },
                false,
            )))
        }
        _ => None,
    }
}

/// An `option`'s text: its descendant text with ASCII whitespace stripped
/// and collapsed (HTML §4.10.10).
fn option_text(document: &Document, option: NodeId) -> Vec<u16> {
    let mut units = Vec::new();
    if let Ok(walk) = document.walk(option) {
        for (node, _) in walk {
            if let Ok(NodeKind::Text(data)) = document.node(node).map(|n| n.kind()) {
                units.extend_from_slice(data.as_units());
            }
        }
    }
    let text = String::from_utf16_lossy(&units);
    text.split(['\t', '\n', '\x0c', '\r', ' '])
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .encode_utf16()
        .collect()
}

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

#[cfg(test)]
mod tests {
    use super::{counter, integer};
    use rustic_css::ListStyleType as T;

    #[test]
    fn counter_styles_render_ordinals_with_decimal_fallbacks() {
        assert_eq!(counter(T::Decimal, -3), "-3");
        assert_eq!(counter(T::DecimalLeadingZero, 7), "07");
        assert_eq!(counter(T::DecimalLeadingZero, 12), "12");
        assert_eq!(counter(T::LowerRoman, 1994), "mcmxciv");
        assert_eq!(counter(T::UpperRoman, 4000), "4000");
        assert_eq!(counter(T::LowerAlpha, 1), "a");
        assert_eq!(counter(T::UpperLatin, 28), "AB");
        assert_eq!(counter(T::LowerAlpha, 0), "0");
        assert_eq!(counter(T::LowerGreek, 25), "αα");
    }

    #[test]
    fn list_values_parse_as_html_integers() {
        assert_eq!(integer(" +12abc"), Some(12));
        assert_eq!(integer("-4"), Some(-4));
        assert_eq!(integer("x1"), None);
    }
}
