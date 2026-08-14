use intimatr::scanner::{PredicateError, ScalarValue, ScanPredicate};

const EPSILON: f64 = 1.0e-6;

#[test]
fn direct_numeric_predicates_match() {
    let current = ScalarValue::Signed(100);

    assert!(
        ScanPredicate::Exact(ScalarValue::Signed(100))
            .matches(current, None, EPSILON)
            .unwrap()
    );
    assert!(
        ScanPredicate::GreaterThan(ScalarValue::Signed(99))
            .matches(current, None, EPSILON)
            .unwrap()
    );
    assert!(
        ScanPredicate::LessOrEqual(ScalarValue::Signed(100))
            .matches(current, None, EPSILON)
            .unwrap()
    );
    assert!(
        ScanPredicate::BetweenInclusive {
            min: ScalarValue::Signed(90),
            max: ScalarValue::Signed(110),
        }
        .matches(current, None, EPSILON)
        .unwrap()
    );
}

#[test]
fn history_predicates_match_changed_increased_and_decreased() {
    assert!(
        ScanPredicate::Changed
            .matches(
                ScalarValue::Unsigned(11),
                Some(ScalarValue::Unsigned(10)),
                EPSILON,
            )
            .unwrap()
    );
    assert!(
        ScanPredicate::Increased
            .matches(
                ScalarValue::Unsigned(11),
                Some(ScalarValue::Unsigned(10)),
                EPSILON,
            )
            .unwrap()
    );
    assert!(
        ScanPredicate::Decreased
            .matches(
                ScalarValue::Signed(-2),
                Some(ScalarValue::Signed(5)),
                EPSILON,
            )
            .unwrap()
    );
}

#[test]
fn unchanged_float_uses_configurable_epsilon() {
    let current = ScalarValue::Float(10.000_000_4);
    let previous = Some(ScalarValue::Float(10.0));

    assert!(
        ScanPredicate::Unchanged
            .matches(current, previous, 1.0e-6)
            .unwrap()
    );
    assert!(
        !ScanPredicate::Unchanged
            .matches(current, previous, 1.0e-8)
            .unwrap()
    );
}

#[test]
fn increased_by_and_decreased_by_match_exact_deltas() {
    assert!(
        ScanPredicate::IncreasedBy(ScalarValue::Signed(5))
            .matches(
                ScalarValue::Signed(25),
                Some(ScalarValue::Signed(20)),
                EPSILON,
            )
            .unwrap()
    );
    assert!(
        ScanPredicate::DecreasedBy(ScalarValue::Unsigned(3))
            .matches(
                ScalarValue::Unsigned(17),
                Some(ScalarValue::Unsigned(20)),
                EPSILON,
            )
            .unwrap()
    );
}

#[test]
fn history_predicates_require_previous_value() {
    let error = ScanPredicate::Changed
        .matches(ScalarValue::Signed(1), None, EPSILON)
        .expect_err("changed requires history");

    assert_eq!(error, PredicateError::MissingPrevious);
}

#[test]
fn mixed_numeric_kinds_are_rejected() {
    let error = ScanPredicate::GreaterThan(ScalarValue::Unsigned(10))
        .matches(ScalarValue::Signed(11), None, EPSILON)
        .expect_err("mixed numeric kinds should not compare implicitly");

    assert!(matches!(error, PredicateError::TypeMismatch { .. }));
}

#[test]
fn invalid_range_is_rejected() {
    let error = ScanPredicate::BetweenInclusive {
        min: ScalarValue::Signed(10),
        max: ScalarValue::Signed(1),
    }
    .matches(ScalarValue::Signed(5), None, EPSILON)
    .expect_err("backwards range should fail");

    assert_eq!(error, PredicateError::InvalidRange);
}

#[test]
fn negative_delta_is_rejected() {
    let error = ScanPredicate::IncreasedBy(ScalarValue::Signed(-1))
        .matches(
            ScalarValue::Signed(11),
            Some(ScalarValue::Signed(10)),
            EPSILON,
        )
        .expect_err("negative increase delta should fail");

    assert_eq!(error, PredicateError::InvalidDelta);
}
