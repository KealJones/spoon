//! Orchestrator review: the arithmetic guarantees the rest of the system will
//! quietly assume. Written against the laws, not against the implementation.

use chrono::{TimeZone, Utc};
use spoon_concept::{Concept, Ground};
use spoon_eval::{Budget, Evaluator, NativeRegistry, Outcome, PermissionMode};
use spoon_store::Store;

fn env() -> (Store, NativeRegistry) {
    let store = Store::open_in_memory().unwrap();
    let registry = spoon_natives::bootstrap();
    spoon_natives::seed_bootstrap(&store, &registry).unwrap();
    (store, registry)
}

fn eval(store: &Store, registry: &NativeRegistry, c: &Concept) -> Outcome {
    Evaluator::new(store, registry)
        .with_budget(Budget::deterministic())
        .with_permission(PermissionMode::Bypass)
        .with_now(Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap())
        .evaluate(c)
}

fn value(store: &Store, registry: &NativeRegistry, c: &Concept) -> Concept {
    match eval(store, registry, c) {
        Outcome::Value(v) => v,
        other => panic!("expected a value, got {other:?}"),
    }
}

fn is_error(out: &Outcome) -> bool {
    matches!(out, Outcome::Failed(_) | Outcome::Stuck { .. })
}

// ---------------------------------------------------------------------------
// The widening rule
// ---------------------------------------------------------------------------

#[test]
fn all_integer_arithmetic_stays_integer() {
    // Downstream code that asks for an Int and silently receives a Float would
    // produce a different concept with a different content id, so the rule has
    // to hold across every operation, not just add.
    let (store, reg) = env();
    let cases = [
        Concept::call("math-add", [Concept::int(2), Concept::int(3)]),
        Concept::call("math-sub", [Concept::int(9), Concept::int(4)]),
        Concept::call("math-mul", [Concept::int(2), Concept::int(3)]),
        Concept::call("math-div", [Concept::int(9), Concept::int(3)]),
        Concept::call("math-modulo", [Concept::int(9), Concept::int(4)]),
        Concept::call("math-neg", [Concept::int(5)]),
        Concept::call("math-abs", [Concept::int(-5)]),
        Concept::call("math-min", [Concept::int(1), Concept::int(2)]),
        Concept::call("math-max", [Concept::int(1), Concept::int(2)]),
        Concept::call("math-pow", [Concept::int(2), Concept::int(8)]),
    ];
    for case in cases {
        let out = value(&store, &reg, &case);
        assert!(
            matches!(out.as_ground(), Some(Ground::Int(_))),
            "{case:?} produced {out:?}, which is not an Int"
        );
    }
}

#[test]
fn one_float_argument_widens_the_whole_result() {
    let (store, reg) = env();
    let cases = [
        Concept::call("math-add", [Concept::int(2), Concept::float(0.5)]),
        Concept::call("math-sub", [Concept::float(9.5), Concept::int(4)]),
        Concept::call("math-mul", [Concept::int(2), Concept::float(1.5)]),
        Concept::call("math-min", [Concept::float(2.5), Concept::int(1)]),
        Concept::call("math-max", [Concept::int(1), Concept::float(2.5)]),
    ];
    for case in cases {
        let out = value(&store, &reg, &case);
        assert!(
            matches!(out.as_ground(), Some(Ground::Float(_))),
            "{case:?} produced {out:?}, which is not a Float"
        );
    }
}

#[test]
fn integer_division_truncates_and_float_division_does_not() {
    let (store, reg) = env();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("math-div", [Concept::int(7), Concept::int(2)])
        ),
        Concept::int(3)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("math-div", [Concept::float(7.0), Concept::int(2)])
        ),
        Concept::float(3.5)
    );
}

// ---------------------------------------------------------------------------
// Failure rather than a quiet wrong answer
// ---------------------------------------------------------------------------

#[test]
fn integer_overflow_is_refused_rather_than_wrapped() {
    // A wrap here would be a silently wrong number propagating into everything
    // downstream, which is worse than no answer.
    let (store, reg) = env();
    let out = eval(
        &store,
        &reg,
        &Concept::call("math-add", [Concept::int(i64::MAX), Concept::int(1)]),
    );
    assert!(is_error(&out), "overflow produced {out:?}");

    let out = eval(
        &store,
        &reg,
        &Concept::call("math-mul", [Concept::int(i64::MAX), Concept::int(2)]),
    );
    assert!(is_error(&out), "overflow produced {out:?}");
}

#[test]
fn integer_division_by_zero_errors_but_float_follows_ieee() {
    // Integers have no answer, so refusing is the only honest option. Floats do
    // have one, and Ground::Float round-trips the infinities, so passing it
    // through loses nothing.
    let (store, reg) = env();
    assert!(is_error(&eval(
        &store,
        &reg,
        &Concept::call("math-div", [Concept::int(1), Concept::int(0)])
    )));

    let out = value(
        &store,
        &reg,
        &Concept::call("math-div", [Concept::float(1.0), Concept::float(0.0)]),
    );
    assert_eq!(
        out.as_ground().and_then(Ground::as_f64),
        Some(f64::INFINITY)
    );
}

#[test]
fn integers_compare_exactly_beyond_the_float_mantissa() {
    // Widening Int/Int through f64 would make these two adjacent values compare
    // equal, so Lt would return false for two genuinely different numbers.
    let (store, reg) = env();
    let big = 9_007_199_254_740_993i64; // 2^53 + 1, not representable in f64
    let out = value(
        &store,
        &reg,
        &Concept::call("logic-lt", [Concept::int(big - 1), Concept::int(big)]),
    );
    assert_eq!(
        out,
        Concept::bool(true),
        "comparison lost precision past 2^53"
    );
}

