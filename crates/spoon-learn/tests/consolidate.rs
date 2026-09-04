//! Consolidation, exercised end to end.
//!
//! The interesting assertions here are the negative ones. Finding a shared
//! shape is easy; anti-unification always finds one. What makes consolidation
//! safe is refusing the shapes that are shared by accident, so most of this
//! file is about what does *not* come back.

use std::sync::Arc;

use chrono::{Duration, Utc};
use spoon_concept::{
    Activation, Concept, Effect, Provenance, Realization, RealizationSpec, SymbolTable, Tier, holes,
};
use spoon_eval::{Budget, Evaluator, NativeRegistry, Outcome};
use spoon_learn::{
    Abstraction, ConsolidateConfig, apply_abstraction, consolidate, consolidate_store, evict_unused,
};
use spoon_store::Store;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn table(names: &[&str]) -> Arc<SymbolTable> {
    let table = SymbolTable::new();
    for name in names {
        table.intern(name);
    }
    Arc::new(table)
}

fn config(names: &[&str]) -> ConsolidateConfig {
    ConsolidateConfig {
        names: Some(table(names)),
        ..ConsolidateConfig::default()
    }
}

fn int(v: i64) -> Concept {
    Concept::int(v)
}

/// `Add<Mul<k, 2>, 1>`: the running example, one constant apart each time.
fn affine(k: i64) -> Concept {
    Concept::call("add", [Concept::call("mul", [int(k), int(2)]), int(1)])
}

fn brain() -> (Store, NativeRegistry) {
    let registry = spoon_natives::bootstrap();
    let store = Store::open_in_memory().unwrap();
    spoon_natives::seed_bootstrap(&store, &registry).unwrap();
    (store, registry)
}

/// Store `body` as the composed realization of a freshly named concept.
fn put_composed(store: &Store, name: &str, body: Concept) {
    store.register_symbol(name).unwrap();
    store
        .put_realization(&Realization {
            target: Concept::named(name),
            name: format!("body-{name}").into(),
            spec: RealizationSpec::Composed { body },
            effect: Effect::Pure,
            activation: Activation::new(Utc::now()),
            provenance: Provenance::Synthesized { episode: None },
            tier: Tier::Provisional,
        })
        .unwrap();
}

fn stored_body(store: &Store, name: &str) -> Concept {
    match store
        .realization_by_name(&format!("body-{name}"))
        .unwrap()
        .expect("realization is stored")
        .spec
    {
        RealizationSpec::Composed { body } => body,
        other => panic!("expected a composed realization, got {other:?}"),
    }
}

fn evaluate(store: &Store, registry: &NativeRegistry, concept: &Concept) -> Outcome {
    Evaluator::new(store, registry)
        .with_budget(Budget::deterministic())
        .evaluate(concept)
}

fn call(name: &str) -> Concept {
    Concept::call(name, Vec::new())
}

// ---------------------------------------------------------------------------
// Finding the shape
// ---------------------------------------------------------------------------

#[test]
fn three_bodies_sharing_a_shape_yield_one_abstraction() {
    let bodies = vec![affine(3), affine(5), affine(7)];
    let found = consolidate(&bodies, config(&["add", "mul"]));

    assert_eq!(found.len(), 1, "got {found:#?}");
    let abstraction = &found[0];
    assert_eq!(
        abstraction.body,
        Concept::call(
            "add",
            [Concept::call("mul", [Concept::hole(0), int(2)]), int(1)]
        )
    );
    assert_eq!(abstraction.arity, 1);
    assert_eq!(abstraction.instances, 3);
    assert!(abstraction.utility > 0.0);

    // The inner Mul<?0, 2> also recurs three times. It is not returned: it
    // saves one node per use and costs four to define, and it overlaps the
    // winner anyway.
    for other in &found {
        assert_ne!(other.body, Concept::call("mul", [Concept::hole(0), int(2)]));
    }
}

