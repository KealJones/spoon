//! Orchestrator review: the laws synthesis and consolidation have to obey.
//!
//! Both of these change what Spoon can do without being asked to, so the
//! properties that matter are the safety ones: a synthesized body must actually
//! satisfy its examples, and a rewritten body must compute exactly what it did
//! before.

use chrono::{TimeZone, Utc};
use spoon_concept::{
    Activation, Concept, Effect, Provenance, Realization, RealizationSpec, SymbolTable, Tier,
    substitute_positional,
};
use spoon_eval::{Budget, Evaluator, NativeRegistry, Outcome, PermissionMode};
use spoon_learn::{
    ConsolidateConfig, SynthBudget, SynthOutcome, apply_abstraction, consolidate, synthesize,
};
use spoon_seat::Spec;
use spoon_store::Store;

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap()
}

fn env() -> (Store, NativeRegistry) {
    let store = Store::open_in_memory().unwrap();
    let registry = spoon_natives::bootstrap();
    spoon_natives::seed_bootstrap(&store, &registry).unwrap();
    (store, registry)
}

fn eval(store: &Store, reg: &NativeRegistry, c: &Concept) -> Outcome {
    Evaluator::new(store, reg)
        .with_budget(Budget::deterministic())
        .with_permission(PermissionMode::Bypass)
        .with_now(now())
        .evaluate(c)
}

fn spec(target: &str, examples: Vec<(Vec<Concept>, Concept)>) -> Spec {
    Spec {
        target: Concept::named(target),
        examples,
        note: None,
    }
}

fn learned(store: &Store, target: &str, body: Concept) {
    store
        .put_realization(&Realization {
            target: Concept::named(target),
            name: format!("composed-{target}").into(),
            spec: RealizationSpec::Composed { body },
            effect: Effect::Pure,
            activation: Activation::new(now()),
            provenance: Provenance::Synthesized { episode: None },
            tier: Tier::Provisional,
        })
        .unwrap();
}

// ---------------------------------------------------------------------------
// Synthesis: whatever it returns must actually work
// ---------------------------------------------------------------------------

#[test]
fn a_synthesized_body_satisfies_every_example_it_was_given() {
    // The one law. A body that fits three of four examples is not a capability,
    // it is a wrong answer that will surface later on the case nobody checked.
    let (store, reg) = env();
    let cases = vec![
        spec(
            "double",
            vec![
                (vec![Concept::int(3)], Concept::int(6)),
                (vec![Concept::int(5)], Concept::int(10)),
                (vec![Concept::int(0)], Concept::int(0)),
            ],
        ),
        spec(
            "add-two",
            vec![
                (vec![Concept::int(1), Concept::int(2)], Concept::int(3)),
                (vec![Concept::int(10), Concept::int(5)], Concept::int(15)),
            ],
        ),
        spec(
            "shout",
            vec![
                (vec![Concept::text("ab")], Concept::text("AB")),
                (vec![Concept::text("hi")], Concept::text("HI")),
            ],
        ),
    ];

    for s in cases {
        let outcome = synthesize(&s, &store, &reg, SynthBudget::default());
        let SynthOutcome::Found { body, .. } = outcome else {
            panic!("expected to synthesize {:?}, got {outcome:?}", s.target);
        };
        for (inputs, expected) in &s.examples {
            let bound = substitute_positional(&body, inputs);
            let got = eval(&store, &reg, &bound);
            assert_eq!(
                got.value(),
                Some(expected),
                "{:?} body {body:?} failed on {inputs:?}",
                s.target
            );
        }
    }
}

#[test]
fn a_synthesized_body_generalizes_beyond_its_examples() {
    // Examples are evidence of a rule, not the rule. A body that only handles
    // the inputs it was shown has memorized rather than learned.
    let (store, reg) = env();
    let s = spec(
        "double",
        vec![
            (vec![Concept::int(3)], Concept::int(6)),
            (vec![Concept::int(5)], Concept::int(10)),
        ],
    );
    let SynthOutcome::Found { body, .. } = synthesize(&s, &store, &reg, SynthBudget::default())
    else {
        panic!("no body found");
    };
    for (input, expected) in [(21i64, 42i64), (100, 200), (-4, -8)] {
        let bound = substitute_positional(&body, &[Concept::int(input)]);
        assert_eq!(
            eval(&store, &reg, &bound).value(),
            Some(&Concept::int(expected)),
            "body {body:?} did not generalize to {input}"
        );
    }
}

