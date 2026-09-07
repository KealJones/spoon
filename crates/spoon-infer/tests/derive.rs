//! Backward chaining: the behaviour the rest of the system will rely on.

use chrono::{TimeZone, Utc};
use spoon_concept::{
    Activation, Concept, Effect, Provenance, Realization, RealizationSpec, RuleDirection, Tier,
};
use spoon_infer::{DeriveBudget, Engine, InferError, ScanIndex, Support};
use spoon_store::Store;

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap()
}

fn h(n: u32) -> Concept {
    Concept::hole(n)
}

fn fact(store: &Store, c: &Concept) {
    store
        .assert_concept(c, Provenance::User { episode: None }, None, None)
        .unwrap();
}

fn rule(
    store: &Store,
    name: &str,
    target: &str,
    pattern: Concept,
    condition: Option<Concept>,
    produce: Concept,
) {
    store
        .put_realization(&Realization {
            target: Concept::named(target),
            name: name.into(),
            spec: RealizationSpec::Rule {
                pattern,
                condition,
                produce,
                direction: RuleDirection::Backward,
            },
            effect: Effect::Read,
            activation: Activation::new(now()),
            provenance: Provenance::Bootstrap,
            tier: Tier::Kernel,
        })
        .unwrap();
}

fn engine_over(store: &Store) -> (ScanIndex, ()) {
    (ScanIndex::from_store(store).unwrap(), ())
}

// ---------------------------------------------------------------------------
// Facts alone
// ---------------------------------------------------------------------------

#[test]
fn a_stored_fact_derives_itself_without_any_rule() {
    let store = Store::open_in_memory().unwrap();
    let f = Concept::call("owns", [Concept::named("greg"), Concept::named("dog")]);
    fact(&store, &f);
    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);

    let found = engine.derive(&f).unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].is_direct());
    assert!(engine.holds(&f).unwrap());
}

#[test]
fn an_unasserted_fact_does_not_hold() {
    let store = Store::open_in_memory().unwrap();
    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);
    assert!(
        !engine
            .holds(&Concept::call(
                "owns",
                [Concept::named("greg"), Concept::named("dog")]
            ))
            .unwrap()
    );
}

#[test]
fn a_goal_with_a_hole_is_a_question_and_every_answer_comes_back() {
    // Owns<?0, Dog> asks who owns a dog. Each derivation carries one answer in
    // its substitution.
    let store = Store::open_in_memory().unwrap();
    for owner in ["greg", "keal"] {
        fact(
            &store,
            &Concept::call("owns", [Concept::named(owner), Concept::named("dog")]),
        );
    }
    fact(
        &store,
        &Concept::call("owns", [Concept::named("syd"), Concept::named("cat")]),
    );

    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);
    let found = engine
        .derive(&Concept::call("owns", [h(0), Concept::named("dog")]))
        .unwrap();

    let mut owners: Vec<String> = found
        .iter()
        .map(|d| format!("{:?}", d.substitution.apply(&h(0))))
        .collect();
    owners.sort();
    assert_eq!(owners.len(), 2, "got {found:?}");
}

#[test]
fn a_bare_hole_goal_is_refused_rather_than_read_off_the_whole_brain() {
    let store = Store::open_in_memory().unwrap();
    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);
    assert!(matches!(
        engine.derive(&h(0)),
        Err(InferError::Unanchored { .. })
    ));
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

#[test]
fn symmetry_answers_the_question_the_other_way_round() {
    // The whole point of derivation. FriendWith<Greg, Keal> is stored;
    // FriendWith<Keal, Greg> was never written down and still holds.
    let store = Store::open_in_memory().unwrap();
    fact(
        &store,
        &Concept::call(
            "friend-with",
            [Concept::named("greg"), Concept::named("keal")],
        ),
    );
    fact(
        &store,
        &Concept::call("symmetric", [Concept::named("friend-with")]),
    );

    rule(
        &store,
        "rule-symmetric-friend-with",
        "friend-with",
        Concept::call("friend-with", [h(0), h(1)]),
        Some(Concept::call("symmetric", [Concept::named("friend-with")])),
        Concept::call("friend-with", [h(1), h(0)]),
    );

    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);
    let goal = Concept::call(
        "friend-with",
        [Concept::named("keal"), Concept::named("greg")],
    );
    let found = engine.derive(&goal).unwrap();

    assert!(!found.is_empty(), "symmetry did not fire");
    assert!(!found[0].is_direct(), "this was never asserted directly");
    assert_eq!(
        found[0].rules_used(),
        vec!["rule-symmetric-friend-with".into()]
    );
}

