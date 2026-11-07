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
    /// Scalar-value text after named/numeric character-reference expansion.
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

/// Parsed doctype syntax; a tree builder still decides the document's quirks mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Doctype {
    pub(crate) name: Option<String>,
    pub(crate) public_id: Option<String>,
    pub(crate) system_id: Option<String>,
    pub(crate) force_quirks: bool,
}

impl Doctype {
    /// ASCII-lowercased name, or `None` when the name was missing.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Case-preserved public identifier; an empty value differs from missing.
    pub fn public_id(&self) -> Option<&str> {
        self.public_id.as_deref()
    }

    /// Case-preserved system identifier; this does not initiate a resource fetch.
    pub fn system_id(&self) -> Option<&str> {
        self.system_id.as_deref()
    }

    /// Tokenizer recovery flag, distinct from the eventual document mode.
    pub fn force_quirks(&self) -> bool {
        self.force_quirks
    }
}

/// Processing-instruction syntax with an accepted target and uninterpreted data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessingInstruction {
    pub(crate) target: String,
    pub(crate) data: String,
}

impl ProcessingInstruction {
    /// Case-preserved ASCII target; disallowed XML targets recover as comments.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Case-preserved data, without leading whitespace or the closing delimiter.
    pub fn data(&self) -> &str {
        &self.data
    }
}

/// Tokens implemented in the declared tokenizer subset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Token {
    /// Deterministically grouped normalized character data.
    Characters(String),
    /// A start tag with attributes and a self-closing syntax flag.
    StartTag(Tag),
    /// End-tag name; malformed attributes/solidus are diagnosed and ignored.
    EndTag(String),
    /// Comment data, including recovery from incorrectly opened declarations.
    Comment(String),
    /// Doctype name, optional identifiers and a force-quirks syntax flag.
    Doctype(Doctype),
    /// Processing-instruction syntax; no instruction is executed.
    ProcessingInstruction(ProcessingInstruction),
    /// Exactly one successful end-of-stream marker.
    Eof,
}
