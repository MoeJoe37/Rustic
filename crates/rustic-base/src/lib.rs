//! Small dependency-free contracts shared by Rustic's engine components.
//!
//! Identities are process-local labels, not authorization capabilities. Budgets
//! account for explicitly charged resources, not all process allocations.
//! Diagnostics carry fixed engine text plus fields with explicit sensitivity,
//! and render redacted by default. Geometry is unit-typed (CSS or device
//! pixels) and checked, and surface allocations are checked before memory
//! is requested.

#![warn(missing_docs)]

mod budget;
mod cancellation;
mod clock;
mod diagnostic;
mod geometry;
mod id;
mod redact;
mod revision;
mod source;

pub use budget::{Budget, BudgetError};
pub use cancellation::{CancellationToken, Cancelled};
pub use clock::{Clock, ClockOverflow, ManualClock, SystemClock};
pub use diagnostic::{
    Diagnostic, DiagnosticLog, DiagnosticLogError, DiagnosticName, InvalidName, MAX_NAME_BYTES,
    RenderError, Severity,
};
pub use geometry::{
    Css, Device, GeometryError, Length, MAX_COORDINATE, Point2, Rect, Scale, Size2,
    SurfaceAllocation, SurfaceError, SurfaceLimits,
};
pub use id::{DocumentId, IdExhausted, NodeId};
pub use redact::{DiagnosticValue, EscapedText, REDACTED, RedactionPolicy, Sensitivity};
pub use revision::{LocationError, RevisionExhausted, SourceId, SourceLocation, SourceRevision};
pub use source::{InvalidSpan, SourceSpan};