#[test]
fn a_rule_whose_condition_fails_does_not_fire() {
    // Symmetric<FriendWith> is deliberately not asserted, so the rule is inert
    // even though its pattern matches.
    let store = Store::open_in_memory().unwrap();
    fact(
        &store,
        &Concept::call(
            "friend-with",
            [Concept::named("greg"), Concept::named("keal")],
        ),
    );
    rule(
        &store,
        "rule-symmetric-friend-with",
        "friend-with",
        Concept::call("friend-with", [h(0), h(1)]),
        Some(Concept::call("symmetric", [Concept::named("friend-with")])),
        Concept::call("friend-with", [h(1), h(0)]),
    );

    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);
    let goal = Concept::call(
        "friend-with",
        [Concept::named("keal"), Concept::named("greg")],
    );
    assert!(!engine.holds(&goal).unwrap());
}

#[test]
fn an_inverse_relation_derives_the_other_direction() {
    let store = Store::open_in_memory().unwrap();
    fact(
        &store,
        &Concept::call(
            "parent-of",
            [Concept::named("alice"), Concept::named("bob")],
        ),
    );
    rule(
        &store,
        "rule-child-of",
        "child-of",
        Concept::call("parent-of", [h(0), h(1)]),
        None,
        Concept::call("child-of", [h(1), h(0)]),
    );

    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);
    assert!(
        engine
            .holds(&Concept::call(
                "child-of",
                [Concept::named("bob"), Concept::named("alice")]
            ))
            .unwrap()
    );
    assert!(
        !engine
            .holds(&Concept::call(
                "child-of",
                [Concept::named("alice"), Concept::named("bob")]
            ))
            .unwrap()
    );
}

#[test]
fn chains_of_rules_compose() {
    // ancestor from parent, then great-ancestor from ancestor. Two links, and
    // neither conclusion was ever stored.
    let store = Store::open_in_memory().unwrap();
    fact(
        &store,
        &Concept::call(
            "parent-of",
            [Concept::named("alice"), Concept::named("bob")],
        ),
    );
    rule(
        &store,
        "rule-ancestor-base",
        "ancestor-of",
        Concept::call("parent-of", [h(0), h(1)]),
        None,
        Concept::call("ancestor-of", [h(0), h(1)]),
    );
    rule(
        &store,
        "rule-forebear",
        "forebear-of",
        Concept::call("ancestor-of", [h(0), h(1)]),
        None,
        Concept::call("forebear-of", [h(0), h(1)]),
    );

    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);
    let goal = Concept::call(
        "forebear-of",
        [Concept::named("alice"), Concept::named("bob")],
    );
    let found = engine.derive(&goal).unwrap();
    assert!(!found.is_empty(), "the chain did not compose");
    let used = found[0].rules_used();
    assert!(used.contains(&"rule-forebear".into()), "got {used:?}");
}

#[test]
fn a_rule_quantified_over_relations_fires_for_any_of_them() {
    // ?0<?1, ?2> in the conclusion is how Symmetric applies to every relation
    // at once rather than needing one rule per relation.
    let store = Store::open_in_memory().unwrap();
    fact(
        &store,
        &Concept::call(
            "friend-with",
            [Concept::named("greg"), Concept::named("keal")],
        ),
    );
    fact(
        &store,
        &Concept::call("symmetric", [Concept::named("friend-with")]),
    );
    fact(
        &store,
        &Concept::call("married-to", [Concept::named("a"), Concept::named("b")]),
    );
    fact(
        &store,
        &Concept::call("symmetric", [Concept::named("married-to")]),
    );

    rule(
        &store,
        "rule-symmetry-general",
        "symmetric",
        Concept::apply(h(0), vec![h(1), h(2)]),
        Some(Concept::call("symmetric", [h(0)])),
        Concept::apply(h(0), vec![h(2), h(1)]),
    );

    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);
    for (rel, x, y) in [("friend-with", "keal", "greg"), ("married-to", "b", "a")] {
        let goal = Concept::call(rel, [Concept::named(x), Concept::named(y)]);
        assert!(engine.holds(&goal).unwrap(), "{rel} symmetry did not fire");
    }
}

