//! End to end over a real evaluator and a real store.
//!
//! Everything here goes through `Evaluator::evaluate`, because the natives are
//! only correct in the shape the evaluator actually calls them: arity checked,
//! arguments reduced, effect weighed against the permission mode.

use chrono::{DateTime, TimeZone, Utc};
use spoon_concept::{Concept, ConceptMeta, Effect, Ground, Provenance, Tier};
use spoon_eval::{Budget, Evaluator, NativeRegistry, Outcome, PermissionMode};
use spoon_natives::{bootstrap, seed_bootstrap};
use spoon_store::Store;

fn pinned() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap()
}

/// Epoch seconds for [`pinned`], written out rather than computed so the test
/// cannot agree with a bug in the code it is checking.
const PINNED_EPOCH_SECONDS: i64 = 1_788_436_800;

fn brain() -> (Store, NativeRegistry) {
    let store = Store::open_in_memory().unwrap();
    let registry = bootstrap();
    seed_bootstrap(&store, &registry).unwrap();
    (store, registry)
}

/// Evaluate with every effect allowed. Most tests here exercise natives that
/// write, and the permission layer gets its own tests below.
fn run(store: &Store, registry: &NativeRegistry, c: &Concept) -> Outcome {
    Evaluator::new(store, registry)
        .with_budget(Budget::deterministic())
        .with_permission(PermissionMode::Bypass)
        .with_now(pinned())
        .evaluate(c)
}

fn value(store: &Store, registry: &NativeRegistry, c: &Concept) -> Concept {
    match run(store, registry, c) {
        Outcome::Value(v) => v,
        other => panic!("expected a value from {c:?}, got {other:?}"),
    }
}

/// Evaluate expecting failure, and hand back what the realization said. The
/// evaluator turns a failed native into `Stuck` at the top level, so the
/// message lives in the trace rather than in the outcome.
fn failure(store: &Store, registry: &NativeRegistry, c: &Concept) -> String {
    let mut ev = Evaluator::new(store, registry)
        .with_budget(Budget::deterministic())
        .with_permission(PermissionMode::Bypass)
        .with_now(pinned());
    let outcome = ev.evaluate(c);
    assert!(
        !outcome.is_value(),
        "expected {c:?} to fail, got {outcome:?}"
    );
    let messages: Vec<String> = ev
        .trace()
        .failures()
        .into_iter()
        .map(|(_, _, message)| message.to_string())
        .collect();
    assert!(
        !messages.is_empty(),
        "expected a recorded failure for {c:?}, got {outcome:?}"
    );
    messages.join(" | ")
}

fn text(s: &str) -> Concept {
    Concept::text(s)
}

fn owns(who: &str, what: &str) -> Concept {
    Concept::call("owns", [Concept::named(who), Concept::named(what)])
}

fn items(c: &Concept) -> Vec<Concept> {
    assert_eq!(
        c.head_symbol(),
        Some(spoon_concept::SymbolId::of("list-of")),
        "expected a list, got {c:?}"
    );
    c.args().to_vec()
}

fn json(raw: &str) -> Concept {
    Concept::json(serde_json::from_str(raw).unwrap())
}

// ---------------------------------------------------------------------------
// Belief
// ---------------------------------------------------------------------------

#[test]
fn asserting_makes_a_concept_exist() {
    let (store, reg) = brain();
    let fact = owns("greg", "dog");

    assert_eq!(
        value(&store, &reg, &Concept::call("store-exists", [fact.clone()])),
        Concept::bool(false)
    );

    // Assert echoes the concept back, so it composes inside a larger term.
    assert_eq!(
        value(&store, &reg, &Concept::call("store-assert", [fact.clone()])),
        fact
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("store-exists", [fact.clone()])),
        Concept::bool(true)
    );
}