#[test]
fn a_shape_seen_twice_is_not_a_pattern() {
    let bodies = vec![affine(3), affine(5)];
    assert!(consolidate(&bodies, config(&["add", "mul"])).is_empty());

    // The same corpus with the bar lowered does produce it, so the emptiness
    // above is the recurrence floor and not some other filter.
    let lenient = ConsolidateConfig {
        min_count: 2,
        ..config(&["add", "mul"])
    };
    assert_eq!(consolidate(&bodies, lenient).len(), 1);
}

#[test]
fn shared_holes_survive_generalizing_more_than_two_instances() {
    // Both positions of the pair move together in every instance, so the
    // generalization has to say so. Two independent holes would let the
    // abstraction be called with two different values, which is a different
    // and weaker claim than the corpus supports.
    let body = |v: i64| Concept::call("wrap", [Concept::call("pair", [int(v), int(v)]), int(1)]);
    let bodies = vec![body(7), body(9), body(4)];
    let found = consolidate(&bodies, config(&["wrap", "pair"]));

    assert_eq!(found.len(), 1, "got {found:#?}");
    let abstraction = &found[0];
    assert_eq!(
        holes(&abstraction.body).len(),
        1,
        "got {:?}",
        abstraction.body
    );
    assert_eq!(abstraction.arity, 1);

    let pair = abstraction.body.arg(0).expect("first argument");
    assert_eq!(
        pair.arg(0),
        pair.arg(1),
        "the two positions must share a hole"
    );
}

// ---------------------------------------------------------------------------
// Refusing the shapes that are shared by accident
// ---------------------------------------------------------------------------

#[test]
fn over_generalization_is_refused() {
    // Eight procedures that all apply F to three unrelated things and a 3.
    // They share a shape and the shape recurs often enough to pay for itself,
    // but F<?0, ?1, ?2, 3> is a coincidence of arity, not a concept. Naming it
    // would make Spoon treat eight unrelated procedures as the same thing,
    // which is the harmful transfer this guard exists to prevent.
    let names = ["f", "a0", "a1", "a2", "b0", "b1", "b2", "c0", "c1", "c2"];
    let bodies: Vec<Concept> = (0..8)
        .map(|i| {
            Concept::call(
                "f",
                [
                    Concept::named(&format!("x{i}")),
                    Concept::named(&format!("y{i}")),
                    Concept::named(&format!("z{i}")),
                    int(3),
                ],
            )
        })
        .collect();

    assert!(
        consolidate(&bodies, config(&names)).is_empty(),
        "a mostly-hole pattern must not be named"
    );

    // With the guard switched off the very same corpus does produce it, which
    // is what proves the guard did the refusing rather than the utility test.
    let unguarded = ConsolidateConfig {
        min_concrete_ratio: 0.0,
        ..config(&names)
    };
    let found = consolidate(&bodies, unguarded);
    assert_eq!(found.len(), 1);
    assert_eq!(holes(&found[0].body).len(), 3);
}

#[test]
fn bodies_with_nothing_in_common_produce_nothing() {
    let bodies = vec![
        Concept::call("pair", [Concept::named("greg"), int(1)]),
        Concept::call("pair", [Concept::named("keal"), int(2)]),
        Concept::call("pair", [Concept::named("bob"), int(3)]),
    ];
    assert!(
        consolidate(&bodies, config(&["pair", "greg", "keal", "bob"])).is_empty(),
        "Pair<?0, ?1> is a constructor, not a concept"
    );

    // Different heads never even meet: they land in different buckets, so no
    // amount of leniency turns them into one abstraction.
    let unrelated = vec![
        Concept::call("f", [int(1), int(2)]),
        Concept::call("g", [int(3), int(4)]),
        Concept::call("h", [int(5), int(6)]),
    ];
    let lenient = ConsolidateConfig {
        min_count: 1,
        min_concrete_ratio: 0.0,
        min_utility: -1000.0,
        ..config(&["f", "g", "h"])
    };
    for abstraction in consolidate(&unrelated, lenient) {
        assert!(
            abstraction
                .body
                .head()
                .and_then(Concept::as_symbol)
                .is_some(),
            "no abstraction may generalize away the operation itself: {:?}",
            abstraction.body
        );
    }
}

