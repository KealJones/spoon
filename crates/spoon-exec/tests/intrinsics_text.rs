//! Behavioural coverage for the text, regex, encoding and hashing intrinsics.
//!
//! Learned procedures compose these operations, so every claim a procedure
//! makes about text rests on what the evaluator actually does with bytes,
//! scalars and grapheme clusters. The evaluator does not use one unit
//! consistently: `Length` counts graphemes, `TextCharAt` counts scalars and
//! `TextSubstring` counts bytes. Each test below pins the unit it observed so
//! that changing one is a deliberate, visible decision rather than a silent
//! behaviour change under a procedure that already shipped.
//!
//! Hash tests use published vectors (FIPS 180-2 for SHA-256, RFC 1321 for MD5)
//! and base64 tests use the RFC 4648 vectors, so correctness is proven against
//! an external source rather than against the implementation itself.

use spoon_core::{Expr, IntrinsicOp, Value};
use spoon_exec::{Env, Evaluator, SpoonError};
use unicode_segmentation::UnicodeSegmentation;

fn intrinsic(op: IntrinsicOp, args: Vec<Expr>) -> Expr {
    Expr::Intrinsic {
        version: 1,
        op,
        args,
    }
}

fn lit_text(text: &str) -> Expr {
    Expr::Literal(Value::Text(text.to_string()))
}

fn lit_int(value: i64) -> Expr {
    Expr::Literal(Value::Int(value))
}

fn lit_map(entries: &[(&str, Value)]) -> Expr {
    Expr::Literal(Value::Map(
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect(),
    ))
}

fn eval(expr: &Expr) -> Result<Value, SpoonError> {
    Evaluator::new().eval(expr, &mut Env::new())
}

fn call(op: IntrinsicOp, args: Vec<Expr>) -> Result<Value, SpoonError> {
    eval(&intrinsic(op, args))
}

fn text_of(op: IntrinsicOp, args: Vec<Expr>) -> String {
    match call(op, args) {
        Ok(Value::Text(text)) => text,
        other => panic!("expected text, got {other:?}"),
    }
}

fn int_of(op: IntrinsicOp, args: Vec<Expr>) -> i64 {
    match call(op, args) {
        Ok(Value::Int(value)) => value,
        other => panic!("expected int, got {other:?}"),
    }
}

fn bool_of(op: IntrinsicOp, args: Vec<Expr>) -> bool {
    match call(op, args) {
        Ok(Value::Bool(value)) => value,
        other => panic!("expected bool, got {other:?}"),
    }
}

fn error_of(op: IntrinsicOp, args: Vec<Expr>) -> SpoonError {
    match call(op, args) {
        Err(error) => error,
        Ok(value) => panic!("expected an error, got {value:?}"),
    }
}

/// U+0065 U+0301: one grapheme cluster spelled with two scalars and three
/// bytes, so it separates byte, scalar and grapheme behaviour in one fixture.
const E_COMBINING: &str = "e\u{301}";

/// U+1F44D U+1F3FD: thumbs up with a medium skin tone modifier. One grapheme
/// cluster, two scalars, eight bytes, and the pair is meaningless when split.
const THUMBS_UP_TONED: &str = "\u{1F44D}\u{1F3FD}";

// -- TextCharAt --

#[test]
fn text_char_at_indexes_by_unicode_scalar_rather_than_by_byte() {
    // "héllo" is six bytes, so a byte-indexed implementation would answer "l"
    // for index 2 and would be unable to return "é" at all.
    assert_eq!(
        text_of(IntrinsicOp::TextCharAt, vec![lit_text("héllo"), lit_int(1)]),
        "é"
    );
    assert_eq!(
        text_of(IntrinsicOp::TextCharAt, vec![lit_text("héllo"), lit_int(2)]),
        "l"
    );
    assert_eq!(
        text_of(IntrinsicOp::TextCharAt, vec![lit_text("中文a"), lit_int(0)]),
        "中"
    );
    assert_eq!(
        text_of(IntrinsicOp::TextCharAt, vec![lit_text("中文a"), lit_int(2)]),
        "a"
    );
}

#[test]
fn text_char_at_splits_a_grapheme_cluster_into_its_separate_scalars() {
    // The unit is a scalar, not a user-visible character, so a procedure that
    // walks indexes 0..length(text) over emoji gets fragments rather than
    // characters. `Length` counts graphemes, which makes the mismatch worse.
    assert_eq!(
        text_of(
            IntrinsicOp::TextCharAt,
            vec![lit_text(E_COMBINING), lit_int(0)]
        ),
        "e"
    );
    assert_eq!(
        text_of(
            IntrinsicOp::TextCharAt,
            vec![lit_text(E_COMBINING), lit_int(1)]
        ),
        "\u{301}"
    );
    assert_eq!(
        text_of(
            IntrinsicOp::TextCharAt,
            vec![lit_text(THUMBS_UP_TONED), lit_int(1)]
        ),
        "\u{1F3FD}"
    );
    assert_eq!(
        int_of(IntrinsicOp::Length, vec![lit_text(THUMBS_UP_TONED)]),
        1
    );
}

#[test]
fn text_char_at_reports_an_error_for_an_index_past_the_last_scalar() {
    let error = error_of(IntrinsicOp::TextCharAt, vec![lit_text("abc"), lit_int(3)]);
    assert!(
        error.to_string().contains("index out of bounds"),
        "unexpected error: {error}"
    );

    let empty = error_of(IntrinsicOp::TextCharAt, vec![lit_text(""), lit_int(0)]);
    assert!(
        empty.to_string().contains("index out of bounds"),
        "unexpected error: {empty}"
    );
}

