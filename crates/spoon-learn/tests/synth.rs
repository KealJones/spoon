//! Synthesis end to end, against a real store seeded with the bootstrap
//! natives. Nothing here stubs the evaluator: a body only counts as found if
//! the same evaluator that will run it later agrees it reproduces the examples.

use spoon_concept::{
    Activation, Concept, Effect, Provenance, Realization, RealizationSpec, SymbolId, Tier,
    pre_order, substitute_positional,
};
use spoon_eval::{Budget, Evaluator, NativeRegistry, Outcome};
use spoon_learn::{SynthBudget, SynthLimit, SynthOutcome, learn_from_spec, synthesize};
use spoon_seat::Spec;
use spoon_store::Store;

// ---- fixtures -------------------------------------------------------------

fn brain() -> (Store, NativeRegistry) {
    let registry = spoon_natives::bootstrap();
    let store = Store::open_in_memory().unwrap();
    spoon_natives::seed_bootstrap(&store, &registry).unwrap();
    (store, registry)
}

fn spec(target: &str, examples: &[(&[Concept], Concept)]) -> Spec {
    Spec {
        target: Concept::named(target),
        examples: examples
            .iter()
            .map(|(inputs, output)| (inputs.to_vec(), output.clone()))
            .collect(),
        note: None,
    }
}

/// Every case here has a small answer, and a test that takes four seconds to
/// fail is a test nobody runs.
fn budget() -> SynthBudget {
    SynthBudget::default().with_size(5)
}

/// Run a synthesized body on fresh arguments, the way the evaluator will once
/// it is stored.
fn apply(store: &Store, registry: &NativeRegistry, body: &Concept, args: &[Concept]) -> Concept {
    let term = substitute_positional(body, args);
    call(store, registry, &term)
}

fn call(store: &Store, registry: &NativeRegistry, concept: &Concept) -> Concept {
    let mut evaluator = Evaluator::new(store, registry).with_budget(Budget::deterministic());
    match evaluator.evaluate(concept) {
        Outcome::Value(value) => value,
        other => panic!("did not evaluate: {other:?}"),
    }
}

fn mentions(body: &Concept, name: &str) -> bool {
    let wanted = SymbolId::of(name);
    pre_order(body).any(|node| node.as_symbol() == Some(wanted))
}

fn found(outcome: &SynthOutcome) -> &Concept {
    match outcome {
        SynthOutcome::Found { body, .. } => body,
        other => panic!("expected a body, got {other:?}"),
    }
}

/// Store a `Composed` realization the way `learn_from_spec` would, so a later
/// search can build on it.
fn put_composed(store: &Store, target: &str, name: &str, body: Concept) {
    store.register_symbol(target).unwrap();
    store
        .put_realization(&Realization {
            target: Concept::named(target),
            name: name.into(),
            spec: RealizationSpec::Composed { body },
            effect: Effect::Pure,
            activation: Activation::new(chrono::Utc::now()),
            provenance: Provenance::Synthesized { episode: None },
            tier: Tier::Provisional,
        })
        .unwrap();
}

// ---- the capability itself ------------------------------------------------

#[test]
fn learns_double_and_the_body_generalizes() {
    let (store, registry) = brain();
    let spec = spec(
        "double",
        &[
            (&[Concept::int(3)], Concept::int(6)),
            (&[Concept::int(5)], Concept::int(10)),
        ],
    );

    let outcome = synthesize(&spec, &store, &registry, budget());
    let body = found(&outcome);

    // The design doc's own example, arrived at from two examples and nothing
    // else.
    assert_eq!(
        *body,
        Concept::call("add", [Concept::hole(0), Concept::hole(0)])
    );
    // The point of learning a capability rather than memorizing the examples:
    // it has to work on an input nobody mentioned.
    assert_eq!(
        apply(&store, &registry, body, &[Concept::int(21)]),
        Concept::int(42)
    );

    // Search order is fixed, so the same store and the same spec give the same
    // body every time. A synthesizer that answered differently run to run would
    // make every downstream trace unreproducible.
    let again = synthesize(&spec, &store, &registry, budget());
    assert_eq!(found(&again), body);
}

#[test]
fn learns_a_two_argument_capability() {
    let (store, registry) = brain();
    let spec = spec(
        "sum",
        &[
            (&[Concept::int(2), Concept::int(3)], Concept::int(5)),
            (&[Concept::int(10), Concept::int(4)], Concept::int(14)),
        ],
    );

    let outcome = synthesize(&spec, &store, &registry, budget());
    let body = found(&outcome);

    assert_eq!(
        apply(&store, &registry, body, &[Concept::int(7), Concept::int(8)]),
        Concept::int(15)
    );
}

#[test]
fn learns_over_text() {
    let (store, registry) = brain();
    let spec = spec(
        "shout",
        &[
            (&[Concept::text("ab")], Concept::text("AB")),
            (&[Concept::text("cd")], Concept::text("CD")),
        ],
    );

    let outcome = synthesize(&spec, &store, &registry, budget());
    let body = found(&outcome);

    assert!(mentions(body, "upper"), "expected upper, got {body:?}");
    assert_eq!(
        apply(&store, &registry, body, &[Concept::text("spoon")]),
        Concept::text("SPOON")
    );
}