#[test]
fn retracting_ends_the_belief_without_erasing_the_history() {
    let (store, reg) = brain();
    let fact = owns("greg", "dog");
    value(&store, &reg, &Concept::call("store-assert", [fact.clone()]));

    // A moment strictly inside the window the belief was held, so the history
    // check below cannot land on the boundary.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let while_believed = Utc::now();
    std::thread::sleep(std::time::Duration::from_millis(5));

    // Retraction is stamped with the context clock, so this evaluator runs on
    // the real one: an invalidation in the far past or the far future would
    // not describe when the belief actually ended.
    let withdrawn = Evaluator::new(&store, &reg)
        .with_budget(Budget::deterministic())
        .with_permission(PermissionMode::Bypass)
        .evaluate(&Concept::call("store-retract-claim", [fact.clone()]));
    assert_eq!(withdrawn.value(), Some(&Concept::int(1)));

    assert_eq!(
        value(&store, &reg, &Concept::call("store-exists", [fact.clone()])),
        Concept::bool(false)
    );

    // The earlier state is still there.
    let then = store.assertions_at(&fact, while_believed).unwrap();
    assert_eq!(then.len(), 1, "the belief held at {while_believed} is gone");
    assert!(store.live_assertions(&fact).unwrap().is_empty());
}

#[test]
fn retracting_something_never_believed_withdraws_nothing() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("store-retract-claim", [owns("greg", "dog")])
        ),
        Concept::int(0)
    );
}

// ---------------------------------------------------------------------------
// Permission
// ---------------------------------------------------------------------------

#[test]
fn writes_are_refused_under_the_default_permission_mode() {
    let (store, reg) = brain();
    let fact = owns("greg", "dog");

    let outcome = Evaluator::new(&store, &reg)
        .with_budget(Budget::deterministic())
        .evaluate(&Concept::call("store-assert", [fact.clone()]));

    match outcome {
        Outcome::NeedsPermission { effect, .. } => assert_eq!(effect, Effect::Write),
        other => panic!("a write ran unattended under the default mode: {other:?}"),
    }
    assert!(
        !store.holds(&fact).unwrap(),
        "the refused write reached the store anyway"
    );

    // And it succeeds once permission is granted, so the gate is the only
    // thing that stopped it.
    value(&store, &reg, &Concept::call("store-assert", [fact.clone()]));
    assert!(store.holds(&fact).unwrap());
}

#[test]
fn retracting_is_gated_the_same_way_asserting_is() {
    let (store, reg) = brain();
    let fact = owns("greg", "dog");
    value(&store, &reg, &Concept::call("store-assert", [fact.clone()]));

    let outcome = Evaluator::new(&store, &reg)
        .with_budget(Budget::deterministic())
        .evaluate(&Concept::call("store-retract-claim", [fact.clone()]));

    match outcome {
        Outcome::NeedsPermission { effect, .. } => assert_eq!(effect, Effect::Write),
        other => panic!("a retraction ran unattended: {other:?}"),
    }
    assert!(
        store.holds(&fact).unwrap(),
        "the belief was withdrawn anyway"
    );
}

#[test]
fn reads_run_unattended_under_the_default_permission_mode() {
    let (store, reg) = brain();
    let fact = owns("greg", "dog");
    value(&store, &reg, &Concept::call("store-assert", [fact.clone()]));

    let outcome = Evaluator::new(&store, &reg)
        .with_budget(Budget::deterministic())
        .evaluate(&Concept::call("store-exists", [fact]));
    assert_eq!(outcome.value(), Some(&Concept::bool(true)));
}

// ---------------------------------------------------------------------------
// Recall
// ---------------------------------------------------------------------------

fn seed_facts(store: &Store, reg: &NativeRegistry) {
    for (who, what) in [
        ("greg", "dog"),
        ("keal", "dog"),
        ("greg", "cat"),
        ("mira", "boat"),
    ] {
        value(
            store,
            reg,
            &Concept::call("store-assert", [owns(who, what)]),
        );
    }
    value(
        store,
        reg,
        &Concept::call(
            "store-assert",
            [Concept::call(
                "friend-with",
                [Concept::named("greg"), Concept::named("keal")],
            )],
        ),
    );
}

