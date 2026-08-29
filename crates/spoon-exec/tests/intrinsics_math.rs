//! Behavioural coverage for the math, numeric-formatting, and bitwise
//! intrinsics.
//!
//! Learned procedures compose these operations, so every one of them is a
//! promise about what a procedure will compute. The tests below pin the
//! promise: a normal case with a hand-checkable value, the boundary where the
//! operation stops being total, and the failure the caller is supposed to see.
//!
//! Where the implementation disagrees with the finiteness policy the rest of
//! this evaluator enforces (`finite_numeric_float`, `numeric_pow`,
//! `numeric_rounding`, and the float-to-text conversion all reject non-finite
//! values), these tests assert the policy rather than the current behaviour.
//! A failing test here is a defect report, not a specification.

use spoon_core::{Expr, IntrinsicOp, Value};
use spoon_exec::{Env, Evaluator, SpoonError};

fn intrinsic(op: IntrinsicOp, args: Vec<Expr>) -> Expr {
    Expr::Intrinsic {
        version: 1,
        op,
        args,
    }
}

fn lit_int(n: i64) -> Expr {
    Expr::Literal(Value::Int(n))
}

fn lit_float(n: f64) -> Expr {
    Expr::Literal(Value::Float(n))
}

fn lit_text(text: &str) -> Expr {
    Expr::Literal(Value::Text(text.to_string()))
}

fn eval(expr: &Expr) -> Result<Value, SpoonError> {
    Evaluator::new().eval(expr, &mut Env::new())
}

fn call(op: IntrinsicOp, args: Vec<Expr>) -> Result<Value, SpoonError> {
    eval(&intrinsic(op, args))
}

/// `SpoonError` is not comparable, and an assertion message must never inline a
/// result that can be megabytes wide, which `numeric_to_fixed` with a large
/// precision can produce. Every assertion below reports through this.
fn brief(result: &Result<Value, SpoonError>) -> String {
    match result {
        Ok(Value::Text(text)) if text.len() > 64 => format!("Ok(Text of {} bytes)", text.len()),
        Ok(value) => format!("Ok({value:?})"),
        Err(error) => format!("Err({error})"),
    }
}

fn assert_int(result: &Result<Value, SpoonError>, expected: i64) {
    assert!(
        matches!(result, Ok(Value::Int(actual)) if *actual == expected),
        "expected Ok(Int({expected})), got {}",
        brief(result)
    );
}

fn assert_text(result: &Result<Value, SpoonError>, expected: &str) {
    assert!(
        matches!(result, Ok(Value::Text(actual)) if actual == expected),
        "expected Ok(Text({expected:?})), got {}",
        brief(result)
    );
}

fn assert_bool(result: &Result<Value, SpoonError>, expected: bool) {
    assert!(
        matches!(result, Ok(Value::Bool(actual)) if *actual == expected),
        "expected Ok(Bool({expected})), got {}",
        brief(result)
    );
}

/// Transcendental results come from the platform libm, so the last bit is not
/// portable. 1e-12 relative error is orders of magnitude looser than any real
/// libm error and still far tighter than any mistake in the operation itself.
const TOLERANCE: f64 = 1e-12;

fn expect_float(result: &Result<Value, SpoonError>) -> f64 {
    match result {
        Ok(Value::Float(value)) => *value,
        other => panic!("expected a float, got {}", brief(other)),
    }
}

fn assert_close(result: &Result<Value, SpoonError>, expected: f64) {
    let actual = expect_float(result);
    // Scaling by the expected magnitude keeps the comparison meaningful for
    // results like 5e200 where an absolute epsilon would be meaningless.
    let scale = expected.abs().max(1.0);
    assert!(
        (actual - expected).abs() < TOLERANCE * scale,
        "expected {expected}, got {actual}"
    );
}

// -- Constants --

#[test]
fn math_pi_and_math_e_take_no_arguments_and_return_the_library_constants() {
    assert_close(&call(IntrinsicOp::MathPi, vec![]), std::f64::consts::PI);
    assert_close(&call(IntrinsicOp::MathE, vec![]), std::f64::consts::E);
}

#[test]
fn the_nullary_constants_reject_being_passed_an_argument() {
    // A procedure that writes `math_pi(1)` has a bug the evaluator should name,
    // and both implementations discard `args` outright.
    for op in [IntrinsicOp::MathPi, IntrinsicOp::MathE] {
        let result = call(op, vec![lit_int(1)]);
        assert!(
            matches!(
                result,
                Err(SpoonError::ArityMismatch {
                    expected: 0,
                    got: 1,
                    ..
                })
            ),
            "{op:?}: {}",
            brief(&result)
        );
    }
}

// -- Roots, logarithms, exponentials --

