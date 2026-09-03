//! Arithmetic, comparison, and logic, exercised end to end.
//!
//! Every test here goes through a real `Evaluator` against a real `Store`
//! seeded from the bootstrap registry. Calling the native functions directly
//! would prove they compute; it would not prove they are registered with the
//! right arity, seeded with a realization, or given the argument strategy that
//! makes `If` and `And` mean what they say. Those are the parts that break.

use spoon_concept::{Concept, Ground};
use spoon_eval::{
    Arity, Budget, Ctx, EvalResult, Evaluator, NativeRegistry, Outcome, native_error,
};
use spoon_store::Store;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Fails whenever it runs. Anything that reaches it leaves a mark in the trace,
/// which is how the short-circuit tests prove a branch was skipped rather than
/// merely surviving it.
fn n_boom(_ctx: &mut dyn Ctx, _args: &[Concept]) -> EvalResult {
    Err(native_error("boom", "this native always fails"))
}

/// Succeeds, but leaves a note. Used where the point is that an argument was
/// never reduced, not that reducing it would have failed.
fn n_spy(ctx: &mut dyn Ctx, _args: &[Concept]) -> EvalResult {
    ctx.note("spy ran");
    Ok(Concept::int(0))
}

/// The real bootstrap registry, plus two probes.
fn setup() -> (Store, NativeRegistry) {
    let mut registry = spoon_natives::bootstrap();
    registry.pure("boom", n_boom, Arity::Any, "always fails");
    registry.pure("spy", n_spy, Arity::Any, "notes that it ran");
    let store = Store::open_in_memory().unwrap();
    spoon_natives::seed_bootstrap(&store, &registry).unwrap();
    (store, registry)
}

fn evaluate(concept: Concept) -> Outcome {
    let (store, registry) = setup();
    let mut ev = Evaluator::new(&store, &registry).with_budget(Budget::deterministic());
    ev.evaluate(&concept)
}

/// Evaluate and insist on a value. Used by every happy-path assertion.
fn value(concept: Concept) -> Concept {
    let out = evaluate(concept);
    match out.value() {
        Some(v) => v.clone(),
        None => panic!("expected a value, got {out:?}"),
    }
}

fn int(v: i64) -> Concept {
    Concept::int(v)
}

fn float(v: f64) -> Concept {
    Concept::float(v)
}

fn yes() -> Concept {
    Concept::bool(true)
}

fn no() -> Concept {
    Concept::bool(false)
}

/// A named concept with no realization and no numeric meaning. Every native
/// that is handed one should decline rather than guess.
fn nonsense() -> Concept {
    Concept::named("nonsense")
}

fn boom() -> Concept {
    Concept::call("boom", [])
}

fn assert_errors(concept: Concept) {
    let out = evaluate(concept.clone());
    assert!(
        out.value().is_none(),
        "expected {concept:?} to fail, got {out:?}"
    );
}

// ---------------------------------------------------------------------------
// Arithmetic: happy paths
// ---------------------------------------------------------------------------

#[test]
fn the_arithmetic_natives_compute() {
    assert_eq!(value(Concept::call("add", [int(1), int(2)])), int(3));
    assert_eq!(
        value(Concept::call("add", [int(1), int(2), int(3), int(4)])),
        int(10),
        "add is variadic"
    );
    assert_eq!(value(Concept::call("sub", [int(10), int(3)])), int(7));
    assert_eq!(
        value(Concept::call("mul", [int(2), int(3), int(4)])),
        int(24)
    );
    assert_eq!(value(Concept::call("div", [int(9), int(3)])), int(3));
    assert_eq!(value(Concept::call("modulo", [int(7), int(3)])), int(1));
    assert_eq!(value(Concept::call("neg", [int(5)])), int(-5));
    assert_eq!(value(Concept::call("abs", [int(-5)])), int(5));
    assert_eq!(value(Concept::call("pow", [int(2), int(10)])), int(1024));
    assert_eq!(
        value(Concept::call("min", [int(3), int(1), int(2)])),
        int(1)
    );
    assert_eq!(
        value(Concept::call("max", [int(3), int(1), int(2)])),
        int(3)
    );
}

