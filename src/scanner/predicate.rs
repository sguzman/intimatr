use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::trace;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ScalarValue {
    Signed(i64),
    Unsigned(u64),
    Float(f64),
}

impl ScalarValue {
    fn kind(self) -> &'static str {
        match self {
            Self::Signed(_) => "signed",
            Self::Unsigned(_) => "unsigned",
            Self::Float(_) => "float",
        }
    }

    fn equals(self, other: Self, epsilon: f64) -> Result<bool, PredicateError> {
        match (self, other) {
            (Self::Signed(left), Self::Signed(right)) => Ok(left == right),
            (Self::Unsigned(left), Self::Unsigned(right)) => Ok(left == right),
            (Self::Float(left), Self::Float(right)) => Ok(float_eq(left, right, epsilon)),
            (left, right) => Err(type_mismatch(left, right)),
        }
    }

    fn compare(self, other: Self) -> Result<Option<Ordering>, PredicateError> {
        match (self, other) {
            (Self::Signed(left), Self::Signed(right)) => Ok(Some(left.cmp(&right))),
            (Self::Unsigned(left), Self::Unsigned(right)) => Ok(Some(left.cmp(&right))),
            (Self::Float(left), Self::Float(right)) => Ok(left.partial_cmp(&right)),
            (left, right) => Err(type_mismatch(left, right)),
        }
    }

    fn is_non_negative(self) -> bool {
        match self {
            Self::Signed(value) => value >= 0,
            Self::Unsigned(_) => true,
            Self::Float(value) => value.is_finite() && value >= 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", content = "operand", rename_all = "snake_case")]
pub enum ScanPredicate {
    Exact(ScalarValue),
    NotEqual(ScalarValue),
    GreaterThan(ScalarValue),
    GreaterOrEqual(ScalarValue),
    LessThan(ScalarValue),
    LessOrEqual(ScalarValue),
    BetweenInclusive { min: ScalarValue, max: ScalarValue },
    Changed,
    Unchanged,
    Increased,
    Decreased,
    IncreasedBy(ScalarValue),
    DecreasedBy(ScalarValue),
}

impl ScanPredicate {
    pub fn requires_previous(self) -> bool {
        matches!(
            self,
            Self::Changed
                | Self::Unchanged
                | Self::Increased
                | Self::Decreased
                | Self::IncreasedBy(_)
                | Self::DecreasedBy(_)
        )
    }

    pub fn matches(
        self,
        current: ScalarValue,
        previous: Option<ScalarValue>,
        float_epsilon: f64,
    ) -> Result<bool, PredicateError> {
        if !float_epsilon.is_finite() || float_epsilon < 0.0 {
            return Err(PredicateError::InvalidFloatEpsilon(float_epsilon));
        }

        trace!(
            predicate = ?self,
            current = ?current,
            previous = ?previous,
            float_epsilon,
            "evaluating scan predicate"
        );

        match self {
            Self::Exact(expected) => current.equals(expected, float_epsilon),
            Self::NotEqual(expected) => Ok(!current.equals(expected, float_epsilon)?),
            Self::GreaterThan(expected) => {
                Ok(current.compare(expected)? == Some(Ordering::Greater))
            }
            Self::GreaterOrEqual(expected) => Ok(matches!(
                current.compare(expected)?,
                Some(Ordering::Greater | Ordering::Equal)
            )),
            Self::LessThan(expected) => Ok(current.compare(expected)? == Some(Ordering::Less)),
            Self::LessOrEqual(expected) => Ok(matches!(
                current.compare(expected)?,
                Some(Ordering::Less | Ordering::Equal)
            )),
            Self::BetweenInclusive { min, max } => {
                let range_order = min.compare(max)?;
                if !matches!(range_order, Some(Ordering::Less | Ordering::Equal)) {
                    return Err(PredicateError::InvalidRange);
                }
                let lower = current.compare(min)?;
                let upper = current.compare(max)?;
                Ok(matches!(lower, Some(Ordering::Greater | Ordering::Equal))
                    && matches!(upper, Some(Ordering::Less | Ordering::Equal)))
            }
            Self::Changed => {
                let previous = previous.ok_or(PredicateError::MissingPrevious)?;
                Ok(!current.equals(previous, float_epsilon)?)
            }
            Self::Unchanged => {
                let previous = previous.ok_or(PredicateError::MissingPrevious)?;
                current.equals(previous, float_epsilon)
            }
            Self::Increased => {
                let previous = previous.ok_or(PredicateError::MissingPrevious)?;
                Ok(current.compare(previous)? == Some(Ordering::Greater))
            }
            Self::Decreased => {
                let previous = previous.ok_or(PredicateError::MissingPrevious)?;
                Ok(current.compare(previous)? == Some(Ordering::Less))
            }
            Self::IncreasedBy(delta) => {
                let previous = previous.ok_or(PredicateError::MissingPrevious)?;
                difference_matches(current, previous, delta, float_epsilon, true)
            }
            Self::DecreasedBy(delta) => {
                let previous = previous.ok_or(PredicateError::MissingPrevious)?;
                difference_matches(current, previous, delta, float_epsilon, false)
            }
        }
    }
}

fn difference_matches(
    current: ScalarValue,
    previous: ScalarValue,
    delta: ScalarValue,
    epsilon: f64,
    increasing: bool,
) -> Result<bool, PredicateError> {
    if !delta.is_non_negative() {
        return Err(PredicateError::InvalidDelta);
    }

    match (current, previous, delta) {
        (ScalarValue::Signed(current), ScalarValue::Signed(previous), ScalarValue::Signed(delta)) => {
            let expected = if increasing {
                previous.checked_add(delta)
            } else {
                previous.checked_sub(delta)
            };
            Ok(expected == Some(current))
        }
        (
            ScalarValue::Unsigned(current),
            ScalarValue::Unsigned(previous),
            ScalarValue::Unsigned(delta),
        ) => {
            let expected = if increasing {
                previous.checked_add(delta)
            } else {
                previous.checked_sub(delta)
            };
            Ok(expected == Some(current))
        }
        (ScalarValue::Float(current), ScalarValue::Float(previous), ScalarValue::Float(delta)) => {
            let expected = if increasing {
                previous + delta
            } else {
                previous - delta
            };
            Ok(float_eq(current, expected, epsilon))
        }
        (current, previous, delta) => Err(PredicateError::TypeMismatch {
            left: current.kind(),
            right: if previous.kind() != current.kind() {
                previous.kind()
            } else {
                delta.kind()
            },
        }),
    }
}

fn type_mismatch(left: ScalarValue, right: ScalarValue) -> PredicateError {
    PredicateError::TypeMismatch {
        left: left.kind(),
        right: right.kind(),
    }
}

fn float_eq(left: f64, right: f64, epsilon: f64) -> bool {
    left == right || (left.is_finite() && right.is_finite() && (left - right).abs() <= epsilon)
}

#[derive(Debug, Error, PartialEq)]
pub enum PredicateError {
    #[error("this predicate requires a previous scan value")]
    MissingPrevious,
    #[error("cannot compare {left} values with {right} values")]
    TypeMismatch {
        left: &'static str,
        right: &'static str,
    },
    #[error("between-range minimum must be less than or equal to maximum")]
    InvalidRange,
    #[error("increase/decrease delta must be finite and non-negative")]
    InvalidDelta,
    #[error("float epsilon must be finite and non-negative, got {0}")]
    InvalidFloatEpsilon(f64),
}
