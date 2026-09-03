//! Collections, end to end through a real evaluator and store.
//!
//! Nothing here reaches into the natives directly. Every assertion goes
//! through `Evaluator`, because the interesting claim about `Map` is not that
//! it iterates but that applying an element routes back through realization
//! selection, which only the real loop can demonstrate.

use chrono::{TimeZone, Utc};
use spoon_concept::{Activation, Concept, Effect, Provenance, Realization, RealizationSpec, Tier};
use spoon_eval::{Budget, Evaluator, NativeRegistry, Outcome};
use spoon_store::Store;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap()
}

/// A store seeded with the whole bootstrap set, plus the registry to run it.
fn brain() -> (Store, NativeRegistry) {
    let store = Store::open_in_memory().expect("in-memory store");
    let registry = spoon_natives::bootstrap();
    spoon_natives::seed_bootstrap(&store, &registry).expect("seed");
    (store, registry)
}

/// Store a learned capability: a `Composed` realization whose body is concepts,
/// not code. `Hole(0)` is argument 0.
fn learn(store: &Store, target: &str, body: Concept) {
    store
        .put_realization(&Realization {
            target: Concept::named(target),
            name: format!("composed-{target}").into(),
            spec: RealizationSpec::Composed { body },
            effect: Effect::Pure,
            activation: Activation::new(now()),
            provenance: Provenance::Bootstrap,
            tier: Tier::Kernel,
        })
        .expect("put realization");
}

fn run(store: &Store, reg: &NativeRegistry, c: &Concept) -> (Outcome, Vec<String>) {
    let mut ev = Evaluator::new(store, reg)
        .with_budget(Budget::deterministic())
        .with_now(now());
    let outcome = ev.evaluate(c);
    let failures = ev
        .trace()
        .failures()
        .iter()
        .map(|(_, _, message)| (*message).to_string())
        .collect();
    (outcome, failures)
}

fn value(store: &Store, reg: &NativeRegistry, c: &Concept) -> Concept {
    let (outcome, failures) = run(store, reg, c);
    match outcome.value() {
        Some(v) => v.clone(),
        None => panic!("expected a value, got {outcome:?} with failures {failures:?}"),
    }
}

/// Assert the call did not produce a value, and hand back what the trace said
/// went wrong. A native error is retryable, so the evaluator exhausts the
/// candidate list and reports `Stuck`; the message lives in the trace.
fn why_failed(store: &Store, reg: &NativeRegistry, c: &Concept) -> String {
    let (outcome, failures) = run(store, reg, c);
    assert!(!outcome.is_value(), "expected a failure, got {outcome:?}");
    failures.join(" | ")
}

fn list(items: impl IntoIterator<Item = Concept>) -> Concept {
    Concept::call("list", items)
}

fn ints(values: impl IntoIterator<Item = i64>) -> Concept {
    list(values.into_iter().map(Concept::int))
}

fn texts(values: impl IntoIterator<Item = &'static str>) -> Concept {
    list(values.into_iter().map(Concept::text))
}

// ---------------------------------------------------------------------------
// Construction and access
// ---------------------------------------------------------------------------

#[test]
fn list_builds_a_compound_headed_by_list() {
    let (store, reg) = brain();
    let out = value(
        &store,
        &reg,
        &Concept::call("list", [Concept::int(1), Concept::int(2)]),
    );
    assert_eq!(out, ints([1, 2]));
    assert_eq!(out.arity(), 2);
    assert_eq!(value(&store, &reg, &Concept::call("list", [])), ints([]));
}

#[test]
fn first_last_and_nth_read_positions() {
    let (store, reg) = brain();
    let l = ints([10, 20, 30]);
    assert_eq!(
        value(&store, &reg, &Concept::call("first", [l.clone()])),
        Concept::int(10)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("last", [l.clone()])),
        Concept::int(30)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("nth", [l, Concept::int(1)])),
        Concept::int(20)
    );
}

#[test]
fn count_and_is_empty_describe_size() {
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("count", [ints([1, 2, 3])])),
        Concept::int(3)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("count", [ints([])])),
        Concept::int(0)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("is-empty", [ints([])])),
        Concept::bool(true)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("is-empty", [ints([1])])),
        Concept::bool(false)
    );
}

#[test]
fn append_and_prepend_keep_the_order_they_were_written_in() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("append", [ints([1]), Concept::int(2), Concept::int(3)])
        ),
        ints([1, 2, 3])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("prepend", [ints([3]), Concept::int(1), Concept::int(2)])
        ),
        ints([1, 2, 3]),
        "extra arguments should not come out reversed"
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("append", [ints([])])),
        ints([])
    );
}

