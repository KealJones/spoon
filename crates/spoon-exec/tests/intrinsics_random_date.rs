//! Behaviour of the six random intrinsics and the six date/time intrinsics.
//!
//! Both families are nondeterministic at the source. The random operations
//! draw from `rand::rng()`, the process-wide thread RNG, and `date_now` reads
//! `SystemTime::now()` directly. Neither is injectable, so nothing here may
//! assert a specific draw or a specific instant. Every assertion is instead an
//! invariant that must hold for *any* draw: bounds, permutation identity,
//! round-trips, and error behaviour. A test that asserts a particular random
//! number is either flaky or a lie, and a flaky test is worse than no test.
//!
//! Where a property is only probabilistic, the iteration count is chosen so
//! that a spurious failure is less likely than a hardware fault, and the
//! reasoning is written down at the assertion.

use std::collections::HashSet;

use spoon_core::{Expr, IntrinsicOp, Value};
use spoon_exec::{Env, Evaluator, SpoonError};

fn intrinsic(op: IntrinsicOp, args: Vec<Expr>) -> Expr {
    Expr::Intrinsic {
        version: 1,
        op,
        args,
    }
}

fn lit_int(n: i64) -> Expr {
    Expr::Literal(Value::Int(n))
}

fn lit_text(s: &str) -> Expr {
    Expr::Literal(Value::Text(s.to_string()))
}

fn lit_list(items: Vec<Value>) -> Expr {
    Expr::Literal(Value::List(items))
}

fn eval(expr: &Expr) -> Result<Value, SpoonError> {
    Evaluator::new().eval(expr, &mut Env::new())
}

fn eval_ok(expr: &Expr) -> Value {
    eval(expr).unwrap_or_else(|error| panic!("expected success, got error: {error}"))
}

fn eval_int(expr: &Expr) -> i64 {
    match eval_ok(expr) {
        Value::Int(n) => n,
        other => panic!("expected an int, got {other:?}"),
    }
}

fn eval_float(expr: &Expr) -> f64 {
    match eval_ok(expr) {
        Value::Float(f) => f,
        other => panic!("expected a float, got {other:?}"),
    }
}

fn eval_text(expr: &Expr) -> String {
    match eval_ok(expr) {
        Value::Text(text) => text,
        other => panic!("expected text, got {other:?}"),
    }
}

fn eval_list(expr: &Expr) -> Vec<Value> {
    match eval_ok(expr) {
        Value::List(items) => items,
        other => panic!("expected a list, got {other:?}"),
    }
}

fn random_int(low: i64, high: i64) -> Result<Value, SpoonError> {
    eval(&intrinsic(
        IntrinsicOp::RandomInt,
        vec![lit_int(low), lit_int(high)],
    ))
}

fn int_list(values: &[i64]) -> Vec<Value> {
    values.iter().copied().map(Value::Int).collect()
}

/// Extract the integers from a list result so that a multiset comparison can
/// use ordinary sorting; `Value` carries a float variant and so is not `Ord`.
fn ints(values: &[Value]) -> Vec<i64> {
    values
        .iter()
        .map(|value| match value {
            Value::Int(n) => *n,
            other => panic!("expected a list of ints, got {other:?}"),
        })
        .collect()
}

fn date_from_parts(year: i64, month: i64, day: i64) -> Result<Value, SpoonError> {
    eval(&intrinsic(
        IntrinsicOp::DateFromParts,
        vec![lit_int(year), lit_int(month), lit_int(day)],
    ))
}

fn date(year: i64, month: i64, day: i64) -> i64 {
    match date_from_parts(year, month, day) {
        Ok(Value::Int(ts)) => ts,
        other => panic!("expected a timestamp for {year}-{month}-{day}, got {other:?}"),
    }
}

fn part(timestamp: i64, name: &str) -> i64 {
    eval_int(&intrinsic(
        IntrinsicOp::DateGetPart,
        vec![lit_int(timestamp), lit_text(name)],
    ))
}

fn ymd(timestamp: i64) -> (i64, i64, i64) {
    (
        part(timestamp, "year"),
        part(timestamp, "month"),
        part(timestamp, "day"),
    )
}

fn date_add(timestamp: i64, amount: i64, unit: &str) -> Result<Value, SpoonError> {
    eval(&intrinsic(
        IntrinsicOp::DateAdd,
        vec![lit_int(timestamp), lit_int(amount), lit_text(unit)],
    ))
}

fn date_diff(from: i64, to: i64, unit: &str) -> Result<Value, SpoonError> {
    eval(&intrinsic(
        IntrinsicOp::DateDiff,
        vec![lit_int(from), lit_int(to), lit_text(unit)],
    ))
}

