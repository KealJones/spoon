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
            .realizations_for(&Concept::named("count"))
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
            body: Concept::call("join", [Concept::hole(0), Concept::text("")]),
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
