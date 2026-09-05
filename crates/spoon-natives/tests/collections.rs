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
    Concept::call("list-of", items)
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
        &Concept::call("list-of", [Concept::int(1), Concept::int(2)]),
    );
    assert_eq!(out, ints([1, 2]));
    assert_eq!(out.arity(), 2);
    assert_eq!(value(&store, &reg, &Concept::call("list-of", [])), ints([]));
}

#[test]
fn first_last_and_nth_read_positions() {
    let (store, reg) = brain();
    let l = ints([10, 20, 30]);
    assert_eq!(
        value(&store, &reg, &Concept::call("list-first", [l.clone()])),
        Concept::int(10)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-last", [l.clone()])),
        Concept::int(30)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-nth", [l, Concept::int(1)])),
        Concept::int(20)
    );
}

#[test]
fn count_and_is_empty_describe_size() {
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("list-count", [ints([1, 2, 3])])),
        Concept::int(3)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-count", [ints([])])),
        Concept::int(0)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-is-empty", [ints([])])),
        Concept::bool(true)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-is-empty", [ints([1])])),
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
            &Concept::call("list-append", [ints([1]), Concept::int(2), Concept::int(3)])
        ),
        ints([1, 2, 3])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-prepend", [ints([3]), Concept::int(1), Concept::int(2)])
        ),
        ints([1, 2, 3]),
        "extra arguments should not come out reversed"
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-append", [ints([])])),
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
            &Concept::call("list-concat", [ints([1, 2]), ints([]), ints([3])])
        ),
        ints([1, 2, 3])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-concat", [])),
        ints([])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-reverse", [ints([1, 2, 3])])),
        ints([3, 2, 1])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-reverse", [ints([])])),
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
            &Concept::call("list-slice", [l.clone(), Concept::int(1), Concept::int(3)])
        ),
        ints([2, 3])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-slice", [l.clone(), Concept::int(2), Concept::int(2)])
        ),
        ints([]),
        "an empty range is empty, not an error"
    );
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("list-slice", [l, Concept::int(0), Concept::int(9)]),
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
            &Concept::call("list-contains", [l.clone(), Concept::int(2)])
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-contains", [l.clone(), Concept::int(9)])
        ),
        Concept::bool(false)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-index-of", [l.clone(), Concept::int(2)])
        ),
        Concept::int(1),
        "index-of reports the first match"
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-unique", [l.clone()])),
        ints([1, 2, 3])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-unique", [ints([])])),
        ints([])
    );

    // Absence is an error rather than a -1 the caller can forget to check.
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("list-index-of", [l, Concept::int(9)]),
    );
    assert!(message.contains("list-index-of"), "got {message}");
}

#[test]
fn flatten_goes_exactly_one_level() {
    let (store, reg) = brain();
    let nested = list([ints([1, 2]), Concept::int(3), list([ints([4])])]);
    assert_eq!(
        value(&store, &reg, &Concept::call("list-flatten", [nested])),
        list([Concept::int(1), Concept::int(2), Concept::int(3), ints([4])]),
        "the doubly nested list should have come out one level shallower, not flat"
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-flatten", [ints([])])),
        ints([])
    );
}

// ---------------------------------------------------------------------------
// Out-of-range access
// ---------------------------------------------------------------------------

#[test]
fn out_of_range_access_names_the_index_and_the_length() {
    let (store, reg) = brain();

    let message = why_failed(&store, &reg, &Concept::call("list-first", [ints([])]));
    assert!(message.contains("out of range"), "got {message}");

    let message = why_failed(&store, &reg, &Concept::call("list-last", [ints([])]));
    assert!(message.contains("out of range"), "got {message}");

    let message = why_failed(
        &store,
        &reg,
        &Concept::call("list-nth", [ints([1, 2, 3]), Concept::int(7)]),
    );
    assert!(
        message.contains("index 7") && message.contains("length 3"),
        "the message should name both the index and the length, got {message}"
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call("list-nth", [ints([1, 2, 3]), Concept::int(-1)]),
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
        Concept::call("math-add", [Concept::hole(0), Concept::hole(0)]),
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-map", [ints([1, 2, 3]), Concept::named("double")])
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
        Concept::call("math-add", [Concept::hole(0), Concept::hole(0)]),
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-map", [ints([]), Concept::named("double")])
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
            &Concept::call("list-map", [texts(["a", "b"]), Concept::named("text-upper")])
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
        Concept::call("logic-gt", [Concept::hole(0), Concept::int(2)]),
    );
    let l = ints([1, 2, 3, 4]);
    let big = Concept::named("big");

    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-filter", [l.clone(), big.clone()])
        ),
        ints([3, 4])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-find", [l.clone(), big.clone()])
        ),
        Concept::int(3)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-all", [l.clone(), big.clone()])
        ),
        Concept::bool(false)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-any", [l, big.clone()])),
        Concept::bool(true)
    );

    // Empty list: `all` is vacuously true, `any` has nothing to find.
    assert_eq!(
        value(&store, &reg, &Concept::call("list-all", [ints([]), big.clone()])),
        Concept::bool(true)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-any", [ints([]), big.clone()])),
        Concept::bool(false)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-filter", [ints([]), big.clone()])
        ),
        ints([])
    );

    let message = why_failed(&store, &reg, &Concept::call("list-find", [ints([1, 2]), big]));
    assert!(message.contains("no element satisfied"), "got {message}");
}

