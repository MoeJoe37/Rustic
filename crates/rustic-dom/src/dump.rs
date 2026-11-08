use std::fmt::{self, Write};

use rustic_base::{Budget, NodeId};

use crate::{Document, DomError, NodeKind, QualifiedName};

/// Failure to inspect a subtree within the requested output limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DumpError {
    /// The selected subtree root was invalid or belongs to another document.
    Node(DomError),
    /// The diagnostic text would exceed the caller's UTF-8 byte limit.
    OutputLimit,
}

impl fmt::Display for DumpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Node(error) => error.fmt(formatter),
            Self::OutputLimit => formatter.write_str("tree dump output byte limit exceeded"),
        }
    }
}

impl std::error::Error for DumpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Node(error) => Some(error),
            Self::OutputLimit => None,
        }
    }
}

struct LimitedString {
    output: String,
    budget: Budget,
}

impl Write for LimitedString {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.budget.charge(text.len()).map_err(|_| fmt::Error)?;
        self.output.push_str(text);
        Ok(())
    }
}

impl Document {
    /// Produce a deterministic preorder diagnostic dump, never HTML serialization.
    ///
    /// Template contents (`#template-contents`) and shadow roots
    /// (`#shadow-root`) follow their element's attributes, one level deeper.
    /// The output excludes IDs, allocation history, and detached nodes outside the
    /// selected subtree. Names and values are quoted; lone UTF-16 surrogates are
    /// escaped rather than replaced. The limit counts final UTF-8 output bytes,
    /// not temporary allocation capacity. Failure returns no partial string.
    pub fn dump_tree(&self, root: NodeId, max_bytes: usize) -> Result<String, DumpError> {
        let mut writer = LimitedString {
            output: String::new(),
            budget: Budget::new(max_bytes),
        };
        // Nested template contents are separate roots; an explicit stack of
        // walkers (bounded by live nodes) avoids content-controlled recursion.
        let mut walks = vec![(self.walk(root).map_err(DumpError::Node)?, 0usize)];
        while let Some((walk, base)) = walks.last_mut() {
            let base = *base;
            let Some((id, depth)) = walk.next() else {
                walks.pop();
                continue;
            };
            let node = self.node(id).map_err(DumpError::Node)?;
            let fragment = if node.template_host().is_some() {
                FragmentLabel::TemplateContents
            } else if node.shadow_host().is_some() {
                FragmentLabel::ShadowRoot
            } else {
                FragmentLabel::Plain
            };
            write_node(&mut writer, node.kind(), fragment, base + depth)
                .map_err(|_| DumpError::OutputLimit)?;
            if let NodeKind::Element(element) = node.kind() {
                // A shadow root prints before the host's children.
                for owned in element.owned_roots() {
                    walks.push((self.walk(owned).map_err(DumpError::Node)?, base + depth + 1));
                }
            }
        }
        Ok(writer.output)
    }
}

/// Deepest level written as plain two-space indentation.
///
/// Deeper lines (only reachable through nested template contents) start with
/// `@<depth> ` instead, so output stays linear in node count rather than
/// quadratic in content-controlled nesting (docs/TECH_DEBT.md TD-18).
const MAX_INDENT_DEPTH: usize = 64;

fn indent(writer: &mut impl Write, depth: usize) -> fmt::Result {
    if depth > MAX_INDENT_DEPTH {
        return write!(writer, "@{depth} ");
    }
    for _ in 0..depth {
        writer.write_str("  ")?;
    }
    Ok(())
}

fn quoted(writer: &mut impl Write, units: impl IntoIterator<Item = u16>) -> fmt::Result {
    writer.write_char('"')?;
    for value in char::decode_utf16(units) {
        match value {
            Ok('"') => writer.write_str("\\\"")?,
            Ok('\\') => writer.write_str("\\\\")?,
            Ok('\n') => writer.write_str("\\n")?,
            Ok('\r') => writer.write_str("\\r")?,
            Ok('\t') => writer.write_str("\\t")?,
            Ok(value) if value.is_control() || matches!(value, '\u{2028}' | '\u{2029}') => {
                write!(writer, "\\u{:04x}", u32::from(value))?
            }
            Ok(value) => writer.write_char(value)?,
            Err(value) => write!(writer, "\\u{:04x}", value.unpaired_surrogate())?,
        }
    }
    writer.write_char('"')
}

fn optional_name(writer: &mut impl Write, value: Option<&str>) -> fmt::Result {
    match value {
        Some(value) => quoted(writer, value.encode_utf16()),
        None => writer.write_str("null"),
    }
}

fn name(writer: &mut impl Write, name: &QualifiedName) -> fmt::Result {
    writer.write_str("namespace=")?;
    optional_name(writer, name.namespace())?;
    writer.write_str(" prefix=")?;
    optional_name(writer, name.prefix())?;
    writer.write_str(" local=")?;
    quoted(writer, name.local().encode_utf16())
}

/// Which kind of fragment a `DocumentFragment` node is.
#[derive(Clone, Copy)]
enum FragmentLabel {
    Plain,
    TemplateContents,
    ShadowRoot,
}

fn write_node(
    writer: &mut impl Write,
    kind: &NodeKind,
    fragment: FragmentLabel,
    depth: usize,
) -> fmt::Result {
    indent(writer, depth)?;
    match kind {
        NodeKind::Document => writer.write_str("#document\n"),
        NodeKind::DocumentFragment => writer.write_str(match fragment {
            FragmentLabel::Plain => "#document-fragment\n",
            FragmentLabel::TemplateContents => "#template-contents\n",
            FragmentLabel::ShadowRoot => "#shadow-root\n",
        }),
        NodeKind::ProcessingInstruction(value) => {
            writer.write_str("processing-instruction target=")?;
            quoted(writer, value.target().as_units().iter().copied())?;
            writer.write_str(" data=")?;
            quoted(writer, value.data().as_units().iter().copied())?;
            writer.write_char('\n')
        }
        NodeKind::CdataSection(value) => {
            writer.write_str("cdata ")?;
            quoted(writer, value.as_units().iter().copied())?;
            writer.write_char('\n')
        }
        NodeKind::Element(element) => {
            writer.write_str("element ")?;
            name(writer, element.name())?;
            writer.write_char('\n')?;
            for attribute in element.attributes() {
                indent(writer, depth + 1)?;
                writer.write_str("attribute ")?;
                name(writer, attribute.name())?;
                writer.write_str(" value=")?;
                quoted(writer, attribute.value().as_units().iter().copied())?;
                writer.write_char('\n')?;
            }
            Ok(())
        }
        NodeKind::Text(value) | NodeKind::Comment(value) => {
            writer.write_str(if matches!(kind, NodeKind::Text(_)) {
                "text "
            } else {
                "comment "
            })?;
            quoted(writer, value.as_units().iter().copied())?;
            writer.write_char('\n')
        }
        NodeKind::DocumentType(value) => {
            writer.write_str("doctype name=")?;
            quoted(writer, value.name.as_units().iter().copied())?;
            writer.write_str(" public=")?;
            quoted(writer, value.public_id.as_units().iter().copied())?;
            writer.write_str(" system=")?;
            quoted(writer, value.system_id.as_units().iter().copied())?;
            writer.write_char('\n')
        }
    }
}