#[test]
fn text_char_at_rejects_a_negative_index_instead_of_wrapping_it() {
    let error = error_of(IntrinsicOp::TextCharAt, vec![lit_text("abc"), lit_int(-1)]);
    assert!(
        error.to_string().contains("must be non-negative"),
        "unexpected error: {error}"
    );
}

// -- TextCharCode and TextFromCharCode --

#[test]
fn text_char_code_returns_the_scalar_value_of_the_first_scalar_only() {
    assert_eq!(int_of(IntrinsicOp::TextCharCode, vec![lit_text("A")]), 65);
    assert_eq!(
        int_of(IntrinsicOp::TextCharCode, vec![lit_text("中")]),
        20013
    );
    assert_eq!(
        int_of(IntrinsicOp::TextCharCode, vec![lit_text("👍")]),
        128077
    );
    assert_eq!(int_of(IntrinsicOp::TextCharCode, vec![lit_text("abc")]), 97);
}

#[test]
fn text_char_code_ignores_the_rest_of_a_grapheme_cluster() {
    // Both fixtures are a single user-visible character, but the trailing
    // scalar that gives each its meaning is invisible to this operation.
    assert_eq!(
        int_of(IntrinsicOp::TextCharCode, vec![lit_text(THUMBS_UP_TONED)]),
        128077
    );
    assert_eq!(
        int_of(IntrinsicOp::TextCharCode, vec![lit_text(E_COMBINING)]),
        101
    );
}

#[test]
fn text_char_code_reports_an_error_for_empty_text() {
    let error = error_of(IntrinsicOp::TextCharCode, vec![lit_text("")]);
    assert!(
        error.to_string().contains("must not be empty"),
        "unexpected error: {error}"
    );
}

#[test]
fn text_from_char_code_builds_one_scalar_of_text() {
    assert_eq!(
        text_of(IntrinsicOp::TextFromCharCode, vec![lit_int(65)]),
        "A"
    );
    assert_eq!(
        text_of(IntrinsicOp::TextFromCharCode, vec![lit_int(20013)]),
        "中"
    );
    assert_eq!(
        text_of(IntrinsicOp::TextFromCharCode, vec![lit_int(128077)]),
        "👍"
    );
    // Zero is a valid scalar, so it must not be confused with "no character".
    assert_eq!(
        text_of(IntrinsicOp::TextFromCharCode, vec![lit_int(0)]),
        "\0"
    );
}

#[test]
fn text_from_char_code_rejects_surrogates_and_values_above_the_unicode_maximum() {
    // Surrogate halves and anything above U+10FFFF are not scalars, so
    // accepting them would produce text that cannot exist in Rust or in JSON.
    for code in [0xD800, 0xDFFF, 0x11_0000, -1, i64::MAX] {
        let error = error_of(IntrinsicOp::TextFromCharCode, vec![lit_int(code)]);
        assert!(
            error.to_string().contains("invalid code point"),
            "code {code} gave an unexpected error: {error}"
        );
    }
}

#[test]
fn text_char_code_and_text_from_char_code_round_trip_a_scalar() {
    let code = int_of(IntrinsicOp::TextCharCode, vec![lit_text("中")]);
    assert_eq!(
        text_of(IntrinsicOp::TextFromCharCode, vec![lit_int(code)]),
        "中"
    );
}

// -- TextSubstring --

#[test]
fn text_substring_slices_by_byte_offset_and_not_by_character() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextSubstring,
            vec![lit_text("hello"), lit_int(1), lit_int(3)]
        ),
        "el"
    );
    // "中文" is six bytes. A scalar-indexed implementation would answer "中文"
    // for 0..3; this one answers "中" because the offsets are byte offsets.
    assert_eq!(
        text_of(
            IntrinsicOp::TextSubstring,
            vec![lit_text("中文"), lit_int(0), lit_int(3)]
        ),
        "中"
    );
}

#[test]
fn text_substring_reports_an_error_for_an_offset_inside_a_multi_byte_scalar() {
    // The important property is that this is an error rather than a panic:
    // slicing a String on a non-boundary would abort the whole process.
    let error = error_of(
        IntrinsicOp::TextSubstring,
        vec![lit_text("中"), lit_int(0), lit_int(1)],
    );
    assert!(
        error.to_string().contains("char boundaries"),
        "unexpected error: {error}"
    );
}

#[test]
fn text_substring_returns_empty_text_when_start_is_not_before_end() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextSubstring,
            vec![lit_text("hello"), lit_int(3), lit_int(1)]
        ),
        ""
    );
    assert_eq!(
        text_of(
            IntrinsicOp::TextSubstring,
            vec![lit_text("hello"), lit_int(2), lit_int(2)]
        ),
        ""
    );
    assert_eq!(
        text_of(
            IntrinsicOp::TextSubstring,
            vec![lit_text(""), lit_int(0), lit_int(5)]
        ),
        ""
    );
}

#[test]
fn text_substring_clamps_offsets_past_the_end_instead_of_failing() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextSubstring,
            vec![lit_text("abc"), lit_int(1), lit_int(99)]
        ),
        "bc"
    );
    assert_eq!(
        text_of(
            IntrinsicOp::TextSubstring,
            vec![lit_text("abc"), lit_int(99), lit_int(100)]
        ),
        ""
    );
}

