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
    Concept::call("list-of", values.into_iter().map(Concept::text))
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
            &Concept::call("text-concat", [Concept::text("spo"), Concept::text("on")])
        ),
        Concept::text("spoon")
    );
    assert_eq!(
        value(&store, &reg, &Concept::call("text-concat", [])),
        Concept::text("")
    );
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("text-concat", [Concept::text("n = "), Concept::int(42)]),
    );
    assert!(message.contains("text-concat"), "got {message}");
}

#[test]
fn split_produces_a_list_and_join_consumes_one() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-split", [Concept::text("a,b,c"), Concept::text(",")])
        ),
        texts(["a", "b", "c"])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-split", [Concept::text(""), Concept::text(",")])
        ),
        texts([""])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-join", [texts(["a", "b"]), Concept::text("-")])
        ),
        Concept::text("a-b")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-join", [texts([]), Concept::text("-")])
        ),
        Concept::text("")
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call("text-split", [Concept::text("abc"), Concept::text("")]),
    );
    assert!(message.contains("empty separator"), "got {message}");
}

#[test]
fn split_and_join_round_trip_through_a_real_pipeline() {
    let (store, reg) = brain();
    let expr = Concept::call(
        "text-join",
        [
            Concept::call(
                "list-map",
                [
                    Concept::call("text-split", [Concept::text("a,b,c"), Concept::text(",")]),
                    Concept::named("text-upper"),
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
            &Concept::call("text-lines", [Concept::text("one\ntwo\r\nthree")])
        ),
        texts(["one", "two", "three"])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-lines", [Concept::text("one\n")])
        ),
        texts(["one"]),
        "a trailing newline should not invent an empty last line"
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-lines", [Concept::text("")])
        ),
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
            &Concept::call("text-upper", [Concept::text("spoon")])
        ),
        Concept::text("SPOON")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-lower", [Concept::text("SPOON")])
        ),
        Concept::text("spoon")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-trim", [Concept::text("  hi \n")])
        ),
        Concept::text("hi")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "text-replace",
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
            "text-replace",
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
            &Concept::call("text-upper", [Concept::text("straße")])
        ),
        Concept::text("STRASSE"),
        "case mapping is not a per-character substitution"
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-lower", [Concept::text("ÄÖÜ Ñ")])
        ),
        Concept::text("äöü ñ")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-upper", [Concept::text("çğışü")])
        ),
        Concept::text("ÇĞIŞÜ")
    );
}

