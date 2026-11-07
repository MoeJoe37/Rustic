use std::fmt;

/// Explicit accounting for one resource in caller-defined units.
///
/// Failed operations leave usage unchanged. Releasing usage does not itself free
/// memory; the owner must pair accounting with its actual resource lifecycle.
#[derive(Debug)]
pub struct Budget {
    limit: usize,
    used: usize,
}

impl Budget {
    /// Create an unused budget, including a zero-capacity budget.
    pub const fn new(limit: usize) -> Self {
        Self { limit, used: 0 }
    }

    /// Account for more units without overflow or exceeding the limit.
    pub fn charge(&mut self, amount: usize) -> Result<(), BudgetError> {
        let used = self
            .used
            .checked_add(amount)
            .filter(|used| *used <= self.limit)
            .ok_or(BudgetError::Exceeded)?;
        self.used = used;
        Ok(())
    }

    /// Return previously charged units; reject accounting underflow.
    pub fn release(&mut self, amount: usize) -> Result<(), BudgetError> {
        self.used = self
            .used
            .checked_sub(amount)
            .ok_or(BudgetError::InvalidRelease)?;
        Ok(())
    }

    /// Maximum permitted units.
    pub const fn limit(&self) -> usize {
        self.limit
    }

    /// Currently charged units.
    pub const fn used(&self) -> usize {
        self.used
    }

    /// Units that can still be charged.
    pub const fn remaining(&self) -> usize {
        self.limit - self.used
    }
}

/// A resource limit or accounting contract was violated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetError {
    /// Requested usage exceeds the limit or arithmetic range.
    Exceeded,
    /// More units were released than were charged.
    InvalidRelease,
}

impl fmt::Display for BudgetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Exceeded => "resource budget exceeded",
            Self::InvalidRelease => "resource release exceeds charged usage",
        })
    }
}

impl std::error::Error for BudgetError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundaries_release_and_failed_operations_are_atomic() {
        let mut budget = Budget::new(10);
        budget.charge(10).unwrap();
        assert_eq!(budget.charge(1), Err(BudgetError::Exceeded));
        assert_eq!(budget.used(), 10);
        assert_eq!(budget.release(11), Err(BudgetError::InvalidRelease));
        assert_eq!(budget.used(), 10);
        budget.release(4).unwrap();
        assert_eq!(budget.remaining(), 4);
        budget.charge(4).unwrap();
    }

    #[test]
    fn overflow_and_zero_capacity_are_checked() {
        let mut budget = Budget::new(usize::MAX);
        budget.charge(1).unwrap();
        assert_eq!(budget.charge(usize::MAX), Err(BudgetError::Exceeded));
        assert_eq!(budget.used(), 1);
        let mut zero = Budget::new(0);
        zero.charge(0).unwrap();
        assert_eq!(zero.charge(1), Err(BudgetError::Exceeded));
    }
}
