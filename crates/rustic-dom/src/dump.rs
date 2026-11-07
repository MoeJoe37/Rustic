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
    /// The output excludes IDs, allocation history, and detached nodes outside the
    /// selected subtree. Names and values are quoted; lone UTF-16 surrogates are
    /// escaped rather than replaced. The limit counts final UTF-8 output bytes,
    /// not temporary allocation capacity. Failure returns no partial string.
    pub fn dump_tree(&self, root: NodeId, max_bytes: usize) -> Result<String, DumpError> {
        let walk = self.walk(root).map_err(DumpError::Node)?;
        let mut writer = LimitedString {
            output: String::new(),
            budget: Budget::new(max_bytes),
        };
        for (id, depth) in walk {
            let node = self.node(id).map_err(DumpError::Node)?;
            write_node(&mut writer, node.kind(), depth).map_err(|_| DumpError::OutputLimit)?;
        }
        Ok(writer.output)
    }
}

fn indent(writer: &mut impl Write, depth: usize) -> fmt::Result {
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

fn write_node(writer: &mut impl Write, kind: &NodeKind, depth: usize) -> fmt::Result {
    indent(writer, depth)?;
    match kind {
        NodeKind::Document => writer.write_str("#document\n"),
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