#[test]
fn text_substring_rejects_a_negative_offset() {
    let error = error_of(
        IntrinsicOp::TextSubstring,
        vec![lit_text("abc"), lit_int(-1), lit_int(2)],
    );
    assert!(
        error.to_string().contains("must be non-negative"),
        "unexpected error: {error}"
    );
}

// -- TextReverse --

#[test]
fn text_reverse_reverses_text_made_of_single_scalar_characters() {
    assert_eq!(
        text_of(IntrinsicOp::TextReverse, vec![lit_text("abc")]),
        "cba"
    );
    assert_eq!(
        text_of(IntrinsicOp::TextReverse, vec![lit_text("中文a")]),
        "a文中"
    );
    assert_eq!(text_of(IntrinsicOp::TextReverse, vec![lit_text("")]), "");
    // Precomposed "é" is a single scalar, so it survives a scalar reversal.
    assert_eq!(
        text_of(IntrinsicOp::TextReverse, vec![lit_text("héllo")]),
        "olléh"
    );
}

#[test]
fn text_reverse_preserves_grapheme_clusters() {
    // A single user-visible character reversed is itself. The evaluator's own
    // definition of a character is the grapheme cluster (`Length` counts
    // graphemes, `TextSplit` on an empty delimiter yields graphemes), so a
    // reversal that reorders the scalars inside a cluster contradicts it: it
    // turns one character into two and moves a combining mark onto the wrong
    // base. The `TextReverse` arm of `apply_intrinsic` reverses `chars()`.
    let toned = text_of(IntrinsicOp::TextReverse, vec![lit_text(THUMBS_UP_TONED)]);
    assert_eq!(toned, THUMBS_UP_TONED);
    assert_eq!(toned.graphemes(true).count(), 1);

    let accented = text_of(IntrinsicOp::TextReverse, vec![lit_text("noe\u{301}l")]);
    assert_eq!(accented, "le\u{301}on");
}

// -- TextPadStart and TextPadEnd --

#[test]
fn text_pad_start_prepends_a_cycled_pad_until_the_target_scalar_length() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextPadStart,
            vec![lit_text("7"), lit_int(3), lit_text("0")]
        ),
        "007"
    );
    assert_eq!(
        text_of(
            IntrinsicOp::TextPadStart,
            vec![lit_text("ab"), lit_int(5), lit_text("xy")]
        ),
        "xyxab"
    );
}

#[test]
fn text_pad_end_appends_a_cycled_pad_until_the_target_scalar_length() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextPadEnd,
            vec![lit_text("7"), lit_int(3), lit_text("0")]
        ),
        "700"
    );
    assert_eq!(
        text_of(
            IntrinsicOp::TextPadEnd,
            vec![lit_text("ab"), lit_int(5), lit_text("xy")]
        ),
        "abxyx"
    );
}

#[test]
fn text_pad_start_measures_the_subject_in_scalars_and_not_in_graphemes() {
    // `E_COMBINING` is one grapheme but two scalars, so padding to a width of
    // three adds one pad scalar. Anything aligning columns with this operation
    // will be off by the number of combining marks in the text.
    assert_eq!(
        text_of(
            IntrinsicOp::TextPadStart,
            vec![lit_text(E_COMBINING), lit_int(3), lit_text("-")]
        ),
        format!("-{E_COMBINING}")
    );
    // Two scalars already satisfy a width of two, so the emoji is untouched
    // even though it occupies a single column.
    assert_eq!(
        text_of(
            IntrinsicOp::TextPadStart,
            vec![lit_text(THUMBS_UP_TONED), lit_int(2), lit_text("-")]
        ),
        THUMBS_UP_TONED
    );
}

#[test]
fn text_pad_start_can_emit_a_partial_grapheme_when_the_pad_is_multi_scalar() {
    // The pad is cycled scalar by scalar, so a pad that is itself one cluster
    // can be cut in half. Pinned because a procedure padding with an emoji
    // gets a bare base character, not the toned one it asked for.
    assert_eq!(
        text_of(
            IntrinsicOp::TextPadStart,
            vec![lit_text("x"), lit_int(2), lit_text(THUMBS_UP_TONED)]
        ),
        "\u{1F44D}x"
    );
}

#[test]
fn padding_returns_the_subject_unchanged_when_the_pad_is_empty() {
    // An empty pad cannot make progress, so returning the subject is the only
    // termination that is not an infinite loop. It is silent rather than an
    // error, which a caller cannot distinguish from "already wide enough".
    assert_eq!(
        text_of(
            IntrinsicOp::TextPadStart,
            vec![lit_text("a"), lit_int(5), lit_text("")]
        ),
        "a"
    );
    assert_eq!(
        text_of(
            IntrinsicOp::TextPadEnd,
            vec![lit_text("a"), lit_int(5), lit_text("")]
        ),
        "a"
    );
}

#[test]
fn padding_never_truncates_a_subject_that_is_already_long_enough() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextPadStart,
            vec![lit_text("abcd"), lit_int(2), lit_text("0")]
        ),
        "abcd"
    );
    assert_eq!(
        text_of(
            IntrinsicOp::TextPadEnd,
            vec![lit_text("abcd"), lit_int(0), lit_text("0")]
        ),
        "abcd"
    );
}