fn date_format(timestamp: i64, format: &str) -> String {
    eval_text(&intrinsic(
        IntrinsicOp::DateFormat,
        vec![lit_int(timestamp), lit_text(format)],
    ))
}

// ---------------------------------------------------------------------------
// RandomInt
// ---------------------------------------------------------------------------

#[test]
fn random_int_never_leaves_the_closed_interval_it_was_given() {
    for _ in 0..2_000 {
        let drawn = match random_int(-5, 5) {
            Ok(Value::Int(n)) => n,
            other => panic!("expected an int draw, got {other:?}"),
        };
        assert!(
            (-5..=5).contains(&drawn),
            "random_int(-5, 5) produced {drawn}, which is outside its declared bounds"
        );
    }
}

#[test]
fn random_int_treats_both_bounds_as_inclusive() {
    // The implementation samples `low..=high`, so the upper bound must be
    // reachable. Over 200 draws from a two-value range, the chance of missing
    // either endpoint is 2 * 2^-200, which is far below any realistic flake
    // rate.
    let mut seen_low = false;
    let mut seen_high = false;
    for _ in 0..200 {
        match random_int(0, 1) {
            Ok(Value::Int(0)) => seen_low = true,
            Ok(Value::Int(1)) => seen_high = true,
            other => panic!("random_int(0, 1) produced {other:?}, outside the two-value range"),
        }
    }
    assert!(
        seen_low,
        "the lower bound was never drawn, so it is exclusive"
    );
    assert!(
        seen_high,
        "the upper bound was never drawn, so the range is half-open rather than closed"
    );
}

#[test]
fn random_int_with_equal_bounds_yields_exactly_that_value() {
    for _ in 0..64 {
        assert_eq!(random_int(42, 42).unwrap(), Value::Int(42));
    }
}

#[test]
fn random_int_rejects_inverted_bounds_instead_of_panicking() {
    // `rand`'s `random_range` panics on an empty range, so the guard in front
    // of it is the only thing standing between a procedure and an abort.
    let error = random_int(10, 1).expect_err("inverted bounds must not produce a value");
    assert!(
        matches!(&error, SpoonError::Other(message) if message.contains("low must be <= high")),
        "expected a descriptive error, got {error:?}"
    );
}

#[test]
fn random_int_over_the_widest_possible_range_neither_overflows_nor_degenerates() {
    // A full-domain inclusive range is the case where a naive
    // `high - low + 1` sizing would overflow, so it is worth pinning.
    let distinct: HashSet<i64> = (0..256)
        .map(|_| match random_int(i64::MIN, i64::MAX) {
            Ok(Value::Int(n)) => n,
            other => panic!("expected an int draw, got {other:?}"),
        })
        .collect();
    assert!(
        distinct.len() > 1,
        "the full i64 range produced a single repeated value"
    );
}

// ---------------------------------------------------------------------------
// RandomFloat
// ---------------------------------------------------------------------------

#[test]
fn random_float_stays_in_the_half_open_unit_interval() {
    // `rand`'s `f64` sampling is documented as [0, 1). Zero is attainable in
    // principle but only once in 2^53 draws, so this asserts only the bound
    // that always holds rather than trying to observe the endpoint.
    for _ in 0..2_000 {
        let drawn = eval_float(&intrinsic(IntrinsicOp::RandomFloat, vec![]));
        assert!(
            (0.0..1.0).contains(&drawn),
            "random_float produced {drawn}, outside [0, 1)"
        );
    }
}

#[test]
fn random_float_does_not_return_a_constant() {
    // Two f64 draws colliding has probability 2^-53; over 64 draws a fully
    // degenerate result would mean the RNG is not being consulted at all.
    let distinct: HashSet<u64> = (0..64)
        .map(|_| eval_float(&intrinsic(IntrinsicOp::RandomFloat, vec![])).to_bits())
        .collect();
    assert!(
        distinct.len() > 1,
        "64 calls to random_float returned a single value, so it is not sampling"
    );
}

// ---------------------------------------------------------------------------
// RandomChoice
// ---------------------------------------------------------------------------

#[test]
fn random_choice_always_returns_an_element_of_the_population() {
    let population = int_list(&[10, 20, 30, 40]);
    for _ in 0..500 {
        let chosen = eval_ok(&intrinsic(
            IntrinsicOp::RandomChoice,
            vec![lit_list(population.clone())],
        ));
        assert!(
            population.contains(&chosen),
            "random_choice invented the value {chosen:?}"
        );
    }
}

