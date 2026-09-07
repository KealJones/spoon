//! Structure: what gets a row, and how it is found again.

use spoon_concept::{Concept, SymbolId};
use spoon_store::{SCHEMA_VERSION, Store, StoreError};

fn friendship(a: &str, b: &str) -> Concept {
    Concept::call("friend-with", [Concept::named(a), Concept::named(b)])
}

#[test]
fn opening_in_memory_migrates_to_the_current_schema() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(store.count_concepts().unwrap(), 0);
}

#[test]
fn a_compound_round_trips_through_its_content_id() {
    let store = Store::open_in_memory().unwrap();
    let greg_and_keal = friendship("greg", "keal");
    let id = store.put_concept(&greg_and_keal).unwrap();

    assert_eq!(id, greg_and_keal.content_id());
    assert!(store.has_concept(id).unwrap());
    let back = store.get_concept(id).unwrap().unwrap();
    assert_eq!(back, greg_and_keal);
    assert_eq!(back.content_id(), id);
}

#[test]
fn concepts_by_head_finds_every_friendship() {
    let store = Store::open_in_memory().unwrap();
    store.put_concept(&friendship("greg", "keal")).unwrap();
    store.put_concept(&friendship("keal", "ada")).unwrap();
    store
        .put_concept(&Concept::call(
            "works-at",
            [Concept::named("greg"), Concept::named("workiva")],
        ))
        .unwrap();

    let found = store
        .concepts_by_head(SymbolId::of("friend-with"), 100)
        .unwrap();
    assert_eq!(found.len(), 2);
    assert!(found.contains(&friendship("greg", "keal")));
    assert!(found.contains(&friendship("keal", "ada")));

    // The limit is honoured, not advisory.
    assert_eq!(
        store
            .concepts_by_head(SymbolId::of("friend-with"), 1)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn concepts_containing_finds_everything_about_greg() {
    let store = Store::open_in_memory().unwrap();
    let greg = Concept::named("greg");
    let employment = Concept::call("works-at", [greg.clone(), Concept::named("workiva")]);
    store.put_concept(&friendship("greg", "keal")).unwrap();
    store.put_concept(&employment).unwrap();
    store.put_concept(&friendship("keal", "ada")).unwrap();

    let about_greg = store.concepts_containing(greg.content_id(), 100).unwrap();
    assert_eq!(about_greg.len(), 2);
    assert!(about_greg.contains(&friendship("greg", "keal")));
    assert!(about_greg.contains(&employment));
    assert!(!about_greg.contains(&friendship("keal", "ada")));
}

#[test]
fn a_nested_compound_round_trips_and_its_inner_participants_are_findable() {
    let store = Store::open_in_memory().unwrap();
    let greg = Concept::named("greg");
    let sadness = Concept::call("is-sad", [greg.clone()]);
    let stated = Concept::call("stated", [Concept::named("keal"), sadness.clone()]);

    let id = store.put_concept(&stated).unwrap();
    assert_eq!(store.get_concept(id).unwrap().unwrap(), stated);

    // Greg is two levels down, and still findable through the outer concept.
    let about_greg = store.concepts_containing(greg.content_id(), 100).unwrap();
    assert!(about_greg.contains(&stated));
    // The inner compound is a citizen in its own right, not a detail of the
    // outer one.
    assert!(about_greg.contains(&sadness));
    assert!(store.has_concept(sadness.content_id()).unwrap());
}

#[test]
fn storing_the_same_concept_twice_leaves_one_row() {
    let store = Store::open_in_memory().unwrap();
    let c = friendship("greg", "keal");
    let first = store.put_concept(&c).unwrap();
    let before = store.count_concepts().unwrap();
    let second = store.put_concept(&c).unwrap();

    assert_eq!(first, second);
    assert_eq!(store.count_concepts().unwrap(), before);
    // compound, friend-with, greg, keal
    assert_eq!(before, 4);
}

#[test]
fn a_bare_hole_is_not_storable() {
    let store = Store::open_in_memory().unwrap();
    let err = store.put_concept(&Concept::hole(0)).unwrap_err();
    assert!(matches!(err, StoreError::HoleNotStorable));
    assert_eq!(store.count_concepts().unwrap(), 0);

    // A pattern that contains holes is fine: the hole is not standing alone.
    let pattern = Concept::call("friend-with", [Concept::hole(0), Concept::hole(1)]);
    store.put_concept(&pattern).unwrap();
    assert!(store.has_concept(pattern.content_id()).unwrap());
    // Holes are not indexed as participants, since hole numbering is local to
    // the pattern that contains it.
    assert!(
        store
            .concepts_containing(Concept::hole(0).content_id(), 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn non_finite_floats_store_and_read_back() {
    // Ground::Float encodes NaN and the infinities as strings precisely so a
    // concept carrying one is not writable-but-unreadable. Nothing in the
    // store needs to special-case them.
    let store = Store::open_in_memory().unwrap();
    for f in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let c = Concept::call("measurement", [Concept::float(f)]);
        let id = store.put_concept(&c).unwrap();
        let back = store
            .get_concept(id)
            .unwrap()
            .expect("stored concept missing");
        assert_eq!(
            back.content_id(),
            c.content_id(),
            "{f} did not survive the store"
        );
    }
}