#[test]
fn text_pad_end_rejects_a_negative_target_length() {
    let error = error_of(
        IntrinsicOp::TextPadEnd,
        vec![lit_text("a"), lit_int(-1), lit_text("0")],
    );
    assert!(
        error.to_string().contains("must be non-negative"),
        "unexpected error: {error}"
    );
}

// -- TextFormat --

#[test]
fn text_format_substitutes_brace_wrapped_keys_from_the_map() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextFormat,
            vec![
                lit_text("Hello {name}, you are {age}"),
                lit_map(&[("name", Value::Text("Ada".into())), ("age", Value::Int(36)),]),
            ]
        ),
        "Hello Ada, you are 36"
    );
}

#[test]
fn text_format_renders_every_scalar_value_kind_without_quoting() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextFormat,
            vec![
                lit_text("{f} {b} {n} {t}"),
                lit_map(&[
                    ("f", Value::Float(1.5)),
                    ("b", Value::Bool(true)),
                    ("n", Value::Null),
                    ("t", Value::Text("raw".into())),
                ]),
            ]
        ),
        "1.5 true null raw"
    );
}

#[test]
fn text_format_leaves_a_placeholder_with_no_matching_key_in_the_output() {
    // A missing argument is not an error, so the placeholder reaches whatever
    // consumes the text. Callers must validate their own maps.
    assert_eq!(
        text_of(
            IntrinsicOp::TextFormat,
            vec![
                lit_text("Hello {name} of {place}"),
                lit_map(&[("name", Value::Text("Ada".into()))]),
            ]
        ),
        "Hello Ada of {place}"
    );
}

#[test]
fn text_format_ignores_map_entries_the_template_never_mentions() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextFormat,
            vec![
                lit_text("Hello {name}"),
                lit_map(&[
                    ("name", Value::Text("Ada".into())),
                    ("unused", Value::Int(1)),
                ]),
            ]
        ),
        "Hello Ada"
    );
}

#[test]
fn text_format_has_no_escape_sequence_for_a_literal_placeholder() {
    // Doubling the braces does not escape anything: the inner pair is still a
    // placeholder, so a template cannot emit the literal text "{name}" while
    // any key named "name" is supplied.
    assert_eq!(
        text_of(
            IntrinsicOp::TextFormat,
            vec![
                lit_text("{{name}}"),
                lit_map(&[("name", Value::Text("Ada".into()))]),
            ]
        ),
        "{Ada}"
    );
}

#[test]
fn text_format_does_not_rescan_substituted_values_for_further_placeholders() {
    // Substitution runs key by key over the growing output, so a value that
    // happens to contain "{other_key}" is expanded again. The result depends
    // on the map's sort order, which makes it a data-driven injection: here
    // "{b}" must stay literal after being inserted. The `TextFormat` arm of
    // `apply_intrinsic` calls `replace` on the accumulating output.
    assert_eq!(
        text_of(
            IntrinsicOp::TextFormat,
            vec![
                lit_text("{a}"),
                lit_map(&[
                    ("a", Value::Text("{b}".into())),
                    ("b", Value::Text("injected".into())),
                ]),
            ]
        ),
        "{b}"
    );
}

#[test]
fn text_format_rejects_a_second_argument_that_is_not_a_map() {
    let error = error_of(
        IntrinsicOp::TextFormat,
        vec![lit_text("{a}"), lit_text("not a map")],
    );
    assert!(
        matches!(&error, SpoonError::TypeError { expected, .. } if expected == "map"),
        "unexpected error: {error:?}"
    );
}

// -- TextMatchesRegex --

#[test]
fn text_matches_regex_asks_only_whether_the_pattern_is_found_anywhere() {
    assert!(bool_of(
        IntrinsicOp::TextMatchesRegex,
        vec![lit_text("hello 42"), lit_text(r"\d+")]
    ));
    // The search is unanchored, so anchoring is the caller's job.
    assert!(!bool_of(
        IntrinsicOp::TextMatchesRegex,
        vec![lit_text("hello 42"), lit_text(r"^\d+$")]
    ));
    // An empty pattern matches at position zero, including in empty text.
    assert!(bool_of(
        IntrinsicOp::TextMatchesRegex,
        vec![lit_text(""), lit_text("")]
    ));
}

#[test]
fn text_matches_regex_treats_a_dot_as_one_scalar_and_not_one_byte() {
    // "中" is three bytes. A byte-oriented engine would need three dots.
    assert!(bool_of(
        IntrinsicOp::TextMatchesRegex,
        vec![lit_text("中"), lit_text("^.$")]
    ));
    assert!(bool_of(
        IntrinsicOp::TextMatchesRegex,
        vec![lit_text("naïve"), lit_text("ï")]
    ));
}

#[test]
fn text_matches_regex_reports_an_invalid_pattern_as_an_error_and_does_not_panic() {
    let error = error_of(
        IntrinsicOp::TextMatchesRegex,
        vec![lit_text("abc"), lit_text("(")],
    );
    assert!(
        error.to_string().contains("invalid pattern"),
        "unexpected error: {error}"
    );
}

// -- TextRegexCapture --

#[test]
fn text_regex_capture_returns_the_first_group_when_the_pattern_has_one() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextRegexCapture,
            vec![lit_text("hello world"), lit_text(r"(\w+) (\w+)")]
        ),
        "hello"
    );
}

#[test]
fn text_regex_capture_returns_a_named_group_because_it_is_also_group_one() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextRegexCapture,
            vec![lit_text("shipped in 2026"), lit_text(r"(?P<year>\d{4})")]
        ),
        "2026"
    );
}