#[test]
fn math_sqrt_accepts_both_integers_and_floats() {
    assert_close(&call(IntrinsicOp::MathSqrt, vec![lit_int(9)]), 3.0);
    assert_close(&call(IntrinsicOp::MathSqrt, vec![lit_float(2.25)]), 1.5);
    assert_close(&call(IntrinsicOp::MathSqrt, vec![lit_float(0.0)]), 0.0);
}

#[test]
fn math_sqrt_of_a_negative_number_is_rejected_rather_than_returning_nan() {
    // The square root is undefined over the reals here. Every other numeric
    // operation in this evaluator refuses to hand back a non-finite float, and
    // a NaN leaking into a procedure is silent: it propagates through
    // arithmetic and only surfaces as a wrong answer much later.
    let result = call(IntrinsicOp::MathSqrt, vec![lit_float(-4.0)]);
    assert!(
        matches!(result, Err(SpoonError::InvalidNumber { .. })),
        "{}",
        brief(&result)
    );
}

#[test]
fn the_three_logarithms_agree_with_their_bases_on_exact_powers() {
    assert_close(&call(IntrinsicOp::MathLog, vec![lit_int(1)]), 0.0);
    assert_close(
        &call(IntrinsicOp::MathLog, vec![lit_float(std::f64::consts::E)]),
        1.0,
    );
    assert_close(&call(IntrinsicOp::MathLog10, vec![lit_int(1000)]), 3.0);
    assert_close(&call(IntrinsicOp::MathLog2, vec![lit_int(1024)]), 10.0);
}

#[test]
fn logarithms_of_zero_are_rejected_rather_than_returning_negative_infinity() {
    // Zero is the pole of every logarithm. Returning -inf makes a divergent
    // input indistinguishable from a very small one further downstream.
    for op in [
        IntrinsicOp::MathLog,
        IntrinsicOp::MathLog10,
        IntrinsicOp::MathLog2,
    ] {
        let result = call(op, vec![lit_float(0.0)]);
        assert!(
            matches!(result, Err(SpoonError::InvalidNumber { .. })),
            "{op:?} of zero: {}",
            brief(&result)
        );
    }
}

#[test]
fn logarithms_of_negative_numbers_are_rejected_rather_than_returning_nan() {
    for op in [
        IntrinsicOp::MathLog,
        IntrinsicOp::MathLog10,
        IntrinsicOp::MathLog2,
    ] {
        let result = call(op, vec![lit_float(-1.0)]);
        assert!(
            matches!(result, Err(SpoonError::InvalidNumber { .. })),
            "{op:?} of a negative: {}",
            brief(&result)
        );
    }
}

#[test]
fn math_exp_inverts_math_log_on_ordinary_inputs() {
    assert_close(&call(IntrinsicOp::MathExp, vec![lit_int(0)]), 1.0);
    assert_close(
        &call(IntrinsicOp::MathExp, vec![lit_float(1.0)]),
        std::f64::consts::E,
    );
    assert_close(
        &call(IntrinsicOp::MathExp, vec![lit_float(-1.0)]),
        1.0 / std::f64::consts::E,
    );
}

#[test]
fn math_exp_beyond_the_float_range_is_rejected_rather_than_returning_infinity() {
    // f64 overflows above roughly exp(709.78). `numeric_pow` already returns
    // `InvalidNumber` for exactly this situation, so exponentiation under a
    // different name should not quietly saturate to infinity instead.
    let result = call(IntrinsicOp::MathExp, vec![lit_float(1000.0)]);
    assert!(
        matches!(result, Err(SpoonError::InvalidNumber { .. })),
        "{}",
        brief(&result)
    );
}

// -- Trigonometry --

#[test]
fn the_forward_trigonometric_functions_match_their_values_at_known_angles() {
    assert_close(&call(IntrinsicOp::MathSin, vec![lit_int(0)]), 0.0);
    assert_close(
        &call(
            IntrinsicOp::MathSin,
            vec![lit_float(std::f64::consts::FRAC_PI_6)],
        ),
        0.5,
    );
    assert_close(&call(IntrinsicOp::MathCos, vec![lit_int(0)]), 1.0);
    assert_close(
        &call(IntrinsicOp::MathCos, vec![lit_float(std::f64::consts::PI)]),
        -1.0,
    );
    assert_close(
        &call(
            IntrinsicOp::MathTan,
            vec![lit_float(std::f64::consts::FRAC_PI_4)],
        ),
        1.0,
    );
}

#[test]
fn math_tan_near_its_pole_stays_finite_because_the_pole_is_not_representable() {
    // tan diverges at exactly pi/2, but `FRAC_PI_2` is the nearest f64 to it
    // rather than the pole itself, so the result is merely enormous. Recorded
    // so that no error is expected here, unlike the undefined cases above.
    let result = call(
        IntrinsicOp::MathTan,
        vec![lit_float(std::f64::consts::FRAC_PI_2)],
    );
    let value = expect_float(&result);
    assert!(value.is_finite(), "tan(pi/2) produced {value}");
    assert!(
        value > 1e15,
        "tan near its pole should be huge, got {value}"
    );
}