#[test]
fn random_choice_can_reach_every_element_of_the_population() {
    // Missing one of four elements across 300 draws has probability
    // 4 * (3/4)^300, roughly 1e-37.
    let population = int_list(&[1, 2, 3, 4]);
    let mut seen: HashSet<i64> = HashSet::new();
    for _ in 0..300 {
        match eval_ok(&intrinsic(
            IntrinsicOp::RandomChoice,
            vec![lit_list(population.clone())],
        )) {
            Value::Int(n) => {
                seen.insert(n);
            }
            other => panic!("expected an int, got {other:?}"),
        }
    }
    assert_eq!(seen.len(), 4, "some elements are unreachable: saw {seen:?}");
}

#[test]
fn random_choice_on_a_single_element_list_returns_that_element() {
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::RandomChoice,
            vec![lit_list(int_list(&[7]))]
        )),
        Value::Int(7)
    );
}

#[test]
fn random_choice_on_an_empty_list_is_an_error_rather_than_a_panic() {
    let error = eval(&intrinsic(
        IntrinsicOp::RandomChoice,
        vec![lit_list(Vec::new())],
    ))
    .expect_err("an empty population has no valid choice");
    assert!(
        matches!(&error, SpoonError::Other(message) if message.contains("must not be empty")),
        "expected a descriptive error, got {error:?}"
    );
}

// ---------------------------------------------------------------------------
// RandomShuffle
// ---------------------------------------------------------------------------

#[test]
fn random_shuffle_is_a_permutation_that_invents_and_loses_nothing() {
    let original: Vec<i64> = (0..12).collect();
    let input = int_list(&original);
    for _ in 0..500 {
        let shuffled = eval_list(&intrinsic(
            IntrinsicOp::RandomShuffle,
            vec![lit_list(input.clone())],
        ));
        assert_eq!(
            shuffled.len(),
            original.len(),
            "shuffle changed the length of the list"
        );
        let mut observed = ints(&shuffled);
        observed.sort_unstable();
        assert_eq!(
            observed, original,
            "shuffle did not preserve the multiset of elements"
        );
    }
}

#[test]
fn random_shuffle_actually_reorders_rather_than_returning_the_input() {
    // A shuffle of twelve elements lands on the identity permutation with
    // probability 1/12!, so 200 consecutive identity results would mean the
    // operation is a no-op, not bad luck.
    let input = int_list(&(0..12).collect::<Vec<_>>());
    let reordered = (0..200).any(|_| {
        eval_list(&intrinsic(
            IntrinsicOp::RandomShuffle,
            vec![lit_list(input.clone())],
        )) != input
    });
    assert!(
        reordered,
        "200 shuffles all returned the input order, so nothing is being shuffled"
    );
}

#[test]
fn random_shuffle_handles_empty_and_single_element_lists() {
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::RandomShuffle,
            vec![lit_list(Vec::new())]
        )),
        Value::List(Vec::new())
    );
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::RandomShuffle,
            vec![lit_list(int_list(&[9]))]
        )),
        Value::List(int_list(&[9]))
    );
}

// ---------------------------------------------------------------------------
// RandomSample
// ---------------------------------------------------------------------------

#[test]
fn random_sample_draws_without_replacement() {
    // Every element of the population is distinct, so a sample containing a
    // repeat would prove the draw is with replacement.
    let population = int_list(&[1, 2, 3, 4, 5, 6, 7, 8]);
    for _ in 0..500 {
        let sample = eval_list(&intrinsic(
            IntrinsicOp::RandomSample,
            vec![lit_list(population.clone()), lit_int(4)],
        ));
        assert_eq!(sample.len(), 4, "sample size does not match the request");
        let distinct: HashSet<i64> = ints(&sample).into_iter().collect();
        assert_eq!(
            distinct.len(),
            4,
            "sample {sample:?} repeats an element, so it draws with replacement"
        );
        for value in &sample {
            assert!(
                population.contains(value),
                "sample contains {value:?}, which is not in the population"
            );
        }
    }
}

#[test]
fn random_sample_of_the_whole_population_returns_every_element_once() {
    let population = int_list(&[3, 1, 4, 1, 5]);
    let sample = eval_list(&intrinsic(
        IntrinsicOp::RandomSample,
        vec![lit_list(population.clone()), lit_int(5)],
    ));
    let mut observed = ints(&sample);
    observed.sort_unstable();
    let mut expected = ints(&population);
    expected.sort_unstable();
    assert_eq!(
        observed, expected,
        "a full-population sample must be a permutation, duplicates included"
    );
}