#[test]
fn recall_with_a_hole_finds_the_matches_and_nothing_else() {
    let (store, reg) = brain();
    seed_facts(&store, &reg);

    let found = value(
        &store,
        &reg,
        &Concept::call(
            "store-recall",
            [Concept::call(
                "owns",
                [Concept::hole(0), Concept::named("dog")],
            )],
        ),
    );
    let mut got = items(&found);
    got.sort_by_key(|c| c.content_id());

    let mut want = vec![owns("greg", "dog"), owns("keal", "dog")];
    want.sort_by_key(|c| c.content_id());
    assert_eq!(got, want);
}

#[test]
fn recall_of_a_pattern_with_no_holes_asks_about_that_exact_concept() {
    let (store, reg) = brain();
    seed_facts(&store, &reg);

    let found = value(
        &store,
        &reg,
        &Concept::call("store-recall", [owns("greg", "dog")]),
    );
    assert_eq!(items(&found), vec![owns("greg", "dog")]);

    let missing = value(
        &store,
        &reg,
        &Concept::call("store-recall", [owns("nobody", "dragon")]),
    );
    assert!(items(&missing).is_empty());
}

#[test]
fn recall_can_anchor_on_a_concrete_argument_when_the_head_is_a_hole() {
    let (store, reg) = brain();
    seed_facts(&store, &reg);

    let pattern = Concept::apply(
        Concept::hole(0),
        vec![Concept::named("greg"), Concept::named("dog")],
    );
    let found = value(&store, &reg, &Concept::call("store-recall", [pattern]));
    assert_eq!(items(&found), vec![owns("greg", "dog")]);
}

#[test]
fn recall_refuses_a_pattern_that_would_match_the_whole_brain() {
    let (store, reg) = brain();
    seed_facts(&store, &reg);
    let message = failure(
        &store,
        &reg,
        &Concept::call("store-recall", [Concept::hole(0)]),
    );
    assert!(
        message.contains("all holes"),
        "unhelpful message: {message}"
    );
}

#[test]
fn recall_ordering_does_not_depend_on_insertion_order() {
    let pattern = Concept::call("owns", [Concept::hole(0), Concept::hole(1)]);
    let forward = ["dog", "cat", "boat", "raven"];

    let (store_a, reg_a) = brain();
    for what in forward {
        value(
            &store_a,
            &reg_a,
            &Concept::call("store-assert", [owns("greg", what)]),
        );
    }
    let first = items(&value(
        &store_a,
        &reg_a,
        &Concept::call("store-recall", [pattern.clone()]),
    ));

    // Same query again, same answer.
    let again = items(&value(
        &store_a,
        &reg_a,
        &Concept::call("store-recall", [pattern.clone()]),
    ));
    assert_eq!(first, again);

    // A second brain holding the same facts, written in the opposite order.
    let (store_b, reg_b) = brain();
    for what in forward.iter().rev() {
        value(
            &store_b,
            &reg_b,
            &Concept::call("store-assert", [owns("greg", what)]),
        );
    }
    let reversed = items(&value(
        &store_b,
        &reg_b,
        &Concept::call("store-recall", [pattern]),
    ));

    assert_eq!(first.len(), 4);
    assert_eq!(first, reversed, "row order leaked into the answer");
}

#[test]
fn recall_honours_an_explicit_limit() {
    let (store, reg) = brain();
    seed_facts(&store, &reg);
    let pattern = Concept::call("owns", [Concept::hole(0), Concept::hole(1)]);

    let capped = value(
        &store,
        &reg,
        &Concept::call("store-recall", [pattern.clone(), Concept::int(2)]),
    );
    assert_eq!(items(&capped).len(), 2);

    // The capped result is the prefix of the uncapped one, so a limit narrows
    // the answer rather than changing it.
    let full = items(&value(
        &store,
        &reg,
        &Concept::call("store-recall", [pattern]),
    ));
    assert_eq!(items(&capped), full[..2].to_vec());
}