#[test]
fn text_regex_capture_falls_back_to_the_whole_match_when_there_is_no_group() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextRegexCapture,
            vec![lit_text("a12b"), lit_text(r"\d+")]
        ),
        "12"
    );
}

#[test]
fn text_regex_capture_returns_null_when_the_pattern_does_not_match() {
    assert_eq!(
        call(
            IntrinsicOp::TextRegexCapture,
            vec![lit_text("abc"), lit_text(r"\d+")]
        )
        .unwrap(),
        Value::Null
    );
}

#[test]
fn text_regex_capture_returns_empty_text_for_a_group_that_matches_nothing() {
    // The group participates but is zero width. Empty text and null must stay
    // distinguishable, because null means "no match at all".
    assert_eq!(
        call(
            IntrinsicOp::TextRegexCapture,
            vec![lit_text("ac"), lit_text("a(b*)c")]
        )
        .unwrap(),
        Value::Text(String::new())
    );
}

#[test]
fn text_regex_capture_reports_the_whole_match_when_group_one_did_not_participate() {
    // With alternation only one branch has a group. When the other branch
    // wins, group one is absent and the fallback returns the whole match, so a
    // caller cannot tell "the group did not participate" from "the group
    // captured this".
    assert_eq!(
        text_of(
            IntrinsicOp::TextRegexCapture,
            vec![lit_text("y"), lit_text("(x)|y")]
        ),
        "y"
    );
}

#[test]
fn text_regex_capture_reports_an_invalid_pattern_as_an_error_and_does_not_panic() {
    let error = error_of(
        IntrinsicOp::TextRegexCapture,
        vec![lit_text("abc"), lit_text("[unclosed")],
    );
    assert!(
        error.to_string().contains("pattern is invalid"),
        "unexpected error: {error}"
    );
}

#[test]
fn text_regex_capture_rejects_a_pattern_larger_than_its_declared_bound() {
    // The bound is 16 KiB of pattern source, checked before compilation so a
    // hostile pattern never reaches the engine.
    let oversized = "a".repeat(16 * 1024 + 1);
    let error = error_of(
        IntrinsicOp::TextRegexCapture,
        vec![lit_text("aaa"), lit_text(&oversized)],
    );
    assert!(
        error.to_string().contains("exceeds its bound"),
        "unexpected error: {error}"
    );
}

// -- TextRegexReplaceAll --

#[test]
fn text_regex_replace_all_rewrites_every_match_not_just_the_first() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextRegexReplaceAll,
            vec![lit_text("a1b22c333"), lit_text(r"\d+"), lit_text("#")]
        ),
        "a#b#c#"
    );
}

#[test]
fn text_regex_replace_all_leaves_text_untouched_when_nothing_matches() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextRegexReplaceAll,
            vec![lit_text("abc"), lit_text(r"\d"), lit_text("#")]
        ),
        "abc"
    );
}

#[test]
fn text_regex_replace_all_expands_dollar_references_to_capture_groups() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextRegexReplaceAll,
            vec![
                lit_text("ada@example"),
                lit_text(r"(\w+)@(\w+)"),
                lit_text("$2/$1"),
            ]
        ),
        "example/ada"
    );
}

#[test]
fn text_regex_replace_all_needs_a_doubled_dollar_for_a_literal_dollar() {
    assert_eq!(
        text_of(
            IntrinsicOp::TextRegexReplaceAll,
            vec![lit_text("price"), lit_text("price"), lit_text("$$5")]
        ),
        "$5"
    );
    // A single dollar followed by digits is a group reference, so a
    // replacement meant to read "$5.00" silently loses the amount. Pinned
    // because it is the most likely way a procedure gets this wrong.
    assert_eq!(
        text_of(
            IntrinsicOp::TextRegexReplaceAll,
            vec![lit_text("price"), lit_text("price"), lit_text("$5.00")]
        ),
        ".00"
    );
}

#[test]
fn text_regex_replace_all_reports_an_invalid_pattern_as_an_error_and_does_not_panic() {
    let error = error_of(
        IntrinsicOp::TextRegexReplaceAll,
        vec![lit_text("abc"), lit_text("a{99,1}"), lit_text("x")],
    );
    assert!(
        error.to_string().contains("invalid pattern"),
        "unexpected error: {error}"
    );
}

// -- Base64 --

#[test]
fn text_base64_encode_matches_the_rfc4648_test_vectors() {
    // RFC 4648 section 10.
    let vectors = [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ];
    for (input, expected) in vectors {
        assert_eq!(
            text_of(IntrinsicOp::TextBase64Encode, vec![lit_text(input)]),
            expected,
            "encoding {input:?}"
        );
        assert_eq!(
            text_of(IntrinsicOp::TextBase64Decode, vec![lit_text(expected)]),
            input,
            "decoding {expected:?}"
        );
    }
}

#[test]
fn text_base64_round_trips_text_containing_multi_byte_scalars() {
    let original = "naïve 中文 👍🏽";
    let encoded = intrinsic(IntrinsicOp::TextBase64Encode, vec![lit_text(original)]);
    assert_eq!(
        text_of(IntrinsicOp::TextBase64Decode, vec![encoded]),
        original
    );
}

#[test]
fn text_base64_decode_rejects_malformed_input_rather_than_decoding_part_of_it() {
    for malformed in ["!!!!", "Zg=", "Zg===", "Z"] {
        let error = error_of(IntrinsicOp::TextBase64Decode, vec![lit_text(malformed)]);
        assert!(
            error.to_string().contains("text_base64_decode"),
            "input {malformed:?} gave an unexpected error: {error}"
        );
    }
}