#[test]
fn random_sample_of_size_zero_is_an_empty_list() {
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::RandomSample,
            vec![lit_list(int_list(&[1, 2, 3])), lit_int(0)]
        )),
        Value::List(Vec::new())
    );
}

#[test]
fn random_sample_larger_than_the_population_is_an_error() {
    let error = eval(&intrinsic(
        IntrinsicOp::RandomSample,
        vec![lit_list(int_list(&[1, 2])), lit_int(3)],
    ))
    .expect_err("a sample cannot exceed the population when drawing without replacement");
    assert!(
        matches!(&error, SpoonError::Other(message) if message.contains("n must be <= list length")),
        "expected a descriptive error, got {error:?}"
    );
}

#[test]
fn random_sample_rejects_a_negative_size() {
    let error = eval(&intrinsic(
        IntrinsicOp::RandomSample,
        vec![lit_list(int_list(&[1, 2])), lit_int(-1)],
    ))
    .expect_err("a negative sample size has no meaning");
    assert!(
        matches!(&error, SpoonError::Other(message) if message.contains("must be non-negative")),
        "expected a descriptive error, got {error:?}"
    );
}

#[test]
fn random_sample_from_an_empty_population_is_only_valid_for_size_zero() {
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::RandomSample,
            vec![lit_list(Vec::new()), lit_int(0)]
        )),
        Value::List(Vec::new())
    );
    assert!(
        eval(&intrinsic(
            IntrinsicOp::RandomSample,
            vec![lit_list(Vec::new()), lit_int(1)],
        ))
        .is_err(),
        "sampling one element from nothing must fail"
    );
}

// ---------------------------------------------------------------------------
// RandomUuid
// ---------------------------------------------------------------------------

#[test]
fn random_uuid_is_a_well_formed_lowercase_version_4_uuid() {
    for _ in 0..500 {
        let id = eval_text(&intrinsic(IntrinsicOp::RandomUuid, vec![]));
        assert_eq!(id.len(), 36, "{id} is not 36 characters");
        let chars: Vec<char> = id.chars().collect();
        for index in [8, 13, 18, 23] {
            assert_eq!(chars[index], '-', "{id} lacks a hyphen at index {index}");
        }
        for (index, character) in chars.iter().enumerate() {
            if matches!(index, 8 | 13 | 18 | 23) {
                continue;
            }
            assert!(
                character.is_ascii_hexdigit() && !character.is_ascii_uppercase(),
                "{id} has the non lowercase-hex character {character:?} at index {index}"
            );
        }
        // A v4 UUID pins the version nibble to 4 and the variant nibble to
        // one of 8, 9, a, b. Getting these wrong yields a string that looks
        // like a UUID but fails validation in any consumer that checks.
        assert_eq!(chars[14], '4', "{id} does not declare version 4");
        assert!(
            matches!(chars[19], '8' | '9' | 'a' | 'b'),
            "{id} has the invalid RFC 4122 variant nibble {:?}",
            chars[19]
        );
    }
}

#[test]
fn random_uuid_does_not_repeat_across_calls() {
    // 122 random bits make a collision in 1,000 draws impossible in practice,
    // so any duplicate means the value is not actually being regenerated.
    let ids: HashSet<String> = (0..1_000)
        .map(|_| eval_text(&intrinsic(IntrinsicOp::RandomUuid, vec![])))
        .collect();
    assert_eq!(ids.len(), 1_000, "random_uuid returned a duplicate");
}

// ---------------------------------------------------------------------------
// DateNow
// ---------------------------------------------------------------------------

#[test]
fn date_now_returns_whole_unix_seconds_inside_a_plausible_window() {
    let now = eval_int(&intrinsic(IntrinsicOp::DateNow, vec![]));
    // 1_700_000_000 is 2023-11-14 and 4_102_444_800 is 2100-01-01. A value
    // outside that window means the clock is being read with the wrong unit
    // (milliseconds, nanoseconds) rather than that time has passed.
    assert!(
        (1_700_000_000..4_102_444_800).contains(&now),
        "date_now returned {now}, which is not a plausible unix second count"
    );
}

#[test]
fn date_now_never_moves_backwards_between_two_reads() {
    let first = eval_int(&intrinsic(IntrinsicOp::DateNow, vec![]));
    let second = eval_int(&intrinsic(IntrinsicOp::DateNow, vec![]));
    assert!(
        second >= first,
        "date_now went backwards: {first} then {second}"
    );
}

#[test]
fn date_now_composes_with_date_get_part_to_yield_a_plausible_year() {
    let now = eval_int(&intrinsic(IntrinsicOp::DateNow, vec![]));
    let year = part(now, "year");
    assert!(
        (2024..2100).contains(&year),
        "the current timestamp decoded to year {year}"
    );
}