#[test]
fn the_inverse_trigonometric_functions_invert_their_forward_counterparts() {
    assert_close(
        &call(IntrinsicOp::MathAsin, vec![lit_float(1.0)]),
        std::f64::consts::FRAC_PI_2,
    );
    assert_close(&call(IntrinsicOp::MathAsin, vec![lit_float(0.0)]), 0.0);
    assert_close(&call(IntrinsicOp::MathAcos, vec![lit_float(1.0)]), 0.0);
    assert_close(
        &call(IntrinsicOp::MathAcos, vec![lit_float(-1.0)]),
        std::f64::consts::PI,
    );
    assert_close(
        &call(IntrinsicOp::MathAtan, vec![lit_float(1.0)]),
        std::f64::consts::FRAC_PI_4,
    );
    assert_close(&call(IntrinsicOp::MathAtan, vec![lit_int(0)]), 0.0);
}

#[test]
fn arcsine_and_arccosine_outside_the_unit_interval_are_rejected_rather_than_returning_nan() {
    // The domain is [-1, 1]. An out-of-range argument is a caller error worth
    // naming, and the alternative is a NaN that spreads silently.
    for op in [IntrinsicOp::MathAsin, IntrinsicOp::MathAcos] {
        for argument in [lit_float(2.0), lit_float(-1.5)] {
            let result = call(op, vec![argument]);
            assert!(
                matches!(result, Err(SpoonError::InvalidNumber { .. })),
                "{op:?} out of domain: {}",
                brief(&result)
            );
        }
    }
}

#[test]
fn math_atan2_uses_the_signs_of_both_arguments_to_pick_the_quadrant() {
    assert_close(
        &call(IntrinsicOp::MathAtan2, vec![lit_float(1.0), lit_float(1.0)]),
        std::f64::consts::FRAC_PI_4,
    );
    // The single-argument arctangent cannot distinguish these two inputs: they
    // share a y/x ratio, and only atan2 places the second in quadrant three.
    assert_close(
        &call(
            IntrinsicOp::MathAtan2,
            vec![lit_float(-1.0), lit_float(-1.0)],
        ),
        -3.0 * std::f64::consts::FRAC_PI_4,
    );
    // Both arguments zero is the one input where atan2 is defined by fiat
    // rather than by limit, and IEEE 754 fixes it at zero.
    assert_close(
        &call(IntrinsicOp::MathAtan2, vec![lit_int(0), lit_int(0)]),
        0.0,
    );
}

#[test]
fn math_atan2_requires_exactly_two_arguments() {
    let result = call(IntrinsicOp::MathAtan2, vec![lit_float(1.0)]);
    assert!(
        matches!(
            result,
            Err(SpoonError::ArityMismatch {
                expected: 2,
                got: 1,
                ..
            })
        ),
        "{}",
        brief(&result)
    );
}

#[test]
fn the_unary_math_operations_reject_non_numeric_arguments() {
    for op in [
        IntrinsicOp::MathSqrt,
        IntrinsicOp::MathLog,
        IntrinsicOp::MathLog10,
        IntrinsicOp::MathLog2,
        IntrinsicOp::MathExp,
        IntrinsicOp::MathSin,
        IntrinsicOp::MathCos,
        IntrinsicOp::MathTan,
        IntrinsicOp::MathAsin,
        IntrinsicOp::MathAcos,
        IntrinsicOp::MathAtan,
    ] {
        let result = call(op, vec![lit_text("2")]);
        assert!(
            matches!(result, Err(SpoonError::TypeError { .. })),
            "{op:?} with text: {}",
            brief(&result)
        );
    }
}

#[test]
fn math_operations_reject_non_finite_inputs_the_way_the_rest_of_the_evaluator_does() {
    // `finite_numeric_float` exists precisely so a NaN or infinity cannot enter
    // a numeric operation. The math operations bypass it, which lets a
    // non-finite value produced elsewhere flow onwards unnoticed.
    for (op, args) in [
        (IntrinsicOp::MathSqrt, vec![lit_float(f64::NAN)]),
        (IntrinsicOp::MathSin, vec![lit_float(f64::INFINITY)]),
        (
            IntrinsicOp::MathHypot,
            vec![lit_float(f64::INFINITY), lit_float(1.0)],
        ),
        (
            IntrinsicOp::MathAtan2,
            vec![lit_float(f64::NAN), lit_float(1.0)],
        ),
    ] {
        let result = call(op, args);
        assert!(
            matches!(result, Err(SpoonError::InvalidNumber { .. })),
            "non-finite input to {op:?}: {}",
            brief(&result)
        );
    }
}

