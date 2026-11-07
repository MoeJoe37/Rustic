use std::fmt;

/// A rejected document operation. Validation errors leave the document unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DomError {
    /// The ID belongs to a different document.
    WrongDocument,
    /// The storage slot is absent, vacant, or belongs to a different generation.
    InvalidNode,
    /// A parent or child kind violates the supported node hierarchy.
    InvalidHierarchy,
    /// The requested reference node is not a child of the requested parent.
    InvalidReference,
    /// Attaching a node below itself would create a cycle.
    Cycle,
    /// The document root cannot be moved, detached, or destroyed.
    ProtectedRoot,
    /// The resulting tree would exceed the configured edge depth.
    DepthLimit,
    /// Live nodes, including detached nodes and the document root, exceed the limit.
    NodeLimit,
    /// Accounted string/name data exceeds the document limit or arithmetic range.
    DataLimit,
    /// The operation requires a different node kind.
    WrongNodeKind,
    /// An internal parsed name is empty or has a prefix without a namespace.
    InvalidName,
    /// The process document-ID counter or node slot address space is exhausted.
    IdentityExhausted,
    /// Storage reservation failed.
    AllocationFailed,
}

impl fmt::Display for DomError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::WrongDocument => "node belongs to another document",
            Self::InvalidNode => "node identity is stale or invalid",
            Self::InvalidHierarchy => "operation violates the node hierarchy",
            Self::InvalidReference => "reference is not a child of the parent",
            Self::Cycle => "operation would create a tree cycle",
            Self::ProtectedRoot => "document root cannot be moved or destroyed",
            Self::DepthLimit => "document tree depth limit exceeded",
            Self::NodeLimit => "document node limit exceeded",
            Self::DataLimit => "document string data limit exceeded",
            Self::WrongNodeKind => "operation is not supported for this node kind",
            Self::InvalidName => "invalid internal parsed name",
            Self::IdentityExhausted => "document or node identity space exhausted",
            Self::AllocationFailed => "document storage reservation failed",
        })
    }
}

impl std::error::Error for DomError {}