#[test]
fn the_float_paths_compute() {
    assert_eq!(
        value(Concept::call("add", [float(0.5), float(0.25)])),
        float(0.75)
    );
    assert_eq!(
        value(Concept::call("sub", [float(1.5), float(0.5)])),
        float(1.0)
    );
    assert_eq!(
        value(Concept::call("mul", [float(1.5), float(2.0)])),
        float(3.0)
    );
    assert_eq!(
        value(Concept::call("div", [float(7.0), float(2.0)])),
        float(3.5)
    );
    assert_eq!(
        value(Concept::call("modulo", [float(7.5), float(2.0)])),
        float(1.5)
    );
    assert_eq!(value(Concept::call("neg", [float(2.5)])), float(-2.5));
    assert_eq!(value(Concept::call("abs", [float(-2.5)])), float(2.5));
    assert_eq!(
        value(Concept::call("pow", [float(2.0), float(0.5)])),
        float(2.0f64.powf(0.5))
    );
    assert_eq!(
        value(Concept::call("min", [float(3.5), float(1.5)])),
        float(1.5)
    );
    assert_eq!(
        value(Concept::call("max", [float(3.5), float(1.5)])),
        float(3.5)
    );
}

#[test]
fn division_truncates_toward_zero_between_integers() {
    // Two integers produce an integer, so there is nowhere for the fraction to
    // go. Ask for a float operand when you want the fraction back.
    assert_eq!(value(Concept::call("div", [int(7), int(2)])), int(3));
    assert_eq!(value(Concept::call("div", [int(-7), int(2)])), int(-3));
    assert_eq!(
        value(Concept::call("div", [float(7.0), int(2)])),
        float(3.5)
    );
}

#[test]
fn modulo_takes_its_sign_from_the_dividend() {
    assert_eq!(value(Concept::call("modulo", [int(-7), int(3)])), int(-1));
    assert_eq!(value(Concept::call("modulo", [int(7), int(-3)])), int(1));
}

// ---------------------------------------------------------------------------
// The widening rule
// ---------------------------------------------------------------------------

#[test]
fn an_all_integer_call_stays_an_integer() {
    let out = value(Concept::call("add", [int(1), int(2)]));
    assert_eq!(out, int(3));
    assert_ne!(
        out,
        float(3.0),
        "3 and 3.0 are different concepts and must not be confused"
    );
    assert!(matches!(out.as_ground(), Some(Ground::Int(3))));
}

#[test]
fn any_float_widens_the_result() {
    for expr in [
        Concept::call("add", [int(1), float(2.0)]),
        Concept::call("add", [float(1.0), int(2)]),
        Concept::call("add", [int(1), int(2), float(0.0)]),
    ] {
        let out = value(expr.clone());
        assert_eq!(out, float(3.0), "{expr:?}");
        assert!(
            matches!(out.as_ground(), Some(Ground::Float(_))),
            "{expr:?}"
        );
    }
    // The widening survives an integer winning the comparison: Min<2.5, 1> is
    // 1.0, not 1.
    assert_eq!(
        value(Concept::call("min", [float(2.5), int(1)])),
        float(1.0)
    );
    assert_eq!(
        value(Concept::call("max", [int(1), float(2.5)])),
        float(2.5)
    );
    assert_eq!(
        value(Concept::call("mul", [int(2), float(3.0)])),
        float(6.0)
    );
    assert_eq!(
        value(Concept::call("pow", [int(2), float(3.0)])),
        float(8.0)
    );
}

// ---------------------------------------------------------------------------
// Integer failure modes
// ---------------------------------------------------------------------------