// -- Float predicates --

#[test]
fn math_is_nan_and_math_is_infinite_classify_the_kinds_of_special_float() {
    assert_bool(
        &call(IntrinsicOp::MathIsNan, vec![lit_float(f64::NAN)]),
        true,
    );
    assert_bool(&call(IntrinsicOp::MathIsNan, vec![lit_float(1.0)]), false);
    // NaN is not infinite and infinity is not NaN, so the two predicates are
    // independent rather than one being the negation of the other.
    assert_bool(
        &call(IntrinsicOp::MathIsNan, vec![lit_float(f64::INFINITY)]),
        false,
    );
    assert_bool(
        &call(IntrinsicOp::MathIsInfinite, vec![lit_float(f64::INFINITY)]),
        true,
    );
    assert_bool(
        &call(
            IntrinsicOp::MathIsInfinite,
            vec![lit_float(f64::NEG_INFINITY)],
        ),
        true,
    );
    assert_bool(
        &call(IntrinsicOp::MathIsInfinite, vec![lit_float(f64::NAN)]),
        false,
    );
    assert_bool(
        &call(IntrinsicOp::MathIsInfinite, vec![lit_float(1.0)]),
        false,
    );
}

#[test]
fn the_float_predicates_reject_integers_because_an_integer_is_never_special() {
    // These two operations match on `Value::Float` alone rather than going
    // through `numeric_to_f64`, so an integer is a type error rather than a
    // trivially false answer. Pinned here so the strictness cannot drift
    // silently in either direction.
    for op in [IntrinsicOp::MathIsNan, IntrinsicOp::MathIsInfinite] {
        let result = call(op, vec![lit_int(1)]);
        assert!(
            matches!(result, Err(SpoonError::TypeError { .. })),
            "{op:?}: {}",
            brief(&result)
        );
    }
}

// -- Integer number theory --

#[test]
fn math_gcd_ignores_sign_and_treats_zero_as_the_identity() {
    assert_int(
        &call(IntrinsicOp::MathGcd, vec![lit_int(12), lit_int(18)]),
        6,
    );
    // gcd is defined on magnitudes, so a negative operand cannot make the
    // result negative.
    assert_int(
        &call(IntrinsicOp::MathGcd, vec![lit_int(-12), lit_int(18)]),
        6,
    );
    assert_int(
        &call(IntrinsicOp::MathGcd, vec![lit_int(-12), lit_int(-18)]),
        6,
    );
    // Zero divides nothing, so gcd(n, 0) is n: the identity element.
    assert_int(
        &call(IntrinsicOp::MathGcd, vec![lit_int(12), lit_int(0)]),
        12,
    );
    assert_int(
        &call(IntrinsicOp::MathGcd, vec![lit_int(0), lit_int(12)]),
        12,
    );
    assert_int(&call(IntrinsicOp::MathGcd, vec![lit_int(0), lit_int(0)]), 0);
    // Coprime inputs are the case a caller most often branches on.
    assert_int(
        &call(IntrinsicOp::MathGcd, vec![lit_int(9), lit_int(28)]),
        1,
    );
}

#[test]
fn math_gcd_of_the_most_negative_integer_reports_overflow_rather_than_misbehaving() {
    // `i64::MIN.abs()` has no representable answer. The implementation takes
    // the absolute value unconditionally, so this either panics or, with
    // overflow checks compiled out, returns a negative "greatest common
    // divisor" that every caller will then reason about incorrectly.
    let result = call(IntrinsicOp::MathGcd, vec![lit_int(i64::MIN), lit_int(6)]);
    assert!(
        matches!(result, Err(SpoonError::ArithmeticOverflow { .. })),
        "{}",
        brief(&result)
    );
}

#[test]
fn math_lcm_returns_the_positive_least_common_multiple() {
    assert_int(
        &call(IntrinsicOp::MathLcm, vec![lit_int(4), lit_int(6)]),
        12,
    );
    // The magnitude is what matters, so a sign cannot leak into the result.
    assert_int(
        &call(IntrinsicOp::MathLcm, vec![lit_int(-4), lit_int(6)]),
        12,
    );
    assert_int(
        &call(IntrinsicOp::MathLcm, vec![lit_int(-4), lit_int(-6)]),
        12,
    );
    assert_int(&call(IntrinsicOp::MathLcm, vec![lit_int(7), lit_int(1)]), 7);
    // Coprime operands multiply out in full, which is the fastest way to
    // notice a gcd that came back wrong.
    assert_int(
        &call(IntrinsicOp::MathLcm, vec![lit_int(9), lit_int(28)]),
        252,
    );
}