#[test]
fn prefers_the_smaller_body() {
    let (store, registry) = brain();
    // Tripling has two shapes in reach: `mul<?0, 3>` at four nodes and
    // `add<?0, add<?0, ?0>>` at seven. The third example is what puts the
    // constant 3 in the bank, so both are available and the smaller one has to
    // win.
    let spec = spec(
        "triple",
        &[
            (&[Concept::int(2)], Concept::int(6)),
            (&[Concept::int(4)], Concept::int(12)),
            (&[Concept::int(3)], Concept::int(9)),
        ],
    );

    let outcome = synthesize(&spec, &store, &registry, SynthBudget::default());
    let body = found(&outcome);
    let SynthOutcome::Found { size, .. } = &outcome else {
        unreachable!()
    };

    assert_eq!(*size, 4, "expected the four-node body, got {body:?}");
    assert_eq!(
        apply(&store, &registry, body, &[Concept::int(6)]),
        Concept::int(18)
    );
}

#[test]
fn builds_on_a_previously_learned_capability() {
    let (store, registry) = brain();
    // Teach `double` first. It is an ordinary stored realization, so the
    // search should pick it up as an operator with no special case anywhere.
    put_composed(
        &store,
        "double",
        "synth-double",
        Concept::call("add", [Concept::hole(0), Concept::hole(0)]),
    );

    let spec = spec(
        "quadruple",
        &[
            (&[Concept::int(3)], Concept::int(12)),
            (&[Concept::int(5)], Concept::int(20)),
        ],
    );

    // The claim is only interesting if `double` is what makes this reachable,
    // so check that a brain without it cannot get there at this size.
    let (bare, bare_registry) = brain();
    assert!(
        matches!(
            synthesize(&spec, &bare, &bare_registry, budget()),
            SynthOutcome::Exhausted { .. }
        ),
        "quadruple should be out of reach without the learned capability"
    );

    let outcome = synthesize(&spec, &store, &registry, budget());
    let body = found(&outcome);

    assert!(
        mentions(body, "double"),
        "expected the learned capability to be reused, got {body:?}"
    );
    assert_eq!(
        apply(&store, &registry, body, &[Concept::int(25)]),
        Concept::int(100)
    );
}

#[test]
fn finds_a_constant_that_only_the_examples_mention() {
    let (store, registry) = brain();
    // 10 is in no stock pool. The only way `max<?0, 10>` is reachable is by
    // reading 10 out of the examples.
    let spec = spec(
        "at-least-ten",
        &[
            (&[Concept::int(3)], Concept::int(10)),
            (&[Concept::int(12)], Concept::int(12)),
            (&[Concept::int(10)], Concept::int(10)),
        ],
    );

    let outcome = synthesize(&spec, &store, &registry, budget());
    let body = found(&outcome);

    assert_eq!(
        apply(&store, &registry, body, &[Concept::int(7)]),
        Concept::int(10)
    );
    assert_eq!(
        apply(&store, &registry, body, &[Concept::int(40)]),
        Concept::int(40)
    );
}

// ---- the ways it declines -------------------------------------------------

#[test]
fn contradictory_examples_are_exhausted_not_guessed_at() {
    let (store, registry) = brain();
    let spec = spec(
        "confused",
        &[
            (&[Concept::int(1)], Concept::int(2)),
            (&[Concept::int(1)], Concept::int(3)),
        ],
    );

    match synthesize(&spec, &store, &registry, budget()) {
        SynthOutcome::Exhausted { stats } => {
            // Proven from the spec alone, so nothing was searched.
            assert_eq!(stats.nodes, 0);
        }
        other => panic!("expected exhaustion, got {other:?}"),
    }
}

#[test]
fn zero_examples_is_an_error_not_a_vacuous_success() {
    let (store, registry) = brain();
    let spec = spec("empty", &[]);

    match synthesize(&spec, &store, &registry, budget()) {
        SynthOutcome::Malformed { reason } => assert!(reason.contains("no examples")),
        other => panic!("expected malformed, got {other:?}"),
    }
    assert!(learn_from_spec(&spec, &store, &registry, budget()).is_err());
}

#[test]
fn examples_of_differing_arity_are_malformed() {
    let (store, registry) = brain();
    let spec = Spec {
        target: Concept::named("ragged"),
        examples: vec![
            (vec![Concept::int(1)], Concept::int(1)),
            (vec![Concept::int(1), Concept::int(2)], Concept::int(3)),
        ],
        note: None,
    };

    assert!(matches!(
        synthesize(&spec, &store, &registry, budget()),
        SynthOutcome::Malformed { .. }
    ));
}

#[test]
fn a_target_that_is_not_a_named_concept_is_malformed() {
    let (store, registry) = brain();
    let spec = Spec {
        target: Concept::int(42),
        examples: vec![(vec![Concept::int(1)], Concept::int(1))],
        note: None,
    };

    assert!(matches!(
        synthesize(&spec, &store, &registry, budget()),
        SynthOutcome::Malformed { .. }
    ));
}