#[test]
fn concat_lists_and_reverse() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("concat-lists", [ints([1, 2]), ints([]), ints([3])])
        ),
        ints([1, 2, 3])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("concat-lists", [])),
        ints([])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("reverse", [ints([1, 2, 3])])),
        ints([3, 2, 1])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("reverse", [ints([])])),
        ints([])
    );
}

#[test]
fn slice_takes_a_half_open_range() {
    let (store, reg) = brain();
    let l = ints([1, 2, 3, 4]);
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("slice", [l.clone(), Concept::int(1), Concept::int(3)])
        ),
        ints([2, 3])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("slice", [l.clone(), Concept::int(2), Concept::int(2)])
        ),
        ints([]),
        "an empty range is empty, not an error"
    );
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("slice", [l, Concept::int(0), Concept::int(9)]),
    );
    assert!(
        message.contains('9') && message.contains('4'),
        "got {message}"
    );
}

#[test]
fn contains_index_of_and_unique() {
    let (store, reg) = brain();
    let l = ints([1, 2, 2, 3]);
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("contains", [l.clone(), Concept::int(2)])
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("contains", [l.clone(), Concept::int(9)])
        ),
        Concept::bool(false)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("index-of", [l.clone(), Concept::int(2)])
        ),
        Concept::int(1),
        "index-of reports the first match"
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("unique", [l.clone()])),
        ints([1, 2, 3])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("unique", [ints([])])),
        ints([])
    );

    // Absence is an error rather than a -1 the caller can forget to check.
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("index-of", [l, Concept::int(9)]),
    );
    assert!(message.contains("index-of"), "got {message}");
}

#[test]
fn flatten_goes_exactly_one_level() {
    let (store, reg) = brain();
    let nested = list([ints([1, 2]), Concept::int(3), list([ints([4])])]);
    assert_eq!(
        value(&store, &reg, &Concept::call("flatten", [nested])),
        list([Concept::int(1), Concept::int(2), Concept::int(3), ints([4])]),
        "the doubly nested list should have come out one level shallower, not flat"
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("flatten", [ints([])])),
        ints([])
    );
}

// ---------------------------------------------------------------------------
// Out-of-range access
// ---------------------------------------------------------------------------

#[test]
fn out_of_range_access_names_the_index_and_the_length() {
    let (store, reg) = brain();

    let message = why_failed(&store, &reg, &Concept::call("first", [ints([])]));
    assert!(message.contains("out of range"), "got {message}");

    let message = why_failed(&store, &reg, &Concept::call("last", [ints([])]));
    assert!(message.contains("out of range"), "got {message}");

    let message = why_failed(
        &store,
        &reg,
        &Concept::call("nth", [ints([1, 2, 3]), Concept::int(7)]),
    );
    assert!(
        message.contains("index 7") && message.contains("length 3"),
        "the message should name both the index and the length, got {message}"
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call("nth", [ints([1, 2, 3]), Concept::int(-1)]),
    );
    assert!(message.contains("negative index -1"), "got {message}");
}

// ---------------------------------------------------------------------------
// Higher-order: the point of the whole module
// ---------------------------------------------------------------------------

#[test]
fn a_learned_composed_realization_works_as_a_mapper() {
    // `double` is stored as Add<Hole(0), Hole(0)>: concepts, not code. Nothing
    // in `map` knows what kind of realization it is applying, which is what
    // makes a capability learned years later usable here with no new machinery.
    let (store, reg) = brain();
    learn(
        &store,
        "double",
        Concept::call("add", [Concept::hole(0), Concept::hole(0)]),
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("map", [ints([1, 2, 3]), Concept::named("double")])
        ),
        ints([2, 4, 6])
    );
}

#[test]
fn map_over_an_empty_list_is_empty() {
    let (store, reg) = brain();
    learn(
        &store,
        "double",
        Concept::call("add", [Concept::hole(0), Concept::hole(0)]),
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("map", [ints([]), Concept::named("double")])
        ),
        ints([])
    );
}

#[test]
fn map_also_accepts_a_native_as_its_function() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("map", [texts(["a", "b"]), Concept::named("upper")])
        ),
        texts(["A", "B"])
    );
}

#[test]
fn filter_find_all_and_any_take_a_composed_predicate() {
    let (store, reg) = brain();
    learn(
        &store,
        "big",
        Concept::call("gt", [Concept::hole(0), Concept::int(2)]),
    );
    let l = ints([1, 2, 3, 4]);
    let big = Concept::named("big");

    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("filter", [l.clone(), big.clone()])
        ),
        ints([3, 4])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("find", [l.clone(), big.clone()])
        ),
        Concept::int(3)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("all", [l.clone(), big.clone()])
        ),
        Concept::bool(false)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("any", [l, big.clone()])),
        Concept::bool(true)
    );

    // Empty list: `all` is vacuously true, `any` has nothing to find.
    assert_eq!(
        value(&store, &reg, &Concept::call("all", [ints([]), big.clone()])),
        Concept::bool(true)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("any", [ints([]), big.clone()])),
        Concept::bool(false)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("filter", [ints([]), big.clone()])
        ),
        ints([])
    );

    let message = why_failed(&store, &reg, &Concept::call("find", [ints([1, 2]), big]));
    assert!(message.contains("no element satisfied"), "got {message}");
}

