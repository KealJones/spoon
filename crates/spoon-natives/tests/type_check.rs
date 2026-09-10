//! Type inspection and coercion, end to end through a real evaluator and store.

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

fn why_failed(store: &Store, reg: &NativeRegistry, c: &Concept) -> String {
    let (outcome, failures) = run(store, reg, c);
    assert!(!outcome.is_value(), "expected a failure, got {outcome:?}");
    failures.join(" | ")
}

fn list(items: impl IntoIterator<Item = Concept>) -> Concept {
    Concept::call("list-of", items)
}

// ---------------------------------------------------------------------------
// type-of
// ---------------------------------------------------------------------------

#[test]
fn type_of_names_every_shape() {
    let (store, reg) = brain();
    let cases: Vec<(Concept, &str)> = vec![
        (Concept::text("hi"), "text"),
        (Concept::int(1), "int"),
        (Concept::float(1.5), "float"),
        (Concept::bool(true), "bool"),
        (list([Concept::int(1)]), "list"),
        (Concept::json(serde_json::json!({"a": 1})), "json"),
        (Concept::named("greg"), "concept"),
        (
            Concept::call("friend-with", [Concept::named("greg")]),
            "concept",
        ),
    ];
    for (c, expected) in cases {
        assert_eq!(
            value(&store, &reg, &Concept::call("type-of", [c.clone()])),
            Concept::text(expected),
            "type-of {c:?} should be {expected}"
        );
    }
}

#[test]
fn a_json_array_is_reported_as_a_list() {
    let (store, reg) = brain();
    let arr = Concept::json(serde_json::json!([1, 2, 3]));
    assert_eq!(
        value(&store, &reg, &Concept::call("type-of", [arr])),
        Concept::text("list")
    );
}

// ---------------------------------------------------------------------------
// type-is-*
// ---------------------------------------------------------------------------

#[test]
fn type_is_text() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-is-text", [Concept::text("a")])
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-is-text", [Concept::int(1)])
        ),
        Concept::bool(false)
    );
}

#[test]
fn type_is_number_covers_int_and_float() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-is-number", [Concept::int(1)])
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-is-number", [Concept::float(1.5)])
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-is-number", [Concept::text("1")])
        ),
        Concept::bool(false)
    );
}

#[test]
fn type_is_list() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-is-list", [list([Concept::int(1)])])
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-is-list", [Concept::int(1)])
        ),
        Concept::bool(false)
    );
}

#[test]
fn type_is_bool() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-is-bool", [Concept::bool(false)])
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-is-bool", [Concept::int(0)])
        ),
        Concept::bool(false)
    );
}

#[test]
fn type_is_concept_is_true_for_named_and_compound_and_false_for_ground_and_lists() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-is-concept", [Concept::named("greg")])
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "type-is-concept",
                [Concept::call("friend-with", [Concept::named("greg")])]
            )
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-is-concept", [Concept::int(1)])
        ),
        Concept::bool(false)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-is-concept", [list([Concept::int(1)])])
        ),
        Concept::bool(false)
    );
}

// ---------------------------------------------------------------------------
// Coercion
// ---------------------------------------------------------------------------

#[test]
fn type_to_number_parses_int_before_float() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-to-number", [Concept::text("42")])
        ),
        Concept::int(42)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-to-number", [Concept::text("3.5")])
        ),
        Concept::float(3.5)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-to-number", [Concept::text("  7  ")])
        ),
        Concept::int(7)
    );
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("type-to-number", [Concept::text("nope")]),
    );
    assert!(message.contains("not a number"), "got {message}");
}

#[test]
fn type_to_bool_coerces_common_falsy_values() {
    let (store, reg) = brain();
    let falsy = [
        Concept::int(0),
        Concept::float(0.0),
        Concept::text(""),
        Concept::text("false"),
        Concept::text("FALSE"),
        Concept::bool(false),
    ];
    for c in falsy {
        assert_eq!(
            value(&store, &reg, &Concept::call("type-to-bool", [c.clone()])),
            Concept::bool(false),
            "{c:?} should be false"
        );
    }
    let truthy = [
        Concept::int(1),
        Concept::int(-1),
        Concept::float(0.1),
        Concept::text("hello"),
        Concept::text("0"),
        Concept::bool(true),
    ];
    for c in truthy {
        assert_eq!(
            value(&store, &reg, &Concept::call("type-to-bool", [c.clone()])),
            Concept::bool(true),
            "{c:?} should be true"
        );
    }
}

#[test]
fn type_to_int_truncates_and_passes_through() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-to-int", [Concept::float(3.9)])
        ),
        Concept::int(3)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-to-int", [Concept::float(-3.9)])
        ),
        Concept::int(-3)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-to-int", [Concept::int(5)])
        ),
        Concept::int(5)
    );
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("type-to-int", [Concept::text("5")]),
    );
    assert!(message.contains("type-to-int"), "got {message}");
}

#[test]
fn type_to_float_widens_and_passes_through() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-to-float", [Concept::int(5)])
        ),
        Concept::float(5.0)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-to-float", [Concept::float(1.5)])
        ),
        Concept::float(1.5)
    );
}

#[test]
fn type_coerce_dispatches_on_target_type_text() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-coerce", [Concept::int(5), Concept::text("text")])
        ),
        Concept::text("5")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-coerce", [Concept::float(3.9), Concept::text("int")])
        ),
        Concept::int(3)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-coerce", [Concept::int(5), Concept::text("float")])
        ),
        Concept::float(5.0)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("type-coerce", [Concept::int(0), Concept::text("bool")])
        ),
        Concept::bool(false)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "type-coerce",
                [Concept::text("42"), Concept::text("number")]
            )
        ),
        Concept::int(42)
    );
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("type-coerce", [Concept::int(1), Concept::text("nonesuch")]),
    );
    assert!(message.contains("nonesuch"), "got {message}");
}

// ---------------------------------------------------------------------------
// Bad input never panics
// ---------------------------------------------------------------------------

#[test]
fn every_native_refuses_a_wrong_typed_argument_instead_of_panicking() {
    let (store, reg) = brain();
    let bad: Vec<Concept> = vec![
        Concept::call("type-to-number", [Concept::int(1)]),
        Concept::call("type-to-int", [Concept::text("x")]),
        Concept::call("type-to-float", [Concept::text("x")]),
        Concept::call("type-coerce", [Concept::int(1), Concept::int(2)]),
        Concept::call("type-of", []),
        Concept::call("type-is-text", []),
    ];
    for expr in bad {
        let (outcome, _) = run(&store, &reg, &expr);
        assert!(
            !outcome.is_value(),
            "{expr:?} should not have produced a value: {outcome:?}"
        );
    }
}
