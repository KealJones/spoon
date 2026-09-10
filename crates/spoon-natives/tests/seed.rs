//! Seeding: what a fresh brain gets, and what a stale one loses.

use chrono::Utc;
use spoon_concept::{
    Activation, Concept, Effect, NativeId, Provenance, Realization, RealizationSpec, Tier,
};
use spoon_natives::seed_bootstrap;
use spoon_store::Store;

#[test]
fn a_native_that_no_longer_exists_is_retired() {
    // The count-matching bug. An older build seeded `native-count-matching`;
    // the Rust function was renamed away, and the realization stayed behind
    // in every existing brain, so the concept still looked realized and every
    // call stalled without ever reporting a gap.
    let store = Store::open_in_memory().expect("store");
    let registry = spoon_natives::bootstrap();

    store
        .put_realization(&Realization {
            target: Concept::named("count-matching"),
            name: "native-count-matching".into(),
            spec: RealizationSpec::Native {
                native: NativeId::new("count-matching"),
            },
            effect: Effect::Pure,
            activation: Activation::new(Utc::now()),
            provenance: Provenance::Bootstrap,
            tier: Tier::Kernel,
        })
        .expect("stale realization");

    let stats = seed_bootstrap(&store, &registry).expect("seed");

    assert_eq!(stats.retired, 1);
    assert!(
        store
            .realizations_for(&Concept::named("count-matching"))
            .expect("lookup")
            .is_empty()
    );
    // Live natives are untouched.
    assert!(
        !store
            .realizations_for(&Concept::named("list-count"))
            .expect("lookup")
            .is_empty()
    );
}

#[test]
fn a_learned_body_survives_seeding() {
    // Only bootstrap natives are ours to retire.
    let store = Store::open_in_memory().expect("store");
    let registry = spoon_natives::bootstrap();
    let learned = Realization {
        target: Concept::named("reverse-text"),
        name: "composed-reverse-text".into(),
        spec: RealizationSpec::Composed {
            body: Concept::call("text-join", [Concept::hole(0), Concept::text("")]),
        },
        effect: Effect::Pure,
        activation: Activation::new(Utc::now()),
        provenance: Provenance::Synthesized { episode: None },
        tier: Tier::Provisional,
    };
    store.put_realization(&learned).expect("learned");

    seed_bootstrap(&store, &registry).expect("seed");

    assert!(
        !store
            .realizations_for(&Concept::named("reverse-text"))
            .expect("lookup")
            .is_empty()
    );
}

#[test]
fn a_phrasing_pointing_at_a_retired_head_is_forgotten() {
    // Retiring the realization alone was not enough. The learned pair that
    // read "how many rs in X" as count-matching outlived it, so the brain
    // that had used the capability most was the one that could no longer
    // answer, while a fresh brain got it right for want of a bad memory.
    let store = Store::open_in_memory().expect("store");
    let registry = spoon_natives::bootstrap();
    let dead = Concept::call("count-matching", [Concept::hole(0), Concept::hole(1)]);

    store
        .put_realization(&Realization {
            target: Concept::named("count-matching"),
            name: "native-count-matching".into(),
            spec: RealizationSpec::Native {
                native: NativeId::new("count-matching"),
            },
            effect: Effect::Pure,
            activation: Activation::new(Utc::now()),
            provenance: Provenance::Bootstrap,
            tier: Tier::Kernel,
        })
        .expect("stale realization");
    store
        .put_pair(
            "how many rs are in strawberry",
            &[dead],
            spoon_store::pairs::PairSource::Confirmed,
        )
        .expect("stale pair");
    let keep = store
        .put_pair(
            "how many things are in this",
            &[Concept::call("list-count", [Concept::hole(0)])],
            spoon_store::pairs::PairSource::Confirmed,
        )
        .expect("live pair");

    let stats = seed_bootstrap(&store, &registry).expect("seed");

    assert_eq!(stats.retired, 1);
    assert_eq!(stats.forgotten, 1);
    let left = store.all_pairs(100).expect("pairs");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, keep);
}

#[test]
fn restart_seeding_preserves_the_evidence_from_user_feedback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain.db");
    let registry = spoon_natives::bootstrap();
    let before = {
        let store = Store::open(&path).unwrap();
        seed_bootstrap(&store, &registry).unwrap();
        store
            .record_realization_use("native-math-add", true, Utc::now())
            .unwrap();
        store
            .record_realization_use("native-math-add", false, Utc::now())
            .unwrap();
        store
            .realization_by_name("native-math-add")
            .unwrap()
            .unwrap()
            .activation
    };
    let store = Store::open(&path).unwrap();
    seed_bootstrap(&store, &registry).unwrap();
    let after = store
        .realization_by_name("native-math-add")
        .unwrap()
        .unwrap()
        .activation;
    assert_eq!(
        after, before,
        "startup must not erase what the user taught us"
    );
}