#[test]
fn predicates_over_text() {
    let (store, reg) = brain();
    let hay = Concept::text("spoonful");
    for (native, needle, expected) in [
        ("text-starts-with", "spoon", true),
        ("text-starts-with", "full", false),
        ("text-ends-with", "ful", true),
        ("text-ends-with", "spoon", false),
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
            &Concept::call(
                "text-substring",
                [s.clone(), Concept::int(0), Concept::int(2)]
            )
        ),
        Concept::text("hé"),
        "a byte-indexed slice here would have cut the é in half"
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "text-substring",
                [s.clone(), Concept::int(5), Concept::int(6)]
            )
        ),
        Concept::text("🌍")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "text-substring",
                [s.clone(), Concept::int(6), Concept::int(6)]
            )
        ),
        Concept::text(""),
        "an empty range at the very end is empty, not an error"
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call(
            "text-substring",
            [s.clone(), Concept::int(0), Concept::int(7)],
        ),
    );
    assert!(
        message.contains("index 7") && message.contains("length 6"),
        "the message should name the index and the length, got {message}"
    );
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("text-substring", [s, Concept::int(4), Concept::int(2)]),
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
            &Concept::call("text-char-at", [s.clone(), Concept::int(1)])
        ),
        Concept::text("é")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-char-at", [s.clone(), Concept::int(5)])
        ),
        Concept::text("🌍")
    );
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("text-char-at", [s, Concept::int(6)]),
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
            &Concept::call("text-pad-left", [Concept::text("7"), Concept::int(3)])
        ),
        Concept::text("  7")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "text-pad-left",
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
                "text-pad-right",
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
                "text-pad-left",
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
            &Concept::call("text-pad-right", [Concept::text("long"), Concept::int(2)])
        ),
        Concept::text("long"),
        "already wide enough means unchanged, never truncated"
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call(
            "text-pad-left",
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
            &Concept::call("text-repeat", [Concept::text("ab"), Concept::int(3)])
        ),
        Concept::text("ababab")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-repeat", [Concept::text("ab"), Concept::int(0)])
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
        &Concept::call(
            "text-repeat",
            [Concept::text("ab"), Concept::int(1_000_000_000)],
        ),
    );
    assert!(message.contains("cap"), "got {message}");

    // The evaluator's node budget cannot catch this on its own: the result is
    // one concept either way, however many characters are inside it.
    let message = why_failed(
        &store,
        &reg,
        &Concept::call("text-repeat", [Concept::text("ab"), Concept::int(i64::MAX)]),
    );
    assert!(
        message.contains("cap") || message.contains("overflow"),
        "got {message}"
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call("text-repeat", [Concept::text("ab"), Concept::int(-1)]),
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
            value(
                &store,
                &reg,
                &Concept::call("text-to-text", [input.clone()])
            ),
            Concept::text(expected),
            "to-text of {input:?}"
        );
    }
    let rendered = value(
        &store,
        &reg,
        &Concept::call("text-to-text", [Concept::datetime(now())]),
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
        let message = why_failed(
            &store,
            &reg,
            &Concept::call("text-to-text", [input.clone()]),
        );
        assert!(
            message.contains("text-to-text"),
            "got {message} for {input:?}"
        );
    }
}

#[test]
fn parsing_numbers_reports_failure_rather_than_defaulting() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-parse-int", [Concept::text("42")])
        ),
        Concept::int(42)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-parse-int", [Concept::text("  -7 ")])
        ),
        Concept::int(-7),
        "surrounding whitespace carries no meaning"
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-parse-float", [Concept::text("1.5")])
        ),
        Concept::float(1.5)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-parse-float", [Concept::text("3")])
        ),
        Concept::float(3.0)
    );

    for (native, input) in [
        ("text-parse-int", "abc"),
        ("text-parse-int", ""),
        ("text-parse-int", "1.5"),
        ("text-parse-float", "abc"),
        ("text-parse-float", ""),
    ] {
        let message = why_failed(&store, &reg, &Concept::call(native, [Concept::text(input)]));
        assert!(message.contains(native), "got {message}");
    }
}

// ---------------------------------------------------------------------------
// Regular expressions
// ---------------------------------------------------------------------------

#[test]
fn regex_match_tells_whether_a_pattern_appears_anywhere() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "text-regex-match",
                [Concept::text("hello123"), Concept::text(r"\d+")]
            )
        ),
        Concept::bool(true)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "text-regex-match",
                [Concept::text("hello"), Concept::text(r"^\d+$")]
            )
        ),
        Concept::bool(false)
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call(
            "text-regex-match",
            [Concept::text("hello"), Concept::text("(unclosed")],
        ),
    );
    assert!(message.contains("text-regex-match"), "got {message}");
}

#[test]
fn regex_replace_swaps_every_match() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "text-regex-replace",
                [
                    Concept::text("a1 b22 c333"),
                    Concept::text(r"\d+"),
                    Concept::text("#")
                ]
            )
        ),
        Concept::text("a# b# c#")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "text-regex-replace",
                [
                    Concept::text("no digits here"),
                    Concept::text(r"\d+"),
                    Concept::text("#")
                ]
            )
        ),
        Concept::text("no digits here")
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call(
            "text-regex-replace",
            [
                Concept::text("hello"),
                Concept::text("["),
                Concept::text("x"),
            ],
        ),
    );
    assert!(message.contains("text-regex-replace"), "got {message}");
}

// ---------------------------------------------------------------------------
// Characters and code points
// ---------------------------------------------------------------------------

