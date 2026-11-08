//! CSS 2.2 §8.3.1: associative margin struts and one block's collapse result.

use crate::{CssPx, FlowError};
use rustic_base::CancellationToken;

/// Keep extrema, not a running sum: adding another margin to an already
/// collapsed positive/negative pair must retain both original extrema.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MarginStrut {
    positive: f64,
    negative: f64,
}

impl MarginStrut {
    pub(crate) fn from(value: CssPx) -> Self {
        Self {
            positive: value.get().max(0.0),
            negative: value.get().min(0.0),
        }
    }

    pub(crate) fn merge(self, other: Self) -> Self {
        Self {
            positive: self.positive.max(other.positive),
            negative: self.negative.min(other.negative),
        }
    }

    pub(crate) fn value(self) -> Result<CssPx, FlowError> {
        Ok(CssPx::new(self.positive + self.negative)?)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CollapseChild {
    pub before: MarginStrut,
    pub after: MarginStrut,
    pub through: bool,
    pub border_height: CssPx,
}

pub(crate) struct CollapseInput {
    pub top: CssPx,
    pub bottom: CssPx,
    pub open_top: bool,
    pub open_bottom: bool,
    pub can_through: bool,
    pub nonzero_minimum: bool,
}

pub(crate) struct CollapsedBlock {
    pub before: MarginStrut,
    pub after: MarginStrut,
    pub through: bool,
    pub natural_height: CssPx,
    pub child_positions: Vec<CssPx>,
}

pub(crate) fn collapse_block(
    input: CollapseInput,
    children: &[CollapseChild],
    cancellation: &CancellationToken,
) -> Result<CollapsedBlock, FlowError> {
    cancellation.check().map_err(|_| FlowError::Cancelled)?;
    let mut all_through = true;
    for child in children {
        cancellation.check().map_err(|_| FlowError::Cancelled)?;
        all_through &= child.through;
    }
    let mut before = MarginStrut::from(input.top);
    if input.open_top {
        for child in children {
            cancellation.check().map_err(|_| FlowError::Cancelled)?;
            before = before.merge(child.before);
            if !child.through {
                break;
            }
            before = before.merge(child.after);
        }
    }
    // The min-height exception prevents an all-through child's bottom margin
    // from also escaping the bottom after it escaped through the parent's top.
    let open_bottom =
        input.open_bottom && !(input.nonzero_minimum && input.open_top && all_through);
    let mut cursor = CssPx::ZERO;
    let mut pending = MarginStrut::default();
    let mut leading = true;
    let mut positions = Vec::with_capacity(children.len());
    for child in children {
        cancellation.check().map_err(|_| FlowError::Cancelled)?;
        let gap = pending.merge(child.before);
        let position = if leading && input.open_top {
            CssPx::ZERO
        } else {
            cursor.checked_add(gap.value()?)?
        };
        positions.push(position);
        if child.through {
            // An empty box's position excludes its own bottom margin, as if
            // it had a nonzero bottom border. That bottom still joins the
            // strut used to place following boxes (CSS 2.2 §8.3.1).
            pending = gap.merge(child.after);
        } else {
            cursor = position.checked_add(child.border_height)?;
            pending = child.after;
            leading = false;
        }
    }
    let mut after = MarginStrut::from(input.bottom);
    if open_bottom {
        after = after.merge(pending);
    }
    let natural = if all_through && input.open_top {
        CssPx::ZERO
    } else if open_bottom {
        cursor
    } else {
        cursor.checked_add(pending.value()?)?
    };
    Ok(CollapsedBlock {
        before,
        after,
        through: input.can_through && all_through,
        natural_height: CssPx::new(natural.get().max(0.0))?,
        child_positions: positions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struts_preserve_extrema_across_every_grouping_and_order() {
        for a in [-30.0, -10.0, 0.0, 10.0, 20.0] {
            for b in [-30.0, -10.0, 0.0, 10.0, 20.0] {
                for c in [-30.0, -10.0, 0.0, 10.0, 20.0] {
                    let strut = |n| MarginStrut::from(CssPx::new(n).unwrap());
                    let expected = [a, b, c].into_iter().fold(0.0_f64, f64::max)
                        + [a, b, c].into_iter().fold(0.0_f64, f64::min);
                    assert_eq!(
                        strut(a)
                            .merge(strut(b))
                            .merge(strut(c))
                            .value()
                            .unwrap()
                            .get(),
                        expected
                    );
                    assert_eq!(
                        strut(c)
                            .merge(strut(a).merge(strut(b)))
                            .value()
                            .unwrap()
                            .get(),
                        expected
                    );
                }
            }
        }
    }
}
