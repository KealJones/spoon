//! Type predicates, scalar conversions, and the two control intrinsics.
//!
//! These thirteen operations decide what every learned procedure believes about
//! the shape of its own data. A predicate that is not total, or a conversion
//! that quietly produces a different number than it was given, corrupts
//! knowledge rather than failing it. So the properties pinned here are
//! totality, mutual consistency, and the exact behavior at the ugly edges:
//! non-finite floats, magnitudes outside `i64`, and text that only looks
//! numeric.

use std::collections::BTreeMap;

use spoon_core::{BinOp, Expr, IntrinsicOp, Value};
use spoon_exec::{Env, Evaluator, SpoonError};

fn intrinsic(op: IntrinsicOp, args: Vec<Expr>) -> Expr {
    Expr::Intrinsic {
        version: 1,
        op,
        args,
    }
}

fn lit(value: Value) -> Expr {
    Expr::Literal(value)
}

fn text(value: &str) -> Value {
    Value::Text(value.to_string())
}

fn eval(expr: &Expr) -> Result<Value, SpoonError> {
    Evaluator::new().eval(expr, &mut Env::new())
}

fn apply1(op: IntrinsicOp, value: Value) -> Result<Value, SpoonError> {
    eval(&intrinsic(op, vec![lit(value)]))
}

fn apply2(op: IntrinsicOp, first: Value, second: Value) -> Result<Value, SpoonError> {
    eval(&intrinsic(op, vec![lit(first), lit(second)]))
}

/// Apply a single-argument intrinsic that is expected to be total.
fn ok1(op: IntrinsicOp, value: Value) -> Value {
    apply1(op, value.clone())
        .unwrap_or_else(|error| panic!("{op:?} on {value:?} must not fail: {error}"))
}

fn ok2(op: IntrinsicOp, first: Value, second: Value) -> Value {
    apply2(op, first.clone(), second.clone())
        .unwrap_or_else(|error| panic!("{op:?} on {first:?}, {second:?} must not fail: {error}"))
}

/// One inhabitant of every `Value` variant.
///
/// The predicates below are asserted against this whole table rather than
/// against remembered cases, because totality is the property under test: a
/// predicate that errors on one variant is broken even if it is right about the
/// other six.
fn every_value_variant() -> Vec<Value> {
    vec![
        Value::Null,
        Value::Bool(true),
        Value::Int(7),
        Value::Float(1.5),
        text("7"),
        Value::List(vec![Value::Int(1)]),
        Value::Map(BTreeMap::from([("key".to_string(), Value::Int(1))])),
    ]
}