#[test]
fn char_code_and_from_char_code_round_trip() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-char-code", [Concept::text("A")])
        ),
        Concept::int(65)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-char-code", [Concept::text("🌍")])
        ),
        Concept::int(0x1F30D)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-from-char-code", [Concept::int(65)])
        ),
        Concept::text("A")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-from-char-code", [Concept::int(0x1F30D)])
        ),
        Concept::text("🌍")
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call("text-char-code", [Concept::text("")]),
    );
    assert!(message.contains("text-char-code"), "got {message}");

    let message = why_failed(
        &store,
        &reg,
        &Concept::call("text-from-char-code", [Concept::int(0xD800)]),
    );
    assert!(message.contains("text-from-char-code"), "got {message}");

    let message = why_failed(
        &store,
        &reg,
        &Concept::call("text-from-char-code", [Concept::int(-1)]),
    );
    assert!(message.contains("text-from-char-code"), "got {message}");
}

// ---------------------------------------------------------------------------
// Word case
// ---------------------------------------------------------------------------

#[test]
fn capitalize_changes_only_the_first_letter() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-capitalize", [Concept::text("hello world")])
        ),
        Concept::text("Hello world")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-capitalize", [Concept::text("HELLO")])
        ),
        Concept::text("HELLO")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-capitalize", [Concept::text("")])
        ),
        Concept::text("")
    );
}

#[test]
fn title_case_capitalizes_every_word() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-title-case", [Concept::text("hello world  again")])
        ),
        Concept::text("Hello World  Again"),
        "internal spacing is preserved"
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-title-case", [Concept::text("")])
        ),
        Concept::text("")
    );
}

#[test]
fn camel_kebab_and_snake_case_all_read_the_same_words() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-camel-case", [Concept::text("hello world")])
        ),
        Concept::text("helloWorld")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-kebab-case", [Concept::text("helloWorld")])
        ),
        Concept::text("hello-world")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-snake-case", [Concept::text("helloWorld")])
        ),
        Concept::text("hello_world")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-kebab-case", [Concept::text("hello_world again")])
        ),
        Concept::text("hello-world-again"),
        "underscores and spaces are both word boundaries"
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-camel-case", [Concept::text("")])
        ),
        Concept::text("")
    );
}

// ---------------------------------------------------------------------------
// Words, counting, and layout
// ---------------------------------------------------------------------------

#[test]
fn words_splits_on_whitespace_and_drops_empty_pieces() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-words", [Concept::text("  hello   world  ")])
        ),
        texts(["hello", "world"])
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-words", [Concept::text("")])
        ),
        texts([])
    );
}

#[test]
fn count_occurrences_counts_non_overlapping_matches() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "text-count-occurrences",
                [Concept::text("banana"), Concept::text("ana")]
            )
        ),
        Concept::int(1),
        "non-overlapping: consuming the first \"ana\" leaves no room for a second"
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "text-count-occurrences",
                [Concept::text("aaaa"), Concept::text("aa")]
            )
        ),
        Concept::int(2)
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "text-count-occurrences",
                [Concept::text("no match"), Concept::text("xyz")]
            )
        ),
        Concept::int(0)
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call(
            "text-count-occurrences",
            [Concept::text("abc"), Concept::text("")],
        ),
    );
    assert!(message.contains("empty search string"), "got {message}");
}

#[test]
fn center_pads_both_sides_favoring_the_right() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-center", [Concept::text("hi"), Concept::int(6)])
        ),
        Concept::text("  hi  ")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-center", [Concept::text("hi"), Concept::int(5)])
        ),
        Concept::text(" hi  "),
        "odd padding favors the right"
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call(
                "text-center",
                [Concept::text("hi"), Concept::int(6), Concept::text("*")]
            )
        ),
        Concept::text("**hi**")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-center", [Concept::text("longer"), Concept::int(3)])
        ),
        Concept::text("longer"),
        "already wide enough means unchanged"
    );

    let message = why_failed(
        &store,
        &reg,
        &Concept::call(
            "text-center",
            [Concept::text("x"), Concept::int(4), Concept::text("ab")],
        ),
    );
    assert!(message.contains("exactly one character"), "got {message}");
}