#[test]
fn math_lcm_with_a_zero_operand_is_zero_including_when_both_are_zero() {
    // Zero is a multiple of everything, and the both-zero case would otherwise
    // divide by a gcd of zero, so it is special-cased in the implementation.
    assert_int(&call(IntrinsicOp::MathLcm, vec![lit_int(0), lit_int(7)]), 0);
    assert_int(&call(IntrinsicOp::MathLcm, vec![lit_int(7), lit_int(0)]), 0);
    assert_int(&call(IntrinsicOp::MathLcm, vec![lit_int(0), lit_int(0)]), 0);
}

#[test]
fn math_lcm_reports_overflow_instead_of_wrapping_past_the_integer_range() {
    let result = call(IntrinsicOp::MathLcm, vec![lit_int(i64::MAX), lit_int(2)]);
    assert!(
        matches!(result, Err(SpoonError::ArithmeticOverflow { .. })),
        "{}",
        brief(&result)
    );
}

#[test]
fn math_lcm_of_the_most_negative_integer_reports_overflow_rather_than_misbehaving() {
    // The same unrepresentable absolute value as gcd, reached through lcm's own
    // call into it before the overflow-checked multiply gets a chance to help.
    let result = call(IntrinsicOp::MathLcm, vec![lit_int(i64::MIN), lit_int(2)]);
    assert!(
        matches!(result, Err(SpoonError::ArithmeticOverflow { .. })),
        "{}",
        brief(&result)
    );
}

#[test]
fn the_integer_number_theory_operations_reject_floats() {
    // gcd and lcm are only meaningful over the integers, so a float is a type
    // error rather than something to truncate.
    for op in [IntrinsicOp::MathGcd, IntrinsicOp::MathLcm] {
        let result = call(op, vec![lit_float(4.0), lit_int(6)]);
        assert!(
            matches!(result, Err(SpoonError::TypeError { .. })),
            "{op:?}: {}",
            brief(&result)
        );
    }
}

#[test]
fn math_hypot_computes_the_euclidean_norm_without_intermediate_overflow() {
    assert_close(
        &call(IntrinsicOp::MathHypot, vec![lit_int(3), lit_int(4)]),
        5.0,
    );
    assert_close(
        &call(IntrinsicOp::MathHypot, vec![lit_float(0.0), lit_float(0.0)]),
        0.0,
    );
    // The norm is symmetric and sign-blind.
    assert_close(
        &call(
            IntrinsicOp::MathHypot,
            vec![lit_float(-3.0), lit_float(-4.0)],
        ),
        5.0,
    );
    // Squaring 3e200 would overflow f64; hypot is specified to avoid that, so
    // this distinguishes a real hypot from a naive sqrt(a*a + b*b).
    assert_close(
        &call(
            IntrinsicOp::MathHypot,
            vec![lit_float(3e200), lit_float(4e200)],
        ),
        5e200,
    );
}

// -- Numeric formatting --

#[test]
fn numeric_to_fixed_formats_with_exactly_the_requested_number_of_decimals() {
    assert_text(
        &call(
            IntrinsicOp::NumericToFixed,
            vec![lit_float(1.23456789), lit_int(3)],
        ),
        "1.235",
    );
    // Trailing zeros are padded rather than trimmed: the digit count is the
    // whole point of the operation.
    assert_text(
        &call(
            IntrinsicOp::NumericToFixed,
            vec![lit_float(2.5), lit_int(3)],
        ),
        "2.500",
    );
    // Integers are accepted and widened, which is what makes this usable for
    // formatting a quantity held as whole units.
    assert_text(
        &call(IntrinsicOp::NumericToFixed, vec![lit_int(5), lit_int(2)]),
        "5.00",
    );
    assert_text(
        &call(
            IntrinsicOp::NumericToFixed,
            vec![lit_float(-0.5), lit_int(1)],
        ),
        "-0.5",
    );
    // Zero decimals drops the point entirely rather than leaving a bare dot.
    assert_text(
        &call(
            IntrinsicOp::NumericToFixed,
            vec![lit_float(9.7), lit_int(0)],
        ),
        "10",
    );
}

#[test]
fn numeric_to_fixed_rejects_a_negative_digit_count() {
    let result = call(
        IntrinsicOp::NumericToFixed,
        vec![lit_float(1.5), lit_int(-1)],
    );
    assert!(
        matches!(result, Err(SpoonError::Other(_))),
        "{}",
        brief(&result)
    );
}