/// The seven predicates that each recognize exactly one variant, paired with
/// the `Value::type_name()` they are expected to agree with.
fn single_type_predicates() -> [(IntrinsicOp, &'static str); 7] {
    [
        (IntrinsicOp::IsNull, "null"),
        (IntrinsicOp::IsBool, "bool"),
        (IntrinsicOp::IsInt, "int"),
        (IntrinsicOp::IsFloat, "float"),
        (IntrinsicOp::IsText, "text"),
        (IntrinsicOp::IsList, "list"),
        (IntrinsicOp::IsMap, "map"),
    ]
}

#[test]
fn each_single_type_predicate_answers_true_for_exactly_its_own_variant() {
    for value in every_value_variant() {
        for (op, recognized) in single_type_predicates() {
            let expected = Value::Bool(value.type_name() == recognized);
            assert_eq!(
                ok1(op, value.clone()),
                expected,
                "{op:?} disagrees with type_name() on {value:?}"
            );
        }
    }
}

#[test]
fn exactly_one_single_type_predicate_holds_for_every_value_variant() {
    // Mutual consistency is what lets a procedure branch on type at all. Two
    // predicates answering true, or none answering true, would let a chain of
    // type tests fall through to the wrong arm.
    for value in every_value_variant() {
        let holding: Vec<IntrinsicOp> = single_type_predicates()
            .into_iter()
            .filter(|(op, _)| ok1(*op, value.clone()) == Value::Bool(true))
            .map(|(op, _)| op)
            .collect();
        assert_eq!(
            holding.len(),
            1,
            "{value:?} is recognized by {holding:?} rather than by exactly one predicate"
        );
    }
}

#[test]
fn every_predicate_answers_null_without_erroring_and_only_is_null_accepts_it() {
    assert_eq!(ok1(IntrinsicOp::IsNull, Value::Null), Value::Bool(true));
    for op in [
        IntrinsicOp::IsBool,
        IntrinsicOp::IsInt,
        IntrinsicOp::IsFloat,
        IntrinsicOp::IsText,
        IntrinsicOp::IsList,
        IntrinsicOp::IsMap,
        IntrinsicOp::IsNumeric,
    ] {
        assert_eq!(
            ok1(op, Value::Null),
            Value::Bool(false),
            "{op:?} must answer false for null rather than erroring"
        );
    }
}

#[test]
fn is_int_and_is_float_test_the_representation_rather_than_the_numeric_value() {
    // 2.0 is a whole number in the mathematical sense, so this pins which
    // question the predicate answers. A procedure using is_int as a proxy for
    // "whole number" is wrong, and this is the assertion that says so.
    assert_eq!(
        ok1(IntrinsicOp::IsInt, Value::Float(2.0)),
        Value::Bool(false)
    );
    assert_eq!(ok1(IntrinsicOp::IsFloat, Value::Int(2)), Value::Bool(false));
    assert_eq!(
        ok1(IntrinsicOp::IsFloat, Value::Float(2.0)),
        Value::Bool(true)
    );
    assert_eq!(ok1(IntrinsicOp::IsInt, Value::Int(2)), Value::Bool(true));
}

#[test]
fn is_numeric_covers_both_number_representations_and_rejects_numeric_looking_text() {
    assert_eq!(
        ok1(IntrinsicOp::IsNumeric, Value::Int(7)),
        Value::Bool(true)
    );
    assert_eq!(
        ok1(IntrinsicOp::IsNumeric, Value::Float(0.5)),
        Value::Bool(true)
    );
    // Text that would parse as a number is still text. Answering true here
    // would turn is_numeric into a claim about parseability, which no caller
    // could then use for dispatch.
    assert_eq!(ok1(IntrinsicOp::IsNumeric, text("42")), Value::Bool(false));
    // Bools convert to numbers but are not numbers.
    assert_eq!(
        ok1(IntrinsicOp::IsNumeric, Value::Bool(true)),
        Value::Bool(false)
    );
}

#[test]
fn is_numeric_is_exactly_the_union_of_is_int_and_is_float_on_every_variant() {
    for value in every_value_variant() {
        let is_int = ok1(IntrinsicOp::IsInt, value.clone()) == Value::Bool(true);
        let is_float = ok1(IntrinsicOp::IsFloat, value.clone()) == Value::Bool(true);
        assert_eq!(
            ok1(IntrinsicOp::IsNumeric, value.clone()),
            Value::Bool(is_int || is_float),
            "is_numeric must be is_int or is_float for {value:?}"
        );
    }
}

#[test]
fn to_int_truncates_a_fractional_float_toward_zero_rather_than_rounding() {
    // Truncation is lossy but deliberate: numeric_round and numeric_truncate
    // exist for callers who want to choose. What matters is that the direction
    // is the same on both sides of zero.
    assert_eq!(ok1(IntrinsicOp::ToInt, Value::Float(2.9)), Value::Int(2));
    assert_eq!(ok1(IntrinsicOp::ToInt, Value::Float(-2.9)), Value::Int(-2));
}

#[test]
fn to_int_rejects_a_float_too_large_for_i64_instead_of_saturating_silently() {
    // A saturating cast turns an out-of-range magnitude into i64::MAX, which is
    // a plausible looking number that is not the input. numeric_rounding
    // reports this class of failure instead of inventing a value, and a
    // conversion has even less license to guess.
    let accepted: Vec<(f64, Value)> = [1e30_f64, -1e30_f64]
        .into_iter()
        .filter_map(|value| {
            apply1(IntrinsicOp::ToInt, Value::Float(value))
                .ok()
                .map(|converted| (value, converted))
        })
        .collect();
    assert!(
        accepted.is_empty(),
        "to_int returned a number that is not its input: {accepted:?}"
    );
}

#[test]
fn to_int_rejects_non_finite_floats() {
    // to_text, parse_float, and numeric_rounding all refuse non-finite floats,
    // so a conversion that maps NaN to a specific integer is inconsistent with
    // the rest of the vocabulary and hides the failure from the caller.
    let accepted: Vec<(f64, Value)> = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY]
        .into_iter()
        .filter_map(|value| {
            apply1(IntrinsicOp::ToInt, Value::Float(value))
                .ok()
                .map(|converted| (value, converted))
        })
        .collect();
    assert!(
        accepted.is_empty(),
        "to_int invented an integer for a non-finite float: {accepted:?}"
    );
}

