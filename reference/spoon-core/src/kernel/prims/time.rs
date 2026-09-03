//! Date/time primitives (time.*).
//! Effect::Read for now/today (they observe the clock); Pure otherwise.

use chrono::{Datelike, Timelike, TimeZone, Utc};

use crate::types::*;

use super::super::{Ctx, EvalError, Kernel};

pub fn register(k: &mut Kernel) {
    use Effect::{Pure, Read};

    macro_rules! p {
        ($id:expr, $verbs:expr, $inputs:expr, $out:expr, $eff:expr, $desc:expr, $f:expr) => {
            k.register(Action::primitive($id, $verbs, $inputs, $out, $eff, $desc), $f)
        };
    }

    let dt = || Type::DateTime;
    let dur = || Type::Duration;
    let t = || Type::Text;
    let i = || Type::Int;
    let f = || Type::Float;

    p!("time.now",             &["now"],            vec![], dt(),  Read, "current date and time (UTC)", time_now);
    p!("time.today",           &["today"],          vec![], dt(),  Read, "current date as start-of-day UTC", time_today);
    p!("time.parse_datetime",  &["parse-datetime"], vec![Input::required("text", t())], dt(), Pure, "parse datetime from RFC3339 or common formats", time_parse);
    p!("time.format",          &["format"],         vec![Input::required("dt", dt()), Input::required("fmt", t())], t(), Pure, "format datetime using strftime pattern", time_format);
    p!("time.add",             &["add"],            vec![Input::required("dt", dt()), Input::required("dur", dur())], dt(), Pure, "add duration to datetime", time_add);
    p!("time.diff",            &["diff"],           vec![Input::required("a", dt()), Input::required("b", dt())], dur(), Pure, "difference (a - b) as a duration", time_diff);
    p!("time.year",            &["year"],           vec![Input::required("dt", dt())], i(), Pure, "year component of a datetime", time_year);
    p!("time.month",           &["month"],          vec![Input::required("dt", dt())], i(), Pure, "month component (1-12)", time_month);
    p!("time.day",             &["day"],            vec![Input::required("dt", dt())], i(), Pure, "day of month (1-31)", time_day);
    p!("time.hour",            &["hour"],           vec![Input::required("dt", dt())], i(), Pure, "hour component (0-23)", time_hour);
    p!("time.minute",          &["minute"],         vec![Input::required("dt", dt())], i(), Pure, "minute component (0-59)", time_minute);
    p!("time.weekday",         &["weekday"],        vec![Input::required("dt", dt())], t(), Pure, "day of week as text (Monday, etc.)", time_weekday);
    p!("time.duration_seconds",&["duration-seconds"], vec![Input::required("n", f())], dur(), Pure, "create a duration from seconds", dur_seconds);
    p!("time.duration_minutes",&["duration-minutes"], vec![Input::required("n", f())], dur(), Pure, "create a duration from minutes", dur_minutes);
    p!("time.duration_hours",  &["duration-hours"],   vec![Input::required("n", f())], dur(), Pure, "create a duration from hours",   dur_hours);
    p!("time.duration_days",   &["duration-days"],    vec![Input::required("n", f())], dur(), Pure, "create a duration from days",    dur_days);
    p!("time.duration_to_seconds", &["duration-to-seconds"], vec![Input::required("dur", dur())], f(), Pure, "extract seconds from a duration as float", dur_to_seconds);
    p!("time.in_days",         &["in-days"],        vec![Input::required("dur", dur())], f(), Pure, "convert a duration to fractional days", in_days);
}

// ---- helpers -----------------------------------------------------------

fn ms_arg(id: &str, args: &[Value], i: usize) -> Result<i64, EvalError> {
    match args.get(i) {
        Some(Value::DateTime(ms)) => Ok(*ms),
        other => Err(EvalError::ty("datetime", other.unwrap_or(&Value::Null), id)),
    }
}

fn dur_arg(id: &str, args: &[Value], i: usize) -> Result<f64, EvalError> {
    match args.get(i) {
        Some(Value::Duration(s)) => Ok(*s),
        other => Err(EvalError::ty("duration", other.unwrap_or(&Value::Null), id)),
    }
}

fn float_arg(id: &str, args: &[Value], i: usize) -> Result<f64, EvalError> {
    args.get(i).and_then(Value::as_f64)
        .ok_or_else(|| EvalError::ty("float", args.get(i).unwrap_or(&Value::Null), id))
}

fn ms_to_dt(ms: i64) -> Option<chrono::DateTime<Utc>> {
    Utc.timestamp_millis_opt(ms).single()
}

// ---- primitives --------------------------------------------------------

fn time_now(_ctx: &mut Ctx<'_>, _args: &[Value]) -> Result<Value, EvalError> {
    Ok(Value::DateTime(Utc::now().timestamp_millis()))
}

