//! Option selectedness and `selectedcontent` mirroring (customizable select).
//!
//! WHATWG HTML "maybe clone an option into selectedcontent", reviewed
//! 2026-11-08: when the parser pops an `option`, a selected option's children
//! are deep-cloned into its select's enabled `selectedcontent`, replacing the
//! previous contents. Selectedness follows the selectedness setting algorithm
//! for a single-selection select: an option with `selected` wins (the last one
//! inserted), otherwise the first non-disabled option.

use rustic_dom::{NodeId, NodeKind};

use super::{ParseError, TreeBuilder};

impl TreeBuilder {
    fn has_attribute(&self, id: NodeId, name: &str) -> bool {
        self.element(id)
            .attributes()
            .iter()
            .any(|a| a.name().namespace().is_none() && a.name().local() == name)
    }

    /// The option's nearest ancestor `select`, if no `datalist` or `option` intervenes.
    fn option_select(&self, option: NodeId) -> Option<NodeId> {
        let mut node = self.document.node(option).ok()?.parent();
        while let Some(id) = node {
            let node_ref = self.document.node(id).ok()?;
            if let NodeKind::Element(_) = node_ref.kind() {
                match self.html_name(id) {
                    Some("select") => return Some(id),
                    Some("datalist" | "option") => return None,
                    _ => {}
                }
            }
            node = node_ref.parent();
        }
        None
    }

    /// Run the selectedness setting algorithm for a newly inserted option.
    pub(super) fn option_inserted(&mut self, option: NodeId) {
        let Some(select) = self.option_select(option) else {
            return;
        };
        if self.has_attribute(select, "multiple") {
            return;
        }
        let explicit = self.has_attribute(option, "selected");
        let disabled = self.has_attribute(option, "disabled");
        match self.selected_options.iter_mut().find(|(s, _)| *s == select) {
            Some((_, selected)) if explicit => *selected = option,
            Some(_) => {}
            None if explicit || !disabled => self.selected_options.push((select, option)),
            None => {}
        }
        self.pending_options.push((select, option));
    }

    /// The select's first `selectedcontent` descendant not inside a nested select.
    fn enabled_selectedcontent(&self, select: NodeId) -> Option<NodeId> {
        if self.has_attribute(select, "multiple") {
            return None;
        }
        let found = self
            .document
            .walk(select)
            .ok()?
            .map(|(id, _)| id)
            .find(|&id| {
                matches!(
                    self.document.node(id).map(|n| n.kind()),
                    Ok(NodeKind::Element(_))
                ) && self.is_html(id, "selectedcontent")
            })?;
        let mut node = self.document.node(found).ok()?.parent();
        while let Some(id) = node {
            if id == select {
                return Some(found);
            }
            if self.is_html(id, "select") {
                return None;
            }
            node = self.document.node(id).ok()?.parent();
        }
        None
    }

    /// Mirror options that have left the stack of open elements. At EOF the
    /// parser's stop-parsing step pops every element, so `all` handles the
    /// rest in pop order (most recently opened first).
    pub(super) fn mirror_popped_options(&mut self, all: bool) -> Result<(), ParseError> {
        if self.pending_options.is_empty() {
            return Ok(());
        }
        let mut index = self.pending_options.len();
        while index > 0 {
            index -= 1;
            let (select, option) = self.pending_options[index];
            if !all && self.open.contains(&option) {
                continue;
            }
            self.pending_options.remove(index);
            let selected = self
                .selected_options
                .iter()
                .any(|&(s, o)| s == select && o == option);
            if !selected {
                continue;
            }
            if let Some(target) = self.enabled_selectedcontent(select) {
                self.clone_into_selectedcontent(option, target)?;
            }
        }
        Ok(())
    }

    /// Replace `target`'s children with deep clones of `option`'s children.
    fn clone_into_selectedcontent(
        &mut self,
        option: NodeId,
        target: NodeId,
    ) -> Result<(), ParseError> {
        let old: Vec<NodeId> = self.document.children(target)?.collect();
        for child in old {
            self.document.detach(child)?;
            if !self.exposed_document {
                self.document.destroy_subtree(child)?;
            }
        }
        let sources: Vec<NodeId> = self.document.children(option)?.collect();
        for source in sources {
            let copy = self.document.clone_node(source, true)?;
            self.attach_or_destroy(target, None, copy)?;
        }
        Ok(())
    }
}