#[test]
fn contradictory_examples_yield_nothing_rather_than_one_of_them() {
    // No function maps one input to two outputs. Returning a body that fits
    // half the evidence would be worse than admitting there is none.
    let (store, reg) = env();
    let s = spec(
        "impossible",
        vec![
            (vec![Concept::int(1)], Concept::int(2)),
            (vec![Concept::int(1)], Concept::int(3)),
        ],
    );
    assert!(
        !matches!(
            synthesize(&s, &store, &reg, SynthBudget::default()),
            SynthOutcome::Found { .. }
        ),
        "a contradiction should not produce a body"
    );
}

#[test]
fn no_examples_is_refused_rather_than_vacuously_satisfied() {
    // Everything satisfies nothing, so an empty spec would make the first
    // candidate examined look correct.
    let (store, reg) = env();
    let s = spec("anything", vec![]);
    assert!(!matches!(
        synthesize(&s, &store, &reg, SynthBudget::default()),
        SynthOutcome::Found { .. }
    ));
}

#[test]
fn synthesis_builds_on_what_spoon_already_learned() {
    // The compositional claim. `quadruple` should be reachable through the
    // stored `double` rather than only from natives.
    let (store, reg) = env();
    learned(
        &store,
        "double",
        Concept::call("math-add", [Concept::hole(0), Concept::hole(0)]),
    );

    let s = spec(
        "quadruple",
        vec![
            (vec![Concept::int(2)], Concept::int(8)),
            (vec![Concept::int(5)], Concept::int(20)),
            (vec![Concept::int(0)], Concept::int(0)),
        ],
    );
    let SynthOutcome::Found { body, .. } = synthesize(&s, &store, &reg, SynthBudget::default())
    else {
        panic!("could not build on a learned capability");
    };
    let bound = substitute_positional(&body, &[Concept::int(7)]);
    assert_eq!(eval(&store, &reg, &bound).value(), Some(&Concept::int(28)));
}

#[test]
fn a_starved_budget_reports_giving_up_rather_than_guessing() {
    let (store, reg) = env();
    let s = spec(
        "hard",
        vec![
            (vec![Concept::int(3)], Concept::int(6)),
            (vec![Concept::int(5)], Concept::int(10)),
        ],
    );
    let tiny = SynthBudget {
        max_nodes: 2,
        max_millis: 1,
        max_size: 7,
    };
    assert!(
        !matches!(
            synthesize(&s, &store, &reg, tiny),
            SynthOutcome::Found { .. }
        ),
        "a starved search must not return a body it never verified"
    );
}

// ---------------------------------------------------------------------------
// Consolidation: rewriting must not change behaviour
// ---------------------------------------------------------------------------

#[test]
fn rewriting_a_body_preserves_exactly_what_it_computes() {
    // The worst possible outcome here is a compression that quietly changes an
    // answer, so this is checked by running both forms rather than by
    // inspecting the shapes.
    let (store, reg) = env();
    let bodies: Vec<Concept> = (1..=4)
        .map(|k| {
            Concept::call(
                "math-add",
                [
                    Concept::call("math-mul", [Concept::hole(0), Concept::int(2)]),
                    Concept::int(k),
                ],
            )
        })
        .collect();

    let abstractions = consolidate(&bodies, ConsolidateConfig::default());
    if abstractions.is_empty() {
        return; // nothing worth naming here is a legitimate answer
    }

    for body in &bodies {
        for abstraction in &abstractions {
            let Some(rewritten) = apply_abstraction(body, abstraction) else {
                continue;
            };
            // The abstraction has to be reachable for the rewrite to run.
            learned(&store, &abstraction.name, abstraction.body.clone());
            for input in [0i64, 1, 7, -3, 1000] {
                let before = eval(
                    &store,
                    &reg,
                    &substitute_positional(body, &[Concept::int(input)]),
                );
                let after = eval(
                    &store,
                    &reg,
                    &substitute_positional(&rewritten, &[Concept::int(input)]),
                );
                assert_eq!(
                    before.value(),
                    after.value(),
                    "rewriting changed the answer at {input}: {body:?} -> {rewritten:?}"
                );
            }
        }
    }
}