#[test]
fn a_predicate_that_does_not_return_a_boolean_is_a_type_error() {
    let (store, reg) = brain();
    learn(&store, "same", Concept::hole(0));
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("list-filter", [ints([1, 2]), Concept::named("same")]),
    );
    assert!(message.contains("list-filter"), "got {message}");
}

#[test]
fn reduce_folds_from_an_explicit_initial_value() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "list-reduce",
                [ints([1, 2, 3]), Concept::named("math-add"), Concept::int(10)]
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
            &Concept::call("list-reduce", [ints([]), Concept::named("math-add"), Concept::int(7)])
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
            &Concept::call("list-sort-by", [ints([3, 1, 2]), Concept::named("same")])
        ),
        ints([1, 2, 3])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-sort-by", [ints([]), Concept::named("same")])
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
    let call = Concept::call("list-sort-by", [input.clone(), Concept::named("text-length")]);
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
        &Concept::call("list-sort-by", [mixed, Concept::named("same")]),
    );
    assert!(message.contains("no shared order"), "got {message}");

    // A key that is not a ground value at all cannot be ordered either.
    let compounds = list([Concept::call("point", [Concept::int(1)])]);
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("list-sort-by", [compounds, Concept::named("same")]),
    );
    assert!(message.contains("list-sort-by"), "got {message}");
}

#[test]
fn group_by_collects_in_first_appearance_order() {
    let (store, reg) = brain();
    learn(
        &store,
        "initial",
        Concept::call("text-char-at", [Concept::hole(0), Concept::int(0)]),
    );
    let out = value(
        &store,
        &reg,
        &Concept::call(
            "list-group-by",
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
            &Concept::call("list-group-by", [texts([]), Concept::named("initial")])
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
        value(&store, &reg, &Concept::call("math-sum", [ints([1, 2, 3])])),
        Concept::int(6)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("math-product", [ints([2, 3, 4])])),
        Concept::int(24)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("math-sum", [list([Concept::int(1), Concept::float(0.5)])])
        ),
        Concept::float(1.5)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("math-product", [list([Concept::int(2), Concept::float(2.5)])])
        ),
        Concept::float(5.0)
    );
}

#[test]
fn sum_of_an_empty_list_is_zero_and_product_is_one() {
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("math-sum", [ints([])])),
        Concept::int(0)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("math-product", [ints([])])),
        Concept::int(1)
    );
}

