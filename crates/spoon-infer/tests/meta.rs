//! The meta-vocabulary end to end.
//!
//! The point of every test here: the user asserts a plain fact about a
//! relation, and the relation starts behaving accordingly. No relation-specific
//! rule is written anywhere.

use spoon_concept::{Concept, Provenance};
use spoon_infer::{DeriveBudget, DiscriminationTree, Engine, seed_meta_rules};
use spoon_store::Store;

fn h(n: u32) -> Concept {
    Concept::hole(n)
}

fn brain() -> Store {
    let store = Store::open_in_memory().unwrap();
    seed_meta_rules(&store).unwrap();
    store
}

fn fact(store: &Store, c: &Concept) {
    store
        .assert_concept(c, Provenance::User { episode: None }, None, None)
        .unwrap();
}

fn holds(store: &Store, goal: &Concept) -> bool {
    let index = DiscriminationTree::from_store(store).unwrap();
    Engine::new(store, &index)
        .with_budget(DeriveBudget::default().with_depth(12).with_steps(20_000))
        .holds(goal)
        .unwrap()
}

fn rel(name: &str, a: &str, b: &str) -> Concept {
    Concept::call(name, [Concept::named(a), Concept::named(b)])
}

// ---------------------------------------------------------------------------
// Symmetric
// ---------------------------------------------------------------------------

#[test]
fn asserting_a_relation_is_symmetric_makes_it_symmetric() {
    let store = brain();
    fact(&store, &rel("friend-with", "greg", "keal"));
    fact(
        &store,
        &Concept::call("symmetric", [Concept::named("friend-with")]),
    );

    assert!(holds(&store, &rel("friend-with", "keal", "greg")));
}

#[test]
fn symmetry_applies_to_every_relation_declared_symmetric() {
    // One general rule, not one per relation. That is the whole point of
    // letting a hole sit in head position.
    let store = brain();
    for (relation, a, b) in [("friend-with", "greg", "keal"), ("married-to", "x", "y")] {
        fact(&store, &rel(relation, a, b));
        fact(
            &store,
            &Concept::call("symmetric", [Concept::named(relation)]),
        );
    }
    assert!(holds(&store, &rel("friend-with", "keal", "greg")));
    assert!(holds(&store, &rel("married-to", "y", "x")));
}

#[test]
fn symmetry_does_not_leak_to_relations_that_did_not_ask_for_it() {
    // Owing money is famously not symmetric. If declaring one relation
    // symmetric made others symmetric, the vocabulary would be worthless.
    let store = brain();
    fact(&store, &rel("friend-with", "greg", "keal"));
    fact(
        &store,
        &Concept::call("symmetric", [Concept::named("friend-with")]),
    );
    fact(&store, &rel("owes", "greg", "keal"));

    assert!(holds(&store, &rel("friend-with", "keal", "greg")));
    assert!(
        !holds(&store, &rel("owes", "keal", "greg")),
        "symmetry leaked"
    );
}

// ---------------------------------------------------------------------------
// InverseOf
// ---------------------------------------------------------------------------

#[test]
fn an_inverse_pair_works_in_both_directions_from_one_assertion() {
    // Asserting InverseOf<ParentOf, ChildOf> once has to be enough. Making the
    // user assert it twice would be a papercut that never stops.
    let store = brain();
    fact(&store, &rel("parent-of", "alice", "bob"));
    fact(
        &store,
        &Concept::call(
            "inverse-of",
            [Concept::named("parent-of"), Concept::named("child-of")],
        ),
    );

    assert!(
        holds(&store, &rel("child-of", "bob", "alice")),
        "forward direction failed"
    );

    fact(&store, &rel("child-of", "carol", "dave"));
    assert!(
        holds(&store, &rel("parent-of", "dave", "carol")),
        "reverse direction failed"
    );
}

#[test]
fn an_inverse_does_not_make_a_relation_symmetric() {
    let store = brain();
    fact(&store, &rel("parent-of", "alice", "bob"));
    fact(
        &store,
        &Concept::call(
            "inverse-of",
            [Concept::named("parent-of"), Concept::named("child-of")],
        ),
    );
    assert!(
        !holds(&store, &rel("parent-of", "bob", "alice")),
        "inverse was treated as symmetry"
    );
}

// ---------------------------------------------------------------------------
// Transitive
// ---------------------------------------------------------------------------

#[test]
fn transitivity_closes_over_a_chain() {
    // Two premises joined on a shared middle term, which is the case a
    // single-antecedent rule cannot express. The conjunction spelling carries
    // the join.
    let store = brain();
    fact(&store, &rel("part-of", "a", "b"));
    fact(&store, &rel("part-of", "b", "c"));
    fact(&store, &rel("part-of", "c", "d"));
    fact(
        &store,
        &Concept::call("transitive", [Concept::named("part-of")]),
    );

    assert!(holds(&store, &rel("part-of", "a", "c")), "one hop failed");
    assert!(holds(&store, &rel("part-of", "a", "d")), "two hops failed");
}

#[test]
fn transitivity_does_not_invent_links_that_are_not_there() {
    // The shared middle term has to actually be shared. If the engine solved
    // the two premises independently it would connect anything to anything.
    let store = brain();
    fact(&store, &rel("part-of", "a", "b"));
    fact(&store, &rel("part-of", "c", "d"));
    fact(
        &store,
        &Concept::call("transitive", [Concept::named("part-of")]),
    );

    assert!(
        !holds(&store, &rel("part-of", "a", "d")),
        "the join was not enforced"
    );
}