#[test]
fn to_int_parses_text_only_after_trimming_and_only_in_plain_decimal_form() {
    assert_eq!(ok1(IntrinsicOp::ToInt, text("  42  ")), Value::Int(42));
    assert_eq!(ok1(IntrinsicOp::ToInt, text("-42")), Value::Int(-42));

    // Hex, empty, fractional, and separator-bearing text are all rejected
    // rather than guessed at, which is what lets a caller branch on failure.
    for rejected in ["0x1f", "1f", "", "   ", "42.5", "1_000"] {
        let Err(error) = apply1(IntrinsicOp::ToInt, text(rejected)) else {
            panic!("to_int must reject {rejected:?}");
        };
        assert!(
            error.to_string().contains("to_int: cannot parse"),
            "unexpected error for {rejected:?}: {error}"
        );
    }
}

#[test]
fn to_int_maps_bool_onto_one_and_zero() {
    assert_eq!(ok1(IntrinsicOp::ToInt, Value::Bool(true)), Value::Int(1));
    assert_eq!(ok1(IntrinsicOp::ToInt, Value::Bool(false)), Value::Int(0));
}

#[test]
fn to_int_reports_a_type_error_for_values_with_no_integer_meaning() {
    for value in [
        Value::Null,
        Value::List(vec![Value::Int(1)]),
        Value::Map(BTreeMap::new()),
    ] {
        let type_name = value.type_name();
        let Err(error) = apply1(IntrinsicOp::ToInt, value.clone()) else {
            panic!("to_int must reject {value:?}");
        };
        assert!(
            matches!(
                &error,
                SpoonError::TypeError { expected, got }
                    if expected.as_str() == "int, float, text, or bool"
                        && got.as_str() == type_name
            ),
            "unexpected error for {value:?}: {error:?}"
        );
    }
}

#[test]
fn to_float_widens_an_int_while_silently_losing_precision_beyond_the_f64_mantissa() {
    assert_eq!(ok1(IntrinsicOp::ToFloat, Value::Int(3)), Value::Float(3.0));

    // i64::MAX has no exact f64 representation. The conversion still succeeds,
    // so two distinct integers land on the same float with no diagnostic.
    // Anything doing identity or equality work on large ids must not route
    // them through to_float.
    let largest = ok1(IntrinsicOp::ToFloat, Value::Int(i64::MAX));
    let one_less = ok1(IntrinsicOp::ToFloat, Value::Int(i64::MAX - 1));
    assert_eq!(largest, Value::Float(9_223_372_036_854_775_808.0));
    assert_eq!(
        largest, one_less,
        "two different i64 values collapse onto one f64 and the conversion does not say so"
    );
}

#[test]
fn to_float_parses_trimmed_decimal_and_exponent_text_and_rejects_the_rest() {
    assert_eq!(ok1(IntrinsicOp::ToFloat, text(" 1.5 ")), Value::Float(1.5));
    assert_eq!(
        ok1(IntrinsicOp::ToFloat, text("1e5")),
        Value::Float(100_000.0)
    );
    assert_eq!(ok1(IntrinsicOp::ToFloat, text("7")), Value::Float(7.0));

    for rejected in ["", "abc", "0x1f"] {
        let Err(error) = apply1(IntrinsicOp::ToFloat, text(rejected)) else {
            panic!("to_float must reject {rejected:?}");
        };
        assert!(
            error.to_string().contains("to_float: cannot parse"),
            "unexpected error for {rejected:?}: {error}"
        );
    }
}

#[test]
fn to_float_rejects_text_that_names_a_non_finite_float() {
    // parse_float filters non-finite results out and to_text refuses to print
    // them, so text must not be a back door that puts NaN or infinity into the
    // value space where a later operation fails far from the cause.
    let accepted: Vec<(&str, Value)> = ["NaN", "nan", "inf", "infinity", "-inf"]
        .into_iter()
        .filter_map(|rejected| {
            apply1(IntrinsicOp::ToFloat, text(rejected))
                .ok()
                .map(|converted| (rejected, converted))
        })
        .collect();
    assert!(
        accepted.is_empty(),
        "to_float admitted non-finite floats into the value space: {accepted:?}"
    );
}