#[test]
fn a_predicate_that_does_not_return_a_boolean_is_a_type_error() {
    let (store, reg) = brain();
    learn(&store, "same", Concept::hole(0));
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("filter", [ints([1, 2]), Concept::named("same")]),
    );
    assert!(message.contains("filter"), "got {message}");
}

#[test]
fn reduce_folds_from_an_explicit_initial_value() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "reduce",
                [ints([1, 2, 3]), Concept::named("add"), Concept::int(10)]
            )
        ),
        Concept::int(16)
    );
}

#[test]
fn reduce_over_an_empty_list_returns_the_initial_value_untouched() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("reduce", [ints([]), Concept::named("add"), Concept::int(7)])
        ),
        Concept::int(7)
    );
}

#[test]
fn sort_by_orders_by_a_key_function() {
    let (store, reg) = brain();
    learn(&store, "same", Concept::hole(0));
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("sort-by", [ints([3, 1, 2]), Concept::named("same")])
        ),
        ints([1, 2, 3])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("sort-by", [ints([]), Concept::named("same")])
        ),
        ints([])
    );
}

#[test]
fn sort_by_is_stable_and_reproduces_across_runs() {
    // Every key is 2, so a stable sort must return the input untouched. An
    // unstable one would be free to shuffle, and a nondeterministic one would
    // shuffle differently each run.
    let (store, reg) = brain();
    let input = texts(["bb", "aa", "cc", "dd"]);
    let call = Concept::call("sort-by", [input.clone(), Concept::named("text-length")]);
    let first = value(&store, &reg, &call);
    assert_eq!(first, input, "equal keys should have preserved input order");
    for _ in 0..10 {
        assert_eq!(value(&store, &reg, &call), first);
    }
}

#[test]
fn sort_by_refuses_keys_that_have_no_shared_order() {
    let (store, reg) = brain();
    learn(&store, "same", Concept::hole(0));
    let mixed = list([Concept::int(1), Concept::text("apple")]);
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("sort-by", [mixed, Concept::named("same")]),
    );
    assert!(message.contains("no shared order"), "got {message}");

    // A key that is not a ground value at all cannot be ordered either.
    let compounds = list([Concept::call("point", [Concept::int(1)])]);
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("sort-by", [compounds, Concept::named("same")]),
    );
    assert!(message.contains("sort-by"), "got {message}");
}

#[test]
fn group_by_collects_in_first_appearance_order() {
    let (store, reg) = brain();
    learn(
        &store,
        "initial",
        Concept::call("char-at", [Concept::hole(0), Concept::int(0)]),
    );
    let out = value(
        &store,
        &reg,
        &Concept::call(
            "group-by",
            [
                texts(["apple", "berry", "avocado"]),
                Concept::named("initial"),
            ],
        ),
    );
    assert_eq!(
        out,
        list([
            Concept::call("group", [Concept::text("a"), texts(["apple", "avocado"])]),
            Concept::call("group", [Concept::text("b"), texts(["berry"])]),
        ])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("group-by", [texts([]), Concept::named("initial")])
        ),
        list([])
    );
}

// ---------------------------------------------------------------------------
// Numeric folds
// ---------------------------------------------------------------------------

#[test]
fn sum_and_product_widen_only_when_a_float_is_present() {
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("sum", [ints([1, 2, 3])])),
        Concept::int(6)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("product", [ints([2, 3, 4])])),
        Concept::int(24)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("sum", [list([Concept::int(1), Concept::float(0.5)])])
        ),
        Concept::float(1.5)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("product", [list([Concept::int(2), Concept::float(2.5)])])
        ),
        Concept::float(5.0)
    );
}

#[test]
fn sum_of_an_empty_list_is_zero_and_product_is_one() {
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("sum", [ints([])])),
        Concept::int(0)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("product", [ints([])])),
        Concept::int(1)
    );
}

#[test]
fn min_and_max_widen_and_refuse_an_empty_list() {
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("min-of", [ints([3, 1, 2])])),
        Concept::int(1)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("max-of", [ints([3, 1, 2])])),
        Concept::int(3)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("max-of", [list([Concept::int(3), Concept::float(1.5)])])
        ),
        Concept::float(3.0),
        "any float in the list widens the answer"
    );

    for native in ["min-of", "max-of"] {
        let message = why_failed(&store, &reg, &Concept::call(native, [ints([])]));
        assert!(
            message.contains("empty list has no extreme value"),
            "got {message}"
        );
    }
}