// ---------------------------------------------------------------------------
// DateFromParts and DateGetPart
// ---------------------------------------------------------------------------

#[test]
fn date_from_parts_and_date_get_part_round_trip_ordinary_dates() {
    let cases = [
        (1970, 1, 1),
        (1969, 12, 31),
        (1900, 3, 1),
        (2000, 1, 1),
        (2024, 3, 15),
        (2024, 12, 31),
        (2099, 6, 30),
    ];
    for (year, month, day) in cases {
        let timestamp = date(year, month, day);
        assert_eq!(
            ymd(timestamp),
            (year, month, day),
            "{year}-{month:02}-{day:02} did not survive a round trip"
        );
    }
}

#[test]
fn date_from_parts_anchors_to_the_known_unix_epoch_values() {
    // Independent anchors: without at least one, a round trip could be
    // self-consistently wrong in both directions.
    assert_eq!(date(1970, 1, 1), 0);
    assert_eq!(date(2000, 1, 1), 946_684_800);
    assert_eq!(date(2024, 2, 29), 1_709_164_800);
}

#[test]
fn date_from_parts_produces_midnight_so_the_time_parts_are_all_zero() {
    let timestamp = date(2024, 3, 15);
    assert_eq!(part(timestamp, "hour"), 0);
    assert_eq!(part(timestamp, "minute"), 0);
    assert_eq!(part(timestamp, "second"), 0);
}

#[test]
fn date_get_part_reports_weekday_with_monday_as_zero() {
    // 2024-03-15 was a Friday and 1970-01-01 was a Thursday. The evaluator
    // documents 0 = Monday, so those are 4 and 3.
    assert_eq!(part(date(2024, 3, 15), "weekday"), 4);
    assert_eq!(part(0, "weekday"), 3);
    // A pre-epoch date exercises the euclidean remainder rather than a
    // truncating one, which would produce a negative weekday.
    let weekday = part(date(1969, 12, 31), "weekday");
    assert!(
        (0..7).contains(&weekday),
        "a pre-epoch date produced weekday {weekday}"
    );
}

#[test]
fn date_get_part_decodes_the_time_of_day_including_before_the_epoch() {
    let timestamp = date(2024, 3, 15) + 13 * 3600 + 45 * 60 + 7;
    assert_eq!(part(timestamp, "hour"), 13);
    assert_eq!(part(timestamp, "minute"), 45);
    assert_eq!(part(timestamp, "second"), 7);

    // One second before the epoch is 1969-12-31T23:59:59, which a truncating
    // division would render as a negative hour.
    assert_eq!(ymd(-1), (1969, 12, 31));
    assert_eq!(part(-1, "hour"), 23);
    assert_eq!(part(-1, "minute"), 59);
    assert_eq!(part(-1, "second"), 59);
}

#[test]
fn date_from_parts_accepts_leap_days_only_in_years_that_actually_have_them() {
    // 2024 is a leap year, 2000 is one because it is divisible by 400, and
    // 1900 is not because it is a century that is not. This is the rule date
    // code gets wrong most often.
    assert_eq!(ymd(date(2024, 2, 29)), (2024, 2, 29));
    assert_eq!(ymd(date(2000, 2, 29)), (2000, 2, 29));
}

#[test]
fn date_from_parts_rejects_out_of_range_months_and_days() {
    for (year, month, day) in [
        (2024, 0, 15),
        (2024, 13, 15),
        (2024, -1, 15),
        (2024, 3, 0),
        (2024, 3, 32),
        (2024, 3, -1),
    ] {
        let error = date_from_parts(year, month, day)
            .expect_err("a month or day outside the calendar must be rejected");
        assert!(
            matches!(&error, SpoonError::Other(message) if message.contains("invalid month or day")),
            "expected a descriptive error for {year}-{month}-{day}, got {error:?}"
        );
    }
}

