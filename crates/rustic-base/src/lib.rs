//! Small dependency-free contracts shared by Rustic's engine components.
//!
//! Identities are process-local labels, not authorization capabilities. Budgets
//! account for explicitly charged resources, not all process allocations.

#![warn(missing_docs)]

mod budget;
mod cancellation;
mod clock;
mod id;
mod source;

pub use budget::{Budget, BudgetError};
pub use cancellation::{CancellationToken, Cancelled};
pub use clock::{Clock, ClockOverflow, ManualClock, SystemClock};
pub use id::{DocumentId, IdExhausted, NodeId};
pub use source::{InvalidSpan, SourceSpan};
