//! Export, import, and the byte-identity that makes seeds diffable.

use chrono::Utc;
use spoon_concept::{
    Activation, Concept, ConceptMeta, Effect, Ground, NativeId, Provenance, Realization,
    RealizationSpec, RuleDirection, Tier,
};
use spoon_store::Store;

/// A store with one of everything, including a materialized ground concept and
/// a json ground value, so the round trip exercises the awkward corners.
fn populated() -> Store {
    let store = Store::open_in_memory().unwrap();
    let now = Utc::now();

    store.register_symbol("friend-with").unwrap();
    store.register_symbol("Greg").unwrap();
    store.register_symbol("math-add").unwrap();

    let friendship = Concept::call(
        "friend-with",
        [Concept::named("greg"), Concept::named("keal")],
    );
    let nested = Concept::call(
        "stated",
        [
            Concept::named("keal"),
            Concept::call("is-sad", [Concept::named("greg")]),
        ],
    );
    let payload = Concept::call(
        "pull-request",
        [
            Concept::int(160),
            Concept::ground(Ground::json(serde_json::json!({
                "title": "unify the representation",
                "merged": true,
            }))),
        ],
    );
    store.put_concept(&friendship).unwrap();
    store.put_concept(&nested).unwrap();
    store.put_concept(&payload).unwrap();

    store
        .put_meta(
            &ConceptMeta::new(
                Concept::int(42),
                Provenance::User { episode: Some(1) },
                Tier::Consolidated,
                now,
            )
            .with_surface_forms(["forty-two", "the answer"])
            .with_note("a ground value that earned a row"),
        )
        .unwrap();
    store
        .put_meta(
            &ConceptMeta::new(
                Concept::named("greg"),
                Provenance::Bootstrap,
                Tier::Kernel,
                now,
            )
            .with_surface_forms(["Greg"]),
        )
        .unwrap();

    store
        .put_realization(&Realization {
            target: Concept::named("math-add"),
            name: "add/native".into(),
            spec: RealizationSpec::Native {
                native: NativeId::new("arith.add"),
            },
            effect: Effect::Pure,
            activation: Activation::new(now),
            provenance: Provenance::Bootstrap,
            tier: Tier::Kernel,
        })
        .unwrap();
    store
        .put_realization(&Realization {
            target: Concept::named("symmetric"),
            name: "symmetric/rule".into(),
            spec: RealizationSpec::Rule {
                pattern: Concept::apply(Concept::hole(0), vec![Concept::hole(1), Concept::hole(2)]),
                condition: Some(Concept::call("symmetric", [Concept::hole(0)])),
                produce: Concept::apply(Concept::hole(0), vec![Concept::hole(2), Concept::hole(1)]),
                direction: RuleDirection::Forward,
            },
            effect: Effect::Pure,
            activation: Activation::new(now),
            provenance: Provenance::Bootstrap,
            tier: Tier::Kernel,
        })
        .unwrap();

    store
        .assert_concept(
            &friendship,
            Provenance::User { episode: Some(1) },
            Some(1),
            Some(0.9),
        )
        .unwrap();
    let retractable = store
        .assert_concept(
            &nested,
            Provenance::Teacher { episode: Some(2) },
            None,
            None,
        )
        .unwrap();
    store.retract(retractable, Utc::now()).unwrap();
    store
        .assert_concept_during(
            &payload,
            Provenance::External {
                source: "github".into(),
            },
            None,
            Some(0.5),
            Some(now),
            None,
        )
        .unwrap();

    store
}

#[test]
fn exporting_the_same_brain_twice_produces_the_same_bytes() {
    let store = populated();
    let first = serde_json::to_string_pretty(&store.export_seed("core").unwrap()).unwrap();
    let second = serde_json::to_string_pretty(&store.export_seed("core").unwrap()).unwrap();
    assert_eq!(first, second);
    assert!(first.contains("\"name\": \"core\""));
}

#[test]
fn export_import_export_is_byte_identical() {
    let source = populated();
    let seed = source.export_seed("core").unwrap();
    let first = serde_json::to_string_pretty(&seed).unwrap();

    // Through JSON and back, the way a seed file is actually used.
    let parsed: spoon_store::Seed = serde_json::from_str(&first).unwrap();
    assert_eq!(parsed, seed);

    let fresh = Store::open_in_memory().unwrap();
    let stats = fresh.import_seed(&parsed).unwrap();
    assert_eq!(stats.concepts, seed.concepts.len());
    assert_eq!(stats.meta, seed.meta.len());
    assert_eq!(stats.realizations, seed.realizations.len());
    assert_eq!(stats.assertions, seed.assertions.len());
    assert_eq!(stats.assertions_skipped, 0);

    let second = serde_json::to_string_pretty(&fresh.export_seed("core").unwrap()).unwrap();
    assert_eq!(first, second);

    // The imported brain behaves like the one it came from, not merely
    // serializes like it.
    assert_eq!(
        fresh.count_concepts().unwrap(),
        source.count_concepts().unwrap()
    );
    assert_eq!(
        fresh.surface_lookup("The Answer").unwrap(),
        source.surface_lookup("The Answer").unwrap()
    );
    let friendship = Concept::call(
        "friend-with",
        [Concept::named("greg"), Concept::named("keal")],
    );
    assert!(fresh.holds(&friendship).unwrap());
    assert_eq!(
        fresh.load_symbol_table().unwrap().len(),
        source.load_symbol_table().unwrap().len()
    );
}

#[test]
fn importing_the_same_seed_twice_changes_nothing() {
    let seed = populated().export_seed("core").unwrap();
    let fresh = Store::open_in_memory().unwrap();

    fresh.import_seed(&seed).unwrap();
    let after_first = serde_json::to_string(&fresh.export_seed("core").unwrap()).unwrap();

    let stats = fresh.import_seed(&seed).unwrap();
    assert_eq!(stats.assertions, 0);
    assert_eq!(stats.assertions_skipped, seed.assertions.len());

    let after_second = serde_json::to_string(&fresh.export_seed("core").unwrap()).unwrap();
    assert_eq!(after_first, after_second);
}

#[test]
fn a_seed_carries_the_ground_rows_that_were_materialized() {
    let seed = populated().export_seed("core").unwrap();
    // 42 has a row because something was said about it, so it travels with
    // the seed. The 160 inside the pull request compound does not.
    assert!(seed.concepts.contains(&Concept::int(42)));
    assert!(!seed.concepts.contains(&Concept::int(160)));

    let fresh = Store::open_in_memory().unwrap();
    fresh.import_seed(&seed).unwrap();
    assert!(fresh.has_concept(Concept::int(42).content_id()).unwrap());
    assert!(!fresh.has_concept(Concept::int(160).content_id()).unwrap());
    // But 160 is still findable through the compound that mentions it.
    assert_eq!(
        fresh
            .concepts_containing(Concept::int(160).content_id(), 10)
            .unwrap()
            .len(),
        1
    );
}