#[test]
fn text_base64_decode_rejects_payloads_that_are_not_valid_utf8() {
    // "/w==" is the byte 0xFF, which is not valid UTF-8. This operation
    // carries text only, so binary payloads must fail loudly instead of being
    // replaced with U+FFFD.
    let error = error_of(IntrinsicOp::TextBase64Decode, vec![lit_text("/w==")]);
    assert!(
        error.to_string().contains("utf-8") || error.to_string().contains("UTF-8"),
        "unexpected error: {error}"
    );
}

// -- Hex --

#[test]
fn text_hex_encode_emits_two_lowercase_digits_per_utf8_byte() {
    assert_eq!(
        text_of(IntrinsicOp::TextHexEncode, vec![lit_text("abc")]),
        "616263"
    );
    // Three bytes for one scalar, so the unit is clearly the byte.
    assert_eq!(
        text_of(IntrinsicOp::TextHexEncode, vec![lit_text("中")]),
        "e4b8ad"
    );
    assert_eq!(text_of(IntrinsicOp::TextHexEncode, vec![lit_text("")]), "");
}

#[test]
fn text_hex_decode_accepts_either_letter_case() {
    assert_eq!(
        text_of(IntrinsicOp::TextHexDecode, vec![lit_text("E4B8AD")]),
        "中"
    );
    assert_eq!(
        text_of(IntrinsicOp::TextHexDecode, vec![lit_text("e4b8ad")]),
        "中"
    );
}

#[test]
fn text_hex_round_trips_text_containing_multi_byte_scalars() {
    let original = "naïve 中文 👍🏽";
    let encoded = intrinsic(IntrinsicOp::TextHexEncode, vec![lit_text(original)]);
    assert_eq!(text_of(IntrinsicOp::TextHexDecode, vec![encoded]), original);
}

#[test]
fn text_hex_decode_also_accepts_an_0x_prefix_that_the_encoder_never_emits() {
    // Asymmetric with the encoder, so pinned rather than assumed.
    assert_eq!(
        text_of(IntrinsicOp::TextHexDecode, vec![lit_text("0x616263")]),
        "abc"
    );
    assert_eq!(
        text_of(IntrinsicOp::TextHexDecode, vec![lit_text("0x")]),
        ""
    );
}

#[test]
fn text_hex_decode_rejects_an_odd_number_of_digits() {
    let error = error_of(IntrinsicOp::TextHexDecode, vec![lit_text("abc")]);
    assert!(
        error.to_string().contains("odd number of hex digits"),
        "unexpected error: {error}"
    );
}

#[test]
fn text_hex_decode_rejects_a_digit_outside_the_hex_alphabet() {
    for malformed in ["zz", "6g", "中中"] {
        let error = error_of(IntrinsicOp::TextHexDecode, vec![lit_text(malformed)]);
        assert!(
            error.to_string().contains("invalid hex digit")
                || error.to_string().contains("odd number of hex digits"),
            "input {malformed:?} gave an unexpected error: {error}"
        );
    }
}

#[test]
fn text_hex_decode_rejects_bytes_that_are_not_valid_utf8() {
    let error = error_of(IntrinsicOp::TextHexDecode, vec![lit_text("ff")]);
    assert!(
        error.to_string().contains("utf-8") || error.to_string().contains("UTF-8"),
        "unexpected error: {error}"
    );
}

// -- URL encoding --

#[test]
fn text_url_encode_escapes_every_byte_outside_the_unreserved_set() {
    assert_eq!(
        text_of(IntrinsicOp::TextUrlEncode, vec![lit_text("a b")]),
        "a%20b"
    );
    assert_eq!(
        text_of(IntrinsicOp::TextUrlEncode, vec![lit_text("aZ09-._~")]),
        "aZ09-._~"
    );
    assert_eq!(
        text_of(IntrinsicOp::TextUrlEncode, vec![lit_text("a/b?c")]),
        "a%2Fb%3Fc"
    );
    // Multi-byte scalars are escaped byte by byte, which is what RFC 3986
    // requires.
    assert_eq!(
        text_of(IntrinsicOp::TextUrlEncode, vec![lit_text("中")]),
        "%E4%B8%AD"
    );
    // A literal plus is escaped, so it cannot be confused with an encoded
    // space on the way back.
    assert_eq!(
        text_of(IntrinsicOp::TextUrlEncode, vec![lit_text("a+b")]),
        "a%2Bb"
    );
}

#[test]
fn text_url_decode_expands_ascii_escapes_and_reads_plus_as_space() {
    assert_eq!(
        text_of(IntrinsicOp::TextUrlDecode, vec![lit_text("a%20b")]),
        "a b"
    );
    assert_eq!(
        text_of(IntrinsicOp::TextUrlDecode, vec![lit_text("%41")]),
        "A"
    );
    assert_eq!(
        text_of(IntrinsicOp::TextUrlDecode, vec![lit_text("a+b")]),
        "a b"
    );
    assert_eq!(text_of(IntrinsicOp::TextUrlDecode, vec![lit_text("")]), "");
}

