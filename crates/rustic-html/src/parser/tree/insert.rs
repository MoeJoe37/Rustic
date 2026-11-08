use rustic_dom::{DomError, MATHML_NAMESPACE, NodeId, NodeKind, QualifiedName, SVG_NAMESPACE};

use super::foreign::{
    adjust_mathml_attribute, adjust_svg_attribute, adjust_svg_tag, foreign_attribute,
};
use super::{Ns, ParseError, Tag, TreeBuilder};

/// An insertion location: before a child of `parent`, or after its last child.
#[derive(Clone, Copy, Debug)]
pub(super) struct Place {
    pub(super) parent: NodeId,
    pub(super) before: Option<NodeId>,
}

impl TreeBuilder {
    /// The appropriate place for inserting a node, including foster parenting
    /// and redirection into template contents.
    pub(super) fn place(&self, target: Option<NodeId>) -> Place {
        let target = target
            .or_else(|| self.open.last().copied())
            .unwrap_or_else(|| self.document.root());
        let place = if self.foster_parenting
            && matches!(
                self.html_name(target),
                Some("table" | "tbody" | "tfoot" | "thead" | "tr")
            ) {
            let template = self
                .open
                .iter()
                .rposition(|id| self.is_html(*id, "template"));
            let table = self.open.iter().rposition(|id| self.is_html(*id, "table"));
            match (template, table) {
                (Some(template), table) if table.is_none_or(|table| template > table) => Place {
                    parent: self.open[template],
                    before: None,
                },
                (_, None) => Place {
                    parent: self.open[0],
                    before: None,
                },
                (_, Some(index)) => {
                    let table = self.open[index];
                    match self.document.node(table).expect("live table").parent() {
                        Some(parent) => Place {
                            parent,
                            before: Some(table),
                        },
                        None => Place {
                            parent: self.open[index - 1],
                            before: None,
                        },
                    }
                }
            }
        } else {
            Place {
                parent: target,
                before: None,
            }
        };
        match self
            .document
            .node(place.parent)
            .expect("live parent")
            .kind()
        {
            NodeKind::Element(element) if element.template_contents().is_some() => Place {
                parent: element.template_contents().expect("checked contents"),
                before: None,
            },
            _ => place,
        }
    }

    /// Create an element for a token in the given namespace, adjusting foreign
    /// names and attributes. A failure destroys the unattached element.
    pub(super) fn create_element(&mut self, tag: &Tag, ns: Ns) -> Result<NodeId, ParseError> {
        let name = match ns {
            Ns::Html => QualifiedName::html(tag.name())?,
            Ns::Svg => {
                QualifiedName::from_parsed(Some(SVG_NAMESPACE), None, adjust_svg_tag(tag.name()))?
            }
            Ns::MathMl => QualifiedName::from_parsed(Some(MATHML_NAMESPACE), None, tag.name())?,
        };
        let id = self.document.create_element(name)?;
        let result = (|| {
            for attribute in tag.attributes() {
                let name = match ns {
                    Ns::Html => QualifiedName::from_parsed(None, None, attribute.name())?,
                    Ns::Svg | Ns::MathMl => {
                        let local = if ns == Ns::Svg {
                            adjust_svg_attribute(attribute.name())
                        } else {
                            adjust_mathml_attribute(attribute.name())
                        };
                        match foreign_attribute(local) {
                            Some((namespace, prefix, local)) => {
                                QualifiedName::from_parsed(Some(namespace), prefix, local)?
                            }
                            None => QualifiedName::from_parsed(None, None, local)?,
                        }
                    }
                };
                self.document.set_attribute(id, name, attribute.value())?;
            }
            if ns == Ns::Html && tag.name() == "template" {
                self.document.create_template_contents(id)?;
            }
            Ok::<(), DomError>(())
        })();
        if let Err(error) = result {
            self.document
                .destroy_subtree(id)
                .expect("unattached new element");
            return Err(error.into());
        }
        Ok(id)
    }

    /// Insert a foreign or HTML element for a token and push it onto the stack.
    pub(super) fn insert_element(&mut self, tag: &Tag, ns: Ns) -> Result<NodeId, ParseError> {
        self.open
            .try_reserve(1)
            .map_err(|_| DomError::AllocationFailed)?;
        let place = self.place(None);
        let id = self.create_element(tag, ns)?;
        self.attach_or_destroy(place.parent, place.before, id)?;
        self.open.push(id);
        Ok(id)
    }

    pub(super) fn insert_html(&mut self, tag: &Tag) -> Result<NodeId, ParseError> {
        self.insert_element(tag, Ns::Html)
    }

    /// Insert an element that is immediately popped, acknowledging `/>`.
    pub(super) fn insert_void(&mut self, tag: &Tag) -> Result<(), ParseError> {
        self.insert_html(tag)?;
        self.open.pop();
        self.self_closing_acknowledged = true;
        Ok(())
    }

    pub(super) fn insert_implied(&mut self, name: &str) -> Result<NodeId, ParseError> {
        self.insert_html(&Tag {
            name: name.into(),
            attributes: Vec::new(),
            self_closing: false,
        })
    }

    /// Add attributes missing from an existing element, without overwriting.
    pub(super) fn add_missing_attributes(
        &mut self,
        id: NodeId,
        tag: &Tag,
    ) -> Result<(), ParseError> {
        for attribute in tag.attributes() {
            let exists =
                self.element(id).attributes().iter().any(|a| {
                    a.name().namespace().is_none() && a.name().local() == attribute.name()
                });
            if !exists {
                self.document.set_attribute(
                    id,
                    QualifiedName::from_parsed(None, None, attribute.name())?,
                    attribute.value(),
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn attach_or_destroy(
        &mut self,
        parent: NodeId,
        before: Option<NodeId>,
        id: NodeId,
    ) -> Result<(), ParseError> {
        if let Err(error) = self.document.insert_before(parent, id, before) {
            self.document
                .destroy_subtree(id)
                .expect("new unattached parser node");
            return Err(error.into());
        }
        Ok(())
    }

    /// Insert a comment at an explicit parent, or the appropriate place.
    pub(super) fn comment(&mut self, parent: Option<NodeId>, text: &str) -> Result<(), ParseError> {
        let place = match parent {
            Some(parent) => Place {
                parent,
                before: None,
            },
            None => self.place(None),
        };
        let id = self.document.create_comment(text)?;
        self.attach_or_destroy(place.parent, place.before, id)
    }

    /// Insert characters at the appropriate place, coalescing adjacent text.
    pub(super) fn insert_text(&mut self, text: &str) -> Result<(), ParseError> {
        if text.is_empty() {
            return Ok(());
        }
        let place = self.place(None);
        if place.parent == self.document.root() {
            return Ok(());
        }
        let previous = match place.before {
            Some(before) => self.document.node(before)?.previous_sibling(),
            None => self.document.node(place.parent)?.last_child(),
        };
        if let Some(id) = previous {
            if matches!(self.document.node(id)?.kind(), NodeKind::Text(_)) {
                self.document.append_utf8_character_data(id, text)?;
                return Ok(());
            }
        }
        let id = self.document.create_text(text)?;
        self.attach_or_destroy(place.parent, place.before, id)
    }
}
