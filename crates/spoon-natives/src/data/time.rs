//! The clock, and arithmetic over it.
//!
//! # Units
//!
//! Every native here that takes or returns a duration accepts an optional unit
//! as its last argument, and defaults to seconds. One vocabulary, one default,
//! so `AddDuration<Now, 3, "days">` and `TimeDiff<a, b, "hours">` read the
//! same way and nothing has to be remembered.
//!
//! # The clock is injected
//!
//! [`now`] reads `ctx.now()` and never `Utc::now()`. The context clock exists
//! so a test can pin it and get the same answer twice; a native that reaches
//! past it for the system clock quietly makes every evaluation above it
//! irreproducible. That is also why `now` is `Effect::Read` rather than
//! `Effect::Pure`: it observes something outside the computation, and caching
//! its result would be wrong.

use std::fmt::Write as _;

use chrono::format::{Item, StrftimeItems};
use chrono::{DateTime, NaiveDateTime, TimeDelta, Utc};
use spoon_concept::{Concept, Effect, Ground};
use spoon_eval::{
    ArgStrategy, Arity, Ctx, EvalError, EvalResult, NativeRegistry, native_error, type_error,
};

use super::{want_int, want_text};

const MILLIS_PER_SECOND: i64 = 1_000;

/// The current time, from the context clock.
fn now(ctx: &mut dyn Ctx, _args: &[Concept]) -> EvalResult {
    Ok(Concept::datetime(ctx.now()))
}

/// A time as a count since the Unix epoch. Seconds unless told otherwise.
///
/// Division floors rather than truncating toward zero, so a pre-epoch instant
/// lands on the second that contains it instead of the one after it.
fn timestamp(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let at = want_time("time-timestamp", &args[0])?;
    let unit = want_unit("time-timestamp", args.get(1))?;
    Ok(Concept::int(at.timestamp_millis().div_euclid(unit)))
}

/// A time as text. RFC 3339 unless given a strftime pattern.
///
/// The pattern is validated before use because chrono's formatter reports a
/// bad specifier through `Display`, and `Display` that fails inside
/// `to_string` panics. Checking the items first and writing through
/// `fmt::Write` keeps a malformed pattern an ordinary error.
fn format_time(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let at = want_time("time-format", &args[0])?;
    let Some(pattern_arg) = args.get(1) else {
        return Ok(Concept::text(at.to_rfc3339()));
    };
    let pattern = want_text("time-format", pattern_arg)?;
    if StrftimeItems::new(pattern).any(|item| matches!(item, Item::Error)) {
        return Err(native_error(
            "time-format",
            format!("{pattern:?} is not a valid strftime pattern"),
        ));
    }
    let mut out = String::new();
    write!(out, "{}", at.format_with_items(StrftimeItems::new(pattern)))
        .map_err(|_| native_error("time-format", format!("could not format with {pattern:?}")))?;
    Ok(Concept::text(out))
}

/// Text to a time. RFC 3339 unless given a strftime pattern.
///
/// With a pattern, an offset in the text is honoured and text without one is
/// read as UTC. Assuming local time instead would make the same document parse
/// differently on two machines.
fn parse_time(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = want_text("time-parse", &args[0])?;
    let Some(pattern_arg) = args.get(1) else {
        return DateTime::parse_from_rfc3339(text)
            .map(|dt| Concept::datetime(dt.with_timezone(&Utc)))
            .map_err(|err| native_error("time-parse", format!("{text:?} is not RFC 3339: {err}")));
    };
    let pattern = want_text("time-parse", pattern_arg)?;
    if let Ok(dt) = DateTime::parse_from_str(text, pattern) {
        return Ok(Concept::datetime(dt.with_timezone(&Utc)));
    }
    NaiveDateTime::parse_from_str(text, pattern)
        .map(|naive| Concept::datetime(naive.and_utc()))
        .map_err(|err| {
            native_error(
                "time-parse",
                format!("{text:?} does not match {pattern:?}: {err}"),
            )
        })
}

