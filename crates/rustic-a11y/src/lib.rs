//! Rustic's accessibility tree (M8 `a11y.semantics`, ADR 0070).
//!
//! [`build`] turns a document, what layout rendered ([`Rendering`]) and the
//! page's live control state ([`LiveState`]) into an [`AxTree`]: one node per
//! element or text run that assistive technology should see, with its
//! [`Role`] (HTML-AAM implicit roles and WAI-ARIA author roles), accessible
//! name and description (AccName 1.2), states, value and bounds. Generic
//! containers with nothing to say are pruned and their children hoisted, as
//! browsers do.
//!
//! The tree is plain data: no platform types and no references into the
//! DOM. Platform bridges (UI Automation, NSAccessibility, AT-SPI) consume it
//! in the trusted browser process; a renderer sends it there with [`wire`],
//! whose decoder treats the bytes as hostile.

#![warn(missing_docs)]

mod dump;
mod name;
mod role;
mod tree;
pub mod wire;

use std::collections::{HashMap, HashSet};

use rustic_dom::NodeId;

pub use role::{Live, Role};
pub use tree::build;

/// A node's identity in an [`AxTree`]: derived from its DOM node so that a
/// tree rebuilt from the same document keeps the same ids.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AxId(pub u64);

impl AxId {
    /// The id of DOM node `node`: the low 32 bits of its generation over its
    /// slot.
    pub fn of(node: NodeId) -> AxId {
        AxId(((node.generation().get() & 0xffff_ffff) << 32) | u64::from(node.slot()))
    }

    /// The slot part (the node's position in its document's arena).
    pub const fn slot(self) -> u32 {
        (self.0 & 0xffff_ffff) as u32
    }

    /// Whether this id names `node` (same slot, same generation bits).
    pub fn names(self, node: NodeId) -> bool {
        AxId::of(node) == self
    }
}

/// An axis-aligned rectangle in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width (never negative).
    pub width: f64,
    /// Height (never negative).
    pub height: f64,
}

impl Rect {
    /// The smallest rectangle containing both.
    pub fn union(self, other: Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = (self.x + self.width).max(other.x + other.width);
        let bottom = (self.y + self.height).max(other.y + other.height);
        Rect {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }

    /// Whether every coordinate is finite and the size is not negative.
    pub fn is_valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            && self.width >= 0.0
            && self.height >= 0.0
    }
}

/// A tri-state checked or pressed value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Checked {
    /// Not checked.
    False,
    /// Checked.
    True,
    /// Indeterminate (`mixed`).
    Mixed,
}

/// A range widget's numeric value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeValue {
    /// Minimum.
    pub min: Option<f64>,
    /// Maximum.
    pub max: Option<f64>,
    /// Current value.
    pub now: Option<f64>,
}

/// Boolean and tri-state accessibility states.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct States {
    /// The node can take keyboard focus.
    pub focusable: bool,
    /// The node has focus.
    pub focused: bool,
    /// The node is disabled.
    pub disabled: bool,
    /// Checked state of a checkable widget.
    pub checked: Option<Checked>,
    /// Pressed state of a toggle button.
    pub pressed: Option<Checked>,
    /// Expanded (`true`) or collapsed (`false`).
    pub expanded: Option<bool>,
    /// Selected state of an option, tab, row or cell.
    pub selected: Option<bool>,
    /// A value is required.
    pub required: bool,
    /// The value fails validation.
    pub invalid: bool,
    /// The value cannot be changed by the user.
    pub readonly: bool,
    /// The text input accepts line breaks.
    pub multiline: bool,
    /// More than one option can be selected.
    pub multiselectable: bool,
    /// The node is an editable text field.
    pub editable: bool,
    /// The dialog is modal.
    pub modal: bool,
    /// The region is being updated.
    pub busy: bool,
    /// The value is protected (a password).
    pub protected: bool,
}

/// One node of an [`AxTree`].
#[derive(Clone, Debug, PartialEq)]
pub struct AxNode {
    /// Identity, stable across rebuilds of the same document.
    pub id: AxId,
    /// Role.
    pub role: Role,
    /// Accessible name (AccName 1.2), whitespace-collapsed.
    pub name: String,
    /// Accessible description.
    pub description: String,
    /// The value of a text field, select or range widget, as the user sees
    /// it (a password's characters are masked).
    pub value: Option<String>,
    /// A text field's placeholder.
    pub placeholder: Option<String>,
    /// A link's target, as written in the document.
    pub url: Option<String>,
    /// States.
    pub states: States,
    /// Heading, tree item or list nesting level (1-based).
    pub level: Option<u32>,
    /// Range widget value.
    pub range: Option<RangeValue>,
    /// Live-region politeness.
    pub live: Option<Live>,
    /// Border box in CSS pixels relative to the viewport, when rendered.
    pub bounds: Option<Rect>,
    /// Selection in a text field, as UTF-16 offsets (anchor, focus).
    pub selection: Option<(u32, u32)>,
    /// Child ids in order.
    pub children: Vec<AxId>,
}

impl AxNode {
    fn new(id: AxId, role: Role) -> Self {
        AxNode {
            id,
            role,
            name: String::new(),
            description: String::new(),
            value: None,
            placeholder: None,
            url: None,
            states: States::default(),
            level: None,
            range: None,
            live: None,
            bounds: None,
            selection: None,
            children: Vec::new(),
        }
    }

