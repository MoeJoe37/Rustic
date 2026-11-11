//! Completion Records (§6.2.4) for statement evaluation. A `throw`
//! completion travels as `Err(Abrupt::Throw)` in [`super::JsResult`]; the
//! other abrupt completions are control flow inside a function body.

use super::value::{StringId, Value};

/// The result of evaluating a statement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Completion {
    /// A normal completion, with a value or empty.
    Normal(Option<Value>),
    /// `return`.
    Return(Value),
    /// `break`, with its target label.
    Break(Option<StringId>, Option<Value>),
    /// `continue`, with its target label.
    Continue(Option<StringId>, Option<Value>),
}

impl Completion {
    /// `UpdateEmpty(completion, value)`.
    pub fn update_empty(self, value: Option<Value>) -> Self {
        match self {
            Completion::Normal(None) => Completion::Normal(value),
            Completion::Break(label, None) => Completion::Break(label, value),
            Completion::Continue(label, None) => Completion::Continue(label, value),
            other => other,
        }
    }

    /// `LoopContinues(completion, labelSet)`.
    pub fn loop_continues(self, labels: &[StringId]) -> bool {
        match self {
            Completion::Normal(_) => true,
            Completion::Continue(None, _) => true,
            Completion::Continue(Some(label), _) => labels.contains(&label),
            _ => false,
        }
    }

    /// Whether this is a normal completion.
    pub fn is_normal(self) -> bool {
        matches!(self, Completion::Normal(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_empty_and_loop_continues() {
        let v = Some(Value::TRUE);
        assert_eq!(
            Completion::Normal(None).update_empty(v),
            Completion::Normal(v)
        );
        assert_eq!(
            Completion::Normal(Some(Value::NULL)).update_empty(v),
            Completion::Normal(Some(Value::NULL))
        );
        assert_eq!(
            Completion::Break(None, None).update_empty(v),
            Completion::Break(None, v)
        );
        assert_eq!(
            Completion::Return(Value::NULL).update_empty(v),
            Completion::Return(Value::NULL)
        );
        let label = StringId(3);
        assert!(Completion::Continue(Some(label), None).loop_continues(&[label]));
        assert!(!Completion::Continue(Some(StringId(4)), None).loop_continues(&[label]));
        assert!(!Completion::Break(None, None).loop_continues(&[]));
    }
}
