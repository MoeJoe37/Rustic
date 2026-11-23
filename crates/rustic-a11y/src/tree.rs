//! Tree construction: which DOM nodes become accessibility nodes, their
//! roles (HTML-AAM and ARIA) and states.

use std::collections::HashMap;

use rustic_dom::{Document, Element, HTML_NAMESPACE, NodeId, NodeKind};

use crate::{
    AxId, AxNode, AxTree, Checked, LiveState, RangeValue, Rendering, Role, States, TreeLimits,
    role::Live,
};

/// Build the accessibility tree of `document`.
///
/// With a `rendering`, nodes that generated no box (`display: none`,
/// `<head>` content) and `visibility: hidden` elements are left out and
/// nodes get bounds; without one, only the `hidden` attribute and
/// non-rendered elements (`script`, `style`, `template`, `head`) hide
/// content.
pub fn build(
    document: &Document,
    rendering: Option<&Rendering>,
    live: &LiveState,
    limits: TreeLimits,
) -> AxTree {
    let cx = Context::new(document, rendering, live, limits);
    let root = document.root();
    let mut root_node = AxNode::new(AxId::of(root), Role::Document);
    root_node.name = cx.clip(cx.title());
    root_node.bounds = rendering.and_then(|r| {
        cx.document_element()
            .and_then(|html| r.bounds(html))
            .or_else(|| cx.body().and_then(|body| r.bounds(body)))
    });
    let mut out = Out {
        nodes: vec![root_node],
        truncated: false,
    };
    let mut kids = Vec::new();
    for child in cx.children(root) {
        cx.visit(child, 1, &mut out, &mut kids);
    }
    out.nodes[0].children = kids;
    let focus = live
        .focused
        .map(AxId::of)
        .filter(|id| out.nodes.iter().any(|node| node.id == *id))
        .unwrap_or(out.nodes[0].id);
    if let Some(node) = out.nodes.iter_mut().skip(1).find(|node| node.id == focus) {
        node.states.focused = true;
    }
    AxTree {
        nodes: out.nodes,
        focus,
        truncated: out.truncated,
    }
}

struct Out {
    nodes: Vec<AxNode>,
    truncated: bool,
}

pub(crate) struct Context<'a> {
    pub(crate) document: &'a Document,
    rendering: Option<&'a Rendering>,
    pub(crate) live: &'a LiveState,
    limits: TreeLimits,
    /// First element with each `id`, in tree order.
    ids: HashMap<Vec<u16>, NodeId>,
    /// `label` elements by the control they label, in tree order.
    labels: HashMap<NodeId, Vec<NodeId>>,
}

impl<'a> Context<'a> {
    fn new(
        document: &'a Document,
        rendering: Option<&'a Rendering>,
        live: &'a LiveState,
        limits: TreeLimits,
    ) -> Self {
        let mut cx = Context {
            document,
            rendering,
            live,
            limits,
            ids: HashMap::new(),
            labels: HashMap::new(),
        };
        let Ok(walk) = document.walk(document.root()) else {
            return cx;
        };
        let mut label_elements = Vec::new();
        for (node, _) in walk {
            let Some(element) = cx.html_element(node) else {
                continue;
            };
            if let Some(id) = attr_units(element, "id").filter(|id| !id.is_empty()) {
                cx.ids.entry(id.to_vec()).or_insert(node);
            }
            if element.name().local() == "label" {
                label_elements.push(node);
            }
        }
        for label in label_elements {
            let target = match cx.html_element(label).and_then(|e| attr_units(e, "for")) {
                Some(for_id) => cx
                    .ids
                    .get(for_id)
                    .copied()
                    .filter(|&target| cx.is_labelable(target)),
                None => cx.descendants(label).find(|&node| cx.is_labelable(node)),
            };
            if let Some(target) = target {
                cx.labels.entry(target).or_default().push(label);
            }
        }
        cx
    }

    // ---- DOM helpers ------------------------------------------------------