#[test]
fn min_and_max_widen_and_refuse_an_empty_list() {
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("list-min-of", [ints([3, 1, 2])])),
        Concept::int(1)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-max-of", [ints([3, 1, 2])])),
        Concept::int(3)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-max-of", [list([Concept::int(3), Concept::float(1.5)])])
        ),
        Concept::float(3.0),
        "any float in the list widens the answer"
    );

    for native in ["list-min-of", "list-max-of"] {
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
        &Concept::call("math-sum", [list([Concept::int(i64::MAX), Concept::int(1)])]),
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
        Concept::call("math-add", [Concept::hole(0), Concept::hole(0)]),
    );
    let expr = Concept::call(
        "math-sum",
        [Concept::call(
            "list-map",
            [ints([1, 2, 3]), Concept::named("double")],
        )],
    );
    assert_eq!(value(&store, &reg, &expr), Concept::int(12));

    learn(
        &store,
        "big",
        Concept::call("logic-gt", [Concept::hole(0), Concept::int(2)]),
    );
    let expr = Concept::call(
        "list-count",
        [Concept::call(
            "list-filter",
            [
                Concept::call("list-map", [ints([1, 2, 3]), Concept::named("double")]),
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
    let f = Concept::named("text-upper");

    let bad: Vec<Concept> = vec![
        Concept::call("list-first", [not_a_list.clone()]),
        Concept::call("list-last", [not_a_list.clone()]),
        Concept::call("list-nth", [not_a_list.clone(), Concept::int(0)]),
        Concept::call("list-nth", [ints([1]), not_a_number.clone()]),
        Concept::call("list-count", [not_a_list.clone()]),
        Concept::call("list-is-empty", [not_a_list.clone()]),
        Concept::call("list-append", [not_a_list.clone(), Concept::int(1)]),
        Concept::call("list-prepend", [not_a_list.clone(), Concept::int(1)]),
        Concept::call("list-concat", [ints([1]), not_a_list.clone()]),
        Concept::call("list-reverse", [not_a_list.clone()]),
        Concept::call(
            "list-slice",
            [not_a_list.clone(), Concept::int(0), Concept::int(0)],
        ),
        Concept::call("list-slice", [ints([1]), not_a_number.clone(), Concept::int(1)]),
        Concept::call("list-slice", [ints([1, 2]), Concept::int(2), Concept::int(1)]),
        Concept::call("list-contains", [not_a_list.clone(), Concept::int(1)]),
        Concept::call("list-index-of", [not_a_list.clone(), Concept::int(1)]),
        Concept::call("list-unique", [not_a_list.clone()]),
        Concept::call("list-flatten", [not_a_list.clone()]),
        Concept::call("list-map", [not_a_list.clone(), f.clone()]),
        Concept::call("list-filter", [not_a_list.clone(), f.clone()]),
        Concept::call("list-reduce", [not_a_list.clone(), f.clone(), Concept::int(0)]),
        Concept::call("list-sort-by", [not_a_list.clone(), f.clone()]),
        Concept::call("list-find", [not_a_list.clone(), f.clone()]),
        Concept::call("list-all", [not_a_list.clone(), f.clone()]),
        Concept::call("list-any", [not_a_list.clone(), f.clone()]),
        Concept::call("list-group-by", [not_a_list.clone(), f.clone()]),
        Concept::call("math-sum", [not_a_list.clone()]),
        Concept::call("math-sum", [list([not_a_number.clone()])]),
        Concept::call("math-product", [not_a_list.clone()]),
        Concept::call("list-min-of", [list([not_a_number.clone()])]),
        Concept::call("list-max-of", [not_a_list.clone()]),
        // Arity mismatches are caught before the native ever runs.
        Concept::call("list-first", []),
        Concept::call("list-nth", [ints([1])]),
        Concept::call("list-map", [ints([1])]),
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
        &Concept::call("list-filter", [ints([1]), Concept::named("nonesuch")]),
    );
    assert!(message.contains("list-filter"), "got {message}");
}

#[test]
fn sort_orders_a_list_by_its_own_elements() {
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("list-sort", [ints([3, 1, 2])])),
        ints([1, 2, 3])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-sort", [texts(["pear", "apple", "fig"])])
        ),
        texts(["apple", "fig", "pear"])
    );
    // Already ordered, and an empty list, both stay themselves.
    assert_eq!(
        value(&store, &reg, &Concept::call("list-sort", [ints([1, 2, 3])])),
        ints([1, 2, 3])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-sort", [ints([])])),
        ints([])
    );
}

#[test]
fn sort_refuses_values_with_no_shared_order() {
    // Guessing an order between 1 and "one" would sort silently and wrongly.
    let (store, reg) = brain();
    let mixed = list([Concept::int(1), Concept::text("one")]);
    let message = why_failed(&store, &reg, &Concept::call("list-sort", [mixed]));
    assert!(message.contains("list-sort"), "got {message}");
}

#[test]
fn reverse_gives_back_the_shape_it_was_given() {
    // The answer to a question about a word is a word. Going through `chars`
    // gave back list<"a", "n", ...> unless something remembered to join it.
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("list-reverse", [Concept::text("banana")])),
        Concept::text("ananab")
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-reverse", [ints([1, 2, 3])])),
        ints([3, 2, 1])
    );
    // By character, not by byte, or an accented word comes back broken.
    assert_eq!(
        value(&store, &reg, &Concept::call("list-reverse", [Concept::text("café")])),
        Concept::text("éfac")
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-reverse", [Concept::text("")])),
        Concept::text("")
    );
}

// ---------------------------------------------------------------------------
// New natives
// ---------------------------------------------------------------------------

#[test]
fn zip_pairs_positionally_and_truncates_to_the_shorter() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-zip", [ints([1, 2, 3]), texts(["a", "b"])])
        ),
        list([
            list([Concept::int(1), Concept::text("a")]),
            list([Concept::int(2), Concept::text("b")]),
        ])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-zip", [ints([]), ints([1, 2])])),
        list([])
    );
}

