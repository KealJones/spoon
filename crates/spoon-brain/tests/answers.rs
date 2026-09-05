//! Telling an answer apart from a term that stalled partway.

use chrono::Utc;
use spoon_brain::is_answer;
use spoon_concept::{
    Activation, Concept, Effect, NativeId, Provenance, Realization, RealizationSpec, Tier,
};
use spoon_store::Store;

fn store() -> Store {
    Store::open_in_memory().expect("store")
}

fn realize(store: &Store, name: &str) {
    store
        .put_realization(&Realization {
            target: Concept::named(name),
            name: format!("native-{name}").into(),
            spec: RealizationSpec::Native {
                native: NativeId::new(name),
            },
            effect: Effect::Pure,
            activation: Activation::new(Utc::now()),
            provenance: Provenance::Bootstrap,
            tier: Tier::Kernel,
        })
        .expect("realization");
}

#[test]
fn a_ground_value_is_an_answer() {
    assert!(is_answer(&store(), &Concept::int(3)));
}

#[test]
fn a_stalled_capability_is_not_an_answer() {
    // The strawberry bug. `chars` reduced, so the term changed, but the head
    // nothing realizes is still sitting there holding the result hostage.
    let store = store();
    realize(&store, "text-chars");
    let stalled = Concept::call(
        "count-matching",
        [
            Concept::call("list-of", [Concept::text("f"), Concept::text("o")]),
            Concept::text("r"),
        ],
    );
    assert!(!is_answer(&store, &stalled));
}

#[test]
fn an_asserted_fact_is_an_answer() {
    // Nothing realizes `friend-with` and nothing ever will. It is data, and
    // data is a perfectly good answer.
    let store = store();
    let fact = Concept::call("friend-with", [Concept::named("greg"), Concept::named("keal")]);
    store
        .assert_concept(&fact, Provenance::User { episode: None }, None, None)
        .expect("store-assert");
    assert!(is_answer(&store, &fact));
}

#[test]
fn an_unasserted_made_up_head_is_not_an_answer() {
    let store = store();
    let made_up = Concept::call("friend-with", [Concept::named("greg"), Concept::named("keal")]);
    assert!(!is_answer(&store, &made_up));
}

#[test]
fn a_realized_head_is_an_answer_even_unasserted() {
    let store = store();
    realize(&store, "list-of");
    assert!(is_answer(
        &store,
        &Concept::call("list-of", [Concept::int(1), Concept::int(2)])
    ));
}

#[test]
fn a_stall_nested_inside_a_realized_head_is_caught() {
    let store = store();
    realize(&store, "list-of");
    let nested = Concept::call("list-of", [Concept::call("mystery", [Concept::int(1)])]);
    assert!(!is_answer(&store, &nested));
}