#[test]
fn text_url_encode_and_decode_round_trip_text_with_multi_byte_scalars() {
    // Percent escapes carry UTF-8 bytes, so decoding must reassemble the byte
    // sequence into scalars. Decoding each byte as if it were a scalar turns
    // every non-ASCII character into mojibake, which is silent corruption
    // rather than an error. The `TextUrlDecode` arm of `apply_intrinsic`
    // pushes each decoded byte with `as char`, which is Latin-1, not UTF-8.
    let original = "naïve 中文 👍🏽";
    let encoded = intrinsic(IntrinsicOp::TextUrlEncode, vec![lit_text(original)]);
    assert_eq!(text_of(IntrinsicOp::TextUrlDecode, vec![encoded]), original);
}

#[test]
fn text_url_decode_leaves_text_without_escapes_unchanged() {
    // Nothing here is percent encoded, so decoding is the identity. It is not,
    // because each byte of the input is pushed as if it were a scalar, so this
    // operation corrupts non-ASCII text that had nothing to decode at all.
    assert_eq!(
        text_of(IntrinsicOp::TextUrlDecode, vec![lit_text("中文")]),
        "中文"
    );
}

#[test]
fn text_url_decode_rejects_a_malformed_percent_escape() {
    // A truncated or non-hex escape is corrupt input. Passing the "%" through
    // as a literal makes decode(encode(x)) != x undetectable by the caller and
    // lets a mangled query string flow onward as if it had parsed.
    for malformed in ["%", "%4", "%zz", "abc%"] {
        let error = error_of(IntrinsicOp::TextUrlDecode, vec![lit_text(malformed)]);
        assert!(
            error.to_string().contains("text_url_decode"),
            "input {malformed:?} gave an unexpected error: {error}"
        );
    }
}

// -- Hashing --