#[test]
fn integer_overflow_is_a_clean_error_rather_than_a_wrap() {
    let (store, registry) = setup();
    let mut ev = Evaluator::new(&store, &registry).with_budget(Budget::deterministic());
    let out = ev.evaluate(&Concept::call("add", [int(i64::MAX), int(1)]));

    assert!(out.value().is_none(), "overflow produced a value: {out:?}");
    assert_ne!(
        out.partial(),
        Some(&int(i64::MIN)),
        "the sum wrapped instead of failing"
    );
    let failures = ev.trace().failures();
    assert!(
        failures.iter().any(|(_, _, msg)| msg.contains("overflow")),
        "expected an overflow failure in the trace, got {failures:?}"
    );
}

#[test]
fn every_integer_operation_that_can_overflow_says_so() {
    assert_errors(Concept::call("add", [int(i64::MAX), int(1)]));
    assert_errors(Concept::call("sub", [int(i64::MIN), int(1)]));
    assert_errors(Concept::call("mul", [int(i64::MAX), int(2)]));
    assert_errors(Concept::call("neg", [int(i64::MIN)]));
    assert_errors(Concept::call("abs", [int(i64::MIN)]));
    assert_errors(Concept::call("pow", [int(i64::MAX), int(2)]));
    // i64::MIN / -1 has no i64 answer either.
    assert_errors(Concept::call("div", [int(i64::MIN), int(-1)]));
}

#[test]
fn integer_division_by_zero_is_an_error() {
    assert_errors(Concept::call("div", [int(1), int(0)]));
    assert_errors(Concept::call("modulo", [int(1), int(0)]));
}

#[test]
fn float_division_by_zero_gives_infinity_and_round_trips() {
    // IEEE already defines the answer, so there is nothing for Spoon to refuse.
    let out = value(Concept::call("div", [float(1.0), float(0.0)]));
    assert_eq!(out, float(f64::INFINITY));

    let encoded = serde_json::to_string(&out).unwrap();
    let decoded: Concept = serde_json::from_str(&encoded).unwrap();
    assert_eq!(
        decoded, out,
        "inf did not survive a serialization round trip"
    );
    assert_eq!(decoded.content_id(), out.content_id());

    assert_eq!(
        value(Concept::call("div", [float(-1.0), float(0.0)])),
        float(f64::NEG_INFINITY)
    );

    // 0.0 / 0.0 is NaN, which Ground collapses to a single self-equal concept.
    let nan = value(Concept::call("div", [float(0.0), float(0.0)]));
    assert!(nan.as_ground().and_then(Ground::as_f64).unwrap().is_nan());
    assert_eq!(nan, float(f64::NAN));
}

#[test]
fn a_negative_integer_exponent_is_refused_rather_than_silently_widened() {
    // Pow<2, -1> is 0.5, and the widening rule promises an all-integer call an
    // integer. Saying so beats breaking the promise quietly.
    assert_errors(Concept::call("pow", [int(2), int(-1)]));
    assert_eq!(
        value(Concept::call("pow", [float(2.0), int(-1)])),
        float(0.5)
    );
}