// ---------------------------------------------------------------------------
// Equality is concept identity
// ---------------------------------------------------------------------------

#[test]
fn equality_is_identity_not_numeric_coercion() {
    // 42 and 42.0 are different concepts with different content ids. Equality
    // that ignored that would contradict the representation.
    let (store, reg) = env();
    let pairs = [
        (Concept::int(42), Concept::int(42), true),
        (Concept::int(42), Concept::float(42.0), false),
        (Concept::int(42), Concept::text("42"), false),
        (Concept::named("greg"), Concept::named("greg"), true),
        (Concept::named("greg"), Concept::named("keal"), false),
        (Concept::bool(true), Concept::int(1), false),
    ];
    for (a, b, expected) in pairs {
        let out = value(
            &store,
            &reg,
            &Concept::call("logic-eq", [a.clone(), b.clone()]),
        );
        assert_eq!(out, Concept::bool(expected), "eq({a:?}, {b:?})");
    }
}

#[test]
fn equality_works_on_compounds_too() {
    let (store, reg) = env();
    let f = Concept::call(
        "friend-with",
        [Concept::named("greg"), Concept::named("keal")],
    );
    let g = Concept::call(
        "friend-with",
        [Concept::named("keal"), Concept::named("greg")],
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("logic-eq", [f.clone(), f.clone()])
        ),
        Concept::bool(true)
    );
    // Argument order matters: that these mean the same thing is something
    // Symmetric has to establish, not something equality may assume.
    assert_eq!(
        value(&store, &reg, &Concept::call("logic-eq", [f, g])),
        Concept::bool(false)
    );
}

// ---------------------------------------------------------------------------
// Short circuiting
// ---------------------------------------------------------------------------

/// A concept whose only realization always fails, used to prove an argument was
/// never reduced. If it runs, the surrounding evaluation cannot succeed.
fn tripwire() -> Concept {
    Concept::call("math-div", [Concept::int(1), Concept::int(0)])
}

#[test]
fn conditionals_never_touch_the_branch_they_do_not_take() {
    let (store, reg) = env();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "logic-if",
                [Concept::bool(true), Concept::int(7), tripwire()]
            )
        ),
        Concept::int(7)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "logic-if",
                [Concept::bool(false), tripwire(), Concept::int(9)]
            )
        ),
        Concept::int(9)
    );
}

#[test]
fn and_or_short_circuit_on_the_first_decisive_argument() {
    let (store, reg) = env();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("logic-and", [Concept::bool(false), tripwire()])
        ),
        Concept::bool(false)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("logic-or", [Concept::bool(true), tripwire()])
        ),
        Concept::bool(true)
    );
}

#[test]
fn short_circuiting_does_not_type_check_what_it_skipped() {
    // An argument that never ran has no type. Rejecting it would mean the
    // evaluator inspected something it promised not to evaluate.
    let (store, reg) = env();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "logic-and",
                [Concept::bool(false), Concept::named("nonsense")]
            )
        ),
        Concept::bool(false)
    );
}

#[test]
fn a_non_boolean_condition_is_an_error_not_a_truthiness_guess() {
    // Guessing that a non-empty string is true is how a system ends up
    // confidently wrong. Saying it does not understand is better.
    let (store, reg) = env();
    for bad in [
        Concept::int(1),
        Concept::text("yes"),
        Concept::named("greg"),
    ] {
        let out = eval(
            &store,
            &reg,
            &Concept::call("logic-if", [bad.clone(), Concept::int(1), Concept::int(2)]),
        );
        assert!(is_error(&out), "{bad:?} was coerced to a boolean: {out:?}");
    }
}

// ---------------------------------------------------------------------------
// Composition
// ---------------------------------------------------------------------------

#[test]
fn nested_arithmetic_reduces_through_arbitrary_depth() {
    let (store, reg) = env();
    let mut expr = Concept::int(0);
    for i in 1..=50 {
        expr = Concept::call("math-add", [expr, Concept::int(i)]);
    }
    assert_eq!(value(&store, &reg, &expr), Concept::int(1275));
}

#[test]
fn predicates_feed_conditionals() {
    let (store, reg) = env();
    let expr = Concept::call(
        "logic-if",
        [
            Concept::call(
                "math-is-even",
                [Concept::call(
                    "math-add",
                    [Concept::int(1), Concept::int(3)],
                )],
            ),
            Concept::text("even"),
            Concept::text("odd"),
        ],
    );
    assert_eq!(value(&store, &reg, &expr), Concept::text("even"));
}

// ---------------------------------------------------------------------------
// Nothing panics
// ---------------------------------------------------------------------------

#[test]
fn no_native_panics_on_nonsense_input() {
    // Natives receive whatever the evaluator hands them. A panic takes the
    // whole process down over one bad argument, so every failure has to be a
    // returned error.
    let (store, reg) = env();
    let junk = [
        Concept::named("nonsense"),
        Concept::text("not a number"),
        Concept::bool(true),
        Concept::call("list-of", [Concept::int(1)]),
        Concept::hole(0),
    ];
    for name in reg.names() {
        for arity in 0..4usize {
            for bad in &junk {
                let args: Vec<Concept> = std::iter::repeat_n(bad.clone(), arity).collect();
                let expr = Concept::apply(
                    Concept::symbol(spoon_concept::SymbolId::of(name.as_str())),
                    args,
                );
                // The assertion is simply that this returns rather than aborts.
                let _ = eval(&store, &reg, &expr);
            }
        }
    }
}