#[test]
fn text_reverse_reverses_by_character() {
    let (store, reg) = brain();
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-reverse", [Concept::text("spoon")])
        ),
        Concept::text("noops")
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-reverse", [Concept::text("🌍🌱")])
        ),
        Concept::text("🌱🌍"),
        "reversed by character, not by byte"
    );
    assert_eq!(
        value(
            &store,
            &reg,
            &Concept::call("text-reverse", [Concept::text("")])
        ),
        Concept::text("")
    );
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
        Concept::call("text-concat", [n.clone()]),
        Concept::call("text-split", [n.clone(), t.clone()]),
        Concept::call("text-split", [t.clone(), n.clone()]),
        Concept::call("text-join", [n.clone(), t.clone()]),
        Concept::call(
            "text-join",
            [Concept::call("list-of", [n.clone()]), t.clone()],
        ),
        Concept::call("text-join", [Concept::call("list-of", []), n.clone()]),
        Concept::call("text-lines", [n.clone()]),
        Concept::call("text-upper", [n.clone()]),
        Concept::call("text-lower", [n.clone()]),
        Concept::call("text-trim", [n.clone()]),
        Concept::call("text-replace", [n.clone(), t.clone(), t.clone()]),
        Concept::call("text-length", [n.clone()]),
        Concept::call("text-starts-with", [n.clone(), t.clone()]),
        Concept::call("text-ends-with", [t.clone(), n.clone()]),
        Concept::call("text-contains", [n.clone(), t.clone()]),
        Concept::call("text-substring", [n.clone(), n.clone(), n.clone()]),
        Concept::call("text-substring", [t.clone(), t.clone(), n.clone()]),
        Concept::call("text-substring", [t.clone(), Concept::int(-1), n.clone()]),
        Concept::call("text-char-at", [n.clone(), n.clone()]),
        Concept::call("text-char-at", [t.clone(), t.clone()]),
        Concept::call("text-pad-left", [n.clone(), n.clone()]),
        Concept::call("text-pad-right", [t.clone(), t.clone()]),
        Concept::call("text-pad-right", [t.clone(), Concept::int(9_000_000)]),
        Concept::call("text-repeat", [n.clone(), n.clone()]),
        Concept::call("text-repeat", [t.clone(), t.clone()]),
        Concept::call("text-to-text", [Concept::hole(0)]),
        Concept::call("text-parse-int", [n.clone()]),
        Concept::call("text-parse-float", [n.clone()]),
        Concept::call("text-regex-match", [n.clone(), t.clone()]),
        Concept::call("text-regex-match", [t.clone(), n.clone()]),
        Concept::call("text-regex-replace", [n.clone(), t.clone(), t.clone()]),
        Concept::call("text-char-code", [n.clone()]),
        Concept::call("text-from-char-code", [t.clone()]),
        Concept::call("text-capitalize", [n.clone()]),
        Concept::call("text-title-case", [n.clone()]),
        Concept::call("text-camel-case", [n.clone()]),
        Concept::call("text-kebab-case", [n.clone()]),
        Concept::call("text-snake-case", [n.clone()]),
        Concept::call("text-words", [n.clone()]),
        Concept::call("text-count-occurrences", [n.clone(), t.clone()]),
        Concept::call("text-center", [n.clone(), n.clone()]),
        Concept::call("text-center", [t.clone(), t.clone()]),
        Concept::call("text-reverse", [n.clone()]),
        // Arity mismatches are caught before the native ever runs.
        Concept::call("text-upper", []),
        Concept::call("text-substring", [t.clone(), n.clone()]),
        Concept::call("text-pad-left", [t.clone()]),
        Concept::call(
            "text-pad-left",
            [t.clone(), n.clone(), t.clone(), t.clone()],
        ),
    ];

    for expr in bad {
        let (outcome, _) = run(&store, &reg, &expr);
        assert!(
            !outcome.is_value(),
            "{expr:?} should not have produced a value: {outcome:?}"
        );
    }
}