#[test]
fn date_from_parts_rejects_days_that_do_not_exist_in_the_requested_month() {
    // Range-checking day against 1..=31 is not enough: a day that the month
    // does not have silently rolls forward into the next month, so a
    // procedure that builds a date from user input gets a plausible-looking
    // timestamp for a date that never existed.
    let impossible = [
        (2023, 2, 29), // 2023 is not a leap year
        (2023, 2, 30),
        (2024, 2, 30), // even a leap year has no 30th of February
        (1900, 2, 29), // century year that is not divisible by 400
        (2024, 4, 31),
        (2024, 6, 31),
        (2024, 9, 31),
        (2024, 11, 31),
    ];
    let accepted: Vec<String> = impossible
        .into_iter()
        .filter_map(
            |(year, month, day)| match date_from_parts(year, month, day) {
                Ok(Value::Int(timestamp)) => {
                    let (ry, rm, rd) = ymd(timestamp);
                    Some(format!(
                    "{year}-{month:02}-{day:02} accepted and silently became {ry}-{rm:02}-{rd:02}"
                ))
                }
                Ok(other) => Some(format!("{year}-{month:02}-{day:02} produced {other:?}")),
                Err(_) => None,
            },
        )
        .collect();
    assert!(
        accepted.is_empty(),
        "date_from_parts accepted dates that do not exist: {accepted:#?}"
    );
}

#[test]
fn date_get_part_rejects_an_unknown_part_name() {
    let error = eval(&intrinsic(
        IntrinsicOp::DateGetPart,
        vec![lit_int(0), lit_text("millisecond")],
    ))
    .expect_err("an unsupported part must not silently return a number");
    assert!(
        matches!(&error, SpoonError::Other(message) if message.contains("unknown part")),
        "expected a descriptive error, got {error:?}"
    );
}

// ---------------------------------------------------------------------------
// DateAdd
// ---------------------------------------------------------------------------

#[test]
fn date_add_crosses_month_year_and_leap_day_boundaries() {
    let cases = [
        ((2024, 1, 31), 1, "days", (2024, 2, 1)),
        ((2023, 12, 31), 1, "days", (2024, 1, 1)),
        ((2024, 2, 28), 1, "days", (2024, 2, 29)),
        ((2023, 2, 28), 1, "days", (2023, 3, 1)),
        ((2024, 2, 29), 1, "days", (2024, 3, 1)),
        ((2024, 12, 31), 1, "days", (2025, 1, 1)),
    ];
    for ((year, month, day), amount, unit, expected) in cases {
        let start = date(year, month, day);
        let shifted = match date_add(start, amount, unit) {
            Ok(Value::Int(ts)) => ts,
            other => panic!("date_add returned {other:?}"),
        };
        assert_eq!(
            ymd(shifted),
            expected,
            "{year}-{month:02}-{day:02} plus {amount} {unit}"
        );
    }
}

#[test]
fn date_add_with_a_negative_amount_moves_backwards_across_boundaries() {
    let cases = [
        ((2024, 3, 1), -1, "days", (2024, 2, 29)),
        ((2023, 3, 1), -1, "days", (2023, 2, 28)),
        ((2024, 1, 1), -1, "days", (2023, 12, 31)),
        ((1970, 1, 1), -1, "days", (1969, 12, 31)),
    ];
    for ((year, month, day), amount, unit, expected) in cases {
        let start = date(year, month, day);
        let shifted = match date_add(start, amount, unit) {
            Ok(Value::Int(ts)) => ts,
            other => panic!("date_add returned {other:?}"),
        };
        assert_eq!(
            ymd(shifted),
            expected,
            "{year}-{month:02}-{day:02} minus {} {unit}",
            -amount
        );
    }
}

#[test]
fn date_add_scales_each_supported_unit_to_seconds() {
    let base = date(2024, 3, 15);
    assert_eq!(
        date_add(base, 90, "seconds").unwrap(),
        Value::Int(base + 90)
    );
    assert_eq!(
        date_add(base, 90, "minutes").unwrap(),
        Value::Int(base + 90 * 60)
    );
    assert_eq!(
        date_add(base, 5, "hours").unwrap(),
        Value::Int(base + 5 * 3600)
    );
    assert_eq!(
        date_add(base, 3, "days").unwrap(),
        Value::Int(base + 3 * 86400)
    );
    assert_eq!(date_add(base, 0, "days").unwrap(), Value::Int(base));
}

#[test]
fn date_add_rejects_calendar_units_it_does_not_implement() {
    // Only fixed-length units are supported. Months and years are variable
    // length, so accepting them would require calendar arithmetic that is not
    // there; the error is what keeps a procedure from getting a wrong answer.
    for unit in ["months", "years", "weeks", "Days", ""] {
        let error = date_add(date(2024, 1, 31), 1, unit)
            .expect_err("an unimplemented unit must not be silently approximated");
        assert!(
            matches!(&error, SpoonError::Other(message) if message.contains("unknown unit")),
            "expected a descriptive error for unit {unit:?}, got {error:?}"
        );
    }
}

