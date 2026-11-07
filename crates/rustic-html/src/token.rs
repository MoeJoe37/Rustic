/// An attribute in source order. Duplicate names retain their first value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attribute {
    pub(crate) name: String,
    pub(crate) value: String,
}

impl Attribute {
    /// Attribute name with ASCII uppercase letters lowercased.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Decoded scalar-value text; character-reference support is still pending.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Start-tag data emitted by the initial tokenizer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tag {
    pub(crate) name: String,
    pub(crate) attributes: Vec<Attribute>,
    pub(crate) self_closing: bool,
}

impl Tag {
    /// Tag name with ASCII uppercase letters lowercased.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Ordered unique attributes; the first duplicate wins.
    pub fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }
    /// Syntax flag only; a tree builder decides whether it is acknowledged.
    pub fn self_closing(&self) -> bool {
        self.self_closing
    }
}

/// Tokens implemented in the first declared tokenizer subset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Token {
    /// Deterministically grouped normalized character data.
    Characters(String),
    /// A start tag with attributes and a self-closing syntax flag.
    StartTag(Tag),
    /// End-tag name; malformed attributes/solidus are diagnosed and ignored.
    EndTag(String),
    /// Exactly one successful end-of-stream marker.
    Eof,
}
