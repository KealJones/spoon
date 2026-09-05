//! Functions written inline, as expressions with holes.
//!
//! Without these the only functions that can be passed to `map` or `filter` are
//! ones that already have names, so "keep the ones equal to r" is
//! inexpressible. That rules out most of what anyone actually asks a collection
//! to do, and the missing piece is not a new concept but the reading of one
//! that already exists: a `Composed` body is already holes bound positionally,
//! so a concept with holes already means a function everywhere else.

use spoon_concept::Concept;
use spoon_eval::{Budget, Evaluator, NativeRegistry, Outcome, PermissionMode};
use spoon_store::Store;

fn env() -> (Store, NativeRegistry) {
    let store = Store::open_in_memory().unwrap();
    let registry = spoon_natives::bootstrap();
    spoon_natives::seed_bootstrap(&store, &registry).unwrap();
    (store, registry)
}

fn eval(store: &Store, reg: &NativeRegistry, c: &Concept) -> Outcome {
    Evaluator::new(store, reg)
        .with_budget(Budget::deterministic())
        .with_permission(PermissionMode::Bypass)
        .evaluate(c)
}

fn value(store: &Store, reg: &NativeRegistry, c: &Concept) -> Concept {
    match eval(store, reg, c) {
        Outcome::Value(v) => v,
        other => panic!("expected a value from {c:?}, got {other:?}"),
    }
}

fn list(items: impl IntoIterator<Item = Concept>) -> Concept {
    Concept::call("list-of", items)
}

#[test]
fn counting_occurrences_of_a_character() {
    // The question that exposed the gap. There is no concept for "count the
    // ones equal to this", and there should not need to be: filter with an
    // inline predicate says it exactly.
    let (store, reg) = env();
    let expr = Concept::call(
        "list-count",
        [Concept::call(
            "list-filter",
            [
                Concept::call("text-chars", [Concept::text("Strawberry")]),
                Concept::call("logic-eq", [Concept::hole(0), Concept::text("r")]),
            ],
        )],
    );
    assert_eq!(value(&store, &reg, &expr), Concept::int(3));
}

#[test]
fn mapping_with_an_inline_function() {
    let (store, reg) = env();
    let expr = Concept::call(
        "list-map",
        [
            list([Concept::int(1), Concept::int(2), Concept::int(3)]),
            Concept::call("math-mul", [Concept::hole(0), Concept::int(10)]),
        ],
    );
    assert_eq!(
        value(&store, &reg, &expr),
        list([Concept::int(10), Concept::int(20), Concept::int(30)])
    );
}

#[test]
fn a_hole_used_twice_binds_to_the_same_element() {
    // Squaring. If each occurrence bound separately this would be nonsense, and
    // it is the same rule that makes `double` work as `add<?0, ?0>`.
    let (store, reg) = env();
    let expr = Concept::call(
        "math-sum",
        [Concept::call(
            "list-map",
            [
                list([
                    Concept::int(1),
                    Concept::int(2),
                    Concept::int(3),
                    Concept::int(4),
                ]),
                Concept::call("math-mul", [Concept::hole(0), Concept::hole(0)]),
            ],
        )],
    );
    assert_eq!(value(&store, &reg, &expr), Concept::int(30));
}

#[test]
fn filtering_with_a_comparison_against_a_value() {
    let (store, reg) = env();
    let expr = Concept::call(
        "list-filter",
        [
            list([
                Concept::int(1),
                Concept::int(8),
                Concept::int(3),
                Concept::int(9),
            ]),
            Concept::call("logic-gt", [Concept::hole(0), Concept::int(4)]),
        ],
    );
    assert_eq!(
        value(&store, &reg, &expr),
        list([Concept::int(8), Concept::int(9)])
    );
}

#[test]
fn a_named_function_still_works() {
    // Inline functions are an addition, not a replacement. A bare concept name
    // is still applied rather than substituted.
    let (store, reg) = env();
    let expr = Concept::call(
        "list-map",
        [list([Concept::text("ab")]), Concept::named("text-upper")],
    );
    assert_eq!(value(&store, &reg, &expr), list([Concept::text("AB")]));
}

#[test]
fn the_function_argument_is_not_evaluated_before_it_is_used() {
    // `eq<?0, "r">` evaluated on its own reduces to `false`, because equality
    // compares concepts and a hole is not the letter r. That is exactly the
    // danger: reducing the argument first hands `filter` a boolean where a
    // function should be, and the function is gone before it is ever used.
    // Inline functions stayed broken even after substitution was written, for
    // precisely this reason.
    let (store, reg) = env();
    let alone = Concept::call("logic-eq", [Concept::hole(0), Concept::text("r")]);
    assert_eq!(
        value(&store, &reg, &alone),
        Concept::bool(false),
        "evaluating a lambda body destroys it, which is why filter must not"
    );
    // And yet it works as an argument.
    let used = Concept::call(
        "list-filter",
        [list([Concept::text("r"), Concept::text("s")]), alone],
    );
    assert_eq!(value(&store, &reg, &used), list([Concept::text("r")]));
}

#[test]
fn reduce_evaluates_its_initial_value_but_not_its_function() {
    let (store, reg) = env();
    let expr = Concept::call(
        "list-reduce",
        [
            list([Concept::int(1), Concept::int(2), Concept::int(3)]),
            Concept::call("math-add", [Concept::hole(0), Concept::hole(1)]),
            Concept::call("math-add", [Concept::int(5), Concept::int(5)]),
        ],
    );
    assert_eq!(value(&store, &reg, &expr), Concept::int(16));
}
