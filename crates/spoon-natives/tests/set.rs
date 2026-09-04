//! Sets, end to end through a real evaluator and store.

use chrono::{TimeZone, Utc};
use spoon_concept::Concept;
use spoon_eval::{Budget, Evaluator, NativeRegistry, Outcome};
use spoon_store::Store;

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap()
}

fn brain() -> (Store, NativeRegistry) {
    let store = Store::open_in_memory().expect("in-memory store");
    let registry = spoon_natives::bootstrap();
    spoon_natives::seed_bootstrap(&store, &registry).expect("seed");
    (store, registry)
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

fn list(items: impl IntoIterator<Item = Concept>) -> Concept {
    Concept::call("list-list", items)
}

fn ints(values: impl IntoIterator<Item = i64>) -> Concept {
    list(values.into_iter().map(Concept::int))
}

#[test]
fn set_set_dedups_keeping_first_appearance() {
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("set-set", [ints([1, 2, 2, 3, 1])])),
        ints([1, 2, 3])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("set-set", [ints([])])),
        ints([])
    );
}

#[test]
fn union_combines_and_dedups() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("set-union", [ints([1, 2]), ints([2, 3])])
        ),
        ints([1, 2, 3])
    );
}

#[test]
fn intersection_keeps_only_shared_elements() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("set-intersection", [ints([1, 2, 3]), ints([2, 3, 4])])
        ),
        ints([2, 3])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("set-intersection", [ints([1, 2]), ints([3, 4])])
        ),
        ints([])
    );
}

#[test]
fn difference_keeps_elements_only_in_a() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("set-difference", [ints([1, 2, 3]), ints([2, 3])])
        ),
        ints([1])
    );
}

#[test]
fn symmetric_difference_keeps_elements_in_exactly_one() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("set-symmetric-difference", [ints([1, 2, 3]), ints([2, 3, 4])])
        ),
        ints([1, 4])
    );
}

#[test]
fn is_subset_and_is_superset() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("set-is-subset", [ints([1, 2]), ints([1, 2, 3])])
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("set-is-subset", [ints([1, 4]), ints([1, 2, 3])])
        ),
        Concept::bool(false)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("set-is-superset", [ints([1, 2, 3]), ints([1, 2])])
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("set-is-superset", [ints([1, 2]), ints([1, 2, 3])])
        ),
        Concept::bool(false)
    );
}

#[test]
fn empty_set_is_a_subset_of_anything_including_itself() {
    let (store, reg) = brain();
    assert_eq!(
        value(&store, &reg, &Concept::call("set-is-subset", [ints([]), ints([])])),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("set-is-subset", [ints([]), ints([1, 2, 3])])
        ),
        Concept::bool(true)
    );
}

// ---------------------------------------------------------------------------
// Bad input never panics
// ---------------------------------------------------------------------------

#[test]
fn every_native_refuses_a_wrong_typed_argument_instead_of_panicking() {
    let (store, reg) = brain();
    let not_a_list = Concept::int(7);

    let bad: Vec<Concept> = vec![
        Concept::call("set-set", [not_a_list.clone()]),
        Concept::call("set-union", [not_a_list.clone(), ints([1])]),
        Concept::call("set-intersection", [ints([1]), not_a_list.clone()]),
        Concept::call("set-difference", [not_a_list.clone(), ints([1])]),
        Concept::call("set-symmetric-difference", [not_a_list.clone(), ints([1])]),
        Concept::call("set-is-subset", [not_a_list.clone(), ints([1])]),
        Concept::call("set-is-superset", [ints([1]), not_a_list]),
    ];
    for expr in bad {
        let (outcome, _) = run(&store, &reg, &expr);
        assert!(
            !outcome.is_value(),
            "{expr:?} should not have produced a value: {outcome:?}"
        );
    }
}