// ---------------------------------------------------------------------------
// Defeasibility
// ---------------------------------------------------------------------------

#[test]
fn a_default_applies_until_direct_evidence_contradicts_it() {
    // People have two arms. Greg is a person, so Greg has two arms, until
    // Spoon learns Greg has one. The world is full of exceptions, so concept
    // participation has to give expectations rather than enforce ontology.
    let store = Store::open_in_memory().unwrap();
    fact(&store, &Concept::call("person", [Concept::named("greg")]));
    fact(&store, &Concept::call("person", [Concept::named("keal")]));

    rule(
        &store,
        "rule-people-have-two-arms",
        "arm-count",
        Concept::call("person", [h(0)]),
        Some(Concept::call(
            "unless",
            [Concept::call("known-arm-count", [h(0)])],
        )),
        Concept::call("arm-count", [h(0), Concept::int(2)]),
    );

    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);
    let greg_two = Concept::call("arm-count", [Concept::named("greg"), Concept::int(2)]);
    assert!(
        engine.holds(&greg_two).unwrap(),
        "the default did not apply"
    );

    // Now Spoon learns something specific about Greg.
    fact(
        &store,
        &Concept::call("known-arm-count", [Concept::named("greg")]),
    );
    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);
    assert!(
        !engine.holds(&greg_two).unwrap(),
        "direct evidence about Greg did not defeat the default"
    );
    // Keal is unaffected: a default defeated for one participant still holds
    // for the others.
    assert!(
        engine
            .holds(&Concept::call(
                "arm-count",
                [Concept::named("keal"), Concept::int(2)]
            ))
            .unwrap()
    );
}

// ---------------------------------------------------------------------------
// Termination
// ---------------------------------------------------------------------------

#[test]
fn a_self_referential_rule_terminates_instead_of_recursing_forever() {
    // P follows from P. Re-entering a goal proves nothing, because the chain
    // would be assuming what it is trying to establish.
    let store = Store::open_in_memory().unwrap();
    rule(
        &store,
        "rule-circular",
        "p",
        Concept::call("p", [h(0)]),
        None,
        Concept::call("p", [h(0)]),
    );
    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);
    assert!(
        !engine
            .holds(&Concept::call("p", [Concept::named("x")]))
            .unwrap()
    );
}

#[test]
fn mutually_recursive_rules_terminate() {
    let store = Store::open_in_memory().unwrap();
    rule(
        &store,
        "rule-a-from-b",
        "a",
        Concept::call("b", [h(0)]),
        None,
        Concept::call("a", [h(0)]),
    );
    rule(
        &store,
        "rule-b-from-a",
        "b",
        Concept::call("a", [h(0)]),
        None,
        Concept::call("b", [h(0)]),
    );
    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);
    assert!(
        !engine
            .holds(&Concept::call("a", [Concept::named("x")]))
            .unwrap()
    );
}

#[test]
fn a_transitive_chain_stops_at_the_depth_budget() {
    // Transitivity over a long chain is legitimate but unbounded. The budget is
    // the only thing that guarantees an answer rather than a hang.
    let store = Store::open_in_memory().unwrap();
    for i in 0..40 {
        fact(
            &store,
            &Concept::call(
                "step",
                [
                    Concept::named(&format!("n{i}")),
                    Concept::named(&format!("n{}", i + 1)),
                ],
            ),
        );
    }
    rule(
        &store,
        "rule-reach-base",
        "reaches",
        Concept::call("step", [h(0), h(1)]),
        None,
        Concept::call("reaches", [h(0), h(1)]),
    );
    rule(
        &store,
        "rule-reach-step",
        "reaches",
        Concept::call("reaches", [h(0), h(1)]),
        None,
        Concept::call("reaches", [h(0), h(2)]),
    );

    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index)
        .with_budget(DeriveBudget::default().with_depth(6).with_steps(500));
    // Whatever it decides, it has to decide it and come back.
    let result = engine.derive(&Concept::call(
        "reaches",
        [Concept::named("n0"), Concept::named("n39")],
    ));
    assert!(result.is_ok() || matches!(result, Err(InferError::Exhausted { .. })));
}

