//! Orchestrator review: the collection and text guarantees the rest of the
//! system will lean on. Written against the laws, not the implementation.

use chrono::{TimeZone, Utc};
use spoon_concept::{Activation, Concept, Ground, Provenance, Realization, RealizationSpec, Tier};
use spoon_eval::{Budget, Evaluator, NativeRegistry, Outcome, PermissionMode};
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

/// Store a learned capability the way Stage 3 synthesis will: as concepts, not
/// as code.
fn learn(store: &Store, target: &str, body: Concept) {
    store
        .put_realization(&Realization {
            target: Concept::named(target),
            name: format!("composed-{target}").into(),
            spec: RealizationSpec::Composed { body },
            effect: spoon_concept::Effect::Pure,
            activation: Activation::new(now()),
            provenance: Provenance::Synthesized { episode: None },
            tier: Tier::Provisional,
        })
        .unwrap();
}

fn eval(store: &Store, registry: &NativeRegistry, c: &Concept) -> Outcome {
    Evaluator::new(store, registry)
        .with_budget(Budget::deterministic())
        .with_permission(PermissionMode::Bypass)
        .with_now(now())
        .evaluate(c)
}

fn value(store: &Store, registry: &NativeRegistry, c: &Concept) -> Concept {
    match eval(store, registry, c) {
        Outcome::Value(v) => v,
        other => panic!("expected a value, got {other:?}"),
    }
}

fn is_error(out: &Outcome) -> bool {
    matches!(out, Outcome::Failed(_) | Outcome::Stuck { .. })
}

fn list(items: impl IntoIterator<Item = Concept>) -> Concept {
    Concept::call("list-of", items)
}

// ---------------------------------------------------------------------------
// Higher-order functions need no special machinery
// ---------------------------------------------------------------------------

#[test]
fn a_learned_capability_works_as_a_mapper() {
    // This is the payoff of having one representation. `double` is stored as
    // concepts, and Map applying it is the evaluator calling itself. Nothing in
    // Map knows the difference between a native and something Spoon learned
    // yesterday.
    let (store, reg) = env();
    learn(
        &store,
        "double",
        Concept::call("math-add", [Concept::hole(0), Concept::hole(0)]),
    );

    let out = value(
        &store,
        &reg,
        &Concept::call(
            "list-map",
            [
                list([Concept::int(1), Concept::int(2), Concept::int(3)]),
                Concept::named("double"),
            ],
        ),
    );
    assert_eq!(
        out,
        list([Concept::int(2), Concept::int(4), Concept::int(6)])
    );
}

#[test]
fn a_capability_learned_on_top_of_another_also_maps() {
    // triple is built on double, which is built on add. Three layers, and Map
    // still needs to know nothing.
    let (store, reg) = env();
    learn(
        &store,
        "double",
        Concept::call("math-add", [Concept::hole(0), Concept::hole(0)]),
    );
    learn(
        &store,
        "triple",
        Concept::call(
            "math-add",
            [
                Concept::hole(0),
                Concept::call("double", [Concept::hole(0)]),
            ],
        ),
    );
    let out = value(
        &store,
        &reg,
        &Concept::call(
            "list-map",
            [
                list([Concept::int(1), Concept::int(5)]),
                Concept::named("triple"),
            ],
        ),
    );
    assert_eq!(out, list([Concept::int(3), Concept::int(15)]));
}

#[test]
fn natives_and_learned_capabilities_are_interchangeable_as_arguments() {
    let (store, reg) = env();
    let native_mapped = value(
        &store,
        &reg,
        &Concept::call(
            "list-map",
            [list([Concept::text("ab")]), Concept::named("text-upper")],
        ),
    );
    assert_eq!(native_mapped, list([Concept::text("AB")]));
}

#[test]
fn higher_order_results_compose_with_ordinary_ones() {
    let (store, reg) = env();
    learn(
        &store,
        "double",
        Concept::call("math-add", [Concept::hole(0), Concept::hole(0)]),
    );
    let out = value(
        &store,
        &reg,
        &Concept::call(
            "math-sum",
            [Concept::call(
                "list-map",
                [
                    list([Concept::int(1), Concept::int(2), Concept::int(3)]),
                    Concept::named("double"),
                ],
            )],
        ),
    );
    assert_eq!(out, Concept::int(12));
}