#[test]
fn nan_has_no_smallest_or_largest() {
    let out = value(Concept::call("min", [float(f64::NAN), int(1)]));
    assert!(out.as_ground().and_then(Ground::as_f64).unwrap().is_nan());
    let out = value(Concept::call("max", [int(1), float(f64::NAN)]));
    assert!(out.as_ground().and_then(Ground::as_f64).unwrap().is_nan());
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

#[test]
fn equality_is_concept_identity() {
    assert_eq!(value(Concept::call("eq", [int(42), int(42)])), yes());
    assert_eq!(
        value(Concept::call("eq", [int(42), float(42.0)])),
        no(),
        "42 and 42.0 are different concepts"
    );
    assert_eq!(
        value(Concept::call(
            "eq",
            [Concept::named("greg"), Concept::named("greg")]
        )),
        yes()
    );
    assert_eq!(
        value(Concept::call(
            "eq",
            [Concept::named("greg"), Concept::named("keal")]
        )),
        no()
    );
    // Compounds too: identity is structural, not reference.
    assert_eq!(
        value(Concept::call(
            "eq",
            [
                Concept::call("friend-with", [Concept::named("greg")]),
                Concept::call("friend-with", [Concept::named("greg")])
            ]
        )),
        yes()
    );
    assert_eq!(value(Concept::call("ne", [int(42), float(42.0)])), yes());
    assert_eq!(value(Concept::call("ne", [int(42), int(42)])), no());
}

#[test]
fn equality_compares_the_reduced_arguments() {
    // Eq is eager, so Add<1, 2> has already become 3 by the time it is compared.
    assert_eq!(
        value(Concept::call(
            "eq",
            [Concept::call("add", [int(1), int(2)]), int(3)]
        )),
        yes()
    );
}

#[test]
fn ordering_compares_numbers_across_int_and_float() {
    assert_eq!(value(Concept::call("lt", [int(1), int(2)])), yes());
    assert_eq!(value(Concept::call("lt", [int(2), int(1)])), no());
    assert_eq!(value(Concept::call("gt", [float(2.5), int(2)])), yes());
    assert_eq!(value(Concept::call("lte", [int(2), float(2.0)])), yes());
    assert_eq!(value(Concept::call("gte", [int(2), float(2.0)])), yes());
    assert_eq!(value(Concept::call("lte", [int(3), float(2.0)])), no());
    assert_eq!(value(Concept::call("gte", [float(1.5), int(2)])), no());
}

#[test]
fn nan_is_unordered() {
    for native in ["lt", "gt", "lte", "gte"] {
        assert_eq!(
            value(Concept::call(native, [float(f64::NAN), float(f64::NAN)])),
            no(),
            "{native} claimed an ordering against NaN"
        );
    }
}

#[test]
fn ordering_refuses_non_numbers_instead_of_guessing() {
    for native in ["lt", "gt", "lte", "gte"] {
        assert_errors(Concept::call(native, [nonsense(), nonsense()]));
        assert_errors(Concept::call(native, [int(1), Concept::text("two")]));
        assert_errors(Concept::call(native, [yes(), no()]));
    }
}

// ---------------------------------------------------------------------------
// Logic
// ---------------------------------------------------------------------------

#[test]
fn the_logic_natives_compute() {
    assert_eq!(value(Concept::call("and", [yes(), yes()])), yes());
    assert_eq!(value(Concept::call("and", [yes(), yes(), no()])), no());
    assert_eq!(value(Concept::call("or", [no(), no()])), no());
    assert_eq!(value(Concept::call("or", [no(), no(), yes()])), yes());
    assert_eq!(value(Concept::call("not", [yes()])), no());
    assert_eq!(value(Concept::call("not", [no()])), yes());
    assert_eq!(value(Concept::call("xor", [yes(), no()])), yes());
    assert_eq!(value(Concept::call("xor", [yes(), yes()])), no());
    assert_eq!(value(Concept::call("xor", [no(), no()])), no());
    assert_eq!(value(Concept::call("if", [yes(), int(1), int(2)])), int(1));
    assert_eq!(value(Concept::call("if", [no(), int(1), int(2)])), int(2));
}

#[test]
fn if_does_not_evaluate_the_branch_it_did_not_take() {
    let (store, registry) = setup();
    let mut ev = Evaluator::new(&store, &registry).with_budget(Budget::deterministic());
    let out = ev.evaluate(&Concept::call("if", [yes(), int(7), boom()]));
    assert_eq!(out.value(), Some(&int(7)));
    assert!(
        ev.trace().failures().is_empty(),
        "the untaken branch ran: {:?}",
        ev.trace().failures()
    );

    let mut ev = Evaluator::new(&store, &registry).with_budget(Budget::deterministic());
    let out = ev.evaluate(&Concept::call("if", [no(), boom(), int(7)]));
    assert_eq!(out.value(), Some(&int(7)));
    assert!(ev.trace().failures().is_empty());
}

#[test]
fn the_untaken_branch_leaves_no_trace_at_all() {
    // `boom` failing is one kind of proof; a branch that would have succeeded
    // and simply never ran is the stricter one.
    let (store, registry) = setup();
    let mut ev = Evaluator::new(&store, &registry).with_budget(Budget::deterministic());
    let out = ev.evaluate(&Concept::call(
        "if",
        [yes(), int(7), Concept::call("spy", [])],
    ));
    assert_eq!(out.value(), Some(&int(7)));
    assert!(
        !ev.trace().notes.iter().any(|n| n.message == "spy ran"),
        "the untaken branch was evaluated"
    );
}

#[test]
fn and_and_or_short_circuit() {
    let (store, registry) = setup();

    let mut ev = Evaluator::new(&store, &registry).with_budget(Budget::deterministic());
    let out = ev.evaluate(&Concept::call("and", [no(), boom()]));
    assert_eq!(out.value(), Some(&no()));
    assert!(
        ev.trace().failures().is_empty(),
        "and evaluated past its first false"
    );

    let mut ev = Evaluator::new(&store, &registry).with_budget(Budget::deterministic());
    let out = ev.evaluate(&Concept::call("or", [yes(), boom()]));
    assert_eq!(out.value(), Some(&yes()));
    assert!(
        ev.trace().failures().is_empty(),
        "or evaluated past its first true"
    );

    // The stopping point is the first decisive argument, not the whole list.
    let mut ev = Evaluator::new(&store, &registry).with_budget(Budget::deterministic());
    let out = ev.evaluate(&Concept::call("and", [yes(), no(), boom(), boom()]));
    assert_eq!(out.value(), Some(&no()));
    assert!(ev.trace().failures().is_empty());
}

#[test]
fn a_non_boolean_after_the_short_circuit_is_never_looked_at() {
    // An argument that did not run has no type, so this is short-circuiting
    // rather than a hole in the type check.
    assert_eq!(value(Concept::call("and", [no(), nonsense()])), no());
    assert_eq!(value(Concept::call("or", [yes(), nonsense()])), yes());
}

#[test]
fn a_non_boolean_condition_is_a_type_error_not_a_truthiness_guess() {
    for condition in [int(0), int(1), Concept::text(""), nonsense()] {
        assert_errors(Concept::call("if", [condition, int(1), int(2)]));
    }
    assert_errors(Concept::call("and", [int(1), yes()]));
    assert_errors(Concept::call("or", [int(0), no()]));
    assert_errors(Concept::call("not", [int(1)]));
    assert_errors(Concept::call("xor", [int(1), yes()]));
}

// ---------------------------------------------------------------------------
// Numeric predicates
// ---------------------------------------------------------------------------

#[test]
fn the_numeric_predicates_compute() {
    assert_eq!(value(Concept::call("is-zero", [int(0)])), yes());
    assert_eq!(value(Concept::call("is-zero", [float(0.0)])), yes());
    assert_eq!(value(Concept::call("is-zero", [int(1)])), no());

    assert_eq!(value(Concept::call("is-positive", [int(1)])), yes());
    assert_eq!(value(Concept::call("is-positive", [float(0.5)])), yes());
    assert_eq!(
        value(Concept::call("is-positive", [int(0)])),
        no(),
        "zero is neither positive nor negative"
    );

    assert_eq!(value(Concept::call("is-negative", [int(-1)])), yes());
    assert_eq!(value(Concept::call("is-negative", [float(-0.5)])), yes());
    assert_eq!(value(Concept::call("is-negative", [int(0)])), no());

    assert_eq!(value(Concept::call("is-even", [int(4)])), yes());
    assert_eq!(value(Concept::call("is-even", [int(-4)])), yes());
    assert_eq!(value(Concept::call("is-even", [int(3)])), no());
    assert_eq!(value(Concept::call("is-odd", [int(3)])), yes());
    assert_eq!(value(Concept::call("is-odd", [int(-3)])), yes());
    assert_eq!(value(Concept::call("is-odd", [int(4)])), no());
}

#[test]
fn nan_is_neither_positive_nor_negative_nor_zero() {
    for native in ["is-zero", "is-positive", "is-negative"] {
        assert_eq!(
            value(Concept::call(native, [float(f64::NAN)])),
            no(),
            "{native} made a claim about NaN"
        );
    }
}

#[test]
fn parity_is_an_integer_question() {
    assert_errors(Concept::call("is-even", [float(2.0)]));
    assert_errors(Concept::call("is-odd", [float(3.0)]));
}

// ---------------------------------------------------------------------------
// Composition
// ---------------------------------------------------------------------------

#[test]
fn nested_arithmetic_reduces_innermost_first() {
    assert_eq!(
        value(Concept::call(
            "add",
            [Concept::call("mul", [int(2), int(3)]), int(4)]
        )),
        int(10)
    );
    // Deeper, and across the widening rule and a comparison.
    assert_eq!(
        value(Concept::call(
            "if",
            [
                Concept::call(
                    "gt",
                    [
                        // Add<7, 3> is 10, Div<10, 4> truncates to 2, and 2 > 1.
                        Concept::call("div", [Concept::call("add", [int(7), int(3)]), int(4)]),
                        int(1)
                    ]
                ),
                Concept::call("abs", [Concept::call("neg", [float(1.5)])]),
                boom(),
            ]
        )),
        float(1.5)
    );
}

// ---------------------------------------------------------------------------
// Nothing panics
// ---------------------------------------------------------------------------

/// Every arithmetic native, handed a concept it cannot possibly work with, at
/// every arity from none to three.
///
/// The point is the absence of a panic. A native receives whatever the
/// evaluator hands it, and reaching for `args[0]` on an empty slice, or
/// unwrapping a ground value that is not there, would take the whole process
/// down over a bad argument. It also checks the outcome is not a value, which
/// catches a native that quietly invents an answer.
#[test]
fn no_native_panics_on_a_nonsense_argument() {
    // Scoped to the natives this module registers. The other bootstrap modules
    // own their own coverage.
    let mut arith_only = NativeRegistry::new();
    spoon_natives::arith::register(&mut arith_only);
    let names: Vec<String> = arith_only
        .names()
        .iter()
        .map(|id| id.as_str().to_string())
        .collect();
    assert!(
        names.len() >= 26,
        "expected the full arith set, got {names:?}"
    );

    let (store, registry) = setup();
    for name in &names {
        for arity in 0..=3 {
            let args: Vec<Concept> = (0..arity).map(|_| nonsense()).collect();
            let expr = Concept::call(name, args);
            let mut ev = Evaluator::new(&store, &registry).with_budget(Budget::deterministic());
            let out = ev.evaluate(&expr);

            // Eq and Ne are identity comparisons and accept any two concepts,
            // so Eq<nonsense, nonsense> is a legitimate `true`.
            if (name == "eq" || name == "ne") && arity == 2 {
                assert!(out.is_value(), "{name}/{arity} should compare: {out:?}");
                continue;
            }
            assert!(
                out.value().is_none(),
                "{name}/{arity} invented an answer for nonsense: {out:?}"
            );
        }
    }
}

#[test]
fn every_arith_native_is_seeded_and_reachable() {
    // Registration makes the code reachable; the store realization makes the
    // concept exist. A native seeded but unregistered, or registered under a
    // name another module later overwrites, would fail here rather than in
    // whatever downstream feature depended on it.
    let mut arith_only = NativeRegistry::new();
    spoon_natives::arith::register(&mut arith_only);
    let (store, registry) = setup();
    for id in arith_only.names() {
        assert!(
            registry.contains(&id),
            "{} did not survive into the bootstrap registry",
            id.as_str()
        );
        let realizations = store
            .realizations_for(&Concept::named(id.as_str()))
            .unwrap();
        assert!(
            !realizations.is_empty(),
            "{} has no seeded realization",
            id.as_str()
        );
    }
}