#[test]
fn numeric_to_fixed_rejects_a_digit_count_that_would_exceed_the_text_limit() {
    // Every other text-producing intrinsic calls `ensure_text` before handing
    // back a string, capping output at MAX_INTRINSIC_TEXT_BYTES. This one does
    // not, so a single call can allocate an arbitrarily large string from a
    // tiny expression and step around the limit that budget rests on. Worse,
    // `format!` carries its precision in a u16, so any digit count above 65535
    // aborts the whole process instead of failing the procedure.
    let result = call(
        IntrinsicOp::NumericToFixed,
        vec![lit_float(1.5), lit_int(2_000_000)],
    );
    assert!(
        matches!(result, Err(SpoonError::IntrinsicLimitExceeded { .. })),
        "{}",
        brief(&result)
    );
}

#[test]
fn numeric_to_fixed_rejects_a_non_finite_value_rather_than_formatting_it_as_a_word() {
    // Converting a float to text elsewhere in the evaluator refuses non-finite
    // input outright. Formatting "inf" or "NaN" here produces text that nothing
    // in this system will read back as a number.
    for value in [f64::NAN, f64::INFINITY] {
        let result = call(
            IntrinsicOp::NumericToFixed,
            vec![lit_float(value), lit_int(2)],
        );
        assert!(
            matches!(result, Err(SpoonError::InvalidNumber { .. })),
            "{}",
            brief(&result)
        );
    }
}

#[test]
fn numeric_to_hex_and_numeric_to_binary_render_integers_without_a_base_prefix() {
    assert_text(&call(IntrinsicOp::NumericToHex, vec![lit_int(255)]), "ff");
    assert_text(&call(IntrinsicOp::NumericToHex, vec![lit_int(0)]), "0");
    assert_text(
        &call(IntrinsicOp::NumericToHex, vec![lit_int(i64::MAX)]),
        "7fffffffffffffff",
    );
    assert_text(&call(IntrinsicOp::NumericToBinary, vec![lit_int(5)]), "101");
    assert_text(&call(IntrinsicOp::NumericToBinary, vec![lit_int(0)]), "0");
}

#[test]
fn numeric_from_hex_and_numeric_from_binary_accept_an_optional_base_prefix() {
    assert_int(
        &call(IntrinsicOp::NumericFromHex, vec![lit_text("ff")]),
        255,
    );
    assert_int(
        &call(IntrinsicOp::NumericFromHex, vec![lit_text("0xFF")]),
        255,
    );
    // The uppercase prefix is handled too, so a caller pasting from a hex dump
    // does not have to normalise first.
    assert_int(
        &call(IntrinsicOp::NumericFromHex, vec![lit_text("0XfF")]),
        255,
    );
    assert_int(
        &call(IntrinsicOp::NumericFromBinary, vec![lit_text("1010")]),
        10,
    );
    assert_int(
        &call(IntrinsicOp::NumericFromBinary, vec![lit_text("0b1010")]),
        10,
    );
    assert_int(
        &call(IntrinsicOp::NumericFromBinary, vec![lit_text("0B1010")]),
        10,
    );
}

#[test]
fn numeric_from_hex_and_numeric_from_binary_reject_empty_and_malformed_text() {
    for (op, text) in [
        // An empty string parses as nothing at all, not as zero.
        (IntrinsicOp::NumericFromHex, ""),
        // A bare prefix leaves an empty string behind once it is stripped.
        (IntrinsicOp::NumericFromHex, "0x"),
        (IntrinsicOp::NumericFromHex, "xyz"),
        // Surrounding whitespace is not trimmed, so it stays a parse failure.
        (IntrinsicOp::NumericFromHex, " ff"),
        (IntrinsicOp::NumericFromBinary, ""),
        (IntrinsicOp::NumericFromBinary, "0b"),
        // A digit outside the base is the mistake this operation exists to
        // catch, rather than reinterpreting the string in some other base.
        (IntrinsicOp::NumericFromBinary, "102"),
        (IntrinsicOp::NumericFromBinary, "ff"),
    ] {
        let result = call(op, vec![lit_text(text)]);
        assert!(
            matches!(result, Err(SpoonError::Other(_))),
            "{op:?} on {text:?}: {}",
            brief(&result)
        );
    }
}

#[test]
fn numeric_from_hex_rejects_a_value_that_overflows_an_integer() {
    // Seventeen hex digits cannot fit in an i64, and truncating quietly would
    // be worse than refusing.
    let result = call(
        IntrinsicOp::NumericFromHex,
        vec![lit_text("10000000000000000")],
    );
    assert!(
        matches!(result, Err(SpoonError::Other(_))),
        "{}",
        brief(&result)
    );
}

#[test]
fn hexadecimal_round_trips_through_negative_numbers() {
    // `numeric_to_hex` renders a negative as its two's-complement bit pattern
    // while `numeric_from_hex` parses sign-and-magnitude, so the pair disagrees
    // about what a negative number looks like and the round trip is lost.
    let hex = call(IntrinsicOp::NumericToHex, vec![lit_int(-5)]);
    let text = match &hex {
        Ok(Value::Text(text)) => text.clone(),
        other => panic!("expected text, got {}", brief(other)),
    };
    let round_tripped = call(IntrinsicOp::NumericFromHex, vec![lit_text(&text)]);
    assert!(
        matches!(&round_tripped, Ok(Value::Int(-5))),
        "numeric_to_hex(-5) produced {text:?}, and reading it back gave {}",
        brief(&round_tripped)
    );
}