#[test]
fn filter_runs_a_learned_predicate() {
    let (store, reg) = env();
    learn(
        &store,
        "big",
        Concept::call("logic-gt", [Concept::hole(0), Concept::int(2)]),
    );
    let out = value(
        &store,
        &reg,
        &Concept::call(
            "list-filter",
            [
                list([
                    Concept::int(1),
                    Concept::int(3),
                    Concept::int(2),
                    Concept::int(5),
                ]),
                Concept::named("big"),
            ],
        ),
    );
    assert_eq!(out, list([Concept::int(3), Concept::int(5)]));
}

// ---------------------------------------------------------------------------
// Empty lists have to have defined answers
// ---------------------------------------------------------------------------

#[test]
fn empty_list_answers_are_the_identity_of_the_operation() {
    let (store, reg) = env();
    assert_eq!(
        value(&store, &reg, &Concept::call("math-sum", [list([])])),
        Concept::int(0)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("math-product", [list([])])),
        Concept::int(1)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-count", [list([])])),
        Concept::int(0)
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("list-is-empty", [list([])])),
        Concept::bool(true)
    );
    // Vacuous truth for all, vacuous falsehood for any.
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-all", [list([]), Concept::named("math-is-even")])
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("list-any", [list([]), Concept::named("math-is-even")])
        ),
        Concept::bool(false)
    );
}

#[test]
fn operations_with_no_defensible_empty_answer_error() {
    // There is no smallest element of nothing. Returning zero would be a lie
    // that propagates.
    let (store, reg) = env();
    for op in ["list-first", "list-last", "list-min-of", "list-max-of"] {
        let out = eval(&store, &reg, &Concept::call(op, [list([])]));
        assert!(is_error(&out), "{op} on an empty list gave {out:?}");
    }
}

#[test]
fn out_of_range_access_is_refused_rather_than_guessed() {
    let (store, reg) = env();
    let three = list([Concept::int(1), Concept::int(2), Concept::int(3)]);
    for index in [3, 99, -1] {
        let out = eval(
            &store,
            &reg,
            &Concept::call("list-nth", [three.clone(), Concept::int(index)]),
        );
        assert!(is_error(&out), "nth {index} gave {out:?}");
    }
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

#[test]
fn sorting_is_stable_and_reproducible() {
    // An evaluation that returns a different order on a rerun makes every
    // downstream test flaky and every diff unreadable.
    let (store, reg) = env();
    learn(&store, "self", Concept::hole(0));
    let input = list(
        (0..40)
            .map(|i| Concept::int((i * 7) % 11))
            .collect::<Vec<_>>(),
    );
    let expr = Concept::call("list-sort-by", [input, Concept::named("self")]);
    let first = value(&store, &reg, &expr);
    for _ in 0..10 {
        assert_eq!(value(&store, &reg, &expr), first);
    }
    let sorted: Vec<i64> = first
        .args()
        .iter()
        .map(|c| c.as_ground().and_then(Ground::as_i64).unwrap())
        .collect();
    let mut expected = sorted.clone();
    expected.sort();
    assert_eq!(sorted, expected, "sort-by did not sort");
}

#[test]
fn mapping_preserves_order() {
    let (store, reg) = env();
    learn(
        &store,
        "double",
        Concept::call("math-add", [Concept::hole(0), Concept::hole(0)]),
    );
    let input = list((1..=20).map(Concept::int).collect::<Vec<_>>());
    let out = value(
        &store,
        &reg,
        &Concept::call("list-map", [input, Concept::named("double")]),
    );
    let got: Vec<i64> = out
        .args()
        .iter()
        .map(|c| c.as_ground().and_then(Ground::as_i64).unwrap())
        .collect();
    assert_eq!(got, (1..=20).map(|i| i * 2).collect::<Vec<i64>>());
}

// ---------------------------------------------------------------------------
// Text is measured in characters, and never splits one
// ---------------------------------------------------------------------------

#[test]
fn text_length_counts_characters_not_bytes() {
    let (store, reg) = env();
    let cases = [
        ("abc", 3),
        ("\u{4E2D}\u{6587}", 2),
        ("caf\u{e9}", 4),
        ("", 0),
    ];
    for (text, expected) in cases {
        let out = value(
            &store,
            &reg,
            &Concept::call("text-length", [Concept::text(text)]),
        );
        assert_eq!(out, Concept::int(expected), "text-length of {text:?}");
    }
}

#[test]
fn substring_never_splits_a_character_and_never_panics() {
    // Byte-based slicing on multibyte input is the classic way to panic in
    // Rust. Every offset into a multibyte string must either work or error.
    let (store, reg) = env();
    let text = "a\u{4E2D}b\u{1F600}c";
    for start in 0..8i64 {
        for end in 0..8i64 {
            let out = eval(
                &store,
                &reg,
                &Concept::call(
                    "text-substring",
                    [Concept::text(text), Concept::int(start), Concept::int(end)],
                ),
            );
            if let Outcome::Value(v) = out {
                let s = v
                    .as_ground()
                    .and_then(Ground::as_str)
                    .expect("substring gives text");
                // Whatever came back has to be a well-formed run of characters
                // from the original, not a byte fragment.
                assert!(
                    text.contains(s),
                    "substring produced {s:?}, not a slice of {text:?}"
                );
            }
        }
    }
}

#[test]
fn char_at_addresses_characters() {
    let (store, reg) = env();
    let text = "a\u{4E2D}b";
    let out = value(
        &store,
        &reg,
        &Concept::call("text-char-at", [Concept::text(text), Concept::int(1)]),
    );
    assert_eq!(out, Concept::text("\u{4E2D}"));
    assert!(is_error(&eval(
        &store,
        &reg,
        &Concept::call("text-char-at", [Concept::text(text), Concept::int(9)])
    )));
}

#[test]
fn case_conversion_handles_non_ascii() {
    let (store, reg) = env();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-upper", [Concept::text("caf\u{e9}")])
        ),
        Concept::text("CAF\u{c9}")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-lower", [Concept::text("STRA\u{df}E")])
        ),
        Concept::text("stra\u{df}e")
    );
}

