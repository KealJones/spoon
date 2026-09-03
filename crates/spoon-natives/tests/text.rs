//! Text natives, end to end through a real evaluator and store.

use chrono::{TimeZone, Utc};
use spoon_concept::{Concept, Ground};
use spoon_eval::{Budget, Evaluator, NativeRegistry, Outcome};
use spoon_store::Store;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap()
}

fn brain() -> (Store, NativeRegistry) {
    let store = Store::open_in_memory().expect("in-memory store");
    let registry = spoon_natives::bootstrap();
    spoon_natives::seed_bootstrap(&store, &registry).expect("seed");
    (store, registry)
}

fn run(store: &Store, reg: &NativeRegistry, c: &Concept) -> (Outcome, Vec<String>) {
    let mut ev = Evaluator::new(store, reg)
        .with_budget(Budget::deterministic())
        .with_now(now());
    let outcome = ev.evaluate(c);
    let failures = ev
        .trace()
        .failures()
        .iter()
        .map(|(_, _, message)| (*message).to_string())
        .collect();
    (outcome, failures)
}

fn value(store: &Store, reg: &NativeRegistry, c: &Concept) -> Concept {
    let (outcome, failures) = run(store, reg, c);
    match outcome.value() {
        Some(v) => v.clone(),
        None => panic!("expected a value, got {outcome:?} with failures {failures:?}"),
    }
}

/// The call must not produce a value; the trace explains why.
fn why_failed(store: &Store, reg: &NativeRegistry, c: &Concept) -> String {
    let (outcome, failures) = run(store, reg, c);
    assert!(!outcome.is_value(), "expected a failure, got {outcome:?}");
    failures.join(" | ")
}

fn texts(values: impl IntoIterator<Item = &'static str>) -> Concept {
    Concept::call("list", values.into_iter().map(Concept::text))
}

// ---------------------------------------------------------------------------
// Building and taking apart
// ---------------------------------------------------------------------------

#[test]
fn concat_joins_text_and_refuses_anything_else() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("concat", [Concept::text("spo"), Concept::text("on")])
        ),
        Concept::text("spoon")
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("concat", [])),
        Concept::text("")
    );
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("concat", [Concept::text("n = "), Concept::int(42)]),
    );
    assert!(message.contains("concat"), "got {message}");
}

#[test]
fn split_produces_a_list_and_join_consumes_one() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("split", [Concept::text("a,b,c"), Concept::text(",")])
        ),
        texts(["a", "b", "c"])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("split", [Concept::text(""), Concept::text(",")])
        ),
        texts([""])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("join", [texts(["a", "b"]), Concept::text("-")])
        ),
        Concept::text("a-b")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("join", [texts([]), Concept::text("-")])
        ),
        Concept::text("")
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call("split", [Concept::text("abc"), Concept::text("")]),
    );
    assert!(message.contains("empty separator"), "got {message}");
}

#[test]
fn split_and_join_round_trip_through_a_real_pipeline() {
    let (store, reg) = brain();
    let expr = Concept::call(
        "join",
        [
            Concept::call(
                "map",
                [
                    Concept::call("split", [Concept::text("a,b,c"), Concept::text(",")]),
                    Concept::named("upper"),
                ],
            ),
            Concept::text("-"),
        ],
    );
    assert_eq!(value(&store, &reg, &expr), Concept::text("A-B-C"));
}

#[test]
fn lines_splits_on_newlines_and_tolerates_carriage_returns() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("lines", [Concept::text("one\ntwo\r\nthree")])
        ),
        texts(["one", "two", "three"])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("lines", [Concept::text("one\n")])
        ),
        texts(["one"]),
        "a trailing newline should not invent an empty last line"
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("lines", [Concept::text("")])),
        texts([])
    );
}

// ---------------------------------------------------------------------------
// Shape
// ---------------------------------------------------------------------------

#[test]
fn case_trim_and_replace() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("upper", [Concept::text("spoon")])
        ),
        Concept::text("SPOON")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("lower", [Concept::text("SPOON")])
        ),
        Concept::text("spoon")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("trim", [Concept::text("  hi \n")])
        ),
        Concept::text("hi")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "replace",
                [
                    Concept::text("a-b-a"),
                    Concept::text("a"),
                    Concept::text("z")
                ]
            )
        ),
        Concept::text("z-b-z")
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call(
            "replace",
            [Concept::text("ab"), Concept::text(""), Concept::text("z")],
        ),
    );
    assert!(message.contains("empty search string"), "got {message}");
}

#[test]
fn upper_and_lower_handle_non_ascii() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("upper", [Concept::text("straße")])
        ),
        Concept::text("STRASSE"),
        "case mapping is not a per-character substitution"
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("lower", [Concept::text("ÄÖÜ Ñ")])
        ),
        Concept::text("äöü ñ")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("upper", [Concept::text("çğışü")])
        ),
        Concept::text("ÇĞIŞÜ")
    );
}