    /// What assistive technology can ask this node to do.
    pub fn actions(&self) -> Actions {
        let s = &self.states;
        let activatable = matches!(
            self.role,
            Role::Button
                | Role::Link
                | Role::CheckBox
                | Role::RadioButton
                | Role::Switch
                | Role::MenuItem
                | Role::MenuItemCheckBox
                | Role::MenuItemRadio
                | Role::Tab
                | Role::DisclosureTriangle
                | Role::ListBoxOption
                | Role::TreeItem
        );
        Actions {
            focus: s.focusable && !s.disabled,
            click: activatable && !s.disabled,
            set_value: s.editable && !s.disabled && !s.readonly,
            set_selection: s.editable && !s.disabled,
            scroll_into_view: self.bounds.is_some(),
        }
    }
}

/// Actions a node supports.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Actions {
    /// Move keyboard focus here.
    pub focus: bool,
    /// Run the default action (activate a link or button, toggle a box).
    pub click: bool,
    /// Replace the text value.
    pub set_value: bool,
    /// Move the caret or selection.
    pub set_selection: bool,
    /// Scroll the node into view.
    pub scroll_into_view: bool,
}

/// A request from assistive technology, addressed to one node.
#[derive(Clone, Debug, PartialEq)]
pub enum ActionRequest {
    /// Focus the node.
    Focus,
    /// The default action.
    Click,
    /// Replace a text field's value.
    SetValue(String),
    /// Select UTF-16 offsets `anchor..focus` in a text field.
    SetSelection(u32, u32),
    /// Scroll the node into view.
    ScrollIntoView,
}

/// A complete accessibility tree in preorder: `nodes[0]` is the document.
#[derive(Clone, Debug, PartialEq)]
pub struct AxTree {
    /// Nodes in preorder.
    pub nodes: Vec<AxNode>,
    /// The focused node (the document when nothing else has focus).
    pub focus: AxId,
    /// Nodes were dropped to stay within [`TreeLimits`].
    pub truncated: bool,
}

impl AxTree {
    /// The document node.
    pub fn root(&self) -> &AxNode {
        &self.nodes[0]
    }

    /// The node with id `id`.
    pub fn get(&self, id: AxId) -> Option<&AxNode> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// An index from id to position in [`AxTree::nodes`].
    pub fn index(&self) -> HashMap<AxId, usize> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (node.id, i))
            .collect()
    }

    /// A deterministic, indented text dump, at most `max_bytes` long.
    pub fn dump(&self, max_bytes: usize) -> Result<String, DumpLimit> {
        dump::dump(self, max_bytes)
    }
}

/// The dump would exceed its byte limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DumpLimit;

impl std::fmt::Display for DumpLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("accessibility tree dump exceeds the output limit")
    }
}

impl std::error::Error for DumpLimit {}

/// Bounds on tree construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TreeLimits {
    /// Nodes at most; later nodes are dropped and the tree marked truncated.
    pub max_nodes: usize,
    /// Nesting depth at most; deeper subtrees are dropped.
    pub max_depth: usize,
    /// Bytes of any one name, description or value (cut at a character
    /// boundary).
    pub max_text_bytes: usize,
}

impl Default for TreeLimits {
    fn default() -> Self {
        TreeLimits {
            max_nodes: 50_000,
            max_depth: 256,
            max_text_bytes: 16 * 1024,
        }
    }
}

/// What layout rendered: which DOM nodes produced boxes, where, and which
/// are `visibility: hidden`.
#[derive(Clone, Debug, Default)]
pub struct Rendering {
    boxes: HashMap<NodeId, Rect>,
    rendered: HashSet<NodeId>,
    invisible: HashSet<NodeId>,
}

impl Rendering {
    /// An empty rendering: nothing has a box yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `node` generated a box (even one with no geometry).
    pub fn mark_rendered(&mut self, node: NodeId) {
        self.rendered.insert(node);
    }

    /// Record one fragment of `node`; its bounds become the union of all.
    pub fn add_fragment(&mut self, node: NodeId, rect: Rect) {
        if !rect.is_valid() {
            return;
        }
        self.rendered.insert(node);
        self.boxes
            .entry(node)
            .and_modify(|bounds| *bounds = bounds.union(rect))
            .or_insert(rect);
    }

    /// Record that `node` is `visibility: hidden` (or `collapse`).
    pub fn mark_invisible(&mut self, node: NodeId) {
        self.invisible.insert(node);
    }

    /// Whether `node` generated a box.
    pub fn is_rendered(&self, node: NodeId) -> bool {
        self.rendered.contains(&node)
    }

    /// Whether `node` is `visibility: hidden`.
    pub fn is_invisible(&self, node: NodeId) -> bool {
        self.invisible.contains(&node)
    }

    /// `node`'s bounds.
    pub fn bounds(&self, node: NodeId) -> Option<Rect> {
        self.boxes.get(&node).copied()
    }
}

/// Page state that lives in script rather than the DOM: focus and the
/// current state of form controls.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveState {
    /// The focused element, if any.
    pub focused: Option<NodeId>,
    /// Control state by element.
    pub controls: HashMap<NodeId, ControlState>,
}

/// One form control's live state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ControlState {
    /// The current value (text fields, ranges).
    pub value: Option<String>,
    /// Checkedness (`Mixed` for an indeterminate checkbox).
    pub checked: Option<Checked>,
    /// The control fails constraint validation.
    pub invalid: bool,
    /// Text selection as UTF-16 offsets (anchor, focus).
    pub selection: Option<(u32, u32)>,
}
