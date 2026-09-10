//! Orchestrator review: the guarantees that protect the user's data and keep
//! evaluation reproducible.

use chrono::{TimeZone, Utc};
use spoon_concept::{Concept, ConceptMeta, Ground, Provenance, Tier};
use spoon_eval::{Budget, Evaluator, NativeRegistry, Outcome, PermissionMode};
use spoon_store::Store;

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap()
}

fn env() -> (Store, NativeRegistry) {
    let store = Store::open_in_memory().unwrap();
    let registry = spoon_natives::bootstrap();
    spoon_natives::seed_bootstrap(&store, &registry).unwrap();
    (store, registry)
}

fn eval_with(
    store: &Store,
    registry: &NativeRegistry,
    mode: PermissionMode,
    c: &Concept,
) -> Outcome {
    Evaluator::new(store, registry)
        .with_budget(Budget::deterministic())
        .with_permission(mode)
        .with_now(now())
        .evaluate(c)
}

fn eval(store: &Store, registry: &NativeRegistry, c: &Concept) -> Outcome {
    eval_with(store, registry, PermissionMode::Bypass, c)
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
// The safeguard
// ---------------------------------------------------------------------------

#[test]
fn a_write_is_suspended_under_the_default_mode_and_nothing_is_written() {
    // The permission layer is the only thing between a learned realization and
    // the user's data. Suspending but writing anyway would be worse than not
    // having it.
    let (store, reg) = env();
    let claim = Concept::call("owns", [Concept::named("greg"), Concept::named("dog")]);
    let expr = Concept::call("store-assert", [claim.clone()]);

    let out = eval_with(&store, &reg, PermissionMode::AskWrites, &expr);
    assert!(
        matches!(out, Outcome::NeedsPermission { .. }),
        "got {out:?}"
    );
    assert!(
        !store.holds(&claim).unwrap(),
        "the write happened despite suspending"
    );

    let out = eval_with(&store, &reg, PermissionMode::Bypass, &expr);
    assert!(out.is_value(), "got {out:?}");
    assert!(store.holds(&claim).unwrap());
}

#[test]
fn retraction_is_gated_the_same_way_as_assertion() {
    // Deleting is at least as consequential as writing. A gate on one and not
    // the other would be a hole.
    let (store, reg) = env();
    let claim = Concept::call("owns", [Concept::named("greg"), Concept::named("dog")]);
    eval(
        &store,
        &reg,
        &Concept::call("store-assert", [claim.clone()]),
    );
    assert!(store.holds(&claim).unwrap());

    let retract = Concept::call("store-retract-claim", [claim.clone()]);
    let out = eval_with(&store, &reg, PermissionMode::AskWrites, &retract);
    assert!(
        matches!(out, Outcome::NeedsPermission { .. }),
        "got {out:?}"
    );
    assert!(
        store.holds(&claim).unwrap(),
        "the retraction happened despite suspending"
    );
}

#[test]
fn reads_run_unattended_but_stop_under_always_ask() {
    let (store, reg) = env();
    let expr = Concept::call("store-exists", [Concept::named("anything")]);
    assert!(eval_with(&store, &reg, PermissionMode::AskWrites, &expr).is_value());
    assert!(matches!(
        eval_with(&store, &reg, PermissionMode::AlwaysAsk, &expr),
        Outcome::NeedsPermission { .. }
    ));
}

#[test]
fn a_write_buried_inside_a_pure_looking_call_is_still_gated() {
    // Wrapping a write in Map does not launder it. The evaluator charges the
    // effect where it actually happens, not where the call chain started.
    let (store, reg) = env();
    let claim = Concept::call("owns", [Concept::named("greg"), Concept::named("dog")]);
    let expr = Concept::call(
        "list-map",
        [
            Concept::call("list-of", [claim.clone()]),
            Concept::named("store-assert"),
        ],
    );
    let out = eval_with(&store, &reg, PermissionMode::AskWrites, &expr);
    assert!(
        matches!(out, Outcome::NeedsPermission { .. }),
        "a write was laundered through map: {out:?}"
    );
    assert!(
        !store.holds(&claim).unwrap(),
        "the laundered write actually landed"
    );
}

// ---------------------------------------------------------------------------
// The clock is injected, not read
// ---------------------------------------------------------------------------

#[test]
fn now_reports_the_context_clock_not_the_system_clock() {
    // Reading the system clock inside a native would defeat the whole point of
    // an injectable clock and make every time-dependent test flaky.
    let (store, reg) = env();
    for pinned in [
        Utc.with_ymd_and_hms(1999, 12, 31, 23, 59, 59).unwrap(),
        Utc.with_ymd_and_hms(2030, 6, 1, 0, 0, 0).unwrap(),
    ] {
        let out = Evaluator::new(&store, &reg)
            .with_budget(Budget::deterministic())
            .with_permission(PermissionMode::Bypass)
            .with_now(pinned)
            .evaluate(&Concept::call("time-now", []));
        assert_eq!(
            out.value().and_then(|c| c.as_ground()).cloned(),
            Some(Ground::DateTime(pinned)),
            "now did not follow the injected clock"
        );
    }
}

#[test]
fn time_comparison_and_arithmetic_agree() {
    let (store, reg) = env();
    let base = Concept::datetime(now());
    let later = value(
        &store,
        &reg,
        &Concept::call("time-add-duration", [base.clone(), Concept::int(3600)]),
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("time-before", [base.clone(), later.clone()])
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("time-after", [base.clone(), later.clone()])
        ),
        Concept::bool(false)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("time-diff", [later, base])),
        Concept::int(3600)
    );
}