#[test]
fn date_add_reports_overflow_instead_of_wrapping() {
    assert!(matches!(
        date_add(i64::MAX, 1, "days"),
        Err(SpoonError::ArithmeticOverflow { .. })
    ));
    assert!(matches!(
        date_add(0, i64::MAX, "days"),
        Err(SpoonError::ArithmeticOverflow { .. })
    ));
    assert!(matches!(
        date_add(i64::MIN, -1, "seconds"),
        Err(SpoonError::ArithmeticOverflow { .. })
    ));
}

// ---------------------------------------------------------------------------
// DateDiff
// ---------------------------------------------------------------------------

#[test]
fn date_diff_measures_the_second_argument_minus_the_first() {
    let earlier = date(2024, 3, 1);
    let later = date(2024, 3, 11);
    assert_eq!(date_diff(earlier, later, "days").unwrap(), Value::Int(10));
    assert_eq!(date_diff(later, earlier, "days").unwrap(), Value::Int(-10));
}

#[test]
fn date_diff_of_a_timestamp_with_itself_is_zero_in_every_unit() {
    let timestamp = date(2024, 3, 15);
    for unit in ["seconds", "minutes", "hours", "days"] {
        assert_eq!(
            date_diff(timestamp, timestamp, unit).unwrap(),
            Value::Int(0),
            "a zero difference in {unit}"
        );
    }
}

#[test]
fn date_diff_reports_whole_units_truncated_toward_zero() {
    let base = date(2024, 3, 15);
    // A day and a quarter is one whole day in each direction: integer
    // division truncates rather than rounding or flooring.
    assert_eq!(
        date_diff(base, base + 108_000, "days").unwrap(),
        Value::Int(1)
    );
    assert_eq!(
        date_diff(base, base - 108_000, "days").unwrap(),
        Value::Int(-1)
    );
    assert_eq!(
        date_diff(base, base + 59, "minutes").unwrap(),
        Value::Int(0)
    );
    assert_eq!(
        date_diff(base, base + 3_599, "hours").unwrap(),
        Value::Int(0)
    );
    assert_eq!(
        date_diff(base, base + 3_600, "hours").unwrap(),
        Value::Int(1)
    );
}

#[test]
fn date_diff_counts_calendar_spans_that_include_a_leap_day() {
    assert_eq!(
        date_diff(date(2024, 1, 1), date(2025, 1, 1), "days").unwrap(),
        Value::Int(366)
    );
    assert_eq!(
        date_diff(date(2023, 1, 1), date(2024, 1, 1), "days").unwrap(),
        Value::Int(365)
    );
}

#[test]
fn date_diff_rejects_units_it_does_not_implement() {
    for unit in ["months", "years", "weeks", ""] {
        let error = date_diff(0, 86_400, unit).expect_err("an unimplemented unit must error");
        assert!(
            matches!(&error, SpoonError::Other(message) if message.contains("unknown unit")),
            "expected a descriptive error for unit {unit:?}, got {error:?}"
        );
    }
}

#[test]
fn date_diff_reports_overflow_instead_of_wrapping() {
    assert!(matches!(
        date_diff(i64::MIN, i64::MAX, "seconds"),
        Err(SpoonError::ArithmeticOverflow { .. })
    ));
}

// ---------------------------------------------------------------------------
// DateFormat
// ---------------------------------------------------------------------------

#[test]
fn date_format_renders_every_supported_token_zero_padded() {
    let timestamp = date(2024, 3, 5) + 7 * 3600 + 8 * 60 + 9;
    assert_eq!(date_format(timestamp, "%Y"), "2024");
    assert_eq!(date_format(timestamp, "%m"), "03");
    assert_eq!(date_format(timestamp, "%d"), "05");
    assert_eq!(date_format(timestamp, "%H"), "07");
    assert_eq!(date_format(timestamp, "%M"), "08");
    assert_eq!(date_format(timestamp, "%S"), "09");
    assert_eq!(
        date_format(timestamp, "%Y-%m-%dT%H:%M:%S"),
        "2024-03-05T07:08:09"
    );
}

#[test]
fn date_format_escapes_a_doubled_percent_and_copies_literal_text() {
    assert_eq!(date_format(0, "%%"), "%");
    assert_eq!(date_format(0, "%%Y"), "%Y");
    assert_eq!(
        date_format(0, "on %Y-%m-%d at 100%%"),
        "on 1970-01-01 at 100%"
    );
    assert_eq!(date_format(0, "no tokens here"), "no tokens here");
}