fn time_today(_ctx: &mut Ctx<'_>, _args: &[Value]) -> Result<Value, EvalError> {
    let now = Utc::now();
    let start = now.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|dt| Utc.from_local_datetime(&dt).single());
    let ms = start.map(|d| d.timestamp_millis()).unwrap_or_else(|| now.timestamp_millis());
    Ok(Value::DateTime(ms))
}

fn time_parse(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("time.parse_datetime".into());
    let s = args.first().and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("text", args.first().unwrap_or(&Value::Null), "time.parse_datetime"))?
        .trim();

    // Try RFC3339 first, then a few common formats.
    if let Ok(dt) = s.parse::<chrono::DateTime<Utc>>() {
        return Ok(Value::DateTime(dt.timestamp_millis()));
    }
    // chrono's NaiveDateTime with common formats.
    let formats = [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d",
        "%m/%d/%Y",
        "%d %B %Y",
    ];
    for fmt in &formats {
        if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            let dt = Utc.from_utc_datetime(&ndt);
            return Ok(Value::DateTime(dt.timestamp_millis()));
        }
        if let Ok(nd) = chrono::NaiveDate::parse_from_str(s, fmt) {
            if let Some(ndt) = nd.and_hms_opt(0, 0, 0) {
                let dt = Utc.from_utc_datetime(&ndt);
                return Ok(Value::DateTime(dt.timestamp_millis()));
            }
        }
    }
    Err(EvalError::runtime(&id, format!("cannot parse datetime: {s}")))
}

fn time_format(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("time.format".into());
    let ms = ms_arg("time.format", args, 0)?;
    let fmt = args.get(1).and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("text", args.get(1).unwrap_or(&Value::Null), "time.format"))?;
    let dt = ms_to_dt(ms).ok_or_else(|| EvalError::runtime(&id, "invalid datetime"))?;
    Ok(Value::Text(dt.format(fmt).to_string()))
}

fn time_add(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let ms = ms_arg("time.add", args, 0)?;
    let secs = dur_arg("time.add", args, 1)?;
    Ok(Value::DateTime(ms + (secs * 1000.0) as i64))
}

fn time_diff(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let a = ms_arg("time.diff", args, 0)?;
    let b = ms_arg("time.diff", args, 1)?;
    Ok(Value::Duration((a - b) as f64 / 1000.0))
}

fn time_year(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("time.year".into());
    let ms = ms_arg("time.year", args, 0)?;
    Ok(Value::Int(ms_to_dt(ms).ok_or_else(|| EvalError::runtime(&id, "invalid datetime"))?.year() as i64))
}
fn time_month(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("time.month".into());
    let ms = ms_arg("time.month", args, 0)?;
    Ok(Value::Int(ms_to_dt(ms).ok_or_else(|| EvalError::runtime(&id, "invalid datetime"))?.month() as i64))
}
fn time_day(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("time.day".into());
    let ms = ms_arg("time.day", args, 0)?;
    Ok(Value::Int(ms_to_dt(ms).ok_or_else(|| EvalError::runtime(&id, "invalid datetime"))?.day() as i64))
}
fn time_hour(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("time.hour".into());
    let ms = ms_arg("time.hour", args, 0)?;
    Ok(Value::Int(ms_to_dt(ms).ok_or_else(|| EvalError::runtime(&id, "invalid datetime"))?.hour() as i64))
}
fn time_minute(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("time.minute".into());
    let ms = ms_arg("time.minute", args, 0)?;
    Ok(Value::Int(ms_to_dt(ms).ok_or_else(|| EvalError::runtime(&id, "invalid datetime"))?.minute() as i64))
}
fn time_weekday(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    use chrono::Weekday;
    let id = ActionId("time.weekday".into());
    let ms = ms_arg("time.weekday", args, 0)?;
    let day = ms_to_dt(ms).ok_or_else(|| EvalError::runtime(&id, "invalid datetime"))?.weekday();
    let name = match day {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    };
    Ok(Value::Text(name.into()))
}

fn dur_seconds(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    Ok(Value::Duration(float_arg("time.duration_seconds", args, 0)?))
}
fn dur_minutes(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    Ok(Value::Duration(float_arg("time.duration_minutes", args, 0)? * 60.0))
}
fn dur_hours(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    Ok(Value::Duration(float_arg("time.duration_hours", args, 0)? * 3600.0))
}
fn dur_days(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    Ok(Value::Duration(float_arg("time.duration_days", args, 0)? * 86400.0))
}
fn dur_to_seconds(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    Ok(Value::Float(dur_arg("time.duration_to_seconds", args, 0)?))
}
fn in_days(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    Ok(Value::Float(dur_arg("time.in_days", args, 0)? / 86400.0))
}