#[test]
fn nothing_meaningful_in_common_yields_no_abstraction() {
    // The harmful-transfer guard. An abstraction that generalizes down to
    // almost nothing covers everything and means nothing, and Spoon would then
    // treat unrelated procedures as the same thing.
    let unrelated = vec![
        Concept::call("text-upper", [Concept::hole(0)]),
        Concept::call("list-count", [Concept::hole(0)]),
        Concept::call("math-neg", [Concept::hole(0)]),
        Concept::call("text-trim", [Concept::hole(0)]),
    ];
    let found = consolidate(&unrelated, ConsolidateConfig::default());
    assert!(
        found.is_empty(),
        "unrelated bodies produced {:?}",
        found.iter().map(|a| &a.name).collect::<Vec<_>>()
    );
}

#[test]
fn a_shape_seen_twice_is_not_yet_a_pattern() {
    let twice = vec![
        Concept::call(
            "math-add",
            [
                Concept::call("math-mul", [Concept::hole(0), Concept::int(2)]),
                Concept::int(1),
            ],
        ),
        Concept::call(
            "math-add",
            [
                Concept::call("math-mul", [Concept::hole(0), Concept::int(2)]),
                Concept::int(9),
            ],
        ),
    ];
    let config = ConsolidateConfig {
        min_count: 3,
        ..ConsolidateConfig::default()
    };
    assert!(consolidate(&twice, config).is_empty());
}

#[test]
fn consolidation_is_deterministic() {
    // A compression pass that produces different names or different
    // abstractions on a rerun makes every downstream diff unreadable.
    let bodies: Vec<Concept> = (1..=5)
        .map(|k| {
            Concept::call(
                "math-add",
                [
                    Concept::call("math-mul", [Concept::hole(0), Concept::int(3)]),
                    Concept::int(k),
                ],
            )
        })
        .collect();
    let first = consolidate(&bodies, ConsolidateConfig::default());
    for _ in 0..5 {
        let again = consolidate(&bodies, ConsolidateConfig::default());
        assert_eq!(first.len(), again.len());
        for (a, b) in first.iter().zip(again.iter()) {
            assert_eq!(a.body, b.body);
            assert_eq!(a.name, b.name);
        }
    }
}

#[test]
fn selected_abstractions_do_not_overlap() {
    let bodies: Vec<Concept> = (1..=6)
        .map(|k| {
            Concept::call(
                "math-add",
                [
                    Concept::call("math-mul", [Concept::hole(0), Concept::int(2)]),
                    Concept::call("math-sub", [Concept::int(k), Concept::int(1)]),
                ],
            )
        })
        .collect();
    let found = consolidate(&bodies, ConsolidateConfig::default());
    for (i, a) in found.iter().enumerate() {
        for b in found.iter().skip(i + 1) {
            assert!(
                spoon_concept::generalizes(&a.body, &b.body).is_none()
                    && spoon_concept::generalizes(&b.body, &a.body).is_none(),
                "{} and {} overlap",
                a.name,
                b.name
            );
        }
    }
}

#[test]
fn names_are_readable_and_stable() {
    let table = SymbolTable::new();
    for n in ["math-add", "math-mul", "hole"] {
        table.intern(n);
    }
    let bodies: Vec<Concept> = (1..=4)
        .map(|k| {
            Concept::call(
                "math-add",
                [
                    Concept::call("math-mul", [Concept::hole(0), Concept::int(2)]),
                    Concept::int(k),
                ],
            )
        })
        .collect();
    for abstraction in consolidate(&bodies, ConsolidateConfig::default()) {
        let name = &abstraction.name;
        assert!(!name.is_empty());
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{name} is not kebab-case"
        );
        assert!(name.len() < 60, "{name} is unreadable");
    }
}

#[test]
fn pathological_bodies_do_not_take_the_process_down() {
    // Every structural operation in spoon-concept recurses, so a deep body is a
    // stack overflow waiting to happen and consolidation sees whatever the
    // synthesizer produced.
    let mut deep = Concept::hole(0);
    for _ in 0..2000 {
        deep = Concept::call("math-add", [deep, Concept::int(1)]);
    }
    let wide = Concept::call("list-of", (0..5000).map(Concept::int).collect::<Vec<_>>());
    let shallow: Vec<Concept> = (1..=3)
        .map(|k| {
            Concept::call(
                "math-add",
                [
                    Concept::call("math-mul", [Concept::hole(0), Concept::int(2)]),
                    Concept::int(k),
                ],
            )
        })
        .collect();

    let mut corpus = vec![deep, wide];
    corpus.extend(shallow);
    // The assertion is that this returns at all.
    let _ = consolidate(&corpus, ConsolidateConfig::default());
}