// ---------------------------------------------------------------------------
// JSON crosses into real concepts
// ---------------------------------------------------------------------------

#[test]
fn a_plucked_number_is_a_number_arithmetic_can_use() {
    // If extraction handed back an opaque JSON scalar, every downstream sum
    // would need an unwrapping step. Crossing the boundary once, at extraction,
    // is what makes "get the weights and add them up" a two-step plan.
    let (store, reg) = env();
    let payload = serde_json::json!({"probes": [{"score": 20}, {"score": 22}]});
    let expr = Concept::call(
        "math-sum",
        [Concept::call(
            "json-pluck",
            [
                Concept::call(
                    "json-field",
                    [Concept::json(payload), Concept::text("probes")],
                ),
                Concept::text("score"),
            ],
        )],
    );
    assert_eq!(value(&store, &reg, &expr), Concept::int(42));
}

#[test]
fn a_missing_field_is_named_rather_than_guessed_as_null() {
    // Returning null for a typo lets the mistake propagate silently through
    // everything downstream.
    let (store, reg) = env();
    let payload = Concept::json(serde_json::json!({"score": 1}));
    let out = eval(
        &store,
        &reg,
        &Concept::call("json-field", [payload, Concept::text("scoer")]),
    );
    assert!(is_error(&out), "a missing field returned {out:?}");
}

#[test]
fn malformed_json_errors_rather_than_producing_an_empty_document() {
    let (store, reg) = env();
    for bad in ["{", "not json", "{\"a\": }", ""] {
        let out = eval(
            &store,
            &reg,
            &Concept::call("json-parse", [Concept::text(bad)]),
        );
        assert!(is_error(&out), "parse-json({bad:?}) gave {out:?}");
    }
}

