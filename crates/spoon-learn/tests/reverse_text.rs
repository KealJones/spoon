//! Where search stops being able to help, measured rather than guessed.
//!
//! Reversing text is `join<reverse<chars<?0>>, "">`: eight nodes, built from
//! three bootstrap natives, and completely ordinary. It is also out of reach of
//! enumeration, and knowing exactly where that line falls is what decides
//! whether a capability has to come from the Teacher.

use spoon_concept::{Concept, substitute_positional};
use spoon_eval::{Budget, Evaluator, Outcome, PermissionMode};
use spoon_learn::{SynthBudget, SynthOutcome, synthesize};
use spoon_seat::Spec;
use spoon_store::Store;

fn env() -> (Store, spoon_eval::NativeRegistry) {
    let store = Store::open_in_memory().unwrap();
    let registry = spoon_natives::bootstrap();
    spoon_natives::seed_bootstrap(&store, &registry).unwrap();
    (store, registry)
}

fn reversal_spec(target: &str) -> Spec {
    Spec {
        target: Concept::named(target),
        examples: vec![
            (vec![Concept::text("hello")], Concept::text("olleh")),
            (vec![Concept::text("world")], Concept::text("dlrow")),
            (vec![Concept::text("ab")], Concept::text("ba")),
        ],
        note: None,
    }
}

#[test]
fn the_answer_exists_and_is_correct() {
    // The body is real and works. Nothing about this problem is exotic, which
    // is what makes it the right measuring stick.
    let (store, registry) = env();
    let body = Concept::call(
        "join",
        [
            Concept::call("reverse", [Concept::call("chars", [Concept::hole(0)])]),
            Concept::text(""),
        ],
    );
    assert_eq!(body.size(), 8);

    for (input, expected) in [("hello", "olleh"), ("spoon", "noops"), ("a", "a"), ("", "")] {
        let term = substitute_positional(&body, &[Concept::text(input)]);
        let got = Evaluator::new(&store, &registry)
            .with_budget(Budget::deterministic())
            .with_permission(PermissionMode::Bypass)
            .evaluate(&term);
        assert_eq!(
            got.value(),
            Some(&Concept::text(expected)),
            "failed on {input:?}"
        );
    }
}

#[test]
fn enumeration_reaches_it_now_that_reverse_knows_about_text() {
    // This test used to assert the opposite, and the reason it flipped is the
    // point. The target was join<reverse<chars<?0>>, "">: eight nodes, past
    // what bottom-up enumeration covers with roughly a hundred operators, and
    // a real run explored four hundred thousand candidates in twenty-eight
    // seconds without arriving.
    //
    // Nothing about the search improved. `reverse` learned to reverse text, so
    // the same program is three nodes instead of eight, and the search walks
    // straight to it. Widening what a primitive accepts moved a problem from
    // out of reach to trivial, which is worth more than a faster search.
    let (store, registry) = env();
    let budget = SynthBudget {
        max_size: 9,
        max_millis: 10_000,
        max_nodes: 150_000,
    };
    let outcome = synthesize(&reversal_spec("reverse-text"), &store, &registry, budget);

    match outcome {
        SynthOutcome::Found { size, .. } => {
            assert!(size <= 4, "expected a small body, got size {size}");
        }
        other => panic!("expected to find a body, got {other:?}"),
    }
}

#[test]
fn verification_is_cheap_where_search_is_not() {
    // This is why the Teacher writing the body is worth having. Checking a
    // guess against three examples is microseconds, against a search that
    // cannot finish, so the Teacher supplies the candidate and the examples
    // supply the verdict.
    let (store, registry) = env();
    let spec = reversal_spec("reverse-text");

    let correct = Concept::call(
        "join",
        [
            Concept::call("reverse", [Concept::call("chars", [Concept::hole(0)])]),
            Concept::text(""),
        ],
    );
    // What the local model actually proposed: self-referential, and missing the
    // step that turns the reversed characters back into text.
    let wrong = Concept::call("reverse", [Concept::call("chars", [Concept::hole(0)])]);

    let verifies = |body: &Concept| {
        spec.examples.iter().all(|(inputs, expected)| {
            let term = substitute_positional(body, inputs);
            matches!(
                Evaluator::new(&store, &registry)
                    .with_budget(Budget::deterministic())
                    .with_permission(PermissionMode::Bypass)
                    .evaluate(&term),
                Outcome::Value(ref v) if v == expected
            )
        })
    };

    assert!(verifies(&correct), "the correct body must pass");
    assert!(
        !verifies(&wrong),
        "a body that fails its own examples must not pass"
    );
}

#[test]
fn small_bodies_are_still_found_by_search() {
    // The limit is size, not synthesis being useless. Anything small stays
    // reachable, which is most of what gets learned in a conversation.
    let (store, registry) = env();
    let spec = Spec {
        target: Concept::named("double"),
        examples: vec![
            (vec![Concept::int(3)], Concept::int(6)),
            (vec![Concept::int(5)], Concept::int(10)),
            (vec![Concept::int(0)], Concept::int(0)),
        ],
        note: None,
    };
    let outcome = synthesize(&spec, &store, &registry, SynthBudget::default());
    let SynthOutcome::Found { body, .. } = outcome else {
        panic!("a small body should still be found: {outcome:?}");
    };
    let term = substitute_positional(&body, &[Concept::int(21)]);
    let got = Evaluator::new(&store, &registry)
        .with_budget(Budget::deterministic())
        .with_permission(PermissionMode::Bypass)
        .evaluate(&term);
    assert_eq!(got.value(), Some(&Concept::int(42)));
}