    pub(crate) fn element(&self, node: NodeId) -> Option<&'a Element> {
        match self.document.node(node).ok()?.kind() {
            NodeKind::Element(element) => Some(element),
            _ => None,
        }
    }

    pub(crate) fn html_element(&self, node: NodeId) -> Option<&'a Element> {
        self.element(node)
            .filter(|element| element.name().namespace() == Some(HTML_NAMESPACE))
    }

    pub(crate) fn local(&self, node: NodeId) -> &'a str {
        self.html_element(node).map_or("", |e| e.name().local())
    }

    pub(crate) fn attr(&self, node: NodeId, name: &str) -> Option<String> {
        self.element(node)
            .and_then(|element| attr_units(element, name))
            .map(String::from_utf16_lossy)
    }

    pub(crate) fn has_attr(&self, node: NodeId, name: &str) -> bool {
        self.element(node)
            .is_some_and(|element| attr_units(element, name).is_some())
    }

    pub(crate) fn text(&self, node: NodeId) -> Option<String> {
        match self.document.node(node).ok()?.kind() {
            NodeKind::Text(text) | NodeKind::CdataSection(text) => {
                Some(String::from_utf16_lossy(text.as_units()))
            }
            _ => None,
        }
    }

    pub(crate) fn parent(&self, node: NodeId) -> Option<NodeId> {
        self.document.node(node).ok()?.parent()
    }

    pub(crate) fn children(&self, node: NodeId) -> Vec<NodeId> {
        self.document
            .children(node)
            .map(Iterator::collect)
            .unwrap_or_default()
    }

    fn descendants(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.document
            .walk(node)
            .into_iter()
            .flatten()
            .map(|(id, _)| id)
            .skip(1)
    }

    fn ancestors(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.parent(node), |&id| self.parent(id))
    }

    pub(crate) fn by_id(&self, id: &str) -> Option<NodeId> {
        let units: Vec<u16> = id.encode_utf16().collect();
        self.ids.get(&units).copied()
    }

    /// Elements an IDREFS attribute names, in attribute order, skipping
    /// unknown ids.
    pub(crate) fn id_refs(&self, node: NodeId, name: &str) -> Vec<NodeId> {
        self.attr(node, name)
            .map(|value| {
                value
                    .split_ascii_whitespace()
                    .filter_map(|id| self.by_id(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn labels_of(&self, node: NodeId) -> &[NodeId] {
        self.labels.get(&node).map_or(&[], Vec::as_slice)
    }

    fn document_element(&self) -> Option<NodeId> {
        self.children(self.document.root())
            .into_iter()
            .find(|&node| self.element(node).is_some())
    }

    fn body(&self) -> Option<NodeId> {
        let html = self.document_element()?;
        self.children(html)
            .into_iter()
            .find(|&node| self.local(node) == "body")
    }

    fn title(&self) -> String {
        let Some(html) = self.document_element() else {
            return String::new();
        };
        self.descendants(html)
            .find(|&node| self.local(node) == "title")
            .map(|title| {
                let mut text = String::new();
                for node in self.descendants(title) {
                    if let Some(part) = self.text(node) {
                        text.push_str(&part);
                    }
                }
                collapse(&text)
            })
            .unwrap_or_default()
    }

    pub(crate) fn clip(&self, mut text: String) -> String {
        if text.len() > self.limits.max_text_bytes {
            let mut end = self.limits.max_text_bytes;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        text
    }

    // ---- visibility -------------------------------------------------------

    /// Whether an element or text is excluded from the tree, with its
    /// subtree (AccName "hidden", HTML-AAM exclusions).
    pub(crate) fn is_hidden(&self, node: NodeId) -> bool {
        if let Some(element) = self.element(node) {
            if attr_units(element, "aria-hidden").is_some_and(|v| ascii_eq(v, "true")) {
                return true;
            }
            if element.name().namespace() == Some(HTML_NAMESPACE) {
                let local = element.name().local();
                if matches!(
                    local,
                    "head" | "script" | "style" | "template" | "noscript" | "datalist"
                ) {
                    return true;
                }
                if local == "input"
                    && attr_units(element, "type").is_some_and(|v| ascii_eq(v, "hidden"))
                {
                    return true;
                }
                if attr_units(element, "hidden").is_some() && self.rendering.is_none() {
                    return true;
                }
            }
        }
        let Some(rendering) = self.rendering else {
            return false;
        };
        if rendering.is_invisible(node) {
            return true;
        }
        if rendering.is_rendered(node) {
            return false;
        }
        // Nodes that never generate boxes of their own but are rendered
        // through their control: options and their groups, image map areas.
        if matches!(self.local(node), "option" | "optgroup" | "area") {
            return false;
        }
        if self.text(node).is_some() {
            // Text inside a select's options has no box either.
            return !self
                .ancestors(node)
                .any(|a| matches!(self.local(a), "option" | "select"));
        }
        if matches!(self.local(node), "html" | "body") {
            return false;
        }
        true
    }

    // ---- roles ------------------------------------------------------------

    /// The node's role: the first supported token of `role`, unless it is a
    /// presentational role on a focusable element; otherwise the implicit
    /// role. `None` means the element has no semantics of its own.
    pub(crate) fn role(&self, node: NodeId) -> Option<Role> {
        if self.text(node).is_some() {
            return Some(Role::StaticText);
        }
        let element = self.element(node)?;
        if let Some(tokens) = attr_units(element, "role") {
            let tokens = String::from_utf16_lossy(tokens);
            for token in tokens.split_ascii_whitespace() {
                if token.eq_ignore_ascii_case("none") || token.eq_ignore_ascii_case("presentation")
                {
                    // Presentational role conflict resolution: focusable
                    // elements and those with global ARIA keep their role.
                    if self.is_focusable(node) || self.has_global_aria(node) {
                        break;
                    }
                    return None;
                }
                if let Some(role) = Role::from_aria(token) {
                    return Some(role);
                }
            }
        }
        self.implicit_role(node)
    }

    fn has_global_aria(&self, node: NodeId) -> bool {
        [
            "aria-label",
            "aria-labelledby",
            "aria-describedby",
            "aria-live",
        ]
        .iter()
        .any(|name| self.has_attr(node, name))
    }

    fn implicit_role(&self, node: NodeId) -> Option<Role> {
        let element = self.html_element(node)?;
        let has = |name: &str| attr_units(element, name).is_some();
        let role = match element.name().local() {
            "a" | "area" if has("href") => Role::Link,
            "address" => Role::Group,
            "article" => Role::Article,
            "aside" => Role::Complementary,
            "blockquote" => Role::Blockquote,
            "br" => Role::LineBreak,
            "button" => Role::Button,
            "caption" => Role::Caption,
            "code" => Role::Code,
            "dd" => Role::Definition,
            "del" | "s" => Role::Deletion,
            "details" => Role::Details,
            "dfn" | "dt" => Role::Term,
            "dialog" => Role::Dialog,
            "dl" => Role::DescriptionList,
            "em" => Role::Emphasis,
            "fieldset" | "optgroup" => Role::Group,
            "figure" => Role::Figure,
            "footer" if !self.in_sectioning(node) => Role::ContentInfo,
            "header" if !self.in_sectioning(node) => Role::Banner,
            "form" => Role::Form,
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => Role::Heading,
            "hr" => Role::Separator,
            "img" => {
                let alt = attr_units(element, "alt");
                if alt.is_some_and(<[u16]>::is_empty)
                    && !has("title")
                    && !has("aria-label")
                    && !has("aria-labelledby")
                {
                    return None;
                }
                Role::Image
            }
            "input" => return self.input_role(node),
            "ins" => Role::Insertion,
            "label" => Role::Label,
            "legend" => Role::Legend,
            "li" => Role::ListItem,
            "main" => Role::Main,
            "mark" => Role::Mark,
            "math" => Role::Math,
            "menu" | "ol" | "ul" => Role::List,
            "meter" => Role::Meter,
            "nav" => Role::Navigation,
            "option" => Role::ListBoxOption,
            "output" => Role::Status,
            "p" => Role::Paragraph,
            "progress" => Role::ProgressIndicator,
            "search" => Role::Search,
            "section" if self.has_author_name(node) => Role::Region,
            "select" => {
                let size = self
                    .attr(node, "size")
                    .and_then(|v| v.trim().parse::<u32>().ok())
                    .unwrap_or(0);
                if has("multiple") || size > 1 {
                    Role::ListBox
                } else {
                    Role::ComboBox
                }
            }
            "strong" => Role::Strong,
            "sub" => Role::Subscript,
            "sup" => Role::Superscript,
            "summary" => {
                let parent = self.parent(node);
                let first = parent
                    .filter(|&p| self.local(p) == "details")
                    .and_then(|p| {
                        self.children(p)
                            .into_iter()
                            .find(|&c| self.local(c) == "summary")
                    });
                if first == Some(node) {
                    Role::DisclosureTriangle
                } else {
                    return None;
                }
            }
            "table" => Role::Table,
            "tbody" | "thead" | "tfoot" => Role::RowGroup,
            "td" => {
                if self.in_grid(node) {
                    Role::GridCell
                } else {
                    Role::Cell
                }
            }
            "textarea" => Role::TextInput,
            "th" => self.header_role(node),
            "time" => Role::Time,
            "tr" => Role::Row,
            "abbr" if has("title") => Role::Abbr,
            _ => return None,
        };
        Some(role)
    }

    fn input_role(&self, node: NodeId) -> Option<Role> {
        let kind = self.input_type(node);
        let list = self.has_attr(node, "list");
        Some(match kind.as_str() {
            "button" | "submit" | "reset" | "image" | "color" | "file" => Role::Button,
            "checkbox" => {
                if self.has_attr(node, "switch") {
                    Role::Switch
                } else {
                    Role::CheckBox
                }
            }
            "radio" => Role::RadioButton,
            "range" => Role::Slider,
            "number" => Role::SpinButton,
            "hidden" => return None,
            "search" if !list => Role::SearchBox,
            "text" | "email" | "tel" | "url" | "search" if list => Role::ComboBox,
            _ => Role::TextInput,
        })
    }

    pub(crate) fn input_type(&self, node: NodeId) -> String {
        const KNOWN: &[&str] = &[
            "hidden",
            "text",
            "search",
            "tel",
            "url",
            "email",
            "password",
            "date",
            "month",
            "week",
            "time",
            "datetime-local",
            "number",
            "range",
            "color",
            "checkbox",
            "radio",
            "file",
            "submit",
            "image",
            "reset",
            "button",
        ];
        let kind = self
            .attr(node, "type")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if KNOWN.contains(&kind.as_str()) {
            kind
        } else {
            "text".to_owned()
        }
    }

    fn in_sectioning(&self, node: NodeId) -> bool {
        self.ancestors(node).any(|a| {
            matches!(
                self.local(a),
                "article" | "aside" | "main" | "nav" | "section"
            )
        })
    }

    fn in_grid(&self, node: NodeId) -> bool {
        self.ancestors(node)
            .find(|&a| self.local(a) == "table")
            .and_then(|table| self.attr(table, "role"))
            .is_some_and(|role| {
                role.split_ascii_whitespace().next().is_some_and(|r| {
                    r.eq_ignore_ascii_case("grid") || r.eq_ignore_ascii_case("treegrid")
                })
            })
    }

    fn header_role(&self, node: NodeId) -> Role {
        match self.attr(node, "scope").map(|s| s.to_ascii_lowercase()) {
            Some(scope) if scope == "row" || scope == "rowgroup" => return Role::RowHeader,
            Some(scope) if scope == "col" || scope == "colgroup" => return Role::ColumnHeader,
            _ => {}
        }
        if self.ancestors(node).any(|a| self.local(a) == "thead") {
            return Role::ColumnHeader;
        }
        // A header cell followed by data cells in its row heads the row.
        let row_has_data = self
            .parent(node)
            .is_some_and(|row| self.children(row).iter().any(|&c| self.local(c) == "td"));
        if row_has_data {
            Role::RowHeader
        } else {
            Role::ColumnHeader
        }
    }

    pub(crate) fn has_author_name(&self, node: NodeId) -> bool {
        self.attr(node, "aria-label")
            .is_some_and(|v| !v.trim().is_empty())
            || !self.id_refs(node, "aria-labelledby").is_empty()
            || self
                .attr(node, "title")
                .is_some_and(|v| !v.trim().is_empty())
    }

    fn is_labelable(&self, node: NodeId) -> bool {
        match self.local(node) {
            "button" | "meter" | "output" | "progress" | "select" | "textarea" => true,
            "input" => self.input_type(node) != "hidden",
            _ => false,
        }
    }

    // ---- states -----------------------------------------------------------

    /// HTML focusable areas as `rustic-bindings` focuses them (ADR 0049).
    pub(crate) fn is_focusable(&self, node: NodeId) -> bool {
        let Some(element) = self.html_element(node) else {
            return false;
        };
        if self.is_disabled_control(node) {
            return false;
        }
        let tabindex = attr_units(element, "tabindex")
            .map(String::from_utf16_lossy)
            .is_some_and(|v| v.trim().parse::<i32>().is_ok());
        if tabindex {
            return true;
        }
        match element.name().local() {
            "a" | "area" => attr_units(element, "href").is_some(),
            "button" | "select" | "textarea" | "summary" => true,
            "input" => self.input_type(node) != "hidden",
            _ => false,
        }
    }

    /// HTML "actually disabled" for form controls.
    pub(crate) fn is_disabled_control(&self, node: NodeId) -> bool {
        let local = self.local(node);
        if !matches!(
            local,
            "button" | "input" | "select" | "textarea" | "optgroup" | "option" | "fieldset"
        ) {
            return false;
        }
        if self.has_attr(node, "disabled") {
            return true;
        }
        if local == "option" {
            return self
                .parent(node)
                .is_some_and(|p| self.local(p) == "optgroup" && self.has_attr(p, "disabled"));
        }
        // A descendant of a disabled fieldset, outside its first legend.
        let mut child = node;
        for ancestor in self.ancestors(node) {
            if self.local(ancestor) == "fieldset" && self.has_attr(ancestor, "disabled") {
                let first_legend = self
                    .children(ancestor)
                    .into_iter()
                    .find(|&c| self.local(c) == "legend");
                return first_legend != Some(child);
            }
            child = ancestor;
        }
        false
    }

    fn aria_bool(&self, node: NodeId, name: &str) -> Option<bool> {
        let value = self.attr(node, name)?.trim().to_ascii_lowercase();
        match value.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }

    fn aria_tristate(&self, node: NodeId, name: &str) -> Option<Checked> {
        let value = self.attr(node, name)?.trim().to_ascii_lowercase();
        match value.as_str() {
            "true" => Some(Checked::True),
            "false" => Some(Checked::False),
            "mixed" => Some(Checked::Mixed),
            _ => None,
        }
    }

    fn aria_number(&self, node: NodeId, name: &str) -> Option<f64> {
        self.attr(node, name)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
    }

    fn states(&self, node: NodeId, role: Role) -> States {
        let local = self.local(node);
        let control = self.live.controls.get(&node);
        let mut s = States {
            focusable: self.is_focusable(node),
            disabled: self.is_disabled_control(node)
                || self.aria_bool(node, "aria-disabled") == Some(true),
            required: self.has_attr(node, "required")
                || self.aria_bool(node, "aria-required") == Some(true),
            readonly: self.has_attr(node, "readonly")
                || self.aria_bool(node, "aria-readonly") == Some(true),
            busy: self.aria_bool(node, "aria-busy") == Some(true),
            modal: self.aria_bool(node, "aria-modal") == Some(true),
            ..States::default()
        };
        s.invalid = control.is_some_and(|c| c.invalid)
            || self
                .attr(node, "aria-invalid")
                .is_some_and(|v| !v.trim().is_empty() && !v.trim().eq_ignore_ascii_case("false"));
        if role.is_checkable() {
            s.checked = control
                .and_then(|c| c.checked)
                .or_else(|| {
                    (local == "input").then(|| {
                        if self.has_attr(node, "checked") {
                            Checked::True
                        } else {
                            Checked::False
                        }
                    })
                })
                .or_else(|| self.aria_tristate(node, "aria-checked"))
                .or(Some(Checked::False));
        }
        s.pressed = self.aria_tristate(node, "aria-pressed");
        s.expanded = self.aria_bool(node, "aria-expanded");
        if role == Role::DisclosureTriangle {
            s.expanded = self.parent(node).map(|p| self.has_attr(p, "open"));
        }
        if role == Role::Details {
            s.expanded = Some(self.has_attr(node, "open"));
        }
        s.selected = self.aria_bool(node, "aria-selected");
        if local == "option" {
            s.selected = Some(self.option_selected(node));
        }
        let text_field = matches!(local, "textarea")
            || (local == "input"
                && matches!(
                    self.input_type(node).as_str(),
                    "text" | "search" | "tel" | "url" | "email" | "password" | "number"
                ));
        s.editable = text_field
            || self
                .attr(node, "contenteditable")
                .is_some_and(|v| v.is_empty() || v.eq_ignore_ascii_case("true"));
        s.multiline = local == "textarea" || self.aria_bool(node, "aria-multiline") == Some(true);
        s.multiselectable = (local == "select" && self.has_attr(node, "multiple"))
            || self.aria_bool(node, "aria-multiselectable") == Some(true);
        s.protected = local == "input" && self.input_type(node) == "password";
        if role == Role::Dialog && local == "dialog" && self.has_attr(node, "open") {
            // `showModal()` state is not tracked yet; an open dialog that
            // says it is modal is.
            s.modal = s.modal || self.aria_bool(node, "aria-modal") == Some(true);
        }
        s
    }

    fn option_selected(&self, option: NodeId) -> bool {
        if self.has_attr(option, "selected") {
            return true;
        }
        // A single-select's first option is selected when none says so.
        let select = self.ancestors(option).find(|&a| self.local(a) == "select");
        let Some(select) = select else {
            return false;
        };
        if self.has_attr(select, "multiple") {
            return false;
        }
        let options: Vec<NodeId> = self
            .descendants(select)
            .filter(|&n| self.local(n) == "option")
            .collect();
        !options.iter().any(|&o| self.has_attr(o, "selected")) && options.first() == Some(&option)
    }

    /// The value shown for a text field, select or range widget.
    pub(crate) fn value(&self, node: NodeId, role: Role) -> Option<String> {
        let local = self.local(node);
        let live = self.live.controls.get(&node).and_then(|c| c.value.clone());
        match local {
            "input" => {
                let kind = self.input_type(node);
                if matches!(
                    kind.as_str(),
                    "checkbox" | "radio" | "button" | "submit" | "reset" | "image" | "file"
                ) {
                    return None;
                }
                let value = live
                    .or_else(|| self.attr(node, "value"))
                    .unwrap_or_default();
                if kind == "password" {
                    return Some("\u{2022}".repeat(value.chars().count()));
                }
                if kind == "range" && value.is_empty() {
                    let min = self.aria_number(node, "min").unwrap_or(0.0);
                    let max = self.aria_number(node, "max").unwrap_or(100.0);
                    return Some(format_number(min + (max - min) / 2.0));
                }
                Some(value)
            }
            "textarea" => Some(live.unwrap_or_else(|| {
                let mut text = String::new();
                for child in self.children(node) {
                    if let Some(part) = self.text(child) {
                        text.push_str(&part);
                    }
                }
                // The parser drops one leading newline; so does the value.
                text.strip_prefix('\n').map(str::to_owned).unwrap_or(text)
            })),
            "select" => {
                let selected = self
                    .descendants(node)
                    .find(|&n| self.local(n) == "option" && self.option_selected(n))?;
                Some(self.option_label(selected))
            }
            "progress" | "meter" => self.attr(node, "value"),
            _ => {
                if let Some(text) = self.attr(node, "aria-valuetext") {
                    return Some(text);
                }
                if role.is_range() {
                    return self.aria_number(node, "aria-valuenow").map(format_number);
                }
                None
            }
        }
    }

    pub(crate) fn option_label(&self, option: NodeId) -> String {
        if let Some(label) = self.attr(option, "label").filter(|l| !l.is_empty()) {
            return label;
        }
        let mut text = String::new();
        for node in self.descendants(option) {
            if let Some(part) = self.text(node) {
                text.push_str(&part);
            }
        }
        collapse(&text)
    }

    fn range(&self, node: NodeId, role: Role) -> Option<RangeValue> {
        if !role.is_range() {
            return None;
        }
        let local = self.local(node);
        if local == "input" || local == "progress" || local == "meter" {
            let default_max = if local == "input" { 100.0 } else { 1.0 };
            let min = if local == "progress" {
                0.0
            } else {
                self.aria_number(node, "min").unwrap_or(0.0)
            };
            let max = self.aria_number(node, "max").unwrap_or(default_max);
            let now = self
                .value(node, role)
                .and_then(|v| v.trim().parse::<f64>().ok());
            return Some(RangeValue {
                min: Some(min),
                max: Some(max),
                now,
            });
        }
        Some(RangeValue {
            min: self.aria_number(node, "aria-valuemin"),
            max: self.aria_number(node, "aria-valuemax"),
            now: self.aria_number(node, "aria-valuenow"),
        })
    }

    fn level(&self, node: NodeId, role: Role) -> Option<u32> {
        if let Some(level) = self
            .attr(node, "aria-level")
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|&l| l > 0)
        {
            return Some(level);
        }
        match self.local(node) {
            "h1" => Some(1),
            "h2" => Some(2),
            "h3" => Some(3),
            "h4" => Some(4),
            "h5" => Some(5),
            "h6" => Some(6),
            _ if role == Role::Heading => Some(2),
            _ => None,
        }
    }

    fn live_politeness(&self, node: NodeId, role: Role) -> Option<Live> {
        match self
            .attr(node, "aria-live")
            .map(|v| v.trim().to_ascii_lowercase())
        {
            Some(v) if v == "polite" => Some(Live::Polite),
            Some(v) if v == "assertive" => Some(Live::Assertive),
            Some(v) if v == "off" => None,
            _ => role.implicit_live(),
        }
    }

    // ---- construction -----------------------------------------------------

    /// Whether an element with no role of its own still needs a node.
    fn keeps_generic(&self, node: NodeId) -> bool {
        self.is_focusable(node)
            || self.has_attr(node, "aria-live")
            || self.has_attr(node, "aria-label")
            || self.has_attr(node, "aria-labelledby")
            || self.attr(node, "contenteditable").is_some()
    }

    fn visit(&self, node: NodeId, depth: usize, out: &mut Out, siblings: &mut Vec<AxId>) {
        if depth > self.limits.max_depth {
            out.truncated = true;
            return;
        }
        if self.is_hidden(node) {
            return;
        }
        if let Some(text) = self.text(node) {
            let collapsed = collapse(&text);
            if collapsed.is_empty() {
                return;
            }
            if out.nodes.len() >= self.limits.max_nodes {
                out.truncated = true;
                return;
            }
            let mut ax = AxNode::new(AxId::of(node), Role::StaticText);
            ax.name = self.clip(collapsed);
            ax.bounds = self.rendering.and_then(|r| r.bounds(node));
            siblings.push(ax.id);
            out.nodes.push(ax);
            return;
        }
        if self.element(node).is_none() {
            return;
        }
        let role = self
            .role(node)
            .or_else(|| self.keeps_generic(node).then_some(Role::Generic));
        let Some(role) = role else {
            for child in self.children(node) {
                self.visit(child, depth, out, siblings);
            }
            return;
        };
        if out.nodes.len() >= self.limits.max_nodes {
            out.truncated = true;
            return;
        }
        let index = out.nodes.len();
        let mut ax = AxNode::new(AxId::of(node), role);
        ax.name = self.clip(self.name(node, role));
        ax.description = self.clip(self.description(node, &ax.name));
        ax.value = self.value(node, role).map(|v| self.clip(v));
        ax.placeholder = self
            .attr(node, "placeholder")
            .or_else(|| self.attr(node, "aria-placeholder"))
            .filter(|p| !p.is_empty())
            .map(|p| self.clip(collapse(&p)));
        if role == Role::Link {
            ax.url = self.attr(node, "href").map(|u| self.clip(u));
        }
        ax.states = self.states(node, role);
        ax.level = self.level(node, role);
        ax.range = self.range(node, role);
        ax.live = self.live_politeness(node, role);
        ax.bounds = self.rendering.and_then(|r| r.bounds(node));
        if ax.states.editable {
            ax.selection = self.live.controls.get(&node).and_then(|c| c.selection);
        }
        siblings.push(ax.id);
        out.nodes.push(ax);
        // Controls whose content is their value, or replaced content, have
        // no accessible children.
        let leaf = matches!(
            self.local(node),
            "input" | "textarea" | "img" | "iframe" | "progress" | "meter" | "br" | "hr"
        );
        let mut kids = Vec::new();
        if !leaf {
            for child in self.children(node) {
                self.visit(child, depth + 1, out, &mut kids);
            }
        }
        out.nodes[index].children = kids;
    }
}

fn attr_units<'e>(element: &'e Element, name: &str) -> Option<&'e [u16]> {
    element
        .attributes()
        .iter()
        .find(|attribute| {
            attribute.name().namespace().is_none() && attribute.name().local() == name
        })
        .map(|attribute| attribute.value().as_units())
}

fn ascii_eq(units: &[u16], expected: &str) -> bool {
    let text = String::from_utf16_lossy(units);
    text.trim().eq_ignore_ascii_case(expected)
}

/// Collapse runs of ASCII whitespace to one space and trim.
pub(crate) fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split(|c: char| c.is_ascii_whitespace()) {
        if word.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}
