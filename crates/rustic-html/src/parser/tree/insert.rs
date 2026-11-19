use rustic_dom::{DomError, MATHML_NAMESPACE, NodeId, NodeKind, QualifiedName, SVG_NAMESPACE};

use super::foreign::{
    adjust_mathml_attribute, adjust_svg_attribute, adjust_svg_tag, foreign_attribute,
};
use super::{Annotation, Ns, ParseError, Tag, TreeBuilder};

/// An insertion location: before a child of `parent`, or after its last child.
#[derive(Clone, Copy, Debug)]
pub(super) struct Place {
    pub(super) parent: NodeId,
    pub(super) before: Option<NodeId>,
}

impl TreeBuilder {
    /// The appropriate place for inserting a node, including foster parenting
    /// and redirection into template contents, capped at the document's
    /// depth limit (see [`Self::cap_depth`]).
    pub(super) fn place(&mut self, target: Option<NodeId>) -> Result<Place, ParseError> {
        let place = self.uncapped_place(target);
        self.cap_depth(place, 0)
    }

    /// Edges from `id` up to the root of its tree, as the document counts
    /// depth (template contents are a root of their own).
    fn tree_depth(&self, id: NodeId) -> usize {
        let mut depth = 0;
        let mut node = self.document.node(id).ok().and_then(|n| n.parent());
        while let Some(parent) = node {
            depth += 1;
            node = self.document.node(parent).ok().and_then(|n| n.parent());
        }
        depth
    }

    /// Move `place` up until a node of subtree `height` fits under the
    /// document's depth limit (ADR 0046). Past the limit a node goes into
    /// the would-be parent's parent instead (a later sibling), as Blink,
    /// WebKit and Gecko do at their parser depth caps, so a deep document
    /// degrades instead of failing. Each capped insertion is a diagnostic.
    pub(super) fn cap_depth(&mut self, place: Place, height: usize) -> Result<Place, ParseError> {
        let max = self.document.limits().max_depth;
        let mut place = place;
        let mut depth = self.tree_depth(place.parent);
        let mut capped = false;
        while depth.saturating_add(1).saturating_add(height) > max {
            let node = self.document.node(place.parent)?;
            let Some(up) = node.parent().filter(|&up| up != self.document.root()) else {
                break;
            };
            // Appending stays appending (Blink's rule), so later capped
            // nodes follow earlier ones; an insertion before a reference
            // lands right after the would-be parent.
            place = Place {
                parent: up,
                before: place.before.and(node.next_sibling()),
            };
            depth -= 1;
            capped = true;
        }
        if capped {
            self.diagnostic(super::ErrorKind::NestingTooDeep)?;
        }
        Ok(place)
    }

    /// Move an existing node (and its subtree) to `parent` before `before`,
    /// capped at the depth limit like a new node.
    pub(super) fn move_capped(
        &mut self,
        parent: NodeId,
        before: Option<NodeId>,
        node: NodeId,
    ) -> Result<(), ParseError> {
        let height = self
            .document
            .walk(node)?
            .map(|(_, depth)| depth)
            .max()
            .unwrap_or(0);
        let place = self.cap_depth(Place { parent, before }, height)?;
        self.document
            .insert_before(place.parent, node, place.before)?;
        Ok(())
    }

    fn uncapped_place(&self, target: Option<NodeId>) -> Place {
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
        // An element keeps one edge free for its text: Chromium caps
        // elements and comments by stack depth and lets text go one deeper.
        let place = self.uncapped_place(None);
        let place = self.cap_depth(place, 1)?;
        let id = self.create_element(tag, ns)?;
        self.attach_or_destroy(place.parent, place.before, id)?;
        self.open.push(id);
        if ns == Ns::Html && tag.name() == "option" {
            self.option_inserted(id);
        }
        if ns == Ns::Html {
            self.associate_form(tag, id)?;
        }
        Ok(id)
    }

    /// HTML "create an element for a token" step 13 for listed controls: the
    /// form element pointer, unless a template is open or the token names a
    /// form itself. Descendants of the form need no record.
    fn associate_form(&mut self, tag: &Tag, id: NodeId) -> Result<(), ParseError> {
        const LISTED: [&str; 7] = [
            "button", "fieldset", "input", "object", "output", "select", "textarea",
        ];
        let Some(form) = self.form else {
            return Ok(());
        };
        if !LISTED.contains(&tag.name())
            || tag
                .attributes()
                .iter()
                .any(|attribute| attribute.name() == "form")
            || self.has_open("template")
        {
            return Ok(());
        }
        let mut ancestor = self.document.node(id)?.parent();
        while let Some(current) = ancestor {
            if current == form {
                return Ok(());
            }
            ancestor = self.document.node(current)?.parent();
        }
        self.document.associate_parser_form(id, form)?;
        Ok(())
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

    /// Insert a comment or instruction at an explicit parent or the appropriate place.
    pub(super) fn insert_annotation(
        &mut self,
        parent: Option<NodeId>,
        annotation: Annotation<'_>,
    ) -> Result<(), ParseError> {
        let place = match parent {
            Some(parent) => Place {
                parent,
                before: None,
            },
            None => self.uncapped_place(None),
        };
        // Comments are capped like elements; only text goes one deeper.
        let place = self.cap_depth(place, 1)?;
        let id = match annotation {
            Annotation::Comment(text) => self.document.create_comment(text)?,
            Annotation::Instruction(instruction) => self
                .document
                .create_processing_instruction(instruction.target(), instruction.data())?,
        };
        self.attach_or_destroy(place.parent, place.before, id)
    }

    /// Insert characters at the appropriate place, coalescing adjacent text.
    pub(super) fn insert_text(&mut self, text: &str) -> Result<(), ParseError> {
        if text.is_empty() {
            return Ok(());
        }
        let place = self.place(None)?;
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