#[test]
fn a_limit_of_zero_is_an_error_rather_than_an_empty_answer() {
    let (store, reg) = brain();
    seed_facts(&store, &reg);
    let message = failure(
        &store,
        &reg,
        &Concept::call(
            "store-recall",
            [
                Concept::call("owns", [Concept::hole(0), Concept::hole(1)]),
                Concept::int(0),
            ],
        ),
    );
    assert!(message.contains("positive"), "unhelpful message: {message}");
}

#[test]
fn count_matching_counts_what_recall_returns() {
    let (store, reg) = brain();
    seed_facts(&store, &reg);
    let pattern = Concept::call("owns", [Concept::hole(0), Concept::named("dog")]);

    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("store-count-recalled", [pattern])
        ),
        Concept::int(2)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("store-count-recalled", [owns("nobody", "dragon")])
        ),
        Concept::int(0)
    );
}

#[test]
fn recall_about_finds_every_mention_of_a_concept() {
    let (store, reg) = brain();
    seed_facts(&store, &reg);

    let found = value(
        &store,
        &reg,
        &Concept::call("store-recall-about", [Concept::named("greg")]),
    );
    let got = items(&found);
    assert!(got.contains(&owns("greg", "dog")));
    assert!(got.contains(&owns("greg", "cat")));
    assert!(got.contains(&Concept::call(
        "friend-with",
        [Concept::named("greg"), Concept::named("keal")]
    )));
    assert!(!got.contains(&owns("mira", "boat")));

    let capped = items(&value(
        &store,
        &reg,
        &Concept::call(
            "store-recall-about",
            [Concept::named("greg"), Concept::int(1)],
        ),
    ));
    assert_eq!(capped.len(), 1);
}

#[test]
fn recall_by_head_takes_the_name_as_text_or_as_the_concept() {
    let (store, reg) = brain();
    seed_facts(&store, &reg);

    let by_text = items(&value(
        &store,
        &reg,
        &Concept::call("store-recall-by-head", [text("owns")]),
    ));
    let by_concept = items(&value(
        &store,
        &reg,
        &Concept::call("store-recall-by-head", [Concept::named("owns")]),
    ));

    assert_eq!(by_text.len(), 4);
    assert_eq!(by_text, by_concept);
    assert!(
        by_text
            .iter()
            .all(|c| c.head_symbol() == Some(spoon_concept::SymbolId::of("owns")))
    );
}

// ---------------------------------------------------------------------------
// Description
// ---------------------------------------------------------------------------

#[test]
fn describe_gives_the_stored_surface_forms() {
    let (store, reg) = brain();
    store
        .put_meta(
            &ConceptMeta::new(
                Concept::named("greg"),
                Provenance::Bootstrap,
                Tier::Kernel,
                pinned(),
            )
            .with_surface_forms(["Greg", "Gregory"]),
        )
        .unwrap();

    let found = value(
        &store,
        &reg,
        &Concept::call("store-describe", [Concept::named("greg")]),
    );
    assert_eq!(items(&found), vec![text("Greg"), text("Gregory")]);
}

#[test]
fn describing_an_unknown_concept_gives_an_empty_list_rather_than_an_error() {
    let (store, reg) = brain();
    let found = value(
        &store,
        &reg,
        &Concept::call(
            "store-describe",
            [Concept::named("nobody-has-mentioned-this")],
        ),
    );
    assert!(items(&found).is_empty());
}

#[test]
fn surface_of_finds_the_concepts_that_go_by_a_form() {
    let (store, reg) = brain();
    store
        .put_meta(
            &ConceptMeta::new(
                Concept::named("greg"),
                Provenance::Bootstrap,
                Tier::Kernel,
                pinned(),
            )
            .with_surface_forms(["Greg"]),
        )
        .unwrap();

    // Case folded, because a surface form is how something is said and not how
    // it happens to be capitalized.
    let found = value(
        &store,
        &reg,
        &Concept::call("store-surface-of", [text("greg")]),
    );
    assert_eq!(items(&found), vec![Concept::named("greg")]);

    let nothing = value(
        &store,
        &reg,
        &Concept::call("store-surface-of", [text("nobody says this")]),
    );
    assert!(items(&nothing).is_empty());
}