#[test]
fn predicates_over_text() {
    let (store, reg) = brain();
    let hay = Concept::text("spoonful");
    for (native, needle, expected) in [
        ("starts-with", "spoon", true),
        ("starts-with", "full", false),
        ("ends-with", "ful", true),
        ("ends-with", "spoon", false),
        ("text-contains", "oonf", true),
        ("text-contains", "zzz", false),
    ] {
        assert_eq!(
            value(
                &store,
                &reg,
                &Concept::call(native, [hay.clone(), Concept::text(needle)])
            ),
            Concept::bool(expected),
            "{native} {needle}"
        );
    }
}

// ---------------------------------------------------------------------------
// Unicode
// ---------------------------------------------------------------------------

#[test]
fn length_is_counted_in_characters_not_bytes() {
    let (store, reg) = brain();
    // "héllo🌍" is 6 characters and 11 bytes.
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-length", [Concept::text("héllo🌍")])
        ),
        Concept::int(6)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-length", [Concept::text("🌍")])
        ),
        Concept::int(1),
        "one emoji is one character"
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-length", [Concept::text("")])
        ),
        Concept::int(0)
    );
}

#[test]
fn substring_never_splits_a_character() {
    let (store, reg) = brain();
    let s = Concept::text("héllo🌍");
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("substring", [s.clone(), Concept::int(0), Concept::int(2)])
        ),
        Concept::text("hé"),
        "a byte-indexed slice here would have cut the é in half"
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("substring", [s.clone(), Concept::int(5), Concept::int(6)])
        ),
        Concept::text("🌍")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("substring", [s.clone(), Concept::int(6), Concept::int(6)])
        ),
        Concept::text(""),
        "an empty range at the very end is empty, not an error"
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call("substring", [s.clone(), Concept::int(0), Concept::int(7)]),
    );
    assert!(
        message.contains("index 7") && message.contains("length 6"),
        "the message should name the index and the length, got {message}"
    );
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("substring", [s, Concept::int(4), Concept::int(2)]),
    );
    assert!(message.contains("start 4 is past end 2"), "got {message}");
}

#[test]
fn char_at_returns_whole_characters() {
    let (store, reg) = brain();
    let s = Concept::text("héllo🌍");
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("char-at", [s.clone(), Concept::int(1)])
        ),
        Concept::text("é")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("char-at", [s.clone(), Concept::int(5)])
        ),
        Concept::text("🌍")
    );
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("char-at", [s, Concept::int(6)]),
    );
    assert!(
        message.contains("index 6") && message.contains("length 6"),
        "got {message}"
    );
}

#[test]
fn padding_counts_characters_too() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("pad-left", [Concept::text("7"), Concept::int(3)])
        ),
        Concept::text("  7")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "pad-left",
                [Concept::text("7"), Concept::int(3), Concept::text("0")]
            )
        ),
        Concept::text("007")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "pad-right",
                [Concept::text("7"), Concept::int(3), Concept::text("·")]
            )
        ),
        Concept::text("7··")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "pad-left",
                [Concept::text("🌍"), Concept::int(3), Concept::text("🌱")]
            )
        ),
        Concept::text("🌱🌱🌍"),
        "width is in characters, so two pads make three"
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("pad-right", [Concept::text("long"), Concept::int(2)])
        ),
        Concept::text("long"),
        "already wide enough means unchanged, never truncated"
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call(
            "pad-left",
            [Concept::text("x"), Concept::int(4), Concept::text("ab")],
        ),
    );
    assert!(message.contains("exactly one character"), "got {message}");
}

// ---------------------------------------------------------------------------
// Repeat, and the cap that keeps it from eating the process
// ---------------------------------------------------------------------------

#[test]
fn repeat_multiplies_a_string() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("repeat", [Concept::text("ab"), Concept::int(3)])
        ),
        Concept::text("ababab")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("repeat", [Concept::text("ab"), Concept::int(0)])
        ),
        Concept::text("")
    );
}

#[test]
fn repeat_past_the_cap_errors_instead_of_allocating() {
    let (store, reg) = brain();
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("repeat", [Concept::text("ab"), Concept::int(1_000_000_000)]),
    );
    assert!(message.contains("cap"), "got {message}");

    // The evaluator's node budget cannot catch this on its own: the result is
    // one concept either way, however many characters are inside it.
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("repeat", [Concept::text("ab"), Concept::int(i64::MAX)]),
    );
    assert!(
        message.contains("cap") || message.contains("overflow"),
        "got {message}"
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call("repeat", [Concept::text("ab"), Concept::int(-1)]),
    );
    assert!(message.contains("negative count -1"), "got {message}");
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