// ---------------------------------------------------------------------------
// Provenance and reproducibility
// ---------------------------------------------------------------------------

#[test]
fn a_derivation_says_which_rules_it_used() {
    // "How do you know that" deserves the chain rather than an assertion of
    // confidence, and credit assignment needs something to blame.
    let store = Store::open_in_memory().unwrap();
    fact(
        &store,
        &Concept::call(
            "parent-of",
            [Concept::named("alice"), Concept::named("bob")],
        ),
    );
    rule(
        &store,
        "rule-child-of",
        "child-of",
        Concept::call("parent-of", [h(0), h(1)]),
        None,
        Concept::call("child-of", [h(1), h(0)]),
    );
    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);
    let found = engine
        .derive(&Concept::call(
            "child-of",
            [Concept::named("bob"), Concept::named("alice")],
        ))
        .unwrap();

    assert_eq!(found[0].rules_used(), vec!["rule-child-of".into()]);
    match &found[0].support {
        Support::Rule { premises, .. } => {
            assert_eq!(premises.len(), 1);
            assert!(premises[0].is_direct(), "the premise was the stored fact");
        }
        other => panic!("expected rule support, got {other:?}"),
    }
}

#[test]
fn derivation_is_reproducible() {
    let store = Store::open_in_memory().unwrap();
    for owner in ["greg", "keal", "syd"] {
        fact(
            &store,
            &Concept::call("owns", [Concept::named(owner), Concept::named("dog")]),
        );
    }
    let (index, _) = engine_over(&store);
    let goal = Concept::call("owns", [h(0), Concept::named("dog")]);

    let first: Vec<Concept> = Engine::new(&store, &index)
        .derive(&goal)
        .unwrap()
        .into_iter()
        .map(|d| d.goal)
        .collect();
    for _ in 0..10 {
        let again: Vec<Concept> = Engine::new(&store, &index)
            .derive(&goal)
            .unwrap()
            .into_iter()
            .map(|d| d.goal)
            .collect();
        assert_eq!(first, again, "derivation order was not stable");
    }
}

#[test]
fn a_forward_only_rule_is_never_used_for_derivation() {
    // Direction is not decoration. A rewrite rule read backward would be
    // unsound in general.
    let store = Store::open_in_memory().unwrap();
    fact(
        &store,
        &Concept::call(
            "parent-of",
            [Concept::named("alice"), Concept::named("bob")],
        ),
    );
    store
        .put_realization(&Realization {
            target: Concept::named("child-of"),
            name: "rewrite-only".into(),
            spec: RealizationSpec::Rule {
                pattern: Concept::call("parent-of", [h(0), h(1)]),
                condition: None,
                produce: Concept::call("child-of", [h(1), h(0)]),
                direction: RuleDirection::Forward,
            },
            effect: Effect::Read,
            activation: Activation::new(now()),
            provenance: Provenance::Bootstrap,
            tier: Tier::Kernel,
        })
        .unwrap();

    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);
    assert!(
        !engine
            .holds(&Concept::call(
                "child-of",
                [Concept::named("bob"), Concept::named("alice")]
            ))
            .unwrap(),
        "a forward-only rule was read backward"
    );
}

#[test]
fn a_deprecated_rule_never_fires() {
    let store = Store::open_in_memory().unwrap();
    fact(
        &store,
        &Concept::call(
            "parent-of",
            [Concept::named("alice"), Concept::named("bob")],
        ),
    );
    store
        .put_realization(&Realization {
            target: Concept::named("child-of"),
            name: "retired".into(),
            spec: RealizationSpec::Rule {
                pattern: Concept::call("parent-of", [h(0), h(1)]),
                condition: None,
                produce: Concept::call("child-of", [h(1), h(0)]),
                direction: RuleDirection::Backward,
            },
            effect: Effect::Read,
            activation: Activation::new(now()),
            provenance: Provenance::Bootstrap,
            tier: Tier::Deprecated,
        })
        .unwrap();

    let (index, _) = engine_over(&store);
    let mut engine = Engine::new(&store, &index);
    assert!(
        !engine
            .holds(&Concept::call(
                "child-of",
                [Concept::named("bob"), Concept::named("alice")]
            ))
            .unwrap()
    );
}