#[test]
fn the_bootstrap_natives_describe_themselves() {
    // seed_bootstrap files each native under its own name, so this is the
    // round trip between describe and surface-of over real stored data.
    let (store, reg) = brain();
    assert_eq!(
        items(&value(
            &store,
            &reg,
            &Concept::call("store-describe", [Concept::named("json-parse")])
        )),
        vec![text("json-parse")]
    );
    assert_eq!(
        items(&value(
            &store,
            &reg,
            &Concept::call("store-surface-of", [text("json-parse")])
        )),
        vec![Concept::named("json-parse")]
    );
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

#[test]
fn parse_json_produces_a_json_concept() {
    let (store, reg) = brain();
    let parsed = value(
        &store,
        &reg,
        &Concept::call("json-parse", [text(r#"{"title":"Spoon","stars":3}"#)]),
    );
    assert_eq!(parsed, json(r#"{"title":"Spoon","stars":3}"#));
}

#[test]
fn malformed_json_fails_with_the_parse_error() {
    let (store, reg) = brain();
    let message = failure(
        &store,
        &reg,
        &Concept::call("json-parse", [text("{ not json")]),
    );
    assert!(message.contains("not valid JSON"), "unhelpful: {message}");
    assert!(message.contains("column"), "no position given: {message}");
}

#[test]
fn field_converts_json_scalars_to_native_concepts() {
    let (store, reg) = brain();
    let doc = json(
        r#"{"title":"Spoon","stars":3,"ratio":1.5,"live":true,"tags":["a"],"nested":{"x":1},"nothing":null}"#,
    );
    let get = |name: &str| {
        value(
            &store,
            &reg,
            &Concept::call("json-field", [doc.clone(), text(name)]),
        )
    };

    assert_eq!(get("title"), text("Spoon"));
    assert_eq!(get("stars"), Concept::int(3));
    assert_eq!(get("ratio"), Concept::float(1.5));
    assert_eq!(get("live"), Concept::bool(true));

    // Containers and a present null stay JSON: there is nothing better to turn
    // them into, and inventing an absence for null would be a guess.
    assert_eq!(get("tags"), json(r#"["a"]"#));
    assert_eq!(get("nested"), json(r#"{"x":1}"#));
    assert_eq!(get("nothing"), json("null"));
}

#[test]
fn a_missing_field_names_the_field_and_the_keys_that_are_there() {
    let (store, reg) = brain();
    let doc = json(r#"{"title":"Spoon","stars":3}"#);
    let message = failure(
        &store,
        &reg,
        &Concept::call("json-field", [doc, text("titel")]),
    );
    assert!(message.contains("titel"), "field not named: {message}");
    assert!(message.contains("stars"), "keys not listed: {message}");
    assert!(message.contains("title"), "keys not listed: {message}");
}

#[test]
fn pluck_maps_a_field_across_an_array() {
    let (store, reg) = brain();
    let docs = json(r#"[{"title":"a","n":1},{"title":"b","n":2},{"title":"c","n":3}]"#);
    let titles = value(
        &store,
        &reg,
        &Concept::call("json-pluck", [docs.clone(), text("title")]),
    );
    assert_eq!(items(&titles), vec![text("a"), text("b"), text("c")]);

    // Order follows the document, not the sort the store queries use.
    let ns = value(
        &store,
        &reg,
        &Concept::call("json-pluck", [docs, text("n")]),
    );
    assert_eq!(
        items(&ns),
        vec![Concept::int(1), Concept::int(2), Concept::int(3)]
    );
}

#[test]
fn a_plucked_number_is_a_real_integer_that_arithmetic_accepts() {
    // This is the whole reason the conversion boundary exists.
    let (store, reg) = brain();
    let docs = json(r#"[{"n":41}]"#);
    let sum = value(
        &store,
        &reg,
        &Concept::call(
            "math-add",
            [
                Concept::call(
                    "json-field",
                    [
                        Concept::call("json-parse", [text(r#"{"n":41}"#)]),
                        text("n"),
                    ],
                ),
                Concept::int(1),
            ],
        ),
    );
    assert_eq!(sum, Concept::int(42));

    let plucked = value(
        &store,
        &reg,
        &Concept::call("json-pluck", [docs, text("n")]),
    );
    assert_eq!(items(&plucked), vec![Concept::int(41)]);
}

#[test]
fn pluck_refuses_an_array_of_things_that_are_not_objects() {
    let (store, reg) = brain();
    let message = failure(
        &store,
        &reg,
        &Concept::call("json-pluck", [json("[1, 2, 3]"), text("n")]),
    );
    assert!(message.contains("objects"), "unhelpful: {message}");
}

#[test]
fn has_field_json_keys_length_and_type() {
    let (store, reg) = brain();
    let doc = json(r#"{"title":"Spoon","stars":3}"#);

    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("json-has-field", [doc.clone(), text("title")])
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("json-has-field", [doc.clone(), text("nope")])
        ),
        Concept::bool(false)
    );

    let keys = value(&store, &reg, &Concept::call("json-keys", [doc.clone()]));
    assert_eq!(items(&keys), vec![text("stars"), text("title")]);

    assert_eq!(
        value(&store, &reg, &Concept::call("json-length", [doc.clone()])),
        Concept::int(2)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("json-length", [json("[1,2,3,4]")])
        ),
        Concept::int(4)
    );

    for (raw, name) in [
        ("null", "null"),
        ("true", "bool"),
        ("3", "number"),
        (r#""x""#, "string"),
        ("[]", "array"),
        ("{}", "object"),
    ] {
        assert_eq!(
            value(&store, &reg, &Concept::call("json-type", [json(raw)])),
            text(name),
            "wrong type name for {raw}"
        );
    }
}

#[test]
fn to_json_renders_ground_values_and_lists() {
    let (store, reg) = brain();
    let render = |c: Concept| value(&store, &reg, &Concept::call("json-to-json", [c]));

    assert_eq!(render(Concept::int(42)), text("42"));
    assert_eq!(render(text("hi")), text(r#""hi""#));
    assert_eq!(render(Concept::bool(true)), text("true"));
    assert_eq!(
        render(Concept::datetime(pinned())),
        text(r#""2026-09-03T12:00:00+00:00""#)
    );
    assert_eq!(
        render(Concept::call(
            "list-of",
            [Concept::int(1), text("two"), Concept::bool(false)]
        )),
        text(r#"[1,"two",false]"#)
    );

    // Round trip through the parser.
    let doc = r#"{"a":[1,2],"b":"c"}"#;
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("json-to-json", [Concept::call("json-parse", [text(doc)])])
        ),
        text(doc)
    );
}

#[test]
fn to_json_refuses_a_named_concept() {
    // Greg's meaning lives in the store, not in his spelling, so a JSON string
    // could not be read back into the same concept.
    let (store, reg) = brain();
    let message = failure(
        &store,
        &reg,
        &Concept::call("json-to-json", [Concept::named("greg")]),
    );
    assert!(message.contains("ground value"), "unhelpful: {message}");
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

#[test]
fn now_reads_the_injected_clock_and_not_the_system_one() {
    let (store, reg) = brain();
    let observed = value(&store, &reg, &Concept::call("time-now", []));
    assert_eq!(observed, Concept::datetime(pinned()));

    // A second evaluator on a different pinned clock sees that one, which no
    // reading of the system clock could produce.
    let other = Utc.with_ymd_and_hms(1999, 12, 31, 23, 59, 59).unwrap();
    let outcome = Evaluator::new(&store, &reg)
        .with_budget(Budget::deterministic())
        .with_now(other)
        .evaluate(&Concept::call("time-now", []));
    assert_eq!(outcome.value(), Some(&Concept::datetime(other)));
}

#[test]
fn timestamp_counts_from_the_epoch_in_the_unit_asked_for() {
    let (store, reg) = brain();
    let at = Concept::datetime(pinned());

    assert_eq!(
        value(&store, &reg, &Concept::call("time-timestamp", [at.clone()])),
        Concept::int(PINNED_EPOCH_SECONDS)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("time-timestamp", [at.clone(), text("milliseconds")])
        ),
        Concept::int(PINNED_EPOCH_SECONDS * 1_000)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("time-timestamp", [at, text("days")])
        ),
        Concept::int(PINNED_EPOCH_SECONDS / 86_400)
    );
}

#[test]
fn format_and_parse_round_trip() {
    let (store, reg) = brain();
    let at = Concept::datetime(pinned());

    let rfc = value(&store, &reg, &Concept::call("time-format", [at.clone()]));
    assert_eq!(rfc, text("2026-09-03T12:00:00+00:00"));
    assert_eq!(
        value(&store, &reg, &Concept::call("time-parse", [rfc])),
        at.clone()
    );

    let pattern = text("%Y-%m-%d %H:%M:%S");
    let formatted = value(
        &store,
        &reg,
        &Concept::call("time-format", [at.clone(), pattern.clone()]),
    );
    assert_eq!(formatted, text("2026-09-03 12:00:00"));
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("time-parse", [formatted, pattern])
        ),
        at
    );
}

#[test]
fn a_pattern_with_an_offset_is_honoured_when_parsing() {
    let (store, reg) = brain();
    let parsed = value(
        &store,
        &reg,
        &Concept::call(
            "time-parse",
            [
                text("2026-09-03 07:00:00 -0500"),
                text("%Y-%m-%d %H:%M:%S %z"),
            ],
        ),
    );
    assert_eq!(parsed, Concept::datetime(pinned()));
}

#[test]
fn unparseable_time_text_fails_cleanly() {
    let (store, reg) = brain();
    let message = failure(
        &store,
        &reg,
        &Concept::call("time-parse", [text("last tuesday")]),
    );
    assert!(message.contains("RFC 3339"), "unhelpful: {message}");

    let bad_pattern = failure(
        &store,
        &reg,
        &Concept::call("time-format", [Concept::datetime(pinned()), text("%Q")]),
    );
    assert!(bad_pattern.contains("strftime"), "unhelpful: {bad_pattern}");
}

#[test]
fn duration_arithmetic_and_comparison_round_trip() {
    let (store, reg) = brain();
    let at = Concept::datetime(pinned());

    let later = value(
        &store,
        &reg,
        &Concept::call(
            "time-add-duration",
            [at.clone(), Concept::int(3), text("days")],
        ),
    );
    assert_eq!(
        later,
        Concept::datetime(Utc.with_ymd_and_hms(2026, 9, 6, 12, 0, 0).unwrap())
    );

    // Back where we started.
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "time-add-duration",
                [later.clone(), Concept::int(-3), text("days")]
            )
        ),
        at
    );

    // A bare amount is seconds.
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("time-add-duration", [at.clone(), Concept::int(90)])
        ),
        Concept::datetime(Utc.with_ymd_and_hms(2026, 9, 3, 12, 1, 30).unwrap())
    );

    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("time-diff", [later.clone(), at.clone(), text("days")])
        ),
        Concept::int(3)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("time-diff", [at.clone(), later.clone()])
        ),
        Concept::int(-3 * 86_400)
    );

    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("time-before", [at.clone(), later.clone()])
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("time-after", [at.clone(), later.clone()])
        ),
        Concept::bool(false)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("time-before", [at.clone(), at.clone()])
        ),
        Concept::bool(false),
        "an instant is not before itself"
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("time-after", [at.clone(), at])),
        Concept::bool(false),
        "an instant is not after itself"
    );
}

#[test]
fn a_duration_that_runs_off_the_calendar_is_an_error() {
    let (store, reg) = brain();
    let message = failure(
        &store,
        &reg,
        &Concept::call(
            "time-add-duration",
            [
                Concept::datetime(pinned()),
                Concept::int(i64::MAX),
                text("weeks"),
            ],
        ),
    );
    assert!(message.contains("out of range"), "unhelpful: {message}");
}

#[test]
fn an_unknown_unit_lists_the_ones_that_work() {
    let (store, reg) = brain();
    let message = failure(
        &store,
        &reg,
        &Concept::call(
            "time-add-duration",
            [
                Concept::datetime(pinned()),
                Concept::int(1),
                text("fortnights"),
            ],
        ),
    );
    assert!(message.contains("fortnights"), "unhelpful: {message}");
    assert!(
        message.contains("weeks"),
        "no alternatives offered: {message}"
    );
}

// ---------------------------------------------------------------------------
// Nothing panics
// ---------------------------------------------------------------------------

#[test]
fn every_native_refuses_a_wrong_typed_argument_instead_of_panicking() {
    let (store, reg) = brain();
    let doc = json(r#"{"a":1}"#);
    let at = Concept::datetime(pinned());
    let wrong = Concept::named("not-what-you-wanted");

    let cases: Vec<Concept> = vec![
        // Store: the concept argument is data and takes any shape, so the
        // wrong type shows up in the limit and the head name.
        // exists answers for any concept, a hole included, so the only way to
        // misuse it is arity.
        Concept::call("store-exists", [Concept::hole(0), Concept::hole(1)]),
        Concept::call("store-assert", [Concept::hole(0)]),
        Concept::call("store-retract-claim", [Concept::hole(0), Concept::hole(1)]),
        Concept::call("store-recall", [Concept::hole(0)]),
        Concept::call("store-recall", [owns("greg", "dog"), text("lots")]),
        Concept::call("store-recall-about", [Concept::named("greg"), text("lots")]),
        Concept::call("store-recall-by-head", [Concept::int(7)]),
        Concept::call("store-surface-of", [Concept::int(7)]),
        Concept::call("store-count-recalled", [Concept::hole(0)]),
        // describe takes any concept and answers for all of them, so the only
        // way to misuse it is arity, which the evaluator checks.
        Concept::call(
            "store-describe",
            [Concept::named("greg"), Concept::named("greg")],
        ),
        // JSON
        Concept::call("json-parse", [Concept::int(1)]),
        Concept::call("json-to-json", [wrong.clone()]),
        Concept::call("json-field", [Concept::int(1), text("a")]),
        Concept::call("json-field", [doc.clone(), Concept::int(1)]),
        Concept::call("json-pluck", [doc.clone(), text("a")]),
        Concept::call("json-has-field", [Concept::int(1), text("a")]),
        Concept::call("json-keys", [json("[1]")]),
        Concept::call("json-length", [json("3")]),
        Concept::call("json-type", [text("not json")]),
        // Time
        Concept::call("time-timestamp", [wrong.clone()]),
        Concept::call("time-timestamp", [at.clone(), Concept::int(1)]),
        Concept::call("time-format", [wrong.clone()]),
        Concept::call("time-parse", [Concept::int(1)]),
        Concept::call("time-add-duration", [at.clone(), text("three")]),
        Concept::call("time-add-duration", [wrong.clone(), Concept::int(1)]),
        Concept::call("time-diff", [at.clone(), wrong.clone()]),
        Concept::call("time-before", [at.clone(), wrong.clone()]),
        Concept::call("time-after", [at, wrong]),
    ];

    for case in cases {
        let outcome = run(&store, &reg, &case);
        assert!(
            !outcome.is_value(),
            "{case:?} should have been refused, got {outcome:?}"
        );
    }
}

#[test]
fn now_takes_no_arguments() {
    let (store, reg) = brain();
    let outcome = run(&store, &reg, &Concept::call("time-now", [Concept::int(1)]));
    assert!(!outcome.is_value(), "now accepted an argument: {outcome:?}");
}

#[test]
fn ground_text_still_reaches_the_natives_that_want_it() {
    // Guards against a helper that accidentally accepts a named concept whose
    // symbol happens to spell the same thing.
    let (store, reg) = brain();
    let ground = Concept::ground(Ground::text("owns"));
    seed_facts(&store, &reg);
    let found = value(
        &store,
        &reg,
        &Concept::call("store-recall-by-head", [ground]),
    );
    assert_eq!(items(&found).len(), 4);
}