#[test]
fn split_and_join_round_trip() {
    let (store, reg) = env();
    let parts = value(
        &store,
        &reg,
        &Concept::call("text-split", [Concept::text("a,b,c"), Concept::text(",")]),
    );
    assert_eq!(parts.arity(), 3);
    let rejoined = value(
        &store,
        &reg,
        &Concept::call("text-join", [parts, Concept::text(",")]),
    );
    assert_eq!(rejoined, Concept::text("a,b,c"));
}

#[test]
fn a_huge_repeat_is_refused_instead_of_allocated() {
    // The evaluator's size budget counts concept nodes, and a gigabyte string
    // is one node, so this guard cannot live in the evaluator.
    let (store, reg) = env();
    let out = eval(
        &store,
        &reg,
        &Concept::call("text-repeat", [Concept::text("xy"), Concept::int(i64::MAX)]),
    );
    assert!(is_error(&out), "an unbounded repeat was allowed: {out:?}");
}

#[test]
fn parsing_refuses_garbage_rather_than_defaulting() {
    let (store, reg) = env();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-parse-int", [Concept::text("42")])
        ),
        Concept::int(42)
    );
    for bad in ["", "abc", "4.2", " 42 x"] {
        let out = eval(
            &store,
            &reg,
            &Concept::call("text-parse-int", [Concept::text(bad)]),
        );
        assert!(is_error(&out), "parse-int({bad:?}) gave {out:?}");
    }
}

// ---------------------------------------------------------------------------
// Nothing panics
// ---------------------------------------------------------------------------

#[test]
fn no_native_panics_on_nonsense_arguments() {
    let (store, reg) = env();
    let junk = [
        Concept::named("nonsense"),
        Concept::text("\u{1F600}\u{4E2D}"),
        Concept::int(-1),
        Concept::bool(false),
        list([Concept::text("a"), Concept::int(1)]),
        Concept::hole(0),
    ];
    for name in reg.names() {
        for arity in 0..4usize {
            for bad in &junk {
                let args: Vec<Concept> = std::iter::repeat_n(bad.clone(), arity).collect();
                let expr = Concept::apply(
                    Concept::symbol(spoon_concept::SymbolId::of(name.as_str())),
                    args,
                );
                let _ = eval(&store, &reg, &expr);
            }
        }
    }
}