#[test]
fn sum_reports_overflow_rather_than_wrapping() {
    let (store, reg) = brain();
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("sum", [list([Concept::int(i64::MAX), Concept::int(1)])]),
    );
    assert!(message.contains("overflow"), "got {message}");
}

// ---------------------------------------------------------------------------
// Nesting
// ---------------------------------------------------------------------------

#[test]
fn operations_nest_because_they_are_ordinary_concepts() {
    let (store, reg) = brain();
    learn(
        &store,
        "double",
        Concept::call("add", [Concept::hole(0), Concept::hole(0)]),
    );
    let expr = Concept::call(
        "sum",
        [Concept::call(
            "map",
            [ints([1, 2, 3]), Concept::named("double")],
        )],
    );
    assert_eq!(value(&store, &reg, &expr), Concept::int(12));

    learn(
        &store,
        "big",
        Concept::call("gt", [Concept::hole(0), Concept::int(2)]),
    );
    let expr = Concept::call(
        "count",
        [Concept::call(
            "filter",
            [
                Concept::call("map", [ints([1, 2, 3]), Concept::named("double")]),
                Concept::named("big"),
            ],
        )],
    );
    // Doubling gives 2, 4, 6; only 4 and 6 are bigger than 2.
    assert_eq!(value(&store, &reg, &expr), Concept::int(2));
}

// ---------------------------------------------------------------------------
// Bad input never panics
// ---------------------------------------------------------------------------

#[test]
fn every_native_refuses_a_wrong_typed_argument_instead_of_panicking() {
    let (store, reg) = brain();
    let not_a_list = Concept::int(7);
    let not_a_number = Concept::text("nope");
    let f = Concept::named("upper");

    let bad: Vec<Concept> = vec![
        Concept::call("first", [not_a_list.clone()]),
        Concept::call("last", [not_a_list.clone()]),
        Concept::call("nth", [not_a_list.clone(), Concept::int(0)]),
        Concept::call("nth", [ints([1]), not_a_number.clone()]),
        Concept::call("count", [not_a_list.clone()]),
        Concept::call("is-empty", [not_a_list.clone()]),
        Concept::call("append", [not_a_list.clone(), Concept::int(1)]),
        Concept::call("prepend", [not_a_list.clone(), Concept::int(1)]),
        Concept::call("concat-lists", [ints([1]), not_a_list.clone()]),
        Concept::call("reverse", [not_a_list.clone()]),
        Concept::call(
            "slice",
            [not_a_list.clone(), Concept::int(0), Concept::int(0)],
        ),
        Concept::call("slice", [ints([1]), not_a_number.clone(), Concept::int(1)]),
        Concept::call("slice", [ints([1, 2]), Concept::int(2), Concept::int(1)]),
        Concept::call("contains", [not_a_list.clone(), Concept::int(1)]),
        Concept::call("index-of", [not_a_list.clone(), Concept::int(1)]),
        Concept::call("unique", [not_a_list.clone()]),
        Concept::call("flatten", [not_a_list.clone()]),
        Concept::call("map", [not_a_list.clone(), f.clone()]),
        Concept::call("filter", [not_a_list.clone(), f.clone()]),
        Concept::call("reduce", [not_a_list.clone(), f.clone(), Concept::int(0)]),
        Concept::call("sort-by", [not_a_list.clone(), f.clone()]),
        Concept::call("find", [not_a_list.clone(), f.clone()]),
        Concept::call("all", [not_a_list.clone(), f.clone()]),
        Concept::call("any", [not_a_list.clone(), f.clone()]),
        Concept::call("group-by", [not_a_list.clone(), f.clone()]),
        Concept::call("sum", [not_a_list.clone()]),
        Concept::call("sum", [list([not_a_number.clone()])]),
        Concept::call("product", [not_a_list.clone()]),
        Concept::call("min-of", [list([not_a_number.clone()])]),
        Concept::call("max-of", [not_a_list.clone()]),
        // Arity mismatches are caught before the native ever runs.
        Concept::call("first", []),
        Concept::call("nth", [ints([1])]),
        Concept::call("map", [ints([1])]),
    ];

    for expr in bad {
        let (outcome, _) = run(&store, &reg, &expr);
        assert!(
            !outcome.is_value(),
            "{expr:?} should not have produced a value: {outcome:?}"
        );
    }
}

#[test]
fn a_predicate_whose_function_does_not_exist_leaves_the_call_unfinished() {
    // `nonesuch` has no realization, so `Nonesuch<1>` reduces to itself rather
    // than to a boolean. `filter` then reports a type error instead of guessing.
    let (store, reg) = brain();
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("filter", [ints([1]), Concept::named("nonesuch")]),
    );
    assert!(message.contains("filter"), "got {message}");
}