#[test]
fn an_example_containing_a_hole_is_malformed() {
    let (store, registry) = brain();
    let spec = Spec {
        target: Concept::named("gappy"),
        examples: vec![(vec![Concept::hole(0)], Concept::int(1))],
        note: None,
    };

    assert!(matches!(
        synthesize(&spec, &store, &registry, budget()),
        SynthOutcome::Malformed { .. }
    ));
}

#[test]
fn a_target_written_as_a_call_uses_its_head() {
    let (store, registry) = brain();
    // The Teacher may well write the target the way it would be used.
    let spec = Spec {
        target: Concept::call("double", [Concept::hole(0)]),
        examples: vec![
            (vec![Concept::int(3)], Concept::int(6)),
            (vec![Concept::int(5)], Concept::int(10)),
        ],
        note: None,
    };

    let learned = learn_from_spec(&spec, &store, &registry, budget())
        .unwrap()
        .expect("a body was found");
    assert_eq!(learned.target, Concept::named("double"));
    assert_eq!(
        call(
            &store,
            &registry,
            &Concept::call("double", [Concept::int(21)])
        ),
        Concept::int(42)
    );
}

#[test]
fn a_tiny_budget_gives_up_rather_than_answering_wrong() {
    let (store, registry) = brain();
    let spec = spec(
        "double",
        &[
            (&[Concept::int(3)], Concept::int(6)),
            (&[Concept::int(5)], Concept::int(10)),
        ],
    );

    match synthesize(&spec, &store, &registry, budget().with_nodes(3)) {
        SynthOutcome::OutOfBudget { limit, stats } => {
            assert_eq!(limit, SynthLimit::Nodes);
            assert_eq!(stats.nodes, 3);
        }
        other => panic!("expected out of budget, got {other:?}"),
    }
}

// ---- pruning --------------------------------------------------------------

#[test]
fn observational_equivalence_prunes_most_of_the_space() {
    let (store, registry) = brain();
    // Nothing of four nodes or fewer maps 1 to 43 and 2 to 44, so this search
    // runs to exhaustion and every candidate it built is accounted for.
    let spec = spec(
        "plus-forty-two",
        &[
            (&[Concept::int(1)], Concept::int(43)),
            (&[Concept::int(2)], Concept::int(44)),
        ],
    );

    let stats = match synthesize(&spec, &store, &registry, budget().with_size(4)) {
        SynthOutcome::Exhausted { stats } => stats,
        other => panic!("expected exhaustion, got {other:?}"),
    };

    assert_eq!(stats.nodes, stats.kept + stats.pruned + stats.discarded);
    assert!(
        stats.pruned > stats.kept,
        "more candidates should die as duplicates than survive: {stats:?}"
    );
    // The bank is what the next level multiplies over, so the ratio between it
    // and the candidates considered is the whole benefit.
    assert!(
        stats.kept * 3 < stats.nodes,
        "pruning barely helped: {stats:?}"
    );
}

// ---- storing what was learned --------------------------------------------

#[test]
fn learn_from_spec_stores_a_realization_that_survives_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain.spoon");
    let registry = spoon_natives::bootstrap();

    let stored = {
        let store = Store::open(&path).unwrap();
        spoon_natives::seed_bootstrap(&store, &registry).unwrap();
        store.register_symbol("double").unwrap();
        let spec = spec(
            "double",
            &[
                (&[Concept::int(3)], Concept::int(6)),
                (&[Concept::int(5)], Concept::int(10)),
            ],
        );
        learn_from_spec(&spec, &store, &registry, budget())
            .unwrap()
            .expect("a body was found")
    };

    assert_eq!(stored.tier, Tier::Provisional);
    assert_eq!(stored.provenance, Provenance::Synthesized { episode: None });
    assert!(matches!(stored.spec, RealizationSpec::Composed { .. }));

    // Reopen: everything survives restart, and the evaluator can now reduce a
    // call to the concept that had no realization a moment ago.
    let store = Store::open(&path).unwrap();
    let reloaded = store
        .realization_by_name(&stored.name)
        .unwrap()
        .expect("the realization is still there");
    assert_eq!(reloaded.spec, stored.spec);

    assert_eq!(
        call(
            &store,
            &registry,
            &Concept::call("double", [Concept::int(21)])
        ),
        Concept::int(42)
    );
}

#[test]
fn a_search_that_finds_nothing_stores_nothing() {
    let (store, registry) = brain();
    let before = store.all_realizations().unwrap().len();
    let spec = spec(
        "plus-forty-two",
        &[
            (&[Concept::int(1)], Concept::int(43)),
            (&[Concept::int(2)], Concept::int(44)),
        ],
    );

    let learned = learn_from_spec(&spec, &store, &registry, budget().with_size(4)).unwrap();

    assert!(learned.is_none());
    assert_eq!(store.all_realizations().unwrap().len(), before);
}
