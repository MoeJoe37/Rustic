//! Accessible name and description computation (AccName 1.2 with the
//! HTML-AAM host-language rules).

use std::collections::HashSet;

use rustic_dom::NodeId;

use crate::Role;
use crate::tree::{Context, collapse};

/// How the current node is reached.
#[derive(Clone, Copy, Default)]
struct Walk {
    /// Inside an `aria-labelledby`/`aria-describedby` traversal.
    referenced: bool,
    /// Computing a name from an ancestor's content (or a label's).
    from_content: bool,
}

impl Context<'_> {
    /// The accessible name of `node` with role `role`.
    pub(crate) fn name(&self, node: NodeId, role: Role) -> String {
        if role.name_prohibited() {
            return String::new();
        }
        let mut visited = HashSet::new();
        collapse(&self.compute(node, Some(role), Walk::default(), &mut visited, true))
    }

    /// The accessible description: `aria-describedby`, then
    /// `aria-description`, then a `title` not already used as the name.
    pub(crate) fn description(&self, node: NodeId, name: &str) -> String {
        let refs = self.id_refs(node, "aria-describedby");
        if !refs.is_empty() {
            let mut visited = HashSet::new();
            let walk = Walk {
                referenced: true,
                from_content: false,
            };
            let parts: Vec<String> = refs
                .into_iter()
                .map(|r| self.compute(r, self.role(r), walk, &mut visited, true))
                .collect();
            return collapse(&parts.join(" "));
        }
        if let Some(text) = self.attr(node, "aria-description") {
            return collapse(&text);
        }
        if let Some(title) = self.attr(node, "title") {
            let title = collapse(&title);
            if title != name {
                return title;
            }
        }
        String::new()
    }

    fn compute(
        &self,
        node: NodeId,
        role: Option<Role>,
        walk: Walk,
        visited: &mut HashSet<NodeId>,
        root: bool,
    ) -> String {
        if !visited.insert(node) {
            return String::new();
        }
        // 2A: hidden nodes contribute nothing unless directly referenced.
        if self.is_hidden(node) && !(walk.referenced && root) {
            return String::new();
        }
        // 2G: text.
        if let Some(text) = self.text(node) {
            return text;
        }
        if self.element(node).is_none() {
            return String::new();
        }
        // 2B: aria-labelledby, once per traversal.
        if !walk.referenced {
            let refs = self.id_refs(node, "aria-labelledby");
            if !refs.is_empty() {
                let next = Walk {
                    referenced: true,
                    from_content: false,
                };
                let parts: Vec<String> = refs
                    .into_iter()
                    .map(|r| {
                        let mut seen = visited.clone();
                        self.compute(r, self.role(r), next, &mut seen, true)
                    })
                    .collect();
                let joined = parts.join(" ");
                if !joined.trim().is_empty() {
                    return joined;
                }
            }
        }
        // 2C: a control embedded in another node's label contributes its
        // value.
        if walk.from_content || (walk.referenced && !root) {
            if let Some(value) = role.and_then(|role| self.embedded_value(node, role)) {
                return value;
            }
        }
        // 2D: aria-label.
        if let Some(label) = self.attr(node, "aria-label") {
            if !label.trim().is_empty() {
                return label;
            }
        }
        // 2E: host language.
        if let Some(native) = self.native_name(node, visited) {
            if !native.trim().is_empty() {
                return native;
            }
        }
        // 2F: name from content.
        let from_content =
            role.is_some_and(Role::name_from_content) || walk.from_content || walk.referenced;
        if from_content && !self.is_leaf_control(node) {
            let text = self.content_text(node, visited);
            if !text.trim().is_empty() {
                return text;
            }
        }
        // 2I: tooltip, then a text field's placeholder (HTML-AAM).
        if let Some(title) = self.attr(node, "title") {
            return title;
        }
        if matches!(self.local(node), "input" | "textarea") {
            if let Some(placeholder) = self.attr(node, "placeholder") {
                return placeholder;
            }
        }
        String::new()
    }

    fn is_leaf_control(&self, node: NodeId) -> bool {
        matches!(self.local(node), "input" | "textarea" | "select" | "img")
    }

    fn embedded_value(&self, node: NodeId, role: Role) -> Option<String> {
        if role.is_text_input() && self.local(node) != "select" {
            return self.value(node, role);
        }
        if matches!(role, Role::ComboBox | Role::ListBox) {
            if self.local(node) == "select" {
                return self.value(node, role);
            }
            return None;
        }
        if role.is_range() {
            return self
                .attr(node, "aria-valuetext")
                .or_else(|| self.value(node, role));
        }
        None
    }

    /// HTML-AAM accessible name sources other than ARIA.
    fn native_name(&self, node: NodeId, visited: &mut HashSet<NodeId>) -> Option<String> {
        let label_text = |cx: &Self, visited: &mut HashSet<NodeId>| -> Option<String> {
            let labels = cx.labels_of(node);
            if labels.is_empty() {
                return None;
            }
            let walk = Walk {
                referenced: false,
                from_content: true,
            };
            let parts: Vec<String> = labels
                .iter()
                .map(|&label| {
                    let mut seen = visited.clone();
                    seen.insert(label);
                    cx.content_text_with(label, &mut seen, walk)
                })
                .collect();
            Some(parts.join(" "))
        };
        match self.local(node) {
            "input" => {
                let kind = self.input_type(node);
                match kind.as_str() {
                    "button" | "submit" | "reset" => {
                        if let Some(value) = self.attr(node, "value") {
                            return Some(value);
                        }
                        if let Some(text) = label_text(self, visited) {
                            return Some(text);
                        }
                        Some(
                            match kind.as_str() {
                                "submit" => "Submit",
                                "reset" => "Reset",
                                _ => "",
                            }
                            .to_owned(),
                        )
                    }
                    "image" => self
                        .attr(node, "alt")
                        .or_else(|| self.attr(node, "value"))
                        .or_else(|| self.attr(node, "title"))
                        .or_else(|| Some("Submit".to_owned())),
                    _ => label_text(self, visited),
                }
            }
            "textarea" | "select" | "meter" | "output" | "progress" => label_text(self, visited),
            "button" => label_text(self, visited),
            "img" | "area" => self.attr(node, "alt"),
            "fieldset" => self.first_child_text(node, "legend", visited),
            "figure" => self.first_child_text(node, "figcaption", visited),
            "table" => self.first_child_text(node, "caption", visited),
            "details" => self.first_child_text(node, "summary", visited),
            "iframe" | "frame" => self.attr(node, "title"),
            "optgroup" => self.attr(node, "label"),
            "option" => Some(self.option_label(node)),
            _ => None,
        }
    }

    fn first_child_text(
        &self,
        node: NodeId,
        local: &str,
        visited: &mut HashSet<NodeId>,
    ) -> Option<String> {
        let child = self
            .children(node)
            .into_iter()
            .find(|&c| self.local(c) == local)?;
        let mut seen = visited.clone();
        seen.insert(child);
        Some(self.content_text_with(
            child,
            &mut seen,
            Walk {
                referenced: false,
                from_content: true,
            },
        ))
    }

    fn content_text(&self, node: NodeId, visited: &mut HashSet<NodeId>) -> String {
        self.content_text_with(
            node,
            visited,
            Walk {
                referenced: false,
                from_content: true,
            },
        )
    }

    /// The concatenated names of `node`'s children, with spaces around
    /// block-level children.
    fn content_text_with(&self, node: NodeId, visited: &mut HashSet<NodeId>, walk: Walk) -> String {
        let walk = Walk {
            referenced: walk.referenced,
            from_content: true,
        };
        let mut out = String::new();
        for child in self.children(node) {
            let part = self.compute(child, self.role(child), walk, visited, false);
            if part.is_empty() {
                continue;
            }
            let block = is_block(self.local(child));
            if block && !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&part);
            if block {
                out.push(' ');
            }
        }
        out
    }
}

/// Elements whose content is separated from its neighbours in a name (the
/// UA stylesheet makes them blocks or line breaks).
fn is_block(local: &str) -> bool {
    matches!(
        local,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "br"
            | "dd"
            | "details"
            | "dialog"
            | "div"
            | "dl"
            | "dt"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hr"
            | "li"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "td"
            | "th"
            | "tr"
            | "ul"
    )
}
