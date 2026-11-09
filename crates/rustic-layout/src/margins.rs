//! CSS 2.2 §8.3.1: associative margin struts. The top-down flow walk keeps
//! one unresolved strut per formatting context and joins margins into it.

use crate::{CssPx, FlowError};

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