/// Move a time forward, or backward when the amount is negative.
///
/// Every step is checked. A duration that overflows is an error rather than a
/// wrapped date decades from where the caller meant.
fn add_duration(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let at = want_time("time-add-duration", &args[0])?;
    let amount = want_int("time-add-duration", &args[1])?;
    let unit = want_unit("time-add-duration", args.get(2))?;
    let overflow = || native_error("time-add-duration", "the resulting time is out of range");
    let millis = amount.checked_mul(unit).ok_or_else(overflow)?;
    let delta = TimeDelta::try_milliseconds(millis).ok_or_else(overflow)?;
    at.checked_add_signed(delta)
        .map(Concept::datetime)
        .ok_or_else(overflow)
}

/// How long from the second time to the first, negative when the first is
/// earlier. Seconds unless told otherwise.
fn time_diff(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let left = want_time("time-diff", &args[0])?;
    let right = want_time("time-diff", &args[1])?;
    let unit = want_unit("time-diff", args.get(2))?;
    let millis = left.signed_duration_since(right).num_milliseconds();
    Ok(Concept::int(millis.div_euclid(unit)))
}

/// Whether the first time is strictly earlier than the second.
fn before(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::bool(
        want_time("time-before", &args[0])? < want_time("time-before", &args[1])?,
    ))
}

/// Whether the first time is strictly later than the second.
fn after(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::bool(
        want_time("time-after", &args[0])? > want_time("time-after", &args[1])?,
    ))
}

// ---- argument helpers ----

fn want_time(native: &str, c: &Concept) -> Result<DateTime<Utc>, EvalError> {
    match c.as_ground() {
        Some(Ground::DateTime(dt)) => Ok(*dt),
        _ => Err(type_error(native, "a time", c)),
    }
}

/// A unit name to its size in milliseconds. Absent means seconds.
fn want_unit(native: &str, arg: Option<&Concept>) -> Result<i64, EvalError> {
    let Some(arg) = arg else {
        return Ok(MILLIS_PER_SECOND);
    };
    let name = want_text(native, arg)?;
    match name.trim().to_lowercase().as_str() {
        "millisecond" | "milliseconds" | "ms" => Ok(1),
        "second" | "seconds" | "s" => Ok(MILLIS_PER_SECOND),
        "minute" | "minutes" | "math-min" => Ok(60 * MILLIS_PER_SECOND),
        "hour" | "hours" | "h" => Ok(3_600 * MILLIS_PER_SECOND),
        "day" | "days" | "d" => Ok(86_400 * MILLIS_PER_SECOND),
        "week" | "weeks" | "w" => Ok(604_800 * MILLIS_PER_SECOND),
        other => Err(native_error(
            native,
            format!(
                "unknown unit {other:?}: use milliseconds, seconds, minutes, hours, days, or weeks"
            ),
        )),
    }
}

pub fn register(registry: &mut NativeRegistry) {
    registry.register(
        "time-now",
        now,
        Arity::Exact(0),
        ArgStrategy::Eager,
        Effect::Read,
        "the current time, from the context clock",
    );
    registry.pure(
        "time-timestamp",
        timestamp,
        Arity::Between(1, 2),
        "a time as a count since the Unix epoch, in seconds unless a unit is given",
    );
    registry.pure(
        "time-format",
        format_time,
        Arity::Between(1, 2),
        "a time as text, RFC 3339 unless given a strftime pattern",
    );
    registry.pure(
        "time-parse",
        parse_time,
        Arity::Between(1, 2),
        "text to a time, RFC 3339 unless given a strftime pattern",
    );
    registry.pure(
        "time-add-duration",
        add_duration,
        Arity::Between(2, 3),
        "move a time by an amount, in seconds unless a unit is given",
    );
    registry.pure(
        "time-diff",
        time_diff,
        Arity::Between(2, 3),
        "how long from the second time to the first, in seconds unless a unit is given",
    );
    registry.pure(
        "time-before",
        before,
        Arity::Exact(2),
        "whether one time is earlier than another",
    );
    registry.pure(
        "time-after",
        after,
        Arity::Exact(2),
        "whether one time is later than another",
    );
}
