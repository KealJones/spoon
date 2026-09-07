//! Name reconciliation: keeping one idea as one concept.

use std::sync::Arc;

use spoon_brain::{reconcile, remember};
use spoon_concept::{Concept, ConceptMeta, Provenance, SymbolTable, Tier};
use spoon_store::Store;

fn brain_with(names: &[&str]) -> (Store, Arc<SymbolTable>) {
    let store = Store::open_in_memory().unwrap();
    let table = Arc::new(SymbolTable::new());
    let now = chrono::Utc::now();
    for name in names {
        table.intern(name);
        store.register_symbol(name).unwrap();
        store
            .put_meta(&ConceptMeta::new(
                Concept::named(name),
                Provenance::User { episode: None },
                Tier::Provisional,
                now,
            ))
            .unwrap();
    }
    (store, table)
}

#[test]
fn a_near_miss_resolves_to_the_concept_already_known() {
    // The case that made symmetry silently fail: the model wrote `friends` one
    // turn and `friendship` the next, and Symmetric<Friendship> says nothing
    // about Friends.
    let (store, table) = brain_with(&["friends", "greg", "keal"]);
    table.intern("friendship");
    let steps = vec![Concept::call("symmetric", [Concept::named("friendship")])];

    let out = reconcile(&steps, &store, &table);
    assert_eq!(
        out.steps[0],
        Concept::call("symmetric", [Concept::named("friends")]),
        "friendship should have resolved to friends, got {:?}",
        out.rewrites
    );
    assert_eq!(out.rewrites.len(), 1);
}

#[test]
fn resolving_records_a_synonym_so_the_next_time_is_exact() {
    let (store, table) = brain_with(&["friends"]);
    table.intern("friendship");
    let steps = vec![Concept::call("symmetric", [Concept::named("friendship")])];
    let out = reconcile(&steps, &store, &table);
    assert_eq!(out.synonyms.len(), 1);
    remember(&out, &store);
    assert!(store.holds(&out.synonyms[0]).unwrap());
}

#[test]
fn an_explicit_synonym_beats_spelling_similarity() {
    // Somebody said so, and nobody guessed.
    let (store, table) = brain_with(&["dog", "canine"]);
    table.intern("pup");
    store
        .assert_concept(
            &Concept::call("synonym", [Concept::text("pup"), Concept::named("dog")]),
            Provenance::User { episode: None },
            None,
            None,
        )
        .unwrap();
    let steps = vec![Concept::call(
        "owns",
        [Concept::named("john"), Concept::named("pup")],
    )];
    let out = reconcile(&steps, &store, &table);
    assert_eq!(out.steps[0].arg(1), Some(&Concept::named("dog")));
}

#[test]
fn a_known_concept_is_left_alone() {
    let (store, table) = brain_with(&["friends", "friendly"]);
    let steps = vec![Concept::call("symmetric", [Concept::named("friends")])];
    let out = reconcile(&steps, &store, &table);
    assert_eq!(out.steps, steps);
    assert!(
        out.rewrites.is_empty(),
        "an exact match must not be rewritten"
    );
}

#[test]
fn unrelated_names_are_not_merged() {
    // A wrong merge silently answers questions about one thing using facts
    // about another, which is worse than leaving a duplicate visible.
    let (store, table) = brain_with(&["friends", "elephant", "database"]);
    for stranger in ["bicycle", "quantum", "sandwich"] {
        table.intern(stranger);
        let steps = vec![Concept::call("about", [Concept::named(stranger)])];
        let out = reconcile(&steps, &store, &table);
        assert!(
            out.rewrites.is_empty(),
            "{stranger} was merged into {:?}",
            out.rewrites
        );
    }
}

#[test]
fn short_names_are_never_fuzzily_matched() {
    // In a short word every edit is a large fraction of it, so similarity stops
    // being informative: "math-add" and "ask" are one edit apart and unrelated.
    let (store, table) = brain_with(&["math-add", "ask", "logic-and"]);
    table.intern("aid");
    let steps = vec![Concept::call("do", [Concept::named("aid")])];
    let out = reconcile(&steps, &store, &table);
    assert!(out.rewrites.is_empty());
}

#[test]
fn an_ambiguous_near_miss_is_left_alone() {
    // Two candidates fitting equally well means the evidence singles out
    // neither, and guessing would be a coin flip that looks like knowledge.
    let (store, table) = brain_with(&["running", "runners"]);
    table.intern("runnings");
    let steps = vec![Concept::call("about", [Concept::named("runnings")])];
    let out = reconcile(&steps, &store, &table);
    assert!(
        out.rewrites.len() <= 1,
        "should resolve to at most one, got {:?}",
        out.rewrites
    );
}

#[test]
fn nested_structure_is_rewritten_throughout() {
    let (store, table) = brain_with(&["friends"]);
    table.intern("friendship");
    let steps = vec![Concept::call(
        "stated",
        [
            Concept::named("keal"),
            Concept::call("friendship", [Concept::named("a")]),
        ],
    )];
    let out = reconcile(&steps, &store, &table);
    let inner = out.steps[0].arg(1).unwrap();
    assert_eq!(
        inner.head_symbol(),
        Some(spoon_concept::SymbolId::of("friends"))
    );
}

#[test]
fn a_bare_word_given_to_a_native_becomes_text() {
    // The ears write "reverse kubernetes" as reverse<kubernetes> about as
    // often as reverse<"kubernetes">, and the first one failed: a native given
    // a named concept where it wanted text. It was the largest single group of
    // wrong answers in the corpus.
    let store = Store::open_in_memory().expect("store");
    let registry = spoon_natives::bootstrap();
    spoon_natives::seed_bootstrap(&store, &registry).expect("seed");
    let symbols = SymbolTable::new();
    let word = symbols.intern("kubernetes");

    let step = Concept::call("list-reverse", [Concept::symbol(word)]);
    let out = reconcile(std::slice::from_ref(&step), &store, &symbols);

    assert_eq!(
        out.steps[0],
        Concept::call("list-reverse", [Concept::text("kubernetes")])
    );
}

#[test]
fn an_entity_in_a_fact_is_left_alone() {
    // Nothing realizes `owns`, so it is a claim rather than a computation, and
    // john and dog are entities. Turning them into strings would be exactly
    // wrong: every later question about john would miss.
    let store = Store::open_in_memory().expect("store");
    let registry = spoon_natives::bootstrap();
    spoon_natives::seed_bootstrap(&store, &registry).expect("seed");
    let symbols = SymbolTable::new();
    let john = symbols.intern("john");
    let dog = symbols.intern("dog");

    let step = Concept::call("owns", [Concept::symbol(john), Concept::symbol(dog)]);
    let out = reconcile(std::slice::from_ref(&step), &store, &symbols);

    assert_eq!(out.steps[0], step);
}

#[test]
fn a_name_the_store_knows_survives_a_native() {
    // Somebody established it as a concept. Passing it on unchanged is how
    // that stays true.
    let store = Store::open_in_memory().expect("store");
    let registry = spoon_natives::bootstrap();
    spoon_natives::seed_bootstrap(&store, &registry).expect("seed");
    let symbols = SymbolTable::new();
    let greg = symbols.intern("greg");
    store.register_symbol("greg").expect("register");

    let step = Concept::call("list-reverse", [Concept::symbol(greg)]);
    let out = reconcile(std::slice::from_ref(&step), &store, &symbols);

    assert_eq!(out.steps[0], step);
}