#[test]
fn to_text_renders_every_ground_kind() {
    let (store, reg) = brain();
    let cases = [
        (Concept::bool(true), "true"),
        (Concept::int(-42), "-42"),
        (Concept::float(1.5), "1.5"),
        (Concept::text("already"), "already"),
        (Concept::bytes([0xde, 0xad]), "dead"),
        (Concept::json(serde_json::json!({"a": 1})), "{\"a\":1}"),
    ];
    for (input, expected) in cases {
        assert_eq!(
            value(&store, &reg, &Concept::call("to-text", [input.clone()])),
            Concept::text(expected),
            "to-text of {input:?}"
        );
    }
    let rendered = value(
        &store,
        &reg,
        &Concept::call("to-text", [Concept::datetime(now())]),
    );
    assert_eq!(
        rendered.as_ground().and_then(Ground::as_str),
        Some("2026-09-03T12:00:00+00:00")
    );
}

#[test]
fn to_text_refuses_a_concept_with_no_single_text_form() {
    let (store, reg) = brain();
    for input in [
        Concept::named("greg"),
        Concept::call(
            "employment",
            [Concept::named("greg"), Concept::named("workiva")],
        ),
    ] {
        let message = why_failed(&store, &reg, &Concept::call("to-text", [input.clone()]));
        assert!(message.contains("to-text"), "got {message} for {input:?}");
    }
}

#[test]
fn parsing_numbers_reports_failure_rather_than_defaulting() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("parse-int", [Concept::text("42")])
        ),
        Concept::int(42)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("parse-int", [Concept::text("  -7 ")])
        ),
        Concept::int(-7),
        "surrounding whitespace carries no meaning"
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("parse-float", [Concept::text("1.5")])
        ),
        Concept::float(1.5)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("parse-float", [Concept::text("3")])
        ),
        Concept::float(3.0)
    );

    for (native, input) in [
        ("parse-int", "abc"),
        ("parse-int", ""),
        ("parse-int", "1.5"),
        ("parse-float", "abc"),
        ("parse-float", ""),
    ] {
        let message = why_failed(&store, &reg, &Concept::call(native, [Concept::text(input)]));
        assert!(message.contains(native), "got {message}");
    }
}

// ---------------------------------------------------------------------------
// Bad input never panics
// ---------------------------------------------------------------------------

#[test]
fn every_native_refuses_a_wrong_typed_argument_instead_of_panicking() {
    let (store, reg) = brain();
    let n = Concept::int(7);
    let t = Concept::text("abc");

    let bad: Vec<Concept> = vec![
        Concept::call("concat", [n.clone()]),
        Concept::call("split", [n.clone(), t.clone()]),
        Concept::call("split", [t.clone(), n.clone()]),
        Concept::call("join", [n.clone(), t.clone()]),
        Concept::call("join", [Concept::call("list", [n.clone()]), t.clone()]),
        Concept::call("join", [Concept::call("list", []), n.clone()]),
        Concept::call("lines", [n.clone()]),
        Concept::call("upper", [n.clone()]),
        Concept::call("lower", [n.clone()]),
        Concept::call("trim", [n.clone()]),
        Concept::call("replace", [n.clone(), t.clone(), t.clone()]),
        Concept::call("text-length", [n.clone()]),
        Concept::call("starts-with", [n.clone(), t.clone()]),
        Concept::call("ends-with", [t.clone(), n.clone()]),
        Concept::call("text-contains", [n.clone(), t.clone()]),
        Concept::call("substring", [n.clone(), n.clone(), n.clone()]),
        Concept::call("substring", [t.clone(), t.clone(), n.clone()]),
        Concept::call("substring", [t.clone(), Concept::int(-1), n.clone()]),
        Concept::call("char-at", [n.clone(), n.clone()]),
        Concept::call("char-at", [t.clone(), t.clone()]),
        Concept::call("pad-left", [n.clone(), n.clone()]),
        Concept::call("pad-right", [t.clone(), t.clone()]),
        Concept::call("pad-right", [t.clone(), Concept::int(9_000_000)]),
        Concept::call("repeat", [n.clone(), n.clone()]),
        Concept::call("repeat", [t.clone(), t.clone()]),
        Concept::call("to-text", [Concept::hole(0)]),
        Concept::call("parse-int", [n.clone()]),
        Concept::call("parse-float", [n.clone()]),
        // Arity mismatches are caught before the native ever runs.
        Concept::call("upper", []),
        Concept::call("substring", [t.clone(), n.clone()]),
        Concept::call("pad-left", [t.clone()]),
        Concept::call("pad-left", [t.clone(), n.clone(), t.clone(), t.clone()]),
    ];

    for expr in bad {
        let (outcome, _) = run(&store, &reg, &expr);
        assert!(
            !outcome.is_value(),
            "{expr:?} should not have produced a value: {outcome:?}"
        );
    }
}
