//! The core design decision, encoded as tests.
//!
//! A ground concept's identity is its value, so it is self-describing and
//! needs no row. That is what keeps every integer in every computation out of
//! the database. The moment something is asserted about one, it materializes
//! and becomes queryable exactly like a named concept.

use chrono::Utc;
use spoon_concept::{Concept, ConceptMeta, Provenance, Tier};
use spoon_store::Store;

#[test]
fn ground_concepts_cost_no_row_until_something_is_asserted_about_them() {
    let store = Store::open_in_memory().unwrap();

    // Evaluating Add<42, 1> is pure structure over self-describing values.
    // Nothing about it needs the store, so nothing is written.
    let expression = Concept::call("math-add", [Concept::int(42), Concept::int(1)]);
    assert_eq!(store.count_concepts().unwrap(), 0);

    // Even handing the ground values to the store directly writes no rows:
    // there is nothing to record that the value does not already say.
    store.put_concept(&Concept::int(42)).unwrap();
    store.put_concept(&Concept::int(1)).unwrap();
    store.put_concept(&Concept::text("hello")).unwrap();
    assert_eq!(store.count_concepts().unwrap(), 0);
    assert!(!store.has_concept(Concept::int(42).content_id()).unwrap());

    // Now assert something about 42. It earns exactly one row.
    let meta = ConceptMeta::new(
        Concept::int(42),
        Provenance::User { episode: None },
        Tier::Provisional,
        Utc::now(),
    )
    .with_surface_forms(["forty-two"]);
    store.put_meta(&meta).unwrap();

    assert_eq!(store.count_concepts().unwrap(), 1);
    assert!(store.has_concept(Concept::int(42).content_id()).unwrap());
    assert_eq!(
        store
            .get_concept(Concept::int(42).content_id())
            .unwrap()
            .unwrap(),
        Concept::int(42)
    );
    // And it is reachable by what it is called, case-insensitively.
    assert_eq!(
        store.surface_lookup("Forty-Two").unwrap(),
        vec![Concept::int(42)]
    );
    // 1 said nothing about itself, so it still has no row.
    assert!(!store.has_concept(Concept::int(1).content_id()).unwrap());

    let _ = expression;
}

#[test]
fn asserting_a_relationship_materializes_the_ground_value_it_mentions() {
    let store = Store::open_in_memory().unwrap();
    let synonym = Concept::call("synonym", [Concept::text("forty-two"), Concept::int(42)]);
    store
        .assert_concept(&synonym, Provenance::User { episode: None }, None, None)
        .unwrap();

    // synonym compound, the synonym head, and nothing for the two ground
    // arguments: they are participants, not rows.
    assert_eq!(store.count_concepts().unwrap(), 2);
    assert!(!store.has_concept(Concept::int(42).content_id()).unwrap());

    // But they are still findable through the compound that mentions them.
    let about_42 = store
        .concepts_containing(Concept::int(42).content_id(), 10)
        .unwrap();
    assert_eq!(about_42, vec![synonym.clone()]);

    // Asserting about 42 itself is what gives it a row.
    store
        .assert_concept(
            &Concept::int(42),
            Provenance::User { episode: None },
            None,
            None,
        )
        .unwrap();
    assert!(store.has_concept(Concept::int(42).content_id()).unwrap());
    assert_eq!(store.count_concepts().unwrap(), 3);
}

#[test]
fn storing_a_computation_writes_rows_for_names_but_not_for_values() {
    let store = Store::open_in_memory().unwrap();
    store
        .put_concept(&Concept::call("math-add", [Concept::int(42), Concept::int(1)]))
        .unwrap();
    // The compound and the head `add`. Not 42, not 1.
    assert_eq!(store.count_concepts().unwrap(), 2);
}