#[test]
fn to_float_maps_bool_onto_one_and_zero_and_reports_a_type_error_for_null() {
    assert_eq!(
        ok1(IntrinsicOp::ToFloat, Value::Bool(true)),
        Value::Float(1.0)
    );
    assert_eq!(
        ok1(IntrinsicOp::ToFloat, Value::Bool(false)),
        Value::Float(0.0)
    );

    let Err(error) = apply1(IntrinsicOp::ToFloat, Value::Null) else {
        panic!("to_float must reject null");
    };
    assert!(
        matches!(
            &error,
            SpoonError::TypeError { expected, got }
                if expected.as_str() == "int, float, text, or bool" && got.as_str() == "null"
        ),
        "unexpected error for null: {error:?}"
    );
}

#[test]
fn to_bool_applies_emptiness_truthiness_and_never_parses_text() {
    // This is the truthiness rule the whole system inherits, since the same
    // question decides which branch If takes. parse_bool is the operation for
    // reading the words "true" and "false"; to_bool only asks whether a value
    // is empty or zero, which is why the string "false" comes back true.
    let cases = [
        (Value::Null, false),
        (Value::Bool(true), true),
        (Value::Bool(false), false),
        (Value::Int(0), false),
        (Value::Int(-1), true),
        (Value::Int(1), true),
        (Value::Float(0.0), false),
        (Value::Float(-0.0), false),
        (Value::Float(0.1), true),
        (text(""), false),
        (text("false"), true),
        (text("FALSE"), true),
        (text("true"), true),
        (text("TRUE"), true),
        (text("yes"), true),
        (text("0"), true),
        (Value::List(vec![]), false),
        (Value::List(vec![Value::Null]), true),
        (Value::Map(BTreeMap::new()), false),
        (
            Value::Map(BTreeMap::from([("a".to_string(), Value::Null)])),
            true,
        ),
    ];

    for (value, expected) in cases {
        assert_eq!(
            ok1(IntrinsicOp::ToBool, value.clone()),
            Value::Bool(expected),
            "to_bool({value:?}) must be {expected}"
        );
    }
}

#[test]
fn to_bool_agrees_with_the_branch_taken_by_if_for_every_value_variant() {
    // eval.rs carries its own is_truthy while Value carries truthy(), and If
    // uses the latter. If the two ever drift, a procedure could store
    // to_bool(x) and then branch on x and reach the other arm.
    for value in every_value_variant() {
        let converted = ok1(IntrinsicOp::ToBool, value.clone());
        let branch = eval(&Expr::If {
            cond: Box::new(lit(value.clone())),
            then: Box::new(lit(Value::Bool(true))),
            else_: Box::new(lit(Value::Bool(false))),
        })
        .expect("If accepts any condition value");
        assert_eq!(converted, branch, "to_bool and If disagree about {value:?}");
    }
}

#[test]
fn default_if_null_substitutes_only_for_null_and_passes_every_other_value_through() {
    assert_eq!(
        ok2(IntrinsicOp::DefaultIfNull, Value::Null, Value::Int(9)),
        Value::Int(9)
    );
    // Falsy but present values are not null, so they survive. A default that
    // also replaced 0 or "" would be a different and far more surprising
    // operation, and the distinction is exactly what makes this op safe to put
    // in front of optional data.
    for present in [
        Value::Int(0),
        text(""),
        Value::Bool(false),
        Value::List(vec![]),
    ] {
        assert_eq!(
            ok2(IntrinsicOp::DefaultIfNull, present.clone(), Value::Int(9)),
            present.clone(),
            "default_if_null replaced the non-null value {present:?}"
        );
    }
}

#[test]
fn default_if_null_returns_a_null_default_unchanged_rather_than_failing() {
    assert_eq!(
        ok2(IntrinsicOp::DefaultIfNull, Value::Null, Value::Null),
        Value::Null
    );
}