#[test]
fn binary_round_trips_through_negative_numbers() {
    // The same mismatch as hexadecimal: the writer emits 64 two's-complement
    // bits and the reader rejects anything that does not fit a signed parse.
    let binary = call(IntrinsicOp::NumericToBinary, vec![lit_int(-5)]);
    let text = match &binary {
        Ok(Value::Text(text)) => text.clone(),
        other => panic!("expected text, got {}", brief(other)),
    };
    let round_tripped = call(IntrinsicOp::NumericFromBinary, vec![lit_text(&text)]);
    assert!(
        matches!(&round_tripped, Ok(Value::Int(-5))),
        "numeric_to_binary(-5) produced {} characters, and reading them back gave {}",
        text.len(),
        brief(&round_tripped)
    );
}

#[test]
fn the_numeric_formatting_operations_reject_wrong_argument_types_and_counts() {
    // The radix writers take integers; a float has no bit pattern to render.
    for op in [IntrinsicOp::NumericToHex, IntrinsicOp::NumericToBinary] {
        let result = call(op, vec![lit_float(255.0)]);
        assert!(
            matches!(result, Err(SpoonError::TypeError { .. })),
            "{op:?}: {}",
            brief(&result)
        );
    }
    // The radix readers take text; an integer has already been parsed.
    for op in [IntrinsicOp::NumericFromHex, IntrinsicOp::NumericFromBinary] {
        let result = call(op, vec![lit_int(255)]);
        assert!(
            matches!(result, Err(SpoonError::TypeError { .. })),
            "{op:?}: {}",
            brief(&result)
        );
    }
    let missing_precision = call(IntrinsicOp::NumericToFixed, vec![lit_float(1.5)]);
    assert!(
        matches!(
            missing_precision,
            Err(SpoonError::ArityMismatch {
                expected: 2,
                got: 1,
                ..
            })
        ),
        "{}",
        brief(&missing_precision)
    );
    let precision_as_text = call(
        IntrinsicOp::NumericToFixed,
        vec![lit_float(1.5), lit_text("2")],
    );
    assert!(
        matches!(precision_as_text, Err(SpoonError::TypeError { .. })),
        "{}",
        brief(&precision_as_text)
    );
}

// -- Bitwise --

#[test]
fn the_binary_bitwise_operations_combine_bit_patterns_position_by_position() {
    // 0b1100 against 0b1010 exercises all four input combinations at once.
    assert_int(
        &call(IntrinsicOp::BitAnd, vec![lit_int(0b1100), lit_int(0b1010)]),
        0b1000,
    );
    assert_int(
        &call(IntrinsicOp::BitOr, vec![lit_int(0b1100), lit_int(0b1010)]),
        0b1110,
    );
    assert_int(
        &call(IntrinsicOp::BitXor, vec![lit_int(0b1100), lit_int(0b1010)]),
        0b0110,
    );
}

#[test]
fn the_binary_bitwise_operations_treat_negatives_as_two_s_complement_bit_patterns() {
    // -1 is all ones, which makes it the identity for and and the annihilator
    // for or. Anything else would mean the sign bit is being special-cased.
    assert_int(&call(IntrinsicOp::BitAnd, vec![lit_int(-1), lit_int(5)]), 5);
    assert_int(&call(IntrinsicOp::BitOr, vec![lit_int(-1), lit_int(5)]), -1);
    // Exclusive-or against all ones is complement.
    assert_int(
        &call(IntrinsicOp::BitXor, vec![lit_int(-1), lit_int(5)]),
        -6,
    );
    // Exclusive-or is its own inverse, which is why it is used for toggling.
    assert_int(
        &call(IntrinsicOp::BitXor, vec![lit_int(-6), lit_int(-1)]),
        5,
    );
    // Masking off the sign bit of the minimum integer leaves nothing.
    assert_int(
        &call(
            IntrinsicOp::BitAnd,
            vec![lit_int(i64::MIN), lit_int(i64::MAX)],
        ),
        0,
    );
}