#[test]
fn json_scalars_become_concepts_but_structure_stays_json() {
    let (store, reg) = env();
    let payload = Concept::json(serde_json::json!({
        "n": 7, "f": 1.5, "s": "hi", "b": true, "arr": [1], "obj": {"k": 1}
    }));
    let expect = [
        ("n", Ground::Int(7)),
        ("f", Ground::Float(1.5)),
        ("s", Ground::text("hi")),
        ("b", Ground::Bool(true)),
    ];
    for (key, want) in expect {
        let got = value(
            &store,
            &reg,
            &Concept::call("json-field", [payload.clone(), Concept::text(key)]),
        );
        assert_eq!(got.as_ground(), Some(&want), "field {key}");
    }
    for key in ["arr", "obj"] {
        let got = value(
            &store,
            &reg,
            &Concept::call("json-field", [payload.clone(), Concept::text(key)]),
        );
        assert!(
            matches!(got.as_ground(), Some(Ground::Json(_))),
            "structure at {key} should stay JSON, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Recall is reproducible
// ---------------------------------------------------------------------------

#[test]
fn recall_finds_the_matches_and_nothing_else() {
    let (store, reg) = env();
    for (owner, thing) in [("greg", "dog"), ("keal", "dog"), ("greg", "car")] {
        let c = Concept::call("owns", [Concept::named(owner), Concept::named(thing)]);
        store
            .assert_concept(&c, Provenance::User { episode: None }, None, None)
            .unwrap();
    }
    store
        .assert_concept(
            &Concept::call(
                "friend-with",
                [Concept::named("greg"), Concept::named("keal")],
            ),
            Provenance::User { episode: None },
            None,
            None,
        )
        .unwrap();

    // Who owns a dog?
    let found = value(
        &store,
        &reg,
        &Concept::call(
            "store-recall",
            [Concept::call(
                "owns",
                [Concept::hole(0), Concept::named("dog")],
            )],
        ),
    );
    assert_eq!(found.arity(), 2, "got {found:?}");
    for match_ in found.args() {
        assert_eq!(match_.arg(1), Some(&Concept::named("dog")));
    }
}

#[test]
fn recall_order_does_not_depend_on_insertion_order() {
    // Two brains holding the same facts must answer identically, or a seed
    // round trip changes behaviour and every downstream test turns flaky.
    let facts: Vec<(&str, &str)> = vec![("a", "dog"), ("b", "dog"), ("c", "dog"), ("d", "dog")];
    let pattern = Concept::call("owns", [Concept::hole(0), Concept::named("dog")]);

    let mut answers = Vec::new();
    for reverse in [false, true] {
        let (store, reg) = env();
        let ordered: Vec<_> = if reverse {
            facts.iter().rev().cloned().collect()
        } else {
            facts.clone()
        };
        for (owner, thing) in ordered {
            store
                .assert_concept(
                    &Concept::call("owns", [Concept::named(owner), Concept::named(thing)]),
                    Provenance::User { episode: None },
                    None,
                    None,
                )
                .unwrap();
        }
        answers.push(value(
            &store,
            &reg,
            &Concept::call("store-recall", [pattern.clone()]),
        ));
    }
    assert_eq!(
        answers[0], answers[1],
        "recall order followed insertion order"
    );
}

#[test]
fn describe_keeps_the_preference_order_it_was_given() {
    // surface_forms is an ordered preference list; the first entry is what the
    // mouth should reach for. Sorting it would silently discard that.
    let (store, reg) = env();
    let greg = Concept::named("greg");
    store
        .put_meta(
            &ConceptMeta::new(
                greg.clone(),
                Provenance::User { episode: None },
                Tier::Provisional,
                now(),
            )
            .with_surface_forms(["Greg", "Greg Littlefield", "that guy"]),
        )
        .unwrap();

    let out = value(&store, &reg, &Concept::call("store-describe", [greg]));
    assert_eq!(
        out,
        Concept::call(
            "list-of",
            [
                Concept::text("Greg"),
                Concept::text("Greg Littlefield"),
                Concept::text("that guy")
            ]
        ),
        "preference order was not preserved"
    );
}

#[test]
fn an_all_hole_pattern_is_refused_rather_than_reading_the_whole_brain() {
    let (store, reg) = env();
    let out = eval(
        &store,
        &reg,
        &Concept::call("store-recall", [Concept::hole(0)]),
    );
    assert!(is_error(&out), "an unanchored recall was allowed: {out:?}");
}

// ---------------------------------------------------------------------------
// Nothing panics
// ---------------------------------------------------------------------------

#[test]
fn no_native_panics_on_nonsense_arguments() {
    let (store, reg) = env();
    let junk = [
        Concept::named("nonsense"),
        Concept::text("%Q not a format"),
        Concept::int(-1),
        Concept::json(serde_json::json!(null)),
        Concept::hole(0),
        Concept::call("list-of", [Concept::bool(true)]),
    ];
    for name in reg.names() {
        for arity in 0..4usize {
            for bad in &junk {
                let args: Vec<Concept> = std::iter::repeat_n(bad.clone(), arity).collect();
                let expr = Concept::apply(
                    Concept::symbol(spoon_concept::SymbolId::of(name.as_str())),
                    args,
                );
                let _ = eval(&store, &reg, &expr);
            }
        }
    }
}