#[test]
fn enumerate_pairs_each_element_with_its_zero_based_index() {
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("list-enumerate", [texts(["a", "b"])])),
        list([
            list([Concept::int(0), Concept::text("a")]),
            list([Concept::int(1), Concept::text("b")]),
        ])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-enumerate", [ints([])])),
        list([])
    );
}

#[test]
fn take_and_drop_split_a_list_at_a_count() {
    let (store, reg) = brain();
    let l = ints([1, 2, 3, 4, 5]);
    assert_eq!(
        value(&store, &reg, &Concept::call("list-take", [l.clone(), Concept::int(2)])),
        ints([1, 2])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-drop", [l.clone(), Concept::int(2)])),
        ints([3, 4, 5])
    );
    // Asking for more than the list holds is not an error.
    assert_eq!(
        value(&store, &reg, &Concept::call("list-take", [l.clone(), Concept::int(99)])),
        l.clone()
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-drop", [l, Concept::int(99)])),
        ints([])
    );
}

#[test]
fn chunk_splits_into_consecutive_sub_lists() {
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("list-chunk", [ints([1, 2, 3, 4, 5]), Concept::int(2)])),
        list([ints([1, 2]), ints([3, 4]), ints([5])])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-chunk", [ints([]), Concept::int(2)])),
        list([])
    );
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("list-chunk", [ints([1, 2]), Concept::int(0)]),
    );
    assert!(message.contains("at least 1"), "got {message}");
}

#[test]
fn interleave_alternates_and_stops_at_the_shorter_list() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-interleave", [ints([1, 2, 3]), texts(["a", "b"])])
        ),
        list([
            Concept::int(1),
            Concept::text("a"),
            Concept::int(2),
            Concept::text("b"),
        ])
    );
}

#[test]
fn frequencies_counts_distinct_elements_in_first_appearance_order() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-frequencies", [texts(["a", "b", "a", "c", "b", "a"])])
        ),
        list([
            list([Concept::text("a"), Concept::int(3)]),
            list([Concept::text("b"), Concept::int(2)]),
            list([Concept::text("c"), Concept::int(1)]),
        ])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-frequencies", [ints([])])),
        list([])
    );
}

#[test]
fn window_produces_overlapping_runs_of_a_given_size() {
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("list-window", [ints([1, 2, 3, 4]), Concept::int(2)])),
        list([ints([1, 2]), ints([2, 3]), ints([3, 4])])
    );
    // Too few elements for even one window gives none, not an error.
    assert_eq!(
        value(&store, &reg, &Concept::call("list-window", [ints([1]), Concept::int(2)])),
        list([])
    );
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("list-window", [ints([1, 2]), Concept::int(0)]),
    );
    assert!(message.contains("at least 1"), "got {message}");
}

#[test]
fn rotate_shifts_left_and_wraps_around() {
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("list-rotate", [ints([1, 2, 3, 4]), Concept::int(1)])),
        ints([2, 3, 4, 1])
    );
    // Negative rotates right.
    assert_eq!(
        value(&store, &reg, &Concept::call("list-rotate", [ints([1, 2, 3, 4]), Concept::int(-1)])),
        ints([4, 1, 2, 3])
    );
    // A rotation by the length, or any multiple of it, is the identity.
    assert_eq!(
        value(&store, &reg, &Concept::call("list-rotate", [ints([1, 2, 3]), Concept::int(3)])),
        ints([1, 2, 3])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-rotate", [ints([]), Concept::int(5)])),
        ints([])
    );
}

#[test]
fn repeat_builds_n_copies_of_an_element() {
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("list-repeat", [Concept::text("x"), Concept::int(3)])),
        texts(["x", "x", "x"])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-repeat", [Concept::text("x"), Concept::int(0)])),
        list([])
    );
}

#[test]
fn flat_map_maps_then_flattens_one_level() {
    let (store, reg) = brain();
    learn(
        &store,
        "pair-with-self",
        Concept::call("list-of", [Concept::hole(0), Concept::hole(0)]),
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-flat-map", [ints([1, 2]), Concept::named("pair-with-self")])
        ),
        ints([1, 1, 2, 2])
    );
    // When the function's result is not itself a list, it is kept as one
    // element rather than an error: only the outer answer needs unwrapping.
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-flat-map", [ints([1, 2]), Concept::named("double")])
        ),
        list([
            Concept::call("double", [Concept::int(1)]),
            Concept::call("double", [Concept::int(2)]),
        ]),
        "an unrealized `double<x>` is not a list, so flat-map keeps it as a single element"
    );
}