#[test]
fn bit_not_complements_every_bit_including_across_the_sign_boundary() {
    assert_int(&call(IntrinsicOp::BitNot, vec![lit_int(0)]), -1);
    assert_int(&call(IntrinsicOp::BitNot, vec![lit_int(5)]), -6);
    // Complementing a negative walks back the other way, so the operation is
    // an involution rather than something that saturates at zero.
    assert_int(&call(IntrinsicOp::BitNot, vec![lit_int(-1)]), 0);
    assert_int(&call(IntrinsicOp::BitNot, vec![lit_int(-6)]), 5);
    // The extremes are where an implementation that reached for negation
    // instead of complement would overflow.
    assert_int(
        &call(IntrinsicOp::BitNot, vec![lit_int(i64::MIN)]),
        i64::MAX,
    );
    assert_int(
        &call(IntrinsicOp::BitNot, vec![lit_int(i64::MAX)]),
        i64::MIN,
    );
}

#[test]
fn the_shifts_move_bits_by_the_requested_number_of_places() {
    assert_int(
        &call(IntrinsicOp::BitShiftLeft, vec![lit_int(1), lit_int(10)]),
        1024,
    );
    assert_int(
        &call(IntrinsicOp::BitShiftLeft, vec![lit_int(3), lit_int(0)]),
        3,
    );
    assert_int(
        &call(IntrinsicOp::BitShiftRight, vec![lit_int(1024), lit_int(3)]),
        128,
    );
    assert_int(
        &call(IntrinsicOp::BitShiftRight, vec![lit_int(3), lit_int(0)]),
        3,
    );
}

#[test]
fn shifting_right_preserves_the_sign_bit_and_shifting_left_keeps_bit_pattern_semantics() {
    // An arithmetic right shift keeps a negative negative, which is what makes
    // it a halving operation rather than a bit-window slide.
    assert_int(
        &call(IntrinsicOp::BitShiftRight, vec![lit_int(-8), lit_int(1)]),
        -4,
    );
    // Right-shifting a negative far enough saturates at -1, never at 0.
    assert_int(
        &call(IntrinsicOp::BitShiftRight, vec![lit_int(-1), lit_int(63)]),
        -1,
    );
    // Left shift is a bit-pattern operation, so shifting a one into the sign
    // position yields the minimum integer rather than an overflow error. It is
    // the one place in the numeric surface where a value change that large is
    // not reported, so it is pinned deliberately rather than by accident.
    assert_int(
        &call(IntrinsicOp::BitShiftLeft, vec![lit_int(1), lit_int(63)]),
        i64::MIN,
    );
}

#[test]
fn the_shifts_reject_a_distance_outside_the_width_of_an_integer() {
    // Rust would panic on a shift of 64 or more, and a negative distance has no
    // meaning, so both are refused before the shift happens.
    for op in [IntrinsicOp::BitShiftLeft, IntrinsicOp::BitShiftRight] {
        for bits in [64_i64, 100, i64::MAX, -1, i64::MIN] {
            let result = call(op, vec![lit_int(1), lit_int(bits)]);
            assert!(
                matches!(result, Err(SpoonError::Other(_))),
                "{op:?} by {bits}: {}",
                brief(&result)
            );
        }
    }
}

#[test]
fn the_bitwise_operations_reject_non_integer_arguments() {
    // Floats have no bit pattern the evaluator is willing to guess at, and a
    // float shift distance is the most likely accidental input.
    for (op, args) in [
        (IntrinsicOp::BitAnd, vec![lit_float(1.0), lit_int(1)]),
        (IntrinsicOp::BitOr, vec![lit_int(1), lit_float(1.0)]),
        (IntrinsicOp::BitXor, vec![lit_text("1"), lit_int(1)]),
        (IntrinsicOp::BitNot, vec![lit_float(1.0)]),
        (IntrinsicOp::BitShiftLeft, vec![lit_int(1), lit_float(2.0)]),
        (IntrinsicOp::BitShiftRight, vec![lit_float(8.0), lit_int(1)]),
    ] {
        let result = call(op, args);
        assert!(
            matches!(result, Err(SpoonError::TypeError { .. })),
            "{op:?}: {}",
            brief(&result)
        );
    }
}

#[test]
fn the_bitwise_operations_reject_the_wrong_number_of_arguments() {
    let unary_with_two = call(IntrinsicOp::BitNot, vec![lit_int(1), lit_int(2)]);
    assert!(
        matches!(
            unary_with_two,
            Err(SpoonError::ArityMismatch {
                expected: 1,
                got: 2,
                ..
            })
        ),
        "{}",
        brief(&unary_with_two)
    );
    for op in [
        IntrinsicOp::BitAnd,
        IntrinsicOp::BitOr,
        IntrinsicOp::BitXor,
        IntrinsicOp::BitShiftLeft,
        IntrinsicOp::BitShiftRight,
    ] {
        let result = call(op, vec![lit_int(1)]);
        assert!(
            matches!(
                result,
                Err(SpoonError::ArityMismatch {
                    expected: 2,
                    got: 1,
                    ..
                })
            ),
            "{op:?}: {}",
            brief(&result)
        );
    }
}