#[test]
fn default_if_null_evaluates_its_default_eagerly_so_a_failing_default_fails_the_whole_call() {
    // Intrinsic arguments are all evaluated before the operation runs, so this
    // does not short circuit the way a null-coalescing operator would. A
    // fallback that can itself fail takes down the call even when the primary
    // value is present, which is the trap this test exists to document.
    let failing_default = Expr::BinOp {
        op: BinOp::Div,
        left: Box::new(lit(Value::Int(1))),
        right: Box::new(lit(Value::Int(0))),
    };
    let expression = intrinsic(
        IntrinsicOp::DefaultIfNull,
        vec![lit(Value::Int(5)), failing_default],
    );

    assert!(
        matches!(eval(&expression), Err(SpoonError::DivisionByZero)),
        "the unused default must have been evaluated for this to be an error"
    );
}

#[test]
fn assert_returns_the_asserted_value_when_it_is_truthy() {
    // Returning the value rather than a bool lets assert sit inline in a
    // pipeline without changing the data flowing through it.
    assert_eq!(
        ok2(IntrinsicOp::Assert, Value::Int(3), text("must be present")),
        Value::Int(3)
    );
    assert_eq!(
        ok2(IntrinsicOp::Assert, Value::Bool(true), text("flag")),
        Value::Bool(true)
    );
}

#[test]
fn assert_fails_with_its_message_for_every_falsy_value() {
    // Assert uses the same truthiness rule as to_bool, so an empty list trips
    // it just as null does.
    for falsy in [
        Value::Null,
        Value::Bool(false),
        Value::Int(0),
        text(""),
        Value::List(vec![]),
        Value::Map(BTreeMap::new()),
    ] {
        let Err(error) = apply2(
            IntrinsicOp::Assert,
            falsy.clone(),
            text("balance must exist"),
        ) else {
            panic!("assert must fail for {falsy:?}");
        };
        assert_eq!(
            error.to_string(),
            "assertion failed: balance must exist",
            "the message must reach the caller for {falsy:?}"
        );
    }
}

#[test]
fn assert_requires_a_text_message_even_when_the_assertion_would_pass() {
    // The message is validated before the condition, so a non-text message is
    // a type error rather than a quietly accepted passing assertion.
    let Err(error) = apply2(IntrinsicOp::Assert, Value::Bool(true), Value::Int(1)) else {
        panic!("assert must reject a non-text message");
    };
    assert!(
        matches!(
            &error,
            SpoonError::TypeError { expected, got }
                if expected.as_str() == "text" && got.as_str() == "int"
        ),
        "unexpected error: {error:?}"
    );
}

#[test]
fn single_argument_intrinsics_reject_any_other_argument_count() {
    let ops = [
        IntrinsicOp::IsNull,
        IntrinsicOp::IsBool,
        IntrinsicOp::IsInt,
        IntrinsicOp::IsFloat,
        IntrinsicOp::IsText,
        IntrinsicOp::IsList,
        IntrinsicOp::IsMap,
        IntrinsicOp::IsNumeric,
        IntrinsicOp::ToInt,
        IntrinsicOp::ToFloat,
        IntrinsicOp::ToBool,
    ];

    for op in ops {
        for args in [vec![], vec![lit(Value::Int(1)), lit(Value::Int(2))]] {
            let count = args.len();
            let Err(error) = eval(&intrinsic(op, args)) else {
                panic!("{op:?} must reject {count} arguments");
            };
            assert!(
                matches!(
                    &error,
                    SpoonError::ArityMismatch { expected, got, .. }
                        if *expected == 1 && *got == count
                ),
                "unexpected error for {op:?} with {count} arguments: {error:?}"
            );
        }
    }
}

#[test]
fn the_two_argument_control_intrinsics_reject_any_other_argument_count() {
    for op in [IntrinsicOp::Assert, IntrinsicOp::DefaultIfNull] {
        for args in [
            vec![],
            vec![lit(Value::Int(1))],
            vec![lit(Value::Int(1)), lit(Value::Int(2)), lit(Value::Int(3))],
        ] {
            let count = args.len();
            let Err(error) = eval(&intrinsic(op, args)) else {
                panic!("{op:?} must reject {count} arguments");
            };
            assert!(
                matches!(
                    &error,
                    SpoonError::ArityMismatch { expected, got, .. }
                        if *expected == 2 && *got == count
                ),
                "unexpected error for {op:?} with {count} arguments: {error:?}"
            );
        }
    }
}