#[test]
fn hash_sha256_matches_the_published_fips_180_2_vectors() {
    // FIPS 180-2 appendix B.1 for "abc"; the empty string digest is the
    // standard published value for SHA-256.
    assert_eq!(
        text_of(IntrinsicOp::HashSha256, vec![lit_text("")]),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        text_of(IntrinsicOp::HashSha256, vec![lit_text("abc")]),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn hash_md5_matches_the_rfc1321_test_suite_vectors() {
    // RFC 1321 appendix A.5.
    assert_eq!(
        text_of(IntrinsicOp::HashMd5, vec![lit_text("")]),
        "d41d8cd98f00b204e9800998ecf8427e"
    );
    assert_eq!(
        text_of(IntrinsicOp::HashMd5, vec![lit_text("abc")]),
        "900150983cd24fb0d6963f7d28e17f72"
    );
    assert_eq!(
        text_of(IntrinsicOp::HashMd5, vec![lit_text("message digest")]),
        "f96b697d7cb7938d525a2f31aaf161d0"
    );
}

#[test]
fn hashes_are_taken_over_utf8_bytes_so_unicode_normalization_matters() {
    // "é" precomposed and "e" plus a combining acute are the same character to
    // a reader and different byte strings to a hash. A procedure comparing
    // digests of user-entered text must normalize first.
    let precomposed_sha = text_of(IntrinsicOp::HashSha256, vec![lit_text("\u{e9}")]);
    let decomposed_sha = text_of(IntrinsicOp::HashSha256, vec![lit_text(E_COMBINING)]);
    assert_ne!(precomposed_sha, decomposed_sha);
    assert_eq!(precomposed_sha.len(), 64);

    let precomposed_md5 = text_of(IntrinsicOp::HashMd5, vec![lit_text("\u{e9}")]);
    let decomposed_md5 = text_of(IntrinsicOp::HashMd5, vec![lit_text(E_COMBINING)]);
    assert_ne!(precomposed_md5, decomposed_md5);
    assert_eq!(precomposed_md5.len(), 32);
}

// -- TextLevenshtein --

#[test]
fn text_levenshtein_counts_the_classic_kitten_sitting_edits() {
    assert_eq!(
        int_of(
            IntrinsicOp::TextLevenshtein,
            vec![lit_text("kitten"), lit_text("sitting")]
        ),
        3
    );
}

#[test]
fn text_levenshtein_is_zero_for_identical_text_and_a_length_for_empty_text() {
    assert_eq!(
        int_of(
            IntrinsicOp::TextLevenshtein,
            vec![lit_text(""), lit_text("")]
        ),
        0
    );
    assert_eq!(
        int_of(
            IntrinsicOp::TextLevenshtein,
            vec![lit_text("中文 👍🏽"), lit_text("中文 👍🏽")]
        ),
        0
    );
    assert_eq!(
        int_of(
            IntrinsicOp::TextLevenshtein,
            vec![lit_text("abc"), lit_text("")]
        ),
        3
    );
    assert_eq!(
        int_of(
            IntrinsicOp::TextLevenshtein,
            vec![lit_text(""), lit_text("abc")]
        ),
        3
    );
}

#[test]
fn text_levenshtein_counts_scalars_and_not_bytes() {
    // "é" is two bytes, so a byte-based matrix would report two edits here and
    // would make every accented word look twice as far away as it is.
    assert_eq!(
        int_of(
            IntrinsicOp::TextLevenshtein,
            vec![lit_text("café"), lit_text("cafe")]
        ),
        1
    );
    assert_eq!(
        int_of(
            IntrinsicOp::TextLevenshtein,
            vec![lit_text("中文"), lit_text("中")]
        ),
        1
    );
}

#[test]
fn text_levenshtein_charges_a_separate_edit_for_a_combining_mark() {
    // Scalars, not graphemes: the decomposed form is two edits away from the
    // precomposed one even though both render identically. Distances are only
    // comparable between strings in the same normal form.
    assert_eq!(
        int_of(
            IntrinsicOp::TextLevenshtein,
            vec![lit_text(E_COMBINING), lit_text("\u{e9}")]
        ),
        2
    );
}

// -- Arity and argument types across the whole assigned surface --

/// Every operation covered by this file with its wire name and declared arity.
fn assigned_operations() -> Vec<(IntrinsicOp, &'static str, usize)> {
    vec![
        (IntrinsicOp::TextCharAt, "text_char_at", 2),
        (IntrinsicOp::TextCharCode, "text_char_code", 1),
        (IntrinsicOp::TextFromCharCode, "text_from_char_code", 1),
        (IntrinsicOp::TextSubstring, "text_substring", 3),
        (IntrinsicOp::TextReverse, "text_reverse", 1),
        (IntrinsicOp::TextPadStart, "text_pad_start", 3),
        (IntrinsicOp::TextPadEnd, "text_pad_end", 3),
        (IntrinsicOp::TextFormat, "text_format", 2),
        (IntrinsicOp::TextLevenshtein, "text_levenshtein", 2),
        (IntrinsicOp::TextMatchesRegex, "text_matches_regex", 2),
        (IntrinsicOp::TextRegexCapture, "text_regex_capture", 2),
        (
            IntrinsicOp::TextRegexReplaceAll,
            "text_regex_replace_all",
            3,
        ),
        (IntrinsicOp::TextBase64Encode, "text_base64_encode", 1),
        (IntrinsicOp::TextBase64Decode, "text_base64_decode", 1),
        (IntrinsicOp::TextHexEncode, "text_hex_encode", 1),
        (IntrinsicOp::TextHexDecode, "text_hex_decode", 1),
        (IntrinsicOp::TextUrlEncode, "text_url_encode", 1),
        (IntrinsicOp::TextUrlDecode, "text_url_decode", 1),
        (IntrinsicOp::HashMd5, "hash_md5", 1),
        (IntrinsicOp::HashSha256, "hash_sha256", 1),
    ]
}

#[test]
fn every_covered_operation_reports_its_declared_arity_before_evaluating() {
    // Arity is checked ahead of argument evaluation, and the helpers that
    // destructure arguments panic if it is not, so this guards the boundary
    // between "wrong call" and "process abort".
    for (op, name, arity) in assigned_operations() {
        for count in [arity.saturating_sub(1), arity + 1] {
            if count == arity {
                continue;
            }
            let args = (0..count).map(|_| lit_text("a")).collect();
            match error_of(op, args) {
                SpoonError::ArityMismatch {
                    name: reported,
                    expected,
                    got,
                } => {
                    assert_eq!(reported, name, "{name} reported the wrong wire name");
                    assert_eq!(expected, arity, "{name} reported the wrong arity");
                    assert_eq!(got, count, "{name} reported the wrong argument count");
                }
                other => panic!("{name} with {count} args gave {other:?}"),
            }
        }
    }
}

#[test]
fn operations_taking_text_reject_a_non_text_first_argument() {
    let text_first = [
        IntrinsicOp::TextCharCode,
        IntrinsicOp::TextReverse,
        IntrinsicOp::TextBase64Encode,
        IntrinsicOp::TextBase64Decode,
        IntrinsicOp::TextHexEncode,
        IntrinsicOp::TextHexDecode,
        IntrinsicOp::TextUrlEncode,
        IntrinsicOp::TextUrlDecode,
        IntrinsicOp::HashMd5,
        IntrinsicOp::HashSha256,
    ];
    for op in text_first {
        let error = error_of(op, vec![lit_int(1)]);
        assert!(
            matches!(&error, SpoonError::TypeError { expected, got } if expected == "text" && got == "int"),
            "{op:?} gave an unexpected error: {error:?}"
        );
    }
}

#[test]
fn operations_taking_an_index_reject_a_non_int_index() {
    let cases = [
        (
            IntrinsicOp::TextCharAt,
            vec![lit_text("abc"), lit_text("0")],
        ),
        (
            IntrinsicOp::TextSubstring,
            vec![lit_text("abc"), lit_text("0"), lit_int(1)],
        ),
        (
            IntrinsicOp::TextPadStart,
            vec![lit_text("abc"), lit_text("5"), lit_text("0")],
        ),
        (
            IntrinsicOp::TextPadEnd,
            vec![lit_text("abc"), lit_text("5"), lit_text("0")],
        ),
    ];
    for (op, args) in cases {
        let error = error_of(op, args);
        assert!(
            matches!(&error, SpoonError::TypeError { expected, .. } if expected == "int"),
            "{op:?} gave an unexpected error: {error:?}"
        );
    }
}

#[test]
fn text_from_char_code_rejects_a_text_argument_because_it_takes_a_code_point() {
    let error = error_of(IntrinsicOp::TextFromCharCode, vec![lit_text("65")]);
    assert!(
        matches!(&error, SpoonError::TypeError { expected, .. } if expected == "int"),
        "unexpected error: {error:?}"
    );
}

#[test]
fn text_levenshtein_rejects_a_non_text_comparand() {
    let error = error_of(
        IntrinsicOp::TextLevenshtein,
        vec![lit_text("abc"), lit_int(3)],
    );
    assert!(
        matches!(&error, SpoonError::TypeError { expected, got } if expected == "text" && got == "int"),
        "unexpected error: {error:?}"
    );
}