#[test]
fn an_abstraction_that_saves_nothing_is_rejected() {
    // Mul<?0, 2> recurs three times and is three quarters concrete, so it
    // clears the recurrence floor and the over-generalization guard. It still
    // loses: it saves one node per use and costs four nodes to define.
    let bodies: Vec<Concept> = [1, 3, 5]
        .iter()
        .map(|k| Concept::call("mul", [int(*k), int(2)]))
        .collect();
    assert!(consolidate(&bodies, config(&["mul"])).is_empty());

    // Five uses does pay for the same definition, so the emptiness above is
    // arithmetic and not a structural refusal.
    let more: Vec<Concept> = [1, 3, 5, 7, 9]
        .iter()
        .map(|k| Concept::call("mul", [int(*k), int(2)]))
        .collect();
    let found = consolidate(&more, config(&["mul"]));
    assert_eq!(found.len(), 1, "got {found:#?}");
    assert!(found[0].utility > 0.0);
}

#[test]
fn selection_never_returns_overlapping_abstractions() {
    let body = |k: i64| {
        Concept::call(
            "outer",
            [
                Concept::call("inner", [int(k), int(2), int(3), int(4)]),
                int(5),
                int(6),
                int(7),
            ],
        )
    };
    let bodies = vec![body(1), body(8), body(9)];

    // On their own, the inner shapes are a perfectly good abstraction.
    let inners: Vec<Concept> = bodies
        .iter()
        .map(|b| b.arg(0).expect("first argument").clone())
        .collect();
    assert_eq!(consolidate(&inners, config(&["inner"])).len(), 1);

    // Inside the outer shape they lose, and they are dropped rather than
    // returned alongside it: the outer abstraction already contains them.
    let found = consolidate(&bodies, config(&["outer", "inner"]));
    assert_eq!(found.len(), 1, "got {found:#?}");
    assert_eq!(found[0].body.head(), Some(&Concept::named("outer")));
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

#[test]
fn names_are_readable_and_deterministic() {
    let bodies = vec![affine(3), affine(5), affine(7)];
    let first = consolidate(&bodies, config(&["add", "mul"]));
    let second = consolidate(&bodies, config(&["add", "mul"]));

    assert_eq!(first[0].name, "add-of-mul");
    assert_eq!(first[0].name, second[0].name);
}

#[test]
fn colliding_names_get_a_numeric_suffix() {
    let bodies = vec![affine(3), affine(5), affine(7)];
    let found = consolidate(&bodies, config(&["add", "mul", "add-of-mul"]));
    assert_eq!(found[0].name, "add-of-mul-2");

    // Camel case in the store is a word boundary, not a spelling, so the name
    // reads the way the concepts were written rather than running together.
    let camel = |k: i64| Concept::call("AddTo", [Concept::call("MulBy", [int(k), int(2)]), int(1)]);
    let camels = vec![camel(3), camel(5), camel(7)];
    let found = consolidate(&camels, config(&["AddTo", "MulBy"]));
    assert_eq!(found[0].name, "add-to-of-mul-by");
}

#[test]
fn a_corpus_with_no_names_still_gets_stable_ones() {
    let bodies = vec![affine(3), affine(5), affine(7)];
    let first = consolidate(&bodies, ConsolidateConfig::default());
    let second = consolidate(&bodies, ConsolidateConfig::default());

    assert_eq!(first.len(), 1);
    assert!(first[0].name.starts_with("shape-"), "got {}", first[0].name);
    assert_eq!(first[0].name, second[0].name);
}

// ---------------------------------------------------------------------------
// Rewriting means the same thing
// ---------------------------------------------------------------------------

#[test]
fn a_rewritten_body_evaluates_to_the_same_answer() {
    let (store, registry) = brain();
    let bodies = vec![affine(3), affine(5), affine(7)];
    let found = consolidate(&bodies, config(&["add", "mul"]));
    let abstraction = &found[0];

    put_composed(&store, &abstraction.name, abstraction.body.clone());

    let rewritten = apply_abstraction(&bodies[0], abstraction).expect("the shape is present");
    assert_eq!(rewritten, Concept::call(&abstraction.name, [int(3)]));
    assert!(rewritten.size() < bodies[0].size());

    let before = evaluate(&store, &registry, &bodies[0]);
    let after = evaluate(&store, &registry, &rewritten);
    assert_eq!(before.value(), Some(&int(7)));
    assert_eq!(
        before.value(),
        after.value(),
        "the rewrite changed behaviour"
    );
}

#[test]
fn applying_an_abstraction_that_is_absent_says_so() {
    let bodies = vec![affine(3), affine(5), affine(7)];
    let found = consolidate(&bodies, config(&["add", "mul"]));
    let elsewhere = Concept::call("sub", [int(1), int(2)]);
    assert_eq!(apply_abstraction(&elsewhere, &found[0]), None);
}

#[test]
fn a_pattern_that_matches_everything_is_never_applied() {
    // Not reachable through `consolidate`, which refuses to build one. Guarded
    // anyway because `apply_abstraction` is public and rewriting a term into a
    // call that takes that same term as its argument is a loop.
    let everything = Abstraction {
        body: Concept::hole(0),
        arity: 1,
        instances: 99,
        utility: 99.0,
        name: "anything".to_string(),
    };
    assert_eq!(apply_abstraction(&affine(3), &everything), None);
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

#[test]
fn consolidate_store_names_the_shape_and_rewrites_its_users() {
    let (store, registry) = brain();
    for (name, k) in [("job-a", 3), ("job-b", 5), ("job-c", 7)] {
        put_composed(&store, name, affine(k));
    }

    let before = evaluate(&store, &registry, &call("job-a"));
    assert_eq!(before.value(), Some(&int(7)));

    let found = consolidate_store(&store, ConsolidateConfig::default()).unwrap();
    assert_eq!(found.len(), 1, "got {found:#?}");
    assert_eq!(found[0].name, "add-of-mul");

    assert_eq!(
        stored_body(&store, "job-a"),
        Concept::call("add-of-mul", [int(3)])
    );
    assert_eq!(
        stored_body(&store, "job-c"),
        Concept::call("add-of-mul", [int(7)])
    );

    let after = evaluate(&store, &registry, &call("job-a"));
    assert_eq!(
        after.value(),
        Some(&int(7)),
        "consolidation changed the answer"
    );
}

#[test]
fn consolidate_store_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain.db");

    {
        let store = Store::open(&path).unwrap();
        let registry = spoon_natives::bootstrap();
        spoon_natives::seed_bootstrap(&store, &registry).unwrap();
        for (name, k) in [("job-a", 3), ("job-b", 5), ("job-c", 7)] {
            put_composed(&store, name, affine(k));
        }
        let found = consolidate_store(&store, ConsolidateConfig::default()).unwrap();
        assert_eq!(found.len(), 1);
    }

    let store = Store::open(&path).unwrap();
    let registry = spoon_natives::bootstrap();

    let abstraction = store
        .realization_by_name("consolidated-add-of-mul")
        .unwrap()
        .expect("the abstraction outlived the restart");
    assert_eq!(abstraction.tier, Tier::Consolidated);
    assert_eq!(abstraction.provenance, Provenance::Consolidated);
    assert_eq!(abstraction.target, Concept::named("add-of-mul"));

    assert_eq!(
        stored_body(&store, "job-b"),
        Concept::call("add-of-mul", [int(5)])
    );
    let out = evaluate(&store, &registry, &call("job-b"));
    assert_eq!(out.value(), Some(&int(11)));
}

#[test]
fn consolidating_an_empty_or_thin_library_does_nothing() {
    let (store, _) = brain();
    assert!(
        consolidate_store(&store, ConsolidateConfig::default())
            .unwrap()
            .is_empty()
    );

    put_composed(&store, "job-a", affine(3));
    assert!(
        consolidate_store(&store, ConsolidateConfig::default())
            .unwrap()
            .is_empty()
    );
}

// ---------------------------------------------------------------------------
// Eviction
// ---------------------------------------------------------------------------

#[test]
fn eviction_deprecates_rather_than_deletes() {
    let (store, registry) = brain();
    for (name, k) in [("job-a", 3), ("job-b", 5), ("job-c", 7)] {
        put_composed(&store, name, affine(k));
    }
    consolidate_store(&store, ConsolidateConfig::default()).unwrap();

    let evicted = evict_unused(&store, 1, Duration::zero()).unwrap();
    assert_eq!(evicted, 1);

    let kept = store
        .realization_by_name("consolidated-add-of-mul")
        .unwrap()
        .expect("the row is retained, because the provenance is worth keeping");
    assert_eq!(kept.tier, Tier::Deprecated);
    assert_eq!(kept.provenance, Provenance::Consolidated);

    // Deprecated is never selected, so the call no longer reduces. The body
    // that used it is left as it is: nothing is silently un-rewritten.
    let out = evaluate(&store, &registry, &call("job-a"));
    assert_eq!(out.value(), Some(&Concept::call("add-of-mul", [int(3)])));
}

#[test]
fn eviction_spares_an_abstraction_that_is_being_used() {
    let (store, _) = brain();
    for (name, k) in [("job-a", 3), ("job-b", 5), ("job-c", 7)] {
        put_composed(&store, name, affine(k));
    }
    consolidate_store(&store, ConsolidateConfig::default()).unwrap();
    store
        .record_realization_use("consolidated-add-of-mul", true, Utc::now())
        .unwrap();

    assert_eq!(evict_unused(&store, 1, Duration::zero()).unwrap(), 0);

    // Nor does it touch anything that was not consolidated in the first place.
    assert_eq!(
        store
            .realization_by_name("body-job-a")
            .unwrap()
            .unwrap()
            .tier,
        Tier::Provisional
    );
}

#[test]
fn eviction_waits_out_the_minimum_age() {
    let (store, _) = brain();
    for (name, k) in [("job-a", 3), ("job-b", 5), ("job-c", 7)] {
        put_composed(&store, name, affine(k));
    }
    consolidate_store(&store, ConsolidateConfig::default()).unwrap();

    assert_eq!(evict_unused(&store, 1, Duration::days(7)).unwrap(), 0);
    assert_eq!(
        store
            .realization_by_name("consolidated-add-of-mul")
            .unwrap()
            .unwrap()
            .tier,
        Tier::Consolidated
    );
}

// ---------------------------------------------------------------------------
// Determinism and pathological input
// ---------------------------------------------------------------------------

#[test]
fn the_same_corpus_yields_the_same_library_every_run() {
    let bodies = vec![
        affine(3),
        affine(5),
        affine(7),
        Concept::call(
            "wrap",
            [affine(11), Concept::call("pair", [int(1), int(1)])],
        ),
        Concept::call(
            "wrap",
            [affine(13), Concept::call("pair", [int(2), int(2)])],
        ),
        Concept::call(
            "wrap",
            [affine(17), Concept::call("pair", [int(3), int(3)])],
        ),
    ];
    let names = ["add", "mul", "wrap", "pair"];

    let first = consolidate(&bodies, config(&names));
    assert!(!first.is_empty());
    for _ in 0..5 {
        assert_eq!(consolidate(&bodies, config(&names)), first);
    }
}

#[test]
fn a_pathologically_deep_body_is_left_alone_rather_than_crashing() {
    let mut deep = Concept::named("seed");
    for _ in 0..1_000 {
        deep = Concept::call("nest", [deep]);
    }
    let bodies = vec![
        deep.clone(),
        deep.clone(),
        deep,
        affine(3),
        affine(5),
        affine(7),
    ];

    // The deep body contributes nothing, the shallow ones still consolidate.
    let found = consolidate(&bodies, config(&["nest", "seed", "add", "mul"]));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "add-of-mul");
}

#[test]
fn a_pathologically_wide_body_is_left_alone_rather_than_crashing() {
    let wide = Concept::call("spread", (0..10_000).map(int));
    let bodies = vec![wide.clone(), wide.clone(), wide];
    assert!(consolidate(&bodies, config(&["spread"])).is_empty());
}

#[test]
fn an_empty_corpus_is_not_a_special_case() {
    assert!(consolidate(&[], ConsolidateConfig::default()).is_empty());
    assert!(consolidate(&[Concept::named("greg")], ConsolidateConfig::default()).is_empty());
    assert!(consolidate(&vec![Concept::hole(0); 3], ConsolidateConfig::default()).is_empty());
}
