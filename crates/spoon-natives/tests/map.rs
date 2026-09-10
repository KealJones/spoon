//! Maps, end to end through a real evaluator and store.

use chrono::{TimeZone, Utc};
use spoon_concept::{Activation, Concept, Effect, Provenance, Realization, RealizationSpec, Tier};
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

fn why_failed(store: &Store, reg: &NativeRegistry, c: &Concept) -> String {
    let (outcome, failures) = run(store, reg, c);
    assert!(!outcome.is_value(), "expected a failure, got {outcome:?}");
    failures.join(" | ")
}

fn list(items: impl IntoIterator<Item = Concept>) -> Concept {
    Concept::call("list-of", items)
}

fn obj(v: serde_json::Value) -> Concept {
    Concept::json(v)
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

#[test]
fn keys_values_and_entries_agree_on_order() {
    let (store, reg) = brain();
    let m = obj(serde_json::json!({"b": 2, "a": 1}));
    assert_eq!(
        value(&store, &reg, &Concept::call("map-keys", [m.clone()])),
        list([Concept::text("a"), Concept::text("b")])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("map-values", [m.clone()])),
        list([Concept::int(1), Concept::int(2)])
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("map-entries", [m])),
        list([
            list([Concept::text("a"), Concept::int(1)]),
            list([Concept::text("b"), Concept::int(2)]),
        ])
    );
}

#[test]
fn from_entries_builds_a_map() {
    let (store, reg) = brain();
    let pairs = list([
        list([Concept::text("x"), Concept::int(1)]),
        list([Concept::text("y"), Concept::text("hi")]),
    ]);
    let out = value(&store, &reg, &Concept::call("map-from-entries", [pairs]));
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("map-get", [out.clone(), Concept::text("x")])
        ),
        Concept::int(1)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("map-get", [out, Concept::text("y")])
        ),
        Concept::text("hi")
    );
}

#[test]
fn get_reads_a_value_and_errors_on_a_missing_key() {
    let (store, reg) = brain();
    let m = obj(serde_json::json!({"name": "greg"}));
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("map-get", [m.clone(), Concept::text("name")])
        ),
        Concept::text("greg")
    );
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("map-get", [m, Concept::text("missing")]),
    );
    assert!(message.contains("missing"), "got {message}");
}

#[test]
fn has_key_answers_presence() {
    let (store, reg) = brain();
    let m = obj(serde_json::json!({"a": 1}));
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("map-has-key", [m.clone(), Concept::text("a")])
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("map-has-key", [m, Concept::text("z")])
        ),
        Concept::bool(false)
    );
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

#[test]
fn set_adds_or_replaces_a_key() {
    let (store, reg) = brain();
    let m = obj(serde_json::json!({"a": 1}));
    let out = value(
        &store,
        &reg,
        &Concept::call("map-set", [m, Concept::text("b"), Concept::int(2)]),
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("map-get", [out.clone(), Concept::text("a")])
        ),
        Concept::int(1)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("map-get", [out, Concept::text("b")])
        ),
        Concept::int(2)
    );
}

#[test]
fn merge_lets_the_second_map_win_on_conflict() {
    let (store, reg) = brain();
    let a = obj(serde_json::json!({"a": 1, "b": 1}));
    let b = obj(serde_json::json!({"b": 2, "c": 3}));
    let out = value(&store, &reg, &Concept::call("map-merge", [a, b]));
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("map-get", [out.clone(), Concept::text("a")])
        ),
        Concept::int(1)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("map-get", [out.clone(), Concept::text("b")])
        ),
        Concept::int(2)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("map-get", [out, Concept::text("c")])
        ),
        Concept::int(3)
    );
}

#[test]
fn pick_and_omit_are_complementary() {
    let (store, reg) = brain();
    let m = obj(serde_json::json!({"a": 1, "b": 2, "c": 3}));
    let keys = list([Concept::text("a"), Concept::text("c")]);
    let picked = value(
        &store,
        &reg,
        &Concept::call("map-pick", [m.clone(), keys.clone()]),
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("map-keys", [picked])),
        list([Concept::text("a"), Concept::text("c")])
    );
    let omitted = value(&store, &reg, &Concept::call("map-omit", [m, keys]));
    assert_eq!(
        value(&store, &reg, &Concept::call("map-keys", [omitted])),
        list([Concept::text("b")])
    );
}

#[test]
fn update_applies_a_function_to_one_value() {
    let (store, reg) = brain();
    learn(
        &store,
        "double",
        Concept::call("math-add", [Concept::hole(0), Concept::hole(0)]),
    );
    let m = obj(serde_json::json!({"count": 4, "other": "untouched"}));
    let out = value(
        &store,
        &reg,
        &Concept::call(
            "map-update",
            [m, Concept::text("count"), Concept::named("double")],
        ),
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("map-get", [out.clone(), Concept::text("count")])
        ),
        Concept::int(8)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("map-get", [out, Concept::text("other")])
        ),
        Concept::text("untouched")
    );
}

#[test]
fn update_errors_on_a_missing_key() {
    let (store, reg) = brain();
    let m = obj(serde_json::json!({"a": 1}));
    let message = why_failed(
        &store,
        &reg,
        &Concept::call(
            "map-update",
            [m, Concept::text("z"), Concept::named("text-upper")],
        ),
    );
    assert!(
        message.contains("map-update") || message.contains('z'),
        "got {message}"
    );
}

// ---------------------------------------------------------------------------
// Bad input never panics
// ---------------------------------------------------------------------------

#[test]
fn every_native_refuses_a_wrong_typed_argument_instead_of_panicking() {
    let (store, reg) = brain();
    let not_a_map = Concept::int(7);
    let m = obj(serde_json::json!({"a": 1}));

    let bad: Vec<Concept> = vec![
        Concept::call("map-keys", [not_a_map.clone()]),
        Concept::call("map-values", [not_a_map.clone()]),
        Concept::call("map-entries", [not_a_map.clone()]),
        Concept::call("map-from-entries", [not_a_map.clone()]),
        Concept::call("map-get", [not_a_map.clone(), Concept::text("a")]),
        Concept::call("map-get", [m.clone(), Concept::int(1)]),
        Concept::call(
            "map-set",
            [not_a_map.clone(), Concept::text("a"), Concept::int(1)],
        ),
        Concept::call("map-has-key", [not_a_map.clone(), Concept::text("a")]),
        Concept::call("map-merge", [not_a_map.clone(), m.clone()]),
        Concept::call("map-pick", [not_a_map.clone(), list([Concept::text("a")])]),
        Concept::call("map-omit", [not_a_map.clone(), list([Concept::text("a")])]),
        Concept::call(
            "map-update",
            [not_a_map, Concept::text("a"), Concept::named("text-upper")],
        ),
    ];
    for expr in bad {
        let (outcome, _) = run(&store, &reg, &expr);
        assert!(
            !outcome.is_value(),
            "{expr:?} should not have produced a value: {outcome:?}"
        );
    }
}