#[test]
fn date_format_passes_an_unsupported_token_through_unchanged() {
    // The implementation deliberately echoes an unknown token rather than
    // failing, so a format string with a typo produces visible output instead
    // of an aborted procedure.
    assert_eq!(date_format(0, "%q"), "%q");
    assert_eq!(date_format(0, "%Z-%Y"), "%Z-1970");
    // A trailing percent has no token after it and is copied verbatim.
    assert_eq!(date_format(0, "%Y%"), "1970%");
    assert_eq!(date_format(0, "%"), "%");
}

#[test]
fn date_format_of_an_empty_format_is_an_empty_string() {
    assert_eq!(date_format(0, ""), "");
    assert_eq!(date_format(1_709_164_800, ""), "");
}

#[test]
fn date_format_agrees_with_date_get_part_on_the_same_timestamp() {
    let timestamp = date(1969, 12, 31) + 23 * 3600 + 59 * 60 + 58;
    let (year, month, day) = ymd(timestamp);
    assert_eq!(
        date_format(timestamp, "%Y-%m-%d"),
        format!("{year:04}-{month:02}-{day:02}")
    );
    assert_eq!(
        date_format(timestamp, "%Y-%m-%d %H:%M:%S"),
        "1969-12-31 23:59:58"
    );
}

// ---------------------------------------------------------------------------
// Arity and argument types
// ---------------------------------------------------------------------------

#[test]
fn the_random_and_date_intrinsics_reject_the_wrong_number_of_arguments() {
    let cases: Vec<(IntrinsicOp, Vec<Expr>)> = vec![
        (IntrinsicOp::RandomInt, vec![lit_int(1)]),
        (
            IntrinsicOp::RandomInt,
            vec![lit_int(1), lit_int(2), lit_int(3)],
        ),
        (IntrinsicOp::RandomFloat, vec![lit_int(1)]),
        (IntrinsicOp::RandomChoice, vec![]),
        (
            IntrinsicOp::RandomShuffle,
            vec![lit_list(Vec::new()), lit_int(1)],
        ),
        (IntrinsicOp::RandomSample, vec![lit_list(Vec::new())]),
        (IntrinsicOp::RandomUuid, vec![lit_int(1)]),
        (IntrinsicOp::DateNow, vec![lit_int(1)]),
        (IntrinsicOp::DateFromParts, vec![lit_int(2024), lit_int(1)]),
        (IntrinsicOp::DateGetPart, vec![lit_int(0)]),
        (
            IntrinsicOp::DateAdd,
            vec![lit_int(0), lit_int(1), lit_text("days"), lit_int(2)],
        ),
        (IntrinsicOp::DateDiff, vec![lit_int(0), lit_int(1)]),
        (
            IntrinsicOp::DateFormat,
            vec![lit_int(0), lit_text("%Y"), lit_int(1)],
        ),
    ];
    for (op, args) in cases {
        let count = args.len();
        assert!(
            matches!(
                eval(&intrinsic(op, args)),
                Err(SpoonError::ArityMismatch { .. })
            ),
            "{op:?} accepted {count} argument(s)"
        );
    }
}

#[test]
fn the_random_and_date_intrinsics_reject_arguments_of_the_wrong_type() {
    let cases: Vec<(IntrinsicOp, Vec<Expr>)> = vec![
        (IntrinsicOp::RandomInt, vec![lit_text("1"), lit_int(2)]),
        (
            IntrinsicOp::RandomInt,
            vec![lit_int(1), Expr::Literal(Value::Float(2.0))],
        ),
        (IntrinsicOp::RandomChoice, vec![lit_text("abc")]),
        (IntrinsicOp::RandomShuffle, vec![lit_int(3)]),
        (
            IntrinsicOp::RandomSample,
            vec![lit_list(Vec::new()), lit_text("1")],
        ),
        (
            IntrinsicOp::DateFromParts,
            vec![lit_text("2024"), lit_int(1), lit_int(1)],
        ),
        (IntrinsicOp::DateGetPart, vec![lit_int(0), lit_int(1)]),
        (
            IntrinsicOp::DateGetPart,
            vec![lit_text("0"), lit_text("year")],
        ),
        (
            IntrinsicOp::DateAdd,
            vec![lit_int(0), lit_int(1), Expr::Literal(Value::Null)],
        ),
        (
            IntrinsicOp::DateDiff,
            vec![
                lit_int(0),
                Expr::Literal(Value::Bool(true)),
                lit_text("days"),
            ],
        ),
        (IntrinsicOp::DateFormat, vec![lit_int(0), lit_int(1)]),
    ];
    for (op, args) in cases {
        assert!(
            matches!(
                eval(&intrinsic(op, args)),
                Err(SpoonError::TypeError { .. })
            ),
            "{op:?} accepted an argument of the wrong type"
        );
    }
}