#[test]
fn partition_splits_matching_from_non_matching_in_one_pass() {
    let (store, reg) = brain();
    learn(
        &store,
        "big",
        Concept::call("logic-gt", [Concept::hole(0), Concept::int(2)]),
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-partition", [ints([1, 2, 3, 4]), Concept::named("big")])
        ),
        list([ints([3, 4]), ints([1, 2])])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-partition", [ints([]), Concept::named("big")])
        ),
        list([ints([]), ints([])])
    );
}

#[test]
fn min_by_and_max_by_pick_the_element_with_the_extreme_key() {
    let (store, reg) = brain();
    learn(
        &store,
        "length",
        Concept::call("text-length", [Concept::hole(0)]),
    );
    let words = texts(["fig", "banana", "kiwi"]);
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-min-by", [words.clone(), Concept::named("length")])
        ),
        Concept::text("fig")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-max-by", [words, Concept::named("length")])
        ),
        Concept::text("banana")
    );

    for native in ["list-min-by", "list-max-by"] {
        let message = why_failed(
            &store,
            &reg,
            &Concept::call(native, [ints([]), Concept::named("length")]),
        );
        assert!(
            message.contains("empty list has no extreme value"),
            "got {message}"
        );
    }
}

#[test]
fn take_while_and_drop_while_split_on_the_first_failing_element() {
    let (store, reg) = brain();
    learn(
        &store,
        "small",
        Concept::call("logic-lt", [Concept::hole(0), Concept::int(3)]),
    );
    let l = ints([1, 2, 3, 1, 2]);
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-take-while", [l.clone(), Concept::named("small")])
        ),
        ints([1, 2]),
        "stops at the first failing element rather than filtering every match"
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-drop-while", [l, Concept::named("small")])
        ),
        ints([3, 1, 2])
    );
}

#[test]
fn scan_returns_every_running_accumulator_including_the_initial_value() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "list-scan",
                [ints([1, 2, 3]), Concept::int(0), Concept::named("math-add")]
            )
        ),
        ints([0, 1, 3, 6])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "list-scan",
                [ints([]), Concept::int(7), Concept::named("math-add")]
            )
        ),
        ints([7])
    );
}

#[test]
fn new_natives_refuse_wrong_typed_arguments_instead_of_panicking() {
    let (store, reg) = brain();
    let not_a_list = Concept::int(7);
    let f = Concept::named("text-upper");

    let bad: Vec<Concept> = vec![
        Concept::call("list-zip", [not_a_list.clone(), ints([1])]),
        Concept::call("list-zip", [ints([1]), not_a_list.clone()]),
        Concept::call("list-enumerate", [not_a_list.clone()]),
        Concept::call("list-take", [not_a_list.clone(), Concept::int(1)]),
        Concept::call("list-take", [ints([1]), Concept::text("nope")]),
        Concept::call("list-drop", [not_a_list.clone(), Concept::int(1)]),
        Concept::call("list-chunk", [not_a_list.clone(), Concept::int(1)]),
        Concept::call("list-interleave", [not_a_list.clone(), ints([1])]),
        Concept::call("list-frequencies", [not_a_list.clone()]),
        Concept::call("list-window", [not_a_list.clone(), Concept::int(1)]),
        Concept::call("list-rotate", [not_a_list.clone(), Concept::int(1)]),
        Concept::call("list-repeat", [Concept::int(1), Concept::text("nope")]),
        Concept::call("list-flat-map", [not_a_list.clone(), f.clone()]),
        Concept::call("list-partition", [not_a_list.clone(), f.clone()]),
        Concept::call("list-min-by", [not_a_list.clone(), f.clone()]),
        Concept::call("list-max-by", [not_a_list.clone(), f.clone()]),
        Concept::call("list-take-while", [not_a_list.clone(), f.clone()]),
        Concept::call("list-drop-while", [not_a_list.clone(), f.clone()]),
        Concept::call(
            "list-scan",
            [not_a_list.clone(), Concept::int(0), f.clone()],
        ),
        // Arity mismatches are caught before the native ever runs.
        Concept::call("list-zip", [ints([1])]),
        Concept::call("list-scan", [ints([1]), Concept::int(0)]),
    ];

    for expr in bad {
        let (outcome, _) = run(&store, &reg, &expr);
        assert!(
            !outcome.is_value(),
            "{expr:?} should not have produced a value: {outcome:?}"
        );
    }
}