#[test]
fn transitivity_requires_the_declaration() {
    let store = brain();
    fact(&store, &rel("beat", "a", "b"));
    fact(&store, &rel("beat", "b", "c"));
    // Beating is not transitive and nobody said it was.
    assert!(!holds(&store, &rel("beat", "a", "c")));
}

// ---------------------------------------------------------------------------
// SubtypeOf, which is is-a without a privileged is-a edge
// ---------------------------------------------------------------------------

#[test]
fn participation_travels_up_a_subtype_chain() {
    let store = brain();
    fact(&store, &rel("participates", "rex", "dog"));
    fact(&store, &rel("subtype-of", "dog", "animal"));
    fact(&store, &rel("subtype-of", "animal", "living-thing"));

    assert!(
        holds(&store, &rel("participates", "rex", "animal")),
        "one level failed"
    );
    assert!(
        holds(&store, &rel("participates", "rex", "living-thing")),
        "the chain did not compose"
    );
}

#[test]
fn participation_does_not_travel_down() {
    // Every dog is an animal; not every animal is a dog. Getting this backwards
    // is the classic ontology bug.
    let store = brain();
    fact(&store, &rel("participates", "rex", "animal"));
    fact(&store, &rel("subtype-of", "dog", "animal"));
    assert!(!holds(&store, &rel("participates", "rex", "dog")));
}

// ---------------------------------------------------------------------------
// Defaults, and how they are defeated
// ---------------------------------------------------------------------------

#[test]
fn a_default_applies_to_participants_and_yields_to_specific_evidence() {
    // People have two arms. Greg is a person, so Greg has two arms, until Spoon
    // learns something specific about Greg. Concept participation gives
    // expectations rather than enforcing ontology, because the world is full of
    // exceptions.
    let store = brain();
    fact(&store, &rel("participates", "greg", "person"));
    fact(&store, &rel("participates", "keal", "person"));
    fact(
        &store,
        &Concept::call(
            "default-expectation",
            [
                Concept::named("person"),
                Concept::named("arm-count"),
                Concept::int(2),
            ],
        ),
    );

    let greg_two = Concept::call("arm-count", [Concept::named("greg"), Concept::int(2)]);
    let keal_two = Concept::call("arm-count", [Concept::named("keal"), Concept::int(2)]);

    assert!(holds(&store, &greg_two), "the default did not apply");
    assert!(holds(&store, &keal_two));

    // Spoon learns something specific about Greg.
    fact(
        &store,
        &Concept::call(
            "known",
            [Concept::named("arm-count"), Concept::named("greg")],
        ),
    );

    assert!(
        !holds(&store, &greg_two),
        "specific evidence did not defeat the default"
    );
    assert!(
        holds(&store, &keal_two),
        "defeating it for Greg defeated it for everyone"
    );
}

#[test]
fn a_default_reaches_through_a_subtype_chain() {
    // Rex participates in Dog, Dog is a subtype of Animal, and animals are
    // alive. Two meta-rules composing, neither aware of the other.
    let store = brain();
    fact(&store, &rel("participates", "rex", "dog"));
    fact(&store, &rel("subtype-of", "dog", "animal"));
    fact(
        &store,
        &Concept::call(
            "default-expectation",
            [
                Concept::named("animal"),
                Concept::named("alive"),
                Concept::bool(true),
            ],
        ),
    );

    assert!(holds(
        &store,
        &Concept::call("alive", [Concept::named("rex"), Concept::bool(true)])
    ));
}

// ---------------------------------------------------------------------------
// Synonym
// ---------------------------------------------------------------------------

#[test]
fn a_synonym_resolves_a_surface_form_to_its_concept() {
    let store = brain();
    fact(
        &store,
        &Concept::call("synonym", [Concept::text("pup"), Concept::named("dog")]),
    );
    assert!(holds(
        &store,
        &Concept::call("denotes", [Concept::text("pup"), Concept::named("dog")])
    ));
}

// ---------------------------------------------------------------------------
// Nothing fires without its declaration
// ---------------------------------------------------------------------------

#[test]
fn an_empty_brain_concludes_nothing() {
    // The meta rules are seeded but no facts are asserted. Every one of these
    // must come back false rather than finding a vacuous derivation.
    let store = brain();
    let goals = [
        rel("friend-with", "a", "b"),
        rel("child-of", "a", "b"),
        rel("part-of", "a", "b"),
        rel("participates", "a", "b"),
        Concept::call("arm-count", [Concept::named("a"), Concept::int(2)]),
    ];
    for goal in goals {
        assert!(!holds(&store, &goal), "{goal:?} was derived from nothing");
    }
}

#[test]
fn answers_are_reproducible() {
    let store = brain();
    fact(&store, &rel("friend-with", "greg", "keal"));
    fact(
        &store,
        &Concept::call("symmetric", [Concept::named("friend-with")]),
    );

    let index = DiscriminationTree::from_store(&store).unwrap();
    let goal = Concept::call("friend-with", [h(0), Concept::named("greg")]);
    let first: Vec<Concept> = Engine::new(&store, &index)
        .derive(&goal)
        .unwrap()
        .into_iter()
        .map(|d| d.goal)
        .collect();
    for _ in 0..5 {
        let again: Vec<Concept> = Engine::new(&store, &index)
            .derive(&goal)
            .unwrap()
            .into_iter()
            .map(|d| d.goal)
            .collect();
        assert_eq!(first, again);
    }
}
