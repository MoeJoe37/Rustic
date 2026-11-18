use rustic_dom::DomError;

use super::stack::Scope;
use super::{ErrorKind, NodeId, Ns, ParseError, Tag, TreeBuilder};

/// A list of active formatting elements entry. Elements retain their token
/// so that reconstruction and the adoption agency can recreate them.
#[derive(Debug)]
pub(super) enum Entry {
    Marker,
    Element(NodeId, Tag),
}

enum Bookmark {
    Replace,
    After(NodeId),
}

fn same_attributes(left: &Tag, right: &Tag) -> bool {
    left.attributes().len() == right.attributes().len()
        && left.attributes().iter().all(|attribute| {
            right
                .attributes()
                .iter()
                .any(|other| other.name() == attribute.name() && other.value() == attribute.value())
        })
}

impl TreeBuilder {
    pub(super) fn formatting_position(&self, id: NodeId) -> Option<usize> {
        self.formatting
            .iter()
            .rposition(|entry| matches!(entry, Entry::Element(element, _) if *element == id))
    }

    /// The last formatting element with this name after the last marker.
    pub(super) fn formatting_after_marker(&self, name: &str) -> Option<usize> {
        for (index, entry) in self.formatting.iter().enumerate().rev() {
            match entry {
                Entry::Marker => return None,
                Entry::Element(_, tag) if tag.name() == name => return Some(index),
                Entry::Element(..) => {}
            }
        }
        None
    }

    pub(super) fn push_marker(&mut self) -> Result<(), ParseError> {
        self.formatting
            .try_reserve(1)
            .map_err(|_| DomError::AllocationFailed)?;
        self.formatting.push(Entry::Marker);
        Ok(())
    }

    /// Push a formatting element, applying the three-identical-entries limit.
    pub(super) fn push_formatting(&mut self, id: NodeId, tag: &Tag) -> Result<(), ParseError> {
        let mut matches = 0;
        let mut earliest = None;
        for (index, entry) in self.formatting.iter().enumerate().rev() {
            match entry {
                Entry::Marker => break,
                Entry::Element(_, other)
                    if other.name() == tag.name() && same_attributes(other, tag) =>
                {
                    matches += 1;
                    earliest = Some(index);
                }
                Entry::Element(..) => {}
            }
        }
        if matches >= 3 {
            self.formatting.remove(earliest.expect("matched entry"));
        }
        self.formatting
            .try_reserve(1)
            .map_err(|_| DomError::AllocationFailed)?;
        self.formatting.push(Entry::Element(id, tag.clone()));
        Ok(())
    }

    pub(super) fn clear_formatting_to_marker(&mut self) {
        while let Some(entry) = self.formatting.pop() {
            if matches!(entry, Entry::Marker) {
                break;
            }
        }
    }

    fn is_open(&self, id: NodeId) -> bool {
        self.open.contains(&id)
    }

    pub(super) fn reconstruct_formatting(&mut self) -> Result<(), ParseError> {
        let open_or_marker = |tree: &Self, entry: &Entry| match entry {
            Entry::Marker => true,
            Entry::Element(id, _) => tree.is_open(*id),
        };
        match self.formatting.last() {
            None => return Ok(()),
            Some(entry) if open_or_marker(self, entry) => return Ok(()),
            Some(_) => {}
        }
        let mut index = self.formatting.len() - 1;
        while index > 0 && !open_or_marker(self, &self.formatting[index - 1]) {
            index -= 1;
        }
        for index in index..self.formatting.len() {
            let Entry::Element(_, tag) = &self.formatting[index] else {
                unreachable!("reconstruction stops at markers");
            };
            let tag = tag.clone();
            let id = self.insert_html(&tag)?;
            self.formatting[index] = Entry::Element(id, tag);
        }
        Ok(())
    }

    /// The adoption agency algorithm for a formatting end tag (or `<a>`/`<nobr>`
    /// start tag). Returns `false` when the caller must use "any other end tag".
    pub(super) fn adoption_agency(&mut self, subject: &str) -> Result<bool, ParseError> {
        let current = self.current();
        if self.is_html(current, subject) && self.formatting_position(current).is_none() {
            self.open.pop();
            return Ok(true);
        }
        for _ in 0..8 {
            let Some(position) = self.formatting_after_marker(subject) else {
                return Ok(false);
            };
            let Entry::Element(element, ref tag) = self.formatting[position] else {
                unreachable!("formatting lookup returns elements");
            };
            let tag = tag.clone();
            let Some(stack_index) = self.open.iter().position(|id| *id == element) else {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
                self.formatting.remove(position);
                return Ok(true);
            };
            if !self.in_scope_where(Scope::Default, |_, id| id == element) {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
                return Ok(true);
            }
            if element != self.current() {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
            }
            let Some(block_index) =
                (stack_index + 1..self.open.len()).find(|index| self.is_special(self.open[*index]))
            else {
                self.open.truncate(stack_index);
                self.formatting.remove(position);
                return Ok(true);
            };
            let block = self.open[block_index];
            let ancestor = self.open[stack_index - 1];
            let mut bookmark = Bookmark::Replace;
            let mut node_index = block_index;
            let mut last = block;
            let mut inner = 0;
            loop {
                inner += 1;
                node_index -= 1;
                let node = self.open[node_index];
                if node == element {
                    break;
                }
                if inner > 3 {
                    if let Some(index) = self.formatting_position(node) {
                        self.formatting.remove(index);
                    }
                }
                let Some(index) = self.formatting_position(node) else {
                    self.open.remove(node_index);
                    continue;
                };
                let Entry::Element(_, ref node_tag) = self.formatting[index] else {
                    unreachable!("position refers to an element");
                };
                let node_tag = node_tag.clone();
                let replacement = self.create_element(&node_tag, Ns::Html)?;
                self.formatting[index] = Entry::Element(replacement, node_tag);
                self.open[node_index] = replacement;
                if last == block {
                    bookmark = Bookmark::After(replacement);
                }
                self.move_capped(replacement, None, last)?;
                last = replacement;
            }
            let place = self.place(Some(ancestor))?;
            self.move_capped(place.parent, place.before, last)?;
            let replacement = self.create_element(&tag, Ns::Html)?;
            while let Some(child) = self.document.node(block)?.first_child() {
                self.move_capped(replacement, None, child)?;
            }
            self.move_capped(block, None, replacement)?;
            match bookmark {
                Bookmark::Replace => {
                    let index = self
                        .formatting_position(element)
                        .expect("formatting element");
                    self.formatting[index] = Entry::Element(replacement, tag);
                }
                Bookmark::After(node) => {
                    let index = self.formatting_position(node).expect("bookmarked element");
                    self.formatting
                        .insert(index + 1, Entry::Element(replacement, tag));
                    let old = self
                        .formatting_position(element)
                        .expect("formatting element");
                    self.formatting.remove(old);
                }
            }
            let old = self
                .open
                .iter()
                .position(|id| *id == element)
                .expect("open element");
            self.open.remove(old);
            let block_index = self
                .open
                .iter()
                .position(|id| *id == block)
                .expect("furthest block");
            self.open.insert(block_index + 1, replacement);
        }
        Ok(true)
    }
}
